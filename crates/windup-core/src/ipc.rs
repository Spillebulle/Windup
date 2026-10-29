//! The commands one Windup process sends another.
//!
//! On Linux a desktop shortcut runs `windup --popup`, which connects to the
//! running Windup's socket, writes one line and exits. The same socket makes
//! Windup single-instance: a second plain `windup` sends `show` instead of
//! starting over. This module is the protocol and the argument parsing; the
//! socket itself is in `windup-desktop`.

use crate::hotkeys::Action;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Open the clipboard history popup.
    Popup,
    /// Paste the clipboard as plain text into the focused window.
    PastePlain,
    /// Open the capture launcher.
    Capture,
    /// Take a screenshot.
    Screenshot,
    /// Start or stop a screen recording.
    Record,
    /// Bring up the settings window.
    Show,
    /// Re-read `config.json`; sent by the settings window after it saves.
    Reload,
}

impl Command {
    pub const ALL: [Command; 7] = [
        Command::Popup,
        Command::PastePlain,
        Command::Capture,
        Command::Screenshot,
        Command::Record,
        Command::Show,
        Command::Reload,
    ];

    /// The word on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Command::Popup => "popup",
            Command::PastePlain => "paste-plain",
            Command::Capture => "capture",
            Command::Screenshot => "screenshot",
            Command::Record => "record",
            Command::Show => "show",
            Command::Reload => "reload",
        }
    }

    /// The command that runs a hotkey action.
    pub fn for_action(action: Action) -> Command {
        match action {
            Action::ClipboardPopup => Command::Popup,
            Action::PastePlain => Command::PastePlain,
            Action::CaptureLauncher => Command::Capture,
            Action::Screenshot => Command::Screenshot,
            // The recording keys are only grabbed while recording, and only
            // where Windup can grab keys at all; a command stands in for them.
            Action::Video | Action::RecordPause | Action::RecordStop | Action::RecordCancel => {
                Command::Record
            }
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
    /// `--capture-test <file.png>`: grab every monitor, write the picture,
    /// exit. A development aid, so it is not in `USAGE`.
    CaptureTest(PathBuf),
    /// `--window-rects-test`: print how many window rectangles the desktop
    /// reports, one line each, and exit. A test aid, so not in `USAGE`.
    WindowRectsTest,
    /// `--encoders-test`: probe the bundled ffmpeg's encoders, print one line
    /// each, exit. A test aid, so not in `USAGE`.
    EncodersTest,
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
  --capture       Open the capture launcher
  --screenshot    Take a screenshot
  --record        Start or stop a screen recording
  --background    Start without opening the window
  --help          Show this text

Bind --popup, --paste-plain, --capture, --screenshot and --record to keys in your desktop's keyboard settings
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
            "--capture" => Launch::Send(Command::Capture),
            "--screenshot" => Launch::Send(Command::Screenshot),
            "--record" => Launch::Send(Command::Record),
            "--show" => Launch::Send(Command::Show),
            "--background" => Launch::Background,
            "--settings" => Launch::Settings,
            "--window-rects-test" => Launch::WindowRectsTest,
            "--encoders-test" => Launch::EncodersTest,
            "--capture-test" => Launch::Unknown("--capture-test needs a file name".into()),
            "--help" | "-h" => Launch::Help,
            other => Launch::Unknown(format!("unknown option '{other}'")),
        },
        [flag, path] if flag == "--capture-test" => Launch::CaptureTest(PathBuf::from(path)),
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

/// Where the new executable is when the one this process runs from has been
/// replaced, from the target of `/proc/self/exe`. The kernel adds
/// " (deleted)" once that file is gone, which is what a package update does:
/// it puts a new file in its place and unlinks the old one.
pub fn replaced_exe(link: &str) -> Option<PathBuf> {
    link.strip_suffix(" (deleted)")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
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
    fn a_replaced_executable_is_recognised_by_its_link() {
        assert_eq!(
            replaced_exe("/usr/bin/windup (deleted)"),
            Some(PathBuf::from("/usr/bin/windup"))
        );
        assert_eq!(replaced_exe("/usr/bin/windup"), None);
        assert_eq!(replaced_exe(" (deleted)"), None);
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
    fn capture_flags_parse() {
        assert_eq!(parse_args(["--capture"]), Launch::Send(Command::Capture));
        assert_eq!(
            parse_args(["--screenshot"]),
            Launch::Send(Command::Screenshot)
        );
        assert!(USAGE.contains("--capture") && USAGE.contains("--screenshot"));
        assert_eq!(parse_args(["--record"]), Launch::Send(Command::Record));
        assert!(USAGE.contains("--record"));
    }

    #[test]
    fn encoders_test_is_a_hidden_flag() {
        assert_eq!(parse_args(["--encoders-test"]), Launch::EncodersTest);
        assert!(!USAGE.contains("encoders-test"));
    }

    #[test]
    fn window_rects_test_is_a_hidden_flag() {
        assert_eq!(parse_args(["--window-rects-test"]), Launch::WindowRectsTest);
        assert!(matches!(
            parse_args(["--window-rects-test", "x"]),
            Launch::Unknown(_)
        ));
        assert!(!USAGE.contains("window-rects-test"));
    }

    #[test]
    fn capture_test_takes_a_path_and_is_hidden() {
        assert_eq!(
            parse_args(["--capture-test", "out.png"]),
            Launch::CaptureTest(PathBuf::from("out.png"))
        );
        assert!(matches!(parse_args(["--capture-test"]), Launch::Unknown(_)));
        assert!(!USAGE.contains("capture-test"));
    }

    #[test]
    fn actions_map_to_commands() {
        assert_eq!(Command::for_action(Action::ClipboardPopup), Command::Popup);
        assert_eq!(Command::for_action(Action::PastePlain), Command::PastePlain);
        assert_eq!(
            Command::for_action(Action::CaptureLauncher),
            Command::Capture
        );
        assert_eq!(Command::for_action(Action::Screenshot), Command::Screenshot);
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
