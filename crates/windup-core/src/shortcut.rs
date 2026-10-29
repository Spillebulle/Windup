use serde::{Deserialize, Serialize};
use std::fmt;

/// The canonical name of the Print Screen key in `Shortcut::key`.
pub const PRINT_SCREEN: &str = "PRINTSCREEN";
/// The canonical name of the Scroll Lock key.
pub const SCROLL_LOCK: &str = "SCROLLLOCK";
/// The canonical name of the Pause key.
pub const PAUSE: &str = "PAUSE";

/// Keys the settings window cannot hear when pressed (egui has no name for
/// them), so the shortcut editor offers them as choices instead.
pub const UNHEARD_KEYS: [&str; 3] = [PRINT_SCREEN, SCROLL_LOCK, PAUSE];

/// How a canonical key name is shown: "Print Screen" for `PRINTSCREEN`.
pub fn key_label(key: &str) -> &str {
    match key {
        PRINT_SCREEN => "Print Screen",
        SCROLL_LOCK => "Scroll Lock",
        PAUSE => "Pause",
        _ => key,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct ShortcutModifiers {
    pub super_key: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl ShortcutModifiers {
    pub const NONE: Self = Self {
        super_key: false,
        ctrl: false,
        alt: false,
        shift: false,
    };

    pub const SUPER: Self = Self {
        super_key: true,
        ctrl: false,
        alt: false,
        shift: false,
    };

    pub const SUPER_SHIFT: Self = Self {
        super_key: true,
        ctrl: false,
        alt: false,
        shift: true,
    };

    pub const CTRL: Self = Self {
        super_key: false,
        ctrl: true,
        alt: false,
        shift: false,
    };

    pub const CTRL_SHIFT: Self = Self {
        super_key: false,
        ctrl: true,
        alt: false,
        shift: true,
    };

    pub fn is_empty(self) -> bool {
        !self.super_key && !self.ctrl && !self.alt && !self.shift
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(from = "RawShortcut")]
pub struct Shortcut {
    pub modifiers: ShortcutModifiers,
    pub key: String,
}

/// The stored shape. Older configs hold keys like "Space"; reading goes
/// through `Shortcut::new` so they come out in the canonical uppercase.
#[derive(Deserialize)]
struct RawShortcut {
    modifiers: ShortcutModifiers,
    key: String,
}

impl From<RawShortcut> for Shortcut {
    fn from(raw: RawShortcut) -> Self {
        Shortcut::new(raw.modifiers, raw.key)
    }
}

impl Shortcut {
    pub fn new(modifiers: ShortcutModifiers, key: impl Into<String>) -> Self {
        Self {
            modifiers,
            key: key.into().to_uppercase(),
        }
    }

    pub fn super_v() -> Self {
        Self::new(ShortcutModifiers::SUPER, "V")
    }

    pub fn super_shift_v() -> Self {
        Self::new(ShortcutModifiers::SUPER_SHIFT, "V")
    }

    pub fn ctrl_shift_s() -> Self {
        Self::new(ShortcutModifiers::CTRL_SHIFT, "S")
    }

    pub fn print_screen() -> Self {
        Self::new(ShortcutModifiers::NONE, PRINT_SCREEN)
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if self.modifiers.super_key {
            #[cfg(target_os = "macos")]
            parts.push("Cmd");
            #[cfg(target_os = "windows")]
            parts.push("Win");
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            parts.push("Super");
        }
        if self.modifiers.ctrl {
            parts.push("Ctrl");
        }
        if self.modifiers.alt {
            parts.push("Alt");
        }
        if self.modifiers.shift {
            parts.push("Shift");
        }
        if !self.key.is_empty() {
            parts.push(key_label(&self.key));
        }
        write!(f, "{}", parts.join("+"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_screen_displays_with_a_space() {
        assert_eq!(Shortcut::print_screen().to_string(), "Print Screen");
        let scroll = Shortcut::new(ShortcutModifiers::CTRL, SCROLL_LOCK);
        assert_eq!(scroll.to_string(), "Ctrl+Scroll Lock");
    }

    #[test]
    fn stored_keys_are_uppercased_when_read() {
        let json = r#"{"modifiers":{"super_key":true,"ctrl":false,"alt":false,"shift":false},"key":"Space"}"#;
        let s: Shortcut = serde_json::from_str(json).unwrap();
        assert_eq!(s.key, "SPACE");
        assert_eq!(s, Shortcut::new(ShortcutModifiers::SUPER, "SPACE"));
    }
}
