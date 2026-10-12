use crate::shortcut::Shortcut;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolId {
    ClipboardHistory,
    PastePlainText,
}

impl ToolId {
    pub const ALL: [ToolId; 2] = [ToolId::ClipboardHistory, ToolId::PastePlainText];

    pub fn as_str(&self) -> &'static str {
        match self {
            ToolId::ClipboardHistory => "clipboard_history",
            ToolId::PastePlainText => "paste_plain_text",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            ToolId::ClipboardHistory => "Clipboard history",
            ToolId::PastePlainText => "Paste plain text",
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
        }
    }

    pub fn default_shortcut(&self) -> Option<Shortcut> {
        match self {
            ToolId::ClipboardHistory => Some(Shortcut::super_v()),
            ToolId::PastePlainText => Some(Shortcut::super_shift_v()),
        }
    }
}
