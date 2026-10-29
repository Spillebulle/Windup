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

use std::cell::RefCell;
use std::io::Cursor;
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

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
    ShmVideoFormat, covered_by, frame_view, layout_from_streams, stream_scale,
};

use super::screencast::{PortalStream, STOPPED};
use super::stream::Latest;
use crate::video::outcome::SCREENS_NOT_SHARED;

/// Longest the streams may take to agree a format and say their size.
const SETUP_TIMEOUT: Duration = Duration::from_secs(5);

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
/// `streams` for `rect` (desktop units). Returns once the picture's size is
/// known; frames then arrive in `latest`.
pub(super) fn spawn(
    fd: OwnedFd,
    streams: &[PortalStream],
    rect: DRect,
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
            if let Err(e) = run(fd, streams, rect, &latest, ready.clone(), stop_rx) {
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

/// Everything the loop's callbacks share. They all run on the loop's
/// thread, one at a time.
struct Feed {
    rect: DRect,
    streams: Vec<PortalStream>,
    /// Each stream's agreed format and size in pixels.
    formats: Vec<Option<(ShmVideoFormat, u32, u32)>>,
    composer: Option<Composer>,
    canvas: Vec<u8>,
    /// Taken when the size is said (or setup fails).
    ready: Option<mpsc::Sender<Result<(u32, u32), String>>>,
    latest: Arc<Mutex<Latest>>,
    mainloop: pw::main_loop::WeakMainLoop,
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
        if !covered_by(self.rect, &layouts) {
            self.fail(SCREENS_NOT_SHARED.into());
            return;
        }
        let monitors: Vec<(DRect, f32, (u32, u32))> = layouts
            .iter()
            .zip(&agreed)
            .map(|(l, &(_, w, h))| (*l, stream_scale(w, l.w), (w, h)))
            .collect();
        let mut composer = Composer::new(self.rect, &monitors);
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
}

/// The loop thread: connect, start a stream per monitor, run until stopped.
fn run(
    fd: OwnedFd,
    streams: Vec<PortalStream>,
    rect: DRect,
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
    let feed = Rc::new(RefCell::new(Feed {
        rect,
        formats: vec![None; streams.len()],
        streams: streams.clone(),
        composer: None,
        canvas: Vec::new(),
        ready: Some(ready),
        latest: Arc::clone(latest),
        mainloop: mainloop.downgrade(),
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
                move |_, i, id, param| {
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
                    feed.borrow_mut()
                        .format(*i, format, size.width, size.height);
                }
            })
            .process({
                let feed = Rc::clone(&feed);
                move |stream, i| {
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
        running.push((listener, stream));
    }

    let _stop = stop.attach(mainloop.loop_(), {
        let mainloop = mainloop.downgrade();
        move |()| {
            if let Some(ml) = mainloop.upgrade() {
                ml.quit();
            }
        }
    });
    mainloop.run();
    drop(running);
    Ok(())
}
