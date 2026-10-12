//! Reading and writing `.umbertheme` files. §3.2 of STYLE-GUIDE.md.

use crate::theme::{Mode, Palette};
use egui::Color32;
use std::path::{Path, PathBuf};

/// The first line of every theme file in the family.
const HEADER: &str = "Umber theme";

/// The extension, matched case-insensitively when listing a directory.
pub const EXTENSION: &str = "umbertheme";

const MAX_NAME: usize = 64;
const MAX_THEMES: usize = 128;
const UNTITLED: &str = "Untitled theme";

/// The twenty-seven keys in the order they are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    Backdrop,
    Window,
    Dock,
    Chrome,
    Popover,
    Border,
    PopoverBorder,
    Control,
    ControlHover,
    ControlActive,
    Rail,
    Knob,
    TextStrong,
    Text,
    TextMuted,
    TextDim,
    Accent,
    AccentDim,
    Warning,
    WarningBg,
    WarningBorder,
    Link1,
    Link2,
    Link3,
    Link4,
    Link5,
    Link6,
}

impl Token {
    pub const ALL: [Token; 27] = [
        Token::Backdrop,
        Token::Window,
        Token::Dock,
        Token::Chrome,
        Token::Popover,
        Token::Border,
        Token::PopoverBorder,
        Token::Control,
        Token::ControlHover,
        Token::ControlActive,
        Token::Rail,
        Token::Knob,
        Token::TextStrong,
        Token::Text,
        Token::TextMuted,
        Token::TextDim,
        Token::Accent,
        Token::AccentDim,
        Token::Warning,
        Token::WarningBg,
        Token::WarningBorder,
        Token::Link1,
        Token::Link2,
        Token::Link3,
        Token::Link4,
        Token::Link5,
        Token::Link6,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Token::Backdrop => "backdrop",
            Token::Window => "window",
            Token::Dock => "dock",
            Token::Chrome => "chrome",
            Token::Popover => "popover",
            Token::Border => "border",
            Token::PopoverBorder => "popover_border",
            Token::Control => "control",
            Token::ControlHover => "control_hover",
            Token::ControlActive => "control_active",
            Token::Rail => "rail",
            Token::Knob => "knob",
            Token::TextStrong => "text_strong",
            Token::Text => "text",
            Token::TextMuted => "text_muted",
            Token::TextDim => "text_dim",
            Token::Accent => "accent",
            Token::AccentDim => "accent_dim",
            Token::Warning => "warning",
            Token::WarningBg => "warning_bg",
            Token::WarningBorder => "warning_border",
            Token::Link1 => "link_1",
            Token::Link2 => "link_2",
            Token::Link3 => "link_3",
            Token::Link4 => "link_4",
            Token::Link5 => "link_5",
            Token::Link6 => "link_6",
        }
    }

    pub fn from_id(id: &str) -> Option<Token> {
        Token::ALL.into_iter().find(|t| t.id() == id)
    }

    pub fn get(self, p: &Palette) -> Color32 {
        match self {
            Token::Backdrop => p.backdrop,
            Token::Window => p.window,
            Token::Dock => p.dock,
            Token::Chrome => p.chrome,
            Token::Popover => p.popover,
            Token::Border => p.line,
            Token::PopoverBorder => p.line_popover,
            Token::Control => p.control,
            Token::ControlHover => p.control_hover,
            Token::ControlActive => p.control_active,
            Token::Rail => p.rail,
            Token::Knob => p.knob,
            Token::TextStrong => p.text_strong,
            Token::Text => p.text,
            Token::TextMuted => p.text_muted,
            Token::TextDim => p.text_dim,
            Token::Accent => p.accent,
            Token::AccentDim => p.accent_dim,
            Token::Warning => p.caution,
            Token::WarningBg => p.caution_bg,
            Token::WarningBorder => p.caution_line,
            Token::Link1 => p.series[0],
            Token::Link2 => p.series[1],
            Token::Link3 => p.series[2],
            Token::Link4 => p.series[3],
            Token::Link5 => p.series[4],
            Token::Link6 => p.series[5],
        }
    }

    pub fn set(self, p: &mut Palette, c: Color32) {
        match self {
            Token::Backdrop => p.backdrop = c,
            Token::Window => p.window = c,
            Token::Dock => p.dock = c,
            Token::Chrome => p.chrome = c,
            Token::Popover => p.popover = c,
            Token::Border => p.line = c,
            Token::PopoverBorder => p.line_popover = c,
            Token::Control => p.control = c,
            Token::ControlHover => p.control_hover = c,
            Token::ControlActive => p.control_active = c,
            Token::Rail => p.rail = c,
            Token::Knob => p.knob = c,
            Token::TextStrong => p.text_strong = c,
            Token::Text => p.text = c,
            Token::TextMuted => p.text_muted = c,
            Token::TextDim => p.text_dim = c,
            Token::Accent => p.accent = c,
            Token::AccentDim => p.accent_dim = c,
            Token::Warning => p.caution = c,
            Token::WarningBg => p.caution_bg = c,
            Token::WarningBorder => p.caution_line = c,
            Token::Link1 => p.series[0] = c,
            Token::Link2 => p.series[1] = c,
            Token::Link3 => p.series[2] = c,
            Token::Link4 => p.series[3] = c,
            Token::Link5 => p.series[4] = c,
            Token::Link6 => p.series[5] = c,
        }
    }
}

/// A custom theme read from a `.umbertheme` file.
#[derive(Clone, Debug)]
pub struct CustomTheme {
    pub id: String,
    pub name: String,
    pub base: String,
    pub palette: Palette,
    pub skipped: usize,
}

impl CustomTheme {
    pub fn is_dark(&self) -> bool {
        !self.base.eq_ignore_ascii_case("paper")
    }
}

fn base_palette(base: &str) -> Palette {
    match base.to_ascii_lowercase().as_str() {
        "paper" => Palette::of(Mode::Light, 28.0),
        _ => Palette::of(Mode::Dark, 28.0),
    }
}

/// Read a `.umbertheme` file content.
pub fn read(text: &str, stem: &str) -> Result<CustomTheme, String> {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();
    let first = first.trim_start_matches('\u{feff}').trim();
    if !first.eq_ignore_ascii_case(HEADER) {
        return Err(format!(
            "File does not start with \"{HEADER}\", so it is not a valid Umber theme."
        ));
    }

    let body: Vec<(String, String)> = lines
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            Some((key.trim().to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect();

    let base = body
        .iter()
        .rev()
        .find(|(k, _)| k == "base")
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| "graphite".to_string());
    let mut palette = base_palette(&base);

    let mut name = String::new();
    let mut skipped = 0usize;
    for (key, value) in &body {
        if key == "base" {
            continue;
        }
        if key == "name" {
            name = clean_name(value);
            continue;
        }
        match (Token::from_id(key), parse_hex(value)) {
            (Some(token), Some(colour)) => token.set(&mut palette, colour),
            _ => skipped += 1,
        }
    }

    if name.is_empty() {
        name = clean_name(stem);
    }
    if name.is_empty() {
        name = UNTITLED.to_string();
    }

    Ok(CustomTheme {
        id: slug(stem),
        name,
        base,
        palette,
        skipped,
    })
}

/// Serialize a theme into `.umbertheme` format.
pub fn write(theme: &CustomTheme) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str(HEADER);
    out.push('\n');
    out.push_str("# Umber theme format §3.2\n");
    out.push_str(&format!("name = {}\n", theme.name));
    out.push_str(&format!("base = {}\n", theme.base));
    for token in Token::ALL {
        out.push_str(&format!(
            "{} = {}\n",
            token.id(),
            to_hex(token.get(&theme.palette))
        ));
    }
    out
}

fn parse_hex(value: &str) -> Option<Color32> {
    let v = value.trim().trim_start_matches('#');
    if !v.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let digits: Vec<u32> = v.chars().filter_map(|c| c.to_digit(16)).collect();
    match digits.len() {
        3 => Some(Color32::from_rgb(
            (digits[0] * 17) as u8,
            (digits[1] * 17) as u8,
            (digits[2] * 17) as u8,
        )),
        6 => Some(Color32::from_rgb(
            (digits[0] * 16 + digits[1]) as u8,
            (digits[2] * 16 + digits[3]) as u8,
            (digits[4] * 16 + digits[5]) as u8,
        )),
        _ => None,
    }
}

fn to_hex(c: Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
}

fn clean_name(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_NAME)
        .collect::<String>()
        .trim()
        .to_string()
}

pub fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c.to_ascii_lowercase());
        } else {
            pending_dash = true;
        }
        if out.len() >= 48 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "theme".to_string()
    } else {
        out
    }
}

/// Load all `.umbertheme` files from directory.
pub fn load_all_from_dir(dir: &Path) -> (Vec<CustomTheme>, Vec<String>) {
    let mut themes = Vec::new();
    let mut problems = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (themes, problems);
    };

    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case(EXTENSION))
        })
        .collect();
    paths.sort();

    for path in paths.into_iter().take(MAX_THEMES) {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        match std::fs::read_to_string(&path) {
            Ok(text) => match read(&text, &stem) {
                Ok(theme) => themes.push(theme),
                Err(why) => problems.push(format!("{}: {why}", path.display())),
            },
            Err(e) => problems.push(format!("{}: {e}", path.display())),
        }
    }
    (themes, problems)
}

/// Import a `.umbertheme` file by copying it into the target themes directory.
pub fn import_theme_file(source: &Path, themes_dir: &Path) -> Result<CustomTheme, String> {
    std::fs::create_dir_all(themes_dir).map_err(|e| e.to_string())?;
    let content = std::fs::read_to_string(source).map_err(|e| e.to_string())?;
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "imported".to_string());
    let theme = read(&content, &stem)?;

    let target_path = themes_dir.join(format!("{}.{EXTENSION}", theme.id));
    std::fs::write(&target_path, &content).map_err(|e| e.to_string())?;
    Ok(theme)
}
