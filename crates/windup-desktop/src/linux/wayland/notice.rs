//! The notice card on the layer host: why a capture could not start or
//! save, or where a recording went (with "Open folder"), with OK; Enter and
//! Esc close it, and so does the keyboard going
//! elsewhere, as it does the popup. A layer surface of the card's size that
//! the compositor centres. The window host shows it as a window of its own
//! instead (`linux::window::notice`).

use super::State;
use super::gpu::{Painted, card_clear, paint};
use super::overlay::{IDLE, MIN_FRAME, Role};
use super::pane::{Canvas, Pane, Spot};
use calloop::timer::{TimeoutAction, Timer};
use std::path::PathBuf;
use std::time::Duration;
use windup_ui::{Mode, Palette, apply_theme, notice};

/// A notice to show.
pub struct NoticeSetup {
    pub message: String,
    /// A saved recording, which "Open folder" shows.
    pub action: Option<PathBuf>,
    pub palette: Palette,
    pub mode: Mode,
}

pub(super) struct Notice {
    pub(super) pane: Pane,
    pub(super) canvas: Canvas,
    pub(super) message: String,
    pub(super) action: Option<PathBuf>,
    pub(super) had_keyboard: bool,
}

impl State {
    pub(super) fn open_notice(&mut self, setup: NoticeSetup) {
        let NoticeSetup {
            message,
            action,
            palette,
            mode,
        } = setup;
        self.close_popup();
        self.close_notice();
        if self.layer_shell.is_none() {
            log::warn!("notice: no layer-shell; not shown: {message}");
            return;
        }
        let mut canvas = Canvas::new(palette, mode);
        canvas.warm_up();
        let card = notice::size(&canvas.ctx, &message, &canvas.palette);
        let size = (card.x.ceil().max(1.0) as u32, card.y.ceil().max(1.0) as u32);
        let id = egui::ViewportId::from_hash_of("windup-notice");
        match self.new_pane(Role::Layer, None, Spot::Card(size), size, 1.0, id) {
            Ok(pane) => {
                log::info!("notice: opening");
                self.notice = Some(Notice {
                    pane,
                    canvas,
                    message,
                    action,
                    had_keyboard: false,
                });
            }
            Err(e) => log::error!("notice: {e}"),
        }
        let _ = self.conn.flush();
    }

    pub(super) fn close_notice(&mut self) {
        if self.notice.take().is_some() {
            log::info!("notice: closed");
            let _ = self.conn.flush();
        }
    }

    pub(super) fn draw_notice(&mut self) {
        let Some(card) = self.notice.as_mut() else {
            return;
        };
        if !card.pane.configured {
            return;
        }
        if self.gpu.is_none() {
            let surface = card.pane.wgpu.take();
            let made = surface.as_ref().map(|s| self.ensure_gpu(s));
            if let Some(n) = self.notice.as_mut() {
                n.pane.wgpu = surface;
            }
            if let Some(Err(e)) = made {
                log::error!("notice: {e}");
                self.close_notice();
                return;
            }
        }
        let (Some(gpu), Some(card)) = (self.gpu.as_ref(), self.notice.as_mut()) else {
            return;
        };
        let Notice {
            pane,
            canvas,
            message,
            action,
            had_keyboard,
        } = card;
        let label = action.as_ref().map(|_| crate::video::outcome::OPEN_FOLDER);
        let time = canvas.time();
        let px = pane.sync(gpu);
        let raw = pane.raw_input(time, self.pane_modifiers, *had_keyboard);
        let (palette, mode) = (canvas.palette, canvas.mode);
        let mut close = false;
        let mut act = false;
        let mut output = canvas.ctx.run(raw, |ctx| {
            apply_theme(ctx, palette, mode);
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(
                ctx,
                |ui| match notice::show(ui, &palette, message, label) {
                    Some(notice::NoticeReply::Action) => act = true,
                    Some(notice::NoticeReply::Dismissed) => close = true,
                    None => {}
                },
            );
        });
        if act && let Some(file) = action.as_deref() {
            crate::video::outcome::open_folder(file);
            close = true;
        }
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
            card_clear(gpu.alpha, palette.dock),
        );
        if painted == Painted::Shown && !pane.shown {
            pane.shown = true;
            log::info!("notice: shown");
        }
        if painted == Painted::Reconfigure {
            pane.synced = None;
        }
        if close {
            self.close_notice();
        } else if painted != Painted::Shown || delay == Some(Duration::ZERO) {
            self.queue_pane_redraw();
        } else if let Some(delay) = delay {
            // A hover fading in or out; it ends by itself.
            let _ = self.handle.insert_source(
                Timer::from_duration(delay.max(MIN_FRAME)),
                |_, _, state: &mut State| {
                    state.queue_pane_redraw();
                    TimeoutAction::Drop
                },
            );
        }
    }
}
