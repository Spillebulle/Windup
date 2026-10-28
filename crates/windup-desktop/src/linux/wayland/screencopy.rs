//! A still of every output through `ext_image_copy_capture_v1` (COSMIC, and
//! wlroots compositors that have it) or, failing that, `zwlr_screencopy_v1`
//! (Sway, Hyprland).
//!
//! Every grab makes a Wayland connection of its own (`grab_from_env`) and
//! closes it when done, so whatever a grab binds goes with it, even on an
//! early error, and nothing is left on the long-lived service connection.
//! The background process runs it on a worker thread, off the Wayland thread
//! (the overlay is drawn there afterwards); `--capture-test` calls it
//! directly. Outputs go one at a time into a shared-memory buffer that is
//! read back with `pread`, so nothing here is mapped or `unsafe`. The cursor
//! is not painted (`options = 0`, `overlay_cursor = 0`).

use crate::capture::CaptureError;
use crate::capture::pixels::{ShmFormat, Transform, to_rgba, untransform};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::{delegate_output, delegate_registry, registry_handlers};
use std::fs::File;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::time::{Duration, Instant};
use wayland_client::globals::{GlobalList, registry_queue_init};
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
use windup_core::capture::geometry::{DRect, DesktopShot, MonitorShot};

/// Longest one output may take, from asking to having the pixels. A compositor
/// that never answers must not hang the capture.
const OUTPUT_TIMEOUT: Duration = Duration::from_secs(2);

/// Which protocol to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    ExtImageCopy,
    WlrScreencopy,
}

fn failed(what: impl std::fmt::Display) -> CaptureError {
    CaptureError::Failed(what.to_string())
}

/// Connect to the compositor and take the picture by `method`.
pub fn grab_from_env(method: Method) -> Result<DesktopShot, CaptureError> {
    let conn = Connection::connect_to_env().map_err(|_| CaptureError::Unsupported)?;
    let (globals, _queue) =
        registry_queue_init::<super::Probe>(&conn).map_err(|_| CaptureError::Unsupported)?;
    grab_by(&conn, &globals, method)
}

/// One of the two protocols, on a queue of its own.
pub fn grab_by(
    conn: &Connection,
    globals: &GlobalList,
    method: Method,
) -> Result<DesktopShot, CaptureError> {
    let (queue, state) = learn_outputs(conn, globals)?;
    let qh = queue.handle();
    let shm: wl_shm::WlShm = globals
        .bind(&qh, 1..=1, ())
        .map_err(|_| CaptureError::Unsupported)?;
    let outputs = outputs_of(&state.output_state);
    if outputs.is_empty() {
        return Err(failed("the compositor reports no outputs"));
    }
    let mut cx = Cx { queue, state, shm };
    let mut monitors = Vec::new();
    match method {
        Method::ExtImageCopy => {
            let source: ExtOutputImageCaptureSourceManagerV1 = globals
                .bind(&qh, 1..=1, ())
                .map_err(|_| CaptureError::Unsupported)?;
            let copy: ExtImageCopyCaptureManagerV1 = globals
                .bind(&qh, 1..=1, ())
                .map_err(|_| CaptureError::Unsupported)?;
            for out in &outputs {
                let image = cx.grab_ext(&source, &copy, out)?;
                monitors.push(out.shot(image));
            }
            source.destroy();
            copy.destroy();
        }
        Method::WlrScreencopy => {
            let manager: ZwlrScreencopyManagerV1 = globals
                .bind(&qh, 1..=3, ())
                .map_err(|_| CaptureError::Unsupported)?;
            let has_buffer_done = manager.version() >= 3;
            for out in &outputs {
                let image = cx.grab_wlr(&manager, has_buffer_done, out)?;
                monitors.push(out.shot(image));
            }
            manager.destroy();
        }
    }
    let _ = conn.flush();
    Ok(DesktopShot { monitors })
}

/// A queue and state that know every output.
fn learn_outputs(
    conn: &Connection,
    globals: &GlobalList,
) -> Result<(EventQueue<CopyState>, CopyState), CaptureError> {
    let mut queue = conn.new_event_queue::<CopyState>();
    let qh = queue.handle();
    let mut state = CopyState {
        registry: RegistryState::new(globals),
        output_state: OutputState::new(globals, &qh),
        job: Job::default(),
    };
    // Two roundtrips: the first delivers wl_output, the second xdg_output.
    for _ in 0..2 {
        queue.roundtrip(&mut state).map_err(failed)?;
    }
    Ok((queue, state))
}

/// Every output's name and logical rectangle, from a Wayland connection of
/// its own. The back ends that do not read pixels through Wayland (KWin, the
/// portal) take their monitors' places from this.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn output_layout_from_env() -> Result<Vec<(String, DRect)>, CaptureError> {
    let conn = Connection::connect_to_env().map_err(|_| CaptureError::Unsupported)?;
    let (globals, _queue) =
        registry_queue_init::<super::Probe>(&conn).map_err(|_| CaptureError::Unsupported)?;
    let (_queue, state) = learn_outputs(&conn, &globals)?;
    let layout: Vec<_> = outputs_of(&state.output_state)
        .into_iter()
        .map(|o| (o.name, o.layout))
        .collect();
    if layout.is_empty() {
        return Err(failed("the compositor reports no outputs"));
    }
    Ok(layout)
}

/// An output as the picture needs it, and as the capture overlay finds the
/// output to cover for each monitor of the picture.
pub(super) struct OutputFacts {
    pub(super) output: wl_output::WlOutput,
    pub(super) name: String,
    /// Logical: the desktop units of every Wayland back end.
    pub(super) layout: DRect,
    transform: Transform,
}

impl OutputFacts {
    fn shot(&self, image: image::RgbaImage) -> MonitorShot {
        MonitorShot {
            layout: self.layout,
            image,
            name: self.name.clone(),
            scale: 1.0,
        }
    }
}

pub(super) fn outputs_of(output_state: &OutputState) -> Vec<OutputFacts> {
    let mut found = Vec::new();
    for output in output_state.outputs() {
        let Some(info) = output_state.info(&output) else {
            continue;
        };
        let scale = info.scale_factor.max(1) as f32;
        let mode = info.modes.iter().find(|m| m.current).map(|m| m.dimensions);
        // Without xdg_output the layout is the wl_output position and the
        // mode over the integer scale (rotation ignored).
        let (x, y) = info.logical_position.unwrap_or(info.location);
        let (w, h) = info
            .logical_size
            .or_else(|| mode.map(|(w, h)| ((w as f32 / scale) as i32, (h as f32 / scale) as i32)))
            .unwrap_or((0, 0));
        if w <= 0 || h <= 0 {
            continue;
        }
        found.push(OutputFacts {
            name: info
                .name
                .clone()
                .unwrap_or_else(|| format!("{} {}", info.make, info.model).trim().to_owned()),
            layout: DRect {
                x: x as f32,
                y: y as f32,
                w: w as f32,
                h: h as f32,
            },
            transform: map_transform(info.transform),
            output,
        });
    }
    found
}

fn map_transform(t: wl_output::Transform) -> Transform {
    use wl_output::Transform as W;
    match t {
        W::_90 => Transform::Rot90,
        W::_180 => Transform::Rot180,
        W::_270 => Transform::Rot270,
        W::Flipped => Transform::Flipped,
        W::Flipped90 => Transform::Flipped90,
        W::Flipped180 => Transform::Flipped180,
        W::Flipped270 => Transform::Flipped270,
        _ => Transform::Normal,
    }
}

fn map_format(f: WEnum<wl_shm::Format>) -> Option<ShmFormat> {
    match f.into_result().ok()? {
        wl_shm::Format::Argb8888 => Some(ShmFormat::Argb8888),
        wl_shm::Format::Xrgb8888 => Some(ShmFormat::Xrgb8888),
        wl_shm::Format::Abgr8888 => Some(ShmFormat::Abgr8888),
        wl_shm::Format::Xbgr8888 => Some(ShmFormat::Xbgr8888),
        _ => None,
    }
}

fn wl_format(f: ShmFormat) -> wl_shm::Format {
    match f {
        ShmFormat::Argb8888 => wl_shm::Format::Argb8888,
        ShmFormat::Xrgb8888 => wl_shm::Format::Xrgb8888,
        ShmFormat::Abgr8888 => wl_shm::Format::Abgr8888,
        ShmFormat::Xbgr8888 => wl_shm::Format::Xbgr8888,
    }
}

/// What the events of the capture in flight have said so far.
#[derive(Default)]
struct Job {
    /// ext: the session's buffer constraints, and that they are complete.
    size: Option<(u32, u32)>,
    formats: Vec<ShmFormat>,
    constraints_done: bool,
    /// The session is over (ext `stopped`).
    stopped: bool,
    /// ext: the transform the compositor reports having applied.
    transform: Option<Transform>,
    /// wlr: the shm buffer it asks for (format, width, height, stride).
    wlr_buffer: Option<(ShmFormat, u32, u32, u32)>,
    wlr_buffer_done: bool,
    wlr_y_invert: bool,
    /// `Some(Ok)` on ready, `Some(Err)` with the reason on failed.
    outcome: Option<Result<(), String>>,
}

struct CopyState {
    registry: RegistryState,
    output_state: OutputState,
    job: Job,
}

struct Cx {
    queue: EventQueue<CopyState>,
    state: CopyState,
    shm: wl_shm::WlShm,
}

impl Cx {
    /// Dispatch until `done` holds, with `deadline` as the limit.
    fn wait(&mut self, deadline: Instant, done: fn(&Job) -> bool) -> Result<(), CaptureError> {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        loop {
            self.queue
                .dispatch_pending(&mut self.state)
                .map_err(failed)?;
            if done(&self.state.job) {
                return Ok(());
            }
            let left = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| failed("the compositor did not answer in time"))?;
            self.queue.flush().map_err(failed)?;
            // None means events arrived since the dispatch: go round again.
            let Some(guard) = self.queue.prepare_read() else {
                continue;
            };
            let timeout = Timespec::try_from(left).map_err(failed)?;
            let conn_fd = guard.connection_fd();
            let mut fds = [PollFd::new(&conn_fd, PollFlags::IN)];
            if poll(&mut fds, Some(&timeout)).map_err(failed)? > 0 {
                guard.read().map_err(failed)?;
            }
        }
    }

    /// An anonymous file of `stride * height` bytes, a pool over it and a
    /// buffer in it.
    fn buffer(
        &self,
        format: ShmFormat,
        width: u32,
        height: u32,
        stride: u32,
    ) -> Result<(File, wl_shm_pool::WlShmPool, wl_buffer::WlBuffer), CaptureError> {
        use rustix::fs::{MemfdFlags, ftruncate, memfd_create};
        let len = u64::from(stride) * u64::from(height);
        let size = i32::try_from(len).map_err(|_| failed("the screen is too large to copy"))?;
        let (Ok(w), Ok(h), Ok(s)) = (
            i32::try_from(width),
            i32::try_from(height),
            i32::try_from(stride),
        ) else {
            return Err(failed("the screen is too large to copy"));
        };
        if w <= 0 || h <= 0 || s <= 0 {
            return Err(failed("the compositor asked for an empty buffer"));
        }
        let fd = memfd_create("windup-capture", MemfdFlags::CLOEXEC).map_err(failed)?;
        ftruncate(&fd, len).map_err(failed)?;
        let file = File::from(fd);
        let qh = self.queue.handle();
        let pool = self.shm.create_pool(file.as_fd(), size, &qh, ());
        let buffer = pool.create_buffer(0, w, h, s, wl_format(format), &qh, ());
        Ok((file, pool, buffer))
    }

    fn read_back(
        file: &File,
        format: ShmFormat,
        width: u32,
        height: u32,
        stride: u32,
    ) -> Result<image::RgbaImage, CaptureError> {
        let mut bytes = vec![0u8; stride as usize * height as usize];
        file.read_exact_at(&mut bytes, 0).map_err(failed)?;
        to_rgba(format, width, height, stride as usize, &bytes)
    }

    fn grab_ext(
        &mut self,
        source: &ExtOutputImageCaptureSourceManagerV1,
        copy: &ExtImageCopyCaptureManagerV1,
        out: &OutputFacts,
    ) -> Result<image::RgbaImage, CaptureError> {
        let qh = self.queue.handle();
        let deadline = Instant::now() + OUTPUT_TIMEOUT;
        self.state.job = Job::default();
        let src = source.create_source(&out.output, &qh, ());
        let session = copy.create_session(&src, Options::empty(), &qh, ());
        let result = self.ext_frame(&session, deadline, out);
        session.destroy();
        src.destroy();
        result
    }

    fn ext_frame(
        &mut self,
        session: &ExtImageCopyCaptureSessionV1,
        deadline: Instant,
        out: &OutputFacts,
    ) -> Result<image::RgbaImage, CaptureError> {
        let qh = self.queue.handle();
        self.wait(deadline, |j| j.constraints_done || j.stopped)?;
        let job = &self.state.job;
        if job.stopped {
            return Err(failed("the capture session was stopped"));
        }
        let (Some((w, h)), Some(&format)) = (job.size, job.formats.first()) else {
            return Err(failed("the compositor offers no shared-memory buffer"));
        };
        let stride = w.saturating_mul(4);
        let (file, pool, buffer) = self.buffer(format, w, h, stride)?;
        let frame = session.create_frame(&qh, ());
        frame.attach_buffer(&buffer);
        frame.damage_buffer(0, 0, w as i32, h as i32);
        frame.capture();
        let waited = self.wait(deadline, |j| j.outcome.is_some() || j.stopped);
        frame.destroy();
        buffer.destroy();
        pool.destroy();
        waited?;
        match self.state.job.outcome.take() {
            Some(Ok(())) => {}
            Some(Err(reason)) => return Err(failed(reason)),
            None => return Err(failed("the capture session was stopped")),
        }
        let img = Self::read_back(&file, format, w, h, stride)?;
        let transform = self.state.job.transform.unwrap_or(out.transform);
        Ok(untransform(img, transform))
    }

    fn grab_wlr(
        &mut self,
        manager: &ZwlrScreencopyManagerV1,
        has_buffer_done: bool,
        out: &OutputFacts,
    ) -> Result<image::RgbaImage, CaptureError> {
        let qh = self.queue.handle();
        let deadline = Instant::now() + OUTPUT_TIMEOUT;
        self.state.job = Job::default();
        let frame = manager.capture_output(0, &out.output, &qh, ());
        let result = self.wlr_frame(&frame, has_buffer_done, deadline, out);
        frame.destroy();
        result
    }

    fn wlr_frame(
        &mut self,
        frame: &ZwlrScreencopyFrameV1,
        has_buffer_done: bool,
        deadline: Instant,
        out: &OutputFacts,
    ) -> Result<image::RgbaImage, CaptureError> {
        // Version 3 ends its list of buffer kinds with buffer_done; before
        // that the single `buffer` event is the whole list.
        if has_buffer_done {
            self.wait(deadline, |j| j.wlr_buffer_done || j.outcome.is_some())?;
        } else {
            self.wait(deadline, |j| j.wlr_buffer.is_some() || j.outcome.is_some())?;
        }
        if let Some(Err(reason)) = self.state.job.outcome.take() {
            return Err(failed(reason));
        }
        let Some((format, w, h, stride)) = self.state.job.wlr_buffer else {
            return Err(failed("the compositor offers no shared-memory buffer"));
        };
        let (file, pool, buffer) = self.buffer(format, w, h, stride)?;
        frame.copy(&buffer);
        let waited = self.wait(deadline, |j| j.outcome.is_some());
        buffer.destroy();
        pool.destroy();
        waited?;
        if let Some(Err(reason)) = self.state.job.outcome.take() {
            return Err(failed(reason));
        }
        let mut img = Self::read_back(&file, format, w, h, stride)?;
        if self.state.job.wlr_y_invert {
            img = image::imageops::flip_vertical(&img);
        }
        // The buffer is in the output's own orientation.
        Ok(untransform(img, out.transform))
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for CopyState {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureSessionV1,
        event: ext_session::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let job = &mut state.job;
        match event {
            ext_session::Event::BufferSize { width, height } => job.size = Some((width, height)),
            ext_session::Event::ShmFormat { format } => {
                if let Some(f) = map_format(format) {
                    job.formats.push(f);
                }
            }
            ext_session::Event::Done => job.constraints_done = true,
            ext_session::Event::Stopped => job.stopped = true,
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, ()> for CopyState {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureFrameV1,
        event: ext_frame::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_frame::Event::Transform {
                transform: WEnum::Value(t),
            } => state.job.transform = Some(map_transform(t)),
            ext_frame::Event::Ready => state.job.outcome = Some(Ok(())),
            ext_frame::Event::Failed { reason } => {
                state.job.outcome =
                    Some(Err(format!("the compositor failed the copy ({reason:?})")));
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for CopyState {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: wlr_frame::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let job = &mut state.job;
        match event {
            wlr_frame::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                // The first format we can read wins.
                if job.wlr_buffer.is_none()
                    && let Some(f) = map_format(format)
                {
                    job.wlr_buffer = Some((f, width, height, stride));
                }
            }
            wlr_frame::Event::BufferDone => job.wlr_buffer_done = true,
            wlr_frame::Event::Flags { flags } => {
                job.wlr_y_invert =
                    matches!(flags, WEnum::Value(f) if f.contains(wlr_frame::Flags::YInvert));
            }
            wlr_frame::Event::Ready { .. } => job.outcome = Some(Ok(())),
            wlr_frame::Event::Failed => {
                job.outcome = Some(Err("the compositor failed the copy".to_owned()));
            }
            _ => {}
        }
    }
}

/// Objects that never send an event we use.
macro_rules! silent {
    ($($proxy:ty),* $(,)?) => {$(
        impl Dispatch<$proxy, ()> for CopyState {
            fn event(
                _: &mut Self,
                _: &$proxy,
                _: <$proxy as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )*};
}

silent!(
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    ExtImageCaptureSourceV1,
    ExtOutputImageCaptureSourceManagerV1,
    ExtImageCopyCaptureManagerV1,
    ZwlrScreencopyManagerV1,
);

impl OutputHandler for CopyState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ProvidesRegistryState for CopyState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}

delegate_output!(CopyState);
delegate_registry!(CopyState);
