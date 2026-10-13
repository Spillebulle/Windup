//! The polling clipboard monitor: Windows, X11, and Wayland sessions whose
//! compositor offers no data-control protocol (through XWayland). Wayland
//! compositors that do are watched by `windup-desktop`'s listener instead.

use super::capture::{Capture, Ingest};
use super::history::ClipboardHistory;
use clipboard_rs::common::RustImage;
use clipboard_rs::{Clipboard, ClipboardContext, ContentFormat};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub struct ClipboardMonitor {
    history: Arc<Mutex<ClipboardHistory>>,
    running: Arc<AtomicBool>,
}

impl ClipboardMonitor {
    pub fn new(history: Arc<Mutex<ClipboardHistory>>) -> Self {
        Self {
            history,
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(&self, max_items: Arc<AtomicUsize>, save_images: Arc<AtomicBool>) {
        if self.running.swap(true, Ordering::SeqCst) {
            return; // Already running
        }

        let history = Arc::clone(&self.history);
        let running = Arc::clone(&self.running);

        thread::spawn(move || {
            let Ok(ctx) = ClipboardContext::new() else {
                log::error!("Failed to initialize clipboard context for monitoring");
                return;
            };

            let mut ingest = Ingest::new();

            // Initial capture of existing clipboard text if any
            if let Some(capture) = read_text(&ctx) {
                let max = max_items.load(Ordering::Relaxed);
                ingest.offer(capture, &mut history.lock().unwrap(), max, false);
            }

            while running.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(250));

                let max = max_items.load(Ordering::Relaxed);
                let capture_images = save_images.load(Ordering::Relaxed);

                match marked(&ctx) {
                    // The owner did not answer this time; a copy is only
                    // recorded once it is known not to be a password.
                    None => continue,
                    Some(true) => {
                        if let Ok(text) = ctx.get_text() {
                            ingest.mark_secret(&text);
                        }
                        continue;
                    }
                    Some(false) => {}
                }

                // Images take priority over text.
                let capture = if capture_images && ctx.has(ContentFormat::Image) {
                    read_image(&ctx)
                } else {
                    read_text(&ctx)
                };
                if let Some(capture) = capture {
                    ingest.offer(capture, &mut history.lock().unwrap(), max, capture_images);
                }
            }
        });
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

/// Whether the clipboard holds a password manager's copy, which is never
/// recorded (see `capture::SECRET_HINT`), or `None` when its owner could not
/// be asked. One request for the types, so a request that fails is not
/// taken for "unmarked". Windows password managers mark theirs differently,
/// and Windows keeps the behaviour it has always had.
#[cfg(not(windows))]
fn marked(ctx: &ClipboardContext) -> Option<bool> {
    let formats = ctx.available_formats().ok()?;
    Some(formats.iter().any(|f| f == super::capture::SECRET_HINT))
}

#[cfg(windows)]
fn marked(_: &ClipboardContext) -> Option<bool> {
    Some(false)
}

fn read_text(ctx: &ClipboardContext) -> Option<Capture> {
    if !ctx.has(ContentFormat::Text) {
        return None;
    }
    let text = ctx.get_text().ok().filter(|t| !t.is_empty())?;
    Some(Capture::Text {
        text,
        html: ctx.get_html().ok(),
    })
}

fn read_image(ctx: &ClipboardContext) -> Option<Capture> {
    let img = ctx.get_image().ok()?;
    let (width, height) = img.get_size();
    if width == 0 || height == 0 {
        return None;
    }
    Some(Capture::Image {
        rgba: img.to_rgba8().ok()?.into_raw(),
        width,
        height,
    })
}
