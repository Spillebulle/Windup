#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod cursor;
mod hotkey;
#[cfg(target_os = "linux")]
mod linux;
mod overlay;
mod paste;
mod platform;
mod recording;
mod review_host;
mod setup;
mod steps;
mod video;

use eframe::egui;
use hotkey::{HotkeyEvent, HotkeyListener};
use platform::{Host, Plan, Target};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use windup_core::capture::session::CaptureMode;
use windup_core::clipboard::{ClipboardHistory, ClipboardMonitor};
use windup_core::placement::{self, Area};
use windup_core::{Config, ThemeMode, ToolId};
use windup_ui::{Mode, Palette, PopupAction, PopupView, SettingsView, apply_theme, install_fonts};

/// How this process was started.
struct Startup {
    plan: Plan,
    /// Only the settings window: a child of the Linux background process,
    /// which owns the clipboard, the hotkeys and the history file.
    settings_only: bool,
    /// The single-instance socket, already bound.
    #[cfg(target_os = "linux")]
    listener: Option<std::os::unix::net::UnixListener>,
}

struct WindupApp {
    config: Config,
    settings_view: SettingsView,
    history: Arc<Mutex<ClipboardHistory>>,
    _monitor: Option<ClipboardMonitor>,
    hotkey_listener: Option<HotkeyListener>,
    applied_bindings: Vec<(windup_core::Shortcut, windup_core::hotkeys::Action)>,
    /// The recording's keys as last bound (live only while one runs).
    applied_recording: Vec<(windup_core::Shortcut, windup_core::hotkeys::Action)>,
    settings_only: bool,
    /// Whether the Video pane was open last frame, to list the sound
    /// devices again each time it opens.
    video_pane_open: bool,
    /// The Steps pane's facts and click setup, found on workers.
    steps_pane: StepsPane,
    popup_view: PopupView,
    popup_open: bool,
    popup_opened_at: Option<std::time::Instant>,
    popup_has_focused: bool,
    popup_first_frame: bool,
    popup_pos: Option<(f32, f32)>,
    popup_logical_pos: Option<[f32; 2]>,
    target: Target,
    /// Where overlays go: only the viewport host draws them here.
    host: Host,
    overlay: overlay::Overlay,
    /// A recording or Steps session, its bar and its end (Windows and X11).
    rec: recording::Rec,
    /// The review of a Steps session that stopped (Windows and X11).
    review: Option<review_host::ReviewViewport>,
    last_saved_state: (usize, u64),
    max_items_atomic: Arc<AtomicUsize>,
    save_images_atomic: Arc<AtomicBool>,
    watching_atomic: Arc<AtomicBool>,
    app_icon: Arc<egui::IconData>,
    last_icon_colors: Option<(egui::Color32, egui::Color32)>,
}

/// Windows key code of the Paste plain text shortcut, if it is bound.
fn paste_plain_vk(list: &[(windup_core::Shortcut, windup_core::hotkeys::Action)]) -> Option<u16> {
    use windup_core::hotkeys::{Action, windows_vk};
    list.iter()
        .find(|(_, a)| *a == Action::PastePlain)
        .and_then(|(s, _)| windows_vk(&s.key))
}

/// What the settings window tells about binding keys in the desktop: only
/// where the plan's hotkeys are commands. The settings-only process has no
/// plan of its own, so it asks the desktop again.
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
fn hotkey_hint(plan: Plan, settings_only: bool) -> windup_ui::HotkeyHint {
    #[cfg(target_os = "linux")]
    {
        let plan = if settings_only { linux::detect() } else { plan };
        if plan.hotkeys == platform::HotkeysVia::Command {
            return if linux::cosmic::is_cosmic() {
                windup_ui::HotkeyHint::Cosmic
            } else {
                windup_ui::HotkeyHint::Command
            };
        }
    }
    windup_ui::HotkeyHint::None
}

/// What the Video settings must say where this desktop has no way to take
/// Esc globally while a recording runs (Sway, GNOME without the
/// GlobalShortcuts portal, Weston): Esc on the recording bar where there is
/// one (Sway), nothing where there is none (GNOME, Weston). Asked of the
/// desktop as `hotkey_hint` is.
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
fn esc_caution(plan: Plan, settings_only: bool) -> windup_ui::EscCaution {
    #[cfg(target_os = "linux")]
    {
        use windup_core::capture::backend::{BarHost, record_bar_host};
        let plan = if settings_only { linux::detect() } else { plan };
        let cosmic = linux::cosmic::is_cosmic();
        // The portal is asked only where it would be the way in.
        let portal = plan.hotkeys == platform::HotkeysVia::Command
            && !cosmic
            && linux::portal::global_shortcuts_available();
        if platform::recording_keys(plan.hotkeys, cosmic, portal).global_esc() {
            return windup_ui::EscCaution::None;
        }
        let bar = record_bar_host(&capture::facts()) != BarHost::Hidden;
        return windup_ui::EscCaution::new(false, bar);
    }
    #[allow(unreachable_code)]
    windup_ui::EscCaution::None
}

impl WindupApp {
    fn new(cc: &eframe::CreationContext<'_>, startup: Startup) -> Self {
        install_fonts(&cc.egui_ctx);
        let config = Config::load();
        let initial_palette = Palette::of(Mode::Dark, config.accent_hue);
        let app_icon = Arc::new(windup_ui::art::window_icon(&initial_palette));

        // Load persisted clipboard history from disk if available
        let history_path = Config::history_path();
        let initial_history = if let Some(ref p) = history_path {
            ClipboardHistory::load_from_disk(p)
        } else {
            ClipboardHistory::new()
        };
        let initial_state = (
            initial_history.len(),
            initial_history.get(0).map(|e| e.id).unwrap_or(0),
        );

        let history = Arc::new(Mutex::new(initial_history));
        let max_items_atomic = Arc::new(AtomicUsize::new(config.clipboard.max_items));
        let save_images_atomic = Arc::new(AtomicBool::new(config.clipboard.save_images));
        let watching_atomic = Arc::new(AtomicBool::new(config.clipboard.enabled));

        #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
        let mut plan = startup.plan;
        let mut monitor = None;
        let mut hotkey_listener = None;
        if startup.settings_only {
            video::ffmpeg::follow_cache();
        } else {
            video::ffmpeg::start_probe();
            video::availability::refresh();
            #[cfg(target_os = "linux")]
            {
                let (events, _) = std::sync::mpsc::channel();
                let shared = linux::wayland::Shared {
                    history: Arc::clone(&history),
                    max_items: Arc::clone(&max_items_atomic),
                    save_images: Arc::clone(&save_images_atomic),
                    watching: Arc::clone(&watching_atomic),
                    events,
                };
                plan = linux::start(plan, shared).plan;
                linux::autostart::sync(config.launch_at_login);
            }

            if plan.watch == platform::Watch::Poll {
                let m = ClipboardMonitor::new(Arc::clone(&history));
                if config.clipboard.enabled {
                    m.start(
                        Arc::clone(&max_items_atomic),
                        Arc::clone(&save_images_atomic),
                    );
                }
                monitor = Some(m);
            }

            let listener = HotkeyListener::new(&plan, &config);
            #[cfg(target_os = "linux")]
            if let Some(socket) = startup.listener {
                let tx = listener.sender();
                let ctx = cc.egui_ctx.clone();
                linux::ipc::serve(socket, move |request| {
                    use windup_core::ipc::{Command, Request};
                    // The review is in this process here: nothing sends
                    // `EditStep` but the Wayland review child.
                    let Request::Do(command) = request else {
                        return;
                    };
                    let target = linux::current_target(&plan);
                    let event = match command {
                        Command::Popup => HotkeyEvent::ClipboardPopup { target },
                        Command::PastePlain => HotkeyEvent::PastePlainText { target },
                        Command::Capture => HotkeyEvent::Capture { target },
                        Command::Screenshot => HotkeyEvent::Screenshot { target },
                        Command::Record => HotkeyEvent::Record { target },
                        Command::RecordPause => HotkeyEvent::RecordPause,
                        Command::RecordStop => HotkeyEvent::RecordStop,
                        Command::RecordCancel => HotkeyEvent::RecordCancel,
                        Command::Steps => HotkeyEvent::Steps,
                        Command::Step => HotkeyEvent::Step,
                        Command::StepsStop => HotkeyEvent::StepsStop,
                        Command::Show => HotkeyEvent::Show,
                        Command::Reload => HotkeyEvent::Reload,
                    };
                    let _ = tx.send(event);
                    ctx.request_repaint();
                });
                linux::portal::start_shortcuts(&plan, &config);
            }
            hotkey_listener = Some(listener);
        }

        let applied_bindings = windup_core::hotkeys::bindings(&config);
        let applied_recording = session_keys(&config);
        paste::set_trigger_vk(paste_plain_vk(&applied_bindings));
        Self {
            config,
            settings_view: SettingsView {
                hotkey_hint: hotkey_hint(plan, startup.settings_only),
                esc_caution: esc_caution(plan, startup.settings_only),
                ..SettingsView::default()
            },
            history,
            _monitor: monitor,
            hotkey_listener,
            applied_bindings,
            applied_recording,
            settings_only: startup.settings_only,
            video_pane_open: false,
            steps_pane: StepsPane::default(),
            popup_view: PopupView::new(),
            popup_open: false,
            popup_opened_at: None,
            popup_has_focused: false,
            popup_first_frame: false,
            popup_pos: None,
            popup_logical_pos: None,
            target: Target::None,
            host: plan.host,
            overlay: overlay::Overlay::default(),
            rec: recording::Rec::default(),
            review: None,
            last_saved_state: initial_state,
            max_items_atomic,
            save_images_atomic,
            watching_atomic,
            app_icon,
            last_icon_colors: Some((initial_palette.accent, initial_palette.accent_ink)),
        }
    }

    fn resolved_mode(&self, ctx: &egui::Context) -> Mode {
        resolve_mode(&self.config, ctx.style().visuals.dark_mode)
    }

    fn resolved_palette(&self, mode: Mode) -> Palette {
        resolve_palette(&self.config, mode)
    }

    /// Hands the Video pane the probe and the sound devices while it is
    /// open. The devices are listed again each time it opens, so one plugged
    /// in since is offered; until the answers are in, it is redrawn now and
    /// then to show them. Only what changed is copied into the view.
    fn sync_video_facts(&mut self, ctx: &egui::Context) {
        use video::availability::{probe_if_changed, refresh_sound, sound_if_changed};
        use video::ffmpeg::ProbeState;
        let open = self.settings_view.active_pane == windup_ui::settings::SettingsPane::Video;
        if open && !self.video_pane_open {
            let _ = refresh_sound();
        }
        self.video_pane_open = open;
        if !open {
            return;
        }
        let facts = &mut self.settings_view.video;
        if let Some(probe) = probe_if_changed(&facts.probe) {
            facts.probe = probe;
        }
        if let Some(sound) = sound_if_changed(&facts.sound) {
            facts.sound = sound;
        }
        // `GaveUp` is an answer too: nothing more will come.
        if facts.probe == ProbeState::Running || facts.sound.is_none() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }
}

/// The Steps pane's work on threads: what the desktop allows (the
/// ScreenCast portal may be asked, for up to 500 ms) and the one-time click
/// setup (pkexec waits for the password).
#[derive(Default)]
struct StepsPane {
    /// Whether the pane was open last frame, to look again each time it opens.
    open: bool,
    facts: Option<std::sync::mpsc::Receiver<windup_ui::StepsFacts>>,
    setup: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
}

impl WindupApp {
    /// Hands the Steps pane what this desktop allows, found again each time
    /// it opens and after the click setup.
    fn sync_steps_facts(&mut self, ctx: &egui::Context) {
        use std::sync::mpsc::TryRecvError;
        use windup_ui::ClickSetup;
        let open = self.settings_view.active_pane == windup_ui::settings::SettingsPane::Steps;
        if open && !self.steps_pane.open {
            self.steps_pane.facts = Some(look_for_steps_facts(self.settings_view.esc_caution));
        }
        self.steps_pane.open = open;
        if let Some(rx) = &self.steps_pane.facts {
            match rx.try_recv() {
                Ok(facts) => {
                    self.settings_view.steps = facts;
                    self.steps_pane.facts = None;
                }
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(200))
                }
                Err(TryRecvError::Disconnected) => self.steps_pane.facts = None,
            }
        }
        if let Some(rx) = &self.steps_pane.setup {
            let done = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                    None
                }
                Err(TryRecvError::Disconnected) => Some(Err("The setup stopped.".into())),
            };
            if let Some(result) = done {
                self.steps_pane.setup = None;
                self.settings_view.click_setup = match result {
                    Ok(()) => ClickSetup::Idle,
                    // Saying no to the password dialog is not a failure.
                    Err(e) if e == "Cancelled." => ClickSetup::Idle,
                    Err(e) => {
                        log::warn!("click setup: {e}");
                        ClickSetup::Failed(e)
                    }
                };
                // In the group now: the route says to log out and in.
                self.steps_pane.facts = Some(look_for_steps_facts(self.settings_view.esc_caution));
            }
        }
    }

    /// Runs what a settings pane asked for.
    fn settings_action(&mut self, action: windup_ui::SettingsAction) {
        match action {
            windup_ui::SettingsAction::JoinInputGroup => {
                if self.steps_pane.setup.is_some() {
                    return;
                }
                let (tx, rx) = std::sync::mpsc::channel();
                let spawned = std::thread::Builder::new()
                    .name("click-setup".into())
                    .spawn(move || {
                        let _ = tx.send(join_input_group());
                    });
                match spawned {
                    Ok(_) => {
                        self.steps_pane.setup = Some(rx);
                        self.settings_view.click_setup = windup_ui::ClickSetup::Waiting;
                    }
                    Err(e) => {
                        self.settings_view.click_setup =
                            windup_ui::ClickSetup::Failed(format!("Could not start: {e}"));
                    }
                }
            }
        }
    }
}

/// Adds this user to the input group (Linux, through pkexec).
fn join_input_group() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        linux::steps::setup::join_input_group()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("Only Linux reads clicks this way.".into())
    }
}

/// [`steps_facts`] on a thread. A finder that panics leaves the facts as
/// they were.
fn look_for_steps_facts(
    esc: windup_ui::EscCaution,
) -> std::sync::mpsc::Receiver<windup_ui::StepsFacts> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("steps-facts".into())
        .spawn(move || {
            let _ = tx.send(steps_facts(esc));
        });
    if let Err(e) = spawned {
        log::debug!("could not start steps-facts: {e}");
    }
    rx
}

/// What the Steps pane says about this desktop: how clicks arrive, whether
/// steps carry a marker, whether the bar shows and whether Steps works at
/// all. `esc` is what the Video pane says, since Steps takes its keys the
/// same way.
fn steps_facts(esc: windup_ui::EscCaution) -> windup_ui::StepsFacts {
    use windup_core::capture::backend::{BarHost, StepRoute, record_bar_host, steps_order};
    use windup_core::steps::avail::{markers_available, steps_available};
    use windup_core::steps::clicks::ClickRoute;
    let f = capture::video_facts();
    let order = steps_order(&f);
    // The OS hands Windows and X11 the clicks; Wayland's are read from the
    // pointer devices.
    #[cfg(target_os = "linux")]
    let route = if f.windows || f.x11_only {
        ClickRoute::Native
    } else {
        windup_core::steps::clicks::click_route(&linux::steps::evdev::click_facts())
    };
    #[cfg(not(target_os = "linux"))]
    let route = ClickRoute::Native;
    let appimage =
        cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_some_and(|v| !v.is_empty());
    windup_ui::StepsFacts {
        route,
        appimage,
        markers: markers_available(order.first().copied().unwrap_or(StepRoute::None)),
        bar_shown: record_bar_host(&f) != BarHost::Hidden,
        esc,
        available: steps_available(&order),
    }
}

/// Every session's own keys (recording, then Steps), to notice a change.
fn session_keys(config: &Config) -> Vec<(windup_core::Shortcut, windup_core::hotkeys::Action)> {
    use windup_core::hotkeys::{LiveKind, session_bindings};
    let mut keys = session_bindings(config, LiveKind::Video);
    keys.extend(session_bindings(config, LiveKind::Steps));
    keys
}

/// Light or dark, with "Follow system" settled by `system_dark`.
pub(crate) fn resolve_mode(config: &Config, system_dark: bool) -> Mode {
    match config.theme_mode {
        ThemeMode::Dark => Mode::Dark,
        ThemeMode::Light => Mode::Light,
        ThemeMode::System if system_dark => Mode::Dark,
        ThemeMode::System => Mode::Light,
    }
}

/// The custom theme the config names, or the built-in palette.
pub(crate) fn resolve_palette(config: &Config, mode: Mode) -> Palette {
    if let Some(ref custom_id) = config.custom_theme
        && let Some(themes_dir) = Config::themes_dir()
    {
        let (custom_themes, _) = windup_ui::themelib::load_all_from_dir(&themes_dir);
        if let Some(ct) = custom_themes.into_iter().find(|t| &t.id == custom_id) {
            return ct.palette;
        }
    }
    Palette::of(mode, config.accent_hue)
}

impl eframe::App for WindupApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Sync config atomics with live settings
        self.max_items_atomic
            .store(self.config.clipboard.max_items, Ordering::Relaxed);
        self.save_images_atomic
            .store(self.config.clipboard.save_images, Ordering::Relaxed);
        self.watching_atomic
            .store(self.config.clipboard.enabled, Ordering::Relaxed);

        // Check if history state changed and auto-persist to disk. A
        // settings-only window must never write it: the background process
        // owns that file.
        let current_state = {
            let hist = self.history.lock().unwrap();
            (hist.len(), hist.get(0).map(|e| e.id).unwrap_or(0))
        };
        if !self.settings_only && current_state != self.last_saved_state {
            self.last_saved_state = current_state;
            if let Some(ref p) = Config::history_path() {
                let hist = self.history.lock().unwrap();
                let _ = hist.save_to_disk(p);
            }
        }

        // A close that would end the app waits for a recording to be
        // saved; one that only hides the window (Linux) lets it run on.
        let hides = cfg!(target_os = "linux") && !self.settings_only && self.config.close_to_tray;
        self.hold_close_while_recording(ctx, hides);
        #[cfg(target_os = "linux")]
        self.keep_running_when_closed(ctx);

        // A package update replaced the program (X11; the Wayland hosts do
        // this in `linux`): the new one takes over while nothing is open.
        #[cfg(target_os = "linux")]
        if !self.settings_only
            && !self.popup_open
            && !self.overlay.busy()
            && !self.rec.live.busy()
            && !self.rec.steps_busy()
            && self.review.is_none()
            && let Some(exe) = linux::ipc::upgraded_exe()
        {
            linux::ipc::hand_over(&exe);
        }

        let wanted = windup_core::hotkeys::bindings(&self.config);
        let wanted_recording = session_keys(&self.config);
        if wanted != self.applied_bindings || wanted_recording != self.applied_recording {
            if let Some(l) = &self.hotkey_listener {
                l.rebind(&self.config);
            }
            paste::set_trigger_vk(paste_plain_vk(&wanted));
            self.applied_bindings = wanted;
            self.applied_recording = wanted_recording;
        }

        // -------------------------------------------------------------
        // Poll Global Hotkeys
        // -------------------------------------------------------------
        while let Some(event) = self.hotkey_listener.as_ref().and_then(|l| l.try_recv()) {
            match event {
                HotkeyEvent::ClipboardPopup { target } => {
                    if self.config.is_tool_enabled(ToolId::ClipboardHistory) {
                        let ((cx, cy), mon) = cursor::get_cursor_and_monitor();
                        let ppp = ctx.pixels_per_point().max(1.0);
                        let area = Area {
                            left: mon.left / ppp,
                            top: mon.top / ppp,
                            right: mon.right / ppp,
                            bottom: mon.bottom / ppp,
                        };
                        let pw = self.config.clipboard.popup_width.clamp(280.0, 900.0);
                        let ph = self.config.clipboard.popup_height.clamp(260.0, 1000.0);
                        let (lx, ly) = placement::place((cx / ppp, cy / ppp), area, (pw, ph));

                        self.popup_logical_pos = Some([lx, ly]);
                        self.target = target;
                        self.popup_pos = Some((cx, cy));
                        self.popup_open = true;
                        self.popup_first_frame = true;
                        self.popup_opened_at = Some(std::time::Instant::now());
                        self.popup_has_focused = false;
                        self.popup_view.reset();
                        ctx.request_repaint();
                    }
                }
                HotkeyEvent::PastePlainText { target } => {
                    if self.config.is_tool_enabled(ToolId::PastePlainText) {
                        paste::paste_current_clipboard_as_plain(target);
                    }
                }
                // Both open the launcher in Screenshot; its mode buttons
                // switch to Video from there.
                HotkeyEvent::Capture { target } => self.start_capture(
                    ctx,
                    CaptureMode::Screenshot,
                    ToolId::CaptureLauncher,
                    target,
                ),
                HotkeyEvent::Screenshot { target } => {
                    self.start_capture(ctx, CaptureMode::Screenshot, ToolId::Screenshot, target)
                }
                // While a recording runs, the Video hotkey stops it.
                HotkeyEvent::Record { target } => self.video_hotkey(ctx, target),
                // Pause, stop and Esc act on whichever session runs.
                HotkeyEvent::RecordPause => self.pause_live(),
                HotkeyEvent::RecordStop => self.stop_live(),
                HotkeyEvent::RecordCancel => self.cancel_live(),
                HotkeyEvent::Steps => self.steps_hotkey(ctx),
                HotkeyEvent::Step => self.take_step(),
                HotkeyEvent::StepsStop => {
                    if self.rec.steps_busy() {
                        self.stop_steps();
                    }
                }
                HotkeyEvent::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                HotkeyEvent::Reload => self.config = Config::load(),
            }
        }

        let mode = self.resolved_mode(ctx);
        let palette = self.resolved_palette(mode);
        apply_theme(ctx, palette, mode);

        // Synchronize window and taskbar icon when palette accent or ink changes
        let current_colors = (palette.accent, palette.accent_ink);
        if self.last_icon_colors != Some(current_colors) {
            self.last_icon_colors = Some(current_colors);
            let new_icon = Arc::new(windup_ui::art::window_icon(&palette));
            self.app_icon = new_icon.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Icon(Some(new_icon)));
        }

        // On Windows, permanently clear the window class background brush on startup
        // so no child/secondary window of this class ever paints an uninitialized white background
        #[cfg(target_os = "windows")]
        {
            static MAIN_INIT: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !MAIN_INIT.swap(true, std::sync::atomic::Ordering::Relaxed) {
                unsafe {
                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                        FindWindowW, GCLP_HBRBACKGROUND, SetClassLongPtrW,
                    };
                    let wide: Vec<u16> =
                        "Windup".encode_utf16().chain(std::iter::once(0)).collect();
                    let main_hwnd = FindWindowW(std::ptr::null(), wide.as_ptr());
                    if !main_hwnd.is_null() {
                        SetClassLongPtrW(main_hwnd, GCLP_HBRBACKGROUND, 0);
                    }
                }
            }
        }

        // -------------------------------------------------------------
        // Main Window: Settings Hub
        // -------------------------------------------------------------
        self.sync_video_facts(ctx);
        self.sync_steps_facts(ctx);
        let action = egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(palette.window))
            .show(ctx, |ui| {
                self.settings_view.ui(ui, &mut self.config, &palette, mode)
            })
            .inner;
        if let Some(action) = action {
            self.settings_action(action);
        }

        // -------------------------------------------------------------
        // Secondary Floating Viewport: Clipboard History Popup
        // -------------------------------------------------------------
        if self.popup_open {
            let ppp = ctx.pixels_per_point().max(1.0);
            let pw = self.config.clipboard.popup_width.clamp(280.0, 900.0);
            let ph = self.config.clipboard.popup_height.clamp(260.0, 1000.0);

            // Anchor popup position once when opened so resizing downwards doesn't push the top edge up
            let [logical_x, logical_y] = self.popup_logical_pos.unwrap_or([100.0, 100.0]);

            let viewport_id = egui::ViewportId::from_hash_of("clipboard_history_popup");

            let viewport_builder = egui::ViewportBuilder::default()
                .with_title("Clipboard History")
                .with_icon(self.app_icon.clone())
                .with_decorations(false)
                .with_transparent(true)
                .with_always_on_top()
                .with_active(true)
                .with_visible(!self.popup_first_frame)
                .with_inner_size([pw, ph])
                .with_position([logical_x, logical_y]);

            let mut close_popup = false;
            let mut popup_action: Option<PopupAction> = None;

            ctx.show_viewport_immediate(viewport_id, viewport_builder, |ctx, class| {
                if class == egui::ViewportClass::Immediate {
                    let popup_hwnd = focus_and_clip_popup_window(
                        "Clipboard History",
                        pw,
                        ph,
                        ppp,
                        !self.popup_has_focused,
                    );
                    // Only the Win32 focus restore below reads it.
                    #[cfg(not(target_os = "windows"))]
                    let _ = popup_hwnd;

                    if self.popup_first_frame {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        self.popup_first_frame = false;
                    }

                    let p_mode = self.resolved_mode(ctx);
                    let p_palette = self.resolved_palette(p_mode);
                    apply_theme(ctx, p_palette, p_mode);

                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE.fill(egui::Color32::TRANSPARENT))
                        .show(ctx, |ui| {
                            let hist = self.history.lock().unwrap();
                            if let Some(action) = self.popup_view.ui(ui, &hist, &p_palette) {
                                popup_action = Some(action);
                            }
                        });

                    let is_focused = ctx.input(|i| i.viewport().focused);
                    if is_focused == Some(true) {
                        self.popup_has_focused = true;
                    }

                    let elapsed = self
                        .popup_opened_at
                        .map(|t| t.elapsed().as_millis())
                        .unwrap_or(0);

                    // No foreground window at all is the middle of a switch,
                    // not another window taking over.
                    #[cfg(target_os = "windows")]
                    let (win32_focused, lost_win32_focus) = unsafe {
                        use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
                        let fg = GetForegroundWindow();
                        let mine = popup_hwnd != 0
                            && fg == popup_hwnd as windows_sys::Win32::Foundation::HWND;
                        (mine, popup_hwnd != 0 && !fg.is_null() && !mine)
                    };
                    #[cfg(not(target_os = "windows"))]
                    let win32_focused = false;
                    if win32_focused {
                        self.popup_has_focused = true;
                    }
                    // egui's flag lags the foreground window (it waits for
                    // WM_SETFOCUS), so on Windows the foreground decides.
                    #[cfg(target_os = "windows")]
                    let unfocused = lost_win32_focus;
                    #[cfg(not(target_os = "windows"))]
                    let unfocused = is_focused == Some(false);

                    if popup_dismissed(elapsed, self.popup_has_focused, unfocused) {
                        close_popup = true;
                    }

                    // Immediate dismiss on Escape key
                    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                        close_popup = true;
                    }
                }
            });

            if let Some(action) = popup_action {
                match action {
                    PopupAction::Paste(entry) => {
                        close_popup = true;
                        if self.config.clipboard.auto_paste {
                            paste::paste_entry_to_target(self.target, &entry);
                        } else {
                            paste::populate_clipboard(&entry);
                        }
                    }
                    PopupAction::PastePlain(text) => {
                        close_popup = true;
                        paste::paste_plain_to_target(self.target, &text);
                    }
                    PopupAction::Delete(id) => {
                        let mut hist = self.history.lock().unwrap();
                        hist.remove_by_id(id);
                        if let Some(ref p) = Config::history_path() {
                            let _ = hist.save_to_disk(p);
                        }
                        self.last_saved_state =
                            (hist.len(), hist.get(0).map(|e| e.id).unwrap_or(0));
                    }
                    PopupAction::Resize(w, h) => {
                        self.config.clipboard.popup_width = w;
                        self.config.clipboard.popup_height = h;
                        self.config.save();
                    }
                    PopupAction::ResizeLeft(w, h, dx) => {
                        if let Some(pos) = self.popup_logical_pos.as_mut() {
                            pos[0] += dx;
                            ctx.send_viewport_cmd_to(
                                viewport_id,
                                egui::ViewportCommand::OuterPosition(egui::pos2(pos[0], pos[1])),
                            );
                        }
                        self.config.clipboard.popup_width = w;
                        self.config.clipboard.popup_height = h;
                        self.config.save();
                    }
                    PopupAction::Dismiss => {
                        close_popup = true;
                    }
                }
            }

            if close_popup {
                self.popup_open = false;
                #[cfg(target_os = "windows")]
                hide_popup_window("Clipboard History");
                reset_popup_clipping_cache();
            }
        }

        // Before the overlays: a Record they give starts next frame.
        self.drive_recording(ctx, &palette, mode);
        // Before the overlays: a step it opens in the editor shows at once.
        self.drive_review(ctx, &palette, mode);
        self.drive_capture(ctx, &palette);

        // Keep UI smooth and responsive (60fps when popup is open, 100ms when idle)
        if self.popup_open {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

/// Whether the clipboard popup closes, `elapsed_ms` after it opened, given
/// whether it has had focus yet and whether something else has it now. It
/// closes when it loses focus it had, not because taking focus is slow; one
/// that never gets focus can take no keys, so it goes after a while.
fn popup_dismissed(elapsed_ms: u128, has_focused: bool, unfocused: bool) -> bool {
    const SETTLE_MS: u128 = 150;
    const NEVER_FOCUSED_MS: u128 = 1500;
    unfocused
        && if has_focused {
            elapsed_ms > SETTLE_MS
        } else {
            elapsed_ms > NEVER_FOCUSED_MS
        }
}

#[cfg(target_os = "windows")]
static LAST_CLIPPED: std::sync::Mutex<Option<(isize, i32, i32)>> = std::sync::Mutex::new(None);

#[cfg(target_os = "windows")]
pub fn reset_popup_clipping_cache() {
    if let Ok(mut guard) = LAST_CLIPPED.lock() {
        *guard = None;
    }
}

#[cfg(not(target_os = "windows"))]
pub fn reset_popup_clipping_cache() {}

#[cfg(target_os = "windows")]
pub fn hide_popup_window(title: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, SW_HIDE, ShowWindow};
    unsafe {
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        let hwnd = FindWindowW(std::ptr::null(), wide.as_ptr());
        if !hwnd.is_null() {
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn hide_popup_window(_title: &str) {}

/// Keeps Windows from painting a frame on a borderless card: activation
/// changes are passed on with `lParam = -1`, which tells Windows not to
/// redraw the non-client area, and non-client paints are dropped.
#[cfg(target_os = "windows")]
unsafe extern "system" fn no_frame_paint(
    hwnd: windows_sys::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
    _id: usize,
    _data: usize,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::Shell::DefSubclassProc;
    use windows_sys::Win32::UI::WindowsAndMessaging::{WM_NCACTIVATE, WM_NCPAINT};
    // SAFETY: called by Windows for a window this process subclassed.
    unsafe {
        match msg {
            WM_NCACTIVATE => DefSubclassProc(hwnd, msg, wparam, -1),
            WM_NCPAINT => 0,
            _ => DefSubclassProc(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(target_os = "windows")]
fn focus_and_clip_popup_window(
    title: &str,
    width: f32,
    height: f32,
    ppp: f32,
    take_focus: bool,
) -> isize {
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
    use windows_sys::Win32::Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn};
    use windows_sys::Win32::System::Threading::{
        AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, FindWindowW, GCLP_HBRBACKGROUND, GetForegroundWindow,
        GetWindowThreadProcessId, SW_SHOW, SetClassLongPtrW, SetForegroundWindow, ShowWindow,
    };

    #[repr(C)]
    struct Margins {
        cx_left: i32,
        cx_right: i32,
        cy_top: i32,
        cy_bottom: i32,
    }

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmExtendFrameIntoClientArea(
            hwnd: *mut core::ffi::c_void,
            pmargins: *const Margins,
        ) -> i32;
    }

    unsafe {
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        let hwnd = FindWindowW(std::ptr::null(), wide.as_ptr());
        if hwnd.is_null() {
            return 0;
        }

        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != GetCurrentProcessId() {
            return 0;
        }

        // Set GCLP_HBRBACKGROUND to NULL to avoid GDI erasing with white
        SetClassLongPtrW(hwnd, GCLP_HBRBACKGROUND, 0);

        let pw = (width * ppp).round() as i32;
        let ph = (height * ppp).round() as i32;

        let needs_clip = if let Ok(mut guard) = LAST_CLIPPED.lock() {
            if *guard != Some((hwnd as isize, pw, ph)) {
                *guard = Some((hwnd as isize, pw, ph));
                true
            } else {
                false
            }
        } else {
            true
        };

        if needs_clip {
            // 1. Extend DWM frame into client area for full window composition
            let margins = Margins {
                cx_left: -1,
                cx_right: -1,
                cy_top: -1,
                cy_bottom: -1,
            };
            let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);

            // 2. Set DWM corner preference (DWMWA_WINDOW_CORNER_PREFERENCE = 33, DWMWCP_ROUND = 2)
            let round_pref: u32 = 2;
            let _ = DwmSetWindowAttribute(hwnd, 33, &round_pref as *const _ as *const _, 4);

            // A window with a region loses DWM's frame drawing, so Windows
            // would paint a classic frame over the card whenever it gains or
            // loses focus. Same id each time: subclassing again only updates.
            windows_sys::Win32::UI::Shell::SetWindowSubclass(hwnd, Some(no_frame_paint), 1, 0);

            // 3. Set physical window region to cut away all corner pixels outside radius
            let r = (8.0 * ppp).round() as i32; // metrics::RADIUS_CARD is 8.0
            let rgn = CreateRoundRectRgn(0, 0, pw + 1, ph + 1, r * 2, r * 2);
            if !rgn.is_null() && SetWindowRgn(hwnd, rgn, 1) == 0 {
                DeleteObject(rgn);
            }
        }

        // Make window visible now that region & DWM composition are configured
        ShowWindow(hwnd, SW_SHOW);

        // Force foreground focus onto the popup window, until it has it:
        // afterwards a click elsewhere must be allowed to take it away.
        let fg = GetForegroundWindow();
        if !take_focus || fg == hwnd {
            return hwnd as isize;
        }
        let cur_thread = GetCurrentThreadId();
        let fg_thread = GetWindowThreadProcessId(fg, std::ptr::null_mut());
        if fg_thread != 0 && fg_thread != cur_thread {
            AttachThreadInput(cur_thread, fg_thread, 1);
        }
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
        if fg_thread != 0 && fg_thread != cur_thread {
            AttachThreadInput(cur_thread, fg_thread, 0);
        }

        hwnd as isize
    }
}

#[cfg(not(target_os = "windows"))]
fn focus_and_clip_popup_window(
    _title: &str,
    _width: f32,
    _height: f32,
    _ppp: f32,
    _take_focus: bool,
) -> isize {
    0
}

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().filter_or("RUST_LOG", "warn,wgpu_hal=error,wgpu_core=error"),
    )
    .init();

    // The setup executable is this binary with the MSI on its end, and it is
    // double-clicked, so it has no arguments: the payload is what says this
    // launch is the installer. Sixteen bytes read off our own file.
    let carries_payload =
        std::env::current_exe().is_ok_and(|exe| windup_core::install::payload::carried_by(&exe));
    if windup_core::install::is_setup(std::env::args(), carries_payload) {
        return setup::run();
    }

    // A development aid: grab the screen, write it out, leave. No window.
    if let windup_core::ipc::Launch::CaptureTest(path) =
        windup_core::ipc::parse_args(std::env::args().skip(1))
    {
        std::process::exit(capture_test(&path));
    }

    // One step's picture, as a Steps session takes it. No window.
    if let windup_core::ipc::Launch::StepShotTest(path) =
        windup_core::ipc::parse_args(std::env::args().skip(1))
    {
        std::process::exit(step_shot_test(&path));
    }

    if windup_core::ipc::parse_args(std::env::args().skip(1))
        == windup_core::ipc::Launch::WindowRectsTest
    {
        std::process::exit(window_rects_test());
    }

    if windup_core::ipc::parse_args(std::env::args().skip(1))
        == windup_core::ipc::Launch::EncodersTest
    {
        std::process::exit(video::ffmpeg::encoders_test());
    }

    // Records a generated pattern through the real pipeline. No window, and
    // a malformed request exits rather than falling through to the app.
    if std::env::args().nth(1).as_deref() == Some("--record-test") {
        match windup_core::ipc::parse_args(std::env::args().skip(1)) {
            windup_core::ipc::Launch::RecordTest(test) => {
                std::process::exit(video::test_source::record_test(&test))
            }
            windup_core::ipc::Launch::Unknown(why) => {
                eprintln!("windup: {why}");
                std::process::exit(2);
            }
            _ => {}
        }
    }

    // Exports a Steps session folder; for the end-to-end tests. No window.
    if std::env::args().nth(1).as_deref() == Some("--steps-export") {
        match windup_core::ipc::parse_args(std::env::args().skip(1)) {
            windup_core::ipc::Launch::StepsExport(job) => std::process::exit(steps_export(&job)),
            windup_core::ipc::Launch::Unknown(why) => {
                eprintln!("windup: {why}");
                std::process::exit(2);
            }
            _ => {}
        }
    }

    // The Steps review, as a child of the background process on Wayland.
    #[cfg(target_os = "linux")]
    if let windup_core::ipc::Launch::StepsReview(dir) =
        windup_core::ipc::parse_args(std::env::args().skip(1))
    {
        return steps::review_app::run(dir);
    }

    #[cfg(target_os = "linux")]
    let Some(startup) = linux_startup() else {
        return Ok(());
    };
    // One Windup at a time: two would both read and write the history.
    #[cfg(target_os = "windows")]
    if !windows_first_instance() {
        return Ok(());
    }
    // Segment folders a Windup that died while recording left behind.
    #[cfg(not(target_os = "linux"))]
    video::run::remove_stale_segments();
    // ... and Steps session folders.
    #[cfg(not(target_os = "linux"))]
    steps::live::remove_stale_sessions();
    #[cfg(not(target_os = "linux"))]
    let startup = Startup {
        plan: if cfg!(target_os = "windows") {
            Plan::WINDOWS
        } else {
            Plan::OTHER
        },
        settings_only: false,
    };
    #[cfg(target_os = "linux")]
    let start_hidden = startup.listener.is_some() && LAUNCHED_IN_BACKGROUND.get() == Some(&true);
    #[cfg(not(target_os = "linux"))]
    let start_hidden = false;

    let config = Config::load();
    let mode = resolve_mode(&config, true);
    let initial_palette = resolve_palette(&config, mode);

    let app_icon = windup_ui::art::window_icon(&initial_palette);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Windup")
            .with_app_id(APP_ID)
            .with_icon(app_icon)
            .with_inner_size([960.0, 620.0])
            .with_min_inner_size([720.0, 480.0])
            .with_visible(!start_hidden)
            .with_transparent(true),
        ..Default::default()
    };

    eframe::run_native(
        "Windup",
        options,
        Box::new(|cc| Ok(Box::new(WindupApp::new(cc, startup)))),
    )
}

/// Whether this is the only Windup of this user's session. Another one has
/// its window brought up instead (restored if minimised), and this launch
/// ends. The mutex is held, never closed, for as long as the process runs.
#[cfg(target_os = "windows")]
fn windows_first_instance() -> bool {
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM};
    use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentProcessId};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsIconic, SW_RESTORE, SW_SHOW,
        SetForegroundWindow, ShowWindow,
    };

    let name: Vec<u16> = "Local\\io.github.spillebulle.windup"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: a named mutex with default security; the handle is kept.
    let first = unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        handle.is_null() || GetLastError() != ERROR_ALREADY_EXISTS
    };
    if first {
        return true;
    }
    log::info!("Windup is already running; showing its window");

    /// The other Windup's main window: titled "Windup", not of this process.
    unsafe extern "system" fn find(hwnd: HWND, found: LPARAM) -> i32 {
        unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == GetCurrentProcessId() {
                return 1;
            }
            let mut title = [0u16; 16];
            let n = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
            if String::from_utf16_lossy(&title[..n.max(0) as usize]) == "Windup" {
                *(found as *mut HWND) = hwnd;
                return 0;
            }
            1
        }
    }
    let mut hwnd: HWND = std::ptr::null_mut();
    // SAFETY: `find` writes only to `hwnd`, which outlives the call.
    unsafe {
        EnumWindows(Some(find), &mut hwnd as *mut HWND as LPARAM);
        if !hwnd.is_null() {
            ShowWindow(
                hwnd,
                if IsIconic(hwnd) != 0 {
                    SW_RESTORE
                } else {
                    SW_SHOW
                },
            );
            SetForegroundWindow(hwnd);
        }
    }
    false
}

/// `windup --window-rects-test`: the window rectangles the capture overlay
/// would use, through the same dispatch. Prints `rects: N`, then one line per
/// rectangle (topmost first).
fn window_rects_test() -> i32 {
    let shot = match capture::grab_all() {
        Ok(shot) => shot,
        Err(e) => {
            eprintln!("windup: {}", e.message());
            return 1;
        }
    };
    let rects = capture::window_rects(&shot);
    println!("rects: {}", rects.len());
    for w in &rects {
        let r = w.rect;
        println!("  {} {} {}x{} {}", r.x, r.y, r.w, r.h, w.title);
    }
    0
}

/// `windup --steps-export <dir> <format> <out>`: the session in `dir`
/// exported as the review would, with the saved marker style. Prints what
/// was written.
fn steps_export(job: &windup_core::ipc::StepsExport) -> i32 {
    let style = Config::load().steps.marker;
    match windup_core::steps::export::export(&job.dir, job.format, &style, &job.out) {
        Ok(path) => {
            println!("{}", path.display());
            0
        }
        Err(e) => {
            eprintln!("windup: {e}");
            1
        }
    }
}

/// `windup --step-shot-test out.png`: one step's picture by the camera a
/// session would open, of the pointer's monitor. Prints `pointer X,Y` in the
/// picture's pixels, or `pointer unknown`. The ScreenCast portal's next
/// restore token is saved, as a Steps session saves it, so a second run
/// shows no dialog.
fn step_shot_test(path: &std::path::Path) -> i32 {
    let order = windup_core::capture::backend::steps_order(&capture::facts());
    let saved = windup_core::config::Config::load_screencast_token();
    let token = Arc::new(Mutex::new(saved.clone()));
    let opened = steps::camera::open_camera(&order, &token);
    let next = token.lock().ok().and_then(|t| t.clone());
    if next != saved {
        windup_core::config::Config::save_screencast_token(next.as_deref());
    }
    let (mut camera, route) = match opened {
        Ok(opened) => opened,
        Err(why) => {
            eprintln!("windup: {why}");
            return 1;
        }
    };
    let shot = match camera.shoot(None) {
        Ok(shot) => shot,
        Err(e) => {
            eprintln!("windup: {route:?}: {}", steps::camera::describe(&e));
            return 1;
        }
    };
    if let Err(e) = shot.image.save_with_format(path, image::ImageFormat::Png) {
        eprintln!("windup: could not write {}: {e}", path.display());
        return 1;
    }
    log::info!(
        "{route:?}: {} {}x{} -> {}",
        shot.monitor_name,
        shot.image.width(),
        shot.image.height(),
        path.display()
    );
    match shot.pointer {
        Some([x, y]) => println!("pointer {},{}", x.round(), y.round()),
        None => println!("pointer unknown"),
    }
    0
}

/// `windup --capture-test out.png`: the whole desktop as one PNG. The exit
/// code is the process's.
fn capture_test(path: &std::path::Path) -> i32 {
    let shot = match capture::grab_all() {
        Ok(shot) => shot,
        Err(e) => {
            eprintln!("windup: {}", e.message());
            if let capture::CaptureError::Failed(why) = &e {
                eprintln!("windup: {why}");
            }
            return 1;
        }
    };
    let Some(image) = shot.compose(shot.bounds()) else {
        eprintln!("windup: the screen could not be captured (nothing to save)");
        return 1;
    };
    if let Err(e) = image.save_with_format(path, image::ImageFormat::Png) {
        eprintln!("windup: could not write {}: {e}", path.display());
        return 1;
    }
    println!(
        "{} monitor(s), {}x{} -> {}",
        shot.monitors.len(),
        image.width(),
        image.height(),
        path.display()
    );
    0
}

/// The desktop entry, icon and Wayland app id all carry this name.
pub(crate) const APP_ID: &str = "io.github.spillebulle.windup";

#[cfg(target_os = "linux")]
static LAUNCHED_IN_BACKGROUND: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// Decides what this launch is on Linux: hand a command to the Windup
/// already running, become the background process (layer-shell desktops), or
/// carry on into the eframe app. `None` means there is nothing left to do.
#[cfg(target_os = "linux")]
fn linux_startup() -> Option<Startup> {
    use windup_core::ipc::{self, Command, Launch};

    let launch = ipc::parse_args(std::env::args().skip(1));
    match launch {
        Launch::Help => {
            println!("{}", ipc::USAGE);
            return None;
        }
        // Handled in `main` before this runs.
        Launch::CaptureTest(_)
        | Launch::StepShotTest(_)
        | Launch::WindowRectsTest
        | Launch::EncodersTest
        | Launch::RecordTest(_)
        | Launch::StepsExport(_)
        | Launch::StepsReview(_) => return None,
        Launch::Unknown(why) => {
            eprintln!("windup: {why}\n\n{}", ipc::USAGE);
            std::process::exit(2);
        }
        Launch::Send(command) => {
            if let Err(e) = linux::ipc::send_or_start(command) {
                eprintln!("windup: could not reach or start Windup: {e}");
                std::process::exit(1);
            }
            return None;
        }
        Launch::Settings => {
            return Some(Startup {
                plan: Plan::OTHER,
                settings_only: true,
                listener: None,
            });
        }
        Launch::App | Launch::Background => {}
    }
    let background = launch == Launch::Background;
    let _ = LAUNCHED_IN_BACKGROUND.set(background);

    let listener = match linux::ipc::bind() {
        Ok(linux::ipc::Bound::Listening(l)) => l,
        Ok(linux::ipc::Bound::AlreadyRunning) => {
            if !background {
                let _ = linux::ipc::send(Command::Show);
            }
            return None;
        }
        Err(e) => {
            log::error!("socket {}: {e}", linux::ipc::socket_path().display());
            eprintln!("windup: cannot create its socket: {e}");
            std::process::exit(1);
        }
    };

    // The one Windup on this display: segment folders a Windup that died
    // while recording left behind are removed.
    video::run::remove_stale_segments();
    steps::live::remove_stale_sessions();

    let plan = linux::detect();
    let listener = if plan.host != platform::Host::Viewport {
        match linux::run_background(plan, listener, !background) {
            Ok(()) => return None,
            // The Wayland service would not start: carry on as an eframe app.
            Err(listener) => listener,
        }
    } else {
        listener
    };
    Some(Startup {
        plan: linux::runtime().map_or(plan, |r| r.plan),
        settings_only: false,
        listener: Some(listener),
    })
}

#[cfg(target_os = "linux")]
impl WindupApp {
    /// Closing the window must not end the clipboard history and the
    /// hotkeys with it. X11 can hide a window; Wayland cannot, so there it
    /// is minimised. Running `windup` again brings it back.
    fn keep_running_when_closed(&mut self, ctx: &egui::Context) {
        if self.settings_only || !self.config.close_to_tray {
            return;
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }
    }
}

#[cfg(test)]
mod popup_tests {
    use super::popup_dismissed;

    #[test]
    fn the_popup_stays_while_it_is_still_taking_focus() {
        assert!(!popup_dismissed(400, false, true));
        assert!(!popup_dismissed(100, true, true));
    }

    #[test]
    fn the_popup_goes_when_it_loses_focus_it_had() {
        assert!(popup_dismissed(400, true, true));
        assert!(!popup_dismissed(400, true, false));
    }

    #[test]
    fn a_popup_that_never_gets_focus_goes_in_the_end() {
        assert!(popup_dismissed(1600, false, true));
    }
}
