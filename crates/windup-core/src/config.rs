use crate::shortcut::Shortcut;
use crate::tool::ToolId;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
    System,
}

impl ThemeMode {
    pub const ALL: [ThemeMode; 3] = [ThemeMode::Dark, ThemeMode::Light, ThemeMode::System];

    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::Dark => "Dark (Graphite)",
            ThemeMode::Light => "Light (Paper)",
            ThemeMode::System => "Follow system",
        }
    }
}

fn default_max_items() -> usize {
    100
}

fn default_popup_width() -> f32 {
    380.0
}

fn default_popup_height() -> f32 {
    480.0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClipboardToolConfig {
    pub enabled: bool,
    pub shortcut: Shortcut,
    #[serde(default = "default_max_items")]
    pub max_items: usize,
    /// If true, selecting an item immediately pastes it into the active target window.
    /// If false, selecting an item populates the OS clipboard without auto-pasting.
    pub auto_paste: bool,
    pub save_images: bool,
    #[serde(default = "default_popup_width")]
    pub popup_width: f32,
    #[serde(default = "default_popup_height")]
    pub popup_height: f32,
}

impl Default for ClipboardToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: Shortcut::super_v(),
            max_items: default_max_items(),
            auto_paste: true,
            save_images: true,
            popup_width: default_popup_width(),
            popup_height: default_popup_height(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PastePlainToolConfig {
    pub enabled: bool,
    pub shortcut: Shortcut,
}

impl Default for PastePlainToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: Shortcut::super_shift_v(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub theme_mode: ThemeMode,
    pub accent_hue: f32,
    #[serde(default)]
    pub custom_theme: Option<String>,
    pub launch_at_login: bool,
    pub close_to_tray: bool,
    pub clipboard: ClipboardToolConfig,
    pub paste_plain: PastePlainToolConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme_mode: ThemeMode::Dark,
            accent_hue: 28.0, // Windup warm terracotta/bronze
            custom_theme: None,
            launch_at_login: false,
            close_to_tray: true,
            clipboard: ClipboardToolConfig::default(),
            paste_plain: PastePlainToolConfig::default(),
        }
    }
}

impl Config {
    pub fn config_dir() -> Option<PathBuf> {
        ProjectDirs::from("com", "spillebulle", "windup").map(|proj| proj.config_dir().to_path_buf())
    }

    pub fn config_file_path() -> Option<PathBuf> {
        Self::config_dir().map(|dir| dir.join("config.json"))
    }

    pub fn history_path() -> Option<PathBuf> {
        Self::config_dir().map(|dir| dir.join("clipboard_history.json"))
    }

    pub fn themes_dir() -> Option<PathBuf> {
        Self::config_dir().map(|dir| dir.join("themes"))
    }

    pub fn load() -> Self {
        let path = match Self::config_file_path() {
            Some(p) => p,
            None => return Self::default(),
        };

        if !path.exists() {
            return Self::default();
        }

        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                log::warn!(
                    "Failed to parse config at {}: {}, using defaults",
                    path.display(),
                    e
                );
                Self::default()
            }),
            Err(e) => {
                log::warn!(
                    "Failed to read config at {}: {}, using defaults",
                    path.display(),
                    e
                );
                Self::default()
            }
        }
    }

    pub fn save(&self) {
        let path = match Self::config_file_path() {
            Some(p) => p,
            None => return,
        };

        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        if let Ok(json) = serde_json::to_string_pretty(self)
            && let Err(e) = fs::write(&path, json)
        {
            log::error!("Failed to save config to {}: {}", path.display(), e);
        }
    }

    pub fn is_tool_enabled(&self, tool: ToolId) -> bool {
        match tool {
            ToolId::ClipboardHistory => self.clipboard.enabled,
            ToolId::PastePlainText => self.paste_plain.enabled,
        }
    }

    pub fn set_tool_enabled(&mut self, tool: ToolId, enabled: bool) {
        match tool {
            ToolId::ClipboardHistory => self.clipboard.enabled = enabled,
            ToolId::PastePlainText => self.paste_plain.enabled = enabled,
        }
        self.save();
    }

    pub fn tool_shortcut(&self, tool: ToolId) -> &Shortcut {
        match tool {
            ToolId::ClipboardHistory => &self.clipboard.shortcut,
            ToolId::PastePlainText => &self.paste_plain.shortcut,
        }
    }

    pub fn set_tool_shortcut(&mut self, tool: ToolId, shortcut: Shortcut) {
        match tool {
            ToolId::ClipboardHistory => self.clipboard.shortcut = shortcut,
            ToolId::PastePlainText => self.paste_plain.shortcut = shortcut,
        }
        self.save();
    }

    pub fn restore_defaults(&mut self) {
        *self = Self::default();
        self.save();
    }
}
