//! The clipboard history and how it is kept on disk.
//!
//! On disk the history is `clipboard_history.json` (text, sizes and the small
//! thumbnails) plus one PNG per picture in `clipboard_images/` beside it,
//! each written once. Before 0.4.2 the pictures were raw pixels inside the
//! JSON, as numbers: a screenshot of three monitors came to hundreds of MB,
//! and a few dozen made a file Windup took minutes to read. Such a file is
//! still read, and written in the new form at the next change, unless it is
//! larger than [`LEGACY_LIMIT`]: then it is set aside, never read.
//!
//! Pictures in memory are bounded too ([`IMAGE_BUDGET`]): past it the oldest
//! pictures go, text never does, and the newest picture always stays.

use super::entry::{ClipboardContent, ClipboardEntry};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The most pixel bytes (RGBA) the history keeps across its pictures.
pub const IMAGE_BUDGET: usize = 256 << 20;

/// A history file larger than this is set aside rather than read.
pub const LEGACY_LIMIT: u64 = 1 << 30;

/// The folder of the history's pictures, beside the history file.
const IMAGES_DIR: &str = "clipboard_images";

#[derive(Default, Clone, Debug)]
pub struct ClipboardHistory {
    entries: Vec<ClipboardEntry>,
}

/// The history file as written. Pictures name their file; files from before
/// 0.4.2 carry the pixels (`rgba`) instead, which is still read.
#[derive(Serialize, Deserialize)]
struct Stored {
    entries: Vec<StoredEntry>,
}

#[derive(Serialize, Deserialize)]
struct StoredEntry {
    id: u64,
    timestamp: u64,
    content: StoredContent,
}

#[derive(Serialize, Deserialize)]
enum StoredContent {
    Text {
        text: String,
        html: Option<String>,
    },
    Image {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<String>,
        #[serde(default, skip_serializing)]
        rgba: Vec<u8>,
        width: u32,
        height: u32,
        thumb_rgba: Vec<u8>,
        thumb_w: u32,
        thumb_h: u32,
    },
}

/// A picture's file name in the images folder.
fn image_file(id: u64) -> String {
    format!("{id}.png")
}

/// Writes `bytes` to `path` through a temporary file beside it, so a reader
/// (or a crash) never sees half a file.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Where an oversized history file is moved.
fn set_aside_path(path: &Path) -> PathBuf {
    path.with_extension("too-large.json")
}

impl ClipboardHistory {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn remove_by_id(&mut self, id: u64) {
        self.entries.retain(|e| e.id != id);
    }

    pub fn load_from_disk(path: &Path) -> Self {
        Self::load_within(path, LEGACY_LIMIT)
    }

    /// [`Self::load_from_disk`], setting aside a file over `limit` bytes.
    fn load_within(path: &Path, limit: u64) -> Self {
        let Ok(meta) = std::fs::metadata(path) else {
            return Self::new();
        };
        if meta.len() > limit {
            let aside = set_aside_path(path);
            log::warn!(
                "clipboard history at {} is {} bytes; moved to {} and started empty",
                path.display(),
                meta.len(),
                aside.display()
            );
            let _ = std::fs::rename(path, &aside);
            return Self::new();
        }
        let stored = match std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str::<Stored>(&s).map_err(|e| e.to_string()))
        {
            Ok(stored) => stored,
            Err(e) => {
                log::warn!(
                    "could not read clipboard history at {}: {e}",
                    path.display()
                );
                return Self::new();
            }
        };
        let images = path.with_file_name(IMAGES_DIR);
        let entries = stored
            .entries
            .into_iter()
            .filter_map(|e| restore(e, &images))
            .collect();
        let mut hist = Self { entries };
        hist.trim_images(IMAGE_BUDGET);
        let max_id = hist.entries.iter().map(|e| e.id).max().unwrap_or(0);
        NEXT_ID.store(max_id + 1, Ordering::Relaxed);
        hist
    }

    /// Writes the history file, and a PNG for each picture that has none
    /// yet; pictures no longer in the history lose their file.
    pub fn save_to_disk(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let images = path.with_file_name(IMAGES_DIR);
        let mut kept: Vec<String> = Vec::new();
        let mut entries = Vec::with_capacity(self.entries.len());
        for e in &self.entries {
            let content = match &e.content {
                ClipboardContent::Text { text, html } => StoredContent::Text {
                    text: text.clone(),
                    html: html.clone(),
                },
                ClipboardContent::Image {
                    rgba,
                    width,
                    height,
                    thumb_rgba,
                    thumb_w,
                    thumb_h,
                } => {
                    let name = image_file(e.id);
                    let file = images.join(&name);
                    if !file.exists() {
                        std::fs::create_dir_all(&images)?;
                        let mut png = Vec::new();
                        image::RgbaImage::from_raw(*width, *height, rgba.clone())
                            .ok_or("picture size does not match its pixels")?
                            .write_to(
                                &mut std::io::Cursor::new(&mut png),
                                image::ImageFormat::Png,
                            )?;
                        write_atomic(&file, &png)?;
                    }
                    kept.push(name.clone());
                    StoredContent::Image {
                        file: Some(name),
                        rgba: Vec::new(),
                        width: *width,
                        height: *height,
                        thumb_rgba: thumb_rgba.clone(),
                        thumb_w: *thumb_w,
                        thumb_h: *thumb_h,
                    }
                }
            };
            entries.push(StoredEntry {
                id: e.id,
                timestamp: e.timestamp,
                content,
            });
        }
        let json = serde_json::to_vec(&Stored { entries })?;
        write_atomic(path, &json)?;
        // Only after the history no longer names them.
        if let Ok(dir) = std::fs::read_dir(&images) {
            for f in dir.flatten() {
                let name = f.file_name().to_string_lossy().into_owned();
                if name.ends_with(".png") && !kept.contains(&name) {
                    let _ = std::fs::remove_file(f.path());
                }
            }
        }
        Ok(())
    }

    /// Drops the oldest pictures until those left hold at most `budget`
    /// pixel bytes. The newest picture stays whatever its size; text stays.
    pub fn trim_images(&mut self, budget: usize) {
        let mut used = 0usize;
        let mut first = true;
        self.entries.retain(|e| {
            let ClipboardContent::Image { rgba, .. } = &e.content else {
                return true;
            };
            used = used.saturating_add(rgba.len());
            let keep = first || used <= budget;
            first = false;
            keep
        });
    }

    pub fn entries(&self) -> &[ClipboardEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Appends a new text entry at the top of the history.
    /// Invariant: copying again should just add a new item, not reorder anything.
    pub fn add_text(&mut self, text: String, html: Option<String>, max_items: usize) {
        if text.trim().is_empty() {
            return;
        }

        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let entry = ClipboardEntry::new_text(id, text, html);

        // Always insert at front: new chronological entry without reordering past entries
        self.entries.insert(0, entry);
        if self.entries.len() > max_items {
            self.entries.truncate(max_items);
        }
    }

    /// Appends a new image entry at the top of the history with generated thumbnail.
    pub fn add_image(&mut self, rgba: Vec<u8>, width: u32, height: u32, max_items: usize) {
        if rgba.is_empty() || width == 0 || height == 0 {
            return;
        }

        // Generate thumbnail maintaining aspect ratio
        let (thumb_rgba, thumb_w, thumb_h) = generate_thumbnail(&rgba, width, height, 120, 64);

        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let entry =
            ClipboardEntry::new_image(id, rgba, width, height, thumb_rgba, thumb_w, thumb_h);

        self.entries.insert(0, entry);
        if self.entries.len() > max_items {
            self.entries.truncate(max_items);
        }
        self.trim_images(IMAGE_BUDGET);
    }

    pub fn get(&self, index: usize) -> Option<&ClipboardEntry> {
        self.entries.get(index)
    }

    pub fn get_by_id(&self, id: u64) -> Option<&ClipboardEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// An entry as read back: a picture's pixels from its file in `images` (or
/// from the entry itself, before 0.4.2). A picture whose pixels cannot be
/// had, or do not match its size, is left out.
fn restore(e: StoredEntry, images: &Path) -> Option<ClipboardEntry> {
    let content = match e.content {
        StoredContent::Text { text, html } => ClipboardContent::Text { text, html },
        StoredContent::Image {
            file,
            rgba,
            width,
            height,
            thumb_rgba,
            thumb_w,
            thumb_h,
        } => {
            let rgba = match file {
                Some(name) if rgba.is_empty() => match image::open(images.join(&name)) {
                    Ok(img) => img.into_rgba8().into_raw(),
                    Err(err) => {
                        log::warn!("clipboard picture {name}: {err}");
                        return None;
                    }
                },
                _ => rgba,
            };
            if rgba.len() != width as usize * height as usize * 4 {
                return None;
            }
            ClipboardContent::Image {
                rgba,
                width,
                height,
                thumb_rgba,
                thumb_w,
                thumb_h,
            }
        }
    };
    Some(ClipboardEntry {
        id: e.id,
        timestamp: e.timestamp,
        content,
    })
}

/// Helper to scale an RGBA buffer down to max bounding box (max_w, max_h)
fn generate_thumbnail(rgba: &[u8], w: u32, h: u32, max_w: u32, max_h: u32) -> (Vec<u8>, u32, u32) {
    if w <= max_w && h <= max_h {
        return (rgba.to_vec(), w, h);
    }

    let aspect = w as f32 / h as f32;
    let (target_w, target_h) = if aspect > (max_w as f32 / max_h as f32) {
        let tw = max_w;
        let th = ((max_w as f32 / aspect).round() as u32).max(1);
        (tw, th)
    } else {
        let th = max_h;
        let tw = ((max_h as f32 * aspect).round() as u32).max(1);
        (tw, th)
    };

    if let Some(img) = image::RgbaImage::from_raw(w, h, rgba.to_vec()) {
        let resized = image::imageops::resize(
            &img,
            target_w,
            target_h,
            image::imageops::FilterType::Triangle,
        );
        let bytes = resized.into_raw();
        (bytes, target_w, target_h)
    } else {
        (rgba.to_vec(), w, h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copying_again_adds_new_item_and_does_not_reorder() {
        let mut history = ClipboardHistory::new();
        history.add_text("First item".to_string(), None, 50);
        history.add_text("Second item".to_string(), None, 50);
        history.add_text("Third item".to_string(), None, 50);

        assert_eq!(history.len(), 3);
        assert_eq!(history.get(0).unwrap().plain_text(), Some("Third item"));
        assert_eq!(history.get(1).unwrap().plain_text(), Some("Second item"));
        assert_eq!(history.get(2).unwrap().plain_text(), Some("First item"));

        let first_item_id = history.get(2).unwrap().id;

        // Copy "First item" again!
        history.add_text("First item".to_string(), None, 50);

        // History now has 4 items
        assert_eq!(history.len(), 4);
        // The newest item at index 0 is "First item" with a new ID
        assert_eq!(history.get(0).unwrap().plain_text(), Some("First item"));
        assert_ne!(history.get(0).unwrap().id, first_item_id);

        // Previous items retain their exact order and positions!
        assert_eq!(history.get(1).unwrap().plain_text(), Some("Third item"));
        assert_eq!(history.get(2).unwrap().plain_text(), Some("Second item"));
        assert_eq!(history.get(3).unwrap().plain_text(), Some("First item"));
        assert_eq!(history.get(3).unwrap().id, first_item_id);
    }

    /// A fresh folder under the system temp folder, removed when dropped.
    struct Dir(PathBuf);
    impl Dir {
        fn new(name: &str) -> Dir {
            let p =
                std::env::temp_dir().join(format!("windup-history-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn pixels(w: u32, h: u32, seed: u8) -> Vec<u8> {
        (0..w * h * 4)
            .map(|i| (i as u8).wrapping_mul(seed))
            .collect()
    }

    fn rgba_of(e: &ClipboardEntry) -> &[u8] {
        match &e.content {
            ClipboardContent::Image { rgba, .. } => rgba,
            _ => panic!("not a picture"),
        }
    }

    #[test]
    fn pictures_are_saved_as_files_and_read_back() {
        let dir = Dir::new("roundtrip");
        let path = dir.0.join("clipboard_history.json");
        let mut h = ClipboardHistory::new();
        h.add_text("hello".into(), Some("<b>hello</b>".into()), 50);
        h.add_image(pixels(40, 30, 7), 40, 30, 50);
        h.save_to_disk(&path).unwrap();

        let json = std::fs::read_to_string(&path).unwrap();
        assert!(
            !json.contains("\"rgba\""),
            "the pixels stay out of the JSON"
        );
        let id = h.get(0).unwrap().id;
        assert!(dir.0.join(IMAGES_DIR).join(image_file(id)).is_file());

        let back = ClipboardHistory::load_from_disk(&path);
        assert_eq!(back.len(), 2);
        assert_eq!(rgba_of(back.get(0).unwrap()), pixels(40, 30, 7).as_slice());
        assert_eq!(back.get(1).unwrap().html(), Some("<b>hello</b>"));
    }

    #[test]
    fn a_picture_that_left_the_history_loses_its_file() {
        let dir = Dir::new("orphans");
        let path = dir.0.join("clipboard_history.json");
        let mut h = ClipboardHistory::new();
        h.add_image(pixels(8, 8, 3), 8, 8, 50);
        h.save_to_disk(&path).unwrap();
        let id = h.get(0).unwrap().id;
        h.remove_by_id(id);
        h.save_to_disk(&path).unwrap();
        assert!(!dir.0.join(IMAGES_DIR).join(image_file(id)).exists());
    }

    #[test]
    fn a_history_from_before_0_4_2_is_still_read() {
        let dir = Dir::new("legacy");
        let path = dir.0.join("clipboard_history.json");
        let px = pixels(2, 1, 5);
        let old = serde_json::json!({ "entries": [
            { "id": 4, "timestamp": 10, "content": { "Image": {
                "rgba": px, "width": 2, "height": 1,
                "thumb_rgba": px, "thumb_w": 2, "thumb_h": 1 } } },
            { "id": 3, "timestamp": 9, "content": { "Text": { "text": "old", "html": null } } }
        ] });
        std::fs::write(&path, old.to_string()).unwrap();
        let h = ClipboardHistory::load_from_disk(&path);
        assert_eq!(h.len(), 2);
        assert_eq!(rgba_of(h.get(0).unwrap()), px.as_slice());
        assert_eq!(h.get(1).unwrap().plain_text(), Some("old"));
    }

    #[test]
    fn a_picture_whose_file_is_gone_is_left_out() {
        let dir = Dir::new("missing");
        let path = dir.0.join("clipboard_history.json");
        let mut h = ClipboardHistory::new();
        h.add_text("kept".into(), None, 50);
        h.add_image(pixels(4, 4, 9), 4, 4, 50);
        h.save_to_disk(&path).unwrap();
        std::fs::remove_dir_all(dir.0.join(IMAGES_DIR)).unwrap();
        let back = ClipboardHistory::load_from_disk(&path);
        assert_eq!(back.len(), 1);
        assert_eq!(back.get(0).unwrap().plain_text(), Some("kept"));
    }

    #[test]
    fn an_oversized_history_is_set_aside_unread() {
        let dir = Dir::new("oversized");
        let path = dir.0.join("clipboard_history.json");
        let mut h = ClipboardHistory::new();
        h.add_text("big".into(), None, 50);
        h.save_to_disk(&path).unwrap();
        let back = ClipboardHistory::load_within(&path, 4);
        assert!(back.is_empty());
        assert!(!path.exists());
        assert!(set_aside_path(&path).is_file());
    }

    #[test]
    fn the_oldest_pictures_go_past_the_budget_and_text_stays() {
        let mut h = ClipboardHistory::new();
        h.add_image(pixels(10, 10, 1), 10, 10, 50); // 400 bytes, oldest
        h.add_text("between".into(), None, 50);
        h.add_image(pixels(10, 10, 2), 10, 10, 50);
        h.add_image(pixels(10, 10, 3), 10, 10, 50); // newest
        h.trim_images(800);
        assert_eq!(h.len(), 3);
        assert_eq!(h.get(2).unwrap().plain_text(), Some("between"));
        assert!(h.entries().iter().filter(|e| e.is_image()).count() == 2);

        // The newest picture stays even when it alone is over.
        h.trim_images(10);
        assert_eq!(h.len(), 2);
        assert_eq!(rgba_of(h.get(0).unwrap()), pixels(10, 10, 3).as_slice());
    }

    #[test]
    fn capacity_limit_truncates_oldest_items() {
        let mut history = ClipboardHistory::new();
        for i in 1..=5 {
            history.add_text(format!("Item {i}"), None, 3);
        }

        assert_eq!(history.len(), 3);
        assert_eq!(history.get(0).unwrap().plain_text(), Some("Item 5"));
        assert_eq!(history.get(1).unwrap().plain_text(), Some("Item 4"));
        assert_eq!(history.get(2).unwrap().plain_text(), Some("Item 3"));
    }
}
