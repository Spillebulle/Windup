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
use windup_core::ipc::RecordTest;
use windup_core::video::{
    AudioIn, Bitrate, Codec, Container, Finish, Pacer, RecState, Recorder, SegmentJob, Source,
    auto_bitrate, candidates, codecs_for, even_size, join_args, pick_encoder, segment_args,
    segment_ext,
};

use super::ffmpeg::{self, PROBE, ProbeState};
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
    fn open_pipe(&self) -> Option<Box<dyn FrameSource>>;
}

/// Sound we capture ourselves, written to ffmpeg as interleaved f32le.
pub(crate) trait AudioSource: Send {
    fn rate(&self) -> u32;
    fn channels(&self) -> u16;
    /// Appends the samples that arrived since the last call; none is fine.
    fn read(&mut self, buf: &mut Vec<f32>) -> Result<(), String>;
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
// Task 13 starts recordings from the app and is the caller.
#[allow(dead_code)]
pub(crate) fn out_dir(cfg: &VideoConfig) -> Option<PathBuf> {
    cfg.save_dir
        .clone()
        .or_else(windup_core::capture::output::default_video_dir)
}

/// How long ffmpeg gets to finish a segment before it is killed.
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);
/// A hardware encoder that dies this soon did not work at all.
const FALLBACK_WINDOW: Duration = Duration::from_secs(1);
const TICK: Duration = Duration::from_millis(20);

/// Numbers the segment folders of this process.
static NEXT_DIR: AtomicU32 = AtomicU32::new(0);

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
        Self::start_in(job, &base, None)
    }

    /// `start` with the segments under `base`, and for tests an encoder
    /// (name, hardware) instead of the probe's choice.
    fn start_in(
        job: RecordJob,
        base: &Path,
        encoder: Option<(&'static str, bool)>,
    ) -> Result<Recording, String> {
        let ffmpeg = ffmpeg::locate().ok_or("ffmpeg was not found.")?;
        let cfg = job.cfg;
        let container = cfg.container;
        let codecs = codecs_for(container);
        let codec = if codecs.contains(&cfg.codec) || codecs.is_empty() {
            cfg.codec
        } else {
            codecs[0]
        };
        let (encoder, hardware) = match encoder {
            Some(e) => e,
            None if container == Container::Gif => (software(codec), false),
            None => choose_encoder(codec, cfg.encoder.as_deref())?,
        };
        let (w, h) = even_size(job.size.0, job.size.1).ok_or("The area is too small to record.")?;
        let bitrate = match (container, cfg.bitrate) {
            (Container::Gif, _) => None,
            (_, Bitrate::Auto) => Some(auto_bitrate(w, h, cfg.fps, codec)),
            (_, Bitrate::Kbps(n)) => Some(n),
        };
        let n = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = base.join(format!("recording-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not make {}: {e}", dir.display()))?;

        let shared = Arc::new(Shared {
            t0: Instant::now(),
            recorder: Mutex::new(Recorder::start(0.0)),
            failed: Mutex::new(None),
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
            bitrate,
            seg: None,
            shared: Arc::clone(&shared),
        };
        match run.open_segment(0) {
            Ok(seg) => run.seg = Some(seg),
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

    // Task 13's recording bar pauses and resumes.
    #[allow(dead_code)]
    pub(crate) fn pause(&self) {
        let _ = self.tx.send(Ctl::Pause(self.shared.now()));
    }

    #[allow(dead_code)]
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
    // Task 13's timer reads it.
    #[allow(dead_code)]
    pub(crate) fn elapsed(&self) -> f64 {
        let now = self.shared.now();
        self.shared.recorder.lock().map_or(0.0, |r| r.elapsed(now))
    }

    /// Set when a segment died; what was recorded before it is still kept.
    pub(crate) fn failed(&self) -> Option<String> {
        self.shared.failed.lock().ok().and_then(|f| f.clone())
    }

    #[cfg(test)]
    fn set_encoder(&self, name: &'static str, hardware: bool) {
        let _ = self.tx.send(Ctl::Encoder(name, hardware));
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
    bitrate: Option<u32>,
    seg: Option<Segment>,
    shared: Arc<Shared>,
}

impl Run {
    fn segment_path(&self, index: u32) -> PathBuf {
        self.dir
            .join(format!("seg-{index}.{}", segment_ext(self.cfg.container)))
    }

    fn open_segment(&mut self, index: u32) -> Result<Segment, String> {
        let fps = self.cfg.fps.max(1);
        let gif = self.cfg.container == Container::Gif;

        let mut pipes = Vec::new();
        let mut video_pipe = None;
        let source = match self.factory.source() {
            Source::RawPipe { .. } => {
                let frames = self
                    .factory
                    .open_pipe()
                    .ok_or("The screen could not be read.")?;
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
        };
        let args = segment_args(&job, self.cfg.container);
        log::debug!("segment {index}: ffmpeg {}", args.join(" "));
        let mut child = ffmpeg::command(&self.ffmpeg)
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
        let error = Arc::new(Mutex::new(None));
        let video = video_pipe.map(|(server, frames)| {
            let (stop, error) = (Arc::clone(&stop), Arc::clone(&error));
            std::thread::spawn(move || write_video(server, frames, fps, &stop, &error))
        });
        let audio = audio_pipes
            .into_iter()
            .map(|(i, server)| {
                let src = self.audio[i].take();
                let (stop, error) = (Arc::clone(&stop), Arc::clone(&error));
                let h = std::thread::spawn(move || {
                    src.map(|src| write_audio(server, src, &stop, &error))
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
    /// at once is swapped for the software one and the segment started
    /// again; anything else ends the recording, keeping what was recorded.
    fn check(&mut self) {
        let Some(seg) = self.seg.as_mut() else {
            return;
        };
        let exited = seg.child.try_wait().ok().flatten();
        let writer = seg.error.lock().ok().and_then(|mut e| e.take());
        if exited.is_none() && writer.is_none() {
            return;
        }
        let index = seg.index;
        let early = seg.started.elapsed() < FALLBACK_WINDOW;
        let (text, _) = self.end_segment(false).unwrap_or_default();
        let mut why = writer.clone().unwrap_or_else(|| {
            text.lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_owned())
                .unwrap_or_else(|| match exited {
                    Some(s) => format!("ffmpeg stopped ({s})."),
                    None => "ffmpeg stopped.".into(),
                })
        });
        if early && writer.is_none() && self.hardware && !self.fell_back {
            let sw = software(self.codec);
            log::warn!(
                "segment {index}: {} failed at once ({why}); using {sw} from here on",
                self.encoder
            );
            self.encoder = sw;
            self.hardware = false;
            self.fell_back = true;
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
                    let next = self.shared.recorder.lock().ok().and_then(|mut r| {
                        (r.state() == RecState::Paused).then(|| {
                            r.resume(t);
                            r.next_segment()
                        })
                    });
                    if let Some(index) = next {
                        match self.open_segment(index) {
                            Ok(seg) => self.seg = Some(seg),
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
        let args = join_args(
            &list_file,
            &out,
            container,
            self.cfg.gif_colours,
            self.cfg.fps,
        );
        let output = ffmpeg::command(&self.ffmpeg)
            .args(&args)
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("Could not start ffmpeg: {e}"))?;
        let written = std::fs::metadata(&out).is_ok_and(|m| m.len() > 0);
        if output.status.success() && written {
            Ok(out)
        } else {
            let _ = std::fs::remove_file(&out);
            let text = String::from_utf8_lossy(&output.stderr);
            Err(format!("The recording could not be saved: {}", text.trim()))
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
    stop: &AtomicBool,
    error: &Mutex<Option<String>>,
) {
    let mut file = match server.connect(stop) {
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
    stop: &AtomicBool,
    error: &Mutex<Option<String>>,
) -> Box<dyn AudioSource> {
    let mut file = match server.connect(stop) {
        Ok(f) => f,
        Err(e) => {
            if !stop.load(Ordering::Relaxed) {
                set_error(error, format!("The sound pipe failed: {e}"));
            }
            return src;
        }
    };
    let mut samples = Vec::new();
    let mut bytes = Vec::new();
    let result = loop {
        if stop.load(Ordering::Relaxed) {
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
        bytes.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
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

/// A moving colour gradient: what `--record-test` records, so a test run
/// never reads the screen.
struct Pattern {
    w: u32,
    h: u32,
    t0: Instant,
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

struct PatternFactory {
    w: u32,
    h: u32,
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

/// `windup --record-test <out> --seconds N`: records a test pattern of the
/// area's size (320×240 without one) through the real pipeline and writes
/// it to `<out>`. Exit 0 when the file is there and not empty, 1 when the
/// recording failed, 2 without ffmpeg or with a bad size.
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
        factory: Box::new(PatternFactory { w, h }),
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

#[cfg(test)]
mod tests {
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
        Recording::start_in(job, &dir.0.join("work"), Some(encoder)).unwrap()
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
        let Some(ff) = ffmpeg_or_skip("a_piped_recording_with_a_pause_is_one_file") else {
            return;
        };
        let dir = TempDir::new("pause");
        let mut rec = start(&dir, job(&dir, vec![]), ("libx264", false));
        sleep(1.0);
        rec.pause();
        sleep(0.4);
        assert!((rec.elapsed() - 1.0).abs() < 0.2, "{}", rec.elapsed());
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

    #[test]
    fn cancel_leaves_no_file_and_no_temp_folder() {
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
        let Some(ff) = ffmpeg_or_skip("a_hardware_encoder_that_fails_at_once_falls_back") else {
            return;
        };
        let dir = TempDir::new("fallback");
        let mut rec = start(&dir, job(&dir, vec![]), ("windup_no_such_encoder", true));
        sleep(1.5);
        assert!(rec.failed().is_none(), "{:?}", rec.failed());
        let out = rec.stop().join().unwrap().unwrap();
        let info = info(&ff, &out);
        assert!(info.contains("64x64") && info.contains("h264"), "{info}");
    }

    #[test]
    fn a_recording_that_never_worked_saves_nothing() {
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
    }

    #[test]
    fn a_gif_is_joined_with_a_palette() {
        let Some(ff) = ffmpeg_or_skip("a_gif_is_joined_with_a_palette") else {
            return;
        };
        let dir = TempDir::new("gif");
        let mut j = job(&dir, vec![]);
        j.cfg.container = Container::Gif;
        j.cfg.fps = 15;
        let mut rec = start(&dir, j, ("libx264", false));
        sleep(0.8);
        let out = rec.stop().join().unwrap().unwrap();
        assert_eq!(out.extension().unwrap(), "gif");
        let info = info(&ff, &out);
        assert!(info.contains("gif") && info.contains("64x64"), "{info}");
    }
}
