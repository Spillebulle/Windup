//! What becomes a history entry, whoever noticed the copy.
//!
//! The polling monitor (Windows, X11) and the Wayland data-control listener
//! both hand what they read to an [`Ingest`], so dedupe and the
//! password-manager rule are the same everywhere.

use super::history::ClipboardHistory;

/// One read of the clipboard.
#[derive(Clone, Debug, PartialEq)]
pub enum Capture {
    Text {
        text: String,
        html: Option<String>,
    },
    Image {
        rgba: Vec<u8>,
        width: u32,
        height: u32,
    },
}

/// Remembers the last thing recorded so that the same clipboard read twice is
/// one entry. Copying the same text again *after something else* is a new
/// entry at the front; history is never reordered.
#[derive(Default, Debug)]
pub struct Ingest {
    last_text: Option<String>,
    last_image: u64,
}

impl Ingest {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `capture` unless it repeats the last one. Returns whether an
    /// entry was added.
    pub fn offer(
        &mut self,
        capture: Capture,
        history: &mut ClipboardHistory,
        max_items: usize,
        save_images: bool,
    ) -> bool {
        match capture {
            Capture::Text { text, html } => {
                if text.trim().is_empty() || self.last_text.as_deref() == Some(text.as_str()) {
                    return false;
                }
                self.last_text = Some(text.clone());
                history.add_text(text, html, max_items);
                true
            }
            Capture::Image {
                rgba,
                width,
                height,
            } => {
                if !save_images || rgba.is_empty() || width == 0 || height == 0 {
                    return false;
                }
                let hash = hash_bytes(&rgba);
                if hash == self.last_image {
                    return false;
                }
                self.last_image = hash;
                history.add_image(rgba, width, height, max_items);
                true
            }
        }
    }
}

pub fn hash_bytes(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Set by KeePassXC, Bitwarden, 1Password, KWallet and others on a copied
/// password, so clipboard managers leave it out. Its presence is enough.
pub const SECRET_HINT: &str = "x-kde-passwordManagerHint";

/// What to read from an offer, in order of preference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    /// The MIME type to read for the entry itself.
    pub mime: String,
    pub is_image: bool,
    /// `text/html`, read alongside text when offered.
    pub html: Option<String>,
}

const TEXT_TYPES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "STRING",
    "TEXT",
];

/// Chooses what to read from the MIME types a copy offers, or nothing when
/// the offer is a password or has no type Windup records.
pub fn pick(offered: &[String], images: bool) -> Option<Pick> {
    if offered.iter().any(|m| m == SECRET_HINT) {
        return None;
    }
    let has = |t: &str| offered.iter().any(|m| m.eq_ignore_ascii_case(t));

    if images && has("image/png") {
        return Some(Pick {
            mime: "image/png".into(),
            is_image: true,
            html: None,
        });
    }
    let text = TEXT_TYPES.iter().find(|t| has(t))?;
    Some(Pick {
        mime: (*text).to_owned(),
        is_image: false,
        html: has("text/html").then(|| "text/html".to_owned()),
    })
}

/// The MIME types Windup offers when it owns the clipboard with text.
pub const OFFER_TEXT: [&str; 4] = [
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
    "STRING",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn text(s: &str) -> Capture {
        Capture::Text {
            text: s.into(),
            html: None,
        }
    }

    #[test]
    fn same_read_twice_is_one_entry() {
        let mut h = ClipboardHistory::new();
        let mut i = Ingest::new();
        assert!(i.offer(text("a"), &mut h, 10, true));
        assert!(!i.offer(text("a"), &mut h, 10, true));
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn copying_again_after_something_else_is_a_new_entry_in_front() {
        let mut h = ClipboardHistory::new();
        let mut i = Ingest::new();
        i.offer(text("a"), &mut h, 10, true);
        i.offer(text("b"), &mut h, 10, true);
        i.offer(text("a"), &mut h, 10, true);
        let order: Vec<_> = h.entries().iter().filter_map(|e| e.plain_text()).collect();
        assert_eq!(order, ["a", "b", "a"]);
    }

    #[test]
    fn blank_text_is_ignored() {
        let mut h = ClipboardHistory::new();
        assert!(!Ingest::new().offer(text("  \n"), &mut h, 10, true));
        assert!(h.is_empty());
    }

    #[test]
    fn images_dedupe_by_content_and_obey_the_setting() {
        let mut h = ClipboardHistory::new();
        let mut i = Ingest::new();
        let img = || Capture::Image {
            rgba: vec![1, 2, 3, 4],
            width: 1,
            height: 1,
        };
        assert!(!i.offer(img(), &mut h, 10, false));
        assert!(i.offer(img(), &mut h, 10, true));
        assert!(!i.offer(img(), &mut h, 10, true));
        assert_eq!(h.len(), 1);
        assert!(h.get(0).unwrap().is_image());
    }

    #[test]
    fn passwords_are_never_picked() {
        let offer = v(&["text/plain;charset=utf-8", SECRET_HINT]);
        assert_eq!(pick(&offer, true), None);
    }

    #[test]
    fn prefers_png_then_utf8_text() {
        let offer = v(&[
            "TEXT",
            "text/plain",
            "image/png",
            "text/plain;charset=utf-8",
        ]);
        assert_eq!(pick(&offer, true).unwrap().mime, "image/png");
        let p = pick(&offer, false).unwrap();
        assert_eq!(p.mime, "text/plain;charset=utf-8");
        assert!(!p.is_image);
    }

    #[test]
    fn reads_html_beside_text() {
        let p = pick(&v(&["text/html", "UTF8_STRING"]), true).unwrap();
        assert_eq!(p.mime, "UTF8_STRING");
        assert_eq!(p.html.as_deref(), Some("text/html"));
    }

    #[test]
    fn nothing_recordable_is_none() {
        assert_eq!(pick(&v(&["application/x-color"]), true), None);
        assert_eq!(pick(&v(&["image/png"]), false), None);
        assert_eq!(pick(&[], true), None);
    }
}
