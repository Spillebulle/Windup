//! Finding the bundled ffmpeg and asking it which encoders really work.
//!
//! The probe takes a second or two (a dozen ffmpeg runs), so it happens on a
//! thread at startup, every start, because hardware encoders depend on the GPU
//! and driver, not only on the ffmpeg file. The last result is cached next to
//! `config.json` and shown while the fresh probe runs. Anything that needs the
//! answer reads `PROBE`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use windup_core::config::Config;
use windup_core::video::{
    AudioCodec, Codec, Encoder, Probe, audio_encoder, audio_probe_args, cache_key, candidates,
    probe_args,
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const PARALLEL: usize = 4;

#[derive(Clone)]
pub(crate) enum ProbeState {
    Running,
    Done(Probe),
    NoFfmpeg,
}

/// `Running` until there is any answer: the cache for this ffmpeg, then the
/// fresh probe, which replaces it.
pub(crate) static PROBE: RwLock<ProbeState> = RwLock::new(ProbeState::Running);

fn publish(state: ProbeState) {
    if let Ok(mut g) = PROBE.write() {
        *g = state;
    }
}

#[derive(Serialize, Deserialize)]
struct Cached {
    key: String,
    probe: Probe,
}

/// Where ffmpeg is, trying in this order: `WINDUP_FFMPEG`, beside the exe,
/// `<exe>/../lib/windup/ffmpeg` (the Linux packages), and in debug builds the
/// `PATH`, so a developer needs no download. A release never picks up
/// whatever ffmpeg the user happens to have, whose encoders we have not tried.
pub(crate) fn locate() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("WINDUP_FFMPEG").map(PathBuf::from)
        && p.is_file()
    {
        return Some(p);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for name in ["ffmpeg.exe", "ffmpeg"] {
            let p = dir.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
        if let Some(up) = dir.parent() {
            let p = up.join("lib").join("windup").join("ffmpeg");
            if p.is_file() {
                return Some(p);
            }
        }
    }
    #[cfg(debug_assertions)]
    {
        on_path()
    }
    #[cfg(not(debug_assertions))]
    None
}

#[cfg(debug_assertions)]
fn on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|d| [d.join("ffmpeg.exe"), d.join("ffmpeg")])
        .find(|p| p.is_file())
}

pub(crate) fn command(ffmpeg: &Path) -> Command {
    let mut c = Command::new(ffmpeg);
    c.stdin(Stdio::null()).stdout(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

/// Runs ffmpeg with `args`; true if it exits 0 inside the timeout. What it
/// says on stderr goes to the debug log, which is how to learn why an encoder
/// failed.
fn passes(ffmpeg: &Path, name: &str, args: &[String]) -> bool {
    let mut child = match command(ffmpeg).args(args).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => {
            log::debug!("probe {name}: could not start ffmpeg: {e}");
            return false;
        }
    };
    // Read stderr on the side so a chatty failure cannot fill the pipe and
    // stall ffmpeg until the timeout.
    let stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut s) = stderr {
            let _ = std::io::Read::read_to_string(&mut s, &mut text);
        }
        text
    });
    let start = Instant::now();
    let ok = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if start.elapsed() < PROBE_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                log::debug!("probe {name}: timed out");
                break false;
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    if let Ok(text) = reader.join()
        && !ok
        && !text.trim().is_empty()
    {
        log::debug!("probe {name}: {}", text.trim());
    }
    ok
}

fn version(ffmpeg: &Path) -> String {
    command(ffmpeg)
        .arg("-version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .map(|l| l.trim().to_owned())
        })
        .unwrap_or_default()
}

const LINUX: bool = cfg!(target_os = "linux");
const CODECS: [Codec; 4] = [Codec::H264, Codec::H265, Codec::Av1, Codec::Vp9];

/// Every candidate, in the order `--encoders-test` prints them.
fn all_candidates() -> Vec<Encoder> {
    CODECS
        .iter()
        .flat_map(|c| candidates(*c, LINUX).iter().copied())
        .collect()
}

/// Runs every probe, four at a time. Returns the per-encoder results too.
pub(crate) fn run_probe(ffmpeg: &Path) -> (Probe, Vec<(&'static str, bool)>) {
    let list = all_candidates();
    let mut results = Vec::with_capacity(list.len());
    for chunk in list.chunks(PARALLEL) {
        std::thread::scope(|s| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|enc| s.spawn(move || (enc.name, passes(ffmpeg, enc.name, &probe_args(enc)))))
                .collect();
            for h in handles {
                if let Ok(r) = h.join() {
                    results.push(r);
                }
            }
        });
    }
    let audio = [AudioCodec::Aac, AudioCodec::Opus]
        .into_iter()
        .filter(|a| passes(ffmpeg, audio_encoder(*a), &audio_probe_args(*a)))
        .collect();
    let probe = Probe {
        ffmpeg_version: version(ffmpeg),
        working: results
            .iter()
            .filter(|(_, ok)| *ok)
            .map(|(n, _)| (*n).to_owned())
            .collect(),
        audio,
    };
    (probe, results)
}

fn key_for(ffmpeg: &Path) -> Option<String> {
    let meta = std::fs::metadata(ffmpeg).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    Some(cache_key(ffmpeg, meta.len(), mtime))
}

fn cache_path() -> Option<PathBuf> {
    Config::config_dir().map(|d| d.join("encoders.json"))
}

/// What a previous run found, if it was for this very ffmpeg. For processes
/// that must not wait on the probe, like the settings child.
pub(crate) fn read_cache() -> Option<Probe> {
    let ffmpeg = locate()?;
    let key = key_for(&ffmpeg)?;
    let cached: Cached = serde_json::from_slice(&std::fs::read(cache_path()?).ok()?).ok()?;
    (cached.key == key).then_some(cached.probe)
}

fn write_cache(ffmpeg: &Path, probe: &Probe) {
    let (Some(key), Some(path)) = (key_for(ffmpeg), cache_path()) else {
        return;
    };
    let cached = Cached {
        key,
        probe: probe.clone(),
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(&cached) {
        let _ = std::fs::write(path, json);
    }
}

/// Probes on a thread of its own, on every start. A cache for this very
/// ffmpeg answers at once and the fresh result replaces it. Called once, by
/// the process that owns recording.
pub(crate) fn start_probe() {
    std::thread::spawn(|| {
        let Some(ffmpeg) = locate() else {
            publish(ProbeState::NoFfmpeg);
            return;
        };
        if let Some(p) = read_cache() {
            publish(ProbeState::Done(p));
        }
        let (probe, _) = run_probe(&ffmpeg);
        write_cache(&ffmpeg, &probe);
        publish(ProbeState::Done(probe));
    });
}

/// `windup --encoders-test`: one line per candidate, `name ok|fails`. Exit 0
/// if any H.264 encoder works, 1 if none does, 2 without ffmpeg.
pub(crate) fn encoders_test() -> i32 {
    let Some(ffmpeg) = locate() else {
        eprintln!("windup: no ffmpeg found (set WINDUP_FFMPEG)");
        return 2;
    };
    println!("{}", ffmpeg.display());
    let (probe, results) = run_probe(&ffmpeg);
    for (name, ok) in &results {
        println!("{name} {}", if *ok { "ok" } else { "fails" });
    }
    for a in [AudioCodec::Aac, AudioCodec::Opus] {
        let ok = probe.audio.contains(&a);
        println!("{} {}", audio_encoder(a), if ok { "ok" } else { "fails" });
    }
    let h264 = candidates(Codec::H264, LINUX);
    i32::from(
        !h264
            .iter()
            .any(|e| probe.working.iter().any(|w| w == e.name)),
    )
}
