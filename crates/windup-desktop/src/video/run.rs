//! A recording as running processes: one ffmpeg per segment, the threads that
//! feed it frames and sound through pipes, the folder the segments wait in,
//! and the join into the final file.
//!
//! `Recorder` (core) keeps the clock and the segment list; this keeps the
//! processes in step with it. One supervisor thread owns everything, the
//! `Recording` handle only sends it commands, so pausing never blocks the
//! caller on ffmpeg finishing a segment.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windup_core::capture::output::{recording_name_now, unique_path};
use windup_core::config::{Config, VideoConfig};
use windup_core::video::pcm::put_f32le;
use windup_core::video::{
    AudioIn, Codec, Container, Failed, Finish, Pacer, RecState, Recorder, Retry, SegmentJob,
    Source, candidates, job_rate, join_passes, pick_encoder, retry, segment_args, segment_ext,
};

use super::ffmpeg::{self, PROBE, ProbeState};
use super::outcome::SIZE_CHANGED;
use super::pipe::{self, PipeServer};

/// Frames we capture ourselves, written to ffmpeg as BGRA.
pub(crate) trait FrameSource: Send {
    fn size(&self) -> (u32, u32);
    /// Fills `buf` with the newest frame (`w * h * 4` bytes). `Ok(false)`
    /// means no new frame yet, and `buf` still holds the last one.
    fn next_frame(&mut self, buf: &mut Vec<u8>) -> Result<bool, String>;
}

/// Where a recording's picture comes from. A self-capturing source (ddagrab,
/// gdigrab, x11grab) is only an ffmpeg input; a `RawPipe` one also opens a
/// `FrameSource` per segment, and the pipe's path is filled in here.
pub(crate) trait SourceFactory: Send {
    fn source(&self) -> Source;
    /// The frames for one segment. Err says why not, in words the notice
    /// may show (`outcome::start_text`, `outcome::failure_text`).
    fn open_pipe(&self) -> Result<Box<dyn FrameSource>, String>;
    /// What to try instead when `source` makes ffmpeg exit within a second
    /// of starting (ddagrab on a machine it does not suit).
    fn fallback(&self) -> Option<Source> {
        None
    }
}

/// Sound we capture ourselves, written to ffmpeg as interleaved f32le.
pub(crate) trait AudioSource: Send {
    fn rate(&self) -> u32;
    fn channels(&self) -> u16;
    /// Appends the samples that arrived since the last call; none is fine.
    /// A source that keeps time pads silence itself, so the sound lasts as
    /// long as the picture.
    fn read(&mut self, buf: &mut Vec<f32>) -> Result<(), String>;
    /// A segment starts: ffmpeg has opened the pipe, and its clock starts
    /// now. Whatever arrived before (while paused, or while ffmpeg started)
    /// is dropped rather than put at the start of the segment, and a source
    /// that pads silence starts counting again.
    fn restart(&mut self) {}
    /// The segment ends: the last `read`, plus whatever silence a source
    /// that keeps time still owes, so the sound lasts to the very end.
    fn finish(&mut self, buf: &mut Vec<f32>) -> Result<(), String> {
        self.read(buf)
    }
    /// Set once the device has gone (unplugged, its server stopped): the
    /// recording carries on silent, and the bar can say so.
    fn lost(&self) -> Option<Arc<AtomicBool>> {
        None
    }
}

pub(crate) struct RecordJob {
    pub factory: Box<dyn SourceFactory>,
    pub audio: Vec<Box<dyn AudioSource>>,
    pub cfg: VideoConfig,
    /// The recorded area's size, for the automatic bitrate.
    pub size: (u32, u32),
    /// Where the finished file goes.
    pub out_dir: PathBuf,
}

/// Where a recording is saved: the configured folder, else `<Videos>/Windup`.
pub(crate) fn out_dir(cfg: &VideoConfig) -> Option<PathBuf> {
    cfg.save_dir
        .clone()
        .or_else(windup_core::capture::output::default_video_dir)
}

/// Why a factory's frames did not open, when it has nothing better to say.
pub(crate) const UNREADABLE: &str = "The screen could not be read.";

/// How long ffmpeg gets to finish a segment before it is killed.
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);
/// A hardware encoder that dies this soon did not work at all.
const FALLBACK_WINDOW: Duration = Duration::from_secs(1);
const TICK: Duration = Duration::from_millis(20);
/// How long a segment that ends with `q` waits for its sound writers.
const SOUND_FLUSH: Duration = Duration::from_millis(500);

/// Numbers the segment folders of this process.
static NEXT_DIR: AtomicU32 = AtomicU32::new(0);
/// The start of every segment folder's name: `recording-<pid>-<n>`.
const DIR_PREFIX: &str = "recording-";

enum Ctl {
    Pause(f64),
    Resume(f64),
    Stop(f64),
    Cancel,
    /// Tests swap the encoder mid-recording to make a segment fail.
    #[cfg(test)]
    Encoder(&'static str, bool),
}

struct Shared {
    t0: Instant,
    recorder: Mutex<Recorder>,
    failed: Mutex<Option<String>>,
    /// Each sound source's `lost` flag.
    sound_lost: Vec<Arc<AtomicBool>>,
    /// The app is going: a join under way kills its ffmpeg and gives up.
    abort: AtomicBool,
    /// Whether the last failure was noticed with a writer error in hand.
    #[cfg(test)]
    writer_error_seen: AtomicBool,
}

/// What tests change about a recording.
#[derive(Default)]
struct Hooks {
    /// An encoder (name, hardware) instead of the probe's choice.
    encoder: Option<(&'static str, bool)>,
    /// Holds a failed segment's check back until a writer has failed too,
    /// so the race between the two goes the writer's way.
    #[cfg(test)]
    hold_check_for_writer: bool,
}

impl Shared {
    fn now(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }
}

pub(crate) struct Recording {
    shared: Arc<Shared>,
    tx: mpsc::Sender<Ctl>,
    thread: Option<JoinHandle<Result<PathBuf, String>>>,
}

impl Recording {
    /// Starts recording segment 0. Segments wait in the cache folder.
    pub(crate) fn start(job: RecordJob) -> Result<Recording, String> {
        let base = Config::cache_dir().ok_or("There is no cache folder to record into.")?;
        Self::start_in(job, &base, Hooks::default())
    }

    /// `start` with the segments under `base`, and the test hooks.
    fn start_in(job: RecordJob, base: &Path, hooks: Hooks) -> Result<Recording, String> {
        let ffmpeg = ffmpeg::locate().ok_or("ffmpeg was not found.")?;
        let cfg = job.cfg;
        let container = cfg.container;
        // The same numbers the recording bar's chip shows.
        let rate = job_rate(container, cfg.codec, cfg.bitrate, cfg.fps, job.size)
            .ok_or("The area is too small to record.")?;
        let (codec, bitrate) = (rate.codec, rate.kbps);
        let (encoder, hardware) = match hooks.encoder {
            Some(e) => e,
            None if container == Container::Gif => (software(codec), false),
            None => choose_encoder(codec, cfg.encoder.as_deref())?,
        };
        let n = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = base.join(format!("{DIR_PREFIX}{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not make {}: {e}", dir.display()))?;

        let shared = Arc::new(Shared {
            t0: Instant::now(),
            // The clock starts once segment 0 is open: the ScreenCast
            // portal's dialog, answered meanwhile, recorded nothing.
            recorder: Mutex::new(Recorder::start_waiting()),
            failed: Mutex::new(None),
            sound_lost: job.audio.iter().filter_map(|a| a.lost()).collect(),
            abort: AtomicBool::new(false),
            #[cfg(test)]
            writer_error_seen: AtomicBool::new(false),
        });
        let mut run = Run {
            ffmpeg,
            factory: job.factory,
            audio: job.audio.into_iter().map(Some).collect(),
            cfg,
            codec,
            out_dir: job.out_dir,
            dir,
            encoder,
            hardware,
            fell_back: false,
            download: false,
            source_fell_back: false,
            first_size: None,
            bitrate,
            seg: None,
            shared: Arc::clone(&shared),
            #[cfg(test)]
            hold_check_for_writer: hooks.hold_check_for_writer,
        };
        match run.open_segment(0) {
            Ok(seg) => {
                run.seg = Some(seg);
                run.begin_clock();
            }
            Err(e) => {
                run.remove_dir();
                return Err(e);
            }
        }
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("windup-recording".into())
            .spawn(move || run.supervise(rx))
            .map_err(|e| format!("Could not start the recording: {e}"))?;
        Ok(Recording {
            shared,
            tx,
            thread: Some(thread),
        })
    }

    pub(crate) fn pause(&self) {
        let _ = self.tx.send(Ctl::Pause(self.shared.now()));
    }

    pub(crate) fn resume(&self) {
        let _ = self.tx.send(Ctl::Resume(self.shared.now()));
    }

    /// Ends the recording. The thread joins the segments and gives the
    /// saved file, or why there is none.
    pub(crate) fn stop(&mut self) -> JoinHandle<Result<PathBuf, String>> {
        match self.thread.take() {
            Some(thread) => {
                let _ = self.tx.send(Ctl::Stop(self.shared.now()));
                thread
            }
            // Cancelled or stopped already: the segments are gone, and the
            // recorder must not be asked to stop after a cancel.
            None => std::thread::spawn(|| Err("The recording has already ended.".into())),
        }
    }

    /// Ends the recording and throws it away: no file, no segment folder.
    /// Returns once both are gone.
    pub(crate) fn cancel(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = self.tx.send(Ctl::Cancel);
            let _ = thread.join();
        }
    }

    /// Seconds recorded, not counting pauses.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn elapsed(&self) -> f64 {
        self.clock().elapsed()
    }

    /// The timer, for a bar drawn on another thread.
    pub(crate) fn clock(&self) -> RecClock {
        RecClock(Arc::clone(&self.shared))
    }

    /// What ends a join under way, for whoever holds the recording while it
    /// joins on another thread.
    pub(crate) fn abort_handle(&self) -> Abort {
        Abort(Arc::clone(&self.shared))
    }

    /// Set when a segment died; what was recorded before it is still kept.
    pub(crate) fn failed(&self) -> Option<String> {
        self.shared.failed.lock().ok().and_then(|f| f.clone())
    }

    /// The supervisor ended though nobody stopped or cancelled it: it
    /// panicked, and nothing records any more.
    pub(crate) fn finished(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| t.is_finished())
    }

    /// A recording whose supervisor is `body`, for tests of its handling.
    /// The body is handed the recording's abort, to watch.
    #[cfg(test)]
    pub(crate) fn with_supervisor(
        body: impl FnOnce(Abort) -> Result<PathBuf, String> + Send + 'static,
    ) -> Recording {
        let (tx, _rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            t0: Instant::now(),
            recorder: Mutex::new(Recorder::start(0.0)),
            failed: Mutex::new(None),
            sound_lost: Vec::new(),
            abort: AtomicBool::new(false),
            writer_error_seen: AtomicBool::new(false),
        });
        let abort = Abort(Arc::clone(&shared));
        Recording {
            shared,
            tx,
            thread: Some(std::thread::spawn(move || body(abort))),
        }
    }

    /// A sound device went away during the recording, which goes on
    /// without it.
    pub(crate) fn sound_lost(&self) -> bool {
        self.shared
            .sound_lost
            .iter()
            .any(|l| l.load(Ordering::Relaxed))
    }

    #[cfg(test)]
    fn set_encoder(&self, name: &'static str, hardware: bool) {
        let _ = self.tx.send(Ctl::Encoder(name, hardware));
    }
}

/// A recording's timer and pause state, readable from any thread: the
/// recording bar on the Wayland thread reads it every frame.
#[derive(Clone)]
pub(crate) struct RecClock(Arc<Shared>);

impl RecClock {
    /// Seconds recorded, not counting pauses.
    pub(crate) fn elapsed(&self) -> f64 {
        let now = self.0.now();
        self.0.recorder.lock().map_or(0.0, |r| r.elapsed(now))
    }

    pub(crate) fn paused(&self) -> bool {
        self.0
            .recorder
            .lock()
            .is_ok_and(|r| r.state() == RecState::Paused)
    }
}

/// Ends a recording's join early: its ffmpeg is killed, the half-written
/// file and the segment folder removed. For an app that is quitting.
#[derive(Clone)]
pub(crate) struct Abort(Arc<Shared>);

impl Abort {
    pub(crate) fn abort(&self) {
        self.0.abort.store(true, Ordering::Relaxed);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_set(&self) -> bool {
        self.0.abort.load(Ordering::Relaxed)
    }
}

impl Drop for Recording {
    /// A recording nobody stopped is cancelled, so no ffmpeg outlives it.
    fn drop(&mut self) {
        self.cancel();
    }
}

/// The probe's pick for `codec` (the saved choice if it still works), else
/// the software encoder when there is no probe to ask yet.
fn choose_encoder(codec: Codec, wanted: Option<&str>) -> Result<(&'static str, bool), String> {
    let probe = match PROBE.read().as_deref() {
        Ok(ProbeState::Done(p)) => Some(p.clone()),
        _ => None,
    }
    .or_else(ffmpeg::read_cache);
    match probe {
        Some(p) => pick_encoder(codec, wanted, &p)
            .map(|name| (name, is_hardware(codec, name)))
            .ok_or_else(|| "No encoder works for this codec.".into()),
        None => Ok((software(codec), false)),
    }
}

fn is_hardware(codec: Codec, name: &str) -> bool {
    candidates(codec, true)
        .iter()
        .any(|e| e.name == name && e.hardware)
}

fn software(codec: Codec) -> &'static str {
    candidates(codec, true)
        .iter()
        .find(|e| !e.hardware)
        .map_or("libx264", |e| e.name)
}

type AudioWriter = JoinHandle<Option<Box<dyn AudioSource>>>;

/// One ffmpeg and the threads feeding it.
struct Segment {
    index: u32,
    child: Child,
    stdin: Option<ChildStdin>,
    /// Pipe sources end by closing the pipes, others by `q` on stdin.
    piped: bool,
    stop: Arc<AtomicBool>,
    /// Set once ffmpeg has exited, so writers stop waiting for it to open
    /// their pipes.
    gone: Arc<AtomicBool>,
    pipes: Vec<String>,
    video: Option<JoinHandle<()>>,
    /// Each sound's index in `Run::audio`, and the thread that gives it back.
    audio: Vec<(usize, AudioWriter)>,
    /// The first thing a writer thread failed at.
    error: Arc<Mutex<Option<String>>>,
    stderr: Option<JoinHandle<String>>,
    started: Instant,
}

struct Run {
    ffmpeg: PathBuf,
    factory: Box<dyn SourceFactory>,
    /// Taken by a segment's writer threads and given back when it ends.
    audio: Vec<Option<Box<dyn AudioSource>>>,
    cfg: VideoConfig,
    codec: Codec,
    out_dir: PathBuf,
    dir: PathBuf,
    encoder: &'static str,
    hardware: bool,
    fell_back: bool,
    /// Ddagrab's frames are downloaded for the hardware encoder from here on.
    download: bool,
    /// The factory's `fallback` source is in use from here on.
    source_fell_back: bool,
    /// Segment 0's picture size. The join copies the segments into one
    /// stream, which cannot change size part way.
    first_size: Option<(u32, u32)>,
    bitrate: Option<u32>,
    seg: Option<Segment>,
    shared: Arc<Shared>,
    #[cfg(test)]
    hold_check_for_writer: bool,
}

impl Run {
    fn segment_path(&self, index: u32) -> PathBuf {
        self.dir
            .join(format!("seg-{index}.{}", segment_ext(self.cfg.container)))
    }

    /// The open segment's source is giving its picture, so the timer runs.
    fn begin_clock(&self) {
        let now = self.shared.now();
        if let Ok(mut r) = self.shared.recorder.lock() {
            r.begin(now);
        }
    }

    /// The source segments use now: the factory's, or its fallback once
    /// that took over.
    fn wanted_source(&self) -> Source {
        match self.factory.fallback() {
            Some(fallback) if self.source_fell_back => fallback,
            _ => self.factory.source(),
        }
    }

    fn open_segment(&mut self, index: u32) -> Result<Segment, String> {
        let fps = self.cfg.fps.max(1);
        let gif = self.cfg.container == Container::Gif;

        let mut pipes = Vec::new();
        let mut video_pipe = None;
        let source = match self.wanted_source() {
            Source::RawPipe { .. } => {
                let frames = self.factory.open_pipe()?;
                let (w, h) = frames.size();
                let server = PipeServer::create(&self.dir, "video")
                    .map_err(|e| format!("Could not make a pipe: {e}"))?;
                let path = server.path().to_owned();
                pipes.push(path.clone());
                video_pipe = Some((server, frames));
                Source::RawPipe { path, w, h }
            }
            other => other,
        };
        let size = source_size(&source);
        match self.first_size {
            Some(first) if index > 0 && first != size => {
                log::warn!("segment {index}: {size:?}, not {first:?} as segment 0");
                return Err(SIZE_CHANGED.into());
            }
            _ => self.first_size = Some(size),
        }
        let piped = video_pipe.is_some();

        // A GIF has no sound, so its audio pipes would never be opened.
        let mut audio_pipes = Vec::new();
        let mut audio_in = Vec::new();
        if !gif {
            for (i, src) in self.audio.iter().enumerate() {
                let Some(src) = src else { continue };
                let server = PipeServer::create(&self.dir, &format!("audio{i}"))
                    .map_err(|e| format!("Could not make a pipe: {e}"))?;
                audio_in.push(AudioIn {
                    path: server.path().to_owned(),
                    rate: src.rate(),
                    channels: src.channels(),
                });
                pipes.push(server.path().to_owned());
                audio_pipes.push((i, server));
            }
        }

        let job = SegmentJob {
            source,
            fps,
            encoder: self.encoder,
            bitrate_kbps: self.bitrate,
            audio: audio_in,
            out: self.segment_path(index),
            download: self.download,
        };
        let args = segment_args(&job, self.cfg.container);
        log::debug!("segment {index}: ffmpeg {}", args.join(" "));
        let mut command = ffmpeg::command(&self.ffmpeg);
        #[cfg(target_os = "windows")]
        if matches!(job.source, Source::Gdigrab { .. }) {
            command.env(
                "__COMPAT_LAYER",
                super::windows::compat_layer(std::env::var("__COMPAT_LAYER").ok().as_deref()),
            );
        }
        let mut child = command
            .args(&args)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Could not start ffmpeg: {e}"))?;
        let stdin = child.stdin.take();
        let stderr = child.stderr.take().map(|mut s| {
            std::thread::spawn(move || {
                let mut text = String::new();
                let _ = std::io::Read::read_to_string(&mut s, &mut text);
                text
            })
        });

        let stop = Arc::new(AtomicBool::new(false));
        let gone = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let flags = Flags {
            stop: Arc::clone(&stop),
            gone: Arc::clone(&gone),
        };
        let video = video_pipe.map(|(server, frames)| {
            let (flags, error) = (flags.clone(), Arc::clone(&error));
            std::thread::spawn(move || write_video(server, frames, fps, &flags, &error))
        });
        let audio = audio_pipes
            .into_iter()
            .map(|(i, server)| {
                let src = self.audio[i].take();
                let (flags, error) = (flags.clone(), Arc::clone(&error));
                let h = std::thread::spawn(move || {
                    src.map(|src| write_audio(server, src, &flags, &error))
                });
                (i, h)
            })
            .collect();

        Ok(Segment {
            index,
            child,
            stdin,
            piped,
            stop,
            gone,
            pipes,
            video,
            audio,
            error,
            stderr,
            started: Instant::now(),
        })
    }

    /// Ends the open segment: gracefully (ffmpeg finishes the file) or by
    /// killing it. Returns what ffmpeg said and whether it exited cleanly.
    fn end_segment(&mut self, kill: bool) -> Option<(String, bool)> {
        let mut seg = self.seg.take()?;
        seg.stop.store(true, Ordering::Relaxed);
        if kill {
            let _ = seg.child.kill();
        } else if !seg.piped
            && let Some(stdin) = seg.stdin.as_mut()
        {
            // The sound writers flush their last silence and close their
            // pipes first: `q` makes ffmpeg drop what it has not read yet,
            // so the sound would end early (or late) by however much that
            // race left behind.
            let deadline = Instant::now() + SOUND_FLUSH;
            let waited = Instant::now();
            while seg.audio.iter().any(|(_, h)| !h.is_finished()) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            log::debug!(
                "segment {}: sound closed after {:?}",
                seg.index,
                waited.elapsed()
            );
            let _ = stdin.write_all(b"q");
            let _ = stdin.flush();
        }
        drop(seg.stdin.take());

        // The writers see the flag, close their pipes, and ffmpeg reads that
        // as the end of its input.
        let deadline = Instant::now() + EXIT_TIMEOUT;
        let clean = loop {
            match seg.child.try_wait() {
                Ok(Some(status)) => break status.success() && !kill,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(TICK),
                _ => {
                    log::warn!("segment {}: ffmpeg did not finish, killed", seg.index);
                    let _ = seg.child.kill();
                    let _ = seg.child.wait();
                    break false;
                }
            }
        };
        // ffmpeg is gone; a writer still waiting for it to open a pipe goes.
        seg.gone.store(true, Ordering::Relaxed);
        for p in &seg.pipes {
            pipe::unblock(p);
        }
        if let Some(v) = seg.video.take() {
            let _ = v.join();
        }
        for (i, h) in seg.audio.drain(..) {
            match h.join() {
                Ok(src) => self.audio[i] = src,
                Err(_) => log::warn!("audio {i}: the writer panicked, recording without it"),
            }
        }
        let text = seg
            .stderr
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        Some((text, clean))
    }

    /// Notices a segment that died on its own. A hardware encoder that died
    /// at once is given ddagrab's frames through memory, then swapped for the
    /// software one, and the segment started again (`retry` has the order);
    /// anything else ends the recording, keeping what was recorded.
    fn check(&mut self) {
        let Some(seg) = self.seg.as_mut() else {
            return;
        };
        let exited = seg.child.try_wait().ok().flatten();
        #[cfg(test)]
        if self.hold_check_for_writer
            && seg.started.elapsed() < Duration::from_secs(3)
            && seg.error.lock().is_ok_and(|e| e.is_none())
        {
            return;
        }
        let writer = seg.error.lock().ok().and_then(|mut e| e.take());
        if exited.is_none() && writer.is_none() {
            return;
        }
        #[cfg(test)]
        self.shared
            .writer_error_seen
            .store(writer.is_some(), Ordering::Relaxed);
        let index = seg.index;
        let early = seg.started.elapsed() < FALLBACK_WINDOW;
        let (text, clean) = self.end_segment(false).unwrap_or_default();
        // ffmpeg failing is the cause whenever it failed: a writer error then
        // is only its pipe breaking under it, and whichever of the two was
        // noticed first must not decide. A writer error with ffmpeg ending
        // cleanly (it read the end of input we gave it) is the writer's.
        let ffmpeg_failed = !clean || writer.is_none();
        let mut why = match writer {
            Some(w) if !ffmpeg_failed => w,
            _ => text
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_owned())
                .unwrap_or_else(|| match exited {
                    Some(s) => format!("ffmpeg stopped ({s})."),
                    None => "ffmpeg stopped.".into(),
                }),
        };
        // The fallbacks are independent, and the encoder is the likelier
        // culprit, so a hardware encoder that failed at once is tried with
        // ddagrab's frames downloaded, then swapped, with the source kept.
        // Only a failure after that (or with no hardware encoder to blame)
        // moves to the source's fallback.
        let next = (early && ffmpeg_failed)
            .then(|| {
                retry(Failed {
                    ddagrab: matches!(self.wanted_source(), Source::Ddagrab { .. }),
                    hardware: self.hardware && !self.fell_back,
                    downloaded: self.download,
                    source_fallback: !self.source_fell_back && self.factory.fallback().is_some(),
                })
            })
            .flatten();
        if let Some(next) = next {
            match next {
                Retry::Download => {
                    log::warn!(
                        "segment {index}: {} failed at once on the GPU's frames ({why}); \
                         copying them through memory from here on",
                        self.encoder
                    );
                    self.download = true;
                }
                Retry::Software => {
                    let sw = software(self.codec);
                    log::warn!(
                        "segment {index}: {} failed at once ({why}); using {sw} from here on",
                        self.encoder
                    );
                    self.encoder = sw;
                    self.hardware = false;
                    self.fell_back = true;
                }
                Retry::Source => {
                    log::warn!(
                        "segment {index}: the screen source failed at once ({why}); using its fallback from here on"
                    );
                    self.source_fell_back = true;
                }
            }
            match self.open_segment(index) {
                Ok(seg) => {
                    self.seg = Some(seg);
                    return;
                }
                Err(e) => why = e,
            }
        }
        log::warn!("segment {index} failed: {why}");
        let now = self.shared.now();
        if let Ok(mut r) = self.shared.recorder.lock() {
            r.segment_failed(now);
        }
        if let Ok(mut f) = self.shared.failed.lock() {
            *f = Some(format!("Recording stopped: {why}"));
        }
    }

    fn supervise(mut self, rx: mpsc::Receiver<Ctl>) -> Result<PathBuf, String> {
        loop {
            match rx.recv_timeout(TICK) {
                Ok(Ctl::Pause(t)) => {
                    let paused = self.shared.recorder.lock().is_ok_and(|mut r| {
                        let was = r.state() == RecState::Recording;
                        r.pause(t);
                        was
                    });
                    if paused {
                        self.finish_segment();
                    }
                }
                Ok(Ctl::Resume(t)) => {
                    // Not paused any more, but the timer holds until the
                    // source gives a picture again (the portal may ask).
                    let next = self.shared.recorder.lock().ok().and_then(|mut r| {
                        (r.state() == RecState::Paused).then(|| {
                            r.resume_waiting();
                            r.next_segment()
                        })
                    });
                    if let Some(index) = next {
                        match self.open_segment(index) {
                            Ok(seg) => {
                                self.seg = Some(seg);
                                self.begin_clock();
                            }
                            Err(e) => {
                                if let Ok(mut r) = self.shared.recorder.lock() {
                                    r.segment_failed(t);
                                }
                                if let Ok(mut f) = self.shared.failed.lock() {
                                    *f = Some(format!("Recording stopped: {e}"));
                                }
                            }
                        }
                    }
                }
                Ok(Ctl::Stop(t)) => {
                    let finish = self.shared.recorder.lock().ok().map(|mut r| r.stop(t));
                    self.finish_segment();
                    let result = match finish {
                        Some(Finish::Join(list)) => self.join(&list),
                        _ => Err("The recording was lost.".into()),
                    };
                    self.remove_dir();
                    return result;
                }
                Ok(Ctl::Cancel) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if let Ok(mut r) = self.shared.recorder.lock() {
                        r.cancel();
                    }
                    self.end_segment(true);
                    self.remove_dir();
                    return Err("The recording was cancelled.".into());
                }
                #[cfg(test)]
                Ok(Ctl::Encoder(name, hardware)) => {
                    self.encoder = name;
                    self.hardware = hardware;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            self.check();
        }
    }

    /// Ends the open segment on purpose, logging if ffmpeg complained.
    fn finish_segment(&mut self) {
        if let Some((text, clean)) = self.end_segment(false)
            && !clean
        {
            log::warn!("a segment ended badly: {}", text.trim());
        }
    }

    /// Joins the kept segments that hold anything into the final file.
    fn join(&self, list: &[u32]) -> Result<PathBuf, String> {
        let names: Vec<String> = list
            .iter()
            .map(|i| self.segment_path(*i))
            .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() > 0))
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        if names.is_empty() {
            return Err("Nothing was recorded.".into());
        }
        // Names relative to the list, which ffmpeg resolves against its folder.
        let text: String = names
            .iter()
            .map(|n| format!("file '{}'\n", n.replace('\'', r"'\''")))
            .collect();
        let list_file = self.dir.join("segments.txt");
        std::fs::write(&list_file, text).map_err(|e| format!("Could not write the list: {e}"))?;
        std::fs::create_dir_all(&self.out_dir)
            .map_err(|e| format!("Could not make {}: {e}", self.out_dir.display()))?;
        let container = self.cfg.container;
        let out = unique_path(
            &self.out_dir,
            &recording_name_now(&self.cfg.name_pattern, container.ext()),
        );
        let passes = join_passes(
            &list_file,
            &out,
            container,
            self.codec,
            self.cfg.gif_colours,
            self.cfg.fps,
        );
        for args in &passes {
            if let Err(e) = run_to_end(&self.ffmpeg, args, &self.shared.abort) {
                let _ = std::fs::remove_file(&out);
                return Err(format!("The recording could not be saved: {e}"));
            }
        }
        if std::fs::metadata(&out).is_ok_and(|m| m.len() > 0) {
            Ok(out)
        } else {
            let _ = std::fs::remove_file(&out);
            Err("The recording could not be saved: the file is empty.".into())
        }
    }

    fn remove_dir(&self) {
        if let Err(e) = std::fs::remove_dir_all(&self.dir)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("could not remove {}: {e}", self.dir.display());
        }
    }
}

/// The size of the picture `source` gives.
fn source_size(source: &Source) -> (u32, u32) {
    match source {
        Source::Ddagrab { w, h, .. }
        | Source::Gdigrab { w, h, .. }
        | Source::X11grab { w, h, .. }
        | Source::RawPipe { w, h, .. } => (*w, *h),
    }
}

/// Runs ffmpeg with `args` to its end. Killed when `abort` is set, so a
/// quitting app does not leave it writing. Err holds what ffmpeg said.
fn run_to_end(ffmpeg: &Path, args: &[String], abort: &AtomicBool) -> Result<(), String> {
    let mut child = ffmpeg::command(ffmpeg)
        .args(args)
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start ffmpeg: {e}"))?;
    let stderr = child.stderr.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut s, &mut text);
            text
        })
    });
    let status = loop {
        if abort.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("the app quit".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(TICK),
            Err(e) => return Err(e.to_string()),
        }
    };
    let text = stderr.and_then(|h| h.join().ok()).unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        Err(text.trim().to_owned())
    }
}

/// The process id in a segment folder's name, `recording-<pid>-<n>`; `None`
/// for any other name.
fn segment_dir_pid(name: &str) -> Option<u32> {
    let (pid, n) = name.strip_prefix(DIR_PREFIX)?.split_once('-')?;
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    if digits(pid) && digits(n) {
        pid.parse().ok()
    } else {
        None
    }
}

/// Removes the segment folders left in `base` by a Windup that died while
/// recording (killed, a crash, a logoff): those whose process no longer
/// runs. Nothing else in the folder is touched.
fn remove_stale_in(base: &Path, running: impl Fn(u32) -> bool) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(segment_dir_pid) else {
            continue;
        };
        if pid == std::process::id() || running(pid) || !entry.file_type().is_ok_and(|t| t.is_dir())
        {
            continue;
        }
        let path = entry.path();
        match std::fs::remove_dir_all(&path) {
            Ok(()) => log::info!(
                "removed {}, left by a recording that never ended",
                path.display()
            ),
            Err(e) => log::warn!("could not remove {}: {e}", path.display()),
        }
    }
}

/// `remove_stale_in` for Windup's own cache folder. Called once at startup.
pub(crate) fn remove_stale_segments() {
    if let Some(base) = Config::cache_dir() {
        remove_stale_in(&base, process_running);
    }
}

/// Whether process `pid` runs. Where that cannot be told it is taken to,
/// so its folder is kept.
#[cfg(target_os = "windows")]
fn process_running(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, GetLastError, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: plain calls; the handle is closed before returning.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            // Another user's (or an elevated) process still exists.
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code) != 0;
        CloseHandle(h);
        !ok || code == STILL_ACTIVE as u32
    }
}

#[cfg(target_os = "linux")]
fn process_running(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn process_running(_pid: u32) -> bool {
    true
}

/// What a writer thread watches: `stop` ends its writing, `gone` (ffmpeg
/// has exited) ends its wait for ffmpeg to open the pipe.
#[derive(Clone)]
struct Flags {
    stop: Arc<AtomicBool>,
    gone: Arc<AtomicBool>,
}

fn set_error(error: &Mutex<Option<String>>, why: String) {
    if let Ok(mut e) = error.lock()
        && e.is_none()
    {
        *e = Some(why);
    }
}

/// Feeds ffmpeg a steady `fps`: each tick writes the newest frame as many
/// times as the pacer says, so a source with nothing new repeats the last.
fn write_video(
    server: PipeServer,
    mut frames: Box<dyn FrameSource>,
    fps: u32,
    flags: &Flags,
    error: &Mutex<Option<String>>,
) {
    let stop = &*flags.stop;
    // Opened even when stopping already, so ffmpeg sees the end of input
    // instead of waiting in its open until it is killed.
    let mut file = match server.connect(&flags.gone) {
        Ok(f) => f,
        Err(e) => {
            if !stop.load(Ordering::Relaxed) {
                set_error(error, format!("The frame pipe failed: {e}"));
            }
            return;
        }
    };
    let (w, h) = frames.size();
    let len = w as usize * h as usize * 4;
    // Black until the first frame arrives.
    let mut frame = vec![0u8; len];
    // The clock starts once ffmpeg reads, which is when its clock starts.
    let t0 = Instant::now();
    let mut pacer = Pacer::new(fps, 0.0);
    let result = 'feed: loop {
        if stop.load(Ordering::Relaxed) {
            break Ok(());
        }
        if let Err(e) = frames.next_frame(&mut frame) {
            break Err(e);
        }
        if frame.len() != len {
            break Err(format!("A frame had {} bytes, not {len}.", frame.len()));
        }
        for _ in 0..pacer.due(t0.elapsed().as_secs_f64()) {
            if let Err(e) = file.write_all(&frame) {
                break 'feed Err(format!("The frame pipe failed: {e}"));
            }
        }
        let next = pacer.sent as f64 / f64::from(pacer.fps);
        let wait = next - t0.elapsed().as_secs_f64();
        if wait > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(wait.min(0.05)));
        }
    };
    // A write that fails because we are stopping is not a failure.
    if let Err(e) = result
        && !stop.load(Ordering::Relaxed)
    {
        set_error(error, e);
    }
    pipe::close(file);
}

/// Copies sound to ffmpeg as it arrives. Gives the source back for the next
/// segment.
fn write_audio(
    server: PipeServer,
    mut src: Box<dyn AudioSource>,
    flags: &Flags,
    error: &Mutex<Option<String>>,
) -> Box<dyn AudioSource> {
    let stop = &*flags.stop;
    let mut file = match server.connect(&flags.gone) {
        Ok(f) => f,
        Err(e) => {
            if !stop.load(Ordering::Relaxed) {
                set_error(error, format!("The sound pipe failed: {e}"));
            }
            return src;
        }
    };
    src.restart();
    let mut samples = Vec::new();
    let mut bytes = Vec::new();
    let result = loop {
        if stop.load(Ordering::Relaxed) {
            // The segment's last sound, so it is as long as the picture. A
            // pipe ffmpeg has closed already is no failure now.
            samples.clear();
            if src.finish(&mut samples).is_ok() && !samples.is_empty() {
                bytes.clear();
                put_f32le(&samples, &mut bytes);
                let _ = file.write_all(&bytes);
            }
            break Ok(());
        }
        samples.clear();
        if let Err(e) = src.read(&mut samples) {
            break Err(e);
        }
        if samples.is_empty() {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        bytes.clear();
        put_f32le(&samples, &mut bytes);
        if let Err(e) = file.write_all(&bytes) {
            break Err(format!("The sound pipe failed: {e}"));
        }
    };
    if let Err(e) = result
        && !stop.load(Ordering::Relaxed)
    {
        set_error(error, e);
    }
    pipe::close(file);
    src
}

#[cfg(test)]
mod tests {
    use super::super::test_source::{Pattern, PatternFactory};
    use super::*;

    /// A folder under the system temp dir, removed at the end.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("windup-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The tests that record run one at a time: each one is real time, and a
    /// dozen encoders at once (more on a small CI runner) starve each other
    /// until the timings they check say more about the machine than the code.
    fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
        static TURN: Mutex<()> = Mutex::new(());
        TURN.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn ffmpeg_or_skip(test: &str) -> Option<PathBuf> {
        let f = ffmpeg::locate();
        if f.is_none() {
            eprintln!("{test}: skipped, no ffmpeg found");
        }
        f
    }

    struct Silence {
        t0: Option<Instant>,
        sent: u64,
    }

    impl AudioSource for Silence {
        fn rate(&self) -> u32 {
            48000
        }
        fn channels(&self) -> u16 {
            2
        }
        fn restart(&mut self) {
            self.t0 = None;
            self.sent = 0;
        }
        fn read(&mut self, buf: &mut Vec<f32>) -> Result<(), String> {
            let t0 = *self.t0.get_or_insert_with(Instant::now);
            let due = (t0.elapsed().as_secs_f64() * 48000.0) as u64;
            let n = due.saturating_sub(self.sent);
            self.sent += n;
            buf.extend(std::iter::repeat_n(0.0, n as usize * 2));
            Ok(())
        }
    }

    fn job(dir: &TempDir, audio: Vec<Box<dyn AudioSource>>) -> RecordJob {
        let cfg = VideoConfig {
            name_pattern: "Test".into(),
            ..VideoConfig::default()
        };
        RecordJob {
            factory: Box::new(PatternFactory { w: 64, h: 64 }),
            audio,
            cfg,
            size: (64, 64),
            out_dir: dir.0.join("out"),
        }
    }

    fn start(dir: &TempDir, job: RecordJob, encoder: (&'static str, bool)) -> Recording {
        let hooks = Hooks {
            encoder: Some(encoder),
            ..Hooks::default()
        };
        Recording::start_in(job, &dir.0.join("work"), hooks).unwrap()
    }

    /// The pattern, except that the first source opened stalls after its
    /// first frame, so a dying ffmpeg is noticed before its pipe breaks.
    struct StallOnce {
        stalled: Arc<AtomicBool>,
    }

    struct Stalling {
        inner: Pattern,
        stall: bool,
        frames: u32,
    }

    impl FrameSource for Stalling {
        fn size(&self) -> (u32, u32) {
            self.inner.size()
        }
        fn next_frame(&mut self, buf: &mut Vec<u8>) -> Result<bool, String> {
            self.frames += 1;
            if self.stall && self.frames == 2 {
                sleep(1.5);
            }
            self.inner.next_frame(buf)
        }
    }

    impl SourceFactory for StallOnce {
        fn source(&self) -> Source {
            PatternFactory { w: 64, h: 64 }.source()
        }
        fn open_pipe(&self) -> Result<Box<dyn FrameSource>, String> {
            Ok(Box::new(Stalling {
                inner: Pattern {
                    w: 64,
                    h: 64,
                    t0: Instant::now(),
                },
                stall: !self.stalled.swap(true, Ordering::Relaxed),
                frames: 0,
            }))
        }
    }

    fn fallback(dir: &TempDir, job: RecordJob, hold_check_for_writer: bool) -> Recording {
        let hooks = Hooks {
            encoder: Some(("windup_no_such_encoder", true)),
            hold_check_for_writer,
        };
        Recording::start_in(job, &dir.0.join("work"), hooks).unwrap()
    }

    fn assert_fell_back(ff: &Path, mut rec: Recording, writer_first: bool) {
        sleep(2.0);
        assert!(rec.failed().is_none(), "{:?}", rec.failed());
        assert_eq!(
            rec.shared.writer_error_seen.load(Ordering::Relaxed),
            writer_first,
            "the race went the way the test meant"
        );
        let out = rec.stop().join().unwrap().unwrap();
        let info = info(ff, &out);
        assert!(info.contains("64x64") && info.contains("h264"), "{info}");
    }

    /// What `ffmpeg -i` says about a file: metadata only, no frames.
    fn info(ffmpeg: &Path, file: &Path) -> String {
        let out = ffmpeg::command(ffmpeg)
            .args(["-hide_banner", "-i"])
            .arg(file)
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stderr).into_owned()
    }

    fn duration(info: &str) -> f64 {
        let at = info.find("Duration: ").expect("a duration") + "Duration: ".len();
        let hms: Vec<f64> = info[at..at + 11]
            .split(':')
            .map(|p| p.parse().unwrap())
            .collect();
        hms[0] * 3600.0 + hms[1] * 60.0 + hms[2]
    }

    fn files_in(dir: &Path) -> usize {
        std::fs::read_dir(dir).map_or(0, |d| d.count())
    }

    fn sleep(secs: f64) {
        std::thread::sleep(Duration::from_secs_f64(secs));
    }

    #[test]
    fn a_piped_recording_with_a_pause_is_one_file() {
        let _turn = one_at_a_time();
        let Some(ff) = ffmpeg_or_skip("a_piped_recording_with_a_pause_is_one_file") else {
            return;
        };
        let dir = TempDir::new("pause");
        let mut rec = start(&dir, job(&dir, vec![]), ("libx264", false));
        // The clock runs from the first segment's opening, a moment before
        // `start` returns, so measure the second from here.
        let before = rec.elapsed();
        sleep(1.0);
        rec.pause();
        sleep(0.4);
        // The pause is stamped when it is asked for but applied by the
        // supervisor, which a loaded machine (CI, or tests beside a build)
        // can keep busy opening the first segment for longer than the sleep.
        // What matters is the stamp, so wait for it to land.
        for _ in 0..50 {
            if rec.clock().paused() {
                break;
            }
            sleep(0.1);
        }
        let paused_at = rec.elapsed() - before;
        assert!((paused_at - 1.0).abs() < 0.2, "{paused_at}");
        rec.resume();
        sleep(1.0);
        let out = rec.stop().join().unwrap().unwrap();
        assert_eq!(out, dir.0.join("out").join("Test.mp4"));
        let info = info(&ff, &out);
        assert!(info.contains("64x64"), "{info}");
        assert!(info.contains("h264"), "{info}");
        let d = duration(&info);
        assert!((1.6..2.6).contains(&d), "two segments joined, got {d} s");
        assert!(rec.failed().is_none());
        assert_eq!(files_in(&dir.0.join("work")), 0, "segments removed");
    }

    /// The pattern behind a source that takes a while to open, as the
    /// ScreenCast portal does while its dialog is up.
    struct SlowToOpen;

    impl SourceFactory for SlowToOpen {
        fn source(&self) -> Source {
            PatternFactory { w: 64, h: 64 }.source()
        }
        fn open_pipe(&self) -> Result<Box<dyn FrameSource>, String> {
            sleep(1.5);
            PatternFactory { w: 64, h: 64 }.open_pipe()
        }
    }

    #[test]
    fn the_timer_does_not_count_a_source_that_is_slow_to_open() {
        let _turn = one_at_a_time();
        if ffmpeg_or_skip("the_timer_does_not_count_a_source_that_is_slow_to_open").is_none() {
            return;
        }
        let dir = TempDir::new("slow-source");
        let mut j = job(&dir, vec![]);
        j.factory = Box::new(SlowToOpen);
        let mut rec = start(&dir, j, ("libx264", false));
        assert!(rec.elapsed() < 0.5, "{}", rec.elapsed());
        sleep(0.5);
        rec.pause();
        for _ in 0..50 {
            if rec.clock().paused() {
                break;
            }
            sleep(0.1);
        }
        let paused_at = rec.elapsed();
        rec.resume();
        // The resume waits on the source; the timer holds meanwhile.
        sleep(1.0);
        assert!(!rec.clock().paused());
        assert!(
            rec.elapsed() - paused_at < 0.1,
            "{paused_at} -> {}",
            rec.elapsed()
        );
        sleep(1.0);
        assert!(rec.elapsed() > paused_at, "running again");
        rec.cancel();
    }

    #[test]
    fn cancel_leaves_no_file_and_no_temp_folder() {
        let _turn = one_at_a_time();
        if ffmpeg_or_skip("cancel_leaves_no_file_and_no_temp_folder").is_none() {
            return;
        }
        let dir = TempDir::new("cancel");
        let mut rec = start(&dir, job(&dir, vec![]), ("libx264", false));
        sleep(0.8);
        assert_eq!(files_in(&dir.0.join("work")), 1, "a segment folder");
        rec.cancel();
        assert_eq!(files_in(&dir.0.join("work")), 0);
        assert_eq!(files_in(&dir.0.join("out")), 0);
        // Stop after cancel does not resurrect the cancelled segments.
        assert!(rec.stop().join().unwrap().is_err());
        assert_eq!(files_in(&dir.0.join("out")), 0);
    }

    #[test]
    fn ffmpeg_dying_keeps_what_was_recorded() {
        let _turn = one_at_a_time();
        let Some(ff) = ffmpeg_or_skip("ffmpeg_dying_keeps_what_was_recorded") else {
            return;
        };
        let dir = TempDir::new("dying");
        let mut rec = start(&dir, job(&dir, vec![]), ("libx264", false));
        sleep(1.0);
        rec.set_encoder("windup_no_such_encoder", false);
        rec.pause();
        rec.resume();
        let deadline = Instant::now() + Duration::from_secs(5);
        while rec.failed().is_none() && Instant::now() < deadline {
            sleep(0.05);
        }
        assert!(rec.failed().is_some());
        let out = rec.stop().join().unwrap().unwrap();
        let info = info(&ff, &out);
        assert!(info.contains("64x64"), "{info}");
        assert!(duration(&info) > 0.6, "{info}");
        assert_eq!(files_in(&dir.0.join("work")), 0);
    }

    #[test]
    fn a_hardware_encoder_that_fails_at_once_falls_back_to_software() {
        let _turn = one_at_a_time();
        let Some(ff) = ffmpeg_or_skip("a_hardware_encoder_that_fails_at_once_falls_back") else {
            return;
        };
        // ffmpeg's exit noticed first: the writer is stalled.
        let dir = TempDir::new("fallback-exit");
        let mut j = job(&dir, vec![]);
        j.factory = Box::new(StallOnce {
            stalled: Arc::new(AtomicBool::new(false)),
        });
        assert_fell_back(&ff, fallback(&dir, j, false), false);

        // The broken pipe noticed first: the check waits for it.
        let dir = TempDir::new("fallback-writer");
        let j = job(&dir, vec![]);
        assert_fell_back(&ff, fallback(&dir, j, true), true);
    }

    /// A screen source ffmpeg cannot open (no such X display, or no x11grab
    /// at all), with the pattern as its fallback.
    struct BrokenSource;

    impl SourceFactory for BrokenSource {
        fn source(&self) -> Source {
            Source::X11grab {
                display: "windup-no-such-display".into(),
                x: 0,
                y: 0,
                w: 64,
                h: 64,
            }
        }
        fn open_pipe(&self) -> Result<Box<dyn FrameSource>, String> {
            PatternFactory { w: 64, h: 64 }.open_pipe()
        }
        fn fallback(&self) -> Option<Source> {
            Some(PatternFactory { w: 64, h: 64 }.source())
        }
    }

    #[test]
    fn a_source_that_fails_at_once_falls_back_to_its_fallback() {
        let _turn = one_at_a_time();
        let Some(ff) = ffmpeg_or_skip("a_source_that_fails_at_once_falls_back") else {
            return;
        };
        let dir = TempDir::new("source-fallback");
        let mut j = job(&dir, vec![]);
        j.factory = Box::new(BrokenSource);
        let mut rec = start(&dir, j, ("libx264", false));
        sleep(2.0);
        assert!(rec.failed().is_none(), "{:?}", rec.failed());
        let out = rec.stop().join().unwrap().unwrap();
        let text = info(&ff, &out);
        assert!(text.contains("64x64") && text.contains("h264"), "{text}");

        // Independent of the encoder fallback: both can happen in turn.
        let dir = TempDir::new("source-and-encoder-fallback");
        let mut j = job(&dir, vec![]);
        j.factory = Box::new(BrokenSource);
        let mut rec = fallback(&dir, j, false);
        sleep(4.0);
        assert!(rec.failed().is_none(), "{:?}", rec.failed());
        let out = rec.stop().join().unwrap().unwrap();
        let text = info(&ff, &out);
        assert!(text.contains("64x64") && text.contains("h264"), "{text}");
    }

    /// A working source (the pattern) whose fallback is one ffmpeg cannot
    /// open, counting the segments that opened the working one.
    struct WorkingSource {
        opened: Arc<AtomicU32>,
    }

    impl SourceFactory for WorkingSource {
        fn source(&self) -> Source {
            PatternFactory { w: 64, h: 64 }.source()
        }
        fn open_pipe(&self) -> Result<Box<dyn FrameSource>, String> {
            self.opened.fetch_add(1, Ordering::Relaxed);
            PatternFactory { w: 64, h: 64 }.open_pipe()
        }
        fn fallback(&self) -> Option<Source> {
            BrokenSource.source().into()
        }
    }

    #[test]
    fn a_failing_hardware_encoder_keeps_a_working_source() {
        let _turn = one_at_a_time();
        let Some(ff) = ffmpeg_or_skip("a_failing_hardware_encoder_keeps_a_working_source") else {
            return;
        };
        let dir = TempDir::new("encoder-not-source");
        let opened = Arc::new(AtomicU32::new(0));
        let mut j = job(&dir, vec![]);
        j.factory = Box::new(WorkingSource {
            opened: Arc::clone(&opened),
        });
        let mut rec = fallback(&dir, j, false);
        sleep(2.0);
        assert!(rec.failed().is_none(), "{:?}", rec.failed());
        let out = rec.stop().join().unwrap().unwrap();
        // The first segment and its retry with the software encoder both
        // read the original source; its (broken) fallback was never used.
        assert!(opened.load(Ordering::Relaxed) >= 2);
        let text = info(&ff, &out);
        assert!(text.contains("64x64") && text.contains("h264"), "{text}");
    }

    #[test]
    fn a_recording_that_never_worked_saves_nothing() {
        let _turn = one_at_a_time();
        if ffmpeg_or_skip("a_recording_that_never_worked_saves_nothing").is_none() {
            return;
        }
        let dir = TempDir::new("nothing");
        let mut rec = start(&dir, job(&dir, vec![]), ("windup_no_such_encoder", false));
        let deadline = Instant::now() + Duration::from_secs(5);
        while rec.failed().is_none() && Instant::now() < deadline {
            sleep(0.05);
        }
        assert!(rec.failed().is_some());
        let err = rec.stop().join().unwrap().unwrap_err();
        assert_eq!(err, "Nothing was recorded.");
        assert_eq!(files_in(&dir.0.join("out")), 0);
        assert_eq!(files_in(&dir.0.join("work")), 0);
    }

    #[test]
    fn a_silent_audio_source_is_muxed() {
        let _turn = one_at_a_time();
        let Some(ff) = ffmpeg_or_skip("a_silent_audio_source_is_muxed") else {
            return;
        };
        let dir = TempDir::new("audio");
        let silence: Box<dyn AudioSource> = Box::new(Silence { t0: None, sent: 0 });
        let mut rec = start(&dir, job(&dir, vec![silence]), ("libx264", false));
        sleep(1.2);
        let out = rec.stop().join().unwrap().unwrap();
        let info = info(&ff, &out);
        assert!(info.contains("64x64"), "{info}");
        assert!(info.contains("Audio: aac"), "{info}");
        // Timed from zero, not from 1970: a sound track stamped with the
        // wall clock made the MP4 decades long, and GStreamer players
        // (COSMIC's, GNOME's) found nothing in it to play.
        let d = duration(&info);
        assert!((0.8..2.5).contains(&d), "{d} s: {info}");
        let at = info.find("start: ").expect("a start") + "start: ".len();
        let start: f64 = info[at..]
            .split(',')
            .next()
            .and_then(|s| s.trim().parse().ok())
            .expect("a start time");
        assert!(start.abs() < 0.5, "starts at {start} s");
    }

    /// Silence that counts how often a segment restarted it.
    struct Restarts {
        inner: Silence,
        count: Arc<AtomicU32>,
    }

    impl AudioSource for Restarts {
        fn rate(&self) -> u32 {
            self.inner.rate()
        }
        fn channels(&self) -> u16 {
            self.inner.channels()
        }
        fn read(&mut self, buf: &mut Vec<f32>) -> Result<(), String> {
            self.inner.read(buf)
        }
        fn restart(&mut self) {
            self.count.fetch_add(1, Ordering::Relaxed);
            self.inner.restart();
        }
    }

    #[test]
    fn each_segment_restarts_its_sound() {
        let _turn = one_at_a_time();
        let Some(ff) = ffmpeg_or_skip("each_segment_restarts_its_sound") else {
            return;
        };
        let dir = TempDir::new("audio-pause");
        let count = Arc::new(AtomicU32::new(0));
        let src: Box<dyn AudioSource> = Box::new(Restarts {
            inner: Silence { t0: None, sent: 0 },
            count: Arc::clone(&count),
        });
        let mut rec = start(&dir, job(&dir, vec![src]), ("libx264", false));
        sleep(1.0);
        rec.pause();
        sleep(1.0);
        rec.resume();
        sleep(1.0);
        let out = rec.stop().join().unwrap().unwrap();
        assert_eq!(count.load(Ordering::Relaxed), 2, "once per segment");
        let info = info(&ff, &out);
        assert!(info.contains("Audio: aac"), "{info}");
        // The pause is not in the sound: had the source kept its clock, the
        // second segment would start with a second of catching up.
        let d = duration(&info);
        assert!((1.6..2.6).contains(&d), "{d} s");
    }

    /// When the sync test's picture flashes white and its sound clicks,
    /// in seconds after the test starts.
    const MARKS: [f64; 4] = [0.8, 1.6, 3.4, 4.2];
    const MARK_LEN: f64 = 0.15;
    /// From here on the fake device sends only the clicks, nothing between
    /// them, as WASAPI loopback does while nothing plays.
    const QUIET_FROM: f64 = 2.0;
    const SYNC_RATE: u32 = 48000;

    fn marked(t: f64) -> bool {
        MARKS.iter().any(|m| (*m..m + MARK_LEN).contains(&t))
    }

    /// Black, white during the marks.
    struct Flash {
        epoch: Instant,
    }

    impl FrameSource for Flash {
        fn size(&self) -> (u32, u32) {
            (64, 64)
        }
        fn next_frame(&mut self, buf: &mut Vec<u8>) -> Result<bool, String> {
            let v = if marked(self.epoch.elapsed().as_secs_f64()) {
                255
            } else {
                0
            };
            buf.clear();
            buf.extend(std::iter::repeat_n([v, v, v, 255], 64 * 64).flatten());
            Ok(true)
        }
    }

    struct FlashFactory {
        epoch: Instant,
    }

    impl SourceFactory for FlashFactory {
        fn source(&self) -> Source {
            PatternFactory { w: 64, h: 64 }.source()
        }
        fn open_pipe(&self) -> Result<Box<dyn FrameSource>, String> {
            Ok(Box::new(Flash { epoch: self.epoch }))
        }
    }

    /// A fake sound device on a thread of its own, pushing 10 ms periods
    /// into a `Captured` source: a 1 kHz click during the marks, silence
    /// between them until `QUIET_FROM`, nothing at all after it.
    struct FakeDevice {
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl Drop for FakeDevice {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    fn clicking_source(epoch: Instant) -> Box<dyn AudioSource> {
        use crate::video::audio::{Captured, ring};
        let shared = ring(SYNC_RATE, 2);
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (shared, stop) = (Arc::clone(&shared), Arc::clone(&stop));
            std::thread::spawn(move || {
                let mut next = 0u64;
                let mut period = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    sleep(0.01);
                    let now = (epoch.elapsed().as_secs_f64() * f64::from(SYNC_RATE)) as u64;
                    period.clear();
                    for k in next..now {
                        let t = k as f64 / f64::from(SYNC_RATE);
                        let v = if marked(t) {
                            0.5 * (std::f64::consts::TAU * 1000.0 * t).sin() as f32
                        } else if t < QUIET_FROM {
                            0.0
                        } else {
                            continue;
                        };
                        period.extend([v, v]);
                    }
                    next = now;
                    if !period.is_empty() {
                        shared.lock().unwrap().push(&period);
                    }
                }
            })
        };
        let device = FakeDevice {
            stop,
            thread: Some(thread),
        };
        Box::new(Captured::new(
            SYNC_RATE,
            2,
            shared,
            Arc::default(),
            Box::new(device),
        ))
    }

    /// What ffmpeg logs running `filter` over one stream of `file`.
    fn analyse(ffmpeg: &Path, file: &Path, filter: &[&str]) -> String {
        let out = ffmpeg::command(ffmpeg)
            .args(["-hide_banner", "-i"])
            .arg(file)
            .args(filter)
            .args(["-f", "null", "-"])
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stderr).into_owned()
    }

    /// The times the picture turns white.
    fn flash_onsets(ffmpeg: &Path, file: &Path) -> Vec<f64> {
        let log = analyse(
            ffmpeg,
            file,
            &[
                "-an",
                "-vf",
                "signalstats,metadata=print:key=lavfi.signalstats.YAVG",
            ],
        );
        let (mut onsets, mut at, mut lit) = (Vec::new(), 0.0, false);
        for line in log.lines() {
            if let Some(i) = line.find("pts_time:") {
                at = line[i + 9..].trim().parse().unwrap_or(at);
            } else if let Some(i) = line.find("YAVG=") {
                let y: f64 = line[i + 5..].trim().parse().unwrap_or(0.0);
                if y > 128.0 && !lit {
                    onsets.push(at);
                }
                lit = y > 128.0;
            }
        }
        onsets
    }

    /// The times the sound starts after a stretch of silence.
    fn click_onsets(ffmpeg: &Path, file: &Path) -> Vec<f64> {
        let log = analyse(
            ffmpeg,
            file,
            &["-vn", "-af", "silencedetect=n=-30dB:d=0.05"],
        );
        log.lines()
            .filter_map(|l| {
                let i = l.find("silence_end: ")? + 13;
                l[i..].split_whitespace().next()?.parse().ok()
            })
            .collect()
    }

    #[test]
    fn sound_and_picture_stay_in_step_through_quiet() {
        let _turn = one_at_a_time();
        let Some(ff) = ffmpeg_or_skip("sound_and_picture_stay_in_step_through_quiet") else {
            return;
        };
        let dir = TempDir::new("sync");
        let epoch = Instant::now();
        let mut j = job(&dir, vec![clicking_source(epoch)]);
        j.factory = Box::new(FlashFactory { epoch });
        j.cfg.fps = 60;
        let mut rec = start(&dir, j, ("libx264", false));
        sleep(5.0 - epoch.elapsed().as_secs_f64());
        let out = rec.stop().join().unwrap().unwrap();
        let flashes = flash_onsets(&ff, &out);
        let text = info(&ff, &out);
        // silencedetect also ends the silence at the end of the file.
        let end = duration(&text);
        let clicks: Vec<f64> = click_onsets(&ff, &out)
            .into_iter()
            .filter(|t| *t < end - 0.1)
            .collect();
        eprintln!(
            "flashes {flashes:?}
clicks  {clicks:?}"
        );
        assert_eq!(flashes.len(), MARKS.len(), "{flashes:?}");
        assert_eq!(clicks.len(), MARKS.len(), "{clicks:?}");
        for (f, c) in flashes.iter().zip(&clicks) {
            eprintln!("click - flash = {:+.3} s", c - f);
            assert!((c - f).abs() < 0.05, "flash {f}, click {c}");
        }
        assert!(text.contains("Audio: aac"), "{text}");
    }

    #[test]
    fn a_gif_is_joined_with_a_palette() {
        let _turn = one_at_a_time();
        let Some(ff) = ffmpeg_or_skip("a_gif_is_joined_with_a_palette") else {
            return;
        };
        let dir = TempDir::new("gif");
        let mut j = job(&dir, vec![]);
        j.cfg.container = Container::Gif;
        j.cfg.fps = 60;
        let mut rec = start(&dir, j, ("libx264", false));
        sleep(0.8);
        rec.pause();
        sleep(0.2);
        rec.resume();
        sleep(0.8);
        let out = rec.stop().join().unwrap().unwrap();
        assert_eq!(out.extension().unwrap(), "gif");
        let info = info(&ff, &out);
        assert!(info.contains("gif") && info.contains("64x64"), "{info}");
        // Recorded at 60, written at 50: browsers slow 1/100 s delays.
        assert!(info.contains("50 fps"), "{info}");
        assert_eq!(files_in(&dir.0.join("work")), 0, "palette removed too");
    }

    #[test]
    fn h265_in_mp4_is_tagged_hvc1() {
        let _turn = one_at_a_time();
        let Some(ff) = ffmpeg_or_skip("h265_in_mp4_is_tagged_hvc1") else {
            return;
        };
        let dir = TempDir::new("hvc1");
        let mut j = job(&dir, vec![]);
        j.cfg.codec = Codec::H265;
        let mut rec = start(&dir, j, ("libx265", false));
        sleep(0.8);
        let out = rec.stop().join().unwrap().unwrap();
        let info = info(&ff, &out);
        assert!(info.contains("hevc") && info.contains("(hvc1 /"), "{info}");
    }

    /// The pattern at 64x64 for the first segment and 32x32 after: a
    /// Wayland scale change during a pause.
    struct Shrinks {
        opened: AtomicU32,
    }

    impl SourceFactory for Shrinks {
        fn source(&self) -> Source {
            PatternFactory { w: 64, h: 64 }.source()
        }
        fn open_pipe(&self) -> Result<Box<dyn FrameSource>, String> {
            let side = if self.opened.fetch_add(1, Ordering::Relaxed) == 0 {
                64
            } else {
                32
            };
            PatternFactory { w: side, h: side }.open_pipe()
        }
    }

    #[test]
    fn a_segment_of_another_size_ends_the_recording_and_keeps_the_rest() {
        let _turn = one_at_a_time();
        let Some(ff) = ffmpeg_or_skip("a_segment_of_another_size_ends_the_recording") else {
            return;
        };
        let dir = TempDir::new("resized");
        let mut j = job(&dir, vec![]);
        j.factory = Box::new(Shrinks {
            opened: AtomicU32::new(0),
        });
        let mut rec = start(&dir, j, ("libx264", false));
        sleep(1.0);
        rec.pause();
        rec.resume();
        let deadline = Instant::now() + Duration::from_secs(5);
        while rec.failed().is_none() && Instant::now() < deadline {
            sleep(0.05);
        }
        assert_eq!(
            rec.failed().as_deref(),
            Some(format!("Recording stopped: {SIZE_CHANGED}").as_str())
        );
        let out = rec.stop().join().unwrap().unwrap();
        let info = info(&ff, &out);
        assert!(info.contains("64x64") && !info.contains("32x32"), "{info}");
        assert_eq!(files_in(&dir.0.join("work")), 0);
    }

    #[test]
    fn an_abort_kills_a_join_under_way() {
        let _turn = one_at_a_time();
        let Some(ff) = ffmpeg_or_skip("an_abort_kills_a_join_under_way") else {
            return;
        };
        let args: Vec<String> = [
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=64x64:rate=30",
            "-f",
            "null",
            "-",
        ]
        .map(String::from)
        .to_vec();
        let abort = Arc::new(AtomicBool::new(false));
        let t0 = Instant::now();
        let flag = Arc::clone(&abort);
        let worker = std::thread::spawn(move || run_to_end(&ff, &args, &flag));
        sleep(0.3);
        abort.store(true, Ordering::Relaxed);
        let result = worker.join().unwrap();
        assert!(result.is_err());
        assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
    }

    #[test]
    fn only_segment_folders_are_named_as_such() {
        assert_eq!(segment_dir_pid("recording-1234-0"), Some(1234));
        assert_eq!(segment_dir_pid("recording-1-17"), Some(1));
        for other in [
            "recording-",
            "recording-12",
            "recording-12-",
            "recording--3",
            "recording-ab-3",
            "recording-12-3x",
            "recording-12-3-4",
            "Recording-12-3",
            "probe.json",
            "xrecording-12-3",
        ] {
            assert_eq!(segment_dir_pid(other), None, "{other}");
        }
    }

    #[test]
    fn segment_folders_of_processes_gone_are_removed_at_startup() {
        let dir = TempDir::new("stale");
        let own = std::process::id();
        let names = [
            "recording-4000000001-0".to_owned(),
            "recording-123-1".to_owned(),
            format!("recording-{own}-0"),
            "recording-abc-1".to_owned(),
            "themes".to_owned(),
        ];
        for n in &names {
            std::fs::create_dir_all(dir.0.join(n).join("inner")).unwrap();
        }
        // A file of that name is not a segment folder.
        std::fs::write(dir.0.join("recording-4000000002-0"), b"x").unwrap();
        remove_stale_in(&dir.0, |pid| pid == 123);
        assert!(!dir.0.join(&names[0]).exists(), "its process is gone");
        for kept in &names[1..] {
            assert!(dir.0.join(kept).exists(), "{kept}");
        }
        assert!(dir.0.join("recording-4000000002-0").exists());
    }
}
