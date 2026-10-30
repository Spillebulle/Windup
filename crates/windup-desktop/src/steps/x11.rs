//! The X11 camera and concealer: `GetImage` of the monitor the press was on,
//! the pointer from `QueryPointer`, and the bar moved off the screen for the
//! instant of a shot of its own monitor.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use windup_core::capture::geometry::DRect;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConfigureWindowAux, ConnectionExt as _, Window};
use x11rb::rust_connection::RustConnection;

use super::camera::{Camera, Conceal, Shot};
use crate::capture::x11::{grab_monitor_at, monitor_list};
use crate::capture::{CaptureError, monitor_index_at};
use crate::linux::x11::window_named;

/// Where the bar goes while hidden: far off every monitor.
const AWAY_X: i32 = -32000;
/// How long a window manager may take to carry the move out.
const MOVE_CAP: Duration = Duration::from_millis(150);
/// Time for the windows the bar uncovered to paint themselves again, which
/// they do only once they hear they were uncovered.
const SETTLE: Duration = Duration::from_millis(30);

/// The root and a connection kept for the session, for `QueryPointer`.
pub struct X11Camera {
    conn: RustConnection,
    root: Window,
}

impl X11Camera {
    pub fn open() -> Result<Self, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| format!("no X display: {e}"))?;
        let root = conn.setup().roots.get(screen).ok_or("no X screen")?.root;
        Ok(Self { conn, root })
    }

    /// The pointer in root pixels; `None` when it is on another screen.
    fn pointer(&self) -> Option<(f32, f32)> {
        let reply = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        reply
            .same_screen
            .then_some((reply.root_x as f32, reply.root_y as f32))
    }

    /// The press point when there is one, else the pointer now.
    fn point(&self, at: Option<[f32; 2]>) -> Option<(f32, f32)> {
        at.map(|[x, y]| (x, y)).or_else(|| self.pointer())
    }
}

impl Camera for X11Camera {
    fn monitor(&mut self, at: Option<[f32; 2]>) -> Option<DRect> {
        let (x, y) = self.point(at)?;
        let list = monitor_list();
        list.get(monitor_index_at(&list, x, y)).copied()
    }

    fn shoot(&mut self, at: Option<[f32; 2]>) -> Result<Shot, CaptureError> {
        let point = self.point(at);
        let (x, y) = point.unwrap_or((0.0, 0.0));
        // Root pixels are the image's pixels less the monitor's origin.
        Ok(Shot::of(grab_monitor_at(x, y)?, point))
    }
}

/// Moves the steps bar off the screen and back, on a connection of its own
/// so the camera thread never waits on the UI thread.
pub struct X11Conceal {
    conn: RustConnection,
    root: Window,
    /// The bar and where it was, while it is away.
    away: Mutex<Option<(Window, i32, i32)>>,
}

impl X11Conceal {
    pub fn open() -> Option<Self> {
        let (conn, screen) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        Some(Self {
            conn,
            root,
            away: Mutex::new(None),
        })
    }

    /// The window's top-left corner on the root.
    fn origin(&self, win: Window) -> Option<(i32, i32)> {
        let at = self
            .conn
            .translate_coordinates(win, self.root, 0, 0)
            .ok()?
            .reply()
            .ok()?;
        Some((at.dst_x as i32, at.dst_y as i32))
    }

    /// Moves `win` and waits until the server has done it: a `GetInputFocus`
    /// round trip, and, where a window manager carries the move out, until
    /// the window is there or `MOVE_CAP` runs out.
    fn move_to(&self, win: Window, x: i32, y: i32) -> bool {
        let aux = ConfigureWindowAux::new().x(x).y(y);
        if self.conn.configure_window(win, &aux).is_err() {
            return false;
        }
        let sync = || {
            self.conn
                .get_input_focus()
                .ok()
                .and_then(|c| c.reply().ok())
                .is_some()
        };
        if !sync() {
            return false;
        }
        let deadline = Instant::now() + MOVE_CAP;
        // Near enough: decorations put the client a little off the frame.
        while self
            .origin(win)
            .is_some_and(|(at_x, _)| (at_x - x).abs() > 100)
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    }
}

impl Conceal for X11Conceal {
    fn hide(&self, _shot_monitor: DRect) -> bool {
        let Some(win) = window_named(crate::recording::BAR_TITLE) else {
            return false;
        };
        let Some((x, y)) = self.origin(win) else {
            return false;
        };
        // Remembered first: a move that went out but was never confirmed
        // must still be undone.
        *self.away.lock().unwrap_or_else(|e| e.into_inner()) = Some((win, x, y));
        if !self.move_to(win, AWAY_X, y) {
            self.show();
            return false;
        }
        std::thread::sleep(SETTLE);
        true
    }

    fn show(&self) {
        let away = self.away.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some((win, x, y)) = away {
            let aux = ConfigureWindowAux::new().x(x).y(y);
            let _ = self.conn.configure_window(win, &aux);
            let _ = self.conn.flush();
        }
    }
}

impl Drop for X11Conceal {
    /// A session that ends mid-shot (a camera thread that panicked) still
    /// puts the bar back.
    fn drop(&mut self) {
        self.show();
    }
}
