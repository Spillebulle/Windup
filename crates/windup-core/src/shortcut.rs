use serde::{Deserialize, Serialize};
use std::fmt;

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
pub struct Shortcut {
    pub modifiers: ShortcutModifiers,
    pub key: String,
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

    pub fn super_shift_s() -> Self {
        Self::new(ShortcutModifiers::SUPER_SHIFT, "S")
    }

    pub fn print_screen() -> Self {
        Self::new(ShortcutModifiers::NONE, "PRINTSCREEN")
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
            parts.push(if self.key == "PRINTSCREEN" {
                "Print Screen"
            } else {
                &self.key
            });
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
    }
}
