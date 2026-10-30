//! The Wayland camera on COSMIC and wlroots: a screencopy still of the one
//! output the pointer is on.
//!
//! Where the pointer is comes from the compositor, by the route
//! `steps_order` chose:
//! - `CursorSession`: an `ext_image_copy_capture_v1` cursor session per
//!   output, kept open for the session on a thread of its own
//!   (`CursorTracker`); it gives the output and the place in its pixels.
//! - `Hyprland`: IPC `cursorpos`, a point in the layout.
//! - `SwayFocus`: Sway only says which output has focus.
//! - `Unknown`: the first output.
//!
//! Where the place is not known the cursor is painted into the picture
//! instead, so the step still shows what was pointed at.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::{delegate_output, delegate_registry, registry_handlers};
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_callback, wl_output, wl_pointer, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_cursor_session_v1::{
        self as cursor, ExtImageCopyCaptureCursorSessionV1,
    },
    ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1,
};
use windup_core::capture::backend::WlPointer;
use windup_core::capture::geometry::DRect;

use crate::capture::CaptureError;
use crate::capture::ipcrects::{hyprland_cursor_live, sway_focused_live};
use crate::linux::wayland::screencopy::{
    Method, grab_output, output_layout_from_env, outputs_of, read_events, silent, wait_until,
};
use crate::steps::camera::{Camera, Shot, describe};

/// How long after its sessions open a tracker waits for one to say the
/// cursor is on its output.
const FIRST_ENTER: Duration = Duration::from_millis(300);
/// The most the tracker may take to connect and open its sessions.
const SETUP_CAP: Duration = Duration::from_secs(2);
/// How long the tracker thread waits for events before looking at its stop
/// flag again.
const POLL: Duration = Duration::from_millis(100);

/// Whether a step's picture gets the cursor painted in: only when the
/// pointer's place is not known, so the step still shows what was pointed at.
pub(crate) fn paint_cursor(pointer: WlPointer, tracker_found: bool) -> bool {
    match pointer {
        WlPointer::CursorSession => !tracker_found,
        WlPointer::Hyprland => false,
        WlPointer::SwayFocus | WlPointer::Unknown => true,
    }
}

/// What a cursor-session camera does without a tracker.
fn fallback(swaysock: bool) -> WlPointer {
    if swaysock {
        WlPointer::SwayFocus
    } else {
        WlPointer::Unknown
    }
}

/// What one output's cursor session has said.
#[derive(Clone, Debug, Default)]
struct Seen {
    name: String,
    /// The output's pixels, upright, when its mode is known.
    size: Option<(i32, i32)>,
    /// Between `enter` and `leave`.
    inside: bool,
    /// The hotspot's place in the output's pixels: the protocol gives the
    /// pointer's tip itself, never the cursor image's corner.
    position: Option<(i32, i32)>,
    /// When it last moved here, to choose between two outputs that both
    /// have a corner of the cursor image.
    stamp: u64,
}

/// The output the pointer is on and its place in that output's pixels. An
/// output whose session puts the point inside its pixels beats one that only
/// has a corner of the cursor image; then the latest to move wins.
fn pick_cursor(seen: &[Seen]) -> Option<(String, [f32; 2])> {
    seen.iter()
        .filter_map(|s| {
            let (x, y) = s.position?;
            let p = [x as f32, y as f32];
            let on = s.size.is_none_or(|(w, h)| {
                p[0] >= 0.0 && p[1] >= 0.0 && p[0] < w as f32 && p[1] < h as f32
            });
            s.inside.then_some(((on, s.stamp), (s.name.clone(), p)))
        })
        .max_by_key(|(key, _)| *key)
        .map(|(_, found)| found)
}

/// Cursor sessions on every output, followed on a connection and thread of
/// their own for as long as the camera lives.
pub struct CursorTracker {
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl CursorTracker {
    /// `None` when the compositor has no cursor sessions, the seat no
    /// pointer, or no session says the cursor is on its output within
    /// `FIRST_ENTER` of opening.
    pub fn start() -> Option<CursorTracker> {
        let conn = Connection::connect_to_env().ok()?;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("windup-cursor".into())
            .spawn({
                let (conn, seen, stop) = (conn.clone(), seen.clone(), stop.clone());
                move || track(conn, seen, stop, tx)
            })
            .ok()?;
        let mut tracker = CursorTracker {
            seen,
            stop,
            thread: Some(thread),
        };
        match rx.recv_timeout(SETUP_CAP + FIRST_ENTER) {
            Ok(Ok(())) => return Some(tracker),
            Ok(Err(why)) => log::info!("steps: no cursor session: {why}"),
            Err(_) => log::info!("steps: the cursor sessions did not open in time"),
        }
        // The thread's own waits end by `SETUP_CAP`, except the registry's
        // first roundtrip, which has no deadline: closing the socket ends
        // that one too. Not joined, so the camera never waits on it.
        hang_up(&conn);
        tracker.thread.take();
        None
    }

    /// The output the pointer is on and the pointer's place in its pixels.
    pub fn where_now(&self) -> Option<(String, [f32; 2])> {
        pick_cursor(&self.seen.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// Shuts the connection's socket, so whatever waits on it gets an error.
fn hang_up(conn: &Connection) {
    use std::os::fd::AsFd;
    if let Ok(fd) = conn.backend().poll_fd().as_fd().try_clone_to_owned() {
        let _ = std::os::unix::net::UnixStream::from(fd).shutdown(std::net::Shutdown::Both);
    }
}

impl Drop for CursorTracker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct TrackState {
    registry: RegistryState,
    output_state: OutputState,
    has_pointer: bool,
    seen: Arc<Mutex<Vec<Seen>>>,
    clock: u64,
    /// `wl_display.sync` callbacks answered.
    synced: u64,
}

impl TrackState {
    fn any_inside(&self) -> bool {
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|s| s.inside)
    }
}

/// The tracker thread: opens the sessions, says on `ready` whether the
/// cursor was found, then follows it until `stop`. Everything it bound goes
/// with its connection when it returns.
fn track(
    conn: Connection,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
    ready: mpsc::Sender<Result<(), String>>,
) {
    let setup = Instant::now() + SETUP_CAP;
    let opened = open_sessions(&conn, seen, setup).and_then(|(mut queue, mut state)| {
        let deadline = Instant::now() + FIRST_ENTER;
        wait_until(&mut queue, &mut state, deadline, TrackState::any_inside)
            .map_err(|_| "the cursor is on no output's session".to_owned())?;
        Ok((queue, state))
    });
    let (mut queue, mut state) = match opened {
        Ok(opened) => opened,
        Err(why) => {
            let _ = ready.send(Err(why));
            return;
        }
    };
    if ready.send(Ok(())).is_err() || stop.load(Ordering::Relaxed) {
        return;
    }
    while !stop.load(Ordering::Relaxed) {
        let followed = queue
            .dispatch_pending(&mut state)
            .map_err(|e| e.to_string())
            .and_then(|_| read_events(&queue, POLL).map_err(|e| describe(&e)));
        if let Err(why) = followed {
            log::warn!("steps: the cursor sessions ended: {why}");
            // Nothing is known any more: the pointer is on no output.
            for s in seen_of(&state).iter_mut() {
                s.inside = false;
            }
            return;
        }
    }
}

fn seen_of(state: &TrackState) -> std::sync::MutexGuard<'_, Vec<Seen>> {
    state.seen.lock().unwrap_or_else(|e| e.into_inner())
}

/// A roundtrip that gives up at `deadline`, so a silent compositor ends the
/// thread instead of holding it.
fn sync(
    conn: &Connection,
    queue: &mut EventQueue<TrackState>,
    state: &mut TrackState,
    deadline: Instant,
) -> Result<(), String> {
    let want = state.synced + 1;
    conn.display().sync(&queue.handle(), ());
    wait_until(queue, state, deadline, |s| s.synced >= want).map_err(|e| describe(&e))
}

/// The seat's pointer and a cursor session per output, set up by `deadline`.
fn open_sessions(
    conn: &Connection,
    seen: Arc<Mutex<Vec<Seen>>>,
    deadline: Instant,
) -> Result<(EventQueue<TrackState>, TrackState), String> {
    // Its one roundtrip has no deadline; `start` hangs up if it never ends.
    let (globals, mut queue) =
        registry_queue_init::<TrackState>(conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let source: ExtOutputImageCaptureSourceManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|_| "no output capture sources".to_owned())?;
    let copy: ExtImageCopyCaptureManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|_| "no image copy capture".to_owned())?;
    let seat: wl_seat::WlSeat = globals
        .bind(&qh, 1..=5, ())
        .map_err(|_| "no seat".to_owned())?;
    let mut state = TrackState {
        registry: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        has_pointer: false,
        seen,
        clock: 0,
        synced: 0,
    };
    // Two roundtrips: the first delivers wl_output and the seat's
    // capabilities, the second xdg_output.
    for _ in 0..2 {
        sync(conn, &mut queue, &mut state, deadline)?;
    }
    if !state.has_pointer {
        return Err("the seat has no pointer".to_owned());
    }
    let pointer = seat.get_pointer(&qh, ());
    let outputs = outputs_of(&state.output_state);
    if outputs.is_empty() {
        return Err("the compositor reports no outputs".to_owned());
    }
    let mut list = Vec::new();
    for (i, out) in outputs.iter().enumerate() {
        let size = state.output_state.info(&out.output).and_then(|info| {
            let (w, h) = info.modes.iter().find(|m| m.current)?.dimensions;
            use wl_output::Transform as T;
            let sideways = matches!(
                info.transform,
                T::_90 | T::_270 | T::Flipped90 | T::Flipped270
            );
            Some(if sideways { (h, w) } else { (w, h) })
        });
        list.push(Seen {
            name: out.name.clone(),
            size,
            ..Seen::default()
        });
        let src = source.create_source(&out.output, &qh, ());
        copy.create_pointer_cursor_session(&src, &pointer, &qh, i);
    }
    *seen_of(&state) = list;
    queue.flush().map_err(|e| e.to_string())?;
    Ok((queue, state))
}

impl Dispatch<wl_seat::WlSeat, ()> for TrackState {
    fn event(
        state: &mut Self,
        _: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            state.has_pointer = caps.contains(wl_seat::Capability::Pointer);
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for TrackState {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.synced += 1;
        }
    }
}

impl Dispatch<ExtImageCopyCaptureCursorSessionV1, usize> for TrackState {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureCursorSessionV1,
        event: cursor::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.clock += 1;
        let clock = state.clock;
        let mut seen = seen_of(state);
        if let Some(s) = seen.get_mut(*index) {
            note(s, event, clock);
        }
    }
}

/// One cursor session event, heard at `clock`. `hotspot` is ignored: the
/// position is the hotspot's own place already.
fn note(s: &mut Seen, event: cursor::Event, clock: u64) {
    match event {
        cursor::Event::Enter => {
            s.inside = true;
            s.stamp = clock;
        }
        cursor::Event::Leave => {
            s.inside = false;
            s.position = None;
        }
        cursor::Event::Position { x, y } => {
            s.position = Some((x, y));
            s.stamp = clock;
        }
        _ => {}
    }
}

silent!(
    TrackState;
    wl_pointer::WlPointer,
    ExtImageCaptureSourceV1,
    ExtOutputImageCaptureSourceManagerV1,
    ExtImageCopyCaptureManagerV1,
);

impl OutputHandler for TrackState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ProvidesRegistryState for TrackState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}

delegate_output!(TrackState);
delegate_registry!(TrackState);

/// Where a shot's pointer is, as the route found it.
enum Place {
    /// In the output's pixels (a cursor session).
    Pixels([f32; 2]),
    /// In the layout (Hyprland).
    Layout((f32, f32)),
    None,
}

/// The output to picture this time, and how its pointer was found.
struct Aim {
    name: String,
    layout: DRect,
    place: Place,
    /// The route that found it: a failed lookup counts as `Unknown`.
    found_by: WlPointer,
}

/// The screencopy camera for COSMIC and wlroots.
pub struct ScreencopyCamera {
    method: Method,
    /// The route actually used: `CursorSession` only with a tracker.
    pointer: WlPointer,
    tracker: Option<CursorTracker>,
    /// What `monitor` found, for the shot that follows it: one lookup per
    /// shot, and the bar is hidden for the output that is pictured.
    aimed: Option<Aim>,
}

impl ScreencopyCamera {
    pub fn new(ext: bool, pointer: WlPointer) -> ScreencopyCamera {
        let method = if ext {
            Method::ExtImageCopy
        } else {
            Method::WlrScreencopy
        };
        let tracker = if pointer == WlPointer::CursorSession {
            CursorTracker::start()
        } else {
            None
        };
        let pointer = if pointer == WlPointer::CursorSession && tracker.is_none() {
            fallback(std::env::var_os("SWAYSOCK").is_some())
        } else {
            pointer
        };
        ScreencopyCamera {
            method,
            pointer,
            tracker,
            aimed: None,
        }
    }

    /// The output under the pointer, by the route. The press point the
    /// click watcher passes is never known on Wayland (evdev has none), so
    /// the route is asked each time.
    fn aim(&self) -> Result<Aim, CaptureError> {
        let layout = output_layout_from_env()?;
        let named = |name: &str| layout.iter().find(|(n, _)| n == name).cloned();
        let found = match self.pointer {
            WlPointer::CursorSession => self
                .tracker
                .as_ref()
                .and_then(CursorTracker::where_now)
                .and_then(|(name, p)| Some((named(&name)?, Place::Pixels(p)))),
            WlPointer::Hyprland => hyprland_cursor_live().and_then(|p| {
                let at = layout.iter().find(|(_, r)| r.contains(p.0, p.1))?;
                Some((at.clone(), Place::Layout(p)))
            }),
            WlPointer::SwayFocus => sway_focused_live()
                .and_then(|(name, _)| named(&name))
                .map(|at| (at, Place::None)),
            WlPointer::Unknown => None,
        };
        let ((name, rect), place, found_by) = match found {
            Some((at, place)) => (at, place, self.pointer),
            // Validated non-empty by output_layout_from_env.
            None => (layout[0].clone(), Place::None, WlPointer::Unknown),
        };
        Ok(Aim {
            name,
            layout: rect,
            place,
            found_by,
        })
    }
}

impl Camera for ScreencopyCamera {
    fn monitor(&mut self, _at: Option<[f32; 2]>) -> Option<DRect> {
        self.aimed = self.aim().ok();
        self.aimed.as_ref().map(|aim| aim.layout)
    }

    fn shoot(&mut self, _at: Option<[f32; 2]>) -> Result<Shot, CaptureError> {
        let aim = match self.aimed.take() {
            Some(aim) => aim,
            None => self.aim()?,
        };
        let paint = paint_cursor(aim.found_by, self.tracker.is_some());
        let m = grab_output(self.method, &aim.name, paint)?;
        Ok(match aim.place {
            Place::Layout(p) => Shot::of(m, Some(p)),
            Place::None => Shot::of(m, None),
            Place::Pixels(p) => {
                let (w, h) = (m.image.width() as f32, m.image.height() as f32);
                let mut shot = Shot::of(m, None);
                let on = p[0] >= 0.0 && p[1] >= 0.0 && p[0] < w && p[1] < h;
                shot.pointer = on.then_some(p);
                shot
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_the_cursor_only_when_its_place_is_unknown() {
        assert!(!paint_cursor(WlPointer::CursorSession, true));
        assert!(paint_cursor(WlPointer::CursorSession, false));
        assert!(!paint_cursor(WlPointer::Hyprland, false));
        assert!(paint_cursor(WlPointer::SwayFocus, false));
        assert!(paint_cursor(WlPointer::Unknown, true));
    }

    #[test]
    fn without_a_tracker_sway_falls_back_to_its_focus() {
        assert_eq!(fallback(true), WlPointer::SwayFocus);
        assert_eq!(fallback(false), WlPointer::Unknown);
    }

    fn seen(name: &str, inside: bool, position: Option<(i32, i32)>, stamp: u64) -> Seen {
        Seen {
            name: name.into(),
            size: Some((1920, 1080)),
            inside,
            position,
            stamp,
        }
    }

    #[test]
    fn the_cursor_is_on_the_output_whose_pixels_hold_its_point() {
        // Near the edge both outputs have the cursor image; the point is on
        // the left one even though the right one moved last.
        let both = [
            seen("DP-1", true, Some((1918, 500)), 1),
            seen("DP-2", true, Some((-2, 500)), 2),
        ];
        assert_eq!(pick_cursor(&both), Some(("DP-1".into(), [1918.0, 500.0])));
        // Otherwise the latest to move wins, and an output it left is out.
        let moved = [
            seen("DP-1", false, None, 3),
            seen("DP-2", true, Some((10, 20)), 2),
        ];
        assert_eq!(pick_cursor(&moved), Some(("DP-2".into(), [10.0, 20.0])));
        assert_eq!(pick_cursor(&[seen("DP-1", true, None, 1)]), None);
        assert_eq!(pick_cursor(&[]), None);
    }

    #[test]
    fn a_hotspot_does_not_move_the_point() {
        // The protocol's position is the hotspot's own place.
        let mut s = seen("DP-1", false, None, 0);
        note(&mut s, cursor::Event::Enter, 1);
        note(&mut s, cursor::Event::Position { x: 100, y: 50 }, 2);
        note(&mut s, cursor::Event::Hotspot { x: 4, y: 2 }, 3);
        assert_eq!(pick_cursor(&[s]), Some(("DP-1".into(), [100.0, 50.0])));
    }
}
