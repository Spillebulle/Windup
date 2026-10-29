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
use super::run::{FrameSource, RecordJob, Recording, SourceFactory};

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

    fn open_pipe(&self) -> Option<Box<dyn FrameSource>> {
        Some(Box::new(Pattern {
            w: self.w,
            h: self.h,
            t0: Instant::now(),
        }))
    }
}

/// The frame source for the real screen on this desktop, for `area` (desktop
/// units; the primary monitor's top-left 320x240 without one).
fn screen_factory(area: Option<(i32, i32, u32, u32)>) -> Result<Box<dyn SourceFactory>, String> {
    use windup_core::capture::backend::{Moving, video_order};
    let order = video_order(&crate::capture::facts());
    let default = |x: f32, y: f32| DRect {
        x,
        y,
        w: 320.0,
        h: 240.0,
    };
    let rect = |(x, y, w, h): (i32, i32, u32, u32)| DRect {
        x: x as f32,
        y: y as f32,
        w: w as f32,
        h: h as f32,
    };
    match order.first() {
        #[cfg(target_os = "windows")]
        Some(Moving::Ddagrab | Moving::Gdigrab) => {
            let monitors = crate::capture::windows::monitor_list();
            let layouts: Vec<DRect> = monitors.iter().map(|(r, _)| *r).collect();
            let primary = layouts
                .iter()
                .find(|m| m.contains(0.0, 0.0))
                .or(layouts.first())
                .ok_or("no monitors found")?;
            let rect = area.map_or_else(|| default(primary.x, primary.y), rect);
            let f = super::windows::WindowsFactory::new(rect, &layouts)
                .ok_or("the area is too small to record")?;
            Ok(Box::new(f))
        }
        #[cfg(target_os = "linux")]
        Some(Moving::X11grab) => {
            let rect = area.map_or_else(|| default(0.0, 0.0), rect);
            let f = super::x11::X11Factory::from_env(rect)
                .ok_or("DISPLAY is not set, or the area is too small to record")?;
            Ok(Box::new(f))
        }
        _ => {
            let _ = (default, rect);
            Err(format!(
                "this desktop has no screen source yet ({order:?}); use --pattern"
            ))
        }
    }
}

/// `windup --record-test <out> --seconds N [--area x,y,w,h] [--pattern]`:
/// records the screen area (a pattern of its size with `--pattern`) through
/// the real pipeline and writes it to `<out>`. Exit 0 when the file is there
/// and not empty, 1 when the recording failed, 2 without ffmpeg, with a bad
/// size, or with no screen source.
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
        audio: Vec::new(),
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
