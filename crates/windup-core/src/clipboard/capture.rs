//! What becomes a history entry, whoever noticed the copy.
//!
//! The polling monitor (Windows, X11) and the Wayland data-control listener
//! both hand what they read to an [`Ingest`], so dedupe and the
//! password-manager rule are the same everywhere.

use super::history::ClipboardHistory;
use std::collections::VecDeque;

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
    /// Hashes of texts a password manager marked secret, newest last. In
    /// memory only. A clipboard manager (Mutter's, Weston's, Klipper) keeps
    /// the text when the password manager quits and offers it again as its
    /// own, without the mark; this is how that copy is still recognised.
    secrets: VecDeque<u64>,
}

/// How many secrets are remembered.
const SECRETS_KEPT: usize = 32;

impl Ingest {
    pub fn new() -> Self {
        Self::default()
    }

    /// Notes that `text` is a password, so it is never recorded, whoever
    /// offers it later.
    pub fn mark_secret(&mut self, text: &str) {
        let hash = hash_bytes(text.as_bytes());
        if !self.secrets.contains(&hash) {
            if self.secrets.len() == SECRETS_KEPT {
                self.secrets.pop_front();
            }
            self.secrets.push_back(hash);
        }
    }

    /// Records `capture` unless it repeats the last one or is a known
    /// secret. Returns whether an entry was added.
    pub fn offer(
        &mut self,
        capture: Capture,
        history: &mut ClipboardHistory,
        max_items: usize,
        save_images: bool,
    ) -> bool {
        match capture {
            Capture::Text { text, html } => {
                if text.trim().is_empty()
                    || self.last_text.as_deref() == Some(text.as_str())
                    || self.secrets.contains(&hash_bytes(text.as_bytes()))
                {
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
    /// A password manager's copy: read only to remember it
    /// ([`Ingest::mark_secret`]), never to record it.
    pub secret: bool,
}

const TEXT_TYPES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "STRING",
    "TEXT",
];

/// Chooses what to read from the MIME types a copy offers, or nothing when
/// it has no type Windup records. A password comes back as text marked
/// `secret`.
pub fn pick(offered: &[String], images: bool) -> Option<Pick> {
    let secret = offered.iter().any(|m| m == SECRET_HINT);
    let has = |t: &str| offered.iter().any(|m| m.eq_ignore_ascii_case(t));

    if images && !secret && has("image/png") {
        return Some(Pick {
            mime: "image/png".into(),
            is_image: true,
            html: None,
            secret: false,
        });
    }
    let text = TEXT_TYPES.iter().find(|t| has(t))?;
    Some(Pick {
        mime: (*text).to_owned(),
        is_image: false,
        html: (!secret && has("text/html")).then(|| "text/html".to_owned()),
        secret,
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
    fn passwords_are_picked_as_secret_text() {
        let offer = v(&[
            "image/png",
            "text/html",
            "text/plain;charset=utf-8",
            SECRET_HINT,
        ]);
        let p = pick(&offer, true).unwrap();
        assert!(p.secret);
        assert!(!p.is_image);
        assert_eq!(p.mime, "text/plain;charset=utf-8");
        assert_eq!(p.html, None);
        assert!(!pick(&v(&["UTF8_STRING"]), true).unwrap().secret);
    }

    #[test]
    fn a_secret_is_not_recorded_when_offered_again_unmarked() {
        let mut h = ClipboardHistory::new();
        let mut i = Ingest::new();
        i.mark_secret("hunter2");
        // A clipboard manager taking over when the password manager quits.
        assert!(!i.offer(text("hunter2"), &mut h, 10, true));
        assert!(i.offer(text("something else"), &mut h, 10, true));
        assert!(!i.offer(text("hunter2"), &mut h, 10, true));
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn only_the_latest_secrets_are_kept() {
        let mut i = Ingest::new();
        for n in 0..SECRETS_KEPT + 5 {
            i.mark_secret(&format!("s{n}"));
        }
        i.mark_secret("s40");
        assert_eq!(i.secrets.len(), SECRETS_KEPT);
        let mut h = ClipboardHistory::new();
        assert!(i.offer(text("s0"), &mut h, 10, true));
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
