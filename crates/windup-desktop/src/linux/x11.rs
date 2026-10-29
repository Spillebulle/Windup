//! The X11 pieces: where the pointer is, the usable area of the monitor it is
//! on, which window has focus, and giving focus back. All through EWMH, which
//! every window manager still in use speaks, and RandR for the monitors.
//!
//! Each call opens its own short connection: they happen once per hotkey, and
//! a connection kept for ever would outlive an X server restart.

use windup_core::placement::Area;
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask, InputFocus, KeyButMask, MapState,
    PropMode, Window,
};
use x11rb::wrapper::ConnectionExt as _;

pub(crate) fn connect() -> Option<(x11rb::rust_connection::RustConnection, Window)> {
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    Some((conn, root))
}

pub(crate) fn atom(conn: &impl Connection, name: &str) -> Option<u32> {
    Some(
        conn.intern_atom(false, name.as_bytes())
            .ok()?
            .reply()
            .ok()?
            .atom,
    )
}

pub(crate) fn cardinals(
    conn: &impl Connection,
    win: Window,
    prop: &str,
    ty: AtomEnum,
) -> Option<Vec<u32>> {
    let prop = atom(conn, prop)?;
    let reply = conn
        .get_property(false, win, prop, ty, 0, 1024)
        .ok()?
        .reply()
        .ok()?;
    Some(reply.value32()?.collect())
}

/// The pointer in physical pixels and the work area of the monitor it is on.
pub fn pointer_and_area() -> Option<((f32, f32), Area)> {
    let (conn, root) = connect()?;
    let p = conn.query_pointer(root).ok()?.reply().ok()?;
    let (px, py) = (p.root_x as i32, p.root_y as i32);

    // The monitor under the pointer, or the whole root window.
    let screen = &conn.setup().roots[0];
    let mut mon = (
        0,
        0,
        screen.width_in_pixels as i32,
        screen.height_in_pixels as i32,
    );
    if let Ok(cookie) = conn.randr_get_monitors(root, true)
        && let Ok(reply) = cookie.reply()
        && let Some(m) = reply.monitors.iter().find(|m| {
            let (x, y) = (m.x as i32, m.y as i32);
            px >= x && py >= y && px < x + m.width as i32 && py < y + m.height as i32
        })
    {
        mon = (m.x as i32, m.y as i32, m.width as i32, m.height as i32);
    }
    let (mut l, mut t, mut r, mut b) = (mon.0, mon.1, mon.0 + mon.2, mon.1 + mon.3);

    // _NET_WORKAREA spans every monitor and only knows about panels on the
    // outer edges, so it narrows the monitor rather than replacing it.
    let desktop = cardinals(&conn, root, "_NET_CURRENT_DESKTOP", AtomEnum::CARDINAL)
        .and_then(|v| v.first().copied())
        .unwrap_or(0) as usize;
    if let Some(wa) = cardinals(&conn, root, "_NET_WORKAREA", AtomEnum::CARDINAL)
        && let Some(w) = wa
            .as_chunks::<4>()
            .0
            .get(desktop)
            .or(wa.as_chunks::<4>().0.first())
    {
        let (wl, wt) = (w[0] as i32, w[1] as i32);
        let (wr, wb) = (wl + w[2] as i32, wt + w[3] as i32);
        if wl < r && wr > l && wt < b && wb > t {
            (l, t, r, b) = (l.max(wl), t.max(wt), r.min(wr), b.min(wb));
        }
    }

    Some((
        (px as f32, py as f32),
        Area {
            left: l as f32,
            top: t as f32,
            right: r as f32,
            bottom: b as f32,
        },
    ))
}

/// The window with focus, by `_NET_ACTIVE_WINDOW`, else the input focus.
pub fn active_window() -> Option<u32> {
    let (conn, root) = connect()?;
    if let Some(w) = cardinals(&conn, root, "_NET_ACTIVE_WINDOW", AtomEnum::WINDOW)
        .and_then(|v| v.first().copied())
        .filter(|&w| w != 0)
    {
        return Some(w);
    }
    let focus = conn.get_input_focus().ok()?.reply().ok()?.focus;
    (focus > 1).then_some(focus)
}

/// Gives focus back to `window`: an EWMH activation request, which a window
/// manager honours, then the input focus directly for one that has none.
pub fn activate(window: u32) {
    let Some((conn, root)) = connect() else {
        return;
    };
    if let Some(net_active) = atom(&conn, "_NET_ACTIVE_WINDOW") {
        // Source 2 is "pager": a request on the user's behalf, which window
        // managers do not subject to focus-stealing prevention.
        let event = ClientMessageEvent::new(32, window, net_active, [2, 0, 0, 0, 0]);
        let _ = conn.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        );
    }
    let _ = conn.set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME);
    let _ = conn.flush();
}

/// Modifiers still held after [`wait_keys_released`] gave up waiting.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    pub shift: bool,
    pub alt: bool,
    pub super_: bool,
}

/// Waits, at most `cap`, for every key to be physically up, so the shortcut
/// that asked for a paste does not mix into the simulated Ctrl+V. The same
/// wait the Windows paste makes; says which modifiers are still down.
pub fn wait_keys_released(cap: std::time::Duration) -> Held {
    let Some((conn, root)) = connect() else {
        return Held::default();
    };
    let start = std::time::Instant::now();
    while start.elapsed() < cap {
        let Some(keys) = conn.query_keymap().ok().and_then(|c| c.reply().ok()) else {
            return Held::default();
        };
        if keys.keys.iter().all(|&b| b == 0) {
            return Held::default();
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    let Some(mask) = conn
        .query_pointer(root)
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| r.mask)
    else {
        return Held::default();
    };
    Held {
        shift: mask.contains(KeyButMask::SHIFT),
        alt: mask.contains(KeyButMask::MOD1),
        super_: mask.contains(KeyButMask::MOD4),
    }
}

/// Asks for our window titled `title` to be fullscreen and above others
/// (`overlay::fullscreen_overlay` says why). Before it is mapped the window
/// is not in the client list yet and is found among the root's children.
pub fn fullscreen_above(title: &str) {
    use crate::overlay::{NetWmState, fullscreen_request};
    let Some((conn, root)) = connect() else {
        return;
    };
    let Some(window) = window_named(title).or_else(|| unmapped_named(&conn, root, title)) else {
        return;
    };
    let (Some(state), Some(full), Some(above)) = (
        atom(&conn, "_NET_WM_STATE"),
        atom(&conn, "_NET_WM_STATE_FULLSCREEN"),
        atom(&conn, "_NET_WM_STATE_ABOVE"),
    ) else {
        return;
    };
    let mapped = conn
        .get_window_attributes(window)
        .ok()
        .and_then(|c| c.reply().ok())
        .is_some_and(|a| a.map_state != MapState::UNMAPPED);
    match fullscreen_request(mapped, full, above) {
        NetWmState::Property(atoms) => {
            let _ =
                conn.change_property32(PropMode::REPLACE, window, state, AtomEnum::ATOM, &atoms);
        }
        NetWmState::Message(data) => {
            let event = ClientMessageEvent::new(32, window, state, data);
            let _ = conn.send_event(
                false,
                root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            );
        }
    }
    let _ = conn.flush();
}

/// A window of the root titled `title` (`_NET_WM_NAME`): ours, created but
/// not yet mapped, so not reparented or listed by the window manager.
fn unmapped_named(conn: &impl Connection, root: Window, title: &str) -> Option<u32> {
    let utf8 = atom(conn, "UTF8_STRING")?;
    let name = atom(conn, "_NET_WM_NAME")?;
    let children = conn.query_tree(root).ok()?.reply().ok()?.children;
    children.into_iter().find(|&w| {
        conn.get_property(false, w, name, utf8, 0, 256)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|r| r.value == title.as_bytes())
    })
}

/// One of our own top-level windows, by its exact `_NET_WM_NAME`, from the
/// window manager's client list.
pub fn window_named(title: &str) -> Option<u32> {
    let (conn, root) = connect()?;
    let utf8 = atom(&conn, "UTF8_STRING")?;
    let name = atom(&conn, "_NET_WM_NAME")?;
    let clients = cardinals(&conn, root, "_NET_CLIENT_LIST", AtomEnum::WINDOW)?;
    clients.into_iter().find(|&w| {
        conn.get_property(false, w, name, utf8, 0, 256)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|r| r.value == title.as_bytes())
    })
}
