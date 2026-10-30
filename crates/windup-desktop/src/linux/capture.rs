//! Capture on the Wayland hosts (layer and window), from the background loop.
//!
//! A capture hotkey starts a worker thread that takes the picture by the
//! `still_order` back ends, each on a Wayland connection of its own: the
//! screenshot portal may wait on a first-time permission dialog, and KWin's
//! calls take up to seconds, so neither may run on the Wayland thread, which
//! keeps serving the clipboard and the popup meanwhile. The result comes back
//! here, and the overlay opens on the Wayland thread
//! (`wayland::Request::OpenCapture`). When it closes, `ServiceEvent::CaptureDone`
//! brings the outcome back, which `capture::finish` copies and saves as on
//! Windows.
//!
//! The window host (GNOME, Weston) runs no Wayland service of its own for the
//! clipboard; the first capture starts one for the overlay, and keeps it.
//! Nothing about its clipboard changes.
//!
//! A Record from the overlay starts the recording here (`video::live`), with
//! the gate kept shut until it is over. The layer host shows the recording
//! bar on the Wayland thread (`wayland::recbar`); the window host shows none,
//! and the Video hotkey (or `windup --record-stop`) stops it.
//!
//! Steps runs here too, from the launcher's Steps or `windup --steps`: its
//! camera is opened on a worker (a portal may ask first), then
//! `steps::live` takes the pictures. The layer host shows the same bar with
//! the step count, off the pointer's monitor where there is another, and
//! hidden for the instant of a shot of its own monitor (`LayerConceal`).
//! Stopped with steps taken, the review opens as a child process,
//! `windup --steps-review <dir>`, which asks for the editor on a step over
//! the socket (`Request::EditStep`); the editor is the overlay on that one
//! picture, and Enter writes it back to the step's file.
//!
//! While a recording or Steps runs, its keys (pause, stop, Esc, and Steps'
//! step key) are bound where the desktop lets Windup bind them
//! (`platform::recording_keys`): temporary entries in COSMIC's shortcuts, or
//! a second GlobalShortcuts portal session. They run `windup --record-pause`
//! and so on, like every Wayland hotkey, and act on whichever session runs.

use super::wayland::{
    CaptureEnd, CaptureSetup, LayerConceal, NoticeSetup, RecBarSetup, Service, Shared,
};
use super::{Background, copy_image, cosmic, popup_setup, portal, window};
use crate::capture::gate::{Gate, Reply};
use crate::capture::{CaptureError, finish, grab_all, window_rects};
use crate::overlay::edit_refusal;
use crate::platform::{Host, PasteVia, Plan, RecordingKeys, Watch, recording_keys};
use crate::recording::NO_STEPS;
use crate::review_host::FINISH_REVIEW_FIRST;
use crate::steps::camera::{Camera, Conceal, NoConceal, open_camera};
use crate::steps::clicks::ClickWatch;
use crate::steps::live::{BarFacts, StepsEnd, StepsEvent, StepsLive, Trigger};
use crate::video::availability;
use crate::video::live::{Ask, Live, LiveEvent};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windup_core::capture::WindowRect;
use windup_core::capture::backend::{BarHost, record_bar_host, steps_order};
use windup_core::capture::session::{
    Action, CaptureMode, ModeAvailability, Outcome, Session, StepsChoice,
};
use windup_core::capture::{DRect, DesktopShot};
use windup_core::hotkeys::{LiveKind, session_bindings, session_keys_untaken};
use windup_core::ipc::Command;
use windup_core::steps::clicks::takes;
use windup_core::steps::model::{Step, StepsDoc};
use windup_core::steps::options::StepTrigger;
use windup_core::steps::place::steps_bar_slot;
use windup_core::{Config, ToolId};
use windup_ui::capture::recbar::RecBarAction;

/// How long the reduced-motion answer may take once the picture is in.
const MOTION_WAIT: Duration = Duration::from_millis(300);

/// The notice when a step's picture cannot be read.
const EDIT_UNREADABLE: &str = "The step's picture could not be read.";

/// What the worker brings back.
pub(super) struct Grab {
    shot: DesktopShot,
    windows: Vec<WindowRect>,
    reduced_motion: bool,
    /// What Video may offer, found alongside the grab
    /// (`video::availability`).
    availability: ModeAvailability,
}

pub(super) type Grabbed = Result<Grab, CaptureError>;

/// The capture state the background loop carries.
pub(super) struct Capturing {
    pub(super) gate: Gate,
    mode: CaptureMode,
    tx: Sender<Grabbed>,
    rx: Receiver<Grabbed>,
    /// The service started for the overlay where none runs for the clipboard.
    own_service: Option<Service>,
    /// The recording, from the overlay's Record to its notice.
    pub(super) live: Live,
    /// What the recording bar is placed beside.
    rec_rect: Option<DRect>,
    /// The reduced-motion answer, asked as the recording starts.
    motion: Option<Receiver<bool>>,
    /// The session's keys, while they are meant to be bound.
    keys: Option<LiveKeys>,
    /// Steps, beside the session itself (`Background::steps`).
    pub(super) stepping: Stepping,
}

/// How a session's keys were bound, so they can be let go at its end.
struct LiveKeys {
    kind: LiveKind,
    how: BoundKeys,
}

enum BoundKeys {
    Cosmic,
    /// Closed when dropped.
    Portal {
        _session: portal::RecordingShortcuts,
    },
    /// Nothing global here (Sway, GNOME without the portal): the bar and
    /// the commands.
    Nothing,
}

/// A Steps session's state other than the session: its start, its bar and
/// its clicks, and the step being edited.
#[derive(Default)]
pub(super) struct Stepping {
    /// The camera opening on its worker.
    opening: Option<Receiver<Result<Opened, String>>>,
    /// Stop was asked: the bar and keys are gone, the session is finishing.
    stopping: bool,
    /// Read by the camera thread for each trigger.
    facts: Option<Arc<Mutex<BarFacts>>>,
    clicks: Option<ClickWatch>,
    /// What the bar shows, and what it tells.
    count: Arc<AtomicU32>,
    paused: Arc<AtomicBool>,
    pointer_on_bar: Arc<AtomicBool>,
    /// The step's file the editor is open on.
    editing: Option<PathBuf>,
}

impl Stepping {
    /// Whether a cancel may still throw the session away: not once it was
    /// stopped, which keeps the steps for the review.
    fn cancellable(&self) -> bool {
        !self.stopping
    }
}

/// A Steps camera opened by the worker, and what the bar is placed from.
struct Opened {
    camera: Box<dyn Camera>,
    /// The pointer's monitor, desktop units, when known.
    pointer: Option<DRect>,
    monitors: Vec<DRect>,
    host: BarHost,
    trigger: StepTrigger,
    reduced_motion: bool,
}

impl Default for Capturing {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            gate: Gate::default(),
            mode: CaptureMode::Screenshot,
            tx,
            rx,
            own_service: None,
            live: Live::default(),
            rec_rect: None,
            motion: None,
            keys: None,
            stepping: Stepping::default(),
        }
    }
}

/// Which session's keys are to be bound: Steps' while it starts or runs,
/// else a recording's while it runs. They never run together.
pub(super) fn live_kind(video_busy: bool, steps_busy: bool) -> Option<LiveKind> {
    if steps_busy {
        Some(LiveKind::Steps)
    } else if video_busy {
        Some(LiveKind::Video)
    } else {
        None
    }
}

/// The monitor a Steps bar goes on, as `steps_bar_slot` places it: one the
/// pointer is not on when there is one. It stands for the bar in what the
/// camera thread decides; on Wayland no click has a position to compare
/// with the bar's own rectangle.
pub(super) fn bar_monitor(
    host: BarHost,
    pointer: Option<DRect>,
    monitors: &[DRect],
) -> Option<DRect> {
    let pointer = pointer?;
    let slot = steps_bar_slot(host, pointer, monitors, (1.0, 1.0), 0.0)?;
    Some(
        monitors
            .iter()
            .copied()
            .find(|m| m.contains(slot.x, slot.y))
            .unwrap_or(pointer),
    )
}

impl Background {
    /// A capture hotkey for `tool`. Ignored when the tool is off, or while a
    /// capture is grabbing or open.
    pub(super) fn start_capture(&mut self, tool: ToolId, mode: CaptureMode, config: &Config) {
        if !config.is_tool_enabled(tool) {
            log::debug!("capture: {} is turned off", tool.as_str());
            return;
        }
        if !self.capture.gate.begin() {
            log::info!("capture: one is under way; ignored");
            return;
        }
        log::info!("capture: grabbing");
        self.capture.mode = mode;
        let video = config.video.clone();
        // A worker that dies without a result (a panic), or never starts,
        // still reports, so the gate never stays `Grabbing`.
        let reply = Reply::new(
            self.capture.tx.clone(),
            Err(CaptureError::Failed(
                "the grab thread ended without a picture".into(),
            )),
        );
        let spawned = std::thread::Builder::new()
            .name("capture-grab".into())
            .spawn(move || {
                let motion = portal::ask_reduced_motion();
                // Video's facts are found while the picture is taken; once it
                // is in they get a moment more, then the last ones kept do,
                // so a screenshot never opens later for them.
                let refresh = availability::refresh();
                let grabbed = grab_all().map(|shot| {
                    let windows = window_rects(&shot);
                    refresh.wait(availability::FRESH_WAIT);
                    let availability = availability::current(&video);
                    Grab {
                        shot,
                        windows,
                        reduced_motion: motion.recv_timeout(MOTION_WAIT).unwrap_or(false),
                        availability,
                    }
                });
                reply.send(grabbed);
            });
        if let Err(e) = spawned {
            // The closure, and with it `reply`, is dropped: that reports.
            log::error!("capture: no thread for the grab: {e}");
        }
    }

    /// Acts on a finished grab, if one came in.
    pub(super) fn poll_capture(
        &mut self,
        config: &mut Config,
        shared: &Shared,
        commands: &Receiver<Command>,
    ) {
        while let Ok(grabbed) = self.capture.rx.try_recv() {
            self.grabbed(grabbed, config, shared, commands);
        }
    }

    fn grabbed(
        &mut self,
        grabbed: Grabbed,
        config: &mut Config,
        shared: &Shared,
        commands: &Receiver<Command>,
    ) {
        let grab = match grabbed {
            Ok(grab) if !grab.shot.monitors.is_empty() => grab,
            other => {
                // A picture of no monitors is no picture.
                let e = other.err().unwrap_or(CaptureError::Unsupported);
                if let CaptureError::Failed(why) = &e {
                    log::warn!("capture: {why}");
                }
                log::info!("capture: no picture ({e:?})");
                self.capture.gate.end();
                self.notice(e.message(), config, shared, commands);
                return;
            }
        };
        let mut session = Session::new(
            grab.shot,
            grab.windows,
            &config.screenshot,
            &config.video,
            grab.availability,
        );
        session.set_steps(&config.steps);
        // Refused, and left on Screenshot, when Video or Steps cannot work here.
        session.act(Action::SetMode(self.capture.mode));
        self.open_overlay(session, grab.reduced_motion, config, shared, commands);
    }

    /// Opens the overlay on `session`, the gate being `Grabbing`.
    fn open_overlay(
        &mut self,
        session: Session,
        reduced_motion: bool,
        config: &mut Config,
        shared: &Shared,
        commands: &Receiver<Command>,
    ) {
        let theme = popup_setup(config);
        let setup = CaptureSetup {
            session,
            keys: config.screenshot.keys.clone(),
            palette: theme.palette,
            mode: theme.mode,
            reduced_motion,
        };
        let opened = self
            .capture_service(shared)
            .is_some_and(|service| service.open_capture(setup));
        if opened {
            self.capture.gate.opened();
        } else {
            self.capture.gate.end();
            self.capture.stepping.editing = None;
            let failed = CaptureError::Failed(String::new());
            self.notice(failed.message(), config, shared, commands);
        }
    }

    /// The overlay closed: remember its choices, then copy and save, start
    /// recording or Steps, or write an edited step back.
    pub(super) fn capture_done(
        &mut self,
        end: CaptureEnd,
        config: &mut Config,
        shared: &Shared,
        commands: &Receiver<Command>,
    ) {
        self.capture.gate.end();
        let editing = self.capture.stepping.editing.take();
        let record = matches!(end.outcome, Outcome::Record { .. });
        let steps = matches!(end.outcome, Outcome::Steps { .. });
        self.change_config(config, |c| {
            finish::remember(&mut c.screenshot, &end.session);
            if record {
                finish::remember_video(&mut c.video, &end.session);
            }
            if steps {
                finish::remember_steps(&mut c.steps, &end.session);
            }
        });
        let mut errors: Vec<String> = end.error.into_iter().collect();
        match &end.outcome {
            Outcome::Steps { choice } => self.start_steps_from(*choice, config, shared, commands),
            // Esc in the editor leaves the step as it was.
            Outcome::StepDone(img) => match editing {
                Some(path) => match finish::write_step(img, &path) {
                    Ok(()) => log::info!("steps: {} edited", path.display()),
                    Err(e) => errors.push(e),
                },
                None => log::warn!("steps: an edited step with no file to go to"),
            },
            _ => {}
        }
        if let Outcome::Record { rect, choice } = &end.outcome {
            // The overlay's surfaces were destroyed before this event was
            // sent; the start waits a moment more on its worker for the
            // compositor to show the desktop again.
            let ask = Ask {
                rect: *rect,
                choice: *choice,
                cfg: config.video.clone(),
            };
            if self.capture.live.start(ask) {
                self.capture.gate.recording();
                self.capture.rec_rect = Some(*rect);
                self.capture.motion = Some(portal::ask_reduced_motion());
            }
        }
        if end.open_settings {
            self.show_settings();
        }
        // The copy is recorded in the history like any other: by the
        // data-control echo on the layer host, by the poll elsewhere.
        if let Some(e) = finish::deliver(end.outcome, &config.screenshot, copy_image) {
            errors.push(e);
        }
        if !errors.is_empty() {
            self.notice(&errors.join("\n"), config, shared, commands);
        }
    }

    /// Says why a capture did not happen or save: a layer card on the layer
    /// host, a small window of its own on the window host, as each shows the
    /// clipboard popup.
    fn notice(
        &mut self,
        message: &str,
        config: &mut Config,
        shared: &Shared,
        commands: &Receiver<Command>,
    ) {
        self.notice_with(message, None, config, shared, commands);
    }

    /// `notice`, with "Open folder" for a saved recording.
    fn notice_with(
        &mut self,
        message: &str,
        action: Option<PathBuf>,
        config: &mut Config,
        shared: &Shared,
        commands: &Receiver<Command>,
    ) {
        log::info!("capture: notice: {message}");
        let theme = popup_setup(config);
        if self.runtime.plan.host == Host::Layer
            && let Some(service) = &self.runtime.service
        {
            service.notice(NoticeSetup {
                message: message.to_owned(),
                action,
                palette: theme.palette,
                mode: theme.mode,
            });
            return;
        }
        let later = window::notice(
            message,
            action.as_deref(),
            theme.palette,
            theme.mode,
            commands,
        );
        for command in later {
            self.command(command, config, shared, commands);
        }
    }

    /// The Video hotkey (`windup --record`): stops a recording that runs,
    /// else opens the launcher in Video, or says why Video cannot work here.
    pub(super) fn record_command(
        &mut self,
        config: &mut Config,
        shared: &Shared,
        commands: &Receiver<Command>,
    ) {
        if self.capture.live.stoppable() {
            self.stop_recording();
            return;
        }
        if self.capture.live.busy() || self.steps_busy() || !config.is_tool_enabled(ToolId::Video) {
            return;
        }
        if let Err(why) = availability::current(&config.video).video {
            self.notice(why, config, shared, commands);
            return;
        }
        self.start_capture(ToolId::Video, CaptureMode::Video, config);
    }

    /// Stop, from the bar, the hotkey or `windup --record-stop`. The bar
    /// goes at once; the notice comes once the file is joined.
    pub(super) fn stop_recording(&mut self) {
        if self.capture.live.stoppable() {
            log::info!("recording: stopping");
        }
        self.capture.live.stop();
        self.close_recbar();
    }

    /// The pause key (`windup --record-pause`): pauses a running recording,
    /// or resumes it.
    pub(super) fn pause_recording(&mut self) {
        if let Some(paused) = self.capture.live.toggle_pause() {
            log::info!("recording: {}", if paused { "pausing" } else { "resuming" });
        }
    }

    /// Esc (`windup --record-cancel`): cancels the recording and discards
    /// it. The bar goes at once; the notice says so once it is gone.
    pub(super) fn cancel_recording(&mut self) {
        self.capture.live.cancel();
        self.close_recbar();
    }

    /// The pause key, for whichever session runs.
    pub(super) fn pause_live(&mut self) {
        if self.steps_running() {
            self.toggle_steps_pause();
        } else {
            self.pause_recording();
        }
    }

    /// The stop key, for whichever session runs.
    pub(super) fn stop_live(&mut self) {
        if self.steps_busy() {
            self.stop_steps();
        } else {
            self.stop_recording();
        }
    }

    /// Esc, for whichever session runs.
    pub(super) fn cancel_live(&mut self) {
        if self.steps_busy() {
            // Stopped means kept: a cancel that comes after (Esc not yet
            // let go, or the command) does not throw the steps away.
            if !self.capture.stepping.cancellable() {
                log::info!("steps: stopping already, the cancel is ignored");
                return;
            }
            self.cancel_steps();
        } else {
            self.cancel_recording();
        }
    }

    /// Binds the running session's keys once it has started and lets them
    /// go when it is stopped or cancelled; every tick.
    fn sync_live_keys(&mut self, config: &Config) {
        let wanted = live_kind(
            self.capture.live.started_keys_wanted(),
            self.steps_keys_wanted(),
        );
        if wanted == self.capture.keys.as_ref().map(|k| k.kind) {
            return;
        }
        // A portal session closes as it is dropped.
        if let Some(LiveKeys {
            how: BoundKeys::Cosmic,
            ..
        }) = self.capture.keys.take()
        {
            cosmic::remove_temporary();
        }
        let Some(kind) = wanted else {
            return;
        };
        // The portal is simply tried: where it is missing it says so.
        let how = recording_keys(self.runtime.plan.hotkeys, cosmic::is_cosmic(), true);
        let how = match how {
            RecordingKeys::Cosmic => {
                let keys: Vec<(windup_core::Shortcut, String)> = session_bindings(config, kind)
                    .into_iter()
                    .map(|(s, a)| (s, format!("--{}", Command::for_action(a).as_str())))
                    .collect();
                let keys: Vec<(windup_core::Shortcut, &str)> =
                    keys.iter().map(|(s, a)| (s.clone(), a.as_str())).collect();
                cosmic::add_temporary(&keys);
                // Removed at the end whether or not anything was added: a
                // stale entry may have been.
                BoundKeys::Cosmic
            }
            // Only keys the first session does not hold: asking for one again
            // can bring GNOME's dialog back at every recording.
            RecordingKeys::Portal => BoundKeys::Portal {
                _session: portal::recording_shortcuts(kind, session_keys_untaken(config, kind)),
            },
            RecordingKeys::Hook | RecordingKeys::XGrab | RecordingKeys::BarOnly => {
                BoundKeys::Nothing
            }
        };
        self.capture.keys = Some(LiveKeys { kind, how });
    }

    /// Pause, Resume or Stop from the bar, for whichever session it shows.
    pub(super) fn recbar_action(&mut self, action: RecBarAction) {
        if self.steps_running() {
            let paused = self.capture.stepping.paused.load(Ordering::Relaxed);
            match action {
                RecBarAction::Pause if !paused => self.toggle_steps_pause(),
                RecBarAction::Resume if paused => self.toggle_steps_pause(),
                RecBarAction::Stop => self.stop_steps(),
                _ => {}
            }
            return;
        }
        match (action, self.capture.live.running()) {
            (RecBarAction::Pause, Some(r)) => r.pause(),
            (RecBarAction::Resume, Some(r)) => r.resume(),
            (RecBarAction::Stop, _) => self.stop_recording(),
            _ => {}
        }
    }

    fn close_recbar(&self) {
        if let Some(service) = &self.runtime.service {
            service.close_recbar();
        }
    }

    /// Moves the recording and Steps along; every tick.
    pub(super) fn poll_recording(
        &mut self,
        config: &mut Config,
        shared: &Shared,
        commands: &Receiver<Command>,
    ) {
        for event in self.capture.live.poll() {
            match event {
                LiveEvent::Started => self.open_recbar(config),
                LiveEvent::Stopping => self.close_recbar(),
                LiveEvent::Over(ended) => {
                    self.close_recbar();
                    self.capture.gate.end();
                    self.capture.rec_rect = None;
                    self.capture.motion = None;
                    // Let Esc go before the notice: on the window host it
                    // blocks until it closes.
                    self.sync_live_keys(config);
                    self.notice_with(&ended.message, ended.saved, config, shared, commands);
                }
                LiveEvent::Token(t) => Config::save_screencast_token(t.as_deref()),
            }
        }
        self.poll_steps(config, shared, commands);
        self.sync_live_keys(config);
    }

    /// The bar, where this desktop can keep it out of the recording.
    fn open_recbar(&mut self, config: &Config) {
        let host = record_bar_host(&crate::capture::facts());
        if host != BarHost::Outside || self.runtime.plan.host != Host::Layer {
            log::info!("recording: no bar here ({host:?}); the hotkey stops it");
            return;
        }
        let (Some(service), Some(rect), Some(running)) = (
            &self.runtime.service,
            self.capture.rec_rect,
            self.capture.live.running(),
        ) else {
            return;
        };
        let reduced_motion = self
            .capture
            .motion
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
            .unwrap_or(false);
        let theme = popup_setup(config);
        service.open_recbar(RecBarSetup {
            clock: Some(running.clock.clone()),
            fps: running.fps,
            label: running.label.clone(),
            rect,
            steps: None,
            paused: Arc::default(),
            pointer_on_bar: Arc::default(),
            palette: theme.palette,
            mode: theme.mode,
            reduced_motion,
        });
    }

    /// The service the overlay opens in: the one that runs for the
    /// clipboard, else one started now for capture alone (the window host).
    fn capture_service(&mut self, shared: &Shared) -> Option<&Service> {
        if self.runtime.service.is_some() {
            return self.runtime.service.as_ref();
        }
        if self.capture.own_service.is_none() {
            // The window host watches by polling and pastes through the
            // portal; this service does neither, only draws.
            let plan = Plan {
                watch: Watch::Poll,
                paste: PasteVia::Portal,
                ..self.runtime.plan
            };
            match Service::spawn(plan, shared.clone()) {
                Ok(service) => {
                    log::info!("capture: started a Wayland connection for the overlay");
                    self.capture.own_service = Some(service);
                }
                Err(e) => log::error!("capture: no Wayland connection for the overlay ({e})"),
            }
        }
        self.capture.own_service.as_ref()
    }

    // --- Steps ---------------------------------------------------------------

    /// Whether Steps is starting, running or stopping.
    pub(super) fn steps_busy(&self) -> bool {
        self.steps.is_some() || self.capture.stepping.opening.is_some()
    }

    /// Whether a session runs and has not been asked to stop.
    fn steps_running(&self) -> bool {
        self.steps.is_some() && !self.capture.stepping.stopping
    }

    /// Whether Steps' keys are to be bound: from the start until it is
    /// stopped or cancelled.
    fn steps_keys_wanted(&self) -> bool {
        self.capture.stepping.opening.is_some() || self.steps_running()
    }

    /// The Steps key (`windup --steps`): stops the session that runs, else
    /// starts one at once with the saved choices.
    pub(super) fn steps_command(
        &mut self,
        config: &mut Config,
        shared: &Shared,
        commands: &Receiver<Command>,
    ) {
        if self.steps_keys_wanted() {
            self.stop_steps();
        } else if !self.steps_busy() {
            self.start_steps(None, config, shared, commands);
        }
    }

    /// The step key (`windup --step`): one step, now.
    pub(super) fn step_command(&mut self) {
        if self.steps_running()
            && let Some(s) = &self.steps
        {
            s.trigger(Trigger::Hotkey);
        }
    }

    /// The launcher's Steps, as the overlay closed on it.
    pub(super) fn start_steps_from(
        &mut self,
        choice: StepsChoice,
        config: &mut Config,
        shared: &Shared,
        commands: &Receiver<Command>,
    ) {
        self.start_steps(Some(choice), config, shared, commands);
    }

    /// Starts a Steps session: `choice` from the launcher, or `None` for
    /// the key, with the saved choices. The camera is opened on a worker;
    /// the session and its bar start once it is. The gate stays shut
    /// throughout, so no capture or recording starts meanwhile.
    fn start_steps(
        &mut self,
        choice: Option<StepsChoice>,
        config: &mut Config,
        shared: &Shared,
        commands: &Receiver<Command>,
    ) {
        if !config.is_tool_enabled(ToolId::Steps) {
            log::debug!("steps: turned off");
            return;
        }
        if self.steps_busy() || self.capture.live.busy() {
            log::debug!("steps: a recording or Steps session is under way");
            return;
        }
        if self.review_open() {
            self.notice(FINISH_REVIEW_FIRST, config, shared, commands);
            return;
        }
        if let Err(why) = availability::current(&config.video).steps {
            self.notice(why, config, shared, commands);
            return;
        }
        if !self.capture.gate.begin() {
            log::info!("steps: a capture is under way; ignored");
            return;
        }
        self.capture.gate.recording();
        let trigger = choice.map_or(config.steps.trigger, |c| c.trigger);
        let layer = self.runtime.plan.host == Host::Layer;
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("steps-open".into())
            .spawn(move || {
                let _ = tx.send(open_steps(trigger, layer));
            });
        match spawned {
            Ok(_) => {
                log::info!("steps: starting");
                self.capture.stepping.opening = Some(rx);
            }
            Err(e) => {
                self.capture.gate.end();
                let text = format!("Steps could not start: {e}");
                self.notice(&text, config, shared, commands);
            }
        }
    }

    /// The camera is open (or not): the session, its click watch and its
    /// bar start.
    fn steps_opened(
        &mut self,
        opened: Result<Opened, String>,
        config: &mut Config,
        shared: &Shared,
        commands: &Receiver<Command>,
    ) {
        let Opened {
            camera,
            pointer,
            monitors,
            host,
            trigger,
            reduced_motion,
        } = match opened {
            Ok(o) => o,
            Err(why) => {
                log::warn!("steps: no camera: {why}");
                self.capture.gate.end();
                let text = format!("Steps could not start: {why}");
                self.notice(&text, config, shared, commands);
                return;
            }
        };
        let service = self
            .runtime
            .service
            .clone()
            .filter(|_| host == BarHost::Outside);
        // No bar, nothing to hide: the window host, GNOME.
        let host = if service.is_some() {
            host
        } else {
            BarHost::Hidden
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
        let s = &mut self.capture.stepping;
        s.count.store(0, Ordering::Relaxed);
        s.paused.store(false, Ordering::Relaxed);
        s.pointer_on_bar.store(false, Ordering::Relaxed);
        let facts = Arc::new(Mutex::new(BarFacts {
            rect: bar_monitor(host, pointer, &monitors),
            pointer_on_bar: false,
            host,
            excluded: false,
        }));
        let conceal: Arc<dyn Conceal> = match &service {
            Some(service) => Arc::new(LayerConceal::new(service.clone())),
            None => Arc::new(NoConceal),
        };
        let cfg = windup_core::config::StepsConfig {
            trigger,
            ..config.steps.clone()
        };
        let takes = takes(trigger, clicks);
        match StepsLive::start(&cfg, takes, camera, conceal, facts.clone(), (tx, rx)) {
            Ok(live) => {
                self.steps = Some(live);
                s.facts = Some(facts);
                s.clicks = watch;
                s.stopping = false;
                if let (Some(service), Some(rect)) = (service, pointer) {
                    let theme = popup_setup(config);
                    service.open_recbar(RecBarSetup {
                        clock: None,
                        fps: 0,
                        label: String::new(),
                        rect,
                        steps: Some(Arc::clone(&s.count)),
                        paused: Arc::clone(&s.paused),
                        pointer_on_bar: Arc::clone(&s.pointer_on_bar),
                        palette: theme.palette,
                        mode: theme.mode,
                        reduced_motion,
                    });
                } else {
                    log::info!("steps: no bar here ({host:?}); the Steps key stops it");
                }
            }
            Err(why) => {
                self.capture.gate.end();
                let text = format!("Steps could not start: {why}");
                self.notice(&text, config, shared, commands);
            }
        }
    }

    fn toggle_steps_pause(&mut self) {
        if let Some(s) = &self.steps {
            let paused = s.toggle_pause();
            self.capture
                .stepping
                .paused
                .store(paused, Ordering::Relaxed);
            self.recbar_changed();
        }
    }

    fn recbar_changed(&self) {
        if let Some(service) = &self.runtime.service {
            service.recbar_changed();
        }
    }

    /// Stop, from the bar or a key: the bar and the keys go at once, the
    /// review opens once the last step is written.
    pub(super) fn stop_steps(&mut self) {
        let s = &mut self.capture.stepping;
        if s.opening.take().is_some() {
            // Not started yet: nothing was taken. The worker's camera is
            // dropped when it finds nobody listening.
            log::info!("steps: stopped before the first step");
            if self.steps.is_none() {
                self.capture.gate.end();
            }
        }
        let s = &mut self.capture.stepping;
        if let Some(live) = self.steps.as_mut()
            && !s.stopping
        {
            log::info!("steps: stopping");
            live.stop();
            s.stopping = true;
        }
        s.clicks = None;
        self.close_recbar();
    }

    /// Esc: the session is thrown away. Everything of it on the screen goes
    /// now; its camera thread removes the folder whenever it is done.
    pub(super) fn cancel_steps(&mut self) {
        let s = &mut self.capture.stepping;
        s.opening = None;
        s.stopping = false;
        s.facts = None;
        s.clicks = None;
        if let Some(mut live) = self.steps.take() {
            log::info!("steps: cancelling");
            live.cancel();
        }
        self.capture.gate.end();
        self.close_recbar();
    }

    /// Starts, polls and follows a Steps session; every tick.
    fn poll_steps(&mut self, config: &mut Config, shared: &Shared, commands: &Receiver<Command>) {
        let opened = match self
            .capture
            .stepping
            .opening
            .as_ref()
            .map(Receiver::try_recv)
        {
            Some(Ok(opened)) => Some(opened),
            Some(Err(TryRecvError::Disconnected)) => {
                Some(Err("the camera could not be opened".to_string()))
            }
            Some(Err(TryRecvError::Empty)) | None => None,
        };
        if let Some(opened) = opened {
            self.capture.stepping.opening = None;
            self.steps_opened(opened, config, shared, commands);
        }
        // The camera thread decides from it whether a click was on the bar.
        if let Some(facts) = &self.capture.stepping.facts {
            let on_bar = self.capture.stepping.pointer_on_bar.load(Ordering::Relaxed);
            facts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pointer_on_bar = on_bar;
        }
        let events = self.steps.as_mut().map(StepsLive::poll);
        for event in events.into_iter().flatten() {
            match event {
                StepsEvent::Step(n) => {
                    self.capture.stepping.count.store(n, Ordering::Relaxed);
                    self.recbar_changed();
                }
                StepsEvent::Over(end) => self.steps_over(end, config, shared, commands),
            }
        }
        if let Some((child, _)) = self.review.as_mut()
            && !matches!(child.try_wait(), Ok(None))
        {
            log::info!("steps: review child exited");
            self.review = None;
        }
    }

    /// The session ended: the review, a notice, or both.
    fn steps_over(
        &mut self,
        end: StepsEnd,
        config: &mut Config,
        shared: &Shared,
        commands: &Receiver<Command>,
    ) {
        self.steps = None;
        let s = &mut self.capture.stepping;
        s.stopping = false;
        s.facts = None;
        s.clicks = None;
        self.capture.gate.end();
        self.close_recbar();
        // Let Esc go before a notice: on the window host it blocks.
        self.sync_live_keys(config);
        match end {
            StepsEnd::Review(dir) => self.open_review(&dir, config, shared, commands),
            StepsEnd::Discarded => {}
            StepsEnd::Empty => self.notice(NO_STEPS, config, shared, commands),
            StepsEnd::Failed { why, review } => {
                if let Some(dir) = review {
                    self.open_review(&dir, config, shared, commands);
                }
                self.notice(&format!("Steps stopped: {why}"), config, shared, commands);
            }
        }
    }

    /// Whether the review child is still open.
    fn review_open(&mut self) -> bool {
        match self.review.as_mut().map(|(child, _)| child.try_wait()) {
            Some(Ok(None)) => true,
            Some(_) => {
                self.review = None;
                false
            }
            None => false,
        }
    }

    /// Opens the review of the session in `dir`: `windup --steps-review`.
    fn open_review(
        &mut self,
        dir: &Path,
        config: &mut Config,
        shared: &Shared,
        commands: &Receiver<Command>,
    ) {
        if self.review_open() {
            // Starting is refused while one is open, so this cannot be
            // reached by hand; the folder is left for the next start.
            log::warn!("steps: a review is open already; {} is left", dir.display());
            return;
        }
        let spawned = std::env::current_exe().and_then(|exe| {
            std::process::Command::new(exe)
                .arg("--steps-review")
                .arg(dir)
                .spawn()
        });
        match spawned {
            Ok(child) => {
                log::info!("steps: review opened");
                self.review = Some((child, dir.to_path_buf()));
            }
            Err(e) => {
                log::error!("steps: the review did not start: {e}");
                let text = format!("The steps could not be opened: {e}");
                self.notice(&text, config, shared, commands);
            }
        }
    }

    /// The review asked for the editor on a step (`Request::EditStep`): the
    /// overlay opens on that picture alone, on the output the step was
    /// taken on (else the first), and Enter writes the edits back.
    pub(super) fn edit_step(
        &mut self,
        path: PathBuf,
        config: &mut Config,
        shared: &Shared,
        commands: &Receiver<Command>,
    ) {
        // Only a step of the open review is ever written over.
        let review_dir = if self.review_open() {
            self.review.as_ref().map(|(_, dir)| dir.clone())
        } else {
            None
        };
        let doc = review_dir.as_deref().and_then(|d| StepsDoc::load(d).ok());
        let Some(step) = editable_step(&path, review_dir.as_deref(), doc.as_ref()) else {
            log::warn!("steps: not a step of the open review: {}", path.display());
            return;
        };
        let open = self.capture.gate != Gate::Idle && !self.steps_busy();
        if let Some(why) = edit_refusal(open, self.steps_busy()) {
            log::info!("steps: the editor cannot open now");
            self.notice(why, config, shared, commands);
            return;
        }
        self.capture.gate.begin();
        let image = match image::open(&path) {
            Ok(img) => img.to_rgba8(),
            Err(e) => {
                log::warn!("steps: cannot read {}: {e}", path.display());
                self.capture.gate.end();
                self.notice(EDIT_UNREADABLE, config, shared, commands);
                return;
            }
        };
        let named = step.monitor;
        let outputs = super::wayland::screencopy::output_layout_from_env().unwrap_or_default();
        let Some((name, layout)) = outputs
            .iter()
            .find(|(n, _)| !named.is_empty() && *n == named)
            .or(outputs.first())
            .cloned()
        else {
            self.capture.gate.end();
            self.notice(
                CaptureError::Unsupported.message(),
                config,
                shared,
                commands,
            );
            return;
        };
        log::info!("steps: editing {} on {name}", path.display());
        // The picture covers the output, as a frozen monitor does; one of a
        // monitor since changed in shape is stretched to it.
        let mut session = Session::edit_image(image, layout, &config.screenshot);
        if let Some(m) = session.shot.monitors.first_mut() {
            m.name = name;
        }
        self.capture.stepping.editing = Some(path);
        self.open_overlay(session, false, config, shared, commands);
    }
}

/// The step `path` names, when the editor may write it: a review is open
/// (`review_dir`, whose `steps.json` is `doc`), the path has no `.` or `..`
/// in it, its folder is the review's, and its file is a step listed there.
pub(super) fn editable_step(
    path: &Path,
    review_dir: Option<&Path>,
    doc: Option<&StepsDoc>,
) -> Option<Step> {
    use std::path::Component;
    let (dir, doc) = (review_dir?, doc?);
    let plain = |p: &Path| {
        p.components()
            .all(|c| !matches!(c, Component::ParentDir | Component::CurDir))
    };
    if !plain(path) || !plain(dir) {
        return None;
    }
    let same = path
        .parent()
        .is_some_and(|p| p.components().eq(dir.components()));
    if !same {
        return None;
    }
    let file = path.file_name()?.to_str()?;
    doc.steps.iter().find(|s| s.file == file).cloned()
}

/// Opens a Steps camera and finds what its bar is placed from; on the
/// worker, since a portal may keep `open_camera` waiting a long time. The
/// ScreenCast restore token is saved afterwards whether or not a camera
/// opened, as video does. `layer`: whether this host can show a bar.
fn open_steps(trigger: StepTrigger, layer: bool) -> Result<Opened, String> {
    let motion = portal::ask_reduced_motion();
    let facts = crate::capture::facts();
    let host = if layer {
        record_bar_host(&facts)
    } else {
        BarHost::Hidden
    };
    let saved = Config::load_screencast_token();
    let token = Arc::new(Mutex::new(saved.clone()));
    let opened = open_camera(&steps_order(&facts), &token);
    let next = token.lock().ok().and_then(|t| t.clone());
    if next != saved {
        Config::save_screencast_token(next.as_deref());
    }
    let (mut camera, route) = opened?;
    let monitors: Vec<DRect> = super::wayland::screencopy::output_layout_from_env()
        .map(|o| o.into_iter().map(|(_, r)| r).collect())
        .unwrap_or_default();
    let pointer = camera.monitor(None).or_else(|| monitors.first().copied());
    log::info!("steps: camera {route:?}, bar {host:?}");
    Ok(Opened {
        camera,
        pointer,
        monitors,
        host,
        trigger,
        reduced_motion: motion.recv_timeout(MOTION_WAIT).unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_kind_follows_the_session() {
        assert_eq!(live_kind(false, false), None);
        assert_eq!(live_kind(true, false), Some(LiveKind::Video));
        assert_eq!(live_kind(false, true), Some(LiveKind::Steps));
        // Never both; Steps is the one that answers if it were.
        assert_eq!(live_kind(true, true), Some(LiveKind::Steps));
    }

    #[test]
    fn a_stopped_session_is_not_cancelled() {
        assert!(Stepping::default().cancellable());
        let stopping = Stepping {
            stopping: true,
            ..Stepping::default()
        };
        assert!(!stopping.cancellable());
    }

    #[test]
    fn only_a_listed_step_of_the_open_review_is_editable() {
        let mut doc = StepsDoc::new(&jiff::Zoned::now());
        let file = doc.next_file();
        doc.push(Step {
            file,
            cause: windup_core::steps::model::StepCause::Hotkey,
            pointer: None,
            scale: 1.0,
            monitor: "DP-1".into(),
            caption: String::new(),
            at_ms: 0,
        });
        let dir = Path::new("/c/windup/steps/2026-10-30 142501");
        let ok = |p: &str| editable_step(Path::new(p), Some(dir), Some(&doc));
        assert_eq!(
            ok("/c/windup/steps/2026-10-30 142501/01.png").map(|s| s.monitor),
            Some("DP-1".to_string())
        );
        // Another session's folder.
        assert!(ok("/c/windup/steps/2026-10-29 090000/01.png").is_none());
        // No review open.
        assert!(
            editable_step(
                Path::new("/c/windup/steps/2026-10-30 142501/01.png"),
                None,
                None
            )
            .is_none()
        );
        // A file the session does not list.
        assert!(ok("/c/windup/steps/2026-10-30 142501/02.png").is_none());
        assert!(ok("/c/windup/steps/2026-10-30 142501/steps.json").is_none());
        // Climbing out, or in and out again.
        assert!(ok("/c/windup/steps/2026-10-30 142501/../2026-10-30 142501/01.png").is_none());
        assert!(ok("/c/windup/steps/2026-10-30 142501/x/../01.png").is_none());
    }

    fn r(x: f32) -> DRect {
        DRect {
            x,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        }
    }

    #[test]
    fn the_steps_bar_stands_for_the_monitor_it_goes_on() {
        let (a, b) = (r(0.0), r(1920.0));
        assert_eq!(bar_monitor(BarHost::Outside, Some(a), &[a, b]), Some(b));
        assert_eq!(bar_monitor(BarHost::Outside, Some(b), &[a, b]), Some(a));
        // One monitor: the bar shares it, and is hidden for every shot.
        assert_eq!(bar_monitor(BarHost::Outside, Some(a), &[a]), Some(a));
        assert_eq!(bar_monitor(BarHost::Outside, Some(a), &[]), Some(a));
        assert_eq!(bar_monitor(BarHost::Hidden, Some(a), &[a, b]), None);
        assert_eq!(bar_monitor(BarHost::Outside, None, &[a, b]), None);
    }
}
