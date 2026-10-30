//! A recording on the viewport host (Windows and X11): started from the
//! launcher's Record, its bar, and the notice at its end. The recording
//! itself is `video::live`; this is where the eframe app hosts it.
//!
//! A Steps session is hosted here too, with the same bar showing its step
//! count: started from the Steps key or the launcher, its camera opened on
//! a worker (`steps::camera::open_camera` can wait on a portal), then
//! `steps::live` takes the pictures. Its bar goes where the steps are not
//! pictured: on Windows where the launcher's was, left out of captures; on
//! X11 on a monitor the pointer is not on (`steps_bar_slot`), moved away by
//! `X11Conceal` for a shot of its own monitor. Stop opens the review
//! (`review_host`); Esc throws everything away at once, without waiting for
//! the camera thread.
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
use crate::platform::{Host, Target};
use crate::review_host::FINISH_REVIEW_FIRST;
use crate::steps::camera::{Camera, Conceal, NoConceal, open_camera};
use crate::steps::clicks::ClickWatch;
use crate::steps::live::{BarFacts, StepsEnd, StepsEvent, StepsLive, Trigger};
use crate::video::availability;
use crate::video::live::{Ask, LiveEvent};
use crate::{WindupApp, capture};
use eframe::egui;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windup_core::ToolId;
use windup_core::capture::DRect;
use windup_core::capture::backend::{BarHost, record_bar_host, steps_order};
use windup_core::capture::session::{CaptureMode, Session, StepsChoice, VideoChoice};
use windup_core::hotkeys::LiveKind;
use windup_core::steps::clicks::takes;
use windup_core::steps::options::StepTrigger;
use windup_core::steps::place::steps_bar_slot;
use windup_core::video::{place_bar, top_centre};
use windup_ui::capture::recbar::{RecBarAction, RecBarState, RecBarView, bar_size, host_size};
use windup_ui::{Mode, Palette, metrics};

/// The bar's window title (also how Win32 finds its window).
pub(crate) const BAR_TITLE: &str = "Windup recording";
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
    /// The Steps session that runs, or is stopping.
    pub(crate) steps: Option<StepsLive>,
    /// The launcher's Steps, started next frame.
    steps_pending: Option<StepsChoice>,
    /// The camera opening on a worker.
    steps_opening: Option<Receiver<Result<Opened, String>>>,
    /// What the session's camera thread knows of the bar.
    steps_facts: Option<Arc<Mutex<BarFacts>>>,
    clicks: Option<ClickWatch>,
    /// Stop was asked: the bar and the keys are gone, `Over` is to come.
    steps_stopping: bool,
}

/// A Steps camera opened by the worker, and where its bar goes.
struct Opened {
    camera: Box<dyn Camera>,
    conceal: Arc<dyn Conceal>,
    plan: BarPlan,
    trigger: StepTrigger,
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
    /// The monitor the launcher bar was on; for Steps, the pointer's.
    launcher: DRect,
    /// A Steps bar: nothing is recorded but the pointer's monitor.
    steps: bool,
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
            BarHost::Hidden => None,
            _ if self.steps => steps_bar_slot(self.host, self.launcher, &self.monitors, size, top),
            BarHost::ExcludedWindow | BarHost::Outside => {
                place_bar(self.rect, &self.monitors, size, gap)
            }
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
            steps: false,
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
        if self.rec.steps_busy() {
            log::debug!("recording: a Steps session is under way");
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
    ///
    /// A Steps session is stopped too, but the app stays: its review opens,
    /// which asks before steps that were not exported are lost. A review
    /// that is open is asked to close, likewise.
    pub(crate) fn hold_close_while_recording(&mut self, ctx: &egui::Context, hides: bool) {
        let requested = ctx.input(|i| i.viewport().close_requested());
        let steps = self.rec.steps_busy() || self.review.is_some();
        if !hold_close(requested, hides, self.rec.live.busy() || steps) {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        if self.rec.live.busy() {
            if !self.rec.quit_when_over {
                log::info!("recording: the window was closed, stopping first");
            }
            self.rec.quit_when_over = true;
            self.stop_recording();
        }
        if self.rec.steps_busy() {
            log::info!("steps: the window was closed, stopping first");
            self.stop_steps();
        } else if self.review.is_some() {
            self.ask_review_to_close(ctx);
        }
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

    /// The pause key, for whichever session runs.
    pub(crate) fn pause_live(&mut self) {
        match self.rec.steps.as_ref() {
            Some(s) if !self.rec.steps_stopping => {
                s.toggle_pause();
            }
            Some(_) => {}
            None => self.pause_recording(),
        }
    }

    /// The stop key, for whichever session runs.
    pub(crate) fn stop_live(&mut self) {
        if self.rec.steps_busy() {
            self.stop_steps();
        } else {
            self.stop_recording();
        }
    }

    /// Esc, for whichever session runs.
    pub(crate) fn cancel_live(&mut self) {
        if self.rec.steps_busy() {
            // Stopped means kept: a cancel that comes after (the command;
            // Esc is let go at the stop) does not throw the steps away.
            if !self.rec.steps_cancellable() {
                log::info!("steps: stopping already, the cancel is ignored");
                return;
            }
            self.cancel_steps();
        } else {
            self.cancel_recording();
        }
    }

    /// The launcher's Steps: remembered, and started next frame.
    pub(crate) fn steps_from(&mut self, choice: StepsChoice) {
        self.rec.steps_pending = Some(choice);
    }

    /// The Steps key: stops the session that runs, else starts one at once
    /// with the saved choices.
    pub(crate) fn steps_hotkey(&mut self, ctx: &egui::Context) {
        let r = &self.rec;
        if r.steps_opening.is_some() || (r.steps.is_some() && !r.steps_stopping) {
            self.stop_steps();
        } else if r.steps.is_none() {
            self.start_steps(ctx, None);
        }
    }

    /// The step key: one step, now.
    pub(crate) fn take_step(&mut self) {
        if let Some(s) = &self.rec.steps
            && !self.rec.steps_stopping
        {
            s.trigger(Trigger::Hotkey);
        }
    }

    /// Starts a Steps session: `choice` from the launcher, or `None` for
    /// the key, with the saved choices. The camera is opened on a worker;
    /// the session and its bar start once it is.
    pub(crate) fn start_steps(&mut self, ctx: &egui::Context, choice: Option<StepsChoice>) {
        if !self.config.is_tool_enabled(ToolId::Steps) || self.host != Host::Viewport {
            return;
        }
        let block = steps_start_block(
            self.rec.live.busy(),
            self.rec.steps.is_some() || self.rec.steps_opening.is_some(),
            self.overlay.open(),
            self.review.is_some(),
        );
        match block {
            Some(StartBlock::Busy) => {
                log::debug!("steps: a recording or Steps session is under way");
                return;
            }
            Some(StartBlock::Overlay) => {
                log::debug!("steps: the capture overlay or the step editor is open");
                return;
            }
            Some(StartBlock::Review) => {
                self.open_notice(ctx, FINISH_REVIEW_FIRST, None);
                return;
            }
            None => {}
        }
        if let Err(why) = availability::cached(&self.config.video).steps {
            self.open_notice(ctx, why, None);
            return;
        }
        let trigger = choice.map_or(self.config.steps.trigger, |c| c.trigger);
        let (tx, rx) = channel();
        let wake = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("steps-open".into())
            .spawn(move || {
                let _ = tx.send(open_steps(trigger));
                wake.request_repaint();
            });
        match spawned {
            Ok(_) => {
                log::info!("steps: starting");
                self.rec.steps_opening = Some(rx);
            }
            Err(e) => {
                let text = format!("Steps could not start: {e}");
                self.open_notice(ctx, &text, None);
            }
        }
    }

    /// The camera is open (or not): the session, its click watch and its
    /// bar start.
    fn steps_opened(&mut self, ctx: &egui::Context, opened: Result<Opened, String>) {
        let Opened {
            camera,
            conceal,
            plan,
            trigger,
        } = match opened {
            Ok(o) => o,
            Err(why) => {
                log::warn!("steps: no camera: {why}");
                let text = format!("Steps could not start: {why}");
                self.open_notice(ctx, &text, None);
                return;
            }
        };
        let (tx, rx) = channel();
        let watch = (trigger != StepTrigger::HotkeyOnly)
            .then(|| ClickWatch::start(tx.clone()))
            .flatten();
        let clicks = if watch.is_some() {
            Ok(())
        } else {
            Err("clicks cannot be seen")
        };
        let facts = Arc::new(Mutex::new(BarFacts {
            rect: None,
            pointer_on_bar: false,
            host: plan.host,
            excluded: false,
        }));
        let cfg = windup_core::config::StepsConfig {
            trigger,
            ..self.config.steps.clone()
        };
        let takes = takes(trigger, clicks);
        match StepsLive::start(&cfg, takes, camera, conceal, facts.clone(), (tx, rx)) {
            Ok(live) => {
                self.rec.steps = Some(live);
                self.rec.steps_facts = Some(facts);
                self.rec.clicks = watch;
                self.rec.plan = Some(plan);
                self.open_bar(ctx);
            }
            Err(why) => {
                let text = format!("Steps could not start: {why}");
                self.open_notice(ctx, &text, None);
            }
        }
    }

    /// Stop, from the bar or a key: the bar and the keys go at once, the
    /// review opens once the last step is written.
    pub(crate) fn stop_steps(&mut self) {
        if self.rec.steps_opening.take().is_some() {
            // Not started yet: nothing was taken. The worker's camera is
            // dropped when it finds nobody listening.
            log::info!("steps: stopped before the first step");
        }
        self.rec.steps_pending = None;
        if let Some(s) = self.rec.steps.as_mut()
            && !self.rec.steps_stopping
        {
            s.stop();
            self.rec.steps_stopping = true;
        }
        self.rec.clicks = None;
        self.close_bar();
    }

    /// Esc: the session is thrown away. Everything of it on the screen goes
    /// now: its camera thread removes the folder whenever it is done (even
    /// a shot that hangs ends some time), and whatever it says then is not
    /// listened to.
    pub(crate) fn cancel_steps(&mut self) {
        self.rec.steps_opening = None;
        self.rec.steps_pending = None;
        if let Some(mut s) = self.rec.steps.take() {
            s.cancel();
        }
        self.rec.steps_stopping = false;
        self.rec.steps_facts = None;
        self.rec.clicks = None;
        self.close_bar();
        self.rec.plan = None;
    }

    /// The session ended: the review, a notice, or both.
    fn steps_over(&mut self, ctx: &egui::Context, end: StepsEnd) {
        self.rec.steps = None;
        self.rec.steps_stopping = false;
        self.rec.steps_facts = None;
        self.rec.clicks = None;
        self.close_bar();
        self.rec.plan = None;
        match end {
            StepsEnd::Review(dir) => self.open_review(ctx, dir),
            StepsEnd::Discarded => {}
            StepsEnd::Empty => self.open_notice(ctx, NO_STEPS, None),
            StepsEnd::Failed { why, review } => {
                self.open_notice(ctx, &format!("Steps stopped: {why}"), None);
                if let Some(dir) = review {
                    self.open_review(ctx, dir);
                }
            }
        }
    }

    /// Starts, polls and follows a Steps session. Every frame, before the
    /// bar is drawn.
    fn drive_steps(&mut self, ctx: &egui::Context) {
        if let Some(choice) = self.rec.steps_pending.take() {
            self.start_steps(ctx, Some(choice));
        }
        let opened = match self.rec.steps_opening.as_ref().map(Receiver::try_recv) {
            Some(Ok(opened)) => Some(opened),
            Some(Err(TryRecvError::Disconnected)) => {
                Some(Err("the camera could not be opened".to_string()))
            }
            Some(Err(TryRecvError::Empty)) | None => None,
        };
        if let Some(opened) = opened {
            self.rec.steps_opening = None;
            self.steps_opened(ctx, opened);
        }
        let events = self.rec.steps.as_mut().map(StepsLive::poll);
        for event in events.into_iter().flatten() {
            match event {
                StepsEvent::Step(_) => ctx.request_repaint(),
                StepsEvent::Over(end) => self.steps_over(ctx, end),
            }
        }
    }

    /// Whether Steps is starting, running or stopping.
    fn steps_keys_wanted(&self) -> bool {
        let r = &self.rec;
        r.steps_opening.is_some() || (r.steps.is_some() && !r.steps_stopping)
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
        self.drive_steps(ctx);
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
        // The camera thread decides from these whether a click was on the
        // bar and whether the bar must be hidden for a shot.
        if let Some(facts) = &self.rec.steps_facts {
            let mut f = facts.lock().unwrap_or_else(|e| e.into_inner());
            f.rect = self.rec.bar.as_ref().map(|b| b.slot);
            f.excluded = self.rec.bar.as_ref().is_some_and(|b| b.excluded);
        }
        // Pause, stop and Esc (and the step key) are bound from the start
        // until it is stopped or cancelled.
        let live = if self.steps_keys_wanted() {
            Some(LiveKind::Steps)
        } else if self.rec.live.keys_wanted() {
            Some(LiveKind::Video)
        } else {
            None
        };
        if let Some(listener) = &self.hotkey_listener {
            listener.set_live(live, &self.config);
        }
        if self.rec.live.busy() || self.rec.steps_busy() {
            ctx.request_repaint_after(POLL);
        }
    }

    fn bar_state(&self) -> Option<RecBarState> {
        if let Some(s) = &self.rec.steps {
            return (!self.rec.steps_stopping).then(|| RecBarState {
                elapsed: 0.0,
                paused: s.paused(),
                fps: 0,
                bitrate_label: String::new(),
                reduced_motion: reduced_motion(),
                steps: Some(s.count()),
            });
        }
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

        if let Some(s) = &self.rec.steps {
            match action {
                Some(RecBarAction::Pause) if !s.paused() => {
                    s.toggle_pause();
                }
                Some(RecBarAction::Resume) if s.paused() => {
                    s.toggle_pause();
                }
                Some(RecBarAction::Stop) => self.stop_steps(),
                _ => {}
            }
            return;
        }
        match (action, self.rec.live.running()) {
            (Some(RecBarAction::Pause), Some(r)) => r.pause(),
            (Some(RecBarAction::Resume), Some(r)) => r.resume(),
            (Some(RecBarAction::Stop), _) => self.stop_recording(),
            _ => {}
        }
    }
}

/// A bar plan's rectangle where there is no monitor at all.
const NOWHERE: DRect = DRect {
    x: 0.0,
    y: 0.0,
    w: 0.0,
    h: 0.0,
};

/// The notice when a Steps session stopped before any step.
pub(crate) const NO_STEPS: &str = "No steps were taken.";

impl Rec {
    /// Whether Steps is asked for, starting, running or stopping.
    pub(crate) fn steps_busy(&self) -> bool {
        self.steps.is_some() || self.steps_opening.is_some() || self.steps_pending.is_some()
    }

    /// Whether a cancel may still throw the session away: not once it was
    /// stopped, which keeps the steps for the review.
    fn steps_cancellable(&self) -> bool {
        !self.steps_stopping
    }
}

/// Why a Steps session cannot start now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartBlock {
    /// A recording or another Steps session is under way.
    Busy,
    /// The capture overlay or the step editor is open: its frozen picture
    /// would be what the steps show.
    Overlay,
    /// A review is open: one at a time.
    Review,
}

pub(crate) fn steps_start_block(
    video_busy: bool,
    steps_live: bool,
    overlay_open: bool,
    review_open: bool,
) -> Option<StartBlock> {
    if video_busy || steps_live {
        Some(StartBlock::Busy)
    } else if overlay_open {
        Some(StartBlock::Overlay)
    } else if review_open {
        Some(StartBlock::Review)
    } else {
        None
    }
}

/// Opens a Steps camera and plans its bar; on the worker, since a portal
/// may keep `open_camera` waiting for a long time. The ScreenCast restore
/// token is saved afterwards whether or not a camera opened, as video does.
fn open_steps(trigger: StepTrigger) -> Result<Opened, String> {
    let facts = capture::facts();
    let host = record_bar_host(&facts);
    let saved = windup_core::config::Config::load_screencast_token();
    let token = Arc::new(Mutex::new(saved.clone()));
    let opened = open_camera(&steps_order(&facts), &token);
    let next = token.lock().ok().and_then(|t| t.clone());
    if next != saved {
        windup_core::config::Config::save_screencast_token(next.as_deref());
    }
    let (mut camera, route) = opened?;
    let mut monitors = monitor_layouts();
    let pointer = camera.monitor(None).or_else(|| monitors.first().copied());
    let Some(launcher) = pointer else {
        // Nowhere to place a bar: the keys still stop the session.
        log::info!("steps: no monitor for the bar");
        return Ok(Opened {
            camera,
            conceal: Arc::new(NoConceal),
            plan: BarPlan {
                host: BarHost::Hidden,
                rect: NOWHERE,
                monitors,
                launcher: NOWHERE,
                steps: true,
            },
            trigger,
        });
    };
    if monitors.is_empty() {
        monitors.push(launcher);
    }
    log::info!("steps: camera {route:?}, bar {host:?}");
    Ok(Opened {
        camera,
        conceal: conceal_for(host),
        plan: BarPlan {
            host,
            rect: launcher,
            monitors,
            launcher,
            steps: true,
        },
        trigger,
    })
}

/// Every monitor's layout in desktop units, without a picture.
fn monitor_layouts() -> Vec<DRect> {
    #[cfg(target_os = "windows")]
    {
        capture::windows::monitor_list()
            .into_iter()
            .map(|(r, _)| r)
            .collect()
    }
    #[cfg(target_os = "linux")]
    {
        capture::x11::monitor_list()
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        Vec::new()
    }
}

/// What hides the bar for a shot of its own monitor: X11 moves it away;
/// Windows leaves it out of captures, so nothing is needed there.
fn conceal_for(host: BarHost) -> Arc<dyn Conceal> {
    #[cfg(target_os = "linux")]
    if host == BarHost::Outside
        && let Some(c) = crate::steps::x11::X11Conceal::open()
    {
        return Arc::new(c);
    }
    let _ = host;
    Arc::new(NoConceal)
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
    fn steps_do_not_start_under_the_overlay_or_beside_a_review() {
        assert_eq!(steps_start_block(false, false, false, false), None);
        assert_eq!(
            steps_start_block(false, false, true, false),
            Some(StartBlock::Overlay),
            "the launcher or the step editor is open"
        );
        assert_eq!(
            steps_start_block(false, false, false, true),
            Some(StartBlock::Review)
        );
        assert_eq!(
            steps_start_block(true, false, true, true),
            Some(StartBlock::Busy)
        );
        assert_eq!(
            steps_start_block(false, true, false, false),
            Some(StartBlock::Busy)
        );
    }

    #[test]
    fn a_stopped_session_is_not_cancelled() {
        assert!(Rec::default().steps_cancellable());
        let stopping = Rec {
            steps_stopping: true,
            ..Rec::default()
        };
        assert!(!stopping.steps_cancellable());
    }

    #[test]
    fn a_close_waits_only_for_a_recording_it_would_end() {
        assert!(hold_close(true, false, true), "quitting mid-recording");
        let steps_busy = Rec {
            steps_pending: Some(StepsChoice::default()),
            ..Rec::default()
        }
        .steps_busy();
        assert!(hold_close(true, false, steps_busy), "quitting mid-steps");
        assert!(!Rec::default().steps_busy());
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
            steps: false,
        }
    }

    #[test]
    fn a_steps_bar_goes_off_the_pointers_monitor_unless_excluded() {
        let other = r(1920.0, 0.0, 1920.0, 1080.0);
        let steps = |host| BarPlan {
            host,
            rect: M,
            monitors: vec![M, other],
            launcher: M,
            steps: true,
        };
        // X11: the top centre of the other monitor.
        assert_eq!(
            steps(BarHost::Outside).slot(false, BAR, 16.0, 8.0),
            Some(r(2730.0, 16.0, 300.0, 80.0))
        );
        // Windows: where the launcher's bar was, out of every picture.
        assert_eq!(
            steps(BarHost::ExcludedWindow).slot(true, BAR, 16.0, 8.0),
            Some(r(810.0, 16.0, 300.0, 80.0))
        );
        // One monitor: on it all the same, hidden for each shot.
        let one = BarPlan {
            monitors: vec![M],
            ..steps(BarHost::Outside)
        };
        assert_eq!(
            one.slot(false, BAR, 16.0, 8.0),
            Some(r(810.0, 16.0, 300.0, 80.0))
        );
        assert_eq!(steps(BarHost::Hidden).slot(false, BAR, 16.0, 8.0), None);
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
