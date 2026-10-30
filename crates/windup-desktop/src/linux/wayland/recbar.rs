//! The recording bar on the layer host: a layer surface on the overlay
//! layer, beside the recorded area and never over it (`place_bar`), on
//! whichever output that slot is on, at margins from the output's corner.
//! The surface is the bar plus room under it for the tooltips
//! (`recbar::host_size`), placed whole, so the room too stays out of the
//! recording. Keyboard interactivity is on demand: the bar takes the keys
//! (Space, Enter) only once it is clicked, and the recorded window keeps
//! them otherwise. No slot, no bar: the hotkey still stops the recording.
//!
//! The bar reads the recording's clock directly (`RecClock`), so the timer
//! and the pulse need nothing from the background loop; what the user asks
//! for goes back as `ServiceEvent::RecBar`.
//!
//! A Steps session shows the same bar with its step count, off the
//! pointer's monitor when there is another (`steps_bar_slot`). Where a shot
//! is of the bar's monitor, the camera thread hides it for the instant of
//! the shot ([`LayerConceal`]): a null buffer unmaps the layer surface, and
//! showing it maps it afresh. Whether the pointer is on the bar is told
//! back through `pointer_on_bar`, since a click read from evdev has no
//! position to compare with the bar's.

use super::Service;
use super::gpu::{Painted, card_clear, paint};
use super::overlay::{IDLE, MIN_FRAME, Role};
use super::pane::{Canvas, Pane, Shell, Spot};
use super::{ServiceEvent, State, screencopy};
use crate::steps::camera::Conceal;
use crate::video::run::RecClock;
use calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::shell::WaylandSurface;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wayland_client::protocol::wl_surface::WlSurface;
use windup_core::capture::DRect;
use windup_core::capture::backend::BarHost;
use windup_core::steps::place::steps_bar_slot;
use windup_core::video::place_bar;
use windup_ui::capture::recbar::{RecBarState, RecBarView, host_size};
use windup_ui::{Mode, Palette, apply_theme, metrics};

/// How long hiding the bar waits for the compositor's frame callback
/// before it says it is done anyway: an unmapped surface may never get one.
const CONCEAL_ACK: Duration = Duration::from_millis(100);
/// How long [`LayerConceal`] waits for the bar to be gone before the shot.
const CONCEAL_WAIT: Duration = Duration::from_millis(250);

/// A bar to show.
pub struct RecBarSetup {
    /// The recording's timer and pause state; none for Steps.
    pub clock: Option<RecClock>,
    pub fps: u32,
    /// The second chip: a bitrate, or a GIF's colours.
    pub label: String,
    /// The recorded area, in desktop (logical) units. For Steps, the
    /// pointer's monitor, which the bar keeps off when there is another.
    pub rect: DRect,
    /// Steps: the steps written so far, shown in place of the timer.
    pub steps: Option<Arc<AtomicU32>>,
    /// Steps: whether the session is paused. A recording's is its clock's.
    pub paused: Arc<AtomicBool>,
    /// Set while the pointer is on the bar's surface.
    pub pointer_on_bar: Arc<AtomicBool>,
    pub palette: Palette,
    pub mode: Mode,
    pub reduced_motion: bool,
}

pub(super) struct RecBar {
    pub(super) pane: Pane,
    canvas: Canvas,
    view: RecBarView,
    clock: Option<RecClock>,
    fps: u32,
    label: String,
    rect: DRect,
    steps: Option<Arc<AtomicU32>>,
    paused: Arc<AtomicBool>,
    pub(super) pointer_on_bar: Arc<AtomicBool>,
    reduced_motion: bool,
    /// The surface's size, logical.
    size: (u32, u32),
    /// The output the surface is on.
    output: wayland_client::protocol::wl_output::WlOutput,
    pub(super) had_keyboard: bool,
    wake_at: Option<Instant>,
    /// Unmapped for a shot (Steps).
    hidden: bool,
    /// Told once the bar is hidden: at the frame callback, or by the timer.
    conceal_ack: Option<Sender<()>>,
    /// Counts the hides, so a late timer does not answer for a later one.
    conceal_seq: u64,
}

impl RecBar {
    fn state(&self) -> RecBarState {
        bar_state(
            self.clock.as_ref(),
            self.steps.as_deref(),
            &self.paused,
            self.fps,
            &self.label,
            self.reduced_motion,
        )
    }
}

impl Drop for RecBar {
    fn drop(&mut self) {
        self.pointer_on_bar.store(false, Ordering::Relaxed);
        // A shot waiting for the bar to go need not wait any longer.
        if let Some(ack) = self.conceal_ack.take() {
            let _ = ack.send(());
        }
    }
}

/// What the bar shows: a recording's clock, or a Steps session's count.
fn bar_state(
    clock: Option<&RecClock>,
    steps: Option<&AtomicU32>,
    paused: &AtomicBool,
    fps: u32,
    label: &str,
    reduced_motion: bool,
) -> RecBarState {
    RecBarState {
        elapsed: clock.map_or(0.0, RecClock::elapsed),
        paused: match (steps, clock) {
            (None, Some(c)) => c.paused(),
            _ => paused.load(Ordering::Relaxed),
        },
        fps,
        bitrate_label: label.to_owned(),
        reduced_motion,
        steps: steps.map(|n| n.load(Ordering::Relaxed)),
    }
}

/// Hides the layer host's bar for a shot of its monitor, through the
/// Wayland thread ([`Service::conceal_recbar`]), and waits up to
/// `CONCEAL_WAIT` for it to be gone.
pub struct LayerConceal(Mutex<Service>);

impl LayerConceal {
    pub fn new(service: Service) -> Self {
        Self(Mutex::new(service))
    }

    fn ask(&self, hide: bool) -> Option<Receiver<()>> {
        self.0.lock().ok().map(|s| s.conceal_recbar(hide))
    }
}

impl Conceal for LayerConceal {
    fn hide(&self, _shot_monitor: DRect) -> bool {
        let Some(done) = self.ask(true) else {
            return false;
        };
        if done.recv_timeout(CONCEAL_WAIT).is_err() {
            log::info!("steps: the bar did not say it was hidden; shooting anyway");
        }
        true
    }

    fn show(&self) {
        // Nothing waits for the bar to come back.
        let _ = self.ask(false);
    }
}

/// The surface's size in whole logical units.
fn whole(size: egui::Vec2) -> (u32, u32) {
    (size.x.ceil().max(1.0) as u32, size.y.ceil().max(1.0) as u32)
}

impl State {
    /// Where a bar of `size` goes: its output and margins. A Steps bar
    /// (`steps`) goes off `rect`, the pointer's monitor, when there is
    /// another; a recording's beside `rect`, the recorded area.
    fn bar_spot(
        &self,
        rect: DRect,
        size: (u32, u32),
        steps: bool,
    ) -> Option<(wayland_client::protocol::wl_output::WlOutput, Spot, f64)> {
        let outputs = screencopy::outputs_of(&self.output_state);
        let layouts: Vec<DRect> = outputs.iter().map(|o| o.layout).collect();
        let bar = (size.0 as f32, size.1 as f32);
        let slot = if steps {
            steps_bar_slot(BarHost::Outside, rect, &layouts, bar, metrics::S4)
        } else {
            place_bar(rect, &layouts, bar, metrics::REC_GAP)
        }?;
        let (cx, cy) = (slot.x + slot.w / 2.0, slot.y + slot.h / 2.0);
        let o = outputs.iter().find(|o| o.layout.contains(cx, cy))?;
        let margin = (
            (slot.x - o.layout.x).round() as i32,
            (slot.y - o.layout.y).round() as i32,
        );
        let scale = self
            .output_state
            .info(&o.output)
            .map_or(1.0, |i| i.scale_factor.max(1) as f64);
        Some((o.output.clone(), Spot::At { size, margin }, scale))
    }

    pub(super) fn open_recbar(&mut self, setup: RecBarSetup) {
        self.close_recbar();
        if self.layer_shell.is_none() {
            log::info!("recording: no layer-shell; no bar");
            return;
        }
        let RecBarSetup {
            clock,
            fps,
            label,
            rect,
            steps,
            paused,
            pointer_on_bar,
            palette,
            mode,
            reduced_motion,
        } = setup;
        let mut canvas = Canvas::new(palette, mode);
        canvas.warm_up();
        let state = bar_state(
            clock.as_ref(),
            steps.as_deref(),
            &paused,
            fps,
            &label,
            reduced_motion,
        );
        let size = whole(host_size(&canvas.ctx, &state));
        let Some((output, spot, scale)) = self.bar_spot(rect, size, steps.is_some()) else {
            log::info!("recording: no room for the bar");
            return;
        };
        let id = egui::ViewportId::from_hash_of("windup-recording");
        match self.new_pane(Role::Layer, Some(&output), spot, size, scale, id) {
            Ok(pane) => {
                log::info!("recording: bar opening ({spot:?})");
                self.recbar = Some(RecBar {
                    pane,
                    canvas,
                    view: RecBarView::new(),
                    clock,
                    fps,
                    label,
                    rect,
                    steps,
                    paused,
                    pointer_on_bar,
                    reduced_motion,
                    size,
                    output: output.clone(),
                    had_keyboard: false,
                    wake_at: None,
                    hidden: false,
                    conceal_ack: None,
                    conceal_seq: 0,
                });
            }
            Err(e) => log::error!("recording: bar: {e}"),
        }
        let _ = self.conn.flush();
    }

    pub(super) fn close_recbar(&mut self) {
        if self.recbar.take().is_some() {
            log::info!("recording: bar closed");
            let _ = self.conn.flush();
        }
    }

    /// The bar's width changes once, when the timer reaches an hour: the
    /// surface is sized and placed again. A layer surface cannot change
    /// output, so a slot on another output means a new surface there.
    fn resize_recbar(&mut self, size: (u32, u32)) {
        let Some((rect, current, steps)) = self
            .recbar
            .as_ref()
            .map(|b| (b.rect, b.output.clone(), b.steps.is_some()))
        else {
            return;
        };
        match self.bar_spot(rect, size, steps) {
            Some((output, _, _)) if output != current => {
                log::info!("recording: the wider bar goes to another output");
                if let Some(b) = self.recbar.take() {
                    let setup = RecBarSetup {
                        clock: b.clock.clone(),
                        fps: b.fps,
                        label: b.label.clone(),
                        rect: b.rect,
                        steps: b.steps.clone(),
                        paused: Arc::clone(&b.paused),
                        pointer_on_bar: Arc::clone(&b.pointer_on_bar),
                        palette: b.canvas.palette,
                        mode: b.canvas.mode,
                        reduced_motion: b.reduced_motion,
                    };
                    drop(b);
                    let _ = self.conn.flush();
                    self.open_recbar(setup);
                }
            }
            Some((_, Spot::At { margin, .. }, _)) => {
                if let Some(b) = self.recbar.as_mut()
                    && let Shell::Layer(layer) = &b.pane._shell
                {
                    layer.set_margin(margin.1, 0, 0, margin.0);
                    layer.set_size(size.0, size.1);
                    layer.commit();
                    b.size = size;
                    b.pane.fallback = size;
                }
                let _ = self.conn.flush();
            }
            _ => {
                log::info!("recording: no room for the bar any more");
                self.close_recbar();
            }
        }
    }

    /// Hides the bar for a shot (`hide`), or shows it again; `ack` is told
    /// once that is done. Hiding attaches a null buffer, which unmaps the
    /// layer surface, and is done at the compositor's next frame callback
    /// or after `CONCEAL_ACK`, whichever comes first. Showing commits with
    /// no buffer, which asks for a configure; the bar is drawn once that
    /// comes, and nothing waits for it.
    pub(super) fn conceal_recbar(&mut self, hide: bool, ack: Sender<()>) {
        let Some(bar) = self.recbar.as_mut() else {
            let _ = ack.send(());
            return;
        };
        if bar.hidden == hide {
            let _ = ack.send(());
            return;
        }
        bar.hidden = hide;
        if let Some(old) = bar.conceal_ack.take() {
            let _ = old.send(());
        }
        if hide {
            let surface = bar.pane.surface.clone();
            surface.attach(None, 0, 0);
            surface.frame(&self.qh, surface.clone());
            surface.commit();
            bar.conceal_ack = Some(ack);
            bar.conceal_seq += 1;
            let seq = bar.conceal_seq;
            let _ = self.handle.insert_source(
                Timer::from_duration(CONCEAL_ACK),
                move |_, _, state: &mut State| {
                    if state.recbar.as_ref().is_some_and(|b| b.conceal_seq == seq) {
                        state.recbar_concealed(None);
                    }
                    TimeoutAction::Drop
                },
            );
        } else {
            // Back to the state right after it was made: mapped again by
            // the configure this commit asks for, then the next frame.
            bar.pane.configured = false;
            if let Shell::Layer(layer) = &bar.pane._shell {
                layer.commit();
            }
            let _ = ack.send(());
        }
        let _ = self.conn.flush();
    }

    /// The frame callback on `surface`, or `None` when the timer ran out:
    /// a shot waiting for the bar to go may go ahead.
    pub(super) fn recbar_concealed(&mut self, surface: Option<&WlSurface>) {
        let Some(bar) = self.recbar.as_mut() else {
            return;
        };
        if surface.is_some_and(|s| !bar.pane.is(s)) {
            return;
        }
        if let Some(ack) = bar.conceal_ack.take() {
            let _ = ack.send(());
        }
    }

    pub(super) fn draw_recbar(&mut self) {
        let Some(bar) = self.recbar.as_mut() else {
            return;
        };
        // Unmapped for a shot: a buffer now would map it before its
        // configure, which the protocol forbids.
        if !bar.pane.configured || bar.hidden {
            return;
        }
        if self.gpu.is_none() {
            let surface = bar.pane.wgpu.take();
            let made = surface.as_ref().map(|s| self.ensure_gpu(s));
            if let Some(b) = self.recbar.as_mut() {
                b.pane.wgpu = surface;
            }
            if let Some(Err(e)) = made {
                log::error!("recording: bar: {e}");
                self.close_recbar();
                return;
            }
        }
        let (Some(gpu), Some(bar)) = (self.gpu.as_ref(), self.recbar.as_mut()) else {
            return;
        };
        let state = bar.state();
        let RecBar {
            pane,
            canvas,
            view,
            had_keyboard,
            size,
            ..
        } = bar;
        let time = canvas.time();
        let px = pane.sync(gpu);
        let raw = pane.raw_input(time, self.pane_modifiers, *had_keyboard);
        let (palette, mode) = (canvas.palette, canvas.mode);
        let mut action = None;
        let mut wanted = *size;
        let mut output = canvas.ctx.run(raw, |ctx| {
            apply_theme(ctx, palette, mode);
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| action = view.show(ui, &palette, mode, &state));
            wanted = whole(host_size(ctx, &state));
        });
        let delay = output
            .viewport_output
            .get(&pane.id)
            .map(|v| v.repaint_delay)
            .filter(|d| *d < IDLE);
        let Some(surface) = pane.wgpu.as_ref() else {
            return;
        };
        canvas.take_pending(&mut output);
        let renderer = canvas.renderer.get_or_insert_with(|| {
            egui_wgpu::Renderer::new(&gpu.device, gpu.format, None, 1, false)
        });
        let painted = paint(
            &gpu.device,
            &gpu.queue,
            renderer,
            &canvas.ctx,
            surface,
            &mut output,
            px,
            card_clear(gpu.alpha, palette.popover),
        );
        if painted == Painted::Shown && !pane.shown {
            pane.shown = true;
            log::info!("recording: bar shown");
        }
        if painted == Painted::Reconfigure {
            pane.synced = None;
        }
        let resize = wanted != *size;
        if let Some(action) = action {
            let _ = self.shared.events.send(ServiceEvent::RecBar(action));
        }
        if resize {
            self.resize_recbar(wanted);
        }
        if painted != Painted::Shown || delay == Some(Duration::ZERO) {
            self.queue_pane_redraw();
        } else if let Some(delay) = delay {
            self.wake_recbar_in(delay);
        }
    }

    /// The pulse and the timer: one timer at a time.
    fn wake_recbar_in(&mut self, delay: Duration) {
        let at = Instant::now() + delay.max(MIN_FRAME);
        let Some(bar) = self.recbar.as_mut() else {
            return;
        };
        if bar.wake_at.is_some_and(|w| w <= at) {
            return;
        }
        bar.wake_at = Some(at);
        let _ =
            self.handle
                .insert_source(Timer::from_deadline(at), move |_, _, state: &mut State| {
                    if let Some(b) = state.recbar.as_mut()
                        && b.wake_at == Some(at)
                    {
                        b.wake_at = None;
                    }
                    state.queue_pane_redraw();
                    TimeoutAction::Drop
                });
    }
}
