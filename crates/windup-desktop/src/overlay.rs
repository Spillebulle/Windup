//! The capture overlay on the viewport host (Windows and X11): the frozen
//! screen in one borderless, always-on-top viewport per monitor, each drawing
//! the shared [`CaptureView`], and what happens to the picture afterwards.
//! Also the notice card that says why a capture could not start or save,
//! or where a recording went. The recording itself is `recording.rs`.
//!
//! The Wayland hosts never come here: they have no eframe viewports to put
//! an overlay in. Their capture is `linux::capture`, with the overlay on the
//! Wayland thread. Both hand the outcome to `capture::finish`.
//!
//! Every overlay is an immediate viewport, drawn inside the root viewport's
//! `update`. A repaint any overlay asks for is routed by eframe to the root
//! (an immediate viewport cannot paint by itself), and the root's update
//! draws every overlay, so all of them repaint together: ants keep marching
//! on the monitor the pointer is not on.

use crate::platform::{Host, Target};
use crate::{WindupApp, capture, cursor, paste};
use capture::finish::{self, remember, remember_steps, remember_video};
use eframe::egui;
use windup_core::ToolId;
use windup_core::capture::DRect;
use windup_core::capture::session::{Action, CaptureMode, Outcome, Session};
use windup_core::placement::{self, Area};
use windup_ui::capture::CaptureView;
use windup_ui::{Palette, notice};

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

impl Overlay {
    /// Whether a capture or its notice is on the screen.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn busy(&self) -> bool {
        self.capture.is_some() || self.finishing.is_some() || self.notice.is_some()
    }
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
    /// A saved recording, which "Open folder" shows.
    action: Option<std::path::PathBuf>,
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

impl WindupApp {
    /// A capture hotkey. Takes the picture first, while nothing of Windup's
    /// is on the screen but what was already there, then opens the overlays.
    /// Ignored while a capture is open or a recording runs.
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
        if self.rec.live.busy() {
            log::debug!("capture: a recording is under way");
            return;
        }
        if !self.config.is_tool_enabled(tool) {
            return;
        }
        if self.host != Host::Viewport {
            // The Wayland hosts capture in `linux::capture`.
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
                self.open_notice(ctx, capture::CaptureError::Unsupported.message(), None);
                return;
            }
            Err(e) => {
                if let capture::CaptureError::Failed(why) = &e {
                    log::warn!("capture: {why}");
                }
                self.open_notice(ctx, e.message(), None);
                return;
            }
        };
        let windows = capture::window_rects(&shot);
        // The UI thread: the sound devices as last listed, never listed here.
        let availability = crate::video::availability::cached(&self.config.video);
        let mut session = Session::new(
            shot,
            windows,
            &self.config.screenshot,
            &self.config.video,
            availability,
        );
        session.set_steps(&self.config.steps);
        // Refused, and left on Screenshot, when Video or Steps cannot work here.
        session.act(Action::SetMode(mode));

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
                if fullscreen_now(step) {
                    fullscreen_overlay(&surface.title);
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
        if let Some(Outcome::Record { rect, choice }) = &outcome {
            remember_video(&mut self.config.video, &cap.session);
            // Started next frame, once the overlays are off the screen.
            self.record_from(*rect, *choice, &cap.session);
            outcome = None;
        }
        if let Some(Outcome::Steps { choice }) = &outcome {
            remember_steps(&mut self.config.steps, &cap.session);
            // Started here once Steps has a runtime (Task 14).
            log::info!("capture: steps asked for ({choice:?}), not yet run");
            outcome = None;
        }
        if let Some(Outcome::StepDone(img)) = &outcome {
            // Written back to the step by the review (Task 14).
            log::info!("capture: step edited ({}x{})", img.width(), img.height());
            outcome = None;
        }
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
        // Windup's own clipboard monitor records the copy in the history.
        if let Some(error) =
            finish::deliver(outcome, &self.config.screenshot, paste::set_clipboard_image)
        {
            self.open_notice(ctx, &error, None);
        }
    }

    /// Opens the notice card at the cursor, the way the clipboard popup opens,
    /// with "Open folder" when there is a saved file to show.
    pub(crate) fn open_notice(
        &mut self,
        ctx: &egui::Context,
        message: &str,
        action: Option<std::path::PathBuf>,
    ) {
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
            action,
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
        let mut act = false;
        let label = n
            .action
            .as_ref()
            .map(|_| crate::video::outcome::OPEN_FOLDER);
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
                .show(vctx, |ui| match notice::show(ui, p, &n.message, label) {
                    Some(notice::NoticeReply::Action) => act = true,
                    Some(notice::NoticeReply::Dismissed) => close = true,
                    None => {}
                });
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
        if act && let Some(file) = n.action.as_deref() {
            crate::video::outcome::open_folder(file);
        }
        if close || act {
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
pub(crate) fn monitor_scale(layout: DRect) -> Option<f32> {
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
pub(crate) fn monitor_scale(_layout: DRect) -> Option<f32> {
    None
}

/// Whether the OS asks for less motion. Windows' closest switch is "Show
/// animations in Windows" (client-area animation). X11 has no standard one.
#[cfg(target_os = "windows")]
pub(crate) fn reduced_motion() -> bool {
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
pub(crate) fn reduced_motion() -> bool {
    false
}

/// Our own top-level window with this title.
#[cfg(target_os = "windows")]
pub(crate) fn own_window(title: &str) -> Option<windows_sys::Win32::Foundation::HWND> {
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
pub(crate) fn place_overlay(title: &str, layout: DRect, focus: bool) {
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
pub(crate) fn place_overlay(title: &str, _layout: DRect, focus: bool) {
    if focus && let Some(w) = crate::linux::x11::window_named(title) {
        crate::linux::x11::activate(w);
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub(crate) fn place_overlay(_title: &str, _layout: DRect, _focus: bool) {}

/// How an X11 overlay asks to be fullscreen and above the panels (EWMH
/// `_NET_WM_STATE`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NetWmState {
    /// Still unmapped: the window states it in its own property, which the
    /// window manager reads when it maps it.
    Property([u32; 2]),
    /// Mapped: the window manager owns the property; ask it with a client
    /// message to the root (`_NET_WM_STATE_ADD`, the two atoms, source 1).
    Message([u32; 5]),
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn fullscreen_request(mapped: bool, fullscreen: u32, above: u32) -> NetWmState {
    const ADD: u32 = 1;
    const SOURCE_APPLICATION: u32 = 1;
    if mapped {
        NetWmState::Message([ADD, fullscreen, above, SOURCE_APPLICATION, 0])
    } else {
        NetWmState::Property([fullscreen, above])
    }
}

/// The frames on which an X11 overlay asks for fullscreen: once while
/// hidden (the property is in place before the map), then on each frame
/// after it is shown while focus is still being settled (the window manager
/// may map it a frame or two late; the request is idempotent).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn fullscreen_now(step: Steps) -> bool {
    step.setup || step.force_focus
}

/// X11: makes the overlay cover its monitor, panels included. Mutter (GNOME
/// on Xorg), Xfwm, Cinnamon and KWin keep an ordinary window, however it is
/// sized, inside the work area, which would shift or squeeze the frozen
/// picture against the live screen.
///
/// EWMH fullscreen rather than the alternatives: winit's borderless
/// fullscreen picks its monitor before the window is mapped and is deferred
/// until it is visible, so it fights the hidden-first-frame dance; and
/// override-redirect cannot be set through eframe's viewport builder and
/// gets no keyboard focus from the window manager. Every window manager puts
/// a fullscreen window on the monitor that holds it, which the position
/// already chose, so `_NET_WM_FULLSCREEN_MONITORS` (by Xinerama index, an
/// order RandR's list need not share) is not needed.
#[cfg(target_os = "linux")]
fn fullscreen_overlay(title: &str) {
    crate::linux::x11::fullscreen_above(title);
}

#[cfg(not(target_os = "linux"))]
fn fullscreen_overlay(_title: &str) {}

/// Keeps the window out of Alt+Tab. winit's `with_taskbar(false)` only takes
/// the taskbar button away (through `ITaskbarList`).
#[cfg(target_os = "windows")]
pub(crate) fn make_tool_window(title: &str) {
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
pub(crate) fn make_tool_window(_title: &str) {}

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
pub(crate) fn hide_overlay(title: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};
    if let Some(hwnd) = own_window(title) {
        // SAFETY: one of this process's windows.
        unsafe { ShowWindow(hwnd, SW_HIDE) };
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn hide_overlay(_title: &str) {}

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

    fn rect(x: f32, y: f32, w: f32, h: f32) -> DRect {
        DRect { x, y, w, h }
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
    fn a_hidden_overlay_states_fullscreen_and_above_in_its_property() {
        let (full, above) = (301, 302);
        assert_eq!(
            fullscreen_request(false, full, above),
            NetWmState::Property([full, above])
        );
    }

    #[test]
    fn a_mapped_overlay_asks_the_window_manager_to_add_both() {
        let (full, above) = (301, 302);
        // _NET_WM_STATE_ADD, both atoms, source 1 (an application).
        assert_eq!(
            fullscreen_request(true, full, above),
            NetWmState::Message([1, full, above, 1, 0])
        );
    }

    #[test]
    fn x11_fullscreen_is_set_up_hidden_and_asked_again_once_mapped() {
        assert!(fullscreen_now(steps(0)), "hidden: the property");
        assert!(!fullscreen_now(steps(1)));
        assert!(fullscreen_now(steps(2)), "mapped: the message");
        assert!(fullscreen_now(steps(FOCUS_FRAMES - 1)));
        assert!(!fullscreen_now(steps(FOCUS_FRAMES)), "not every frame");
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
}
