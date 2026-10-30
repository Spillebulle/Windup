//! The Steps review on the viewport host (Windows and X11): an ordinary
//! window of the app, an immediate viewport titled "Windup steps", drawn
//! from `update` like the capture overlays. What it shows and does is
//! `steps::review::ReviewHost`; this opens and closes it, opens the editor
//! on a step, and says where an export went.

use crate::WindupApp;
use crate::steps::review::ReviewHost;
use eframe::egui;
use std::path::{Path, PathBuf};
use windup_ui::steps::review::Effect;
use windup_ui::{Mode, Palette, apply_theme, metrics};

/// The review's window title.
pub(crate) const REVIEW_TITLE: &str = "Windup steps";
/// The notice when Steps is asked for while a review is open.
pub(crate) const FINISH_REVIEW_FIRST: &str = "Finish the steps you are reviewing first.";

pub(crate) struct ReviewViewport {
    host: ReviewHost,
}

impl ReviewViewport {
    fn id() -> egui::ViewportId {
        egui::ViewportId::from_hash_of("steps_review")
    }
}

/// The notice after an export: the name of what was written.
pub(crate) fn exported_text(path: &Path) -> String {
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    format!("Exported to {name}")
}

impl WindupApp {
    /// Opens the review of the session in `dir`.
    pub(crate) fn open_review(&mut self, ctx: &egui::Context, dir: PathBuf) {
        if self.review.is_some() {
            // Starting is refused while one is open, so this is a session
            // that ended as the review opened: it is kept for the next start.
            log::warn!("steps: a review is open already; {} is left", dir.display());
            return;
        }
        match ReviewHost::new(dir, &self.config.steps) {
            Ok(host) => {
                log::info!("steps: review opened");
                self.review = Some(ReviewViewport { host });
                ctx.request_repaint();
            }
            Err(why) => {
                let text = format!("The steps could not be opened: {why}");
                self.open_notice(ctx, &text, None);
            }
        }
    }

    /// Draws the review while one is open. Every frame, from `update`.
    pub(crate) fn drive_review(&mut self, ctx: &egui::Context, p: &Palette, mode: Mode) {
        let Some(r) = self.review.as_mut() else {
            return;
        };
        let builder = egui::ViewportBuilder::default()
            .with_title(REVIEW_TITLE)
            .with_app_id(crate::APP_ID)
            .with_icon(self.app_icon.clone())
            .with_inner_size([metrics::REVIEW_W, metrics::REVIEW_H])
            .with_min_inner_size([metrics::REVIEW_W / 2.0, metrics::REVIEW_H / 2.0]);
        let mut ask = None;
        ctx.show_viewport_immediate(ReviewViewport::id(), builder, |vctx, class| {
            if class != egui::ViewportClass::Immediate {
                return;
            }
            apply_theme(vctx, *p, mode);
            let close = vctx.input(|i| i.viewport().close_requested());
            if close {
                // Steps that were not exported are asked about first.
                vctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
            ask = r.host.ui(vctx, p, mode);
            if close && ask.is_none() {
                ask = r.host.request_close(vctx);
            }
        });
        match ask {
            Some(Effect::OpenEditor(i)) => {
                let Some(step) = r.host.model.doc.steps.get(i) else {
                    return;
                };
                let path = r.host.dir.join(&step.file);
                let monitor = step.monitor.clone();
                self.edit_step(ctx, path, &monitor);
            }
            Some(Effect::Closed) => self.close_review(ctx),
            _ => {}
        }
    }

    /// The review is over: its choices are kept for the next one, and an
    /// export is announced. Not drawn again, the window goes.
    fn close_review(&mut self, ctx: &egui::Context) {
        let Some(r) = self.review.take() else {
            return;
        };
        log::info!("steps: review closed");
        self.config.steps.marker.kind = r.host.model.style.kind;
        self.config.steps.export = r.host.model.format;
        self.config.save();
        if let Some(path) = r.host.exported() {
            self.open_notice(ctx, &exported_text(path), Some(path.to_path_buf()));
        }
    }

    /// The window was asked to close while a review is open: the review is
    /// asked to close, which asks about steps not exported.
    pub(crate) fn ask_review_to_close(&mut self, ctx: &egui::Context) {
        let closed = self
            .review
            .as_mut()
            .and_then(|r| r.host.request_close(ctx))
            .is_some();
        if closed {
            self.close_review(ctx);
        } else {
            ctx.send_viewport_cmd_to(ReviewViewport::id(), egui::ViewportCommand::Focus);
        }
    }

    /// A step's picture was edited: the review draws its thumbnail again.
    pub(crate) fn reload_step(&mut self, ctx: &egui::Context, path: &Path) {
        let Some(r) = self.review.as_mut() else {
            return;
        };
        if path.parent() != Some(r.host.dir.as_path()) {
            return;
        }
        if let Some(i) = path
            .file_name()
            .and_then(|f| f.to_str())
            .and_then(|f| r.host.index_of(f))
        {
            r.host.reload(ctx, i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_export_notice_names_what_was_written() {
        assert_eq!(
            exported_text(Path::new(
                "/home/a/Documents/Windup/Steps 2026-10-30 142501.html"
            )),
            "Exported to Steps 2026-10-30 142501.html"
        );
        assert_eq!(
            exported_text(Path::new("/d/Steps 2026-10-30 142501")),
            "Exported to Steps 2026-10-30 142501"
        );
    }

    #[test]
    fn notices_are_sentences_without_dashes() {
        for s in [FINISH_REVIEW_FIRST, REVIEW_TITLE] {
            assert!(!s.contains('\u{2014}') && !s.contains(" - "), "{s}");
            assert!(s.chars().next().is_some_and(char::is_uppercase), "{s}");
        }
    }
}
