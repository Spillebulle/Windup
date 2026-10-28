//! The X11 back end: one `GetImage` of the root window per RandR monitor,
//! and the window manager's stacking list for the picker.
//!
//! Sizes are physical pixels and the images are exactly their layouts' size
//! (X has no fractional scale), so `px_per_unit` is 1. The desktop's scaling
//! is `Xft.dpi`, which gives every monitor its `scale`.

use super::{CaptureError, XFormat, x_pixels_to_rgba, xft_scale};
use crate::linux::x11::{atom, cardinals, connect};
use windup_core::capture::geometry::{DRect, DesktopShot, MonitorShot};
use windup_core::capture::pick::WindowRect;
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, ImageFormat, ImageOrder, MapState, VisualClass, Window,
};

fn failed(what: &str, e: impl std::fmt::Display) -> CaptureError {
    CaptureError::Failed(format!("{what}: {e}"))
}

/// The root visual's pixel layout.
fn root_format(conn: &impl Connection, screen_num: usize) -> Result<XFormat, CaptureError> {
    let setup = conn.setup();
    let screen = &setup.roots[screen_num];
    let depth = screen
        .allowed_depths
        .iter()
        .find(|d| d.visuals.iter().any(|v| v.visual_id == screen.root_visual))
        .ok_or_else(|| CaptureError::Failed("the root visual is unknown".into()))?;
    let visual = depth
        .visuals
        .iter()
        .find(|v| v.visual_id == screen.root_visual)
        .expect("found above");
    if !matches!(
        visual.class,
        VisualClass::TRUE_COLOR | VisualClass::DIRECT_COLOR
    ) {
        return Err(CaptureError::Failed(format!(
            "unsupported visual class {:?}",
            visual.class
        )));
    }
    let bits_per_pixel = setup
        .pixmap_formats
        .iter()
        .find(|f| f.depth == depth.depth)
        .map(|f| f.bits_per_pixel)
        .ok_or_else(|| CaptureError::Failed("no pixmap format for the root depth".into()))?;
    Ok(XFormat {
        bits_per_pixel,
        lsb_first: setup.image_byte_order == ImageOrder::LSB_FIRST,
        red_mask: visual.red_mask,
        green_mask: visual.green_mask,
        blue_mask: visual.blue_mask,
    })
}

pub fn grab() -> Result<DesktopShot, CaptureError> {
    let (conn, screen_num) = x11rb::connect(None).map_err(|e| failed("no X display", e))?;
    let root = conn.setup().roots[screen_num].root;
    let format = root_format(&conn, screen_num)?;

    // RandR's monitors; the whole root when the extension is missing.
    let mut rects: Vec<(String, i32, i32, u32, u32)> = conn
        .randr_get_monitors(root, true)
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| {
            r.monitors
                .iter()
                .filter(|m| m.width > 0 && m.height > 0)
                .map(|m| {
                    (
                        atom_name(&conn, m.name).unwrap_or_default(),
                        m.x as i32,
                        m.y as i32,
                        m.width as u32,
                        m.height as u32,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if rects.is_empty() {
        let s = &conn.setup().roots[screen_num];
        rects.push((
            String::new(),
            0,
            0,
            s.width_in_pixels as u32,
            s.height_in_pixels as u32,
        ));
    }

    // One scale for every monitor: X has no per-monitor one.
    let scale = resources(&conn, root).map_or(1.0, |db| xft_scale(&db));
    let mut monitors = Vec::new();
    for (name, x, y, w, h) in rects {
        let reply = conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                root,
                x as i16,
                y as i16,
                w as u16,
                h as u16,
                !0,
            )
            .map_err(|e| failed("GetImage", e))?
            .reply()
            .map_err(|e| failed("GetImage", e))?;
        let image = x_pixels_to_rgba(&reply.data, w, h, format).map_err(CaptureError::Failed)?;
        monitors.push(MonitorShot {
            layout: DRect {
                x: x as f32,
                y: y as f32,
                w: w as f32,
                h: h as f32,
            },
            image,
            name,
            scale,
        });
    }
    Ok(DesktopShot { monitors })
}

/// The resource database the desktop set on the root window (`xrdb`).
fn resources(conn: &impl Connection, root: Window) -> Option<String> {
    let reply = conn
        .get_property(
            false,
            root,
            AtomEnum::RESOURCE_MANAGER,
            AtomEnum::STRING,
            0,
            1 << 16,
        )
        .ok()?
        .reply()
        .ok()?;
    String::from_utf8(reply.value).ok()
}

fn atom_name(conn: &impl Connection, atom: u32) -> Option<String> {
    let reply = conn.get_atom_name(atom).ok()?.reply().ok()?;
    String::from_utf8(reply.name).ok()
}

/// Client windows from the window manager's stacking list, topmost first,
/// with their decorations. Empty when the WM keeps no such list.
pub fn window_rects(_shot: &DesktopShot) -> Vec<WindowRect> {
    let Some((conn, root)) = connect() else {
        return Vec::new();
    };
    let Some(mut list) = cardinals(&conn, root, "_NET_CLIENT_LIST_STACKING", AtomEnum::WINDOW)
    else {
        return Vec::new();
    };
    // Bottom to top in the property.
    list.reverse();
    list.into_iter()
        .filter_map(|win| window_rect(&conn, root, win))
        .collect()
}

fn window_rect(conn: &impl Connection, root: Window, win: Window) -> Option<WindowRect> {
    let attrs = conn.get_window_attributes(win).ok()?.reply().ok()?;
    if attrs.map_state != MapState::VIEWABLE {
        return None;
    }
    let geo = conn.get_geometry(win).ok()?.reply().ok()?;
    // The client's own origin on the root, whatever the WM reparented it into.
    let at = conn
        .translate_coordinates(win, root, 0, 0)
        .ok()?
        .reply()
        .ok()?;
    let (left, right, top, bottom) =
        match cardinals(conn, win, "_NET_FRAME_EXTENTS", AtomEnum::CARDINAL) {
            Some(e) if e.len() >= 4 => (e[0], e[1], e[2], e[3]),
            _ => (0, 0, 0, 0),
        };
    let (w, h) = (
        geo.width as f32 + (left + right) as f32,
        geo.height as f32 + (top + bottom) as f32,
    );
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some(WindowRect {
        rect: DRect {
            x: at.dst_x as f32 - left as f32,
            y: at.dst_y as f32 - top as f32,
            w,
            h,
        },
        title: title(conn, win).unwrap_or_default(),
    })
}

fn title(conn: &impl Connection, win: Window) -> Option<String> {
    let utf8 = atom(conn, "UTF8_STRING")?;
    let name = atom(conn, "_NET_WM_NAME")?;
    let reply = conn
        .get_property(false, win, name, utf8, 0, 256)
        .ok()?
        .reply()
        .ok()?;
    if !reply.value.is_empty() {
        return String::from_utf8(reply.value).ok();
    }
    let reply = conn
        .get_property(false, win, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 256)
        .ok()?
        .reply()
        .ok()?;
    Some(String::from_utf8_lossy(&reply.value).into_owned())
}
