use crate::shortcut::Shortcut;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolId {
    ClipboardHistory,
    PastePlainText,
    CaptureLauncher,
    Screenshot,
}

impl ToolId {
    pub const ALL: [ToolId; 4] = [
        ToolId::ClipboardHistory,
        ToolId::PastePlainText,
        ToolId::CaptureLauncher,
        ToolId::Screenshot,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            ToolId::ClipboardHistory => "clipboard_history",
            ToolId::PastePlainText => "paste_plain_text",
            ToolId::CaptureLauncher => "capture_launcher",
            ToolId::Screenshot => "screenshot",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            ToolId::ClipboardHistory => "Clipboard history",
            ToolId::PastePlainText => "Paste plain text",
            ToolId::CaptureLauncher => "Capture",
            ToolId::Screenshot => "Screenshot",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            ToolId::ClipboardHistory => {
                "Opens a floating popup of your clipboard history at your cursor with image previews and rich formatting."
            }
            ToolId::PastePlainText => {
                "Instantly pastes the current clipboard contents with all styling, formatting, and HTML stripped."
            }
            ToolId::CaptureLauncher => {
                "Opens the capture bar to take a screenshot or record the screen."
            }
            ToolId::Screenshot => "Takes a screenshot straight away and opens it in the editor.",
        }
    }

    pub fn default_shortcut(&self) -> Option<Shortcut> {
        match self {
            ToolId::ClipboardHistory => Some(Shortcut::super_v()),
            ToolId::PastePlainText => Some(Shortcut::super_shift_v()),
            ToolId::CaptureLauncher => Some(Shortcut::super_shift_s()),
            ToolId::Screenshot => Some(Shortcut::print_screen()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_default_shortcut_and_none_collide() {
        let mut seen = std::collections::HashSet::new();
        for t in ToolId::ALL {
            assert!(seen.insert(t.default_shortcut().unwrap()), "{t:?}");
        }
        assert_eq!(ToolId::ALL.len(), 4);
    }
}
