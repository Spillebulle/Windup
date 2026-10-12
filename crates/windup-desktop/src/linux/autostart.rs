//! "Launch at login": an XDG autostart entry, which every desktop that has a
//! session (and `dex`, for window managers that do not) reads at sign-in.

use std::path::PathBuf;

pub const APP_ID: &str = "io.github.spillebulle.windup";

fn entry_path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(config.join("autostart").join(format!("{APP_ID}.desktop")))
}

/// What to run at sign-in. An AppImage's own path, not the mount it runs
/// from, which is gone after logout; a packaged install by name.
fn command() -> String {
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        return quote(&appimage.to_string_lossy());
    }
    match std::env::current_exe() {
        Ok(exe) if !exe.starts_with("/usr/") => quote(&exe.to_string_lossy()),
        _ => "windup".to_owned(),
    }
}

/// Desktop-entry Exec quoting.
fn quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c))
    {
        s.to_owned()
    } else {
        let escaped: String = s
            .chars()
            .flat_map(|c| match c {
                '"' | '`' | '$' | '\\' => vec!['\\', c],
                c => vec![c],
            })
            .collect();
        format!("\"{escaped}\"")
    }
}

pub fn entry(command: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Windup\n\
         Comment=Clipboard history and plain-text paste\n\
         Exec={command} --background\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n"
    )
}

/// Makes the autostart entry match the setting.
pub fn sync(enabled: bool) {
    let Some(path) = entry_path() else {
        return;
    };
    let result = if enabled {
        path.parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .and_then(|_| std::fs::write(&path, entry(&command())))
    } else if path.exists() {
        std::fs::remove_file(&path)
    } else {
        Ok(())
    };
    if let Err(e) = result {
        log::warn!("autostart entry {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_starts_in_the_background() {
        let e = entry("windup");
        assert!(e.starts_with("[Desktop Entry]\n"));
        assert!(e.contains("\nExec=windup --background\n"));
        assert!(e.contains("\nIcon=io.github.spillebulle.windup\n"));
    }

    #[test]
    fn paths_with_spaces_are_quoted() {
        assert_eq!(quote("/home/a/Apps/windup"), "/home/a/Apps/windup");
        assert_eq!(
            quote("/home/a/My Apps/Windup.AppImage"),
            "\"/home/a/My Apps/Windup.AppImage\""
        );
        assert_eq!(quote("/x/$y"), "\"/x/\\$y\"");
    }
}
