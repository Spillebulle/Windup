//! What the launcher bar may offer for Video: gathered here, decided by
//! `windup_core::video::avail`, handed to `Session::new` as plain data.
//!
//! Listing the sound devices can take up to seconds on Linux (a PipeWire
//! roundtrip), and the overlay must open at once, so the list is kept in a
//! static: filled on a thread at startup and refreshed on a thread at each
//! capture. A capture that is built on a worker anyway (the Wayland hosts)
//! lists afresh there; one built on the UI thread (Windows, X11) reads the
//! last list, which is at worst one capture old. Before the first list is in,
//! the toggles say Windup is still looking.

use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use windup_core::capture::backend::video_order;
use windup_core::capture::session::ModeAvailability;
use windup_core::config::VideoConfig;
use windup_core::video::avail::{self, Probed, SoundDevices};

use super::audio::{AudioDevices, audio_devices};
use super::ffmpeg::{PROBE, ProbeState};

static SOUND: RwLock<Option<SoundDevices>> = RwLock::new(None);
static LISTING: AtomicBool = AtomicBool::new(false);

fn counted(d: &AudioDevices) -> SoundDevices {
    SoundDevices {
        system: d.available,
        outputs: d.outputs.len(),
        inputs: d.inputs.len(),
    }
}

/// Lists the sound devices now, blocking, and keeps the list.
fn list_sound() -> SoundDevices {
    let d = counted(&audio_devices());
    if let Ok(mut g) = SOUND.write() {
        *g = Some(d);
    }
    d
}

/// Lists the sound devices on a thread of its own, unless one is already
/// at it.
pub(crate) fn refresh_sound() {
    if LISTING.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("sound-devices".into())
        .spawn(|| {
            list_sound();
            LISTING.store(false, Ordering::Release);
        });
    if spawned.is_err() {
        LISTING.store(false, Ordering::Release);
    }
}

fn video(config: &VideoConfig) -> Result<(), &'static str> {
    let order = video_order(&crate::capture::video_facts());
    let state = PROBE
        .read()
        .map(|g| g.clone())
        .unwrap_or(ProbeState::Running);
    let probed = match &state {
        ProbeState::NoFfmpeg => Probed::NoFfmpeg,
        ProbeState::Running => Probed::Running,
        ProbeState::Done(p) => Probed::Done(p),
    };
    avail::video(probed, config.container, config.codec, &order)
}

fn with_sound(video: Result<(), &'static str>, sound: Option<SoundDevices>) -> ModeAvailability {
    let (system_audio, mic) = avail::sound(sound);
    ModeAvailability {
        video,
        system_audio,
        mic,
    }
}

/// For a capture built on the UI thread: the last sound list, and a fresh
/// one started for next time. `video_facts` asks nothing slow off Wayland.
pub(crate) fn cached(config: &VideoConfig) -> ModeAvailability {
    let sound = SOUND.read().ok().and_then(|g| *g);
    refresh_sound();
    with_sound(video(config), sound)
}

/// For a capture built on a worker: everything asked afresh, on a thread
/// of its own so it runs while the picture is taken rather than after it
/// (the portal is asked for up to 500 ms, PipeWire for up to seconds).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn ask(config: &VideoConfig) -> Receiver<ModeAvailability> {
    let (tx, rx) = channel();
    let config = config.clone();
    let spawned = std::thread::Builder::new()
        .name("video-availability".into())
        .spawn(move || {
            let _ = tx.send(with_sound(video(&config), Some(list_sound())));
        });
    if let Err(e) = spawned {
        log::debug!("could not start the video availability check: {e}");
    }
    rx
}
