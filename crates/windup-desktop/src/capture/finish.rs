//! What happens to the picture once a capture ends, on every host: copy it,
//! save it (never over an existing file), and remember the session's
//! choices for the next capture. The viewport host (`overlay.rs`) and the
//! Wayland hosts (`linux::capture`) both end here; only the clipboard they
//! copy to differs, so that is passed in.

use image::RgbaImage;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use windup_core::capture::output;
use windup_core::capture::session::{CaptureMode, Outcome, Session};
use windup_core::config::{ScreenshotConfig, VideoConfig};

/// How many taken names a save steps past before it gives up.
pub(crate) const SAVE_TRIES: usize = 16;

/// The notice's text when the picture would not go on the clipboard.
pub(crate) const COPY_FAILED: &str = "The screenshot could not be copied to the clipboard.";

/// What an outcome asks for: (copy, save).
pub(crate) fn wants(outcome: &Outcome) -> (bool, bool) {
    match outcome {
        Outcome::Copy(_) => (true, false),
        Outcome::Save(_) => (false, true),
        Outcome::CopyAndSave(_) => (true, true),
        Outcome::Record { .. } | Outcome::Cancel => (false, false),
    }
}

/// What the next capture starts with: this one's target, colour and sizes.
pub(crate) fn remember(cfg: &mut ScreenshotConfig, session: &Session) {
    cfg.last_target = session.target_of(CaptureMode::Screenshot);
    cfg.colour = session.colour;
    cfg.sizes = session.sizes.clone();
}

/// What the next recording starts with, once one is asked for: the bar's
/// choices as picked (a sound toggle that could not apply this time keeps
/// its setting) and Video's target.
pub(crate) fn remember_video(cfg: &mut VideoConfig, session: &Session) {
    let v = &session.video;
    cfg.fps = v.fps;
    cfg.bitrate = v.bitrate;
    cfg.gif_colours = v.gif_colours;
    cfg.system_audio = v.system_audio;
    cfg.mic = v.mic;
    cfg.last_target = session.target_of(CaptureMode::Video);
}

/// Writes `bytes` to a new file in `dir`, named by `pick(dir, name)`. The file
/// is created with `create_new`, so an existing file is never overwritten:
/// when the name was taken between picking and creating, `pick` is asked
/// again, at most [`SAVE_TRIES`] times.
pub(crate) fn write_new(
    dir: &Path,
    name: &str,
    bytes: &[u8],
    mut pick: impl FnMut(&Path, &str) -> PathBuf,
) -> io::Result<PathBuf> {
    for _ in 0..SAVE_TRIES {
        let path = pick(dir, name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                if let Err(e) = file.write_all(bytes).and_then(|()| file.sync_all()) {
                    drop(file);
                    let _ = std::fs::remove_file(&path);
                    return Err(e);
                }
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "every free name was taken before it could be used",
    ))
}

/// Encodes `img` and saves it where the config says, creating the folder.
/// The error is the text for the notice.
pub(crate) fn save_image(img: &RgbaImage, cfg: &ScreenshotConfig) -> Result<PathBuf, String> {
    let dir =
        cfg.save_dir.clone().or_else(output::default_dir).ok_or(
            "The screenshot could not be saved: there is no Pictures folder to save it in.",
        )?;
    std::fs::create_dir_all(&dir).map_err(|e| {
        format!(
            "The screenshot could not be saved: the folder {} could not be made ({e}).",
            dir.display()
        )
    })?;
    let bytes = output::encode(img, cfg.format)
        .map_err(|e| format!("The screenshot could not be encoded ({e})."))?;
    let name = output::file_name_now(&cfg.name_pattern, cfg.format);
    write_new(&dir, &name, &bytes, output::unique_path).map_err(|e| {
        format!(
            "The screenshot could not be saved in {} ({e}).",
            dir.display()
        )
    })
}

/// Saves and copies the picture `outcome` carries, as it asks. `copy` puts a
/// picture on the host's clipboard and says whether it took. The text for
/// the notice when any part failed, one line per failure.
pub(crate) fn deliver(
    outcome: Outcome,
    cfg: &ScreenshotConfig,
    copy: impl FnOnce(RgbaImage) -> bool,
) -> Option<String> {
    let (want_copy, want_save) = wants(&outcome);
    let img = match outcome {
        Outcome::Copy(img) | Outcome::Save(img) | Outcome::CopyAndSave(img) => img,
        Outcome::Record { .. } | Outcome::Cancel => return None,
    };
    let mut errors = Vec::new();
    if want_save {
        match save_image(&img, cfg) {
            Ok(path) => log::debug!("capture: saved {}", path.display()),
            Err(e) => errors.push(e),
        }
    }
    if want_copy && !copy(img) {
        errors.push(COPY_FAILED.to_owned());
    }
    (!errors.is_empty()).then(|| {
        errors.join(
            "
",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use windup_core::capture::DesktopShot;
    use windup_core::capture::session::{Action, ModeAvailability};
    use windup_core::config::{ImageFormat, TargetKind, ToolSizes};
    use windup_core::video::Bitrate;

    /// A fresh, empty folder under the temp dir.
    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("windup-finish-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn saving_to(dir: &Path) -> ScreenshotConfig {
        ScreenshotConfig {
            save_dir: Some(dir.to_path_buf()),
            name_pattern: "shot".into(),
            format: ImageFormat::Png,
            ..ScreenshotConfig::default()
        }
    }

    fn files(dir: &Path) -> usize {
        std::fs::read_dir(dir).map_or(0, |d| d.count())
    }

    #[test]
    fn outcomes_map_to_copy_and_save() {
        let img = || RgbaImage::new(1, 1);
        assert_eq!(wants(&Outcome::Copy(img())), (true, false));
        assert_eq!(wants(&Outcome::Save(img())), (false, true));
        assert_eq!(wants(&Outcome::CopyAndSave(img())), (true, true));
        assert_eq!(wants(&Outcome::Cancel), (false, false));
    }

    #[test]
    fn the_session_choices_are_remembered() {
        let mut cfg = ScreenshotConfig::default();
        let mut s = Session::new(
            DesktopShot { monitors: vec![] },
            vec![],
            &cfg,
            &VideoConfig::default(),
            ModeAvailability::ALL,
        );
        s.target = TargetKind::Window;
        s.colour = [1, 2, 3, 255];
        s.sizes = ToolSizes {
            pen: 9.0,
            ..ToolSizes::default()
        };
        remember(&mut cfg, &s);
        assert_eq!(cfg.last_target, TargetKind::Window);
        assert_eq!(cfg.colour, [1, 2, 3, 255]);
        assert_eq!(cfg.sizes.pen, 9.0);
    }

    #[test]
    fn the_video_choices_are_remembered_and_the_targets_kept_apart() {
        let mut shots = ScreenshotConfig::default();
        let mut video = VideoConfig {
            last_target: TargetKind::FullScreen,
            ..VideoConfig::default()
        };
        let mut s = Session::new(
            DesktopShot { monitors: vec![] },
            vec![],
            &shots,
            &video,
            ModeAvailability::ALL,
        );
        s.act(Action::SetMode(CaptureMode::Video));
        s.act(Action::SetTarget(TargetKind::Window));
        s.act(Action::SetFps(60));
        s.act(Action::SetBitrate(Bitrate::Kbps(20000)));
        s.act(Action::ToggleMic);
        remember(&mut shots, &s);
        remember_video(&mut video, &s);
        assert_eq!(shots.last_target, TargetKind::Area, "the screenshot's own");
        assert_eq!(video.last_target, TargetKind::Window);
        assert_eq!(video.fps, 60);
        assert_eq!(video.bitrate, Bitrate::Kbps(20000));
        assert!(video.mic && !video.system_audio);
    }

    #[test]
    fn a_name_taken_after_it_was_picked_moves_on_to_the_next() {
        let dir = temp("race");
        // Another process got there between picking and creating.
        std::fs::write(dir.join("a.png"), b"theirs").unwrap();
        let mut calls = 0;
        let path = write_new(&dir, "a.png", b"ours", |d, n| {
            calls += 1;
            if calls == 1 {
                d.join(n)
            } else {
                output::unique_path(d, n)
            }
        })
        .unwrap();
        assert_eq!(path, dir.join("a (2).png"));
        assert_eq!(std::fs::read(&path).unwrap(), b"ours");
        assert_eq!(std::fs::read(dir.join("a.png")).unwrap(), b"theirs");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_name_that_stays_taken_gives_up_without_overwriting() {
        let dir = temp("taken");
        std::fs::write(dir.join("a.png"), b"theirs").unwrap();
        let mut calls = 0;
        let e = write_new(&dir, "a.png", b"ours", |d, n| {
            calls += 1;
            d.join(n)
        })
        .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(calls, SAVE_TRIES);
        assert_eq!(std::fs::read(dir.join("a.png")).unwrap(), b"theirs");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saving_makes_the_folder_and_writes_a_picture_of_the_selection() {
        let dir = temp("save");
        let cfg = saving_to(&dir.join("deeper").join("still"));
        let img = RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255]));
        let first = save_image(&img, &cfg).unwrap();
        let second = save_image(&img, &cfg).unwrap();
        assert_eq!(first, dir.join("deeper").join("still").join("shot.png"));
        assert_eq!(
            second,
            dir.join("deeper").join("still").join("shot (2).png")
        );
        let back = image::open(&first).unwrap().to_rgba8();
        assert_eq!(back, img);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_that_cannot_be_made_is_an_error_for_the_notice() {
        let dir = temp("blocked");
        // A file where the folder should be.
        std::fs::write(dir.join("file"), b"x").unwrap();
        let cfg = ScreenshotConfig {
            save_dir: Some(dir.join("file").join("sub")),
            ..ScreenshotConfig::default()
        };
        let e = save_image(&RgbaImage::new(1, 1), &cfg).unwrap_err();
        assert!(e.starts_with("The screenshot could not be saved"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_copies_and_does_not_save() {
        let dir = temp("deliver-copy");
        let img = RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]));
        let mut got = None;
        let e = deliver(Outcome::Copy(img.clone()), &saving_to(&dir), |i| {
            got = Some(i);
            true
        });
        assert_eq!(e, None);
        assert_eq!(got, Some(img));
        assert_eq!(files(&dir), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_saves_and_does_not_copy() {
        let dir = temp("deliver-save");
        let mut copied = false;
        let e = deliver(
            Outcome::Save(RgbaImage::new(2, 2)),
            &saving_to(&dir),
            |_| {
                copied = true;
                true
            },
        );
        assert_eq!(e, None);
        assert!(!copied);
        assert!(dir.join("shot.png").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn both_failures_are_told_one_per_line_and_cancel_does_nothing() {
        let dir = temp("deliver-fail");
        std::fs::write(dir.join("file"), b"x").unwrap();
        let cfg = saving_to(&dir.join("file").join("sub"));
        let e = deliver(Outcome::CopyAndSave(RgbaImage::new(1, 1)), &cfg, |_| false).unwrap();
        let lines: Vec<&str> = e.lines().collect();
        assert_eq!(lines.len(), 2, "{e}");
        assert!(
            lines[0].starts_with("The screenshot could not be saved"),
            "{e}"
        );
        assert_eq!(lines[1], COPY_FAILED);

        let mut called = false;
        let none = deliver(Outcome::Cancel, &cfg, |_| {
            called = true;
            true
        });
        assert_eq!(none, None);
        assert!(!called);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
