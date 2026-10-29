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

use super::wayland::{CaptureEnd, CaptureSetup, NoticeSetup, Service, Shared};
use super::{Background, copy_image, popup_setup, portal, window};
use crate::capture::gate::{Gate, Reply};
use crate::capture::{CaptureError, finish, grab_all, window_rects};
use crate::platform::{Host, PasteVia, Plan, Watch};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;
use windup_core::capture::DesktopShot;
use windup_core::capture::WindowRect;
use windup_core::capture::session::{CaptureMode, Session};
use windup_core::ipc::Command;
use windup_core::{Config, ToolId};

/// How long the reduced-motion answer may take once the picture is in.
const MOTION_WAIT: Duration = Duration::from_millis(300);

/// What the worker brings back.
pub(super) struct Grab {
    shot: DesktopShot,
    windows: Vec<WindowRect>,
    reduced_motion: bool,
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
                let grabbed = grab_all().map(|shot| {
                    let windows = window_rects(&shot);
                    Grab {
                        shot,
                        windows,
                        reduced_motion: motion.recv_timeout(MOTION_WAIT).unwrap_or(false),
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
        let mut session = Session::new(grab.shot, grab.windows, &config.screenshot);
        session.mode = self.capture.mode;
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

    /// The overlay closed: remember its choices, then copy and save.
    pub(super) fn capture_done(
        &mut self,
        end: CaptureEnd,
        config: &mut Config,
        shared: &Shared,
        commands: &Receiver<Command>,
    ) {
        self.capture.gate.end();
        self.change_config(config, |c| {
            finish::remember(&mut c.screenshot, &end.session)
        });
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
        log::info!("capture: notice: {message}");
        let theme = popup_setup(config);
        if self.runtime.plan.host == Host::Layer
            && let Some(service) = &self.runtime.service
        {
            service.notice(NoticeSetup {
                message: message.to_owned(),
                palette: theme.palette,
                mode: theme.mode,
            });
            return;
        }
        let later = window::notice(message, theme.palette, theme.mode, commands);
        for command in later {
            self.command(command, config, shared, commands);
        }
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
