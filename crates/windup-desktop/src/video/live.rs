//! A recording as the app holds it, from the launcher's Record to the
//! notice: started on a worker (the ScreenCast portal may wait on its
//! dialog, ffmpeg takes a moment to start), run, stopped, and joined on a
//! worker again. Every host drives one [`Live`] from its own loop: the
//! eframe app (Windows, X11) each frame, the Wayland background loop each
//! tick. The bar is the host's; this only says when it goes up and down.
//!
//! The frame source is chosen here for the recording and for
//! `--record-test` alike ([`screen_factory`]).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(any(target_os = "windows", target_os = "linux"))]
use windup_core::capture::backend::Moving;
use windup_core::capture::backend::video_order;
use windup_core::capture::geometry::DRect;
use windup_core::capture::session::VideoChoice;
use windup_core::config::VideoConfig;
use windup_core::video::{Bitrate, even_size, job_rate};
use windup_ui::capture::bar::{bitrate_label, colours_label};

use super::outcome::{self, Ended, Unopened};
use super::run::{AudioSource, RecClock, RecordJob, Recording, SourceFactory, out_dir};
use crate::capture::gate::Reply;

/// How long after the overlay closed the first frame is taken: the overlay
/// was hidden a frame before, and this lets the compositor or window manager
/// put the desktop back on the screen before anything is read off it.
const START_SETTLE: Duration = Duration::from_millis(100);

/// Where the ScreenCast factory leaves the next restore token.
pub(crate) type TokenSlot = Arc<Mutex<Option<String>>>;

/// A frame source, and the ScreenCast token slot when it is the portal.
pub(crate) struct Picked {
    pub factory: Box<dyn SourceFactory>,
    pub token: Option<TokenSlot>,
}

/// The frame source for `rect` (desktop units) on this desktop: the first
/// of `video_order`. `token` is the saved ScreenCast restore token.
// Only the ScreenCast portal (Linux) takes a token.
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
pub(crate) fn screen_factory(rect: DRect, token: Option<String>) -> Result<Picked, String> {
    let order = video_order(&crate::capture::video_facts());
    let plain = |factory: Box<dyn SourceFactory>| Picked {
        factory,
        token: None,
    };
    match order.first() {
        #[cfg(target_os = "windows")]
        Some(Moving::Ddagrab | Moving::Gdigrab) => {
            let layouts: Vec<DRect> = crate::capture::windows::monitor_list()
                .into_iter()
                .map(|(r, _)| r)
                .collect();
            let f = super::windows::WindowsFactory::new(
                rect,
                &layouts,
                &super::windows::dxgi_outputs(),
            )
            .ok_or(outcome::TOO_SMALL)?;
            Ok(plain(Box::new(f)))
        }
        #[cfg(target_os = "linux")]
        Some(Moving::X11grab) => {
            let f = super::x11::X11Factory::from_env(rect)
                .ok_or("DISPLAY is not set, or the area is too small to record")?;
            Ok(plain(Box::new(f)))
        }
        #[cfg(target_os = "linux")]
        Some(m @ (Moving::ExtImageCopy | Moving::WlrScreencopy)) => {
            use crate::linux::wayland::screencopy::Method;
            let method = if *m == Moving::ExtImageCopy {
                Method::ExtImageCopy
            } else {
                Method::WlrScreencopy
            };
            let f =
                crate::linux::video::WaylandFactory::new(method, rect).ok_or(outcome::TOO_SMALL)?;
            Ok(plain(Box::new(f)))
        }
        #[cfg(target_os = "linux")]
        Some(Moving::ScreenCast) => {
            let f = crate::linux::video::ScreenCastFactory::new(rect, token)
                .ok_or(outcome::TOO_SMALL)?;
            let slot = f.token_slot();
            Ok(Picked {
                factory: Box::new(f),
                token: Some(slot),
            })
        }
        _ => Err(format!(
            "this desktop has no screen source yet ({order:?}); use --pattern"
        )),
    }
}

/// The configuration a recording runs with: the saved one, with what was
/// picked on the launcher bar on top.
pub(crate) fn job_config(cfg: &VideoConfig, choice: &VideoChoice) -> VideoConfig {
    VideoConfig {
        fps: choice.fps,
        bitrate: choice.bitrate,
        gif_colours: choice.gif_colours,
        system_audio: choice.system_audio,
        mic: choice.mic,
        ..cfg.clone()
    }
}

/// The area's size in whole units.
fn size_of(rect: DRect) -> (u32, u32) {
    let r = rect.normalised();
    (r.w.round().max(0.0) as u32, r.h.round().max(0.0) as u32)
}

/// The bar's two read-only chips: the frame rate, and the bitrate the
/// recording really runs at (Auto worked out as `Recording::start` does it)
/// or a GIF's colour count.
pub(crate) fn chips(cfg: &VideoConfig, size: (u32, u32)) -> (u32, String) {
    let rate = job_rate(cfg.container, cfg.codec, cfg.bitrate, cfg.fps, size);
    let label = match rate.and_then(|r| r.kbps) {
        Some(kbps) => bitrate_label(Bitrate::Kbps(kbps)),
        // A GIF; or an area too small, which never starts.
        None => colours_label(cfg.gif_colours),
    };
    (cfg.fps, label)
}

/// What to record, from the launcher's Record.
pub(crate) struct Ask {
    pub rect: DRect,
    pub choice: VideoChoice,
    /// `config.video` as it was saved, with the bar's picks remembered.
    pub cfg: VideoConfig,
}

/// The recording, once ffmpeg runs.
pub(crate) struct Running {
    rec: Recording,
    /// The sound sources asked for that did not open.
    unopened: Unopened,
    pub clock: RecClock,
    pub fps: u32,
    /// The second chip: a bitrate, or a GIF's colours.
    pub label: String,
}

impl Running {
    pub(crate) fn pause(&self) {
        self.rec.pause();
    }

    pub(crate) fn resume(&self) {
        self.rec.resume();
    }
}

type Started = (Recording, Option<TokenSlot>, Unopened);
/// Why it did not start, and the token slot if the portal was asked: the
/// token sent is spent either way, so the next one is still saved.
type NotStarted = (String, Option<TokenSlot>);

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Starting {
        rx: Receiver<Result<Started, NotStarted>>,
        /// Stop came before the start did.
        stop: bool,
        fps: u32,
        label: String,
    },
    Running(Running),
    Joining(Receiver<Ended>),
}

/// What a host does after [`Live::poll`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LiveEvent {
    /// Recording: put the bar up.
    Started,
    /// Stopping: take the bar down; the join runs.
    Stopping,
    /// Over, with this to say. The host shows the notice and reopens its
    /// gate.
    Over(Ended),
    /// The ScreenCast portal handed a new restore token, to save.
    Token(String),
}

/// One recording at a time, from its start to its notice.
#[derive(Default)]
pub(crate) struct Live {
    phase: Phase,
    token: Option<TokenSlot>,
    /// The token last saved, so each new one is saved once.
    saved_token: Option<String>,
}

impl Live {
    /// Starting, running or joining.
    pub(crate) fn busy(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    /// Starting or running: what the Video hotkey stops.
    pub(crate) fn stoppable(&self) -> bool {
        matches!(self.phase, Phase::Starting { .. } | Phase::Running(_))
    }

    pub(crate) fn running(&self) -> Option<&Running> {
        match &self.phase {
            Phase::Running(r) => Some(r),
            _ => None,
        }
    }

    /// Starts recording on a worker. False, and nothing starts, when a
    /// recording is under way already.
    pub(crate) fn start(&mut self, ask: Ask) -> bool {
        if self.busy() {
            return false;
        }
        let Ask { rect, choice, cfg } = ask;
        let cfg = job_config(&cfg, &choice);
        let size = size_of(rect);
        let (fps, label) = chips(&cfg, size);
        self.saved_token = cfg.screencast_token.clone();
        self.token = None;
        let (tx, rx) = channel();
        let reply = Reply::new(tx, Err(("the recording's start thread ended".into(), None)));
        let spawned = std::thread::Builder::new()
            .name("recording-start".into())
            .spawn(move || reply.send(begin(rect, size, cfg)));
        if let Err(e) = spawned {
            // The closure, and with it `reply`, is dropped: that reports.
            log::error!("recording: no thread to start it: {e}");
        }
        self.phase = Phase::Starting {
            rx,
            stop: false,
            fps,
            label,
        };
        true
    }

    /// Stops the recording (or, while it is starting, as soon as it has
    /// started). The join runs on a worker; `poll` says when it is over.
    pub(crate) fn stop(&mut self) {
        match &mut self.phase {
            Phase::Starting { stop, .. } => *stop = true,
            Phase::Running(_) => {
                if let Phase::Running(running) = std::mem::take(&mut self.phase) {
                    self.phase = Phase::Joining(join(running.rec, running.unopened));
                }
            }
            Phase::Idle | Phase::Joining(_) => {}
        }
    }

    /// Moves the recording along. Call it often (each frame, each tick).
    pub(crate) fn poll(&mut self) -> Vec<LiveEvent> {
        let mut events = Vec::new();
        let mut failed = false;
        match &mut self.phase {
            Phase::Idle => {}
            Phase::Starting {
                rx,
                stop,
                fps,
                label,
            } => match rx.try_recv() {
                Ok(Ok((rec, token, unopened))) => {
                    log::info!("recording: started");
                    self.token = token;
                    let running = Running {
                        clock: rec.clock(),
                        rec,
                        unopened,
                        fps: *fps,
                        label: std::mem::take(label),
                    };
                    if *stop {
                        self.phase = Phase::Joining(join(running.rec, running.unopened));
                        events.push(LiveEvent::Stopping);
                    } else {
                        self.phase = Phase::Running(running);
                        events.push(LiveEvent::Started);
                    }
                }
                Ok(Err((e, token))) => {
                    log::warn!("recording: did not start: {e}");
                    self.token = token;
                    self.phase = Phase::Idle;
                    events.push(LiveEvent::Over(Ended {
                        message: outcome::start_text(&e).to_owned(),
                        saved: None,
                    }));
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.phase = Phase::Idle;
                    events.push(LiveEvent::Over(Ended {
                        message: outcome::NOT_STARTED.to_owned(),
                        saved: None,
                    }));
                }
            },
            Phase::Running(running) => {
                // A segment that failed ended the recording; what was
                // recorded before it is joined and saved.
                if let Some(why) = running.rec.failed() {
                    log::warn!("recording: {why}");
                    failed = true;
                } else if running.rec.finished() {
                    // Its thread ended with nobody asking: it panicked.
                    // Stopping joins it, which says nothing was saved.
                    log::error!("recording: the recording thread ended by itself");
                    failed = true;
                }
            }
            Phase::Joining(rx) => match rx.try_recv() {
                Ok(ended) => {
                    log::info!("recording: over ({:?})", ended.saved);
                    self.phase = Phase::Idle;
                    events.push(LiveEvent::Over(ended));
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.phase = Phase::Idle;
                    events.push(LiveEvent::Over(Ended {
                        message: outcome::NOT_SAVED.to_owned(),
                        saved: None,
                    }));
                }
            },
        }
        if failed {
            self.stop();
            events.push(LiveEvent::Stopping);
        }
        // The token is spent by each session (a pause starts a new one), so
        // it is looked at for as long as the slot is held.
        if let Some(slot) = &self.token
            && let Some(t) = slot.lock().ok().and_then(|t| t.clone())
            && self.saved_token.as_ref() != Some(&t)
        {
            self.saved_token = Some(t.clone());
            events.push(LiveEvent::Token(t));
        }
        // An over event (put last, after any token) ends the slot.
        if events.iter().any(|e| matches!(e, LiveEvent::Over(_))) {
            self.token = None;
            if let Some(i) = events.iter().position(|e| matches!(e, LiveEvent::Over(_))) {
                let over = events.remove(i);
                events.push(over);
            }
        }
        events
    }
}

/// The worker's half of `Live::start`.
fn begin(rect: DRect, size: (u32, u32), cfg: VideoConfig) -> Result<Started, NotStarted> {
    std::thread::sleep(START_SETTLE);
    if even_size(size.0, size.1).is_none() {
        return Err((outcome::TOO_SMALL.into(), None));
    }
    let out_dir = out_dir(&cfg).ok_or((outcome::NO_FOLDER.to_owned(), None))?;
    let picked = screen_factory(rect, cfg.screencast_token.clone()).map_err(|e| (e, None))?;
    let (audio, unopened) = open_audio(&cfg);
    let job = RecordJob {
        factory: picked.factory,
        audio,
        cfg,
        size,
        out_dir,
    };
    match Recording::start(job) {
        Ok(rec) => Ok((rec, picked.token, unopened)),
        Err(e) => Err((e, picked.token)),
    }
}

type Opener = fn(Option<&str>) -> Result<Box<dyn AudioSource>, String>;

/// The sound sources asked for. One that will not open is left out (the
/// launcher offered it, so this is a device gone since): the picture is
/// still recorded, and the notice at the end says which sound is missing.
fn open_audio(cfg: &VideoConfig) -> (Vec<Box<dyn AudioSource>>, Unopened) {
    let mut audio = Vec::new();
    let mut unopened = Unopened::default();
    let system = (cfg.system_audio, cfg.system_device.as_deref());
    let mic = (cfg.mic, cfg.mic_device.as_deref());
    for (i, ((wanted, device), what, open)) in [
        (
            system,
            "computer sound",
            super::audio::open_system as Opener,
        ),
        (mic, "the microphone", super::audio::open_mic),
    ]
    .into_iter()
    .enumerate()
    {
        if !wanted {
            continue;
        }
        match open(device) {
            Ok(src) => audio.push(src),
            Err(e) => {
                log::warn!("recording: {what} did not open, recording without it: {e}");
                if i == 0 {
                    unopened.system = true;
                } else {
                    unopened.mic = true;
                }
            }
        }
    }
    (audio, unopened)
}

/// Stops `rec` and joins it on a worker; the receiver gets the notice.
fn join(mut rec: Recording, unopened: Unopened) -> Receiver<Ended> {
    let (tx, rx) = channel();
    let reply = Reply::new(
        tx,
        Ended {
            message: outcome::NOT_SAVED.to_owned(),
            saved: None,
        },
    );
    let handle = rec.stop();
    let spawned = std::thread::Builder::new()
        .name("recording-join".into())
        .spawn(move || {
            let result: Result<PathBuf, String> = handle
                .join()
                .unwrap_or_else(|_| Err("the recording thread panicked".into()));
            if let Err(e) = &result {
                log::warn!("recording: {e}");
            }
            let failed = rec.failed();
            reply.send(outcome::ended(
                &result,
                failed.as_deref(),
                rec.sound_lost(),
                unopened,
            ));
        });
    if let Err(e) = spawned {
        log::error!("recording: no thread for the join: {e}");
    }
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use windup_core::video::{Codec, Container};

    fn choice() -> VideoChoice {
        VideoChoice {
            fps: 60,
            bitrate: Bitrate::Kbps(12000),
            gif_colours: 64,
            system_audio: true,
            mic: false,
        }
    }

    #[test]
    fn the_bars_picks_override_the_saved_settings() {
        let saved = VideoConfig {
            screencast_token: Some("t".into()),
            mic: true,
            ..VideoConfig::default()
        };
        let c = job_config(&saved, &choice());
        assert_eq!(c.fps, 60);
        assert_eq!(c.bitrate, Bitrate::Kbps(12000));
        assert_eq!(c.gif_colours, 64);
        assert!(c.system_audio);
        assert!(!c.mic, "the bar turned it off");
        // The rest is the saved configuration's.
        assert_eq!(c.screencast_token.as_deref(), Some("t"));
        assert_eq!(c.container, saved.container);
    }

    #[test]
    fn the_chips_show_the_bitrate_auto_works_out() {
        let cfg = VideoConfig::default();
        assert_eq!(chips(&cfg, (1920, 1080)), (30, "8 Mb/s".to_owned()));
        let fixed = VideoConfig {
            bitrate: Bitrate::Kbps(4000),
            ..cfg.clone()
        };
        assert_eq!(chips(&fixed, (1920, 1080)).1, "4 Mb/s");
        let gif = VideoConfig {
            container: Container::Gif,
            gif_colours: 128,
            ..cfg
        };
        assert_eq!(chips(&gif, (640, 480)).1, "128 colours");
    }

    #[test]
    fn the_chip_shows_what_ffmpeg_is_given() {
        // WebM takes no H.264: the recording falls back to VP9, and so does
        // the chip's Auto figure.
        let cfg = VideoConfig {
            container: Container::WebM,
            codec: Codec::H264,
            ..VideoConfig::default()
        };
        let given = job_rate(cfg.container, cfg.codec, cfg.bitrate, cfg.fps, (1921, 1081))
            .and_then(|r| r.kbps)
            .unwrap();
        assert_eq!(
            chips(&cfg, (1921, 1081)).1,
            bitrate_label(Bitrate::Kbps(given))
        );
        assert_ne!(
            given,
            windup_core::video::auto_bitrate(1920, 1080, 30, Codec::H264)
        );
    }

    #[test]
    fn a_recording_thread_that_dies_stops_the_recording() {
        let rec = Recording::with_supervisor(|| panic!("in the supervisor"));
        let mut live = Live {
            phase: Phase::Running(Running {
                clock: rec.clock(),
                rec,
                unopened: Unopened::default(),
                fps: 30,
                label: String::new(),
            }),
            ..Live::default()
        };
        let mut events = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !events.iter().any(|e| matches!(e, LiveEvent::Over(_)))
            && std::time::Instant::now() < deadline
        {
            events.extend(live.poll());
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(events.contains(&LiveEvent::Stopping), "{events:?}");
        match events.last() {
            Some(LiveEvent::Over(e)) => assert_eq!(e.message, outcome::NOT_SAVED),
            other => panic!("{other:?}"),
        }
        assert!(!live.busy());
    }

    #[test]
    fn an_idle_recording_polls_quietly_and_stop_does_nothing() {
        let mut live = Live::default();
        assert!(!live.busy());
        live.stop();
        assert!(live.poll().is_empty());
        assert!(live.running().is_none());
    }

    #[test]
    fn a_start_that_fails_is_over_with_its_reason() {
        let mut live = Live::default();
        let (tx, rx) = channel();
        tx.send(Err((outcome::TOO_SMALL.to_owned(), None))).unwrap();
        live.phase = Phase::Starting {
            rx,
            stop: false,
            fps: 30,
            label: String::new(),
        };
        assert!(live.busy() && live.stoppable());
        assert_eq!(
            live.poll(),
            vec![LiveEvent::Over(Ended {
                message: outcome::TOO_SMALL.to_owned(),
                saved: None,
            })]
        );
        assert!(!live.busy(), "the gate may reopen");
    }

    #[test]
    fn a_start_thread_that_died_still_ends_the_recording() {
        let mut live = Live::default();
        let (tx, rx) = channel::<Result<Started, NotStarted>>();
        drop(Reply::new(tx, Err(("gone".into(), None))));
        live.phase = Phase::Starting {
            rx,
            stop: false,
            fps: 30,
            label: String::new(),
        };
        match live.poll().as_slice() {
            [LiveEvent::Over(e)] => assert_eq!(e.message, outcome::NOT_STARTED),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_start_that_failed_after_the_portal_answered_still_saves_its_token() {
        let mut live = Live {
            saved_token: Some("spent".into()),
            ..Live::default()
        };
        let slot: TokenSlot = Arc::new(Mutex::new(Some("next".into())));
        let (tx, rx) = channel();
        tx.send(Err(("ffmpeg was not found.".to_owned(), Some(slot))))
            .unwrap();
        live.phase = Phase::Starting {
            rx,
            stop: false,
            fps: 30,
            label: String::new(),
        };
        let events = live.poll();
        assert_eq!(events[0], LiveEvent::Token("next".into()));
        assert!(matches!(&events[1], LiveEvent::Over(e) if e.message == outcome::NOT_STARTED));
    }

    #[test]
    fn a_new_token_is_saved_once_and_before_the_end() {
        let mut live = Live::default();
        let slot: TokenSlot = Arc::new(Mutex::new(Some("old".into())));
        live.saved_token = Some("old".into());
        live.token = Some(Arc::clone(&slot));
        let (tx, rx) = channel();
        live.phase = Phase::Joining(rx);
        assert!(live.poll().is_empty(), "the saved one is not saved again");
        *slot.lock().unwrap() = Some("new".into());
        tx.send(Ended {
            message: "Saved to a.mp4".into(),
            saved: None,
        })
        .unwrap();
        let events = live.poll();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], LiveEvent::Token("new".into()));
        assert!(matches!(events[1], LiveEvent::Over(_)));
        assert!(live.poll().is_empty());
    }
}
