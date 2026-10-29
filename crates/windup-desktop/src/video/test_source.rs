//! What `--record-test` records: the real screen through the frame source this
//! desktop uses, or with `--pattern` a generated picture, so pipe tests never
//! read the screen.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windup_core::capture::geometry::DRect;
use windup_core::config::VideoConfig;
use windup_core::ipc::RecordTest;
use windup_core::video::{Source, even_size};

use super::ffmpeg;
use super::run::{AudioSource, FrameSource, RecordJob, Recording, SourceFactory};

/// A moving colour gradient: what `--record-test` records, so a test run
/// never reads the screen.
pub(crate) struct Pattern {
    pub(crate) w: u32,
    pub(crate) h: u32,
    pub(crate) t0: Instant,
}

impl FrameSource for Pattern {
    fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    fn next_frame(&mut self, buf: &mut Vec<u8>) -> Result<bool, String> {
        let (w, h) = (self.w as usize, self.h as usize);
        buf.resize(w * h * 4, 0);
        let shift = (self.t0.elapsed().as_secs_f64() * 120.0) as usize;
        for (i, px) in buf.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (x, y) = (i % w, i / w);
            px[0] = ((x * 255 / w.max(1) + shift) % 256) as u8;
            px[1] = (y * 255 / h.max(1)) as u8;
            px[2] = (shift % 256) as u8;
            px[3] = 255;
        }
        Ok(true)
    }
}

pub(crate) struct PatternFactory {
    pub(crate) w: u32,
    pub(crate) h: u32,
}

impl SourceFactory for PatternFactory {
    fn source(&self) -> Source {
        Source::RawPipe {
            path: String::new(),
            w: self.w,
            h: self.h,
        }
    }

    fn open_pipe(&self) -> Result<Box<dyn FrameSource>, String> {
        Ok(Box::new(Pattern {
            w: self.w,
            h: self.h,
            t0: Instant::now(),
        }))
    }
}

/// The frame source for the real screen on this desktop, for `area` (desktop
/// units; the primary monitor's top-left 320x240 without one). The choice
/// is the recording's own (`live::screen_factory`); a test never passes a
/// ScreenCast token, so it neither spends the owner's grant (each token is
/// good once) nor saves a new one: the portal asks.
fn screen_factory(area: Option<(i32, i32, u32, u32)>) -> Result<Box<dyn SourceFactory>, String> {
    let rect = match area {
        Some((x, y, w, h)) => DRect {
            x: x as f32,
            y: y as f32,
            w: w as f32,
            h: h as f32,
        },
        None => {
            let (x, y) = primary_origin()?;
            DRect {
                x,
                y,
                w: 320.0,
                h: 240.0,
            }
        }
    };
    super::live::screen_factory(rect, None).map(|picked| picked.factory)
}

/// The primary monitor's top-left: the one holding the origin on Windows,
/// the origin itself elsewhere.
#[cfg(target_os = "windows")]
fn primary_origin() -> Result<(f32, f32), String> {
    let layouts: Vec<DRect> = crate::capture::windows::monitor_list()
        .into_iter()
        .map(|(r, _)| r)
        .collect();
    let primary = layouts
        .iter()
        .find(|m| m.contains(0.0, 0.0))
        .or(layouts.first())
        .ok_or("no monitors found")?;
    Ok((primary.x, primary.y))
}

#[cfg(not(target_os = "windows"))]
fn primary_origin() -> Result<(f32, f32), String> {
    Ok((0.0, 0.0))
}

type Opener = fn(Option<&str>) -> Result<Box<dyn AudioSource>, String>;

/// `windup --record-test <out> --seconds N [--area x,y,w,h] [--pattern]
/// [--system-audio] [--mic]`: records the screen area (a pattern of its size
/// with `--pattern`), with the default devices' sound if asked, through the
/// real pipeline and writes it to `<out>`. Exit 0 when the file is there and
/// not empty, 1 when the recording failed, 2 without ffmpeg, with a bad size,
/// or with no screen or sound source.
pub(crate) fn record_test(test: &RecordTest) -> i32 {
    if ffmpeg::locate().is_none() {
        eprintln!("windup: no ffmpeg found (set WINDUP_FFMPEG)");
        return 2;
    }
    let (w, h) = test.area.map_or((320, 240), |(_, _, w, h)| (w, h));
    let Some((w, h)) = even_size(w, h) else {
        eprintln!("windup: the area is too small to record");
        return 2;
    };
    let factory: Box<dyn SourceFactory> = if test.pattern {
        Box::new(PatternFactory { w, h })
    } else {
        match screen_factory(test.area) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("windup: {e}");
                return 2;
            }
        }
    };
    let mut audio = Vec::new();
    for (wanted, what, open) in [
        (
            test.system_audio,
            "computer sound",
            super::audio::open_system as Opener,
        ),
        (test.mic, "the microphone", super::audio::open_mic),
    ] {
        if !wanted {
            continue;
        }
        match open(None) {
            Ok(src) => audio.push(src),
            Err(e) => {
                eprintln!("windup: {what}: {e}");
                return 2;
            }
        }
    }
    let mut cfg = VideoConfig::default();
    if let Some(c) = test.container {
        cfg.container = c;
    }
    cfg.name_pattern = format!("windup-record-test-{}", std::process::id());
    let out_dir = test
        .out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let job = RecordJob {
        factory,
        audio,
        cfg,
        size: (w, h),
        out_dir,
    };
    let mut rec = match Recording::start(job) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("windup: {e}");
            return 1;
        }
    };
    std::thread::sleep(Duration::from_secs(u64::from(test.seconds)));
    if let Some(why) = rec.failed() {
        eprintln!("windup: {why}");
    }
    let saved = match rec.stop().join() {
        Ok(Ok(path)) => path,
        Ok(Err(e)) => {
            eprintln!("windup: {e}");
            return 1;
        }
        Err(_) => return 1,
    };
    if let Err(e) = std::fs::rename(&saved, &test.out) {
        eprintln!("windup: could not move {} into place: {e}", saved.display());
        let _ = std::fs::remove_file(&saved);
        return 1;
    }
    if std::fs::metadata(&test.out).is_ok_and(|m| m.len() > 0) {
        println!("{}", test.out.display());
        0
    } else {
        1
    }
}
