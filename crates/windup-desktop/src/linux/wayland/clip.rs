//! The clipboard through data-control: told about every copy without having
//! focus, and able to own the selection while the app has no window at all.
//!
//! `ext_data_control_v1` and its wlroots predecessor have the same shape
//! under different names, so the four proxies are wrapped in enums and one
//! macro writes the dispatch for both.

use super::State;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1 as ext_device, ext_data_control_manager_v1 as ext_manager,
    ext_data_control_offer_v1 as ext_offer, ext_data_control_source_v1 as ext_source,
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1 as wlr_device, zwlr_data_control_manager_v1 as wlr_manager,
    zwlr_data_control_offer_v1 as wlr_offer, zwlr_data_control_source_v1 as wlr_source,
};
use windup_core::clipboard::capture::{self, Capture, OFFER_TEXT};

/// Offered alongside our own data, so the selection event that echoes our
/// own copy can be told from somebody else's.
const OWN_MARKER: &str = "application/x-windup-owned";

/// Longest a read of somebody else's copy may take. A client that never
/// answers must not leave a thread blocked for ever.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub enum Manager {
    Ext(ext_manager::ExtDataControlManagerV1),
    Wlr(wlr_manager::ZwlrDataControlManagerV1),
}

#[derive(Clone)]
pub enum Device {
    Ext(ext_device::ExtDataControlDeviceV1),
    Wlr(wlr_device::ZwlrDataControlDeviceV1),
}

#[derive(Clone, PartialEq)]
pub enum Offer {
    Ext(ext_offer::ExtDataControlOfferV1),
    Wlr(wlr_offer::ZwlrDataControlOfferV1),
}

#[derive(Clone, PartialEq)]
pub enum Source {
    Ext(ext_source::ExtDataControlSourceV1),
    Wlr(wlr_source::ZwlrDataControlSourceV1),
}

impl Manager {
    fn device(&self, seat: &WlSeat, qh: &QueueHandle<State>) -> Device {
        match self {
            Manager::Ext(m) => Device::Ext(m.get_data_device(seat, qh, ())),
            Manager::Wlr(m) => Device::Wlr(m.get_data_device(seat, qh, ())),
        }
    }

    fn source(&self, qh: &QueueHandle<State>) -> Source {
        match self {
            Manager::Ext(m) => Source::Ext(m.create_data_source(qh, ())),
            Manager::Wlr(m) => Source::Wlr(m.create_data_source(qh, ())),
        }
    }
}

impl Device {
    fn set_selection(&self, source: &Source) {
        match (self, source) {
            (Device::Ext(d), Source::Ext(s)) => d.set_selection(Some(s)),
            (Device::Wlr(d), Source::Wlr(s)) => d.set_selection(Some(s)),
            _ => unreachable!("one manager makes both"),
        }
    }
}

impl Offer {
    fn id(&self) -> ObjectId {
        match self {
            Offer::Ext(o) => o.id(),
            Offer::Wlr(o) => o.id(),
        }
    }

    /// Asks the owner to write `mime` into a pipe, and returns its read end.
    fn receive(&self, mime: &str) -> Option<OwnedFd> {
        let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).ok()?;
        match self {
            Offer::Ext(o) => o.receive(mime.to_owned(), write.as_fd()),
            Offer::Wlr(o) => o.receive(mime.to_owned(), write.as_fd()),
        }
        // Our copy of the write end closes here; the owner holds its own.
        Some(read)
    }

    fn destroy(&self) {
        match self {
            Offer::Ext(o) => o.destroy(),
            Offer::Wlr(o) => o.destroy(),
        }
    }
}

impl Source {
    fn offer(&self, mime: &str) {
        match self {
            Source::Ext(s) => s.offer(mime.to_owned()),
            Source::Wlr(s) => s.offer(mime.to_owned()),
        }
    }

    fn destroy(&self) {
        match self {
            Source::Ext(s) => s.destroy(),
            Source::Wlr(s) => s.destroy(),
        }
    }
}

/// What we serve while we own the selection.
#[derive(Clone, Debug, PartialEq)]
pub enum Payload {
    Text(String),
    /// PNG bytes, and the pixels they came from for our own history.
    Png {
        png: Vec<u8>,
        rgba: Vec<u8>,
        width: u32,
        height: u32,
    },
}

impl Payload {
    fn mimes(&self) -> &'static [&'static str] {
        match self {
            Payload::Text(_) => &OFFER_TEXT,
            Payload::Png { .. } => &["image/png"],
        }
    }

    fn bytes_for(&self, mime: &str) -> Option<&[u8]> {
        match self {
            Payload::Text(t) if OFFER_TEXT.contains(&mime) => Some(t.as_bytes()),
            Payload::Png { png, .. } if mime == "image/png" => Some(png),
            _ => None,
        }
    }

    pub fn capture(&self) -> Capture {
        match self {
            Payload::Text(t) => Capture::Text {
                text: t.clone(),
                html: None,
            },
            Payload::Png {
                rgba,
                width,
                height,
                ..
            } => Capture::Image {
                rgba: rgba.clone(),
                width: *width,
                height: *height,
            },
        }
    }
}

/// The data-control half of the Wayland state.
pub struct Clip {
    manager: Manager,
    device: Option<Device>,
    /// Offers announced but not yet made the selection, with their types.
    pending: HashMap<ObjectId, (Offer, Vec<String>)>,
    /// The current selection, if it is somebody else's.
    current: Option<(Offer, Vec<String>)>,
    /// Ours, while we own the selection.
    own: Option<(Source, Arc<Payload>)>,
}

impl Clip {
    pub fn new(manager: Manager) -> Self {
        Self {
            manager,
            device: None,
            pending: HashMap::new(),
            current: None,
            own: None,
        }
    }

    pub fn attach(&mut self, seat: &WlSeat, qh: &QueueHandle<State>) {
        if self.device.is_none() {
            self.device = Some(self.manager.device(seat, qh));
        }
    }

    /// Takes the selection with `payload`.
    pub fn set(&mut self, payload: Payload, qh: &QueueHandle<State>) {
        let Some(device) = &self.device else {
            log::warn!("no data-control device yet; clipboard not set");
            return;
        };
        let source = self.manager.source(qh);
        for mime in payload.mimes() {
            source.offer(mime);
        }
        source.offer(OWN_MARKER);
        device.set_selection(&source);
        if let Some((old, _)) = self.own.replace((source, Arc::new(payload))) {
            old.destroy();
        }
    }

    /// Whether the selection is ours and is text.
    pub fn owns_text(&self) -> bool {
        self.current.is_none()
            && self
                .own
                .as_ref()
                .is_some_and(|(_, p)| matches!(**p, Payload::Text(_)))
    }

    /// The types the current selection offers, or `None` when it is ours or
    /// empty.
    pub fn current_mimes(&self) -> Option<&[String]> {
        self.current.as_ref().map(|(_, m)| m.as_slice())
    }

    /// Reads `mime` (and `extra`, if any) from the current selection on a
    /// thread, and hands the bytes to `done` there.
    pub fn read_current(
        &self,
        conn: &Connection,
        mime: &str,
        extra: Option<&str>,
        done: impl FnOnce(Option<Vec<u8>>, Option<Vec<u8>>) + Send + 'static,
    ) {
        let Some((offer, _)) = &self.current else {
            return;
        };
        let main = offer.receive(mime);
        let side = extra.and_then(|m| offer.receive(m));
        // The owner only learns of the request once it is sent.
        let _ = conn.flush();
        thread::spawn(move || {
            let main = main.and_then(read_all);
            let side = side.and_then(read_all);
            done(main, side);
        });
    }

    fn new_offer(&mut self, offer: Offer) {
        self.pending.insert(offer.id(), (offer, Vec::new()));
    }

    fn add_mime(&mut self, id: ObjectId, mime: String) {
        if let Some((_, mimes)) = self.pending.get_mut(&id) {
            mimes.push(mime);
        }
    }

    /// The selection changed. Returns what the caller should do about it.
    fn selection(&mut self, offer: Option<Offer>) -> Selection {
        if let Some((old, _)) = self.current.take() {
            old.destroy();
        }
        let Some(offer) = offer else {
            return Selection::Cleared;
        };
        let mimes = self
            .pending
            .remove(&offer.id())
            .map(|(_, m)| m)
            .unwrap_or_default();
        if mimes.iter().any(|m| m == OWN_MARKER) {
            offer.destroy();
            return match &self.own {
                Some((_, payload)) => Selection::Ours(Arc::clone(payload)),
                None => Selection::Cleared,
            };
        }
        self.current = Some((offer, mimes));
        Selection::Theirs
    }

    fn drop_offer(&mut self, offer: Offer) {
        self.pending.remove(&offer.id());
        offer.destroy();
    }

    fn send(&self, source: &Source, mime: &str, fd: OwnedFd) {
        let Some((own, payload)) = &self.own else {
            return;
        };
        if own != source {
            return;
        }
        let payload = Arc::clone(payload);
        let mime = mime.to_owned();
        // A large image must not stall the event loop while the reader
        // drains the pipe.
        thread::spawn(move || {
            if let Some(bytes) = payload.bytes_for(&mime) {
                let mut file = std::fs::File::from(fd);
                let _ = file.write_all(bytes);
            }
        });
    }

    fn cancelled(&mut self, source: &Source) {
        if self.own.as_ref().is_some_and(|(s, _)| s == source) {
            self.own = None;
        }
        source.destroy();
    }

    fn finished(&mut self) {
        self.device = None;
    }
}

pub enum Selection {
    Cleared,
    /// The echo of our own copy.
    Ours(Arc<Payload>),
    /// Somebody else's; its types are in `current_mimes`.
    Theirs,
}

fn read_all(fd: OwnedFd) -> Option<Vec<u8>> {
    use rustix::event::{PollFd, PollFlags, poll};
    let deadline = Instant::now() + READ_TIMEOUT;
    let mut file = std::fs::File::from(fd);
    let mut out = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        let timeout = rustix::event::Timespec::try_from(left).ok()?;
        let mut fds = [PollFd::new(&file, PollFlags::IN)];
        if poll(&mut fds, Some(&timeout)).ok()? == 0 {
            log::warn!("clipboard owner did not answer in time");
            return None;
        }
        match file.read(&mut buf) {
            Ok(0) => return Some(out),
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
}

/// Turns what was read into a history capture.
pub fn decode(pick: &capture::Pick, main: Vec<u8>, html: Option<Vec<u8>>) -> Option<Capture> {
    if pick.is_image {
        let img = image::load_from_memory_with_format(&main, image::ImageFormat::Png).ok()?;
        let rgba = img.to_rgba8();
        let (width, height) = rgba.dimensions();
        return Some(Capture::Image {
            rgba: rgba.into_raw(),
            width,
            height,
        });
    }
    let text = String::from_utf8(main)
        .ok()
        .map(|t| t.trim_end_matches('\0').to_owned())?;
    Some(Capture::Text {
        text,
        html: html.and_then(|h| String::from_utf8(h).ok()),
    })
}

pub fn png(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let img = image::RgbaImage::from_raw(width, height, rgba.to_vec())?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

macro_rules! dispatch_family {
    ($Kind:ident, $Mgr:ty, $Dev:ty, $Off:ty, $Src:ty, $dev:ident, $off:ident, $src:ident) => {
        impl Dispatch<$Mgr, ()> for State {
            fn event(
                _: &mut State,
                _: &$Mgr,
                _: <$Mgr as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<State>,
            ) {
            }
        }

        impl Dispatch<$Dev, ()> for State {
            fn event(
                state: &mut State,
                _: &$Dev,
                event: $dev::Event,
                _: &(),
                conn: &Connection,
                qh: &QueueHandle<State>,
            ) {
                let Some(clip) = state.clip.as_mut() else {
                    return;
                };
                match event {
                    $dev::Event::DataOffer { id } => clip.new_offer(Offer::$Kind(id)),
                    $dev::Event::Selection { id } => {
                        let what = clip.selection(id.map(Offer::$Kind));
                        state.on_selection(what, conn, qh);
                    }
                    $dev::Event::PrimarySelection { id: Some(id) } => {
                        clip.drop_offer(Offer::$Kind(id))
                    }
                    $dev::Event::Finished => clip.finished(),
                    _ => {}
                }
            }

            event_created_child!(State, $Dev, [
                $dev::EVT_DATA_OFFER_OPCODE => ($Off, ()),
            ]);
        }

        impl Dispatch<$Off, ()> for State {
            fn event(
                state: &mut State,
                offer: &$Off,
                event: $off::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<State>,
            ) {
                if let $off::Event::Offer { mime_type } = event
                    && let Some(clip) = state.clip.as_mut()
                {
                    clip.add_mime(offer.id(), mime_type);
                }
            }
        }

        impl Dispatch<$Src, ()> for State {
            fn event(
                state: &mut State,
                source: &$Src,
                event: $src::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<State>,
            ) {
                let Some(clip) = state.clip.as_mut() else {
                    return;
                };
                let source = Source::$Kind(source.clone());
                match event {
                    $src::Event::Send { mime_type, fd } => clip.send(&source, &mime_type, fd),
                    $src::Event::Cancelled => clip.cancelled(&source),
                    _ => {}
                }
            }
        }
    };
}

dispatch_family!(
    Ext,
    ext_manager::ExtDataControlManagerV1,
    ext_device::ExtDataControlDeviceV1,
    ext_offer::ExtDataControlOfferV1,
    ext_source::ExtDataControlSourceV1,
    ext_device,
    ext_offer,
    ext_source
);
dispatch_family!(
    Wlr,
    wlr_manager::ZwlrDataControlManagerV1,
    wlr_device::ZwlrDataControlDeviceV1,
    wlr_offer::ZwlrDataControlOfferV1,
    wlr_source::ZwlrDataControlSourceV1,
    wlr_device,
    wlr_offer,
    wlr_source
);
