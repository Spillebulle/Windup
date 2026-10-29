use crate::shortcut::{Shortcut, ShortcutModifiers};
use crate::tool::ToolId;
use crate::video::{Bitrate, Codec, Container};
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
#[serde(default)]
pub struct CaptureLauncherConfig {
    pub enabled: bool,
    pub shortcut: Shortcut,
}

impl Default for CaptureLauncherConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: Shortcut::print_screen(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TargetKind {
    #[default]
    Area,
    Window,
    FullScreen,
}

/// Editor keys. Esc is deliberately not a field: it always cancels.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorKeys {
    pub copy: Shortcut,
    pub save: Shortcut,
    pub undo: Shortcut,
    pub redo: Shortcut,
    pub loupe: Shortcut,
    pub select_all: Shortcut,
}

impl Default for EditorKeys {
    fn default() -> Self {
        Self {
            copy: Shortcut::new(ShortcutModifiers::NONE, "ENTER"),
            save: Shortcut::new(ShortcutModifiers::CTRL, "S"),
            undo: Shortcut::new(ShortcutModifiers::CTRL, "Z"),
            redo: Shortcut::new(ShortcutModifiers::CTRL, "Y"),
            loupe: Shortcut::new(ShortcutModifiers::NONE, "Z"),
            select_all: Shortcut::new(ShortcutModifiers::CTRL, "A"),
        }
    }
}

/// The editor's tool sizes, in screen points, so a size looks the same on
/// every desktop and monitor. A drawn annotation keeps its own width.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolSizes {
    pub pen: f32,
    pub rect: f32,
    pub ellipse: f32,
    pub blur: f32,
    pub mosaic: f32,
}

impl Default for ToolSizes {
    fn default() -> Self {
        Self {
            pen: 4.0,
            rect: 3.0,
            ellipse: 3.0,
            blur: 12.0,
            mosaic: 12.0,
        }
    }
}

/// `windup_ui::theme::Palette::dark(28.0).critical`, i.e. `oklch(0.66, 0.13, 22.0)`
/// as sRGB. Core cannot depend on the UI crate, so the value is written out.
const DEFAULT_COLOUR: [u8; 4] = [214, 111, 110, 255];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenshotConfig {
    pub enabled: bool,
    pub shortcut: Shortcut,
    pub loupe_on: bool,
    pub save_dir: Option<PathBuf>,
    pub name_pattern: String,
    pub format: ImageFormat,
    pub also_save_on_copy: bool,
    pub last_target: TargetKind,
    pub colour: [u8; 4],
    pub sizes: ToolSizes,
    pub keys: EditorKeys,
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: Shortcut::ctrl_shift_s(),
            loupe_on: false,
            save_dir: None,
            name_pattern: "Screenshot {date} {time}".to_string(),
            format: ImageFormat::default(),
            also_save_on_copy: false,
            last_target: TargetKind::default(),
            colour: DEFAULT_COLOUR,
            sizes: ToolSizes::default(),
            keys: EditorKeys::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoConfig {
    pub enabled: bool,
    pub shortcut: Shortcut,
    pub pause_shortcut: Shortcut,
    pub save_dir: Option<PathBuf>,
    pub name_pattern: String,
    pub container: Container,
    pub codec: Codec,
    /// `None` picks the encoder automatically.
    pub encoder: Option<String>,
    pub fps: u32,
    pub bitrate: Bitrate,
    pub gif_colours: u16,
    pub system_audio: bool,
    pub mic: bool,
    pub system_device: Option<String>,
    pub mic_device: Option<String>,
    pub last_target: TargetKind,
    /// The ScreenCast portal's restore token, so Wayland asks once.
    pub screencast_token: Option<String>,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: Shortcut::super_shift_r(),
            pause_shortcut: Shortcut::super_shift_p(),
            save_dir: None,
            name_pattern: "Recording {date} {time}".to_string(),
            container: Container::Mp4,
            codec: Codec::H264,
            encoder: None,
            fps: 30,
            bitrate: Bitrate::Auto,
            gif_colours: 256,
            system_audio: false,
            mic: false,
            system_device: None,
            mic_device: None,
            last_target: TargetKind::default(),
            screencast_token: None,
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
    #[serde(default)]
    pub capture: CaptureLauncherConfig,
    #[serde(default)]
    pub screenshot: ScreenshotConfig,
    #[serde(default)]
    pub video: VideoConfig,
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
            capture: CaptureLauncherConfig::default(),
            screenshot: ScreenshotConfig::default(),
            video: VideoConfig::default(),
        }
    }
}

impl Config {
    pub fn config_dir() -> Option<PathBuf> {
        ProjectDirs::from("com", "spillebulle", "windup")
            .map(|proj| proj.config_dir().to_path_buf())
    }

    /// Where scratch files go, such as a recording's segments before the join.
    pub fn cache_dir() -> Option<PathBuf> {
        ProjectDirs::from("com", "spillebulle", "windup").map(|proj| proj.cache_dir().to_path_buf())
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
            ToolId::CaptureLauncher => self.capture.enabled,
            ToolId::Screenshot => self.screenshot.enabled,
            ToolId::Video => self.video.enabled,
        }
    }

    pub fn set_tool_enabled(&mut self, tool: ToolId, enabled: bool) {
        match tool {
            ToolId::ClipboardHistory => self.clipboard.enabled = enabled,
            ToolId::PastePlainText => self.paste_plain.enabled = enabled,
            ToolId::CaptureLauncher => self.capture.enabled = enabled,
            ToolId::Screenshot => self.screenshot.enabled = enabled,
            ToolId::Video => self.video.enabled = enabled,
        }
        self.save();
    }

    pub fn tool_shortcut(&self, tool: ToolId) -> &Shortcut {
        match tool {
            ToolId::ClipboardHistory => &self.clipboard.shortcut,
            ToolId::PastePlainText => &self.paste_plain.shortcut,
            ToolId::CaptureLauncher => &self.capture.shortcut,
            ToolId::Screenshot => &self.screenshot.shortcut,
            ToolId::Video => &self.video.shortcut,
        }
    }

    pub fn set_tool_shortcut(&mut self, tool: ToolId, shortcut: Shortcut) {
        match tool {
            ToolId::ClipboardHistory => self.clipboard.shortcut = shortcut,
            ToolId::PastePlainText => self.paste_plain.shortcut = shortcut,
            ToolId::CaptureLauncher => self.capture.shortcut = shortcut,
            ToolId::Screenshot => self.screenshot.shortcut = shortcut,
            ToolId::Video => self.video.shortcut = shortcut,
        }
        self.save();
    }

    pub fn restore_defaults(&mut self) {
        *self = Self::default();
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // config.json exactly as 0.1.2 writes it, with accent_hue and max_items changed.
    const OLD_CONFIG: &str = r#"{
  "theme_mode": "Dark",
  "accent_hue": 200.0,
  "custom_theme": null,
  "launch_at_login": false,
  "close_to_tray": true,
  "clipboard": {
    "enabled": true,
    "shortcut": {
      "modifiers": {
        "super_key": true,
        "ctrl": false,
        "alt": false,
        "shift": false
      },
      "key": "V"
    },
    "max_items": 42,
    "auto_paste": true,
    "save_images": true,
    "popup_width": 380.0,
    "popup_height": 480.0
  },
  "paste_plain": {
    "enabled": true,
    "shortcut": {
      "modifiers": {
        "super_key": true,
        "ctrl": false,
        "alt": false,
        "shift": true
      },
      "key": "V"
    }
  }
}"#;

    #[test]
    fn a_012_config_keeps_its_settings_and_gains_capture_defaults() {
        let c: Config = serde_json::from_str(OLD_CONFIG).unwrap();
        assert_eq!(c.accent_hue, 200.0);
        assert_eq!(c.clipboard.max_items, 42);
        assert_eq!(c.capture.shortcut, Shortcut::print_screen());
        assert_eq!(c.screenshot.shortcut, Shortcut::ctrl_shift_s());
        assert!(!c.screenshot.loupe_on);
    }

    #[test]
    fn a_config_from_0_2_without_video_keeps_everything_else() {
        let mut v = serde_json::to_value(Config::default()).unwrap();
        v.as_object_mut().unwrap().remove("video");
        v["clipboard"]["max_items"] = serde_json::json!(37);
        let c: Config = serde_json::from_value(v).unwrap();
        assert_eq!(c.clipboard.max_items, 37);
        assert_eq!(c.video, VideoConfig::default());
    }

    #[test]
    fn video_defaults_and_tool_accessors_cover_video() {
        let mut c = Config::default();
        assert_eq!(c.video.shortcut, Shortcut::super_shift_r());
        assert_eq!(c.video.pause_shortcut, Shortcut::super_shift_p());
        assert_eq!(c.video.name_pattern, "Recording {date} {time}");
        assert_eq!(
            (c.video.container, c.video.codec),
            (Container::Mp4, Codec::H264)
        );
        assert_eq!((c.video.fps, c.video.gif_colours), (30, 256));
        assert!(!c.video.system_audio && !c.video.mic);
        assert!(c.is_tool_enabled(ToolId::Video));
        c.video.enabled = false;
        assert!(!c.is_tool_enabled(ToolId::Video));
        assert_eq!(c.tool_shortcut(ToolId::Video), &Shortcut::super_shift_r());
    }

    #[test]
    fn new_structs_fill_missing_fields_with_defaults() {
        let mut v = serde_json::to_value(Config::default()).unwrap();
        v["screenshot"] = serde_json::json!({"loupe_on": true, "sizes": {"pen": 9.0}});
        let c: Config = serde_json::from_value(v).unwrap();
        assert!(c.screenshot.loupe_on);
        assert_eq!(c.screenshot.sizes.pen, 9.0);
        assert_eq!(c.screenshot.sizes.blur, 12.0);
    }

    #[test]
    fn screenshot_defaults_and_tool_accessors_cover_new_ids() {
        let mut c = Config::default();
        assert_eq!(c.screenshot.name_pattern, "Screenshot {date} {time}");
        assert_eq!(c.screenshot.colour, [214, 111, 110, 255]);
        assert_eq!(c.screenshot.sizes.pen, 4.0);
        assert_eq!(
            c.screenshot.keys.redo,
            Shortcut::new(ShortcutModifiers::CTRL, "Y")
        );
        assert_eq!(c.screenshot.last_target, TargetKind::Area);
        assert_eq!(c.screenshot.format, ImageFormat::Png);
        assert!(c.is_tool_enabled(ToolId::Screenshot));
        c.screenshot.enabled = false;
        assert!(!c.is_tool_enabled(ToolId::Screenshot));
        assert_eq!(
            c.tool_shortcut(ToolId::CaptureLauncher),
            &Shortcut::print_screen()
        );
    }
}
