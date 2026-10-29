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
//!
//! A recording's keys (pause, stop, Esc) go in the same file for as long as
//! it runs, tagged so the ones a crash leaves are removed at the next start.

use std::path::{Path, PathBuf};
use std::{fs, io};
use windup_core::Shortcut;
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

pub fn is_cosmic() -> bool {
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

/// The shortcuts file's text, `None` when there is none.
fn read(file: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(file) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", file.display())),
    }
}

/// Replaces the shortcuts file with `text`, renamed into place so the
/// compositor never reads half a file.
fn write(file: &Path, text: &str) -> Result<(), String> {
    let parent = file.parent().ok_or("no parent directory")?;
    fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let temp = parent.join(".custom.windup");
    fs::write(&temp, text).map_err(|e| format!("{}: {e}", temp.display()))?;
    fs::rename(&temp, file).map_err(|e| format!("{}: {e}", file.display()))
}

/// How a command names Windup: an AppImage is not on the PATH, so the
/// command names the file itself.
fn program() -> String {
    std::env::var("APPIMAGE")
        .ok()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "windup".into())
}

/// Binds a recording's keys for as long as it runs: `keys` is each shortcut
/// and the arguments it runs Windup with (`--record-cancel`). Entries a
/// crash left are replaced. False when nothing was added; the recording goes
/// on, with the bar and the commands.
pub fn add_temporary(keys: &[(Shortcut, &str)]) -> bool {
    let result = (|| -> Result<bool, String> {
        let file = shortcuts_file().ok_or("no home directory")?;
        let existing = read(&file)?;
        let unread = |e: cosmic_shortcuts::Unreadable| {
            format!("{} is left alone, it did not read: {}", file.display(), e.0)
        };
        let clean = match existing.as_deref() {
            Some(text) => cosmic_shortcuts::remove_temporary(text).map_err(unread)?,
            None => None,
        };
        let base = clean.as_deref().or(existing.as_deref());
        match cosmic_shortcuts::add_temporary(base, keys, &program()).map_err(unread)? {
            Some(new) => write(&file, &new).map(|()| true),
            None => {
                if let Some(clean) = clean {
                    write(&file, &clean)?;
                }
                Ok(false)
            }
        }
    })();
    match result {
        Ok(added) => {
            log::info!(
                "COSMIC: the recording's keys {}",
                if added { "bound" } else { "were all taken" }
            );
            added
        }
        Err(e) => {
            log::warn!("COSMIC: the recording's keys not bound: {e}");
            false
        }
    }
}

/// Takes the recording's keys out again, and any a crash left behind.
/// Entries of the user's own are never touched.
pub fn remove_temporary() {
    if !is_cosmic() {
        return;
    }
    let result = (|| -> Result<bool, String> {
        let file = shortcuts_file().ok_or("no home directory")?;
        let Some(text) = read(&file)? else {
            return Ok(false);
        };
        match cosmic_shortcuts::remove_temporary(&text) {
            Ok(Some(clean)) => write(&file, &clean).map(|()| true),
            Ok(None) => Ok(false),
            Err(e) => Err(format!(
                "{} is left alone, it did not read: {}",
                file.display(),
                e.0
            )),
        }
    })();
    match result {
        Ok(true) => log::info!("COSMIC: the recording's keys released"),
        Ok(false) => {}
        Err(e) => log::warn!("COSMIC: the recording's keys not released: {e}"),
    }
}

fn bind(wanted: &[Wanted]) -> Result<(), String> {
    let file = shortcuts_file().ok_or("no home directory")?;
    let existing = read(&file)?;
    let program = program();
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
        write(&file, &new)?;
    }
    Ok(())
}
