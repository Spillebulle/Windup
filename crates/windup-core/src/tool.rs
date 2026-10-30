use crate::shortcut::Shortcut;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolId {
    ClipboardHistory,
    PastePlainText,
    CaptureLauncher,
    Screenshot,
    Video,
    Steps,
}

impl ToolId {
    pub const ALL: [ToolId; 6] = [
        ToolId::ClipboardHistory,
        ToolId::PastePlainText,
        ToolId::CaptureLauncher,
        ToolId::Screenshot,
        ToolId::Video,
        ToolId::Steps,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            ToolId::ClipboardHistory => "clipboard_history",
            ToolId::PastePlainText => "paste_plain_text",
            ToolId::CaptureLauncher => "capture_launcher",
            ToolId::Screenshot => "screenshot",
            ToolId::Video => "video",
            ToolId::Steps => "steps",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            ToolId::ClipboardHistory => "Clipboard history",
            ToolId::PastePlainText => "Paste plain text",
            ToolId::CaptureLauncher => "Capture",
            ToolId::Screenshot => "Screenshot",
            ToolId::Video => "Video",
            ToolId::Steps => "Steps",
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
            ToolId::Video => {
                "Records the screen to a video file or a GIF, with sound if you want it."
            }
            ToolId::Steps => {
                "Records the steps you take, a picture for each click or key press, and exports them as a document."
            }
        }
    }

    pub fn default_shortcut(&self) -> Option<Shortcut> {
        match self {
            ToolId::ClipboardHistory => Some(Shortcut::super_v()),
            ToolId::PastePlainText => Some(Shortcut::super_shift_v()),
            ToolId::CaptureLauncher => Some(Shortcut::print_screen()),
            ToolId::Screenshot => Some(Shortcut::ctrl_shift_s()),
            ToolId::Video => Some(Shortcut::super_shift_r()),
            ToolId::Steps => Some(Shortcut::super_shift_g()),
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
        assert_eq!(ToolId::ALL.len(), 6);
    }

    #[test]
    fn the_step_and_pause_keys_collide_with_no_tool_default() {
        let c = crate::config::Config::default();
        for s in [&c.steps.step_shortcut, &c.video.pause_shortcut] {
            assert!(
                ToolId::ALL
                    .iter()
                    .all(|t| t.default_shortcut().as_ref() != Some(s)),
                "{s}"
            );
        }
        assert_ne!(c.steps.step_shortcut, c.video.pause_shortcut);
    }
}
