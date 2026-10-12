//! The commands one Windup process sends another.
//!
//! On Linux a desktop shortcut runs `windup --popup`, which connects to the
//! running Windup's socket, writes one line and exits. The same socket makes
//! Windup single-instance: a second plain `windup` sends `show` instead of
//! starting over. This module is the protocol and the argument parsing; the
//! socket itself is in `windup-desktop`.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Open the clipboard history popup.
    Popup,
    /// Paste the clipboard as plain text into the focused window.
    PastePlain,
    /// Bring up the settings window.
    Show,
    /// Re-read `config.json`; sent by the settings window after it saves.
    Reload,
}

impl Command {
    pub const ALL: [Command; 4] = [
        Command::Popup,
        Command::PastePlain,
        Command::Show,
        Command::Reload,
    ];

    /// The word on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Command::Popup => "popup",
            Command::PastePlain => "paste-plain",
            Command::Show => "show",
            Command::Reload => "reload",
        }
    }

    /// One line of the protocol, surrounding whitespace ignored.
    pub fn parse(line: &str) -> Option<Command> {
        let word = line.trim();
        Self::ALL.into_iter().find(|c| c.as_str() == word)
    }

    /// The line written to the socket.
    pub fn encode(self) -> String {
        format!("{}\n", self.as_str())
    }
}

/// What this launch of the program was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    /// The normal app: run, or hand `show` to the copy already running.
    App,
    /// Started at sign-in: run without opening the settings window.
    Background,
    /// Only the settings window, as a child of the running app.
    Settings,
    /// Deliver a command to the running app, starting it if there is none.
    Send(Command),
    /// `--help`.
    Help,
    /// Something we do not understand; the text says what.
    Unknown(String),
}

pub const USAGE: &str = "\
Usage: windup [OPTION]

  (none)          Open Windup, or its window if it is already running
  --popup         Open the clipboard history at the cursor
  --paste-plain   Paste the clipboard as plain text
  --background    Start without opening the window
  --help          Show this text

Bind --popup and --paste-plain to keys in your desktop's keyboard settings
where Windup cannot grab them itself (most Wayland desktops).";

/// Parses the arguments after the program name.
pub fn parse_args<I, S>(args: I) -> Launch
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<String> = args.into_iter().map(|s| s.as_ref().to_owned()).collect();
    match args.as_slice() {
        [] => Launch::App,
        [one] => match one.as_str() {
            "--popup" => Launch::Send(Command::Popup),
            "--paste-plain" => Launch::Send(Command::PastePlain),
            "--show" => Launch::Send(Command::Show),
            "--background" => Launch::Background,
            "--settings" => Launch::Settings,
            "--help" | "-h" => Launch::Help,
            other => Launch::Unknown(format!("unknown option '{other}'")),
        },
        _ => Launch::Unknown("expected at most one option".into()),
    }
}

/// Where the socket lives. `$XDG_RUNTIME_DIR` is per user, private and
/// cleared at logout, which is everything a socket wants; without it (a bare
/// X session, a container) a per-user directory under the temp dir.
pub fn socket_path(runtime_dir: Option<&str>, uid: u32, display: Option<&str>) -> PathBuf {
    // One Windup per display: two sessions of the same user (a nested
    // compositor, a second seat) must not answer for each other.
    let name = match display.filter(|d| !d.is_empty()) {
        Some(d) => format!("windup-{}.sock", sanitise(d)),
        None => "windup.sock".to_owned(),
    };
    match runtime_dir.filter(|d| !d.is_empty()) {
        Some(dir) => PathBuf::from(dir).join(name),
        None => std::env::temp_dir()
            .join(format!("windup-{uid}"))
            .join(name),
    }
}

fn sanitise(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_round_trips() {
        for c in Command::ALL {
            assert_eq!(Command::parse(&c.encode()), Some(c));
        }
    }

    #[test]
    fn junk_is_not_a_command() {
        assert_eq!(Command::parse(""), None);
        assert_eq!(Command::parse("POPUP"), None);
        assert_eq!(Command::parse("popup extra"), None);
    }

    #[test]
    fn arguments() {
        let none: [&str; 0] = [];
        assert_eq!(parse_args(none), Launch::App);
        assert_eq!(parse_args(["--popup"]), Launch::Send(Command::Popup));
        assert_eq!(
            parse_args(["--paste-plain"]),
            Launch::Send(Command::PastePlain)
        );
        assert_eq!(parse_args(["--show"]), Launch::Send(Command::Show));
        assert_eq!(parse_args(["--background"]), Launch::Background);
        assert_eq!(parse_args(["--settings"]), Launch::Settings);
        assert_eq!(parse_args(["-h"]), Launch::Help);
        assert!(matches!(parse_args(["--nope"]), Launch::Unknown(_)));
        assert!(matches!(
            parse_args(["--popup", "--show"]),
            Launch::Unknown(_)
        ));
    }

    #[test]
    fn socket_in_runtime_dir_named_for_the_display() {
        assert_eq!(
            socket_path(Some("/run/user/1000"), 1000, Some("wayland-1")),
            PathBuf::from("/run/user/1000/windup-wayland_1.sock")
        );
        assert_eq!(
            socket_path(Some("/run/user/1000"), 1000, Some(":0")),
            PathBuf::from("/run/user/1000/windup-_0.sock")
        );
        assert_eq!(
            socket_path(Some("/run/user/1000"), 1000, None),
            PathBuf::from("/run/user/1000/windup.sock")
        );
    }

    #[test]
    fn socket_without_runtime_dir_is_per_user() {
        let p = socket_path(None, 1234, None);
        assert!(p.ends_with("windup-1234/windup.sock"));
        assert_eq!(socket_path(Some(""), 1234, None), p);
    }

    #[test]
    fn display_cannot_escape_the_directory() {
        let p = socket_path(Some("/run/user/1"), 1, Some("../../etc/x"));
        assert_eq!(p.parent().unwrap(), std::path::Path::new("/run/user/1"));
    }
}
