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
//! While it runs, its keys (pause, stop, Esc) are bound where the desktop
//! lets Windup bind them (`platform::recording_keys`): temporary entries in
//! COSMIC's shortcuts, or a second GlobalShortcuts portal session. They run
//! `windup --record-pause` and so on, like every Wayland hotkey.

use super::wayland::{CaptureEnd, CaptureSetup, NoticeSetup, RecBarSetup, Service, Shared};
use super::{Background, copy_image, cosmic, popup_setup, portal, window};
use crate::capture::gate::{Gate, Reply};
use crate::capture::{CaptureError, finish, grab_all, window_rects};
use crate::platform::{Host, PasteVia, Plan, RecordingKeys, Watch, recording_keys};
use crate::video::availability;
use crate::video::live::{Ask, Live, LiveEvent};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;
use windup_core::capture::WindowRect;
use windup_core::capture::backend::{BarHost, record_bar_host};
use windup_core::capture::session::{Action, CaptureMode, ModeAvailability, Outcome, Session};
use windup_core::capture::{DRect, DesktopShot};
use windup_core::ipc::Command;
use windup_core::{Config, ToolId};
use windup_ui::capture::recbar::RecBarAction;

/// How long the reduced-motion answer may take once the picture is in.
const MOTION_WAIT: Duration = Duration::from_millis(300);

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
    /// The recording's keys, while they are meant to be bound.
    keys: Option<BoundKeys>,
}

/// How a recording's keys were bound, so they can be let go at its end.
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
        }
    }
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
        // Refused, and left on Screenshot, when Video cannot work here.
        session.act(Action::SetMode(self.capture.mode));
        let theme = popup_setup(config);
        let setup = CaptureSetup {
            session,
            keys: config.screenshot.keys.clone(),
            palette: theme.palette,
            mode: theme.mode,
            reduced_motion: grab.reduced_motion,
        };
        let opened = self
            .capture_service(shared)
            .is_some_and(|service| service.open_capture(setup));
        if opened {
            self.capture.gate.opened();
        } else {
            self.capture.gate.end();
            let failed = CaptureError::Failed(String::new());
            self.notice(failed.message(), config, shared, commands);
        }
    }

    /// The overlay closed: remember its choices, then copy and save, or
    /// start recording.
    pub(super) fn capture_done(
        &mut self,
        end: CaptureEnd,
        config: &mut Config,
        shared: &Shared,
        commands: &Receiver<Command>,
    ) {
        self.capture.gate.end();
        let record = matches!(end.outcome, Outcome::Record { .. });
        self.change_config(config, |c| {
            finish::remember(&mut c.screenshot, &end.session);
            if record {
                finish::remember_video(&mut c.video, &end.session);
            }
        });
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
        let mut errors: Vec<String> = end.error.into_iter().collect();
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
        if self.capture.live.busy() || !config.is_tool_enabled(ToolId::Video) {
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

    /// Binds the recording's keys once it has started and lets them go when
    /// it is stopped or cancelled; every tick.
    fn sync_recording_keys(&mut self, config: &Config) {
        let wanted = self.capture.live.started_keys_wanted();
        if wanted == self.capture.keys.is_some() {
            return;
        }
        if !wanted {
            if let Some(BoundKeys::Cosmic) = self.capture.keys.take() {
                cosmic::remove_temporary();
            }
            // A portal session closes as it is dropped.
            return;
        }
        // The portal is simply tried: where it is missing it says so.
        let how = recording_keys(self.runtime.plan.hotkeys, cosmic::is_cosmic(), true);
        self.capture.keys = Some(match how {
            RecordingKeys::Cosmic => {
                let keys: Vec<(windup_core::Shortcut, String)> =
                    windup_core::hotkeys::recording_bindings(config)
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
                _session: portal::recording_shortcuts(
                    windup_core::hotkeys::recording_keys_untaken(config),
                ),
            },
            RecordingKeys::Hook | RecordingKeys::XGrab | RecordingKeys::BarOnly => {
                BoundKeys::Nothing
            }
        });
    }

    /// Pause, Resume or Stop from the bar.
    pub(super) fn recbar_action(&mut self, action: RecBarAction) {
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

    /// Moves the recording along; every tick.
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
                    self.sync_recording_keys(config);
                    self.notice_with(&ended.message, ended.saved, config, shared, commands);
                }
                LiveEvent::Token(t) => {
                    self.change_config(config, |c| c.video.screencast_token = Some(t));
                }
            }
        }
        self.sync_recording_keys(config);
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
            clock: running.clock.clone(),
            fps: running.fps,
            label: running.label.clone(),
            rect,
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
}
