//! What the Steps settings choose between.

use crate::config::DEFAULT_COLOUR;
use serde::{Deserialize, Serialize};

/// What adds a step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepTrigger {
    #[default]
    ClicksAndHotkey,
    HotkeyOnly,
    ClicksOnly,
}

impl StepTrigger {
    pub const ALL: [StepTrigger; 3] = [
        StepTrigger::ClicksAndHotkey,
        StepTrigger::HotkeyOnly,
        StepTrigger::ClicksOnly,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StepTrigger::ClicksAndHotkey => "Clicks and hotkey",
            StepTrigger::HotkeyOnly => "Hotkey only",
            StepTrigger::ClicksOnly => "Clicks only",
        }
    }
}

/// How a step's picture marks where the click was.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkerKind {
    Ring,
    #[default]
    Badge,
    Arrow,
}

impl MarkerKind {
    pub const ALL: [MarkerKind; 3] = [MarkerKind::Ring, MarkerKind::Badge, MarkerKind::Arrow];

    pub fn label(self) -> &'static str {
        match self {
            MarkerKind::Ring => "Ring",
            MarkerKind::Badge => "Numbered badge",
            MarkerKind::Arrow => "Arrow",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkerSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl MarkerSize {
    pub const ALL: [MarkerSize; 3] = [MarkerSize::Small, MarkerSize::Medium, MarkerSize::Large];

    pub fn points(self) -> f32 {
        match self {
            MarkerSize::Small => 24.0,
            MarkerSize::Medium => 32.0,
            MarkerSize::Large => 44.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MarkerSize::Small => "Small",
            MarkerSize::Medium => "Medium",
            MarkerSize::Large => "Large",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MarkerStyle {
    pub kind: MarkerKind,
    pub colour: [u8; 4],
    pub size: MarkerSize,
}

impl Default for MarkerStyle {
    fn default() -> Self {
        Self {
            kind: MarkerKind::default(),
            colour: DEFAULT_COLOUR,
            size: MarkerSize::default(),
        }
    }
}

/// What the steps are exported as.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    #[default]
    Html,
    Pdf,
    Markdown,
    Images,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 4] = [
        ExportFormat::Html,
        ExportFormat::Pdf,
        ExportFormat::Markdown,
        ExportFormat::Images,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Html => "Web page (HTML)",
            ExportFormat::Pdf => "PDF",
            ExportFormat::Markdown => "Markdown",
            ExportFormat::Images => "Images",
        }
    }

    /// The word a command line names the format by.
    pub fn arg(self) -> &'static str {
        match self {
            ExportFormat::Html => "html",
            ExportFormat::Pdf => "pdf",
            ExportFormat::Markdown => "md",
            ExportFormat::Images => "images",
        }
    }

    pub fn from_arg(arg: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.arg() == arg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_sizes_and_defaults() {
        assert_eq!(StepTrigger::default(), StepTrigger::ClicksAndHotkey);
        assert_eq!(StepTrigger::HotkeyOnly.label(), "Hotkey only");
        assert_eq!(MarkerKind::default(), MarkerKind::Badge);
        assert_eq!(MarkerKind::Badge.label(), "Numbered badge");
        let points: Vec<f32> = MarkerSize::ALL.iter().map(|s| s.points()).collect();
        assert_eq!(points, [24.0, 32.0, 44.0]);
        assert_eq!(MarkerStyle::default().colour, DEFAULT_COLOUR);
    }

    #[test]
    fn export_formats_round_trip_through_their_arguments() {
        assert_eq!(ExportFormat::default(), ExportFormat::Html);
        for f in ExportFormat::ALL {
            assert_eq!(ExportFormat::from_arg(f.arg()), Some(f));
        }
        assert_eq!(ExportFormat::from_arg("md"), Some(ExportFormat::Markdown));
        assert_eq!(ExportFormat::from_arg("docx"), None);
    }
}
