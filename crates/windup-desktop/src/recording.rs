//! A recording on the viewport host (Windows and X11): started from the
//! launcher's Record, its bar, and the notice at its end. The recording
//! itself is `video::live`; this is where the eframe app hosts it.
//!
//! The bar is a small borderless, always-on-top immediate viewport, drawn
//! like the notice: frames 0 and 1 hidden, shown from frame 2, never
//! focused (the recorded window keeps the keyboard). Its window is the bar
//! plus room under it for the tooltips (`recbar::host_size`), placed whole:
//! - Windows (`BarHost::ExcludedWindow`): where the launcher bar was (the
//!   top centre of the pointer's monitor), with the window left out of every
//!   capture by `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`, set on
//!   frame 0 while it is still hidden. Where that fails (Windows before
//!   2004) it moves outside the recorded area as on X11, or goes.
//! - X11 (`BarHost::Outside`): outside the recorded area (`place_bar`), or
//!   none when there is no room for it.
//!
//! On Windows the tooltip room lies over the recorded area, and a
//! transparent window still takes clicks, so the window's region
//! (`SetWindowRgn`) is the bar alone, widened to the room only while a
//! button is hovered and its tooltip due ([`hit_rect`]).

use crate::overlay::{
    hide_overlay, make_tool_window, monitor_scale, place_overlay, points_rect, reduced_motion,
};
use crate::platform::Target;
use crate::video::availability;
use crate::video::live::{Ask, LiveEvent};
use crate::{WindupApp, capture};
use eframe::egui;
use std::time::Duration;
use windup_core::ToolId;
use windup_core::capture::DRect;
use windup_core::capture::backend::{BarHost, record_bar_host};
use windup_core::capture::session::{CaptureMode, Session, VideoChoice};
use windup_core::video::{place_bar, top_centre};
use windup_ui::capture::recbar::{RecBarAction, RecBarState, RecBarView, bar_size, host_size};
use windup_ui::{Mode, Palette, metrics};

/// The bar's window title (also how Win32 finds its window).
const BAR_TITLE: &str = "Windup recording";
/// How often a recording that is starting or joining is looked at.
const POLL: Duration = Duration::from_millis(50);

/// Recording state the app carries between frames.
#[derive(Default)]
pub(crate) struct Rec {
    pub(crate) live: crate::video::live::Live,
    /// The launcher's Record, started next frame, once the overlays hidden
    /// this frame are off the screen.
    pending: Option<(Ask, BarPlan)>,
    plan: Option<BarPlan>,
    bar: Option<Bar>,
    /// The window was closed while recording: the recording was stopped,
    /// and the app closes once it is saved.
    quit_when_over: bool,
}

/// Whether a close of the main window waits for the recording: one that
/// would end the app (not just hide the window) while a recording is under
/// way. The app would otherwise drop it, and with it what was recorded.
pub(crate) fn hold_close(requested: bool, hides: bool, busy: bool) -> bool {
    requested && !hides && busy
}

/// What the bar is placed from, kept from the launcher.
#[derive(Clone, Debug)]
struct BarPlan {
    host: BarHost,
    /// The recorded area, and the monitors, in desktop units.
    rect: DRect,
    monitors: Vec<DRect>,
    /// The monitor the launcher bar was on.
    launcher: DRect,
}

struct Bar {
    view: RecBarView,
    frames: u32,
    /// Windows left it out of captures.
    excluded: bool,
    /// The window in desktop units.
    slot: DRect,
    /// Its scale, the monitor's times egui's zoom.
    ppp: f32,
    /// Its size in points, as last measured.
    size: egui::Vec2,
    /// The window region last set (Windows), in window pixels.
    region: Option<(i32, i32)>,
}

/// The part of the bar's window that takes the pointer, as a width and
/// height in window pixels from its top-left: the bar alone, or the whole
/// window (bar and tooltip room) while a tooltip is due. `bar` and `host`
/// are in points.
pub(crate) fn hit_rect(bar: egui::Vec2, host: egui::Vec2, tip: bool, ppp: f32) -> (i32, i32) {
    let size = if tip { host } else { bar };
    ((size.x * ppp).ceil() as i32, (size.y * ppp).ceil() as i32)
}

impl BarPlan {
    /// Where the bar's window goes in desktop units, or `None` for no bar.
    /// `size`, `top` (the launcher bar's distance from the monitor's top)
    /// and `gap` (from the recorded area) are in desktop units too.
    fn slot(&self, excluded: bool, size: (f32, f32), top: f32, gap: f32) -> Option<DRect> {
        match self.host {
            BarHost::ExcludedWindow if excluded => Some(top_centre(self.launcher, size, top)),
            BarHost::ExcludedWindow | BarHost::Outside => {
                place_bar(self.rect, &self.monitors, size, gap)
            }
            BarHost::Hidden => None,
        }
    }

    /// The slot for a window of `size` points at the scale of the monitor it
    /// lands on (`guess` where the OS does not say): placed at `guess`, then
    /// again if that monitor's scale differs. Returns the slot and its scale.
    fn place(
        &self,
        excluded: bool,
        size: egui::Vec2,
        guess: f32,
        zoom: f32,
    ) -> Option<(DRect, f32)> {
        let at = |ppp: f32| {
            self.slot(
                excluded,
                (size.x * ppp, size.y * ppp),
                metrics::S4 * ppp,
                metrics::REC_GAP * ppp,
            )
        };
        let first = at(guess)?;
        let ppp = monitor_scale(first).map_or(guess, |s| s * zoom);
        if (ppp - guess).abs() < 1e-3 {
            return Some((first, guess));
        }
        at(ppp).map(|slot| (slot, ppp))
    }
}

impl WindupApp {
    /// The launcher's Record: remembered, and started next frame.
    pub(crate) fn record_from(&mut self, rect: DRect, choice: VideoChoice, session: &Session) {
        let monitors: Vec<DRect> = session.shot.monitors.iter().map(|m| m.layout).collect();
        let launcher = session
            .pointer
            .and_then(|p| session.shot.monitor_at(p[0], p[1]))
            .and_then(|i| monitors.get(i).copied())
            .or_else(|| monitors.first().copied())
            .unwrap_or(rect);
        let plan = BarPlan {
            host: record_bar_host(&capture::facts()),
            rect,
            monitors,
            launcher,
        };
        let ask = Ask {
            rect,
            choice,
            cfg: self.config.video.clone(),
        };
        self.rec.pending = Some((ask, plan));
    }

    /// The Video hotkey: stops a recording that runs, else opens the
    /// launcher in Video, or says why Video cannot work here.
    pub(crate) fn video_hotkey(&mut self, ctx: &egui::Context, target: Target) {
        if self.rec.live.stoppable() {
            self.stop_recording();
            return;
        }
        if self.rec.live.busy() || !self.config.is_tool_enabled(ToolId::Video) {
            return;
        }
        if let Err(why) = availability::cached(&self.config.video).video {
            self.open_notice(ctx, why, None);
            return;
        }
        self.start_capture(ctx, CaptureMode::Video, ToolId::Video, target);
    }

    /// Stop, from the bar or a command. The bar goes at once; the notice
    /// comes once the file is joined.
    pub(crate) fn stop_recording(&mut self) {
        self.rec.live.stop();
        self.close_bar();
    }

    /// Holds a close of the main window while a recording is under way:
    /// the recording is stopped (a join already under way goes on), and
    /// the app closes when it is over. `hides` is a close that only hides
    /// the window (Linux with "close to tray"), which the recording outlives.
    pub(crate) fn hold_close_while_recording(&mut self, ctx: &egui::Context, hides: bool) {
        let requested = ctx.input(|i| i.viewport().close_requested());
        if !hold_close(requested, hides, self.rec.live.busy()) {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        if !self.rec.quit_when_over {
            log::info!("recording: the window was closed, stopping first");
        }
        self.rec.quit_when_over = true;
        self.stop_recording();
    }

    /// The pause key: pauses a running recording, or resumes it.
    pub(crate) fn pause_recording(&mut self) {
        if let Some(paused) = self.rec.live.toggle_pause() {
            log::info!("recording: {}", if paused { "pausing" } else { "resuming" });
        }
    }

    /// Esc: cancels the recording and discards it. The bar goes at once;
    /// the notice says so once the recording is gone.
    pub(crate) fn cancel_recording(&mut self) {
        self.rec.live.cancel();
        self.close_bar();
    }

    /// Starts, polls and draws the recording. Every frame, from `update`,
    /// before the capture overlays: a Record they gave this frame starts on
    /// the next, once they are hidden.
    pub(crate) fn drive_recording(&mut self, ctx: &egui::Context, p: &Palette, mode: Mode) {
        if let Some((ask, plan)) = self.rec.pending.take()
            && self.rec.live.start(ask)
        {
            self.rec.plan = Some(plan);
        }
        for event in self.rec.live.poll() {
            match event {
                LiveEvent::Started => self.open_bar(ctx),
                LiveEvent::Stopping => self.close_bar(),
                LiveEvent::Over(ended) => {
                    self.close_bar();
                    self.rec.plan = None;
                    self.open_notice(ctx, &ended.message, ended.saved);
                    if std::mem::take(&mut self.rec.quit_when_over) {
                        log::info!("recording: over, closing as asked");
                        ctx.send_viewport_cmd_to(
                            egui::ViewportId::ROOT,
                            egui::ViewportCommand::Close,
                        );
                    }
                }
                LiveEvent::Token(t) => {
                    windup_core::config::Config::save_screencast_token(t.as_deref())
                }
            }
        }
        self.draw_bar(ctx, p, mode);
        // Pause, stop and Esc are bound from the start until it is stopped
        // or cancelled.
        if let Some(listener) = &self.hotkey_listener {
            listener.set_recording(self.rec.live.keys_wanted(), &self.config);
        }
        if self.rec.live.busy() {
            ctx.request_repaint_after(POLL);
        }
    }

    fn bar_state(&self) -> Option<RecBarState> {
        let running = self.rec.live.running()?;
        Some(RecBarState {
            elapsed: running.clock.elapsed(),
            paused: running.clock.paused(),
            fps: running.fps,
            bitrate_label: running.label.clone(),
            reduced_motion: reduced_motion(),
            steps: None,
        })
    }

    fn open_bar(&mut self, ctx: &egui::Context) {
        let (Some(plan), Some(state)) = (self.rec.plan.as_ref(), self.bar_state()) else {
            return;
        };
        let size = host_size(ctx, &state);
        let zoom = ctx.zoom_factor();
        let guess = monitor_scale(plan.launcher).map_or(ctx.pixels_per_point(), |s| s * zoom);
        let excluded = plan.host == BarHost::ExcludedWindow;
        match plan.place(excluded, size, guess, zoom) {
            Some((slot, ppp)) => {
                log::info!("recording: bar at {slot:?}");
                self.rec.bar = Some(Bar {
                    view: RecBarView::new(),
                    frames: 0,
                    region: None,
                    excluded,
                    slot,
                    ppp,
                    size,
                });
            }
            None => log::info!("recording: no room for the bar ({:?})", plan.host),
        }
        ctx.request_repaint();
    }

    fn close_bar(&mut self) {
        if self.rec.bar.take().is_some() {
            hide_overlay(BAR_TITLE);
        }
    }

    fn draw_bar(&mut self, ctx: &egui::Context, p: &Palette, mode: Mode) {
        let Some(state) = self.bar_state() else {
            return;
        };
        let icon = self.app_icon.clone();
        let zoom = ctx.zoom_factor();
        let Rec { bar, plan, .. } = &mut self.rec;
        let (Some(b), Some(plan)) = (bar.as_mut(), plan.as_ref()) else {
            return;
        };
        let rect = points_rect(b.slot, b.ppp);
        let builder = egui::ViewportBuilder::default()
            .with_title(BAR_TITLE)
            .with_icon(icon)
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(false)
            .with_visible(b.frames >= 2)
            .with_position(rect.min)
            .with_inner_size(rect.size());
        let mut action = None;
        let mut measured = b.size;
        let mut lost_exclusion = false;
        let id = egui::ViewportId::from_hash_of("recording_bar");
        ctx.show_viewport_immediate(id, builder, |vctx, class| {
            if class != egui::ViewportClass::Immediate {
                return;
            }
            if b.frames == 0 {
                make_tool_window(BAR_TITLE);
                glass(BAR_TITLE);
                // Before the window is ever shown, so no frame of the
                // recording can hold it.
                if b.excluded && !exclude_from_capture(BAR_TITLE) {
                    lost_exclusion = true;
                }
            }
            if !lost_exclusion {
                place_overlay(BAR_TITLE, b.slot, false);
            }
            if b.frames == 2 {
                vctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(egui::Color32::TRANSPARENT))
                .show(vctx, |ui| action = b.view.show(ui, p, mode, &state));
            measured = host_size(vctx, &state);
            // The window exists from frame 0; the region follows the hover.
            let hit = hit_rect(
                bar_size(vctx, &state),
                measured,
                b.view.tip_pending(),
                vctx.pixels_per_point(),
            );
            if b.region != Some(hit) && limit_hits(BAR_TITLE, hit) {
                b.region = Some(hit);
            }
        });
        b.frames += 1;

        // The timer crossed the hour (the bar is wider), or exclusion is not
        // to be had (still hidden: it fails on frame 0): placed again, and
        // the next frame moves the window.
        let moved = lost_exclusion || (measured - b.size).length() > 0.5;
        if moved {
            if lost_exclusion {
                log::warn!(
                    "recording: the bar could not be left out of captures; placing it outside"
                );
                b.excluded = false;
            }
            match plan.place(b.excluded, measured, b.ppp, zoom) {
                Some((slot, ppp)) => {
                    b.slot = slot;
                    b.ppp = ppp;
                    b.size = measured;
                }
                None => {
                    log::info!("recording: no room for the bar any more");
                    *bar = None;
                    hide_overlay(BAR_TITLE);
                }
            }
        }

        match (action, self.rec.live.running()) {
            (Some(RecBarAction::Pause), Some(r)) => r.pause(),
            (Some(RecBarAction::Resume), Some(r)) => r.resume(),
            (Some(RecBarAction::Stop), _) => self.stop_recording(),
            _ => {}
        }
    }
}

/// Leaves the window out of screenshots and recordings (Windows 10 2004
/// and later). False where that cannot be done.
#[cfg(target_os = "windows")]
fn exclude_from_capture(title: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
    };
    let Some(hwnd) = crate::overlay::own_window(title) else {
        return false;
    };
    // SAFETY: one of this process's windows, on its own thread.
    unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) != 0 }
}

#[cfg(not(target_os = "windows"))]
fn exclude_from_capture(_title: &str) -> bool {
    false
}

/// Lets the window's transparent pixels through, as the popup does: the
/// room under the bar for its tooltips is clear until one is drawn there.
#[cfg(target_os = "windows")]
fn glass(title: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GCLP_HBRBACKGROUND, SetClassLongPtrW};

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

    let Some(hwnd) = crate::overlay::own_window(title) else {
        return;
    };
    let sheet = Margins {
        cx_left: -1,
        cx_right: -1,
        cy_top: -1,
        cy_bottom: -1,
    };
    // SAFETY: one of this process's windows; `sheet` outlives the call.
    unsafe {
        SetClassLongPtrW(hwnd, GCLP_HBRBACKGROUND, 0);
        let _ = DwmExtendFrameIntoClientArea(hwnd, &sheet);
    }
}

#[cfg(not(target_os = "windows"))]
fn glass(_title: &str) {}

/// Limits where the window takes (and paints) anything to `w` x `h` window
/// pixels from its top-left. True once set.
#[cfg(target_os = "windows")]
fn limit_hits(title: &str, (w, h): (i32, i32)) -> bool {
    use windows_sys::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject, SetWindowRgn};
    let Some(hwnd) = crate::overlay::own_window(title) else {
        return false;
    };
    // SAFETY: one of this process's windows. On success the system owns the
    // region; on failure it is ours to delete.
    unsafe {
        let rgn = CreateRectRgn(0, 0, w, h);
        if rgn.is_null() {
            return false;
        }
        if SetWindowRgn(hwnd, rgn, 1) == 0 {
            DeleteObject(rgn);
            return false;
        }
    }
    true
}

/// X11 places the bar outside the recorded area, so the room covers nothing
/// recorded.
#[cfg(not(target_os = "windows"))]
fn limit_hits(_title: &str, _hit: (i32, i32)) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> DRect {
        DRect { x, y, w, h }
    }
    const BAR: (f32, f32) = (300.0, 80.0);

    #[test]
    fn a_close_waits_only_for_a_recording_it_would_end() {
        assert!(hold_close(true, false, true), "quitting mid-recording");
        assert!(!hold_close(true, true, true), "the window only hides");
        assert!(!hold_close(true, false, false), "nothing recording");
        assert!(!hold_close(false, false, true), "no close asked");
    }

    #[test]
    fn the_region_is_the_bar_until_a_tooltip_is_due() {
        let bar = egui::vec2(300.0, 40.0);
        let host = egui::vec2(300.0, 72.0);
        assert_eq!(hit_rect(bar, host, false, 1.0), (300, 40));
        assert_eq!(hit_rect(bar, host, true, 1.0), (300, 72));
        // In window pixels, rounded out.
        assert_eq!(hit_rect(bar, host, false, 1.25), (375, 50));
        assert_eq!(
            hit_rect(egui::vec2(300.5, 40.1), host, false, 2.0),
            (601, 81)
        );
    }
    const M: DRect = DRect {
        x: 0.0,
        y: 0.0,
        w: 1920.0,
        h: 1080.0,
    };

    fn plan(host: BarHost, rect: DRect) -> BarPlan {
        BarPlan {
            host,
            rect,
            monitors: vec![M],
            launcher: M,
        }
    }

    #[test]
    fn an_excluded_bar_stays_where_the_launcher_was_even_over_the_area() {
        let s = plan(BarHost::ExcludedWindow, M).slot(true, BAR, 16.0, 8.0);
        assert_eq!(s, Some(r(810.0, 16.0, 300.0, 80.0)));
    }

    #[test]
    fn without_exclusion_the_bar_goes_outside_the_area_or_nowhere() {
        let area = r(600.0, 400.0, 400.0, 300.0);
        let outside = plan(BarHost::ExcludedWindow, area).slot(false, BAR, 16.0, 8.0);
        assert_eq!(outside, Some(r(650.0, 312.0, 300.0, 80.0)));
        assert_eq!(
            plan(BarHost::Outside, area).slot(true, BAR, 16.0, 8.0),
            outside,
            "X11 never excludes"
        );
        // The whole of the only monitor leaves no room.
        assert_eq!(
            plan(BarHost::ExcludedWindow, M).slot(false, BAR, 16.0, 8.0),
            None
        );
    }

    #[test]
    fn a_hidden_bar_is_never_placed() {
        let area = r(600.0, 400.0, 400.0, 300.0);
        assert_eq!(
            plan(BarHost::Hidden, area).slot(false, BAR, 16.0, 8.0),
            None
        );
    }
}
