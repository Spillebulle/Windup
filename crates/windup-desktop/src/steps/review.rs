//! The Steps review as a host runs it: the session folder, the review's
//! model and view, the thumbnails, the saves and the export. The window it
//! is drawn in is the host's (`review_host` on Windows and X11, a child
//! process on Wayland); this only needs an egui context.
//!
//! - Thumbnails are drawn on a worker at the window's pixel size and
//!   uploaded as textures as they come.
//! - `steps.json` is written 500 ms after the last change (a caption is
//!   one change per keystroke), before an export, and when the host drops.
//! - Export asks where with the system's dialog, then writes on the same
//!   worker. Done, the session folder goes; failed, it stays and the review
//!   says why.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use eframe::egui;
use windup_core::capture::output::default_steps_dir;
use windup_core::config::StepsConfig;
use windup_core::steps::export::{export, export_name};
use windup_core::steps::model::{Step, StepsDoc};
use windup_core::steps::options::ExportFormat;
use windup_core::steps::render::thumbnail;
use windup_ui::steps::review::{Closing, Effect, ReviewAction, ReviewModel, ReviewView};
use windup_ui::{Mode, Palette, metrics};

/// How long after the last change `steps.json` is written.
pub(crate) const SAVE_AFTER: Duration = Duration::from_millis(500);

/// When the document is due to be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SaveWhen {
    /// Nothing changed since the last save.
    Idle,
    Now,
    /// Look again in this long.
    In(Duration),
}

/// `dirty_at` is when the last unsaved change was made, so typing keeps
/// putting the save off until a pause.
pub(crate) fn save_when(dirty_at: Option<Instant>, now: Instant) -> SaveWhen {
    match dirty_at {
        None => SaveWhen::Idle,
        Some(at) => {
            let since = now.saturating_duration_since(at);
            if since >= SAVE_AFTER {
                SaveWhen::Now
            } else {
                SaveWhen::In(SAVE_AFTER - since)
            }
        }
    }
}

/// What an export's end does: (remove the session folder, close the
/// review, the error line). A failed export keeps everything, so it can be
/// tried again.
pub(crate) fn after_export(result: Result<PathBuf, String>) -> (bool, bool, Option<String>) {
    match result {
        Ok(_) => (true, true, None),
        Err(why) => (
            false,
            false,
            Some(format!("The steps could not be exported: {why}")),
        ),
    }
}

/// One thumbnail from the worker.
struct Thumb {
    generation: u64,
    index: usize,
    file: String,
    image: egui::ColorImage,
}

/// An export's end: `None` when the dialog was dismissed.
type ExportDone = Option<Result<PathBuf, String>>;

pub(crate) struct ReviewHost {
    pub dir: PathBuf,
    pub model: ReviewModel,
    /// Step `i`'s picture with its marker, once drawn.
    pub thumbs: Vec<Option<egui::TextureHandle>>,
    pub view: ReviewView,
    /// The export name's pattern and the folder the dialog opens in.
    pattern: String,
    export_dir: Option<PathBuf>,
    /// The last unsaved change.
    dirty_at: Option<Instant>,
    /// Raised whenever every thumbnail is drawn again; older ones are
    /// dropped as they arrive.
    generation: u64,
    started: bool,
    /// The review window's pixels per point, which thumbnails are drawn at.
    ppp: f32,
    thumb_tx: Sender<Thumb>,
    thumb_rx: Receiver<Thumb>,
    export: Option<Receiver<ExportDone>>,
    exported: Option<PathBuf>,
    /// The folder is gone or the review has ended: nothing more is saved.
    closed: bool,
}

impl ReviewHost {
    /// The review of the session in `dir`, with the marker and format the
    /// settings name.
    pub fn new(dir: PathBuf, cfg: &StepsConfig) -> Result<Self, String> {
        let doc = StepsDoc::load(&dir)?;
        let (thumb_tx, thumb_rx) = channel();
        Ok(Self {
            thumbs: vec![None; doc.steps.len()],
            model: ReviewModel::new(doc, cfg.marker, cfg.export),
            view: ReviewView::new(),
            dir,
            pattern: cfg.name_pattern.clone(),
            export_dir: cfg.export_dir.clone(),
            dirty_at: None,
            generation: 0,
            started: false,
            ppp: 1.0,
            thumb_tx,
            thumb_rx,
            export: None,
            exported: None,
            closed: false,
        })
    }

    /// Where the last export went, once the review has closed after it.
    pub fn exported(&self) -> Option<&Path> {
        self.exported.as_deref()
    }

    /// The index of the step whose picture is `file`, if it is still there.
    pub fn index_of(&self, file: &str) -> Option<usize> {
        self.model.doc.steps.iter().position(|s| s.file == file)
    }

    /// Draws the review over the whole of `ctx`'s window. Returns what only
    /// the host can do: `OpenEditor(i)`, or `Closed` once the review is over
    /// (discarded, exported, or emptied), its folder then gone.
    pub fn ui(&mut self, ctx: &egui::Context, p: &Palette, mode: Mode) -> Option<Effect> {
        self.ppp = ctx.pixels_per_point();
        if !self.started {
            self.started = true;
            self.draw_thumbs(ctx, None);
        }
        self.take_thumbs(ctx);
        let mut ask = self.poll_export();

        let mut actions = Vec::new();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                actions = self.view.show(ui, p, mode, &self.model, &self.thumbs);
            });
        // Enter on the confirm card is its Discard, as on any such card.
        if self.model.closing == Closing::Confirming
            && ctx.input(|i| i.key_pressed(egui::Key::Enter))
        {
            actions.push(ReviewAction::Discard);
        }
        for a in actions {
            if let ReviewAction::Remove(i) = a
                && i < self.thumbs.len()
            {
                // The others keep their pictures until redrawn.
                self.thumbs.remove(i);
            }
            if let Some(e) = self.act(ctx, a) {
                ask = ask.or(Some(e));
            }
        }

        match save_when(self.dirty_at, Instant::now()) {
            SaveWhen::Now => self.save(),
            SaveWhen::In(wait) => ctx.request_repaint_after(wait),
            SaveWhen::Idle => {}
        }
        if self.export.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        ask
    }

    /// The window's own close: asks first while steps remain.
    pub fn request_close(&mut self, ctx: &egui::Context) -> Option<Effect> {
        self.act(ctx, ReviewAction::Close)
    }

    /// Step `i`'s picture changed on disk (the editor): its thumbnail is
    /// drawn again.
    pub fn reload(&mut self, ctx: &egui::Context, i: usize) {
        if i < self.model.doc.steps.len() {
            self.draw_thumbs(ctx, Some(i));
        }
    }

    fn act(&mut self, ctx: &egui::Context, a: ReviewAction) -> Option<Effect> {
        match self.model.apply(a) {
            Effect::None => None,
            Effect::Save => {
                self.dirty_at = Some(Instant::now());
                None
            }
            Effect::Rethumb => {
                self.dirty_at = Some(Instant::now());
                self.draw_thumbs(ctx, None);
                None
            }
            Effect::OpenEditor(i) => Some(Effect::OpenEditor(i)),
            Effect::Export(format) => {
                // The export reads the document from the folder.
                self.save();
                self.start_export(format);
                None
            }
            Effect::Discard | Effect::Closed => {
                self.remove_folder();
                Some(Effect::Closed)
            }
        }
    }

    /// Writes `steps.json` now, if anything changed.
    pub fn save(&mut self) {
        if self.dirty_at.take().is_none() || self.closed {
            return;
        }
        if let Err(e) = self.model.doc.save(&self.dir) {
            log::warn!("steps: could not save {}: {e}", self.dir.display());
            self.model.error = Some(format!("The steps could not be saved ({e})."));
        }
    }

    fn remove_folder(&mut self) {
        self.closed = true;
        self.dirty_at = None;
        match std::fs::remove_dir_all(&self.dir) {
            Ok(()) => log::info!("steps: removed {}", self.dir.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("steps: could not remove {}: {e}", self.dir.display()),
        }
    }

    /// Draws every thumbnail again (`None`), or step `only`'s, on a worker.
    fn draw_thumbs(&mut self, ctx: &egui::Context, only: Option<usize>) {
        let steps = &self.model.doc.steps;
        if only.is_none() {
            self.generation += 1;
            self.thumbs.resize(steps.len(), None);
        }
        let jobs: Vec<(usize, Step)> = steps
            .iter()
            .cloned()
            .enumerate()
            .filter(|(i, _)| only.is_none_or(|o| o == *i))
            .collect();
        let max_w = (metrics::REVIEW_THUMB_W * self.ppp).round() as u32;
        let (dir, style, tx, generation, ctx) = (
            self.dir.clone(),
            self.model.style,
            self.thumb_tx.clone(),
            self.generation,
            ctx.clone(),
        );
        let spawned = std::thread::Builder::new()
            .name("steps-thumbs".into())
            .spawn(move || {
                for (index, step) in jobs {
                    let img = match image::open(dir.join(&step.file)) {
                        Ok(img) => img.to_rgba8(),
                        Err(e) => {
                            log::warn!("steps: cannot read {}: {e}", step.file);
                            continue;
                        }
                    };
                    let small = thumbnail(&img, &step, index + 1, &style, max_w);
                    let size = [small.width() as usize, small.height() as usize];
                    let image = egui::ColorImage::from_rgba_unmultiplied(size, small.as_raw());
                    let thumb = Thumb {
                        generation,
                        index,
                        file: step.file,
                        image,
                    };
                    if tx.send(thumb).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            });
        if let Err(e) = spawned {
            log::warn!("steps: no thread for the thumbnails: {e}");
        }
    }

    /// Uploads the thumbnails that are in, dropping any drawn for an older
    /// list.
    fn take_thumbs(&mut self, ctx: &egui::Context) {
        while let Ok(t) = self.thumb_rx.try_recv() {
            let current = t.generation == self.generation
                && self
                    .model
                    .doc
                    .steps
                    .get(t.index)
                    .is_some_and(|s| s.file == t.file);
            if !current {
                continue;
            }
            let texture = ctx.load_texture(
                format!("steps-thumb-{}", t.file),
                t.image,
                egui::TextureOptions::LINEAR,
            );
            if let Some(slot) = self.thumbs.get_mut(t.index) {
                *slot = Some(texture);
            }
        }
    }

    /// Asks where, then exports, on a worker.
    fn start_export(&mut self, format: ExportFormat) {
        let (tx, rx) = channel();
        let dir = self.dir.clone();
        let style = self.model.style;
        let folder = self.export_dir.clone().or_else(default_steps_dir);
        let name = export_name(&self.pattern, jiff::Zoned::now(), format);
        let spawned = std::thread::Builder::new()
            .name("steps-export".into())
            .spawn(move || {
                let done = ask_destination(format, folder.as_deref(), &name).map(|dest| {
                    log::info!("steps: exporting to {}", dest.display());
                    export(&dir, format, &style, &dest)
                });
                let _ = tx.send(done);
            });
        match spawned {
            Ok(_) => self.export = Some(rx),
            Err(e) => {
                self.model.busy = false;
                self.model.error = Some(format!("The steps could not be exported: {e}"));
            }
        }
    }

    /// The export's end, once it is in: `Closed` when it worked.
    fn poll_export(&mut self) -> Option<Effect> {
        let done = match self.export.as_ref()?.try_recv() {
            Ok(done) => done,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                Some(Err("the export stopped unexpectedly".to_string()))
            }
        };
        self.export = None;
        self.model.busy = false;
        let result = done?;
        let path = result.as_ref().ok().cloned();
        let (remove, close, error) = after_export(result);
        self.model.error = error;
        if remove {
            self.remove_folder();
        }
        if close {
            log::info!(
                "steps: exported to {}",
                path.as_deref().unwrap_or(Path::new("")).display()
            );
            self.exported = path;
            return Some(Effect::Closed);
        }
        None
    }
}

impl Drop for ReviewHost {
    /// A review that ends with the app still has its changes written.
    fn drop(&mut self) {
        self.save();
    }
}

/// Where the export goes, asked with the system's dialog: a file for HTML
/// and PDF, a folder to make the export's own folder in for Markdown and
/// Images. `None` when dismissed.
fn ask_destination(format: ExportFormat, folder: Option<&Path>, name: &str) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title("Export steps");
    if let Some(dir) = folder {
        // The default folder may not exist yet.
        let _ = std::fs::create_dir_all(dir);
        dialog = dialog.set_directory(dir);
    }
    match format {
        ExportFormat::Html => dialog
            .set_file_name(name)
            .add_filter("Web page", &["html"])
            .save_file(),
        ExportFormat::Pdf => dialog
            .set_file_name(name)
            .add_filter("PDF", &["pdf"])
            .save_file(),
        ExportFormat::Markdown | ExportFormat::Images => {
            dialog.pick_folder().map(|dir| dir.join(name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_export_that_failed_keeps_the_folder() {
        let (remove, close, error) = after_export(Err("disk full".into()));
        assert!(!remove && !close);
        assert!(error.is_some_and(|e| e.contains("disk full")));
        let (remove, close, error) = after_export(Ok(PathBuf::from("/x/Steps.html")));
        assert!(remove && close && error.is_none());
    }

    #[test]
    fn a_save_waits_for_a_pause_in_the_changes() {
        let t0 = Instant::now();
        assert_eq!(save_when(None, t0), SaveWhen::Idle);
        assert_eq!(
            save_when(Some(t0), t0 + Duration::from_millis(200)),
            SaveWhen::In(Duration::from_millis(300))
        );
        assert_eq!(save_when(Some(t0), t0 + SAVE_AFTER), SaveWhen::Now);
        // A later keystroke puts it off again.
        let t1 = t0 + Duration::from_millis(400);
        assert_eq!(
            save_when(Some(t1), t0 + SAVE_AFTER),
            SaveWhen::In(Duration::from_millis(400))
        );
    }

    fn session(name: &str, steps: usize) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("windup-review-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut doc = StepsDoc::new(&jiff::Zoned::now());
        for _ in 0..steps {
            let file = doc.next_file();
            doc.push(Step {
                file,
                cause: windup_core::steps::model::StepCause::Hotkey,
                pointer: None,
                scale: 1.0,
                monitor: String::new(),
                caption: String::new(),
                at_ms: 0,
            });
        }
        doc.save(&dir).unwrap();
        dir
    }

    #[test]
    fn a_caption_is_saved_when_the_host_goes() {
        let dir = session("caption", 1);
        let ctx = egui::Context::default();
        let mut host = ReviewHost::new(dir.clone(), &StepsConfig::default()).unwrap();
        assert_eq!(
            host.act(&ctx, ReviewAction::Caption(0, "Open the menu".into())),
            None
        );
        assert!(host.dirty_at.is_some());
        drop(host);
        let doc = StepsDoc::load(&dir).unwrap();
        assert_eq!(doc.steps[0].caption, "Open the menu");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn discarding_removes_the_folder_and_closing_with_steps_asks_first() {
        let dir = session("discard", 2);
        let ctx = egui::Context::default();
        let mut host = ReviewHost::new(dir.clone(), &StepsConfig::default()).unwrap();
        assert_eq!(host.request_close(&ctx), None);
        assert_eq!(host.model.closing, Closing::Confirming);
        assert!(dir.is_dir());
        assert_eq!(host.act(&ctx, ReviewAction::Discard), Some(Effect::Closed));
        assert!(!dir.exists());
        // Nothing is written back into a folder that is gone.
        host.dirty_at = Some(Instant::now());
        drop(host);
        assert!(!dir.exists());
    }

    #[test]
    fn closing_an_emptied_review_removes_the_folder_at_once() {
        let dir = session("empty", 0);
        let ctx = egui::Context::default();
        let mut host = ReviewHost::new(dir.clone(), &StepsConfig::default()).unwrap();
        assert_eq!(host.request_close(&ctx), Some(Effect::Closed));
        assert!(!dir.exists());
    }
}
