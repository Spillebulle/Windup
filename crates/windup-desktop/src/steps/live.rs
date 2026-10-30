//! A Steps session as the app holds it. A camera thread takes a picture for
//! each trigger the filter accepts, hiding the bar for the instant of the
//! shot where nothing else keeps it out; a writer thread encodes it and
//! files it as `NN.png` beside `steps.json`. Every host drives one
//! [`StepsLive`] from its own loop and reads what happened with `poll`; the
//! bar is the host's, and it keeps [`BarFacts`] up to date.
//!
//! Stop and cancel never wait: the camera thread notices, lets the writer
//! finish, decides what is left, and says so with `StepsEvent::Over`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{
    Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError, channel, sync_channel,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windup_core::capture::backend::BarHost;
use windup_core::capture::geometry::DRect;
use windup_core::config::StepsConfig;
use windup_core::steps::clicks::{ClickFilter, Press, Takes, is_own_click};
use windup_core::steps::model::{Step, StepCause, StepsDoc, new_session_dir, steps_root};
use windup_core::steps::place::must_conceal;

use super::camera::{Camera, Conceal, Shot};
use crate::capture::CaptureError;

/// Failed shots in a row that end the session.
const MAX_FAILURES: u32 = 3;
/// How often the camera thread looks at the stop and cancel flags while no
/// trigger comes.
const IDLE_CHECK: Duration = Duration::from_millis(50);
/// Shots waiting for the writer before the camera waits for it: a few full
/// screens, so a burst of clicks does not hold every picture in memory.
const WRITE_QUEUE: usize = 4;
/// In each session folder: the id of the process recording it, so that
/// another Windup starting does not take it for a leftover.
const OWNER_FILE: &str = "owner";

/// What asks for a step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Trigger {
    /// The step key.
    Hotkey,
    Click(Press),
}

/// What the host knows about its bar. The camera thread reads it for every
/// trigger.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarFacts {
    /// Where the bar is, desktop units; none while it is not up.
    pub rect: Option<DRect>,
    /// Wayland: the bar's surface has the pointer, its position unknown.
    pub pointer_on_bar: bool,
    pub host: BarHost,
    /// Windows: the bar is excluded from capture.
    pub excluded: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StepsEvent {
    /// Step N is written.
    Step(u32),
    /// The session is over; nothing follows.
    Over(StepsEnd),
}

#[derive(Clone, Debug, PartialEq)]
pub enum StepsEnd {
    /// Stopped with steps taken: review the folder.
    Review(PathBuf),
    /// Cancelled; the folder is gone.
    Discarded,
    /// Stopped before any step; the folder is gone.
    Empty,
    /// Shots or writes kept failing. The steps taken so far are in `review`,
    /// when there were any.
    Failed {
        why: String,
        review: Option<PathBuf>,
    },
}

/// What the threads and the host share.
#[derive(Default)]
struct Shared {
    paused: AtomicBool,
    stop: AtomicBool,
    cancel: AtomicBool,
    count: AtomicU32,
}

/// One running session.
pub struct StepsLive {
    dir: PathBuf,
    triggers: Sender<Trigger>,
    shared: Arc<Shared>,
    events: Receiver<StepsEvent>,
    over: bool,
}

impl StepsLive {
    /// Creates the session folder and starts the threads. `camera` is open
    /// already (`open_camera`).
    pub fn start(
        cfg: &StepsConfig,
        takes: Takes,
        camera: Box<dyn Camera>,
        conceal: Arc<dyn Conceal>,
        bar: Arc<Mutex<BarFacts>>,
    ) -> Result<StepsLive, String> {
        let root = steps_root().ok_or("there is no cache folder for the steps")?;
        Self::start_in(&root, cfg, takes, camera, conceal, bar)
    }

    fn start_in(
        root: &Path,
        cfg: &StepsConfig,
        takes: Takes,
        camera: Box<dyn Camera>,
        conceal: Arc<dyn Conceal>,
        bar: Arc<Mutex<BarFacts>>,
    ) -> Result<StepsLive, String> {
        let now = jiff::Zoned::now();
        let dir = new_session_dir(root, &now)
            .map_err(|e| format!("cannot create the steps folder: {e}"))?;
        if let Err(e) = std::fs::write(dir.join(OWNER_FILE), std::process::id().to_string()) {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!("cannot write to the steps folder: {e}"));
        }
        let shared = Arc::new(Shared::default());
        let (triggers, trigger_rx) = channel();
        let (events_tx, events) = channel();
        let (jobs, job_rx) = sync_channel(WRITE_QUEUE);

        let writer = {
            let (dir, shared, events_tx) = (dir.clone(), shared.clone(), events_tx.clone());
            let doc = StepsDoc::new(&now);
            std::thread::Builder::new()
                .name("steps-writer".into())
                .spawn(move || write_steps(&dir, doc, job_rx, &shared, &events_tx))
        };
        let writer = match writer {
            Ok(h) => h,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(format!("no thread to write the steps: {e}"));
            }
        };
        let cam = CameraThread {
            dir: dir.clone(),
            takes,
            camera,
            conceal,
            bar,
            shared: shared.clone(),
            triggers: trigger_rx,
            jobs,
            writer,
            events: events_tx,
            started: Instant::now(),
        };
        if let Err(e) = std::thread::Builder::new()
            .name("steps-camera".into())
            .spawn(move || cam.run())
        {
            // The job sender went with the closure, which ends the writer.
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!("no thread to take the steps: {e}"));
        }
        log::info!(
            "steps: started ({:?}, {takes:?}) in {}",
            cfg.trigger,
            dir.display()
        );
        Ok(StepsLive {
            dir,
            triggers,
            shared,
            events,
            over: false,
        })
    }

    /// The session folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// For a watcher on its own thread (the click hook).
    pub fn sender(&self) -> Sender<Trigger> {
        self.triggers.clone()
    }

    pub fn trigger(&self, t: Trigger) {
        let _ = self.triggers.send(t);
    }

    /// Pauses, or resumes; says whether it is now paused.
    pub fn toggle_pause(&self) -> bool {
        let paused = !self.shared.paused.fetch_xor(true, Ordering::SeqCst);
        log::info!("steps: {}", if paused { "paused" } else { "resumed" });
        paused
    }

    /// Steps written so far.
    pub fn count(&self) -> u32 {
        self.shared.count.load(Ordering::SeqCst)
    }

    /// Ends the session, keeping the steps; `poll` says when it is over.
    pub fn stop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
    }

    /// Ends the session and throws the steps away; `poll` says when the
    /// folder is gone.
    pub fn cancel(&mut self) {
        self.shared.cancel.store(true, Ordering::SeqCst);
    }

    /// What happened since the last call. `Over` comes once, last.
    pub fn poll(&mut self) -> Vec<StepsEvent> {
        let mut out = Vec::new();
        while !self.over {
            match self.events.try_recv() {
                Ok(e) => {
                    self.over = matches!(e, StepsEvent::Over(_));
                    out.push(e);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // The camera thread died without a word (a panic).
                    log::error!("steps: the camera thread ended by itself");
                    self.over = true;
                    out.push(StepsEvent::Over(StepsEnd::Failed {
                        why: "the steps stopped unexpectedly".into(),
                        review: None,
                    }));
                }
            }
        }
        out
    }
}

impl Drop for StepsLive {
    /// A session nobody holds any more is thrown away.
    fn drop(&mut self) {
        if !self.over {
            self.shared.cancel.store(true, Ordering::SeqCst);
        }
    }
}

/// A picture on its way to the writer.
struct Job {
    shot: Shot,
    cause: StepCause,
    at_ms: u64,
}

struct CameraThread {
    dir: PathBuf,
    takes: Takes,
    camera: Box<dyn Camera>,
    conceal: Arc<dyn Conceal>,
    bar: Arc<Mutex<BarFacts>>,
    shared: Arc<Shared>,
    triggers: Receiver<Trigger>,
    jobs: SyncSender<Job>,
    writer: std::thread::JoinHandle<Result<(), String>>,
    events: Sender<StepsEvent>,
    started: Instant,
}

/// Why the camera loop ended.
enum Ending {
    /// Stop, cancel, the writer gave up, or nobody is left to send triggers.
    Asked,
    Failed(String),
}

impl CameraThread {
    fn run(mut self) {
        let ending = self.shoot_until_done();
        let CameraThread {
            dir,
            jobs,
            writer,
            shared,
            events,
            ..
        } = self;
        // The writer files what it has, then sees the end of the queue.
        drop(jobs);
        let written = writer
            .join()
            .unwrap_or_else(|_| Err("the steps writer stopped".into()));
        let count = shared.count.load(Ordering::SeqCst);
        // Only now, with both threads done, so no write can follow it.
        let remove = || match std::fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("steps: could not remove {}: {e}", dir.display()),
        };
        let failure = match (ending, written) {
            (Ending::Failed(why), _) | (_, Err(why)) => Some(why),
            (Ending::Asked, Ok(())) => None,
        };
        let end = if shared.cancel.load(Ordering::SeqCst) {
            remove();
            log::info!("steps: cancelled");
            StepsEnd::Discarded
        } else if let Some(why) = failure {
            let review = if count > 0 {
                Some(dir.clone())
            } else {
                remove();
                None
            };
            StepsEnd::Failed { why, review }
        } else if count > 0 {
            StepsEnd::Review(dir.clone())
        } else {
            remove();
            StepsEnd::Empty
        };
        log::info!("steps: over ({end:?})");
        let _ = events.send(StepsEvent::Over(end));
    }

    fn shoot_until_done(&mut self) -> Ending {
        let mut filter = ClickFilter::default();
        // Desktop units per point, from the last picture; until then, one.
        let mut units_per_point = 1.0;
        let mut failures = 0;
        loop {
            if self.shared.cancel.load(Ordering::SeqCst) {
                return Ending::Asked;
            }
            // Stopping still takes the presses that came before it.
            let t = if self.shared.stop.load(Ordering::SeqCst) {
                match self.triggers.try_recv() {
                    Ok(t) => t,
                    Err(_) => return Ending::Asked,
                }
            } else {
                match self.triggers.recv_timeout(IDLE_CHECK) {
                    Ok(t) => t,
                    // A writer that ended has failed (it ends otherwise only
                    // when this thread does): end now, not at the next press.
                    Err(RecvTimeoutError::Timeout) if self.writer.is_finished() => {
                        return Ending::Asked;
                    }
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => return Ending::Asked,
                }
            };
            let facts = *self.bar.lock().unwrap_or_else(|e| e.into_inner());
            let paused = self.shared.paused.load(Ordering::SeqCst);
            let (at, cause) = match t {
                Trigger::Hotkey => {
                    if !self.takes.hotkey || paused {
                        continue;
                    }
                    (None, StepCause::Hotkey)
                }
                Trigger::Click(p) => {
                    let own = is_own_click(p.at, facts.rect, facts.pointer_on_bar);
                    if !self.takes.clicks || !filter.accept(p, units_per_point, paused, own) {
                        continue;
                    }
                    (p.at, StepCause::Click(p.button))
                }
            };
            let at_ms = self.started.elapsed().as_millis() as u64;
            match self.shoot(at, &facts) {
                Ok(mut shot) => {
                    failures = 0;
                    if shot.image.width() > 0 {
                        units_per_point = shot.scale * shot.monitor.w / shot.image.width() as f32;
                    }
                    // A key press has no point to mark.
                    if cause == StepCause::Hotkey {
                        shot.pointer = None;
                    }
                    if self.jobs.send(Job { shot, cause, at_ms }).is_err() {
                        // The writer has given up; its reason ends the session.
                        return Ending::Asked;
                    }
                }
                Err(e) => {
                    failures += 1;
                    let why = match e {
                        CaptureError::Failed(why) => why,
                        other => other.message().to_string(),
                    };
                    log::warn!("steps: a shot failed ({failures} in a row): {why}");
                    if failures >= MAX_FAILURES {
                        return Ending::Failed(why);
                    }
                }
            }
        }
    }

    /// One picture, with the bar hidden for it when it would be in it.
    fn shoot(&mut self, at: Option<[f32; 2]>, facts: &BarFacts) -> Result<Shot, CaptureError> {
        // A camera that cannot tell the monitor is taken to picture the
        // bar's, so the bar is hidden whenever it is up.
        let monitor = self.camera.monitor(at).or(facts.rect);
        let hidden = monitor.is_some_and(|m| {
            must_conceal(facts.host, facts.excluded, facts.rect, m) && self.conceal.hide(m)
        });
        if hidden {
            log::info!("steps: bar hidden for the shot");
        }
        let shot = self.camera.shoot(at);
        if hidden {
            self.conceal.show();
        }
        shot
    }
}

/// Files each shot as the next `NN.png` and saves the document after it.
/// Once cancelled it writes nothing more, but still empties the queue so the
/// camera never waits on it.
fn write_steps(
    dir: &Path,
    mut doc: StepsDoc,
    jobs: Receiver<Job>,
    shared: &Shared,
    events: &Sender<StepsEvent>,
) -> Result<(), String> {
    let cancelled = || shared.cancel.load(Ordering::SeqCst);
    for job in jobs {
        if cancelled() {
            continue;
        }
        let png = encode_png(&job.shot.image)?;
        if cancelled() {
            continue;
        }
        let file = doc.next_file();
        std::fs::write(dir.join(&file), png).map_err(|e| format!("cannot write {file}: {e}"))?;
        doc.push(Step {
            file,
            cause: job.cause,
            pointer: job.shot.pointer,
            scale: job.shot.scale,
            monitor: job.shot.monitor_name,
            caption: String::new(),
            at_ms: job.at_ms,
        });
        if cancelled() {
            continue;
        }
        doc.save(dir)
            .map_err(|e| format!("cannot save the steps: {e}"))?;
        let n = shared.count.fetch_add(1, Ordering::SeqCst) + 1;
        log::info!("steps: step {n} written");
        let _ = events.send(StepsEvent::Step(n));
    }
    Ok(())
}

/// PNG, fast rather than small: the exports encode again anyway.
fn encode_png(img: &image::RgbaImage) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Adaptive)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("cannot encode a step: {e}"))?;
    Ok(out)
}

/// Removes the session folders a Windup that ended mid-session left behind.
/// Called once at startup.
pub fn remove_stale_sessions() {
    if let Some(root) = steps_root() {
        remove_stale_in(&root, crate::video::run::process_running);
    }
}

/// Removes every folder in `root` but those whose owner is this process or
/// another that still runs (a second Windup on Windows, which has no single
/// instance). A folder with no readable owner is a leftover. Anything that
/// is not a folder is left.
fn remove_stale_in(root: &Path, running: impl Fn(u32) -> bool) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let path = entry.path();
        let owner = std::fs::read_to_string(path.join(OWNER_FILE))
            .ok()
            .and_then(|t| t.trim().parse::<u32>().ok());
        if owner.is_some_and(|pid| pid == std::process::id() || running(pid)) {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => log::info!(
                "removed {}, left by a Steps session that never ended",
                path.display()
            ),
            Err(e) => log::warn!("could not remove {}: {e}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;
    use std::sync::Barrier;
    use std::time::Instant;
    use windup_core::steps::model::{Button, StepsDoc};

    const MON_A: DRect = DRect {
        x: 0.0,
        y: 0.0,
        w: 1920.0,
        h: 1080.0,
    };
    const MON_B: DRect = DRect {
        x: 1920.0,
        y: 0.0,
        w: 1920.0,
        h: 1080.0,
    };

    type Log = Arc<Mutex<Vec<&'static str>>>;

    /// 32×24 pictures of `MON_A`; `fails` says which shots (by number from
    /// 0) fail; with a barrier, each shot meets it twice: once on entering,
    /// once to be let go.
    struct FakeCamera {
        log: Log,
        fails: Vec<usize>,
        n: usize,
        barrier: Option<Arc<Barrier>>,
    }

    impl Camera for FakeCamera {
        fn monitor(&mut self, _at: Option<[f32; 2]>) -> Option<DRect> {
            Some(MON_A)
        }

        fn shoot(&mut self, _at: Option<[f32; 2]>) -> Result<Shot, CaptureError> {
            self.log.lock().unwrap().push("shoot");
            if let Some(b) = &self.barrier {
                b.wait();
                b.wait();
            }
            let n = self.n;
            self.n += 1;
            if self.fails.contains(&n) {
                return Err(CaptureError::Failed("fake".into()));
            }
            Ok(Shot {
                image: RgbaImage::new(32, 24),
                pointer: Some([3.0, 4.0]),
                monitor: MON_A,
                monitor_name: "A".into(),
                scale: 1.0,
            })
        }
    }

    struct FakeConceal {
        log: Log,
    }

    impl Conceal for FakeConceal {
        fn hide(&self, _shot_monitor: DRect) -> bool {
            self.log.lock().unwrap().push("hide");
            true
        }

        fn show(&self) {
            self.log.lock().unwrap().push("show");
        }
    }

    struct Rig {
        live: StepsLive,
        log: Log,
        bar: Arc<Mutex<BarFacts>>,
    }

    fn rig(takes: Takes, fails: Vec<usize>, barrier: Option<Arc<Barrier>>) -> Rig {
        let log: Log = Arc::default();
        let camera = FakeCamera {
            log: log.clone(),
            fails,
            n: 0,
            barrier,
        };
        let bar = Arc::new(Mutex::new(BarFacts {
            rect: None,
            pointer_on_bar: false,
            host: BarHost::Outside,
            excluded: false,
        }));
        // A root each, so no test reuses the name of a folder another removed.
        static RIGS: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "windup-steps-live-{}-{}",
            std::process::id(),
            RIGS.fetch_add(1, Ordering::SeqCst)
        ));
        let live = StepsLive::start_in(
            &root,
            &StepsConfig::default(),
            takes,
            Box::new(camera),
            Arc::new(FakeConceal { log: log.clone() }),
            bar.clone(),
        )
        .unwrap();
        Rig { live, log, bar }
    }

    const BOTH: Takes = Takes {
        clicks: true,
        hotkey: true,
    };

    fn click(x: f32, t: f64) -> Trigger {
        Trigger::Click(Press {
            button: Button::Left,
            at: Some([x, 10.0]),
            t,
        })
    }

    /// Polls until the session is over, collecting every event.
    fn until_over(live: &mut StepsLive) -> (Vec<StepsEvent>, StepsEnd) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            for e in live.poll() {
                if let StepsEvent::Over(end) = e {
                    return (seen, end);
                }
                seen.push(e);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the session never ended");
    }

    /// Waits for `n` steps to be written.
    fn until_count(live: &StepsLive, n: u32) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while live.count() < n {
            assert!(Instant::now() < deadline, "only {} steps", live.count());
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn two_steps_write_two_pngs_and_the_json() {
        let mut r = rig(BOTH, vec![], None);
        r.live.trigger(Trigger::Hotkey);
        r.live.trigger(click(500.0, 10.0));
        until_count(&r.live, 2);
        let dir = r.live.dir().to_path_buf();
        r.live.stop();
        let (events, end) = until_over(&mut r.live);
        assert_eq!(end, StepsEnd::Review(dir.clone()));
        assert!(matches!(
            events[..],
            [StepsEvent::Step(1), StepsEvent::Step(2)]
        ));
        let doc = StepsDoc::load(&dir).unwrap();
        let files: Vec<_> = doc.steps.iter().map(|s| s.file.as_str()).collect();
        assert_eq!(files, ["01.png", "02.png"]);
        assert_eq!(doc.steps[0].cause, StepCause::Hotkey);
        assert_eq!(doc.steps[0].pointer, None);
        assert_eq!(doc.steps[1].cause, StepCause::Click(Button::Left));
        assert_eq!(doc.steps[1].pointer, Some([3.0, 4.0]));
        assert_eq!(doc.steps[1].monitor, "A");
        let png = image::open(dir.join("02.png")).unwrap();
        assert_eq!((png.width(), png.height()), (32, 24));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancel_while_a_shot_is_being_written_leaves_no_folder() {
        let barrier = Arc::new(Barrier::new(2));
        let mut r = rig(BOTH, vec![], Some(barrier.clone()));
        let dir = r.live.dir().to_path_buf();
        r.live.trigger(Trigger::Hotkey);
        // The camera is in its shot.
        barrier.wait();
        r.live.cancel();
        barrier.wait();
        let (_, end) = until_over(&mut r.live);
        assert_eq!(end, StepsEnd::Discarded);
        assert!(!dir.exists());
        std::thread::sleep(Duration::from_millis(200));
        assert!(!dir.exists());
    }

    #[test]
    fn a_concealed_bar_is_hidden_for_the_shot_and_shown_after() {
        let mut r = rig(BOTH, vec![], None);
        r.bar.lock().unwrap().rect = Some(DRect {
            x: 800.0,
            y: 24.0,
            w: 300.0,
            h: 48.0,
        });
        r.live.trigger(click(10.0, 0.0));
        until_count(&r.live, 1);
        assert_eq!(*r.log.lock().unwrap(), ["hide", "shoot", "show"]);

        r.log.lock().unwrap().clear();
        r.bar.lock().unwrap().rect = Some(DRect {
            x: MON_B.x + 800.0,
            y: 24.0,
            w: 300.0,
            h: 48.0,
        });
        r.live.trigger(click(10.0, 5.0));
        until_count(&r.live, 2);
        assert_eq!(*r.log.lock().unwrap(), ["shoot"]);

        // Excluded from capture: never hidden.
        r.log.lock().unwrap().clear();
        {
            let mut b = r.bar.lock().unwrap();
            b.rect = Some(MON_A);
            b.host = BarHost::ExcludedWindow;
            b.excluded = true;
        }
        r.live.trigger(Trigger::Hotkey);
        until_count(&r.live, 3);
        assert_eq!(*r.log.lock().unwrap(), ["shoot"]);
        r.live.cancel();
        until_over(&mut r.live);
    }

    #[test]
    fn pause_ignores_triggers() {
        let mut r = rig(BOTH, vec![], None);
        assert!(r.live.toggle_pause());
        r.live.trigger(Trigger::Hotkey);
        r.live.trigger(click(10.0, 0.0));
        r.live.stop();
        let (_, end) = until_over(&mut r.live);
        assert_eq!(r.live.count(), 0);
        assert_eq!(end, StepsEnd::Empty);
        assert!(r.log.lock().unwrap().is_empty());
    }

    #[test]
    fn resume_takes_steps_again() {
        let mut r = rig(BOTH, vec![], None);
        assert!(r.live.toggle_pause());
        assert!(!r.live.toggle_pause());
        r.live.trigger(Trigger::Hotkey);
        until_count(&r.live, 1);
        r.live.cancel();
        until_over(&mut r.live);
    }

    #[test]
    fn clicks_follow_takes() {
        let mut r = rig(
            Takes {
                clicks: false,
                hotkey: true,
            },
            vec![],
            None,
        );
        r.live.trigger(click(10.0, 0.0));
        r.live.stop();
        until_over(&mut r.live);
        assert_eq!(r.live.count(), 0);

        let mut r = rig(
            Takes {
                clicks: true,
                hotkey: false,
            },
            vec![],
            None,
        );
        r.live.trigger(Trigger::Hotkey);
        r.live.stop();
        until_over(&mut r.live);
        assert_eq!(r.live.count(), 0);
    }

    #[test]
    fn a_click_on_the_bar_takes_no_step() {
        let mut r = rig(BOTH, vec![], None);
        r.bar.lock().unwrap().rect = Some(DRect {
            x: 2700.0,
            y: 24.0,
            w: 300.0,
            h: 48.0,
        });
        r.live.trigger(Trigger::Click(Press {
            button: Button::Left,
            at: Some([2750.0, 40.0]),
            t: 0.0,
        }));
        // Wayland: no position, but the bar's surface has the pointer.
        r.bar.lock().unwrap().pointer_on_bar = true;
        r.live.trigger(Trigger::Click(Press {
            button: Button::Right,
            at: None,
            t: 1.0,
        }));
        r.live.stop();
        let (_, end) = until_over(&mut r.live);
        assert_eq!(r.live.count(), 0);
        assert_eq!(end, StepsEnd::Empty);
    }

    #[test]
    fn a_double_click_is_one_step() {
        let mut r = rig(BOTH, vec![], None);
        r.live.trigger(click(10.0, 0.0));
        r.live.trigger(click(10.0, 0.2));
        r.live.trigger(click(10.0, 2.0));
        until_count(&r.live, 2);
        r.live.stop();
        until_over(&mut r.live);
        assert_eq!(r.live.count(), 2);
    }

    #[test]
    fn three_failed_shots_end_with_what_was_taken() {
        let mut r = rig(BOTH, vec![2, 3, 4], None);
        let dir = r.live.dir().to_path_buf();
        for t in 0..5 {
            r.live.sender().send(click(10.0, t as f64)).unwrap();
        }
        let (_, end) = until_over(&mut r.live);
        let StepsEnd::Failed { why, review } = end else {
            panic!("{end:?}");
        };
        assert!(why.contains("fake"), "{why}");
        assert_eq!(review, Some(dir.clone()));
        assert_eq!(StepsDoc::load(&dir).unwrap().steps.len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failure_between_good_shots_is_skipped() {
        let mut r = rig(BOTH, vec![1, 2], None);
        for t in 0..4 {
            r.live.trigger(click(10.0, t as f64));
        }
        until_count(&r.live, 2);
        r.live.cancel();
        assert_eq!(until_over(&mut r.live).1, StepsEnd::Discarded);
    }

    #[test]
    fn three_failures_with_nothing_taken_leave_no_folder() {
        let mut r = rig(BOTH, vec![0, 1, 2], None);
        let dir = r.live.dir().to_path_buf();
        for t in 0..3 {
            r.live.trigger(click(10.0, t as f64));
        }
        let (_, end) = until_over(&mut r.live);
        assert!(
            matches!(end, StepsEnd::Failed { review: None, .. }),
            "{end:?}"
        );
        assert!(!dir.exists());
    }

    #[test]
    fn stop_with_no_steps_removes_the_folder() {
        let mut r = rig(BOTH, vec![], None);
        let dir = r.live.dir().to_path_buf();
        assert!(dir.is_dir());
        r.live.stop();
        let (_, end) = until_over(&mut r.live);
        assert_eq!(end, StepsEnd::Empty);
        assert!(!dir.exists());
    }

    #[test]
    fn stale_sessions_are_all_removed() {
        let root = std::env::temp_dir().join(format!("windup-steps-stale-{}", std::process::id()));
        std::fs::create_dir_all(root.join("2026-10-30 142501")).unwrap();
        std::fs::create_dir_all(root.join("2026-10-30 142501 (2)/x")).unwrap();
        std::fs::write(root.join("note.txt"), "kept").unwrap();
        remove_stale_in(&root, |_| false);
        let left: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, ["note.txt"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_live_session_folder_is_kept() {
        let root = std::env::temp_dir().join(format!("windup-steps-owned-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let ours = root.join("2026-10-30 142501");
        let dead = root.join("2026-10-30 142502");
        let other = root.join("2026-10-30 142503");
        for (dir, pid) in [
            (&ours, std::process::id()),
            (&dead, u32::MAX - 1),
            (&other, 4242),
        ] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join(OWNER_FILE), pid.to_string()).unwrap();
        }
        // 4242 stands for another Windup that still runs.
        remove_stale_in(&root, |pid| pid == 4242);
        assert!(ours.is_dir() && other.is_dir());
        assert!(!dead.exists());
        // The real check: no process has this id.
        remove_stale_in(&root, crate::video::run::process_running);
        assert!(ours.is_dir());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_session_folder_names_its_owner() {
        let mut r = rig(BOTH, vec![], None);
        let dir = r.live.dir().to_path_buf();
        let owner = std::fs::read_to_string(dir.join(OWNER_FILE)).unwrap();
        assert_eq!(owner.trim(), std::process::id().to_string());
        r.live.cancel();
        until_over(&mut r.live);
    }

    #[test]
    fn a_writer_failure_ends_the_session_without_another_trigger() {
        let mut r = rig(BOTH, vec![], None);
        let dir = r.live.dir().to_path_buf();
        // Nowhere to write: the shot cannot be filed.
        std::fs::remove_dir_all(&dir).unwrap();
        r.live.trigger(Trigger::Hotkey);
        let started = Instant::now();
        let (_, end) = until_over(&mut r.live);
        assert!(started.elapsed() < Duration::from_secs(2));
        let StepsEnd::Failed { why, review } = end else {
            panic!("{end:?}");
        };
        assert!(why.contains("01.png"), "{why}");
        assert_eq!(review, None);
    }
}
