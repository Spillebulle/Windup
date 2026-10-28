//! Windup's shortcuts on COSMIC, bound by Windup itself.
//!
//! COSMIC has no GlobalShortcuts portal, but its compositor watches the
//! custom-shortcuts file COSMIC Settings writes, so Windup adds its shortcuts
//! there (`windup_core::cosmic_shortcuts` decides what to add, from the
//! config). It offers each one once: a marker in Windup's own config directory
//! lists the commands already offered, so a shortcut the user removes in
//! Settings stays removed, while a tool added by a later Windup is still
//! offered. A key changed in Windup's own settings is not carried over to a
//! binding COSMIC already has.

use std::path::PathBuf;
use std::{fs, io};
use windup_core::config::Config;
use windup_core::cosmic_shortcuts::{self, Wanted};

const MARKER: &str = "cosmic-shortcuts";

/// Adds the config's shortcuts to COSMIC's custom shortcuts, each the first
/// time Windup runs on COSMIC with it.
pub fn bind_once(config: &Config) {
    if !is_cosmic() {
        return;
    }
    let Some(dir) = Config::config_dir() else {
        return;
    };
    let marker = dir.join(MARKER);
    let mut offered = cosmic_shortcuts::offered(fs::read_to_string(&marker).ok().as_deref());
    let pending = cosmic_shortcuts::pending(cosmic_shortcuts::wanted(config), &offered);
    if pending.is_empty() {
        return;
    }
    match bind(&pending) {
        Ok(()) => {
            offered.extend(pending.into_iter().map(|w| w.args));
            let _ = fs::create_dir_all(&dir);
            let _ = fs::write(&marker, cosmic_shortcuts::marker_text(&offered));
        }
        Err(e) => log::warn!("COSMIC shortcuts not added: {e}"),
    }
}

fn is_cosmic() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .is_ok_and(|d| d.split(':').any(|d| d.eq_ignore_ascii_case("cosmic")))
}

fn shortcuts_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("cosmic/com.system76.CosmicSettings.Shortcuts/v1/custom"))
}

fn bind(wanted: &[Wanted]) -> Result<(), String> {
    let file = shortcuts_file().ok_or("no home directory")?;
    let existing = match fs::read_to_string(&file) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{}: {e}", file.display())),
    };
    // An AppImage is not on the PATH; the command names the file itself.
    let program = std::env::var("APPIMAGE")
        .ok()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "windup".into());
    let (new, outcomes) = cosmic_shortcuts::merge(existing.as_deref(), wanted, &program)
        .map_err(|e| format!("{} is left alone, it did not read: {}", file.display(), e.0))?;
    for (wanted, outcome) in wanted.iter().zip(&outcomes) {
        log::info!(
            "COSMIC shortcut {}+{} ({}): {outcome:?}",
            wanted.modifiers.join("+"),
            wanted.key,
            wanted.args
        );
    }
    if let Some(new) = new {
        let parent = file.parent().ok_or("no parent directory")?;
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        // Renamed into place, so the compositor never reads half a file.
        let temp = parent.join(".custom.windup");
        fs::write(&temp, new).map_err(|e| format!("{}: {e}", temp.display()))?;
        fs::rename(&temp, &file).map_err(|e| format!("{}: {e}", file.display()))?;
    }
    Ok(())
}
