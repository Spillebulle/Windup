//! The Steps review on Wayland: a process of its own,
//! `windup --steps-review <dir>`, which the background process starts when
//! a session stops with steps taken. Wayland cannot hide a window, so the
//! background process keeps none; the review is a window for as long as it
//! is open, like the settings.
//!
//! What it shows and does is `steps::review::ReviewHost`, as on Windows and
//! X11. The editor is the background process's overlay: Edit asks for it
//! over the socket (`Request::EditStep`), and the picture's file is watched
//! here, so its thumbnail is drawn again once the editor writes it back.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui;
use windup_core::Config;
use windup_core::ipc::Request;
use windup_ui::steps::review::Effect;
use windup_ui::{Mode, Palette, apply_theme, install_fonts, metrics};

use super::review::ReviewHost;
use crate::linux::ipc::send_request;
use crate::review_host::{REVIEW_TITLE, exported_text};

/// How often an edited step's file is looked at.
const WATCH_EVERY: Duration = Duration::from_millis(500);

/// Shows the review of the session in `dir` until it is over: exported,
/// discarded or emptied. An export is announced afterwards, as on X11.
pub fn run(dir: PathBuf) -> eframe::Result<()> {
    let config = Config::load();
    let theme = crate::linux::popup_setup(&config);
    let (palette, mode) = (theme.palette, theme.mode);
    let host = match ReviewHost::new(dir, &config.steps) {
        Ok(host) => host,
        Err(why) => {
            log::error!("steps: the review cannot open: {why}");
            eprintln!("windup: the steps could not be opened: {why}");
            return Ok(());
        }
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(REVIEW_TITLE)
            .with_app_id(crate::APP_ID)
            .with_icon(windup_ui::art::window_icon(&palette))
            .with_inner_size([metrics::REVIEW_W, metrics::REVIEW_H])
            .with_min_inner_size([metrics::REVIEW_W / 2.0, metrics::REVIEW_H / 2.0]),
        run_and_return: true,
        ..Default::default()
    };
    let mut exported = None;
    eframe::run_native(
        "Windup",
        options,
        Box::new(|cc| {
            install_fonts(&cc.egui_ctx);
            Ok(Box::new(ReviewApp {
                host,
                palette,
                mode,
                watched: Vec::new(),
                looked: Instant::now(),
                done: false,
                exported: &mut exported,
            }))
        }),
    )?;
    if let Some(path) = exported {
        let (_tx, none) = std::sync::mpsc::channel();
        crate::linux::window::notice(&exported_text(&path), Some(&path), palette, mode, &none);
    }
    Ok(())
}

struct ReviewApp<'a> {
    host: ReviewHost,
    palette: Palette,
    mode: Mode,
    /// The steps sent to the editor, and their files' times when last seen.
    watched: Vec<(String, Option<SystemTime>)>,
    looked: Instant,
    /// The review is over; the window may close.
    done: bool,
    exported: &'a mut Option<PathBuf>,
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl ReviewApp<'_> {
    /// Asks the background process for the editor on step `i`.
    fn edit(&mut self, i: usize) {
        let Some(step) = self.host.model.doc.steps.get(i) else {
            return;
        };
        let path = self.host.dir.join(&step.file);
        if !self.watched.iter().any(|(f, _)| *f == step.file) {
            self.watched.push((step.file.clone(), modified(&path)));
        }
        if let Err(e) = send_request(&Request::EditStep(path)) {
            log::warn!("steps: the editor was not asked for: {e}");
            self.host.model.error = Some(format!("The editor could not be opened ({e})."));
        }
    }

    /// Draws again the thumbnail of each edited step whose file changed.
    fn look_at_edits(&mut self, ctx: &egui::Context) {
        if self.looked.elapsed() < WATCH_EVERY {
            return;
        }
        self.looked = Instant::now();
        for (file, seen) in &mut self.watched {
            let now = modified(&self.host.dir.join(&*file));
            if now == *seen {
                continue;
            }
            *seen = now;
            if let Some(i) = self.host.index_of(file) {
                log::info!("steps: {file} changed; its thumbnail is drawn again");
                self.host.reload(ctx, i);
            }
        }
    }

    /// The review is over: its choices are kept for the next one.
    fn finish(&mut self, ctx: &egui::Context) {
        self.done = true;
        let mut config = Config::load();
        config.steps.marker.kind = self.host.model.style.kind;
        config.steps.export = self.host.model.format;
        config.save();
        *self.exported = self.host.exported().map(Path::to_path_buf);
        log::info!("steps: review closed");
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

impl eframe::App for ReviewApp<'_> {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if self.done {
            return;
        }
        apply_theme(ctx, self.palette, self.mode);
        let close = ctx.input(|i| i.viewport().close_requested());
        if close {
            // Steps that were not exported are asked about first.
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        let mut ask = self.host.ui(ctx, &self.palette, self.mode);
        if close && ask.is_none() {
            ask = self.host.request_close(ctx);
        }
        match ask {
            Some(Effect::OpenEditor(i)) => self.edit(i),
            Some(Effect::Closed) => {
                self.finish(ctx);
                return;
            }
            _ => {}
        }
        self.look_at_edits(ctx);
        ctx.request_repaint_after(WATCH_EVERY);
    }
}
