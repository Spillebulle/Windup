//! What the notice says when a recording ends: the file it was saved to, or
//! why there is none. The runtime's own texts (`run.rs`) carry ffmpeg's last
//! line, which is for the log; the user gets one of the sentences here.

use std::path::{Path, PathBuf};

/// Why a screencopy stream ends: the output it read went away.
pub(crate) const MONITOR_GONE: &str = "A monitor was disconnected.";
/// Why a ScreenCast stream ends: the desktop stopped the sharing.
pub(crate) const SHARING_STOPPED: &str = "The screen sharing was stopped.";
pub(crate) const ENCODER_FAILED: &str = "Recording stopped because the encoder failed.";
pub(crate) const NOTHING: &str = "Nothing was recorded.";
pub(crate) const SOUND_LOST: &str = "The sound device was lost during the recording.";
pub(crate) const NO_MIC: &str =
    "The microphone could not be opened, so the recording has no microphone sound.";
pub(crate) const NO_SYSTEM_SOUND: &str =
    "Computer sound could not be opened, so the recording has no computer sound.";
pub(crate) const NOT_SAVED: &str = "The recording could not be saved.";
pub(crate) const NOT_STARTED: &str = "The recording could not start.";
/// Esc during a recording: it is thrown away.
pub(crate) const CANCELLED: &str = "The recording was cancelled.";
pub(crate) const TOO_SMALL: &str = "The area is too small to record.";
pub(crate) const NO_FOLDER: &str = "There is no Videos folder to save the recording in.";
/// The area is not wholly on the screens the ScreenCast grant shares. The
/// grant is forgotten, so the next recording asks again.
pub(crate) const SCREENS_NOT_SHARED: &str =
    "Windup can only record the screens you shared. Record again to choose them.";
/// Why a segment would not start: its picture is not segment 0's size (a
/// Wayland scale or layout change during a pause). For the log; the notice
/// only says what was kept.
pub(crate) const SIZE_CHANGED: &str = "The recorded area changed size.";
/// The notice's button when there is a file to show.
pub(crate) const OPEN_FOLDER: &str = "Open folder";

/// The runtime's prefix on a segment's failure (`Run::check`).
const STOPPED_PREFIX: &str = "Recording stopped: ";

/// What the user is told about `Recording::failed`'s text: a stream's own
/// reason verbatim, anything else (ffmpeg's last line) as the encoder.
/// `None` when the line about what was kept says enough.
pub(crate) fn failure_text(raw: &str) -> Option<&'static str> {
    let why = raw.strip_prefix(STOPPED_PREFIX).unwrap_or(raw).trim();
    if why == SIZE_CHANGED {
        return None;
    }
    Some(
        [MONITOR_GONE, SHARING_STOPPED, SCREENS_NOT_SHARED]
            .into_iter()
            .find(|known| why == *known)
            .unwrap_or(ENCODER_FAILED),
    )
}

/// What a recording that could not start says. The size is the user's to
/// fix; the rest is for the log.
pub(crate) fn start_text(raw: &str) -> &'static str {
    match raw.trim() {
        TOO_SMALL => TOO_SMALL,
        NO_FOLDER => NO_FOLDER,
        SCREENS_NOT_SHARED => SCREENS_NOT_SHARED,
        _ => NOT_STARTED,
    }
}

/// The sound sources asked for that would not open when the recording
/// started; it went on without them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Unopened {
    pub system: bool,
    pub mic: bool,
}

/// The end of a recording, for the notice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ended {
    pub message: String,
    /// The saved file, for "Open folder".
    pub saved: Option<PathBuf>,
}

fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// The notice for a stopped recording: the join's `result`, the failure the
/// recording ended on if any (`Recording::failed`, raw), whether a sound
/// device went away, and which never opened.
pub(crate) fn ended(
    result: &Result<PathBuf, String>,
    failed: Option<&str>,
    sound_lost: bool,
    unopened: Unopened,
) -> Ended {
    let mut lines: Vec<String> = Vec::new();
    if let Some(line) = failed.and_then(failure_text) {
        lines.push(line.to_owned());
    }
    match (result, failed.is_some()) {
        (Ok(path), false) => lines.push(format!("Saved to {}", name_of(path))),
        (Ok(path), true) => lines.push(format!(
            "Saved what was recorded before the error to {}.",
            name_of(path)
        )),
        (Err(e), _) if e.trim() == NOTHING => lines.push(NOTHING.to_owned()),
        (Err(_), _) => lines.push(NOT_SAVED.to_owned()),
    }
    if unopened.system {
        lines.push(NO_SYSTEM_SOUND.to_owned());
    }
    if unopened.mic {
        lines.push(NO_MIC.to_owned());
    }
    if sound_lost {
        lines.push(SOUND_LOST.to_owned());
    }
    Ended {
        message: lines.join("\n"),
        saved: result.as_ref().ok().cloned(),
    }
}

/// Shows `file` in the file manager: selected in Explorer on Windows, its
/// folder opened by `xdg-open` on Linux. Never waits for it.
pub(crate) fn open_folder(file: &Path) {
    #[cfg(target_os = "windows")]
    let command = {
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("explorer");
        // Explorer reads its own command line: the path is quoted inside the
        // one argument, which std would otherwise quote as a whole.
        c.raw_arg(format!("/select,\"{}\"", file.display()));
        Some(c)
    };
    #[cfg(target_os = "linux")]
    let command = file.parent().map(|dir| {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(dir);
        c
    });
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    let command: Option<std::process::Command> = {
        let _ = file;
        None
    };
    let Some(mut command) = command else {
        return;
    };
    // Waited for on a thread of its own, so no zombie is left behind.
    let spawned = std::thread::Builder::new()
        .name("open-folder".into())
        .spawn(move || match command.status() {
            Ok(_) => {}
            Err(e) => log::warn!("could not open the folder: {e}"),
        });
    if let Err(e) = spawned {
        log::warn!("could not open the folder: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved() -> Result<PathBuf, String> {
        Ok(Path::new("videos").join("Recording 2026-10-29 101500.mp4"))
    }

    #[test]
    fn a_clean_stop_says_where_it_went_and_offers_the_folder() {
        let e = ended(&saved(), None, false, Unopened::default());
        assert_eq!(e.message, "Saved to Recording 2026-10-29 101500.mp4");
        assert_eq!(e.saved, saved().ok());
    }

    #[test]
    fn an_encoder_failure_keeps_ffmpegs_line_out_of_the_notice() {
        let raw = "Recording stopped: [h264_nvenc @ 0x1] OpenEncodeSessionEx failed: out of memory";
        assert_eq!(failure_text(raw), Some(ENCODER_FAILED));
        let e = ended(&saved(), Some(raw), false, Unopened::default());
        assert_eq!(
            e.message,
            "Recording stopped because the encoder failed.\n\
             Saved what was recorded before the error to Recording 2026-10-29 101500.mp4."
        );
        assert!(!e.message.contains("nvenc"));
        assert!(e.saved.is_some());
    }

    #[test]
    fn a_stream_says_its_own_reason() {
        for why in [MONITOR_GONE, SHARING_STOPPED] {
            let raw = format!("Recording stopped: {why}");
            assert_eq!(failure_text(&raw), Some(why));
            assert_eq!(failure_text(why), Some(why));
        }
    }

    #[test]
    fn a_failure_with_nothing_kept_says_nothing_was_recorded() {
        let e = ended(
            &Err("Nothing was recorded.".into()),
            Some("Recording stopped: A monitor was disconnected."),
            false,
            Unopened::default(),
        );
        assert_eq!(
            e.message,
            "A monitor was disconnected.\nNothing was recorded."
        );
        assert_eq!(e.saved, None);
        let e = ended(
            &Err("Nothing was recorded.".into()),
            None,
            false,
            Unopened::default(),
        );
        assert_eq!(e.message, "Nothing was recorded.");
    }

    #[test]
    fn a_join_that_failed_says_it_could_not_be_saved() {
        let e = ended(
            &Err("The recording could not be saved: Invalid data found".into()),
            None,
            false,
            Unopened::default(),
        );
        assert_eq!(e.message, NOT_SAVED);
        assert_eq!(e.saved, None);
    }

    #[test]
    fn a_lost_sound_device_adds_a_line() {
        let e = ended(&saved(), None, true, Unopened::default());
        assert_eq!(
            e.message,
            "Saved to Recording 2026-10-29 101500.mp4\n\
             The sound device was lost during the recording."
        );
    }

    #[test]
    fn a_sound_that_never_opened_gets_a_line_each() {
        let both = Unopened {
            system: true,
            mic: true,
        };
        let e = ended(&saved(), None, false, both);
        assert_eq!(
            e.message,
            "Saved to Recording 2026-10-29 101500.mp4\n\
             Computer sound could not be opened, so the recording has no computer sound.\n\
             The microphone could not be opened, so the recording has no microphone sound."
        );
        let mic = Unopened {
            mic: true,
            ..Unopened::default()
        };
        let e = ended(&saved(), None, false, mic);
        assert!(e.message.ends_with(NO_MIC));
        assert!(!e.message.contains(NO_SYSTEM_SOUND));
    }

    #[test]
    fn screens_not_shared_says_how_to_choose_them() {
        assert_eq!(
            SCREENS_NOT_SHARED,
            "Windup can only record the screens you shared. Record again to choose them."
        );
        assert_eq!(start_text(SCREENS_NOT_SHARED), SCREENS_NOT_SHARED);
        let raw = format!("Recording stopped: {SCREENS_NOT_SHARED}");
        assert_eq!(failure_text(&raw), Some(SCREENS_NOT_SHARED));
    }

    #[test]
    fn a_size_change_only_says_what_was_kept() {
        let raw = format!("Recording stopped: {SIZE_CHANGED}");
        assert_eq!(failure_text(&raw), None);
        let e = ended(&saved(), Some(&raw), false, Unopened::default());
        assert_eq!(
            e.message,
            "Saved what was recorded before the error to Recording 2026-10-29 101500.mp4."
        );
    }

    #[test]
    fn a_start_failure_says_the_size_or_that_it_could_not_start() {
        assert_eq!(start_text("The area is too small to record."), TOO_SMALL);
        assert_eq!(start_text("Could not start ffmpeg: denied"), NOT_STARTED);
        assert_eq!(start_text(NO_FOLDER), NO_FOLDER);
    }

    #[test]
    fn the_copy_is_british_sentence_case_without_dashes() {
        for s in [
            MONITOR_GONE,
            SHARING_STOPPED,
            ENCODER_FAILED,
            NOTHING,
            SOUND_LOST,
            NOT_SAVED,
            NOT_STARTED,
            TOO_SMALL,
            NO_FOLDER,
            OPEN_FOLDER,
            SCREENS_NOT_SHARED,
        ] {
            assert!(!s.contains('\u{2014}') && !s.contains(" - "), "{s}");
            assert!(s.chars().next().is_some_and(char::is_uppercase), "{s}");
        }
    }
}
