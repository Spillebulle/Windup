//! The capture overlay on the viewport host (Windows and X11): the frozen
//! screen in one borderless, always-on-top viewport per monitor, each drawing
//! the shared [`CaptureView`], and what happens to the picture afterwards.
//! Also the notice card that says why a capture could not start or save.
//!
//! The Wayland hosts never come here: they have no eframe viewports to put
//! an overlay in. Their capture is `linux::start_capture` (Task 19).
//!
//! Every overlay is an immediate viewport, drawn inside the root viewport's
//! `update`. A repaint any overlay asks for is routed by eframe to the root
//! (an immediate viewport cannot paint by itself), and the root's update
//! draws every overlay, so all of them repaint together: ants keep marching
//! on the monitor the pointer is not on.

use crate::platform::{Host, Target};
use crate::{WindupApp, capture, cursor, paste};
use eframe::egui;
use image::RgbaImage;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use windup_core::ToolId;
use windup_core::capture::DRect;
use windup_core::capture::output;
use windup_core::capture::session::{CaptureMode, Outcome, Session};
use windup_core::config::ScreenshotConfig;
use windup_core::placement::{self, Area};
use windup_ui::capture::CaptureView;
use windup_ui::{Palette, notice};

/// How many taken names a save steps past before it gives up.
const SAVE_TRIES: usize = 16;
/// The frame by which the overlay under the pointer stops asking for the
/// keyboard if it still does not have it.
const FOCUS_FRAMES: u32 = 12;
/// The notice's window title (also how Win32 finds its window).
const NOTICE_TITLE: &str = "Windup notice";

/// Capture state the app carries between frames.
#[derive(Default)]
pub(crate) struct Overlay {
    capture: Option<Capture>,
    /// An outcome whose overlays were hidden last frame: handled this frame,
    /// once they are off the screen.
    finishing: Option<Outcome>,
    notice: Option<Notice>,
}

struct Capture {
    session: Session,
    view: CaptureView,
    surfaces: Vec<Surface>,
    /// The overlay that takes the keyboard: the one under the pointer.
    focus: usize,
    /// Frames drawn; the first is drawn invisible.
    frames: u32,
    /// The window that had focus at the hotkey, given it back at the end.
    target: Target,
}

#[derive(Clone)]
struct Surface {
    /// The monitor's layout: physical pixels (Windows virtual screen, X11 root).
    layout: DRect,
    /// The viewport's pixels per point: the monitor's scale times egui's zoom.
    ppp: f32,
    title: String,
}

struct Notice {
    message: String,
    rect: egui::Rect,
    frames: u32,
    had_focus: bool,
}

/// What a capture overlay does on its `frame`th frame.
///
/// Frame 0 is laid out at the scale the window was created with, and the
/// placement then moves it, maybe onto another DPI, whose resize winit only
/// delivers after that frame. So frame 0 stays hidden: it only places the
/// window. Frame 1 is laid out at the right scale and asks to be shown and
/// focused; egui carries out both after that frame is painted. From frame 2
/// the window is on the screen, and Win32 may push it to the foreground.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Steps {
    /// Set the window up while it is hidden (kept out of Alt+Tab).
    pub setup: bool,
    /// The builder's visibility.
    pub visible: bool,
    /// Send `Visible(true)` and `Focus`.
    pub show: bool,
    /// May take the foreground with Win32 (only while it lacks focus).
    pub force_focus: bool,
}

pub(crate) fn steps(frame: u32) -> Steps {
    Steps {
        setup: frame == 0,
        visible: frame >= 1,
        show: frame == 1,
        force_focus: (2..FOCUS_FRAMES).contains(&frame),
    }
}

/// A monitor's layout in egui points, at `ppp` pixels per point.
pub(crate) fn points_rect(layout: DRect, ppp: f32) -> egui::Rect {
    let ppp = if ppp > 0.0 { ppp } else { 1.0 };
    egui::Rect::from_min_size(
        egui::pos2(layout.x / ppp, layout.y / ppp),
        egui::vec2(layout.w / ppp, layout.h / ppp),
    )
}

/// What an outcome asks for: (copy, save).
pub(crate) fn wants(outcome: &Outcome) -> (bool, bool) {
    match outcome {
        Outcome::Copy(_) => (true, false),
        Outcome::Save(_) => (false, true),
        Outcome::CopyAndSave(_) => (true, true),
        Outcome::Cancel => (false, false),
    }
}

/// What the next capture starts with: this one's target, colour and sizes.
pub(crate) fn remember(cfg: &mut ScreenshotConfig, session: &Session) {
    cfg.last_target = session.target;
    cfg.colour = session.colour;
    cfg.sizes = session.sizes.clone();
}

/// Writes `bytes` to a new file in `dir`, named by `pick(dir, name)`. The file
/// is created with `create_new`, so an existing file is never overwritten:
/// when the name was taken between picking and creating, `pick` is asked
/// again, at most [`SAVE_TRIES`] times.
pub(crate) fn write_new(
    dir: &Path,
    name: &str,
    bytes: &[u8],
    mut pick: impl FnMut(&Path, &str) -> PathBuf,
) -> io::Result<PathBuf> {
    for _ in 0..SAVE_TRIES {
        let path = pick(dir, name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                if let Err(e) = file.write_all(bytes).and_then(|()| file.sync_all()) {
                    drop(file);
                    let _ = std::fs::remove_file(&path);
                    return Err(e);
                }
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "every free name was taken before it could be used",
    ))
}

/// Encodes `img` and saves it where the config says, creating the folder.
/// The error is the text for the notice.
pub(crate) fn save_image(img: &RgbaImage, cfg: &ScreenshotConfig) -> Result<PathBuf, String> {
    let dir =
        cfg.save_dir.clone().or_else(output::default_dir).ok_or(
            "The screenshot could not be saved: there is no Pictures folder to save it in.",
        )?;
    std::fs::create_dir_all(&dir).map_err(|e| {
        format!(
            "The screenshot could not be saved: the folder {} could not be made ({e}).",
            dir.display()
        )
    })?;
    let bytes = output::encode(img, cfg.format)
        .map_err(|e| format!("The screenshot could not be encoded ({e})."))?;
    let name = output::file_name_now(&cfg.name_pattern, cfg.format);
    write_new(&dir, &name, &bytes, output::unique_path).map_err(|e| {
        format!(
            "The screenshot could not be saved in {} ({e}).",
            dir.display()
        )
    })
}

impl WindupApp {
    /// A capture hotkey. Takes the picture first, while nothing of Windup's
    /// is on the screen but what was already there, then opens the overlays.
    /// Ignored while a capture is open.
    pub(crate) fn start_capture(
        &mut self,
        ctx: &egui::Context,
        mode: CaptureMode,
        tool: ToolId,
        target: Target,
    ) {
        let o = &self.overlay;
        if o.capture.is_some() || o.finishing.is_some() {
            log::debug!("capture: already open");
            return;
        }
        if !self.config.is_tool_enabled(tool) {
            return;
        }
        if self.host != Host::Viewport {
            // Task 19: the Wayland hosts capture through `linux::start_capture`.
            log::debug!("capture: no overlay on this host");
            return;
        }
        // A notice still up would be in the picture.
        if self.overlay.notice.take().is_some() {
            crate::hide_popup_window(NOTICE_TITLE);
            crate::reset_popup_clipping_cache();
        }

        let shot = match capture::grab_all() {
            Ok(shot) if !shot.monitors.is_empty() => shot,
            Ok(_) => {
                self.open_notice(ctx, capture::CaptureError::Unsupported.message());
                return;
            }
            Err(e) => {
                if let capture::CaptureError::Failed(why) = &e {
                    log::warn!("capture: {why}");
                }
                self.open_notice(ctx, e.message());
                return;
            }
        };
        let windows = capture::window_rects(&shot);
        let mut session = Session::new(shot, windows, &self.config.screenshot);
        session.mode = mode;

        // Desktop units are the cursor's own: physical pixels on Windows,
        // root pixels on X11. Seeding the pointer puts the bar on its monitor
        // from the first frame.
        let (cx, cy) = cursor::get_cursor_position();
        session.pointer = Some([cx, cy]);
        let focus = session.shot.monitor_at(cx, cy).unwrap_or(0);

        let fallback = ctx.native_pixels_per_point().unwrap_or(1.0);
        let zoom = ctx.zoom_factor();
        let surfaces = session
            .shot
            .monitors
            .iter()
            .enumerate()
            .map(|(i, m)| Surface {
                layout: m.layout,
                ppp: monitor_scale(m.layout).unwrap_or(fallback) * zoom,
                title: format!("Windup capture {}", i + 1),
            })
            .collect();

        let mut view = CaptureView::new();
        view.reduced_motion = reduced_motion();
        self.overlay.capture = Some(Capture {
            session,
            view,
            surfaces,
            focus,
            frames: 0,
            target,
        });
        ctx.request_repaint();
    }

    /// Draws the overlays and the notice, and acts on an outcome. Every
    /// frame, from `update`.
    pub(crate) fn drive_capture(&mut self, ctx: &egui::Context, p: &Palette) {
        self.drive_notice(ctx, p);

        if let Some(outcome) = self.overlay.finishing.take() {
            self.finish(ctx, outcome);
        }

        let Some(cap) = self.overlay.capture.as_mut() else {
            return;
        };
        let keys = self.config.screenshot.keys.clone();
        let Capture {
            session,
            view,
            surfaces,
            focus,
            frames,
            ..
        } = cap;
        let step = steps(*frames);
        let mut outcome = None;
        for (i, surface) in surfaces.iter_mut().enumerate() {
            let rect = points_rect(surface.layout, surface.ppp);
            let builder = egui::ViewportBuilder::default()
                .with_title(surface.title.clone())
                .with_icon(self.app_icon.clone())
                .with_decorations(false)
                .with_resizable(false)
                .with_always_on_top()
                .with_taskbar(false)
                .with_active(i == *focus)
                .with_visible(step.visible)
                .with_position(rect.min)
                .with_inner_size(rect.size());
            let id = egui::ViewportId::from_hash_of(("capture", i));
            ctx.show_viewport_immediate(id, builder, |vctx, class| {
                if class != egui::ViewportClass::Immediate {
                    return;
                }
                let mine = i == *focus;
                let unfocused = vctx.input(|inp| inp.viewport().focused) != Some(true);
                if step.setup {
                    make_tool_window(&surface.title);
                }
                place_overlay(
                    &surface.title,
                    surface.layout,
                    mine && unfocused && step.force_focus,
                );
                if step.show {
                    // Both are carried out after this frame is painted.
                    vctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    if mine {
                        vctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                }
                // Windows is placed exactly by `place_overlay`. Elsewhere the
                // window may land at another scale than guessed: re-derive the
                // points from the scale it got, and egui moves it.
                #[cfg(not(target_os = "windows"))]
                {
                    let actual = vctx.pixels_per_point();
                    if (actual - surface.ppp).abs() > 1e-3 {
                        surface.ppp = actual;
                    }
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(vctx, |ui| {
                        if let Some(o) = view.monitor_ui(ui, session, i, p, &keys) {
                            outcome = Some(o);
                        }
                    });
            });
        }
        *frames += 1;
        if *frames < FOCUS_FRAMES {
            ctx.request_repaint();
        }

        let settings = view.take_open_settings();
        if outcome.is_none() && !settings {
            return;
        }
        let Some(cap) = self.overlay.capture.take() else {
            return;
        };
        // Close first: hide every overlay now, and handle the picture next
        // frame, when they are off the screen.
        if !settings {
            give_focus_back(cap.target);
        }
        for (i, s) in cap.surfaces.iter().enumerate() {
            hide_overlay(&s.title);
            ctx.send_viewport_cmd_to(
                egui::ViewportId::from_hash_of(("capture", i)),
                egui::ViewportCommand::Visible(false),
            );
        }
        remember(&mut self.config.screenshot, &cap.session);
        self.config.save();
        if settings {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        } else {
            self.overlay.finishing = outcome;
        }
        ctx.request_repaint();
    }

    /// Copies and saves the picture. A failure is shown in the notice.
    fn finish(&mut self, ctx: &egui::Context, outcome: Outcome) {
        let (copy, save) = wants(&outcome);
        let img = match outcome {
            Outcome::Copy(img) | Outcome::Save(img) | Outcome::CopyAndSave(img) => img,
            Outcome::Cancel => return,
        };
        let mut errors = Vec::new();
        if save {
            match save_image(&img, &self.config.screenshot) {
                Ok(path) => log::debug!("capture: saved {}", path.display()),
                Err(e) => errors.push(e),
            }
        }
        // Windup's own clipboard monitor records it in the history.
        if copy && !paste::set_clipboard_image(img) {
            errors.push("The screenshot could not be copied to the clipboard.".to_owned());
        }
        if !errors.is_empty() {
            self.open_notice(ctx, &errors.join("\n"));
        }
    }

    /// Opens the notice card at the cursor, the way the clipboard popup opens.
    fn open_notice(&mut self, ctx: &egui::Context, message: &str) {
        let palette = self.resolved_palette(self.resolved_mode(ctx));
        let size = notice::size(ctx, message, &palette);
        let ((cx, cy), mon) = cursor::get_cursor_and_monitor();
        let ppp = ctx.pixels_per_point().max(1.0);
        let area = Area {
            left: mon.left / ppp,
            top: mon.top / ppp,
            right: mon.right / ppp,
            bottom: mon.bottom / ppp,
        };
        let (x, y) = placement::place((cx / ppp, cy / ppp), area, (size.x, size.y));
        self.overlay.notice = Some(Notice {
            message: message.to_owned(),
            rect: egui::Rect::from_min_size(egui::pos2(x, y), size),
            frames: 0,
            had_focus: false,
        });
        ctx.request_repaint();
    }

    fn drive_notice(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(n) = self.overlay.notice.as_mut() else {
            return;
        };
        let builder = egui::ViewportBuilder::default()
            .with_title(NOTICE_TITLE)
            .with_icon(self.app_icon.clone())
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(true)
            .with_visible(n.frames >= 2)
            .with_inner_size(n.rect.size())
            .with_position(n.rect.min);
        let mut close = false;
        let id = egui::ViewportId::from_hash_of("capture_notice");
        ctx.show_viewport_immediate(id, builder, |vctx, class| {
            if class != egui::ViewportClass::Immediate {
                return;
            }
            // As the overlays, one frame later: frames 0 and 1 are painted
            // hidden, so the first one shown is laid out at the window's own
            // scale. The popup's clip-and-focus shows the window (after
            // setting its region), so it waits for frame 2.
            if n.frames == 0 {
                make_tool_window(NOTICE_TITLE);
            }
            if n.frames >= 2 && !n.had_focus {
                let size = n.rect.size();
                crate::focus_and_clip_popup_window(
                    NOTICE_TITLE,
                    size.x,
                    size.y,
                    vctx.pixels_per_point(),
                );
            }
            if n.frames == 2 {
                vctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                vctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(egui::Color32::TRANSPARENT))
                .show(vctx, |ui| close |= notice::show(ui, p, &n.message));
            // Clicking elsewhere puts it away, as it does the popup.
            match vctx.input(|i| i.viewport().focused) {
                Some(true) => n.had_focus = true,
                Some(false) if n.had_focus => close = true,
                _ => {}
            }
        });
        n.frames += 1;
        if n.frames < FOCUS_FRAMES {
            ctx.request_repaint();
        }
        if close {
            self.overlay.notice = None;
            crate::hide_popup_window(NOTICE_TITLE);
            crate::reset_popup_clipping_cache();
        }
    }
}

/// Physical pixels: `x, y, width, height`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn physical(layout: DRect) -> (i32, i32, i32, i32) {
    let (x, y) = (layout.x.round() as i32, layout.y.round() as i32);
    (
        x,
        y,
        layout.right().round() as i32 - x,
        layout.bottom().round() as i32 - y,
    )
}

/// The monitor's scale (its DPI over 96) on Windows. Elsewhere `None`: the
/// window's own scale corrects the guess once it exists.
#[cfg(target_os = "windows")]
fn monitor_scale(layout: DRect) -> Option<f32> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    let (x, y, w, h) = physical(layout);
    let centre = POINT {
        x: x + w / 2,
        y: y + h / 2,
    };
    // SAFETY: plain queries; the out-parameters are locals.
    unsafe {
        let monitor = MonitorFromPoint(centre, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (0u32, 0u32);
        (GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) == 0 && dx > 0)
            .then(|| dx as f32 / 96.0)
    }
}

#[cfg(not(target_os = "windows"))]
fn monitor_scale(_layout: DRect) -> Option<f32> {
    None
}

/// Whether the OS asks for less motion. Windows' closest switch is "Show
/// animations in Windows" (client-area animation). X11 has no standard one.
#[cfg(target_os = "windows")]
fn reduced_motion() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
    };
    let mut on: windows_sys::core::BOOL = 1;
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL to the pointer.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            (&mut on as *mut windows_sys::core::BOOL).cast(),
            0,
        )
    };
    ok != 0 && on == 0
}

#[cfg(not(target_os = "windows"))]
fn reduced_motion() -> bool {
    false
}

/// Our own top-level window with this title.
#[cfg(target_os = "windows")]
fn own_window(title: &str) -> Option<windows_sys::Win32::Foundation::HWND> {
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId};
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), wide.as_ptr());
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        (pid == GetCurrentProcessId()).then_some(hwnd)
    }
}

/// Puts the overlay exactly on its monitor in physical pixels, whatever
/// winit made of the points, and takes the foreground for it when asked.
/// Never shows it: see [`steps`]. Moving a window onto a monitor of another
/// scale makes winit resize it for the new DPI, so the move is repeated
/// until it holds.
#[cfg(target_os = "windows")]
fn place_overlay(title: &str, layout: DRect, focus: bool) {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowRect, HWND_TOPMOST, SWP_NOACTIVATE, SetWindowPos,
    };
    let Some(hwnd) = own_window(title) else {
        return;
    };
    let (x, y, w, h) = physical(layout);
    // SAFETY: `hwnd` is one of this process's windows, found just now on
    // this (its own) thread.
    unsafe {
        for _ in 0..3 {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut r);
            if (r.left, r.top, r.right - r.left, r.bottom - r.top) == (x, y, w, h) {
                break;
            }
            SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE);
        }
        if focus {
            force_foreground(hwnd);
        }
    }
}

/// X11: egui's focus request is an application's, which window managers may
/// refuse; ask again as a pager does.
#[cfg(target_os = "linux")]
fn place_overlay(title: &str, _layout: DRect, focus: bool) {
    if focus && let Some(w) = crate::linux::x11::window_named(title) {
        crate::linux::x11::activate(w);
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn place_overlay(_title: &str, _layout: DRect, _focus: bool) {}

/// Keeps the window out of Alt+Tab. winit's `with_taskbar(false)` only takes
/// the taskbar button away (through `ITaskbarList`).
#[cfg(target_os = "windows")]
fn make_tool_window(title: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    };
    if let Some(hwnd) = own_window(title) {
        // SAFETY: one of this process's windows; its other ex-styles are kept.
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let tool = (ex | WS_EX_TOOLWINDOW as isize) & !(WS_EX_APPWINDOW as isize);
            if tool != ex {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, tool);
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn make_tool_window(_title: &str) {}

/// Foreground and keyboard focus for `hwnd`, with the input of the current
/// foreground thread attached so Windows allows it (as the popup does).
#[cfg(target_os = "windows")]
unsafe fn force_foreground(hwnd: windows_sys::Win32::Foundation::HWND) {
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
    };
    // SAFETY: the caller passes a live window of this thread.
    unsafe {
        let fg = GetForegroundWindow();
        let me = GetCurrentThreadId();
        let theirs = GetWindowThreadProcessId(fg, std::ptr::null_mut());
        let attach = theirs != 0 && theirs != me;
        if attach {
            AttachThreadInput(me, theirs, 1);
        }
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
        if attach {
            AttachThreadInput(me, theirs, 0);
        }
    }
}

/// Hides the overlay now rather than when egui drops its viewport at the
/// end of the next frame.
#[cfg(target_os = "windows")]
fn hide_overlay(title: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};
    if let Some(hwnd) = own_window(title) {
        // SAFETY: one of this process's windows.
        unsafe { ShowWindow(hwnd, SW_HIDE) };
    }
}

#[cfg(not(target_os = "windows"))]
fn hide_overlay(_title: &str) {}

/// Focus back to the window the hotkey was pressed in, before the overlay
/// that has it goes away (Windows lets the foreground window hand it on).
fn give_focus_back(target: Target) {
    match target {
        #[cfg(target_os = "windows")]
        Target::Hwnd(h) if h != 0 => {
            use windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
            // SAFETY: a stale handle only makes the call fail.
            unsafe { SetForegroundWindow(h as windows_sys::Win32::Foundation::HWND) };
        }
        #[cfg(target_os = "linux")]
        Target::X11(w) => crate::linux::x11::activate(w),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windup_core::capture::DesktopShot;
    use windup_core::config::{ImageFormat, TargetKind, ToolSizes};

    fn rect(x: f32, y: f32, w: f32, h: f32) -> DRect {
        DRect { x, y, w, h }
    }

    /// A fresh, empty folder under the temp dir.
    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("windup-overlay-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_monitor_left_of_the_primary_at_2x_is_half_as_many_points() {
        let r = points_rect(rect(-2560.0, -200.0, 2560.0, 1440.0), 2.0);
        assert_eq!(r.min, egui::pos2(-1280.0, -100.0));
        assert_eq!(r.size(), egui::vec2(1280.0, 720.0));
    }

    #[test]
    fn points_times_the_scale_give_back_the_physical_rect() {
        for ppp in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let l = rect(1920.0, 0.0, 3840.0, 2160.0);
            let r = points_rect(l, ppp);
            assert!((r.min.x * ppp - l.x).abs() < 1e-3, "{ppp}");
            assert!((r.width() * ppp - l.w).abs() < 1e-3, "{ppp}");
            assert!((r.height() * ppp - l.h).abs() < 1e-3, "{ppp}");
        }
    }

    #[test]
    fn a_zero_scale_is_taken_as_one() {
        let r = points_rect(rect(10.0, 20.0, 30.0, 40.0), 0.0);
        assert_eq!(
            r,
            egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(30.0, 40.0))
        );
    }

    #[test]
    fn physical_rounds_edges_not_sizes() {
        assert_eq!(physical(rect(-0.4, 0.6, 100.4, 50.0)), (0, 1, 100, 50));
    }

    #[test]
    fn frame_0_is_hidden_frame_1_shows_and_win32_focus_waits_for_frame_2() {
        assert_eq!(
            steps(0),
            Steps {
                setup: true,
                visible: false,
                show: false,
                force_focus: false,
            }
        );
        let f1 = steps(1);
        assert!(f1.visible && f1.show && !f1.setup && !f1.force_focus);
        let f2 = steps(2);
        assert!(f2.visible && !f2.show && f2.force_focus);
        assert!(steps(FOCUS_FRAMES - 1).force_focus);
        assert!(!steps(FOCUS_FRAMES).force_focus);
        assert!(steps(FOCUS_FRAMES).visible);
    }

    #[test]
    fn outcomes_map_to_copy_and_save() {
        let img = || RgbaImage::new(1, 1);
        assert_eq!(wants(&Outcome::Copy(img())), (true, false));
        assert_eq!(wants(&Outcome::Save(img())), (false, true));
        assert_eq!(wants(&Outcome::CopyAndSave(img())), (true, true));
        assert_eq!(wants(&Outcome::Cancel), (false, false));
    }

    #[test]
    fn the_session_choices_are_remembered() {
        let mut cfg = ScreenshotConfig::default();
        let mut s = Session::new(DesktopShot { monitors: vec![] }, vec![], &cfg);
        s.target = TargetKind::Window;
        s.colour = [1, 2, 3, 255];
        s.sizes = ToolSizes {
            pen: 9.0,
            ..ToolSizes::default()
        };
        remember(&mut cfg, &s);
        assert_eq!(cfg.last_target, TargetKind::Window);
        assert_eq!(cfg.colour, [1, 2, 3, 255]);
        assert_eq!(cfg.sizes.pen, 9.0);
    }

    #[test]
    fn a_name_taken_after_it_was_picked_moves_on_to_the_next() {
        let dir = temp("race");
        // Another process got there between picking and creating.
        std::fs::write(dir.join("a.png"), b"theirs").unwrap();
        let mut calls = 0;
        let path = write_new(&dir, "a.png", b"ours", |d, n| {
            calls += 1;
            if calls == 1 {
                d.join(n)
            } else {
                output::unique_path(d, n)
            }
        })
        .unwrap();
        assert_eq!(path, dir.join("a (2).png"));
        assert_eq!(std::fs::read(&path).unwrap(), b"ours");
        assert_eq!(std::fs::read(dir.join("a.png")).unwrap(), b"theirs");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_name_that_stays_taken_gives_up_without_overwriting() {
        let dir = temp("taken");
        std::fs::write(dir.join("a.png"), b"theirs").unwrap();
        let mut calls = 0;
        let e = write_new(&dir, "a.png", b"ours", |d, n| {
            calls += 1;
            d.join(n)
        })
        .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(calls, SAVE_TRIES);
        assert_eq!(std::fs::read(dir.join("a.png")).unwrap(), b"theirs");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saving_makes_the_folder_and_writes_a_picture_of_the_selection() {
        let dir = temp("save");
        let cfg = ScreenshotConfig {
            save_dir: Some(dir.join("deeper").join("still")),
            name_pattern: "shot".into(),
            format: ImageFormat::Png,
            ..ScreenshotConfig::default()
        };
        let img = RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255]));
        let first = save_image(&img, &cfg).unwrap();
        let second = save_image(&img, &cfg).unwrap();
        assert_eq!(first, dir.join("deeper").join("still").join("shot.png"));
        assert_eq!(
            second,
            dir.join("deeper").join("still").join("shot (2).png")
        );
        let back = image::open(&first).unwrap().to_rgba8();
        assert_eq!(back, img);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_that_cannot_be_made_is_an_error_for_the_notice() {
        let dir = temp("blocked");
        // A file where the folder should be.
        std::fs::write(dir.join("file"), b"x").unwrap();
        let cfg = ScreenshotConfig {
            save_dir: Some(dir.join("file").join("sub")),
            ..ScreenshotConfig::default()
        };
        let e = save_image(&RgbaImage::new(1, 1), &cfg).unwrap_err();
        assert!(e.starts_with("The screenshot could not be saved"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
