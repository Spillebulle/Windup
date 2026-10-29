//! A recording's frames from `ext_image_copy_capture_v1` (COSMIC) or
//! `zwlr_screencopy_v1` (Sway, Hyprland), one stream per segment.
//!
//! Like the stills (`wayland::screencopy`), a stream makes a Wayland
//! connection of its own, on a thread of its own, so nothing it binds is
//! left on the service connection and a slow compositor never holds up the
//! service thread. Every output the recorded rectangle crosses is copied
//! whole into a shared-memory buffer, read back with `pread` and placed in
//! the picture by a `Composer`, whose mapping is worked out once.
//!
//! wlr: each frame is a `capture_output` of the whole output, then cropped
//! by the Composer, rather than `capture_output_region` of the crossed part.
//! The region's buffer is in the output's transformed, scaled pixels with
//! its own rounding, and differs per compositor; the whole output is the
//! same buffer ext gives, so both protocols share one mapping. The cost is
//! copying the parts of an output the rectangle leaves out.
//!
//! Cursors are painted (ext `PaintCursors`, wlr `overlay_cursor = 1`): a
//! recording shows the pointer. The next frame of an output is asked for
//! only once the last picture was taken, so the compositor is never made to
//! copy faster than the encoder reads.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::{delegate_output, delegate_registry, registry_handlers};
use wayland_client::protocol::{wl_buffer, wl_output, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1::{self as ext_frame, ExtImageCopyCaptureFrameV1},
    ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options},
    ext_image_copy_capture_session_v1::{self as ext_session, ExtImageCopyCaptureSessionV1},
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self as wlr_frame, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};
use windup_core::capture::geometry::{DRect, Transform};
use windup_core::video::compose::Buffer;
use windup_core::video::{Composer, Source, even_size};

use crate::capture::CaptureError;
use crate::capture::pixels::ShmFormat;
use crate::linux::wayland::screencopy::{
    Method, OutputFacts, ShmBuffer, connect, map_format, map_transform, outputs_of, read_events,
    silent, wait_until,
};
use crate::video::run::{FrameSource, SourceFactory};

/// Longest the compositor may take to say what buffers it wants.
const SETUP_TIMEOUT: Duration = Duration::from_secs(3);
/// How long the stream thread waits for events before looking at its stop
/// flag again.
const POLL: Duration = Duration::from_millis(50);
/// The same while an output waits for the last picture to be taken: that
/// is no Wayland event, so a long wait would hold the next frame back.
const POLL_TAKING: Duration = Duration::from_millis(2);
/// After a failed copy, how long to let a vanished output's removal arrive,
/// so the error says what happened.
const SETTLE: Duration = Duration::from_millis(200);

const DISCONNECTED: &str = "A monitor was disconnected.";

fn reason(e: CaptureError) -> String {
    match e {
        CaptureError::Failed(why) => why,
        other => other.message().to_owned(),
    }
}

/// The recorded rectangle through screencopy: what `--record-test` and the
/// recording use on COSMIC and wlroots.
pub(crate) struct WaylandFactory {
    method: Method,
    rect: DRect,
    /// The logical size, for `source` before a stream knows the real one
    /// (the recording takes the stream's own `size`).
    size: (u32, u32),
}

impl WaylandFactory {
    /// `rect` in desktop (logical) units. `None` when it is too small.
    pub(crate) fn new(method: Method, rect: DRect) -> Option<WaylandFactory> {
        let rect = rect.normalised();
        let size = even_size(rect.w.round() as u32, rect.h.round() as u32)?;
        Some(WaylandFactory { method, rect, size })
    }
}

impl SourceFactory for WaylandFactory {
    fn source(&self) -> Source {
        Source::RawPipe {
            path: String::new(),
            w: self.size.0,
            h: self.size.1,
        }
    }

    fn open_pipe(&self) -> Option<Box<dyn FrameSource>> {
        match ScreencopyStream::open(self.method, self.rect) {
            Ok(s) => Some(Box::new(s)),
            Err(e) => {
                log::warn!("screencopy stream: {e}");
                None
            }
        }
    }
}

/// The newest picture, handed from the stream thread to `next_frame`.
#[derive(Default)]
struct Latest {
    frame: Vec<u8>,
    fresh: bool,
    error: Option<String>,
}

pub(crate) struct ScreencopyStream {
    size: (u32, u32),
    latest: Arc<Mutex<Latest>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ScreencopyStream {
    /// Starts the stream thread and waits until it knows the picture's size.
    fn open(method: Method, rect: DRect) -> Result<ScreencopyStream, String> {
        let latest = Arc::new(Mutex::new(Latest::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let thread = {
            let (latest, stop) = (Arc::clone(&latest), Arc::clone(&stop));
            std::thread::Builder::new()
                .name("windup-screencopy".into())
                .spawn(move || run(method, rect, &tx, &latest, &stop))
                .map_err(|e| e.to_string())?
        };
        let size = match rx.recv_timeout(SETUP_TIMEOUT + Duration::from_secs(1)) {
            Ok(Ok(size)) => size,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                // Its own waits have deadlines; it ends on its own.
                stop.store(true, Ordering::Relaxed);
                return Err("the compositor did not answer in time".into());
            }
        };
        Ok(ScreencopyStream {
            size,
            latest,
            stop,
            thread: Some(thread),
        })
    }
}

impl FrameSource for ScreencopyStream {
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn next_frame(&mut self, buf: &mut Vec<u8>) -> Result<bool, String> {
        let mut latest = self.latest.lock().map_err(|_| "The screen copy stopped.")?;
        if let Some(e) = &latest.error {
            return Err(e.clone());
        }
        if !latest.fresh {
            return Ok(false);
        }
        latest.fresh = false;
        // The stream thread overwrites all of `frame` next time, so whatever
        // `buf` held can go there instead of being copied.
        if buf.len() == latest.frame.len() {
            std::mem::swap(buf, &mut latest.frame);
        } else {
            buf.clone_from(&latest.frame);
        }
        Ok(true)
    }
}

impl Drop for ScreencopyStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The stream thread: set up, say the size, then copy until told to stop.
/// The connection closes when this returns, taking everything it bound.
fn run(
    method: Method,
    rect: DRect,
    ready: &mpsc::Sender<Result<(u32, u32), String>>,
    latest: &Mutex<Latest>,
    stop: &AtomicBool,
) {
    let mut stream = match Stream::setup(method, rect) {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    if ready.send(Ok(stream.composer.size())).is_err() {
        return;
    }
    if let Err(e) = stream.pump(latest, stop)
        && let Ok(mut l) = latest.lock()
    {
        l.error = Some(e);
    }
}

enum Protocol {
    Ext,
    Wlr(ZwlrScreencopyManagerV1),
}

/// One crossed output, and what the events about its frame in flight said.
struct Out {
    facts: OutputFacts,
    /// ext: the session's buffer constraints, and that they are complete.
    size: Option<(u32, u32)>,
    formats: Vec<ShmFormat>,
    constraints_done: bool,
    /// ext: the session is over (the output went away).
    stopped: bool,
    session: Option<ExtImageCopyCaptureSessionV1>,
    ext_frame: Option<ExtImageCopyCaptureFrameV1>,
    wlr_frame: Option<ZwlrScreencopyFrameV1>,
    /// wlr: the shm buffer it asks for (format, width, height, stride), the
    /// list is complete, and `copy` was sent.
    wlr_buffer: Option<(ShmFormat, u32, u32, u32)>,
    wlr_buffer_done: bool,
    copying: bool,
    y_invert: bool,
    /// ext: the transform the compositor reports for this frame.
    transform: Option<Transform>,
    /// `Some(Ok)` on ready, `Some(Err)` with the reason on failed.
    outcome: Option<Result<(), String>>,
    /// The next frame is to be asked for once the last picture is taken.
    due: bool,
    shm: Option<ShmBuffer>,
}

impl Out {
    fn new(facts: OutputFacts) -> Out {
        Out {
            facts,
            size: None,
            formats: Vec::new(),
            constraints_done: false,
            stopped: false,
            session: None,
            ext_frame: None,
            wlr_frame: None,
            wlr_buffer: None,
            wlr_buffer_done: false,
            copying: false,
            y_invert: false,
            transform: None,
            outcome: None,
            due: false,
            shm: None,
        }
    }

    /// What the wlr `buffer` events offered, once they are all in.
    fn wlr_offer(&self, has_buffer_done: bool) -> Option<(ShmFormat, u32, u32, u32)> {
        if has_buffer_done && !self.wlr_buffer_done {
            return None;
        }
        self.wlr_buffer
    }
}

struct StreamState {
    registry: RegistryState,
    output_state: OutputState,
    outs: Vec<Out>,
    /// A crossed output was removed.
    gone: bool,
}

struct Stream {
    // Kept so the connection outlives the queue's use of it.
    _conn: Connection,
    queue: EventQueue<StreamState>,
    state: StreamState,
    protocol: Protocol,
    composer: Composer,
    canvas: Vec<u8>,
    bytes: Vec<u8>,
}

impl Stream {
    fn setup(method: Method, rect: DRect) -> Result<Stream, String> {
        // The registry's own queue, so an output's removal reaches
        // `output_destroyed` below.
        let (conn, globals, mut queue) = connect::<StreamState>().map_err(reason)?;
        let qh = queue.handle();
        let mut state = StreamState {
            registry: RegistryState::new(&globals),
            output_state: OutputState::new(&globals, &qh),
            outs: Vec::new(),
            gone: false,
        };
        // Two roundtrips: the first delivers wl_output, the second xdg_output.
        for _ in 0..2 {
            queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
        }
        let rect = rect.normalised();
        state.outs = outputs_of(&state.output_state)
            .into_iter()
            .filter(|o| o.layout.intersect(&rect).is_some())
            .map(Out::new)
            .collect();
        if state.outs.is_empty() {
            return Err("The area is not on any monitor.".into());
        }
        let unsupported = |_| "the compositor cannot copy the screen".to_owned();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).map_err(unsupported)?;
        let deadline = Instant::now() + SETUP_TIMEOUT;
        let protocol = match method {
            Method::ExtImageCopy => {
                let sources: ExtOutputImageCaptureSourceManagerV1 =
                    globals.bind(&qh, 1..=1, ()).map_err(unsupported)?;
                let copy: ExtImageCopyCaptureManagerV1 =
                    globals.bind(&qh, 1..=1, ()).map_err(unsupported)?;
                for (i, out) in state.outs.iter_mut().enumerate() {
                    let src = sources.create_source(&out.facts.output, &qh, ());
                    out.session = Some(copy.create_session(&src, Options::PaintCursors, &qh, i));
                }
                wait_until(&mut queue, &mut state, deadline, |s| {
                    s.gone || s.outs.iter().all(|o| o.constraints_done || o.stopped)
                })
                .map_err(reason)?;
                Protocol::Ext
            }
            Method::WlrScreencopy => {
                let manager: ZwlrScreencopyManagerV1 =
                    globals.bind(&qh, 1..=3, ()).map_err(unsupported)?;
                let done = manager.version() >= 3;
                for (i, out) in state.outs.iter_mut().enumerate() {
                    out.wlr_frame = Some(manager.capture_output(1, &out.facts.output, &qh, i));
                }
                wait_until(&mut queue, &mut state, deadline, |s| {
                    s.gone
                        || s.outs
                            .iter()
                            .all(|o| o.wlr_offer(done).is_some() || o.outcome.is_some())
                })
                .map_err(reason)?;
                Protocol::Wlr(manager)
            }
        };
        if state.gone || state.outs.iter().any(|o| o.stopped) {
            return Err(DISCONNECTED.into());
        }

        let mut monitors = Vec::new();
        let mut buffers = Vec::new();
        for out in &mut state.outs {
            let (format, w, h, stride) = match &protocol {
                Protocol::Ext => {
                    let (Some((w, h)), Some(&format)) = (out.size, out.formats.first()) else {
                        return Err("the compositor offers no shared-memory buffer".into());
                    };
                    (format, w, h, w.saturating_mul(4))
                }
                Protocol::Wlr(_) => {
                    if let Some(Err(why)) = out.outcome.take() {
                        return Err(why);
                    }
                    out.wlr_buffer
                        .ok_or("the compositor offers no shared-memory buffer")?
                }
            };
            out.shm = Some(ShmBuffer::new(&shm, &qh, format, w, h, stride).map_err(reason)?);
            let transform = out.facts.transform;
            let screen = if transform.swaps_axes() {
                (h, w)
            } else {
                (w, h)
            };
            let layout = out.facts.layout;
            monitors.push((layout, screen.0 as f32 / layout.w, screen));
            buffers.push(Buffer {
                transform,
                order: format.order(),
                y_invert: false,
            });
            // wlr's first frame was asked for above, to learn the buffer.
            out.due = matches!(protocol, Protocol::Ext);
        }
        let mut composer = Composer::new(rect, &monitors);
        for (i, b) in buffers.into_iter().enumerate() {
            composer.set_buffer(i, b);
        }
        let canvas = composer.canvas();
        Ok(Stream {
            _conn: conn,
            queue,
            state,
            protocol,
            composer,
            canvas,
            bytes: Vec::new(),
        })
    }

    /// Copy frames until `stop`, publishing each new picture into `latest`.
    fn pump(&mut self, latest: &Mutex<Latest>, stop: &AtomicBool) -> Result<(), String> {
        while !stop.load(Ordering::Relaxed) {
            self.queue
                .dispatch_pending(&mut self.state)
                .map_err(|e| e.to_string())?;
            if self.state.gone {
                return Err(DISCONNECTED.into());
            }
            let taken = latest.lock().map(|l| !l.fresh).unwrap_or(false);
            let mut fresh = false;
            for i in 0..self.state.outs.len() {
                match self.step(i, taken) {
                    Ok(f) => fresh |= f,
                    Err(e) => return Err(self.settle(e)),
                }
            }
            if fresh && let Ok(mut l) = latest.lock() {
                l.frame.clear();
                l.frame.extend_from_slice(&self.canvas);
                l.fresh = true;
            }
            let waiting = self.state.outs.iter().any(|o| o.due);
            read_events(&self.queue, if waiting { POLL_TAKING } else { POLL }).map_err(reason)?;
        }
        Ok(())
    }

    /// Moves output `i` along: asks for its next frame once the last picture
    /// is `taken`, sends wlr's copy once the buffer is agreed, and places a
    /// finished frame. True when the picture changed.
    fn step(&mut self, i: usize, taken: bool) -> Result<bool, String> {
        let qh = self.queue.handle();
        let out = &mut self.state.outs[i];
        if out.stopped {
            return Err(DISCONNECTED.into());
        }
        if out.due && taken {
            out.due = false;
            out.outcome = None;
            out.transform = None;
            match &self.protocol {
                Protocol::Ext => {
                    let (Some(session), Some(shm)) = (&out.session, &out.shm) else {
                        return Err("the capture session was lost".into());
                    };
                    let frame = session.create_frame(&qh, i);
                    frame.attach_buffer(&shm.buffer);
                    frame.damage_buffer(0, 0, shm.width as i32, shm.height as i32);
                    frame.capture();
                    out.ext_frame = Some(frame);
                }
                Protocol::Wlr(manager) => {
                    out.wlr_buffer = None;
                    out.wlr_buffer_done = false;
                    out.copying = false;
                    out.wlr_frame = Some(manager.capture_output(1, &out.facts.output, &qh, i));
                }
            }
        }
        if let Protocol::Wlr(manager) = &self.protocol
            && !out.copying
            && let (Some(frame), Some(shm)) = (&out.wlr_frame, &out.shm)
            && out.outcome.is_none()
        {
            match out.wlr_offer(manager.version() >= 3) {
                None => return Ok(false),
                Some(offer) if offer == (shm.format, shm.width, shm.height, shm.stride) => {
                    frame.copy(&shm.buffer);
                    out.copying = true;
                }
                Some(_) => return Err("The monitor's resolution changed.".into()),
            }
        }
        let Some(outcome) = out.outcome.take() else {
            return Ok(false);
        };
        if let Some(f) = out.ext_frame.take() {
            f.destroy();
        }
        if let Some(f) = out.wlr_frame.take() {
            f.destroy();
        }
        outcome?;
        out.due = true;
        let (y_invert, reported) = (out.y_invert, out.transform);
        let Some(shm) = &out.shm else {
            return Ok(false);
        };
        shm.read(&mut self.bytes).map_err(reason)?;
        let stride = shm.stride as usize;
        // ext says the transform per frame; it only moves pixels within the
        // same on-screen size, so a turn of the monitor is a new recording.
        let mut want = self.composer.buffer(i).unwrap_or_default();
        if let Some(t) = reported {
            if t.swaps_axes() != want.transform.swaps_axes() {
                return Err("The monitor was rotated.".into());
            }
            want.transform = t;
        }
        want.y_invert = y_invert;
        if Some(want) != self.composer.buffer(i) {
            self.composer.set_buffer(i, want);
        }
        self.composer.blit(i, &self.bytes, stride, &mut self.canvas);
        Ok(true)
    }

    /// After a failure, let an output's removal arrive, and blame it if so.
    fn settle(&mut self, e: String) -> String {
        let deadline = Instant::now() + SETTLE;
        let _ = wait_until(&mut self.queue, &mut self.state, deadline, |s| s.gone);
        if self.state.gone {
            DISCONNECTED.into()
        } else {
            e
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, usize> for StreamState {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureSessionV1,
        event: ext_session::Event,
        i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(out) = state.outs.get_mut(*i) else {
            return;
        };
        match event {
            ext_session::Event::BufferSize { width, height } => out.size = Some((width, height)),
            ext_session::Event::ShmFormat { format } => {
                if let Some(f) = map_format(format) {
                    out.formats.push(f);
                }
            }
            ext_session::Event::Done => out.constraints_done = true,
            ext_session::Event::Stopped => out.stopped = true,
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, usize> for StreamState {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureFrameV1,
        event: ext_frame::Event,
        i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(out) = state.outs.get_mut(*i) else {
            return;
        };
        match event {
            ext_frame::Event::Transform {
                transform: WEnum::Value(t),
            } => out.transform = Some(map_transform(t)),
            ext_frame::Event::Ready => out.outcome = Some(Ok(())),
            ext_frame::Event::Failed { reason } => {
                use ext_frame::FailureReason as R;
                out.outcome = Some(Err(match reason {
                    WEnum::Value(R::Stopped) => DISCONNECTED.to_owned(),
                    WEnum::Value(R::BufferConstraints) => {
                        "The monitor's resolution changed.".to_owned()
                    }
                    _ => "The compositor failed the copy.".to_owned(),
                }));
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, usize> for StreamState {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: wlr_frame::Event,
        i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(out) = state.outs.get_mut(*i) else {
            return;
        };
        match event {
            wlr_frame::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                // The first format we can read wins.
                if out.wlr_buffer.is_none()
                    && let Some(f) = map_format(format)
                {
                    out.wlr_buffer = Some((f, width, height, stride));
                }
            }
            wlr_frame::Event::BufferDone => out.wlr_buffer_done = true,
            wlr_frame::Event::Flags { flags } => {
                out.y_invert =
                    matches!(flags, WEnum::Value(f) if f.contains(wlr_frame::Flags::YInvert));
            }
            wlr_frame::Event::Ready { .. } => out.outcome = Some(Ok(())),
            wlr_frame::Event::Failed => {
                out.outcome = Some(Err("The compositor failed the copy.".to_owned()));
            }
            _ => {}
        }
    }
}

silent!(
    StreamState;
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    ExtImageCaptureSourceV1,
    ExtOutputImageCaptureSourceManagerV1,
    ExtImageCopyCaptureManagerV1,
    ZwlrScreencopyManagerV1,
);

impl OutputHandler for StreamState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if self.outs.iter().any(|o| o.facts.output == output) {
            self.gone = true;
        }
    }
}

impl ProvidesRegistryState for StreamState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}

delegate_output!(StreamState);
delegate_registry!(StreamState);
