use super::history::ClipboardHistory;
use clipboard_rs::common::RustImage;
use clipboard_rs::{Clipboard, ClipboardContext, ContentFormat};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub struct ClipboardMonitor {
    history: Arc<Mutex<ClipboardHistory>>,
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl ClipboardMonitor {
    pub fn new(history: Arc<Mutex<ClipboardHistory>>) -> Self {
        Self {
            history,
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn start(
        &self,
        max_items: Arc<std::sync::atomic::AtomicUsize>,
        save_images: Arc<std::sync::atomic::AtomicBool>,
    ) {
        if self.running.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return; // Already running
        }

        let history = Arc::clone(&self.history);
        let running = Arc::clone(&self.running);

        thread::spawn(move || {
            let Ok(ctx) = ClipboardContext::new() else {
                log::error!("Failed to initialize clipboard context for monitoring");
                return;
            };

            let mut last_text: Option<String> = None;
            let mut last_img_hash: u64 = 0;

            // Initial capture of existing clipboard text if any
            if ctx.has(ContentFormat::Text)
                && let Ok(text) = ctx.get_text()
            {
                let html = ctx.get_html().ok();
                let max = max_items.load(std::sync::atomic::Ordering::Relaxed);
                history.lock().unwrap().add_text(text.clone(), html, max);
                last_text = Some(text);
            }

            while running.load(std::sync::atomic::Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(250));

                let max = max_items.load(std::sync::atomic::Ordering::Relaxed);
                let capture_images = save_images.load(std::sync::atomic::Ordering::Relaxed);

                // 1. Check for Image if enabled
                if capture_images && ctx.has(ContentFormat::Image) {
                    if let Ok(img_data) = ctx.get_image() {
                        let (w, h) = img_data.get_size();
                        if w > 0
                            && h > 0
                            && let Ok(rgba_img) = img_data.to_rgba8()
                        {
                            let raw = rgba_img.into_raw();
                            let hash = calculate_simple_hash(&raw);
                            if hash != last_img_hash {
                                last_img_hash = hash;
                                history.lock().unwrap().add_image(raw, w, h, max);
                            }
                        }
                    }
                }
                // 2. Otherwise check for Text
                else if ctx.has(ContentFormat::Text)
                    && let Ok(text) = ctx.get_text()
                    && !text.is_empty()
                    && last_text.as_deref() != Some(&text)
                {
                    let html = ctx.get_html().ok();
                    last_text = Some(text.clone());
                    history.lock().unwrap().add_text(text, html, max);
                }
            }
        });
    }

    pub fn stop(&self) {
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

fn calculate_simple_hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}
