use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClipboardContent {
    Text {
        text: String,
        html: Option<String>,
    },
    Image {
        rgba: Vec<u8>,
        width: u32,
        height: u32,
        thumb_rgba: Vec<u8>,
        thumb_w: u32,
        thumb_h: u32,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClipboardEntry {
    pub id: u64,
    pub timestamp: u64, // UNIX timestamp in seconds
    pub content: ClipboardContent,
}

impl ClipboardEntry {
    pub fn new_text(id: u64, text: String, html: Option<String>) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            id,
            timestamp,
            content: ClipboardContent::Text { text, html },
        }
    }

    pub fn new_image(
        id: u64,
        rgba: Vec<u8>,
        width: u32,
        height: u32,
        thumb_rgba: Vec<u8>,
        thumb_w: u32,
        thumb_h: u32,
    ) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            id,
            timestamp,
            content: ClipboardContent::Image {
                rgba,
                width,
                height,
                thumb_rgba,
                thumb_w,
                thumb_h,
            },
        }
    }

    pub fn is_image(&self) -> bool {
        matches!(self.content, ClipboardContent::Image { .. })
    }

    pub fn plain_text(&self) -> Option<&str> {
        match &self.content {
            ClipboardContent::Text { text, .. } => Some(text),
            _ => None,
        }
    }

    pub fn html(&self) -> Option<&str> {
        match &self.content {
            ClipboardContent::Text { html, .. } => html.as_deref(),
            _ => None,
        }
    }

    pub fn preview_label(&self) -> String {
        match &self.content {
            ClipboardContent::Text { text, .. } => {
                let trimmed = text.trim();
                let first_line = trimmed.lines().next().unwrap_or("");
                if first_line.chars().count() > 60 {
                    let s: String = first_line.chars().take(57).collect();
                    format!("{s}...")
                } else {
                    first_line.to_string()
                }
            }
            ClipboardContent::Image { width, height, .. } => {
                format!("Image ({} × {} px)", width, height)
            }
        }
    }

    pub fn relative_time_string(&self) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let elapsed = now.saturating_sub(self.timestamp);
        if elapsed < 60 {
            "just now".to_string()
        } else if elapsed < 3600 {
            format!("{}m ago", elapsed / 60)
        } else if elapsed < 86400 {
            format!("{}h ago", elapsed / 3600)
        } else {
            format!("{}d ago", elapsed / 86400)
        }
    }
}
