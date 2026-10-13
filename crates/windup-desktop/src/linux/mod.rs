//! Everything Linux-specific: choosing the backend, the socket, the Wayland
//! service, X11 focus, the portals, and the background process that runs
//! on Wayland, where Windup keeps no window open. See docs/linux.md.

pub mod autostart;
pub mod cosmic;
pub mod ipc;
pub mod portal;
pub mod wayland;
pub mod window;
pub mod x11;

use crate::platform::{self, Env, HotkeysVia, PasteVia, Plan, Target};
use std::process::Child;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use wayland::{PopupSetup, Service, ServiceEvent, Shared};
use windup_core::clipboard::{ClipboardEntry, ClipboardHistory, ClipboardMonitor};
use windup_core::ipc::Command;
use windup_core::{Config, ToolId};
use windup_ui::PopupAction;

/// The backend chosen at startup, and the Wayland service when it runs.
pub struct Runtime {
    pub plan: Plan,
    pub service: Option<Service>,
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

pub fn runtime() -> Option<&'static Runtime> {
    RUNTIME.get()
}

/// Asks the compositor what it offers and picks a backend from the answer.
pub fn detect() -> Plan {
    let env = Env::from_process();
    let globals = env.wayland_display.as_ref().and_then(|_| wayland::probe());
    let plan = platform::detect(&env, globals.as_deref());
    log::info!("backend: {plan:?}");
    plan
}

/// Starts the Wayland service when `plan` needs one, and records the
/// runtime. A service that fails to start degrades the plan to what works
/// without it.
pub fn start(mut plan: Plan, shared: Shared) -> &'static Runtime {
    let mut service = None;
    if plan.needs_wayland() {
        match Service::spawn(plan, shared) {
            Ok(s) => service = Some(s),
            Err(e) => {
                log::error!("Wayland service did not start ({e}); falling back");
                if plan.host == platform::Host::Layer {
                    plan.host = platform::Host::Window;
                }
                plan.watch = platform::Watch::Poll;
                if plan.paste == PasteVia::VirtualKeyboard {
                    plan.paste = PasteVia::Portal;
                }
            }
        }
    }
    RUNTIME.get_or_init(|| Runtime { plan, service })
}

/// The window a paste should go back to, captured when the hotkey fires.
pub fn current_target(plan: &Plan) -> Target {
    if plan.hotkeys == HotkeysVia::XGrab || plan.paste == PasteVia::XTest {
        x11::active_window().map_or(Target::None, Target::X11)
    } else {
        Target::None
    }
}

/// Puts `entry` on the clipboard through the Wayland service. False when
/// there is none and the caller should use `clipboard-rs`.
pub fn set_entry(entry: &ClipboardEntry, paste: bool) -> bool {
    match runtime().and_then(|r| r.service.as_ref()) {
        Some(service) if r_has_data_control() => {
            service.set_entry(entry, paste);
            true
        }
        _ => false,
    }
}

pub fn set_text(text: &str, paste: bool) -> bool {
    match runtime().and_then(|r| r.service.as_ref()) {
        Some(service) if r_has_data_control() => {
            service.set_text(text.to_owned(), paste);
            true
        }
        _ => false,
    }
}

pub fn paste_plain_current() -> bool {
    match runtime().and_then(|r| r.service.as_ref()) {
        Some(service) if r_has_data_control() => {
            service.paste_plain_current();
            true
        }
        _ => false,
    }
}

fn r_has_data_control() -> bool {
    runtime().is_some_and(|r| matches!(r.plan.watch, platform::Watch::DataControl(_)))
}

/// Light or dark, for "Follow system" when there is no winit window to ask.
/// COSMIC keeps it in a one-word file; GNOME and most others answer through
/// gsettings. Unknown means dark, Windup's default.
pub fn system_prefers_dark() -> bool {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".config")))
        && let Ok(s) =
            std::fs::read_to_string(dir.join("cosmic/com.system76.CosmicTheme.Mode/v1/is_dark"))
    {
        return s.trim() != "false";
    }
    if let Ok(out) = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
        && out.status.success()
    {
        return !String::from_utf8_lossy(&out.stdout).contains("light");
    }
    true
}

// --- the background process on Wayland ---------------------------------------

/// Runs Windup with no window of its own, on Wayland: the socket and the
/// clipboard are watched here, the settings window is a separate
/// `windup --settings` process started on demand, and the popup is either a
/// layer surface on the Wayland thread or, without layer-shell, a window of
/// its own for as long as it is open. Returns only if the plan turns out to
/// need the eframe app after all.
pub fn run_background(
    plan: Plan,
    listener: std::os::unix::net::UnixListener,
    show_settings: bool,
) -> Result<(), std::os::unix::net::UnixListener> {
    let mut config = Config::load();
    let history = Arc::new(Mutex::new(
        Config::history_path()
            .map(|p| ClipboardHistory::load_from_disk(&p))
            .unwrap_or_default(),
    ));
    let (events_tx, events) = channel();
    let shared = Shared {
        history: Arc::clone(&history),
        max_items: Arc::new(AtomicUsize::new(config.clipboard.max_items)),
        save_images: Arc::new(AtomicBool::new(config.clipboard.save_images)),
        watching: Arc::new(AtomicBool::new(config.clipboard.enabled)),
        events: events_tx,
    };
    let runtime = start(plan, shared.clone());
    if runtime.plan.host == platform::Host::Viewport {
        return Err(listener);
    }

    let (cmd_tx, commands) = channel();
    ipc::serve(listener, move |c| {
        let _ = cmd_tx.send(c);
    });
    cosmic::bind_once();
    portal::start_shortcuts(&runtime.plan);

    let mut host = Background {
        runtime,
        monitor: (runtime.plan.watch == platform::Watch::Poll)
            .then(|| ClipboardMonitor::new(Arc::clone(&history))),
        watching: false,
        settings: None,
        config_stamp: config_stamp(),
        saved: history_state(&history),
    };
    if show_settings {
        host.show_settings();
    }
    autostart::sync(config.launch_at_login);

    loop {
        host.tick(&mut config, &shared, &commands, &events);
    }
}

struct Background {
    runtime: &'static Runtime,
    /// The polling monitor, where there is no data-control to listen to.
    monitor: Option<ClipboardMonitor>,
    watching: bool,
    settings: Option<Child>,
    config_stamp: Option<SystemTime>,
    saved: (usize, u64),
}

impl Background {
    fn tick(
        &mut self,
        config: &mut Config,
        shared: &Shared,
        commands: &Receiver<Command>,
        events: &Receiver<ServiceEvent>,
    ) {
        if let Ok(command) = commands.recv_timeout(Duration::from_millis(200)) {
            self.command(command, config, shared, commands);
        }
        while let Ok(event) = events.try_recv() {
            match event {
                ServiceEvent::PopupResized(w, h) => self.save_popup_size(config, w, h),
                ServiceEvent::PasteElsewhere => portal::paste_ctrl_v(),
            }
        }

        // The settings window saves config.json as it changes; pick it up.
        let stamp = config_stamp();
        if stamp != self.config_stamp {
            self.config_stamp = stamp;
            let before = config.launch_at_login;
            *config = Config::load();
            if config.launch_at_login != before {
                autostart::sync(config.launch_at_login);
            }
        }
        shared
            .max_items
            .store(config.clipboard.max_items, Ordering::Relaxed);
        shared
            .save_images
            .store(config.clipboard.save_images, Ordering::Relaxed);
        shared
            .watching
            .store(config.clipboard.enabled, Ordering::Relaxed);
        if let Some(monitor) = &self.monitor
            && self.watching != config.clipboard.enabled
        {
            self.watching = config.clipboard.enabled;
            if self.watching {
                monitor.start(
                    Arc::clone(&shared.max_items),
                    Arc::clone(&shared.save_images),
                );
            } else {
                monitor.stop();
            }
        }

        let state = history_state(&shared.history);
        if state != self.saved {
            self.saved = state;
            if let Some(p) = Config::history_path() {
                let _ = shared.history.lock().unwrap().save_to_disk(&p);
            }
        }
    }

    fn command(
        &mut self,
        command: Command,
        config: &mut Config,
        shared: &Shared,
        commands: &Receiver<Command>,
    ) {
        match command {
            Command::Popup if config.is_tool_enabled(ToolId::ClipboardHistory) => {
                match &self.runtime.service {
                    Some(service) if self.runtime.plan.host == platform::Host::Layer => {
                        service.open_popup(popup_setup(config));
                    }
                    _ => self.popup_window(config, shared, commands),
                }
            }
            Command::PastePlain if config.is_tool_enabled(ToolId::PastePlainText) => {
                crate::paste::paste_current_clipboard_as_plain(Target::None);
            }
            Command::Show => self.show_settings(),
            Command::Reload => {
                *config = Config::load();
                self.config_stamp = config_stamp();
            }
            _ => {}
        }
    }

    /// The popup as a window of its own; returns once it has closed.
    fn popup_window(&mut self, config: &mut Config, shared: &Shared, commands: &Receiver<Command>) {
        let setup = popup_setup(config);
        let closed = window::run(&setup, &shared.history, commands);
        if let Some((w, h)) = closed.size {
            self.save_popup_size(config, w, h);
        }
        match closed.action {
            Some(PopupAction::Paste(entry)) if setup.auto_paste => {
                crate::paste::paste_entry_to_target(Target::None, &entry);
            }
            Some(PopupAction::Paste(entry)) => crate::paste::populate_clipboard(&entry),
            Some(PopupAction::PastePlain(text)) => {
                crate::paste::paste_plain_to_target(Target::None, &text);
            }
            _ => {}
        }
        for command in closed.later {
            self.command(command, config, shared, commands);
        }
    }

    fn save_popup_size(&mut self, config: &mut Config, w: f32, h: f32) {
        config.clipboard.popup_width = w;
        config.clipboard.popup_height = h;
        config.save();
        self.config_stamp = config_stamp();
    }

    fn show_settings(&mut self) {
        if let Some(child) = self.settings.as_mut()
            && matches!(child.try_wait(), Ok(None))
        {
            return; // Already open.
        }
        match std::env::current_exe()
            .and_then(|exe| std::process::Command::new(exe).arg("--settings").spawn())
        {
            Ok(child) => self.settings = Some(child),
            Err(e) => log::error!("could not open the settings window: {e}"),
        }
    }
}

pub fn popup_setup(config: &Config) -> PopupSetup {
    let mode = crate::resolve_mode(config, system_prefers_dark());
    PopupSetup {
        palette: crate::resolve_palette(config, mode),
        mode,
        size: (
            config.clipboard.popup_width.clamp(280.0, 900.0),
            config.clipboard.popup_height.clamp(260.0, 1000.0),
        ),
        auto_paste: config.clipboard.auto_paste,
    }
}

fn config_stamp() -> Option<SystemTime> {
    Config::config_file_path()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
}

fn history_state(history: &Mutex<ClipboardHistory>) -> (usize, u64) {
    let h = history.lock().unwrap();
    (h.len(), h.get(0).map(|e| e.id).unwrap_or(0))
}
