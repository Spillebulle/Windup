//! Reading the ScreenCast portal's PipeWire streams: the only code that
//! touches PipeWire, behind the `pipewire` feature (its bindings need
//! libclang and the PipeWire headers at build time).
//!
//! One PipeWire main loop on a thread of its own, connected through the
//! descriptor the portal opened, with one stream per shared monitor. Each
//! stream offers only shared-memory formats (`ShmVideoFormat::OFFERED`, no
//! DMA-BUF modifiers), so the compositor hands over frames in memory. Once
//! every stream has said its size the layout is fixed and a `Composer`
//! built; from then on each buffer is placed in the picture as it arrives
//! and the picture is published to the one-slot mailbox.
//!
//! For Steps (`Want::Stills`) nothing is composed: each stream keeps its
//! own recent frame and the pointer from its cursor metadata
//! (`SPA_META_Cursor`), which is the one place this module reads PipeWire's
//! buffers through raw pointers.

use std::cell::RefCell;
use std::io::Cursor;
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::spa;
use pw::spa::buffer::{ChunkFlags, DataType};
use pw::spa::param::ParamType;
use pw::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use pw::spa::param::video::{VideoFormat, VideoInfoRaw};
use pw::spa::pod::{ChoiceValue, Object, Pod, Property, Value};
use pw::spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Fraction, Id, Rectangle, SpaTypes};
use pw::stream::{StreamFlags, StreamState};
use windup_core::capture::geometry::DRect;
use windup_core::video::Composer;
use windup_core::video::compose::Buffer;
use windup_core::video::screencast::{
    ShmVideoFormat, StillPace, StillTick, covered_by, cursor_in_stream, frame_view,
    layout_from_streams, still_rgba, stream_scale,
};

use super::screencast::{PortalStream, STOPPED, Want};
use super::stream::Latest;
use crate::video::outcome::SCREENS_NOT_SHARED;

/// Longest the streams may take to agree a format and say their size.
const SETUP_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the Steps timer looks for a held picture that is due or a shot
/// that waits.
const STILLS_TICK: Duration = Duration::from_millis(20);

/// The running streams. Dropping it stops the loop and waits for the thread.
pub(super) struct PwStream {
    /// The picture's size, even both ways.
    pub(super) size: (u32, u32),
    stop: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for PwStream {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Connects to the portal's PipeWire remote `fd` and starts reading
/// `streams` for what is wanted. Returns once every stream has agreed a
/// format; for a recording that gives the picture's size, and frames then
/// arrive in `latest`. For stills the size is (0, 0), each stream's frames
/// and pointer go to its slot, and only a failure reaches `latest`.
pub(super) fn spawn(
    fd: OwnedFd,
    streams: &[PortalStream],
    want: Want,
    latest: Arc<Mutex<Latest>>,
) -> Result<PwStream, String> {
    if streams.is_empty() {
        return Err("The desktop shared no screen.".into());
    }
    let (stop, stop_rx) = pw::channel::channel::<()>();
    let (ready, ready_rx) = mpsc::channel();
    let streams = streams.to_vec();
    let thread = std::thread::Builder::new()
        .name("windup-pipewire".into())
        .spawn(move || {
            if let Err(e) = run(fd, streams, want, &latest, ready.clone(), stop_rx) {
                // Before the size was known this reaches `spawn`; after,
                // the recording.
                let _ = ready.send(Err(e.clone()));
                if let Ok(mut l) = latest.lock() {
                    l.fail(e);
                }
            }
        })
        .map_err(|e| e.to_string())?;
    let mut pw = PwStream {
        size: (0, 0),
        stop,
        thread: Some(thread),
    };
    match ready_rx.recv_timeout(SETUP_TIMEOUT) {
        Ok(Ok(size)) => {
            pw.size = size;
            Ok(pw)
        }
        // Dropping `pw` stops the loop and joins the thread.
        Ok(Err(e)) => Err(e),
        Err(_) => Err("the shared screens sent no picture in time".into()),
    }
}

/// SPA's name for each format; SPA names formats in memory order, as
/// `ShmVideoFormat` does.
fn spa_format(f: ShmVideoFormat) -> VideoFormat {
    match f {
        ShmVideoFormat::Bgrx => VideoFormat::BGRx,
        ShmVideoFormat::Bgra => VideoFormat::BGRA,
        ShmVideoFormat::Rgbx => VideoFormat::RGBx,
        ShmVideoFormat::Rgba => VideoFormat::RGBA,
    }
}

fn shm_format(f: VideoFormat) -> Option<ShmVideoFormat> {
    ShmVideoFormat::OFFERED
        .into_iter()
        .find(|s| spa_format(*s) == f)
}

/// The `EnumFormat` every stream offers: raw video in one of the shared
/// memory formats, any size, any rate. No modifier property, so no DMA-BUF.
fn enum_format() -> Result<Vec<u8>, String> {
    let formats: Vec<Id> = ShmVideoFormat::OFFERED
        .into_iter()
        .map(|f| Id(spa_format(f).as_raw()))
        .collect();
    let choice = |v: ChoiceValue| Value::Choice(v);
    let object = Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: vec![
            Property::new(
                FormatProperties::MediaType.as_raw(),
                Value::Id(Id(MediaType::Video.as_raw())),
            ),
            Property::new(
                FormatProperties::MediaSubtype.as_raw(),
                Value::Id(Id(MediaSubtype::Raw.as_raw())),
            ),
            Property::new(
                FormatProperties::VideoFormat.as_raw(),
                // The default first, then every choice, the default again
                // among them (SPA's enum layout).
                choice(ChoiceValue::Id(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Enum {
                        default: formats[0],
                        alternatives: formats.clone(),
                    },
                ))),
            ),
            Property::new(
                FormatProperties::VideoSize.as_raw(),
                choice(ChoiceValue::Rectangle(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: Rectangle {
                            width: 1920,
                            height: 1080,
                        },
                        min: Rectangle {
                            width: 1,
                            height: 1,
                        },
                        max: Rectangle {
                            width: 16384,
                            height: 16384,
                        },
                    },
                ))),
            ),
            Property::new(
                FormatProperties::VideoFramerate.as_raw(),
                choice(ChoiceValue::Fraction(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: Fraction { num: 60, denom: 1 },
                        min: Fraction { num: 0, denom: 1 },
                        max: Fraction {
                            num: 1000,
                            denom: 1,
                        },
                    },
                ))),
            ),
        ],
    };
    spa::pod::serialize::PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(object))
        .map(|(out, _)| out.into_inner())
        .map_err(|e| format!("could not describe the formats: {e:?}"))
}

/// The `SPA_PARAM_Meta` a Steps stream asks for: cursor metadata with room
/// for a 64 x 64 bitmap (the compositor may send a bitmap with it; Windup
/// only reads the position, but a block too small for the bitmap could be
/// refused).
fn cursor_meta() -> Result<Vec<u8>, String> {
    use spa::sys::{SPA_META_Cursor, SPA_PARAM_META_size, SPA_PARAM_META_type};
    use spa::sys::{spa_meta_bitmap, spa_meta_cursor};
    let size = |side: usize| {
        (std::mem::size_of::<spa_meta_cursor>()
            + std::mem::size_of::<spa_meta_bitmap>()
            + side * side * 4) as i32
    };
    let object = Object {
        type_: SpaTypes::ObjectParamMeta.as_raw(),
        id: ParamType::Meta.as_raw(),
        properties: vec![
            Property::new(SPA_PARAM_META_type, Value::Id(Id(SPA_META_Cursor))),
            Property::new(
                SPA_PARAM_META_size,
                Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: size(64),
                        min: size(1),
                        max: size(256),
                    },
                ))),
            ),
        ],
    };
    spa::pod::serialize::PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(object))
        .map(|(out, _)| out.into_inner())
        .map_err(|e| format!("could not describe the cursor metadata: {e:?}"))
}

/// Everything the loop's callbacks share. They all run on the loop's
/// thread, one at a time.
struct Feed {
    want: Want,
    streams: Vec<PortalStream>,
    /// Each stream's agreed format and size in pixels.
    formats: Vec<Option<(ShmVideoFormat, u32, u32)>>,
    composer: Option<Composer>,
    canvas: Vec<u8>,
    /// Taken when the size is said (or setup fails).
    ready: Option<mpsc::Sender<Result<(u32, u32), String>>>,
    latest: Arc<Mutex<Latest>>,
    mainloop: pw::main_loop::WeakMainLoop,
    /// Counts cursor changes across the streams (`StillSlot::cursor_stamp`).
    cursor_clock: u64,
    /// Steps: each stream's newest picture not copied yet, its buffer kept
    /// from the stream until it is (null for none), and when to copy.
    held: Vec<RawBuffer>,
    pace: Vec<StillPace>,
}

impl Feed {
    /// Ends the loop with a reason: to `spawn` while it still waits, to the
    /// recording after.
    fn fail(&mut self, why: String) {
        match self.ready.take() {
            Some(ready) => {
                let _ = ready.send(Err(why));
            }
            None => {
                if let Ok(mut l) = self.latest.lock() {
                    l.fail(why);
                }
            }
        }
        if let Some(ml) = self.mainloop.upgrade() {
            ml.quit();
        }
    }

    /// Stream `i` agreed a format.
    fn format(&mut self, i: usize, format: ShmVideoFormat, w: u32, h: u32) {
        let Some(slot) = self.formats.get_mut(i) else {
            return;
        };
        let before = slot.replace((format, w, h));
        if let Want::Stills(_) = self.want {
            // Each stream is read on its own: a new size is only that.
            if self.formats.iter().all(Option::is_some)
                && let Some(ready) = self.ready.take()
            {
                let _ = ready.send(Ok((0, 0)));
            }
            return;
        }
        let Some(composer) = self.composer.as_mut() else {
            if self.formats.iter().all(Option::is_some) {
                self.build();
            }
            return;
        };
        // The layout is fixed once built; a new size is a new recording.
        if before.is_some_and(|(_, bw, bh)| (bw, bh) != (w, h)) {
            self.fail("The monitor's resolution changed.".into());
            return;
        }
        let mut buffer = composer.buffer(i).unwrap_or_default();
        buffer.order = format.order();
        composer.set_buffer(i, buffer);
    }

    /// Every stream has a size: fix the layout and say the picture's size.
    fn build(&mut self) {
        let rect = match &self.want {
            Want::Compose(rect) => *rect,
            Want::Stills(_) => return,
        };
        let agreed: Vec<(ShmVideoFormat, u32, u32)> =
            self.formats.iter().flatten().copied().collect();
        if agreed.len() != self.streams.len() {
            return;
        }
        // A stream the portal gave no size is as big as its buffer.
        let described: Vec<_> = self
            .streams
            .iter()
            .zip(&agreed)
            .map(|(s, &(_, w, h))| {
                let size = s.size.unwrap_or((w as i32, h as i32));
                (s.node, s.position, Some(size))
            })
            .collect();
        let layouts = layout_from_streams(&described);
        // Partly on a screen the grant leaves out would record black there.
        if !covered_by(rect, &layouts) {
            self.fail(SCREENS_NOT_SHARED.into());
            return;
        }
        let monitors: Vec<(DRect, f32, (u32, u32))> = layouts
            .iter()
            .zip(&agreed)
            .map(|(l, &(_, w, h))| (*l, stream_scale(w, l.w), (w, h)))
            .collect();
        let mut composer = Composer::new(rect, &monitors);
        for (i, &(format, _, _)) in agreed.iter().enumerate() {
            composer.set_buffer(
                i,
                Buffer {
                    order: format.order(),
                    ..Buffer::default()
                },
            );
        }
        self.canvas = composer.canvas();
        if let Some(ready) = self.ready.take() {
            let _ = ready.send(Ok(composer.size()));
        }
        self.composer = Some(composer);
    }

    /// A buffer of stream `i` arrived.
    fn frame(&mut self, i: usize, bytes: &[u8], chunk: (u32, i32, u32, bool)) {
        let (Some(composer), Some(Some((_, w, h)))) = (&self.composer, self.formats.get(i)) else {
            return;
        };
        let (offset, stride, size, corrupted) = chunk;
        let Some((start, stride)) =
            frame_view(offset, stride, size, corrupted, bytes.len(), *w, *h)
        else {
            return;
        };
        composer.blit(i, &bytes[start..], stride, &mut self.canvas);
        if let Ok(mut l) = self.latest.lock() {
            l.publish(&self.canvas);
        }
    }

    /// Stream `i`'s cursor metadata said `id` at `position`. An id of 0 is
    /// "no new cursor data", so the pointer stays where it was.
    fn cursor(&mut self, i: usize, id: u32, position: (i32, i32)) {
        let (Want::Stills(slots), Some(Some((_, w, h)))) = (&self.want, self.formats.get(i)) else {
            return;
        };
        if id == 0 {
            return;
        }
        let cursor = cursor_in_stream(id, position, (*w, *h));
        let mut slots = slots.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = slots.get_mut(i)
            && slot.cursor != cursor
        {
            self.cursor_clock += 1;
            slot.cursor = cursor;
            slot.cursor_stamp = self.cursor_clock;
        }
    }

    /// Whether `data` holds a whole picture of stream `i`: a buffer with
    /// only the cursor in it (empty or corrupted) does not. A buffer Windup
    /// cannot read ends the stream.
    fn is_picture(&mut self, i: usize, data: &mut spa::buffer::Data) -> bool {
        let Some(Some((_, w, h))) = self.formats.get(i).copied() else {
            return false;
        };
        let kind = data.type_();
        if kind != DataType::MemPtr && kind != DataType::MemFd {
            self.fail(format!(
                "PipeWire sent {kind:?} buffers, which Windup cannot read."
            ));
            return false;
        }
        let (offset, stride, size, corrupted) = chunk_of(data);
        data.data().is_some_and(|bytes| {
            frame_view(offset, stride, size, corrupted, bytes.len(), w, h).is_some()
        })
    }

    /// Copies the picture in `data` to stream `i`'s slot, answering a shot
    /// that waits for it.
    fn copy(&mut self, i: usize, data: &mut spa::buffer::Data, now: Instant) {
        let (Want::Stills(slots), Some(Some((format, w, h)))) =
            (&self.want, self.formats.get(i).copied())
        else {
            return;
        };
        let (offset, stride, size, corrupted) = chunk_of(data);
        let Some(bytes) = data.data() else {
            return;
        };
        let Some((start, stride)) = frame_view(offset, stride, size, corrupted, bytes.len(), w, h)
        else {
            return;
        };
        // Converted outside the lock, so a shot looking is not held up.
        let rgba = still_rgba(&bytes[start..], stride, w, h, format);
        let Some(image) = image::RgbaImage::from_raw(w, h, rgba) else {
            return;
        };
        let mut slots = slots.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = slots.get_mut(i) {
            slot.frame = Some((image, now));
            slot.want_now = false;
        }
    }

    /// Whether a shot waits on stream `i`.
    fn wanted(&self, i: usize) -> bool {
        match &self.want {
            Want::Stills(slots) => slots
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(i)
                .is_some_and(|s| s.want_now),
            Want::Compose(_) => false,
        }
    }

    /// A shot on stream `i` is answered by the copy there is.
    fn answer(&self, i: usize) {
        if let Want::Stills(slots) = &self.want
            && let Some(slot) = slots.lock().unwrap_or_else(|e| e.into_inner()).get_mut(i)
        {
            slot.want_now = false;
        }
    }
}

/// A chunk's offset, stride, size and whether it is corrupted.
fn chunk_of(data: &spa::buffer::Data) -> (u32, i32, u32, bool) {
    let chunk = data.chunk();
    (
        chunk.offset(),
        chunk.stride(),
        chunk.size(),
        chunk.flags().contains(ChunkFlags::CORRUPTED),
    )
}

/// A Steps stream's buffer, taken raw: the pipewire crate's `Buffer` gives
/// no way to a buffer's metadata, nor to keep one past a callback.
type RawBuffer = *mut pw::sys::pw_buffer;

/// A cursor metadata block's id and position.
type CursorMeta = (u32, (i32, i32));

/// The cursor metadata and the first data block of `b`.
///
/// # Safety
///
/// `b` is non-null, was dequeued from its stream and has not been given
/// back, and the stream has not taken it back (`remove_buffer`). The data
/// block lives as long as that holds; the caller drops it before then.
unsafe fn open_buffer<'a>(b: RawBuffer) -> (Option<CursorMeta>, Option<&'a mut spa::buffer::Data>) {
    use spa::sys::{SPA_META_Cursor, spa_buffer_find_meta_data, spa_meta_cursor};
    // SAFETY: per the contract `b` points at a live `pw_buffer`, whose
    // `spa_buffer` is checked for null. `spa_buffer_find_meta_data` returns
    // null unless the buffer has a cursor block at least as big as
    // `spa_meta_cursor`, which is all that is read of it. `Data` is
    // `repr(transparent)` over `spa_data`, as `Buffer::datas_mut` relies on,
    // and the stream maps the buffers (`MAP_BUFFERS`).
    unsafe {
        let spa = (*b).buffer;
        if spa.is_null() {
            return (None, None);
        }
        let meta =
            spa_buffer_find_meta_data(spa, SPA_META_Cursor, std::mem::size_of::<spa_meta_cursor>())
                as *const spa_meta_cursor;
        let cursor = meta.as_ref().map(|c| (c.id, (c.position.x, c.position.y)));
        let data = ((*spa).n_datas > 0 && !(*spa).datas.is_null())
            .then(|| &mut *((*spa).datas as *mut spa::buffer::Data));
        (cursor, data)
    }
}

/// Gives `b` back to `stream`, which it was dequeued from.
fn give_back(stream: &pw::stream::StreamRef, b: RawBuffer) {
    // SAFETY: every caller passes a buffer dequeued from this stream that is
    // not given back yet and that the stream has not taken back, and forgets
    // it after this.
    unsafe { stream.queue_raw_buffer(b) }
}

/// Every buffer waiting on Steps stream `i`, oldest first. The cursor
/// metadata of each is read. A buffer with only the cursor in it goes back
/// at once and never touches the picture held. A picture is copied when
/// due (`StillPace`), and otherwise held in place of the one held before,
/// for the timer (`tick_stills`) or a shot to copy.
fn drain_stills(stream: &pw::stream::StreamRef, i: usize, feed: &RefCell<Feed>) {
    let mut feed = feed.borrow_mut();
    let now = Instant::now();
    loop {
        // SAFETY: dequeued from this stream; checked for null before use.
        let b = unsafe { stream.dequeue_raw_buffer() };
        if b.is_null() {
            break;
        }
        // SAFETY: `b` was just dequeued and is given back or held below.
        let (cursor, data) = unsafe { open_buffer(b) };
        if let Some((id, position)) = cursor {
            feed.cursor(i, id, position);
        }
        let Some(data) = data.filter(|_| i < feed.held.len()) else {
            give_back(stream, b);
            continue;
        };
        if !feed.is_picture(i, data) {
            give_back(stream, b);
            continue;
        }
        let wanted = feed.wanted(i);
        let old = std::mem::replace(&mut feed.held[i], std::ptr::null_mut());
        if !old.is_null() {
            give_back(stream, old);
        }
        if feed.pace[i].picture(now, wanted) {
            feed.copy(i, data, now);
            give_back(stream, b);
        } else {
            feed.held[i] = b;
        }
    }
}

/// The loop's timer for Steps: copies each held picture once it is due or
/// a shot asks, and answers a shot when nothing newer is held.
fn tick_stills(streams: &[Rc<pw::stream::Stream>], feed: &RefCell<Feed>) {
    let mut feed = feed.borrow_mut();
    let now = Instant::now();
    for (i, stream) in streams.iter().enumerate() {
        if i >= feed.held.len() {
            break;
        }
        let wanted = feed.wanted(i);
        match feed.pace[i].tick(now, wanted) {
            StillTick::CopyHeld => {
                let b = std::mem::replace(&mut feed.held[i], std::ptr::null_mut());
                if b.is_null() {
                    continue;
                }
                // SAFETY: held buffers were dequeued from this stream and
                // are dropped from `held` when the stream takes them back.
                if let (_, Some(data)) = unsafe { open_buffer(b) } {
                    feed.copy(i, data, now);
                }
                give_back(stream, b);
            }
            StillTick::Answer => feed.answer(i),
            StillTick::Wait => {}
        }
    }
}

/// Gives back every held buffer, before the streams go.
fn release_held(streams: &[Rc<pw::stream::Stream>], feed: &RefCell<Feed>) {
    let mut feed = feed.borrow_mut();
    for (stream, held) in streams.iter().zip(feed.held.iter_mut()) {
        let b = std::mem::replace(held, std::ptr::null_mut());
        if !b.is_null() {
            give_back(stream, b);
        }
    }
}

/// The loop thread: connect, start a stream per monitor, run until stopped.
fn run(
    fd: OwnedFd,
    streams: Vec<PortalStream>,
    want: Want,
    latest: &Arc<Mutex<Latest>>,
    ready: mpsc::Sender<Result<(u32, u32), String>>,
    stop: pw::channel::Receiver<()>,
) -> Result<(), String> {
    let err = |e: pw::Error| format!("PipeWire: {e}");
    // Declared in the order they are made, so they are dropped in reverse:
    // listeners before streams, streams before the core, the loop last.
    let mainloop = pw::main_loop::MainLoop::new(None).map_err(err)?;
    let context = pw::context::Context::new(&mainloop).map_err(err)?;
    let core = context.connect_fd(fd, None).map_err(err)?;
    let stills = matches!(want, Want::Stills(_));
    // Steps asks each stream for the pointer as metadata once its format is
    // agreed.
    let meta = if stills { Some(cursor_meta()?) } else { None };
    let feed = Rc::new(RefCell::new(Feed {
        want,
        formats: vec![None; streams.len()],
        streams: streams.clone(),
        composer: None,
        canvas: Vec::new(),
        ready: Some(ready),
        latest: Arc::clone(latest),
        mainloop: mainloop.downgrade(),
        cursor_clock: 0,
        held: vec![std::ptr::null_mut(); if stills { streams.len() } else { 0 }],
        pace: vec![StillPace::default(); streams.len()],
    }));
    let _core_listener = core
        .add_listener_local()
        .error({
            let feed = Rc::clone(&feed);
            move |id, _seq, _res, message| {
                if id == pw::core::PW_ID_CORE {
                    feed.borrow_mut().fail(format!("PipeWire: {message}"));
                }
            }
        })
        .register();

    let format = enum_format()?;
    let mut running = Vec::new();
    for (i, s) in streams.iter().enumerate() {
        let stream = pw::stream::Stream::new(
            &core,
            "windup-screencast",
            pw::properties::properties! {
                *pw::keys::MEDIA_TYPE => "Video",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Screen",
            },
        )
        .map_err(err)?;
        let listener = stream
            .add_local_listener_with_user_data(i)
            .state_changed({
                let feed = Rc::clone(&feed);
                move |_, _, old, new| match new {
                    StreamState::Error(e) => feed.borrow_mut().fail(format!("{STOPPED} ({e})")),
                    StreamState::Unconnected if old != StreamState::Unconnected => {
                        feed.borrow_mut().fail(STOPPED.into());
                    }
                    _ => {}
                }
            })
            .param_changed({
                let feed = Rc::clone(&feed);
                let meta = meta.clone();
                move |stream, i, id, param| {
                    let Some(param) = param else {
                        return;
                    };
                    if id != ParamType::Format.as_raw() {
                        return;
                    }
                    let Ok((media, sub)) = spa::param::format_utils::parse_format(param) else {
                        return;
                    };
                    if media != MediaType::Video || sub != MediaSubtype::Raw {
                        return;
                    }
                    let mut info = VideoInfoRaw::default();
                    if info.parse(param).is_err() {
                        feed.borrow_mut()
                            .fail("PipeWire sent a format Windup cannot read.".into());
                        return;
                    }
                    let Some(format) = shm_format(info.format()) else {
                        feed.borrow_mut().fail(format!(
                            "PipeWire sent pixel format {}, which Windup cannot read.",
                            info.format().as_raw()
                        ));
                        return;
                    };
                    let size = info.size();
                    let mut fed = feed.borrow_mut();
                    // A held picture is of the old format: give it back
                    // before the buffers change.
                    if let Some(held) = fed.held.get_mut(*i) {
                        let b = std::mem::replace(held, std::ptr::null_mut());
                        if !b.is_null() {
                            give_back(stream, b);
                            fed.pace[*i].lost_held();
                        }
                    }
                    fed.format(*i, format, size.width, size.height);
                    drop(fed);
                    if let Some(pod) = meta.as_deref().and_then(Pod::from_bytes)
                        && let Err(e) = stream.update_params(&mut [pod])
                    {
                        // The stills still come; only the pointer's place
                        // does not.
                        log::warn!("PipeWire: no cursor metadata: {e}");
                    }
                }
            })
            .remove_buffer({
                let feed = Rc::clone(&feed);
                move |_, i, b| {
                    // The stream took back the buffer held: forget it.
                    let mut fed = feed.borrow_mut();
                    if fed.held.get(*i).is_some_and(|h| *h == b) {
                        fed.held[*i] = std::ptr::null_mut();
                        fed.pace[*i].lost_held();
                    }
                }
            })
            .process({
                let feed = Rc::clone(&feed);
                move |stream, i| {
                    if stills {
                        drain_stills(stream, *i, &feed);
                        return;
                    }
                    // Only the newest picture matters; older ones go back.
                    let mut newest = None;
                    while let Some(b) = stream.dequeue_buffer() {
                        newest = Some(b);
                    }
                    let Some(mut buffer) = newest else {
                        return;
                    };
                    let Some(data) = buffer.datas_mut().first_mut() else {
                        return;
                    };
                    let kind = data.type_();
                    if kind != DataType::MemPtr && kind != DataType::MemFd {
                        feed.borrow_mut().fail(format!(
                            "PipeWire sent {kind:?} buffers, which Windup cannot read."
                        ));
                        return;
                    }
                    let chunk = data.chunk();
                    let chunk = (
                        chunk.offset(),
                        chunk.stride(),
                        chunk.size(),
                        chunk.flags().contains(ChunkFlags::CORRUPTED),
                    );
                    let Some(bytes) = data.data() else {
                        return;
                    };
                    feed.borrow_mut().frame(*i, bytes, chunk);
                }
            })
            .register()
            .map_err(err)?;
        let pod = Pod::from_bytes(&format).ok_or("could not describe the formats")?;
        stream
            .connect(
                spa::utils::Direction::Input,
                Some(s.node),
                StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
                &mut [pod],
            )
            .map_err(err)?;
        // The listener first, so it is dropped before its stream.
        running.push((listener, Rc::new(stream)));
    }
    let handles: Vec<Rc<pw::stream::Stream>> = running.iter().map(|(_, s)| Rc::clone(s)).collect();
    let timer = stills.then(|| {
        let timer = mainloop.loop_().add_timer({
            let (feed, handles) = (Rc::clone(&feed), handles.clone());
            move |_| tick_stills(&handles, &feed)
        });
        let _ = timer.update_timer(Some(STILLS_TICK), Some(STILLS_TICK));
        timer
    });

    let _stop = stop.attach(mainloop.loop_(), {
        let mainloop = mainloop.downgrade();
        move |()| {
            if let Some(ml) = mainloop.upgrade() {
                ml.quit();
            }
        }
    });
    mainloop.run();
    // The timer and the held buffers go before the streams.
    drop(timer);
    release_held(&handles, &feed);
    drop(handles);
    drop(running);
    Ok(())
}
