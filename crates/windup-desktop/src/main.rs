#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cursor;
mod hotkey;
mod paste;

use eframe::egui;
use hotkey::{HotkeyEvent, HotkeyListener};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use windup_core::clipboard::{ClipboardHistory, ClipboardMonitor};
use windup_core::{Config, ThemeMode, ToolId};
use windup_ui::{Mode, Palette, PopupAction, PopupView, SettingsView, apply_theme, install_fonts};

struct WindupApp {
    config: Config,
    settings_view: SettingsView,
    history: Arc<Mutex<ClipboardHistory>>,
    _monitor: ClipboardMonitor,
    hotkey_listener: HotkeyListener,
    popup_view: PopupView,
    popup_open: bool,
    popup_opened_at: Option<std::time::Instant>,
    popup_has_focused: bool,
    popup_first_frame: bool,
    popup_pos: Option<(f32, f32)>,
    popup_logical_pos: Option<[f32; 2]>,
    target_hwnd: isize,
    last_saved_state: (usize, u64),
    max_items_atomic: Arc<AtomicUsize>,
    save_images_atomic: Arc<AtomicBool>,
    app_icon: Arc<egui::IconData>,
    last_icon_colors: Option<(egui::Color32, egui::Color32)>,
}

impl WindupApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        let config = Config::load();
        let initial_palette = Palette::of(Mode::Dark, config.accent_hue);
        let app_icon = Arc::new(create_app_icon(
            initial_palette.accent,
            initial_palette.accent_ink,
        ));

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

        let monitor = ClipboardMonitor::new(Arc::clone(&history));
        if config.clipboard.enabled {
            monitor.start(
                Arc::clone(&max_items_atomic),
                Arc::clone(&save_images_atomic),
            );
        }

        let hotkey_listener = HotkeyListener::new();

        Self {
            config,
            settings_view: SettingsView::default(),
            history,
            _monitor: monitor,
            hotkey_listener,
            popup_view: PopupView::new(),
            popup_open: false,
            popup_opened_at: None,
            popup_has_focused: false,
            popup_first_frame: false,
            popup_pos: None,
            popup_logical_pos: None,
            target_hwnd: 0,
            last_saved_state: initial_state,
            max_items_atomic,
            save_images_atomic,
            app_icon,
            last_icon_colors: Some((initial_palette.accent, initial_palette.accent_ink)),
        }
    }

    fn resolved_mode(&self, ctx: &egui::Context) -> Mode {
        match self.config.theme_mode {
            ThemeMode::Dark => Mode::Dark,
            ThemeMode::Light => Mode::Light,
            ThemeMode::System => {
                if ctx.style().visuals.dark_mode {
                    Mode::Dark
                } else {
                    Mode::Light
                }
            }
        }
    }

    fn resolved_palette(&self, mode: Mode) -> Palette {
        if let Some(ref custom_id) = self.config.custom_theme
            && let Some(themes_dir) = Config::themes_dir()
        {
            let (custom_themes, _) = windup_ui::themelib::load_all_from_dir(&themes_dir);
            if let Some(ct) = custom_themes.into_iter().find(|t| &t.id == custom_id) {
                return ct.palette;
            }
        }
        Palette::of(mode, self.config.accent_hue)
    }
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

        // Check if history state changed and auto-persist to disk
        let current_state = {
            let hist = self.history.lock().unwrap();
            (hist.len(), hist.get(0).map(|e| e.id).unwrap_or(0))
        };
        if current_state != self.last_saved_state {
            self.last_saved_state = current_state;
            if let Some(ref p) = Config::history_path() {
                let hist = self.history.lock().unwrap();
                let _ = hist.save_to_disk(p);
            }
        }

        // -------------------------------------------------------------
        // Poll Global Hotkeys
        // -------------------------------------------------------------
        while let Some(event) = self.hotkey_listener.try_recv() {
            match event {
                HotkeyEvent::ClipboardPopup { target_hwnd } => {
                    if self.config.is_tool_enabled(ToolId::ClipboardHistory) {
                        let ((cx, cy), mon) = cursor::get_cursor_and_monitor();
                        let ppp = ctx.pixels_per_point().max(1.0);

                        let cur_x = cx / ppp;
                        let cur_y = cy / ppp;
                        let work_left = mon.left / ppp;
                        let work_top = mon.top / ppp;
                        let work_right = mon.right / ppp;
                        let work_bottom = mon.bottom / ppp;

                        let pw = self.config.clipboard.popup_width.clamp(280.0, 900.0);
                        let ph = self.config.clipboard.popup_height.clamp(260.0, 1000.0);

                        // Horizontal placement:
                        // Default to 4 points right of cursor; flip to left of cursor if overflowing right edge
                        let mut lx = cur_x + 4.0;
                        if lx + pw > work_right - 8.0 {
                            lx = (cur_x - pw - 4.0).max(work_left + 8.0);
                        }
                        if lx < work_left + 8.0 {
                            lx = work_left + 8.0;
                        }

                        // Vertical placement:
                        // Default to 12 points below cursor; flip to above cursor if overflowing bottom edge
                        let mut ly = cur_y + 12.0;
                        if ly + ph > work_bottom - 8.0 {
                            ly = cur_y - ph - 12.0;
                        }
                        if ly < work_top + 8.0 {
                            ly = work_top + 8.0;
                        }

                        self.popup_logical_pos = Some([lx, ly]);
                        self.target_hwnd = target_hwnd;
                        self.popup_pos = Some((cx, cy));
                        self.popup_open = true;
                        self.popup_first_frame = true;
                        self.popup_opened_at = Some(std::time::Instant::now());
                        self.popup_has_focused = false;
                        self.popup_view.reset();
                        ctx.request_repaint();
                    }
                }
                HotkeyEvent::PastePlainText { target_hwnd } => {
                    if self.config.is_tool_enabled(ToolId::PastePlainText) {
                        paste::paste_current_clipboard_as_plain(target_hwnd);
                    }
                }
            }
        }

        let mode = self.resolved_mode(ctx);
        let palette = self.resolved_palette(mode);
        apply_theme(ctx, palette, mode);

        // Synchronize window and taskbar icon when palette accent or ink changes
        let current_colors = (palette.accent, palette.accent_ink);
        if self.last_icon_colors != Some(current_colors) {
            self.last_icon_colors = Some(current_colors);
            let new_icon = Arc::new(create_app_icon(palette.accent, palette.accent_ink));
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
                    #[cfg(target_os = "windows")]
                    let popup_hwnd = focus_and_clip_popup_window("Clipboard History", pw, ph, ppp);
                    #[cfg(not(target_os = "windows"))]
                    let popup_hwnd = 0isize;

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
                            paste::paste_entry_to_target(self.target_hwnd, &entry);
                        } else {
                            paste::populate_clipboard(&entry);
                        }
                    }
                    PopupAction::PastePlain(text) => {
                        close_popup = true;
                        paste::paste_plain_to_target(self.target_hwnd, &text);
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

fn dist_to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let ab_x = b.0 - a.0;
    let ab_y = b.1 - a.1;
    let ap_x = p.0 - a.0;
    let ap_y = p.1 - a.1;
    let ab_len_sq = ab_x * ab_x + ab_y * ab_y;
    if ab_len_sq < 1e-6 {
        return (ap_x * ap_x + ap_y * ap_y).sqrt();
    }
    let t = ((ap_x * ab_x + ap_y * ab_y) / ab_len_sq).clamp(0.0, 1.0);
    let proj_x = a.0 + t * ab_x;
    let proj_y = a.1 + t * ab_y;
    let dx = p.0 - proj_x;
    let dy = p.1 - proj_y;
    (dx * dx + dy * dy).sqrt()
}

fn create_app_icon(accent: egui::Color32, ink: egui::Color32) -> egui::IconData {
    let width = 64;
    let height = 64;
    let mut rgba = vec![0u8; width * height * 4];

    let outlines = windup_ui::lucide::flatten(windup_ui::Icon::Wrench.nodes());

    // Match the exact palette from the app's mark:
    // Accent background with accent_ink wrench!
    let bg_color = [accent.r() as f32, accent.g() as f32, accent.b() as f32];
    let ink_color = [ink.r() as f32, ink.g() as f32, ink.b() as f32];

    let icon_box_size = 36.0;
    let icon_scale = icon_box_size / 24.0;
    let icon_offset_x = (width as f32 - icon_box_size) * 0.5;
    let icon_offset_y = (height as f32 - icon_box_size) * 0.5;

    let inset = 4.0;
    let corner_r = 12.0;
    let half_w = (width as f32 - inset * 2.0) * 0.5;
    let center = width as f32 * 0.5;

    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) * 4;
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;

            // Rounded rectangle distance
            let rx = (px - center).abs() - (half_w - corner_r);
            let ry = (py - center).abs() - (half_w - corner_r);
            let d_outside = ((rx.max(0.0).powi(2) + ry.max(0.0).powi(2)).sqrt() - corner_r)
                .max(rx.max(ry).min(0.0) - corner_r);

            let bg_alpha = (0.5 - d_outside).clamp(0.0, 1.0);
            if bg_alpha <= 0.0 {
                continue;
            }

            // Lucide wrench stroke
            let bx = (px - icon_offset_x) / icon_scale;
            let by = (py - icon_offset_y) / icon_scale;

            let mut min_dist = f32::MAX;
            for outline in &outlines {
                let pts = &outline.points;
                let n = pts.len();
                if n < 2 {
                    continue;
                }
                for i in 0..n {
                    if i + 1 == n && !outline.closed {
                        break;
                    }
                    let p1 = pts[i];
                    let p2 = pts[(i + 1) % n];
                    let dist = dist_to_segment((bx, by), (p1.x, p1.y), (p2.x, p2.y));
                    if dist < min_dist {
                        min_dist = dist;
                    }
                }
            }

            // Lucide stroke width is 2.0 in 24x24 box (stroke radius = 1.0)
            let stroke_radius = 1.0;
            let pixel_dist = (min_dist - stroke_radius) * icon_scale;
            let wrench_alpha = (0.5 - pixel_dist).clamp(0.0, 1.0);

            // Blend dark wrench ink over accent background
            let r = bg_color[0] * (1.0 - wrench_alpha) + ink_color[0] * wrench_alpha;
            let g = bg_color[1] * (1.0 - wrench_alpha) + ink_color[1] * wrench_alpha;
            let b = bg_color[2] * (1.0 - wrench_alpha) + ink_color[2] * wrench_alpha;

            rgba[idx] = r.round() as u8;
            rgba[idx + 1] = g.round() as u8;
            rgba[idx + 2] = b.round() as u8;
            rgba[idx + 3] = (bg_alpha * 255.0).round() as u8;
        }
    }

    egui::IconData {
        rgba,
        width: width as u32,
        height: height as u32,
    }
}

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().filter_or("RUST_LOG", "warn,wgpu_hal=error,wgpu_core=error"),
    )
    .init();

    let config = Config::load();
    let mode = match config.theme_mode {
        ThemeMode::Dark => Mode::Dark,
        ThemeMode::Light => Mode::Light,
        ThemeMode::System => Mode::Dark,
    };
    let initial_palette = if let Some(ref custom_id) = config.custom_theme
        && let Some(themes_dir) = Config::themes_dir()
    {
        let (custom_themes, _) = windup_ui::themelib::load_all_from_dir(&themes_dir);
        custom_themes
            .into_iter()
            .find(|t| &t.id == custom_id)
            .map(|t| t.palette)
            .unwrap_or_else(|| Palette::of(mode, config.accent_hue))
    } else {
        Palette::of(mode, config.accent_hue)
    };

    let app_icon = create_app_icon(initial_palette.accent, initial_palette.accent_ink);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Windup")
            .with_icon(app_icon)
            .with_inner_size([960.0, 620.0])
            .with_min_inner_size([720.0, 480.0])
            .with_transparent(true),
        ..Default::default()
    };

    eframe::run_native(
        "Windup",
        options,
        Box::new(|cc| Ok(Box::new(WindupApp::new(cc)))),
    )
}
