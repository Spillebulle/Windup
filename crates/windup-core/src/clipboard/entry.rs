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

    /// When the entry was copied, as a clock time in the local zone.
    pub fn time_string(&self) -> String {
        time_label(self.timestamp, &jiff::Zoned::now())
    }
}

/// `14:32` today, `27 Oct 14:32` earlier this year, `27 Oct 2025` before
/// that. Days and years are counted in `now`'s zone.
fn time_label(timestamp: u64, now: &jiff::Zoned) -> String {
    let Ok(at) = jiff::Timestamp::from_second(timestamp as i64) else {
        return String::new();
    };
    let at = at.to_zoned(now.time_zone().clone());
    let format = if at.date() == now.date() {
        "%H:%M"
    } else if at.year() == now.year() {
        "%-d %b %H:%M"
    } else {
        "%-d %b %Y"
    };
    at.strftime(format).to_string()
}

#[cfg(test)]
mod time_tests {
    use super::time_label;
    use jiff::{Zoned, civil::date, tz::TimeZone};

    fn oslo(y: i16, m: i8, d: i8, h: i8, min: i8) -> Zoned {
        date(y, m, d)
            .at(h, min, 0, 0)
            .to_zoned(TimeZone::get("Europe/Oslo").unwrap())
            .unwrap()
    }

    fn secs(z: &Zoned) -> u64 {
        z.timestamp().as_second() as u64
    }

    #[test]
    fn today_is_the_clock_time_in_the_local_zone() {
        let now = oslo(2026, 10, 28, 9, 15);
        // 00:05 here is 23:05 the day before in UTC; the local day decides.
        assert_eq!(time_label(secs(&oslo(2026, 10, 28, 0, 5)), &now), "00:05");
    }

    #[test]
    fn earlier_this_year_adds_the_day() {
        let now = oslo(2026, 10, 28, 9, 15);
        assert_eq!(
            time_label(secs(&oslo(2026, 10, 27, 23, 59)), &now),
            "27 Oct 23:59"
        );
        assert_eq!(
            time_label(secs(&oslo(2026, 3, 4, 8, 0)), &now),
            "4 Mar 08:00"
        );
    }

    #[test]
    fn another_year_drops_the_time() {
        let now = oslo(2026, 1, 2, 9, 0);
        assert_eq!(
            time_label(secs(&oslo(2025, 12, 31, 22, 0)), &now),
            "31 Dec 2025"
        );
    }
}
