use super::entry::ClipboardEntry;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct ClipboardHistory {
    entries: Vec<ClipboardEntry>,
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

    pub fn load_from_disk(path: &std::path::Path) -> Self {
        if !path.exists() {
            return Self::new();
        }
        match std::fs::read_to_string(path) {
            Ok(content) => match serde_json::from_str::<ClipboardHistory>(&content) {
                Ok(hist) => {
                    let max_id = hist.entries.iter().map(|e| e.id).max().unwrap_or(0);
                    NEXT_ID.store(max_id + 1, Ordering::Relaxed);
                    hist
                }
                Err(e) => {
                    log::warn!("Failed to parse clipboard history at {}: {}", path.display(), e);
                    Self::new()
                }
            },
            Err(e) => {
                log::warn!("Failed to read clipboard history at {}: {}", path.display(), e);
                Self::new()
            }
        }
    }

    pub fn save_to_disk(&self, path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string(&self)?;
        std::fs::write(path, json)?;
        Ok(())
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
