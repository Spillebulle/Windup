//! The one table of global hotkeys, derived from the config, plus the key
//! name translations each backend needs.

use crate::config::Config;
use crate::shortcut::{PRINT_SCREEN, Shortcut};
use crate::tool::ToolId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    ClipboardPopup,
    PastePlain,
    CaptureLauncher,
    Screenshot,
}

impl Action {
    pub fn for_tool(tool: ToolId) -> Action {
        match tool {
            ToolId::ClipboardHistory => Action::ClipboardPopup,
            ToolId::PastePlainText => Action::PastePlain,
            ToolId::CaptureLauncher => Action::CaptureLauncher,
            ToolId::Screenshot => Action::Screenshot,
        }
    }
}

/// Enabled tools in `ToolId::ALL` order. A shortcut already taken by an
/// earlier tool is left out: the first one wins.
pub fn bindings(config: &Config) -> Vec<(Shortcut, Action)> {
    let mut out: Vec<(Shortcut, Action)> = Vec::new();
    for tool in ToolId::ALL {
        if !config.is_tool_enabled(tool) {
            continue;
        }
        let shortcut = config.tool_shortcut(tool);
        if out.iter().all(|(s, _)| s != shortcut) {
            out.push((shortcut.clone(), Action::for_tool(tool)));
        }
    }
    out
}

/// Enabled tools whose shortcut an earlier enabled tool already has.
pub fn clashes(config: &Config) -> Vec<ToolId> {
    let mut seen: Vec<&Shortcut> = Vec::new();
    let mut out = Vec::new();
    for tool in ToolId::ALL {
        if !config.is_tool_enabled(tool) {
            continue;
        }
        let shortcut = config.tool_shortcut(tool);
        if seen.contains(&shortcut) {
            out.push(tool);
        } else {
            seen.push(shortcut);
        }
    }
    out
}

/// "F1".."F24" -> 1..=24 (no leading zeros, no sign).
fn function_key(key: &str) -> Option<u16> {
    let digits = key.strip_prefix('F')?;
    if digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|n| (1..=24).contains(n))
}

/// Windows virtual-key code for a canonical key name.
pub fn windows_vk(key: &str) -> Option<u16> {
    let key = key.to_uppercase();
    let key = key.as_str();
    if let [c @ (b'A'..=b'Z' | b'0'..=b'9')] = key.as_bytes() {
        return Some(*c as u16);
    }
    if let Some(n) = function_key(key) {
        return Some(0x70 + n - 1);
    }
    Some(match key {
        PRINT_SCREEN => 0x2C,
        "SPACE" => 0x20,
        "ENTER" => 0x0D,
        "TAB" => 0x09,
        "INSERT" => 0x2D,
        "DELETE" => 0x2E,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" => 0x21,
        "PAGEDOWN" => 0x22,
        _ => return None,
    })
}

/// X keysym name for a canonical key name (COSMIC and the portals use these).
pub fn xkb_keysym_name(key: &str) -> Option<&'static str> {
    let key = key.to_uppercase();
    let key = key.as_str();
    const LETTERS: &str = "abcdefghijklmnopqrstuvwxyz";
    const DIGITS: &str = "0123456789";
    const FUNCTION: [&str; 24] = [
        "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14",
        "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24",
    ];
    if let [c @ b'A'..=b'Z'] = key.as_bytes() {
        let i = (c - b'A') as usize;
        return Some(&LETTERS[i..=i]);
    }
    if let [c @ b'0'..=b'9'] = key.as_bytes() {
        let i = (c - b'0') as usize;
        return Some(&DIGITS[i..=i]);
    }
    if let Some(n) = function_key(key) {
        return Some(FUNCTION[n as usize - 1]);
    }
    Some(match key {
        PRINT_SCREEN => "Print",
        "SPACE" => "space",
        "ENTER" => "Return",
        "TAB" => "Tab",
        "INSERT" => "Insert",
        "DELETE" => "Delete",
        "HOME" => "Home",
        "END" => "End",
        "PAGEUP" => "Prior",
        "PAGEDOWN" => "Next",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_bind_all_four() {
        assert_eq!(bindings(&Config::default()).len(), 4);
        assert!(clashes(&Config::default()).is_empty());
    }

    #[test]
    fn a_disabled_tool_is_not_bound() {
        let mut c = Config::default();
        c.screenshot.enabled = false;
        let b = bindings(&c);
        assert_eq!(b.len(), 3);
        assert!(!b.contains(&(Shortcut::print_screen(), Action::Screenshot)));
    }

    #[test]
    fn the_first_tool_wins_a_shared_shortcut() {
        let mut c = Config::default();
        c.screenshot.shortcut = Shortcut::super_shift_s();
        let b = bindings(&c);
        assert_eq!(
            b.iter()
                .filter(|(s, _)| *s == Shortcut::super_shift_s())
                .count(),
            1
        );
        assert!(b.contains(&(Shortcut::super_shift_s(), Action::CaptureLauncher)));
        assert_eq!(clashes(&c), vec![ToolId::Screenshot]);
    }

    #[test]
    fn vk_codes() {
        assert_eq!(windows_vk("PRINTSCREEN"), Some(0x2C));
        assert_eq!(windows_vk("V"), Some(0x56));
        assert_eq!(windows_vk("7"), Some(0x37));
        assert_eq!(windows_vk("F1"), Some(0x70));
        assert_eq!(windows_vk("F13"), Some(0x7C));
        assert_eq!(windows_vk("F24"), Some(0x87));
        assert_eq!(windows_vk("F25"), None);
        assert_eq!(windows_vk("F01"), None);
        assert_eq!(windows_vk("nonsense"), None);
    }

    #[test]
    fn lookups_ignore_case() {
        assert_eq!(windows_vk("Space"), Some(0x20));
        assert_eq!(windows_vk("f5"), Some(0x74));
        assert_eq!(xkb_keysym_name("PageDown"), Some("Next"));
    }

    #[test]
    fn legacy_case_still_clashes() {
        // Built by hand, as an unnormalised value would be.
        let mut c = Config::default();
        c.clipboard.shortcut.key = "SPACE".into();
        c.clipboard.shortcut.modifiers = crate::shortcut::ShortcutModifiers::SUPER;
        let json = r#"{"modifiers":{"super_key":true,"ctrl":false,"alt":false,"shift":false},"key":"Space"}"#;
        c.paste_plain.shortcut = serde_json::from_str(json).unwrap();
        assert_eq!(clashes(&c), vec![ToolId::PastePlainText]);
        assert_eq!(bindings(&c).len(), 3);
    }

    #[test]
    fn keysym_names() {
        assert_eq!(xkb_keysym_name("PRINTSCREEN"), Some("Print"));
        assert_eq!(xkb_keysym_name("V"), Some("v"));
        assert_eq!(xkb_keysym_name("5"), Some("5"));
        assert_eq!(xkb_keysym_name("F24"), Some("F24"));
        assert_eq!(xkb_keysym_name("PAGEDOWN"), Some("Next"));
        assert_eq!(xkb_keysym_name("nonsense"), None);
    }

    #[test]
    fn both_tables_know_the_same_keys() {
        let mut keys: Vec<String> = ('A'..='Z').map(String::from).collect();
        keys.extend(('0'..='9').map(String::from));
        keys.extend((1..=24).map(|n| format!("F{n}")));
        for k in [
            "PRINTSCREEN",
            "SPACE",
            "ENTER",
            "TAB",
            "INSERT",
            "DELETE",
            "HOME",
            "END",
            "PAGEUP",
            "PAGEDOWN",
        ] {
            keys.push(k.into());
        }
        for k in keys {
            assert!(windows_vk(&k).is_some(), "{k}");
            assert!(xkb_keysym_name(&k).is_some(), "{k}");
        }
    }
}
