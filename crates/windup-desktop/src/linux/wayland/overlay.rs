//! The capture overlay on Wayland.
//!
//! The overlay is one surface per output, each showing that output's part of
//! the frozen picture through the shared [`CaptureView`]:
//! - [`Role::Layer`] (COSMIC, KDE, wlroots): a layer surface on the overlay
//!   layer of its output, anchored to every edge, taking the keyboard;
//! - [`Role::Fullscreen`] (GNOME, Weston): an `xdg_toplevel` made fullscreen
//!   on its output.
//!
//! A surface is shown by the compositor when its first buffer is attached,
//! which is the first frame wgpu presents: nothing appears half drawn.
//!
//! All the surfaces share one egui context, each as a viewport of its own, so
//! the frozen screens, the loupe and the previews are textures of one
//! renderer whichever surface draws them. Any input on any surface redraws
//! them all, so the selection and its ants move on every output together.
//! The context and renderer are made for one capture and dropped with it,
//! and the picture's textures with them.
//!
//! Surface coordinates are logical, as are the picture's desktop units, so a
//! surface covering an output in full is the output's logical rectangle and
//! the view's `Mapping` turns its points into desktop units unchanged.

use super::gpu::{Painted, paint};
use super::pane::{Canvas, Pane, wl_cursor};
use super::{ServiceEvent, State, screencopy};
use crate::capture::CaptureError;
use crate::capture::surfaces::{Output, logical_size, match_outputs};
use crate::platform::Host;
use calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::seat::pointer::CursorIcon;
use std::time::{Duration, Instant};
use windup_core::capture::session::{Outcome, Session};
use windup_core::config::EditorKeys;
use windup_ui::capture::CaptureView;
use windup_ui::{Mode, Palette, apply_theme};

/// Repaint delays longer than this mean "not until something happens".
pub(super) const IDLE: Duration = Duration::from_secs(5);
/// The shortest wait between frames asked for with a delay.
pub(super) const MIN_FRAME: Duration = Duration::from_millis(8);

/// What kind of surface covers an output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// `zwlr_layer_shell_v1`, overlay layer.
    Layer,
    /// `xdg_toplevel.set_fullscreen(output)`.
    Fullscreen,
}

/// Everything the overlay needs, from the background loop.
pub struct CaptureSetup {
    pub session: Session,
    pub keys: EditorKeys,
    pub palette: Palette,
    pub mode: Mode,
    pub reduced_motion: bool,
}

/// How a capture ended, for the background loop to act on.
pub struct CaptureEnd {
    pub outcome: Outcome,
    /// Given back so its choices (target, colour, sizes) are remembered.
    pub session: Box<Session>,
    /// The settings button was clicked; the capture was cancelled for it.
    pub open_settings: bool,
    /// The overlay could not open; the text is for the notice.
    pub error: Option<String>,
}

pub(super) struct Overlay {
    pub(super) session: Session,
    pub(super) view: CaptureView,
    pub(super) keys: EditorKeys,
    pub(super) canvas: Canvas,
    pub(super) panes: Vec<Pane>,
    /// The pane with the keyboard, which key presses go to.
    pub(super) keyboard: Option<usize>,
    /// The pane the pointer is on, whose cursor is shown.
    pub(super) pointer: Option<usize>,
    pub(super) cursor: Option<CursorIcon>,
    /// When the pending wake-up timer fires.
    pub(super) wake_at: Option<Instant>,
    pub(super) opened: Instant,
}

impl State {
    /// The role surfaces take on this compositor, if it offers one.
    fn role(&self) -> Option<Role> {
        match (self.host, &self.layer_shell, &self.xdg_shell) {
            (Host::Layer, Some(_), _) => Some(Role::Layer),
            (_, _, Some(_)) => Some(Role::Fullscreen),
            (_, Some(_), None) => Some(Role::Layer),
            _ => None,
        }
    }

    pub(super) fn open_capture(&mut self, setup: Box<CaptureSetup>) {
        let CaptureSetup {
            session,
            keys,
            palette,
            mode,
            reduced_motion,
        } = *setup;
        if self.overlay.is_some() {
            log::warn!("capture: one is already open");
            self.capture_done(session, Outcome::Cancel, false, None);
            return;
        }
        self.close_popup();
        self.close_notice();
        let failed = || Some(CaptureError::Failed(String::new()).message().to_owned());
        let Some(role) = self.role() else {
            log::warn!("capture: neither layer-shell nor xdg-shell; no overlay here");
            self.capture_done(session, Outcome::Cancel, false, failed());
            return;
        };

        let outputs = screencopy::outputs_of(&self.output_state);
        let pairs = {
            let monitors: Vec<Output> = session
                .shot
                .monitors
                .iter()
                .map(|m| Output {
                    name: &m.name,
                    layout: m.layout,
                })
                .collect();
            let known: Vec<Output> = outputs
                .iter()
                .map(|o| Output {
                    name: &o.name,
                    layout: o.layout,
                })
                .collect();
            match_outputs(&monitors, &known)
        };
        log::info!(
            "capture: opening the overlay ({role:?}) on {} of {} monitors",
            pairs.iter().flatten().count(),
            pairs.len()
        );

        let mut panes = Vec::new();
        for (monitor, output) in pairs.iter().enumerate() {
            let Some(o) = output.map(|i| &outputs[i]) else {
                log::warn!("capture: no output shows monitor {monitor} any more");
                continue;
            };
            let scale = self
                .output_state
                .info(&o.output)
                .map_or(1.0, |i| i.scale_factor.max(1) as f64);
            let id = egui::ViewportId::from_hash_of(("windup-capture", monitor));
            let fallback = logical_size(session.shot.monitors[monitor].layout);
            match self.new_pane(role, Some(&o.output), None, fallback, scale, id) {
                Ok(mut pane) => {
                    pane.monitor = monitor;
                    panes.push(pane);
                }
                Err(e) => log::error!("capture: {e}"),
            }
        }
        if panes.is_empty() {
            self.capture_done(session, Outcome::Cancel, false, failed());
            return;
        }
        let mut view = CaptureView::new();
        view.reduced_motion = reduced_motion;
        self.overlay = Some(Overlay {
            session,
            view,
            keys,
            canvas: Canvas::new(palette, mode),
            panes,
            keyboard: None,
            pointer: None,
            cursor: None,
            wake_at: None,
            opened: Instant::now(),
        });
        let _ = self.conn.flush();
    }

    /// Ends the capture: the surfaces go, and the background loop is told.
    pub(super) fn end_capture(
        &mut self,
        outcome: Outcome,
        open_settings: bool,
        error: Option<String>,
    ) {
        let Some(overlay) = self.overlay.take() else {
            return;
        };
        let Overlay {
            session,
            panes,
            canvas,
            ..
        } = overlay;
        drop(panes);
        drop(canvas);
        let _ = self.conn.flush();
        self.capture_done(session, outcome, open_settings, error);
    }

    fn capture_done(
        &mut self,
        session: Session,
        outcome: Outcome,
        open_settings: bool,
        error: Option<String>,
    ) {
        log::info!(
            "capture: closed ({})",
            match &outcome {
                Outcome::Copy(_) => "copy",
                Outcome::Save(_) => "save",
                Outcome::CopyAndSave(_) => "copy and save",
                Outcome::Record { .. } => "record",
                Outcome::Cancel if open_settings => "settings",
                Outcome::Cancel => "cancel",
            }
        );
        let end = CaptureEnd {
            outcome,
            session: Box::new(session),
            open_settings,
            error,
        };
        let _ = self
            .shared
            .events
            .send(ServiceEvent::CaptureDone(Box::new(end)));
    }

    pub(super) fn draw_capture(&mut self) {
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        if !overlay.panes.iter().any(|p| p.configured) {
            return;
        }
        // The first surface's wgpu surface picks the adapter; it is taken
        // out for the call so `self` can be borrowed whole.
        if self.gpu.is_none() {
            let Some(i) = overlay.panes.iter().position(|p| p.wgpu.is_some()) else {
                return;
            };
            let surface = overlay.panes[i].wgpu.take();
            let made = surface.as_ref().map(|s| self.ensure_gpu(s));
            if let Some(o) = self.overlay.as_mut() {
                o.panes[i].wgpu = surface;
            }
            if let Some(Err(e)) = made {
                log::error!("capture: {e}");
                let error = CaptureError::Failed(String::new()).message().to_owned();
                self.end_capture(Outcome::Cancel, false, Some(error));
                return;
            }
        }
        let (Some(gpu), Some(overlay)) = (self.gpu.as_ref(), self.overlay.as_mut()) else {
            return;
        };
        let Overlay {
            session,
            view,
            keys,
            canvas,
            panes,
            keyboard,
            pointer,
            cursor,
            opened,
            ..
        } = overlay;
        let time = canvas.time();
        let renderer = canvas.renderer.get_or_insert_with(|| {
            egui_wgpu::Renderer::new(&gpu.device, gpu.format, None, 1, false)
        });
        let mut outcome = None;
        let mut again: Option<Duration> = None;
        let mut wanted_cursor = None;
        for (i, pane) in panes.iter_mut().enumerate() {
            if !pane.configured || pane.wgpu.is_none() {
                continue;
            }
            let px = pane.sync(gpu);
            let raw = pane.raw_input(time, self.pane_modifiers, *keyboard == Some(i));
            let monitor = pane.monitor;
            let (palette, mode) = (canvas.palette, canvas.mode);
            let mut output = canvas.ctx.run(raw, |ctx| {
                apply_theme(ctx, palette, mode);
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        if let Some(o) = view.monitor_ui(ui, session, monitor, &palette, keys) {
                            outcome = Some(o);
                        }
                    });
            });
            if *pointer == Some(i) {
                wanted_cursor = Some(wl_cursor(output.platform_output.cursor_icon));
            }
            let delay = output
                .viewport_output
                .get(&pane.id)
                .map(|v| v.repaint_delay)
                .filter(|d| *d < IDLE);
            let Some(surface) = pane.wgpu.as_ref() else {
                continue;
            };
            let painted = paint(
                &gpu.device,
                &gpu.queue,
                renderer,
                &canvas.ctx,
                surface,
                &mut output,
                px,
                wgpu::Color::BLACK,
            );
            let delay = match painted {
                Painted::Shown => {
                    if !pane.shown {
                        pane.shown = true;
                        log::info!(
                            "capture: output {i} shown, {} ms after opening",
                            opened.elapsed().as_millis()
                        );
                    }
                    delay
                }
                Painted::Reconfigure => {
                    pane.synced = None;
                    Some(Duration::ZERO)
                }
                Painted::Skipped => Some(Duration::from_millis(16)),
            };
            again = match (again, delay) {
                (Some(a), Some(d)) => Some(a.min(d)),
                (a, d) => a.or(d),
            };
        }
        if let Some(icon) = wanted_cursor
            && *cursor != Some(icon)
        {
            *cursor = Some(icon);
            if let Some(p) = &self.pointer {
                let _ = p.set_cursor(&self.conn, icon);
            }
        }
        let settings = view.take_open_settings();
        if let Some(outcome) = outcome {
            self.end_capture(outcome, false, None);
        } else if settings {
            self.end_capture(Outcome::Cancel, true, None);
        } else if let Some(delay) = again {
            self.wake_capture_in(delay);
        }
    }

    /// Redraws every overlay surface after `delay`, unless a redraw is due
    /// sooner already: one timer at a time, however many surfaces ask.
    fn wake_capture_in(&mut self, delay: Duration) {
        if delay.is_zero() {
            self.queue_pane_redraw();
            return;
        }
        let at = Instant::now() + delay.max(MIN_FRAME);
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        if overlay.wake_at.is_some_and(|w| w <= at) {
            return;
        }
        overlay.wake_at = Some(at);
        let _ =
            self.handle
                .insert_source(Timer::from_deadline(at), move |_, _, state: &mut State| {
                    if let Some(o) = state.overlay.as_mut()
                        && o.wake_at == Some(at)
                    {
                        o.wake_at = None;
                    }
                    state.queue_pane_redraw();
                    TimeoutAction::Drop
                });
    }
}
