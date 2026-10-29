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

use super::gpu::{Painted, card_clear, paint};
use super::overlay::{IDLE, MIN_FRAME, Role};
use super::pane::{Canvas, Pane, Shell, Spot};
use super::{ServiceEvent, State, screencopy};
use crate::video::run::RecClock;
use calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::shell::WaylandSurface;
use std::time::{Duration, Instant};
use windup_core::capture::DRect;
use windup_core::video::place_bar;
use windup_ui::capture::recbar::{RecBarState, RecBarView, host_size};
use windup_ui::{Mode, Palette, apply_theme, metrics};

/// A bar to show.
pub struct RecBarSetup {
    pub clock: RecClock,
    pub fps: u32,
    /// The second chip: a bitrate, or a GIF's colours.
    pub label: String,
    /// The recorded area, in desktop (logical) units.
    pub rect: DRect,
    pub palette: Palette,
    pub mode: Mode,
    pub reduced_motion: bool,
}

pub(super) struct RecBar {
    pub(super) pane: Pane,
    canvas: Canvas,
    view: RecBarView,
    clock: RecClock,
    fps: u32,
    label: String,
    rect: DRect,
    reduced_motion: bool,
    /// The surface's size, logical.
    size: (u32, u32),
    /// The output the surface is on.
    output: wayland_client::protocol::wl_output::WlOutput,
    pub(super) had_keyboard: bool,
    wake_at: Option<Instant>,
}

impl RecBar {
    fn state(&self) -> RecBarState {
        RecBarState {
            elapsed: self.clock.elapsed(),
            paused: self.clock.paused(),
            fps: self.fps,
            bitrate_label: self.label.clone(),
            reduced_motion: self.reduced_motion,
        }
    }
}

/// The surface's size in whole logical units.
fn whole(size: egui::Vec2) -> (u32, u32) {
    (size.x.ceil().max(1.0) as u32, size.y.ceil().max(1.0) as u32)
}

impl State {
    /// Where a bar of `size` goes: its output and margins.
    fn bar_spot(
        &self,
        rect: DRect,
        size: (u32, u32),
    ) -> Option<(wayland_client::protocol::wl_output::WlOutput, Spot, f64)> {
        let outputs = screencopy::outputs_of(&self.output_state);
        let layouts: Vec<DRect> = outputs.iter().map(|o| o.layout).collect();
        let slot = place_bar(
            rect,
            &layouts,
            (size.0 as f32, size.1 as f32),
            metrics::REC_GAP,
        )?;
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
            palette,
            mode,
            reduced_motion,
        } = setup;
        let canvas = Canvas::new(palette, mode);
        // Fonts are ready after one pass.
        let _ = canvas.ctx.run(egui::RawInput::default(), |_| {});
        let bar_state = RecBarState {
            elapsed: clock.elapsed(),
            paused: clock.paused(),
            fps,
            bitrate_label: label.clone(),
            reduced_motion,
        };
        let size = whole(host_size(&canvas.ctx, &bar_state));
        let Some((output, spot, scale)) = self.bar_spot(rect, size) else {
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
                    reduced_motion,
                    size,
                    output: output.clone(),
                    had_keyboard: false,
                    wake_at: None,
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
        let Some((rect, current)) = self.recbar.as_ref().map(|b| (b.rect, b.output.clone())) else {
            return;
        };
        match self.bar_spot(rect, size) {
            Some((output, _, _)) if output != current => {
                log::info!("recording: the wider bar goes to another output");
                if let Some(b) = self.recbar.take() {
                    let setup = RecBarSetup {
                        clock: b.clock,
                        fps: b.fps,
                        label: b.label,
                        rect: b.rect,
                        palette: b.canvas.palette,
                        mode: b.canvas.mode,
                        reduced_motion: b.reduced_motion,
                    };
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

    pub(super) fn draw_recbar(&mut self) {
        let Some(bar) = self.recbar.as_mut() else {
            return;
        };
        if !bar.pane.configured {
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
        let renderer = canvas.renderer.get_or_insert_with(|| {
            egui_wgpu::Renderer::new(&gpu.device, gpu.format, None, 1, false)
        });
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
