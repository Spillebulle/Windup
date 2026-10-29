#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod cursor;
mod hotkey;
#[cfg(target_os = "linux")]
mod linux;
mod overlay;
mod paste;
mod platform;
mod setup;
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
    settings_only: bool,
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
        if !startup.settings_only {
            video::ffmpeg::start_probe();
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
                linux::ipc::serve(socket, move |command| {
                    use windup_core::ipc::Command;
                    let target = linux::current_target(&plan);
                    let event = match command {
                        Command::Popup => HotkeyEvent::ClipboardPopup { target },
                        Command::PastePlain => HotkeyEvent::PastePlainText { target },
                        Command::Capture => HotkeyEvent::Capture { target },
                        Command::Screenshot => HotkeyEvent::Screenshot { target },
                        Command::Record => HotkeyEvent::Record { target },
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
        paste::set_trigger_vk(paste_plain_vk(&applied_bindings));
        Self {
            config,
            settings_view: SettingsView {
                hotkey_hint: hotkey_hint(plan, startup.settings_only),
                ..SettingsView::default()
            },
            history,
            _monitor: monitor,
            hotkey_listener,
            applied_bindings,
            settings_only: startup.settings_only,
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

        #[cfg(target_os = "linux")]
        self.keep_running_when_closed(ctx);

        // A package update replaced the program (X11; the Wayland hosts do
        // this in `linux`): the new one takes over while nothing is open.
        #[cfg(target_os = "linux")]
        if !self.settings_only
            && !self.popup_open
            && !self.overlay.busy()
            && let Some(exe) = linux::ipc::upgraded_exe()
        {
            linux::ipc::hand_over(&exe);
        }

        let wanted = windup_core::hotkeys::bindings(&self.config);
        if wanted != self.applied_bindings {
            if let Some(l) = &self.hotkey_listener {
                l.rebind(&self.config);
            }
            paste::set_trigger_vk(paste_plain_vk(&wanted));
            self.applied_bindings = wanted;
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
                // Phase 1 has one mode, so both open the same overlay; the
                // launcher's mode buttons arrive with Video and Steps.
                HotkeyEvent::Capture { target } => self.start_capture(
                    ctx,
                    CaptureMode::Screenshot,
                    ToolId::CaptureLauncher,
                    target,
                ),
                HotkeyEvent::Screenshot { target } => {
                    self.start_capture(ctx, CaptureMode::Screenshot, ToolId::Screenshot, target)
                }
                HotkeyEvent::Record { .. } => log::info!("Video arrives in a later task"),
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
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(palette.window))
            .show(ctx, |ui| {
                self.settings_view.ui(ui, &mut self.config, &palette, mode);
            });

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
                    let popup_hwnd = focus_and_clip_popup_window("Clipboard History", pw, ph, ppp);
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

                    #[cfg(target_os = "windows")]
                    let lost_win32_focus = unsafe {
                        use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
                        let fg = GetForegroundWindow();
                        popup_hwnd != 0 && fg != popup_hwnd as windows_sys::Win32::Foundation::HWND
                    };
                    #[cfg(not(target_os = "windows"))]
                    let lost_win32_focus = false;

                    // Auto-dismiss if not focused:
                    // After initial 150ms for window creation & activation,
                    // if egui says not focused OR Win32 foreground window is not our popup,
                    // close immediately!
                    if elapsed > 150 && (is_focused == Some(false) || lost_win32_focus) {
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

        self.drive_capture(ctx, &palette);

        // Keep UI smooth and responsive (60fps when popup is open, 100ms when idle)
        if self.popup_open {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
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

#[cfg(target_os = "windows")]
fn focus_and_clip_popup_window(title: &str, width: f32, height: f32, ppp: f32) -> isize {
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

            // 3. Set physical window region to cut away all corner pixels outside radius
            let r = (8.0 * ppp).round() as i32; // metrics::RADIUS_CARD is 8.0
            let rgn = CreateRoundRectRgn(0, 0, pw + 1, ph + 1, r * 2, r * 2);
            if !rgn.is_null() && SetWindowRgn(hwnd, rgn, 1) == 0 {
                DeleteObject(rgn);
            }
        }

        // Make window visible now that region & DWM composition are configured
        ShowWindow(hwnd, SW_SHOW);

        // Force foreground focus onto the popup window
        let fg = GetForegroundWindow();
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
fn focus_and_clip_popup_window(_title: &str, _width: f32, _height: f32, _ppp: f32) -> isize {
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
                std::process::exit(video::run::record_test(&test))
            }
            windup_core::ipc::Launch::Unknown(why) => {
                eprintln!("windup: {why}");
                std::process::exit(2);
            }
            _ => {}
        }
    }

    #[cfg(target_os = "linux")]
    let Some(startup) = linux_startup() else {
        return Ok(());
    };
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
        | Launch::WindowRectsTest
        | Launch::EncodersTest
        | Launch::RecordTest(_) => return None,
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
