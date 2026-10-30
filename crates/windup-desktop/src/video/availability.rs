//! What the launcher bar may offer for Video and Steps: gathered here,
//! decided by `windup_core::video::avail` and `windup_core::steps::avail`,
//! handed to `Session::new` as plain data.
//!
//! Two of the facts can be slow: the frame sources (on Wayland the
//! ScreenCast portal is asked, for up to 500 ms) and the sound devices (a
//! PipeWire roundtrip, up to seconds). The overlay must open at once, for a
//! screenshot above all, so both are kept in statics: found on threads at
//! startup and again at each capture. A capture built on the UI thread
//! (Windows, X11) reads what is kept; the Wayland grab worker gives the fresh
//! answers until the picture is in plus `FRESH_WAIT`, then reads what is
//! kept, which is at worst one capture old. Before the first answer the
//! controls say Windup is still checking.
//!
//! A finder that panics still leaves an answer (no sound devices, no frame
//! source), so nothing says "still checking" until the next start.

use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use windup_core::capture::backend::{Facts, Moving, StepRoute, steps_order, video_order};
use windup_core::capture::session::ModeAvailability;
use windup_core::config::VideoConfig;
use windup_core::steps::avail::steps_available;
use windup_core::steps::clicks::{ClickFacts, click_route, clicks_reason};
use windup_core::video::avail::{self, AudioDevices};

use super::audio::audio_devices;
use super::ffmpeg::{PROBE, ProbeState};

/// How long the Wayland grab worker gives fresh answers once the picture is
/// in. Short: a screenshot must not open later for a check only Video needs.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) const FRESH_WAIT: Duration = Duration::from_millis(150);

static SOUND: RwLock<Option<AudioDevices>> = RwLock::new(None);
static LISTING: AtomicBool = AtomicBool::new(false);
static SOURCES: RwLock<Option<Sources>> = RwLock::new(None);
static FINDING: AtomicBool = AtomicBool::new(false);

/// What the desktop can capture with, found together from one look at it.
#[derive(Clone)]
struct Sources {
    video: Vec<Moving>,
    steps: Vec<StepRoute>,
    clicks: Result<(), &'static str>,
}

impl Sources {
    /// `wayland_clicks` looks at the pointer devices; it is asked only on
    /// Wayland.
    fn from(f: &Facts, wayland_clicks: impl FnOnce() -> ClickFacts) -> Sources {
        // The OS hands Windows and X11 the clicks. On Wayland they are read
        // from the pointer devices, when Windup may.
        let wayland = !f.windows && !f.x11_only;
        let clicks = if wayland {
            wayland_clicks()
        } else {
            ClickFacts {
                native: true,
                ..ClickFacts::default()
            }
        };
        Sources {
            video: video_order(f),
            steps: steps_order(f),
            clicks: clicks_reason(click_route(&clicks)),
        }
    }

    /// A finder that panicked: nothing to capture with.
    fn failed() -> Sources {
        Sources {
            video: vec![Moving::None],
            steps: vec![StepRoute::None],
            clicks: Ok(()),
        }
    }
}

/// Whether this Wayland session can read the pointer devices.
fn wayland_click_facts() -> ClickFacts {
    #[cfg(target_os = "linux")]
    {
        crate::linux::steps::evdev::click_facts()
    }
    #[cfg(not(target_os = "linux"))]
    {
        ClickFacts::default()
    }
}

/// What a sound listing that panicked leaves: a sound system with no
/// devices, which the toggles report as "No sound device was found."
const SOUND_FAILED: AudioDevices = AudioDevices {
    outputs: Vec::new(),
    inputs: Vec::new(),
    available: true,
};

/// Runs `find` and keeps its answer in `slot`, or `failed` if it panics.
/// `busy` is cleared either way, so the next refresh runs.
fn keep<T>(slot: &RwLock<Option<T>>, busy: &AtomicBool, failed: T, find: impl FnOnce() -> T) {
    struct Guard<'a, T> {
        slot: &'a RwLock<Option<T>>,
        busy: &'a AtomicBool,
        failed: Option<T>,
    }
    impl<T> Drop for Guard<'_, T> {
        fn drop(&mut self) {
            if std::thread::panicking()
                && let Some(failed) = self.failed.take()
                && let Ok(mut g) = self.slot.write()
            {
                *g = Some(failed);
            }
            self.busy.store(false, Ordering::Release);
        }
    }
    let mut guard = Guard {
        slot,
        busy,
        failed: Some(failed),
    };
    let found = find();
    if let Ok(mut g) = slot.write() {
        *g = Some(found);
    }
    guard.failed = None;
}

/// `keep` on a thread of its own, unless one is already at it. The receiver
/// hears when the answer is in, or is cut off at once when nothing started.
fn spawn_keep<T: Send + Sync + 'static>(
    name: &str,
    slot: &'static RwLock<Option<T>>,
    busy: &'static AtomicBool,
    failed: T,
    find: impl FnOnce() -> T + Send + 'static,
) -> Receiver<()> {
    let (tx, rx) = channel();
    if busy.swap(true, Ordering::AcqRel) {
        return rx;
    }
    let spawned = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            keep(slot, busy, failed, find);
            let _ = tx.send(());
        });
    if let Err(e) = spawned {
        log::debug!("could not start {name}: {e}");
        busy.store(false, Ordering::Release);
    }
    rx
}

/// Fresh answers on their way.
pub(crate) struct Refresh([Receiver<()>; 2]);

impl Refresh {
    /// Waits for them, at most `cap` in all.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn wait(self, cap: Duration) {
        let end = Instant::now() + cap;
        for rx in self.0 {
            let _ = rx.recv_timeout(end.saturating_duration_since(Instant::now()));
        }
    }
}

/// Lists the sound devices again, on a thread.
pub(crate) fn refresh_sound() -> Receiver<()> {
    spawn_keep(
        "sound-devices",
        &SOUND,
        &LISTING,
        SOUND_FAILED,
        audio_devices,
    )
}

/// Finds the sound devices and the frame sources again, on threads.
pub(crate) fn refresh() -> Refresh {
    let sound = refresh_sound();
    let sources = spawn_keep(
        "video-sources",
        &SOURCES,
        &FINDING,
        Sources::failed(),
        || Sources::from(&crate::capture::video_facts(), wayland_click_facts),
    );
    Refresh([sound, sources])
}

/// What is kept now. Never blocks.
pub(crate) fn current(config: &VideoConfig) -> ModeAvailability {
    let state = probe_state();
    let sources = SOURCES.read().ok().and_then(|g| g.clone());
    let video = avail::video(
        state.probed(),
        config.container,
        config.codec,
        sources.as_ref().map(|s| s.video.as_slice()),
    );
    // Until the first look is in, Steps is offered: a session that cannot
    // take its first picture says so when it starts.
    let (steps, clicks) = match &sources {
        Some(s) => (steps_available(&s.steps), s.clicks),
        None => (Ok(()), Ok(())),
    };
    let sound = SOUND
        .read()
        .ok()
        .and_then(|g| g.as_ref().map(AudioDevices::counts));
    let (system_audio, mic) = avail::sound(sound);
    ModeAvailability {
        video,
        system_audio,
        mic,
        steps,
        clicks,
    }
}

/// The probe as published now.
pub(crate) fn probe_state() -> ProbeState {
    PROBE
        .read()
        .map(|g| g.clone())
        .unwrap_or(ProbeState::Running)
}

/// The probe if it differs from `seen`, compared under the lock so an
/// unchanged one is not copied.
pub(crate) fn probe_if_changed(seen: &ProbeState) -> Option<ProbeState> {
    let g = PROBE.read().ok()?;
    (*g != *seen).then(|| g.clone())
}

/// The sound devices if they differ from `seen`, likewise.
pub(crate) fn sound_if_changed(seen: &Option<AudioDevices>) -> Option<Option<AudioDevices>> {
    let g = SOUND.read().ok()?;
    (*g != *seen).then(|| g.clone())
}

/// For a capture built on the UI thread: what is kept, and a refresh
/// started for next time.
pub(crate) fn cached(config: &VideoConfig) -> ModeAvailability {
    let now = current(config);
    refresh();
    now
}

#[cfg(test)]
mod tests {
    use super::*;
    use windup_core::video::pcm::NO_SOUND_DEVICE;

    #[test]
    fn a_finder_that_panics_leaves_its_failed_answer_and_is_not_busy() {
        static SLOT: RwLock<Option<u32>> = RwLock::new(None);
        static BUSY: AtomicBool = AtomicBool::new(true);
        let joined = std::thread::spawn(|| keep(&SLOT, &BUSY, 7, || panic!("enumeration")));
        assert!(joined.join().is_err());
        assert_eq!(*SLOT.read().unwrap(), Some(7));
        assert!(!BUSY.load(Ordering::Acquire));
    }

    #[test]
    fn a_finder_that_answers_keeps_its_answer() {
        static SLOT: RwLock<Option<u32>> = RwLock::new(None);
        static BUSY: AtomicBool = AtomicBool::new(true);
        keep(&SLOT, &BUSY, 7, || 3);
        assert_eq!(*SLOT.read().unwrap(), Some(3));
        assert!(!BUSY.load(Ordering::Acquire));
    }

    #[test]
    fn a_failed_sound_listing_says_no_device_not_still_checking() {
        assert_eq!(
            avail::sound(Some(SOUND_FAILED.counts())),
            (Err(NO_SOUND_DEVICE), Err(NO_SOUND_DEVICE))
        );
    }

    #[test]
    fn steps_and_clicks_follow_the_desktop() {
        use windup_core::steps::avail::NO_STEPS_SOURCE;
        use windup_core::steps::clicks::{CLICKS_NO_POINTER, CLICKS_NOT_SET_UP};
        let never = || -> ClickFacts { panic!("only Wayland looks at the devices") };
        let windows = Sources::from(
            &Facts {
                windows: true,
                ..Facts::default()
            },
            never,
        );
        assert_eq!(steps_available(&windows.steps), Ok(()));
        assert_eq!(windows.clicks, Ok(()));
        let x11 = Sources::from(
            &Facts {
                x11_only: true,
                ..Facts::default()
            },
            never,
        );
        assert_eq!(steps_available(&x11.steps), Ok(()));
        assert_eq!(x11.clicks, Ok(()));
        // Wayland with nothing to take pictures with.
        let unreadable = || ClickFacts {
            pointer_nodes: 2,
            ..ClickFacts::default()
        };
        let bare = Sources::from(&Facts::default(), unreadable);
        assert_eq!(steps_available(&bare.steps), Err(NO_STEPS_SOURCE));
        assert_eq!(bare.clicks, Err(CLICKS_NOT_SET_UP));
        // Readable through the udev rule, or no mouse at all.
        let udev = Sources::from(&Facts::default(), || ClickFacts {
            readable: 1,
            ..unreadable()
        });
        assert_eq!(udev.clicks, Ok(()));
        let none = Sources::from(&Facts::default(), ClickFacts::default);
        assert_eq!(none.clicks, Err(CLICKS_NO_POINTER));
    }

    #[test]
    fn a_busy_finder_is_not_started_twice() {
        static SLOT: RwLock<Option<u32>> = RwLock::new(None);
        static BUSY: AtomicBool = AtomicBool::new(true);
        let rx = spawn_keep("test", &SLOT, &BUSY, 7, || 3);
        // Cut off at once: nothing started, nothing to wait for.
        assert!(rx.recv_timeout(Duration::from_secs(1)).is_err());
        assert_eq!(*SLOT.read().unwrap(), None);
    }
}
