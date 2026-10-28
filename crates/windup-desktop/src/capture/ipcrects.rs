//! Window rectangles from the Sway and Hyprland IPC sockets. The parsers are
//! pure so they are tested everywhere; the socket I/O is a thin Linux layer.

// Only Linux asks the sockets; the parsers are built everywhere for the tests.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use serde_json::Value;
use windup_core::capture::geometry::DRect;
use windup_core::capture::pick::WindowRect;

fn num(v: &Value) -> Option<f32> {
    v.as_f64().map(|n| n as f32).filter(|n| n.is_finite())
}

fn rect(x: Option<f32>, y: Option<f32>, w: Option<f32>, h: Option<f32>) -> Option<DRect> {
    let r = DRect {
        x: x?,
        y: y?,
        w: w?,
        h: h?,
    };
    (r.w > 0.0 && r.h > 0.0).then_some(r)
}

/// Windows from Sway's `GET_TREE` reply, topmost first. Only windows on a
/// workspace that is showing count (the scratchpad's `__i3` output never is).
/// On each workspace floating windows come before tiled ones, and within each
/// group the `focus` arrays (child ids, most recently focused first) order
/// them, which is the closest Sway gives to a stacking order.
///
/// The rectangle is the container's `rect`, which sway-ipc(7) documents as
/// absolute and including the border and title bar, so `deco_rect` (relative
/// to the container, inside it) adds nothing. A window is a leaf with a `pid`,
/// `app_id` or `window_properties`; the title is `name`.
pub(crate) fn sway_rects(json: &str) -> Vec<WindowRect> {
    let Ok(root) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    find_workspaces(&root, &mut out);
    out
}

/// A node's children under `key`, most recently focused first; any the
/// `focus` array does not list go last.
fn children<'a>(node: &'a Value, key: &str) -> Vec<&'a Value> {
    let mut kids: Vec<&Value> = node
        .get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let focus: Vec<i64> = node
        .get("focus")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    kids.sort_by_key(|k| {
        let id = k.get("id").and_then(Value::as_i64);
        focus
            .iter()
            .position(|f| Some(*f) == id)
            .unwrap_or(usize::MAX)
    });
    kids
}

fn find_workspaces(node: &Value, out: &mut Vec<WindowRect>) {
    if node.get("type").and_then(Value::as_str) == Some("workspace") {
        if node.get("visible").and_then(Value::as_bool) == Some(true) {
            for k in children(node, "floating_nodes") {
                sway_windows(k, out);
            }
            for k in children(node, "nodes") {
                sway_windows(k, out);
            }
        }
        return;
    }
    for k in children(node, "nodes") {
        find_workspaces(k, out);
    }
}

fn sway_windows(node: &Value, out: &mut Vec<WindowRect>) {
    let kids: Vec<&Value> = children(node, "floating_nodes")
        .into_iter()
        .chain(children(node, "nodes"))
        .collect();
    if !kids.is_empty() {
        for k in kids {
            sway_windows(k, out);
        }
        return;
    }
    let is_window = node.get("pid").is_some()
        || node.get("app_id").is_some_and(|a| !a.is_null())
        || node.get("window_properties").is_some();
    // Newer Sway marks a hidden tab or stacked sibling `visible: false`.
    if !is_window || node.get("visible").and_then(Value::as_bool) == Some(false) {
        return;
    }
    let r = node.get("rect");
    let field = |k: &str| r.and_then(|r| r.get(k)).and_then(num);
    if let Some(rect) = rect(field("x"), field("y"), field("width"), field("height")) {
        out.push(WindowRect {
            rect,
            title: node
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        });
    }
}

/// Windows from `hyprctl clients -j`, topmost first, keeping the mapped,
/// unhidden ones on a workspace `hyprctl monitors -j` says is showing (each
/// monitor's active workspace, and its special workspace when one is open).
/// Fullscreen windows come first, then floating, then tiled; within each
/// group `focusHistoryID` (0 is the most recent) orders them. `at` and
/// `size` are in the compositor's logical layout.
pub(crate) fn hyprland_rects(clients_json: &str, monitors_json: &str) -> Vec<WindowRect> {
    let (Ok(Value::Array(clients)), Ok(Value::Array(monitors))) = (
        serde_json::from_str::<Value>(clients_json),
        serde_json::from_str::<Value>(monitors_json),
    ) else {
        return Vec::new();
    };
    let mut shown = Vec::new();
    for m in &monitors {
        for key in ["activeWorkspace", "specialWorkspace"] {
            if let Some(id) = m.get(key).and_then(|w| w.get("id")).and_then(Value::as_i64)
                && id != 0
            {
                shown.push(id);
            }
        }
    }
    let mut found: Vec<((bool, bool, i64), WindowRect)> = Vec::new();
    for c in &clients {
        let flag = |k: &str| c.get(k).and_then(Value::as_bool).unwrap_or(false);
        let ws = c
            .get("workspace")
            .and_then(|w| w.get("id"))
            .and_then(Value::as_i64);
        if !flag("mapped") || flag("hidden") || !ws.is_some_and(|w| shown.contains(&w)) {
            continue;
        }
        let pair = |k: &str| {
            let a = c.get(k).and_then(Value::as_array);
            (
                a.and_then(|a| a.first()).and_then(num),
                a.and_then(|a| a.get(1)).and_then(num),
            )
        };
        let ((x, y), (w, h)) = (pair("at"), pair("size"));
        let Some(rect) = rect(x, y, w, h) else {
            continue;
        };
        // `fullscreen` is a mode number (0 none) now, a bool in old versions.
        let fullscreen = match c.get("fullscreen") {
            Some(Value::Bool(b)) => *b,
            Some(v) => v.as_i64().is_some_and(|n| n > 0),
            None => false,
        };
        let key = (
            !fullscreen,
            !flag("floating"),
            c.get("focusHistoryID")
                .and_then(Value::as_i64)
                .unwrap_or(i64::MAX),
        );
        let title = c.get("title").and_then(Value::as_str).unwrap_or_default();
        found.push((
            key,
            WindowRect {
                rect,
                title: title.to_owned(),
            },
        ));
    }
    found.sort_by_key(|(k, _)| *k);
    found.into_iter().map(|(_, w)| w).collect()
}

// ---- the sockets ----------------------------------------------------------

/// How long a compositor gets to answer before edges are used instead.
#[cfg(target_os = "linux")]
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

/// Sway's windows, asked over `$SWAYSOCK`. Empty on any failure.
#[cfg(target_os = "linux")]
pub(crate) fn sway_live() -> Vec<WindowRect> {
    match sway_get_tree() {
        Ok(json) => sway_rects(&json),
        Err(e) => {
            log::debug!("sway get_tree: {e}");
            Vec::new()
        }
    }
}

#[cfg(target_os = "linux")]
fn sway_get_tree() -> std::io::Result<String> {
    use std::io::{Error, ErrorKind, Read, Write};
    use std::os::unix::net::UnixStream;
    /// i3-ipc message type GET_TREE.
    const GET_TREE: u32 = 4;
    /// A tree larger than this is not believed.
    const MAX_REPLY: usize = 64 << 20;

    let path = std::env::var_os("SWAYSOCK")
        .ok_or_else(|| Error::new(ErrorKind::NotFound, "SWAYSOCK is not set"))?;
    let mut s = UnixStream::connect(path)?;
    s.set_read_timeout(Some(READ_TIMEOUT))?;
    s.set_write_timeout(Some(READ_TIMEOUT))?;
    let mut msg = Vec::with_capacity(14);
    msg.extend_from_slice(b"i3-ipc");
    msg.extend_from_slice(&0u32.to_le_bytes());
    msg.extend_from_slice(&GET_TREE.to_le_bytes());
    s.write_all(&msg)?;
    let mut head = [0u8; 14];
    s.read_exact(&mut head)?;
    if &head[..6] != b"i3-ipc" {
        return Err(Error::new(ErrorKind::InvalidData, "not an i3-ipc reply"));
    }
    let len = u32::from_le_bytes([head[6], head[7], head[8], head[9]]) as usize;
    if len > MAX_REPLY {
        return Err(Error::new(ErrorKind::InvalidData, "reply too large"));
    }
    let mut body = vec![0u8; len];
    s.read_exact(&mut body)?;
    String::from_utf8(body).map_err(|e| Error::new(ErrorKind::InvalidData, e))
}

/// Hyprland's windows, asked over its command socket. Empty on any failure.
#[cfg(target_os = "linux")]
pub(crate) fn hyprland_live() -> Vec<WindowRect> {
    let asked = hypr_request("j/clients").and_then(|c| Ok((c, hypr_request("j/monitors")?)));
    match asked {
        Ok((clients, monitors)) => hyprland_rects(&clients, &monitors),
        Err(e) => {
            log::debug!("hyprland clients: {e}");
            Vec::new()
        }
    }
}

#[cfg(target_os = "linux")]
fn hypr_request(command: &str) -> std::io::Result<String> {
    use std::io::{Error, ErrorKind, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;

    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .map_err(|_| Error::new(ErrorKind::NotFound, "no Hyprland instance signature"))?;
    // Newer Hyprland keeps its sockets under the runtime dir, older in /tmp.
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(run) = std::env::var_os("XDG_RUNTIME_DIR") {
        dirs.push(PathBuf::from(run).join("hypr").join(&sig));
    }
    dirs.push(PathBuf::from("/tmp/hypr").join(&sig));
    let mut last = Error::new(ErrorKind::NotFound, "no Hyprland socket");
    for dir in dirs {
        match UnixStream::connect(dir.join(".socket.sock")) {
            Ok(mut s) => {
                s.set_read_timeout(Some(READ_TIMEOUT))?;
                s.set_write_timeout(Some(READ_TIMEOUT))?;
                s.write_all(command.as_bytes())?;
                let mut reply = String::new();
                s.read_to_string(&mut reply)?;
                return Ok(reply);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures are hand-made, not recorded, but shaped field for field like
    // the real replies (trimmed to what matters): Sway's GET_TREE per
    // sway-ipc(7), https://man.archlinux.org/man/sway-ipc.7, and Hyprland's
    // `hyprctl clients -j` / `monitors -j` per
    // https://wiki.hypr.land/Configuring/Using-hyprctl/ and the JSON written
    // by Hyprland's HyprCtl.cpp. Replace with recordings when a session is at
    // hand.
    const SWAY: &str = include_str!("../../tests/fixtures/sway_tree.json");
    const CLIENTS: &str = include_str!("../../tests/fixtures/hypr_clients.json");
    const MONITORS: &str = include_str!("../../tests/fixtures/hypr_monitors.json");

    fn titles(v: &[WindowRect]) -> Vec<&str> {
        v.iter().map(|w| w.title.as_str()).collect()
    }

    #[test]
    fn sway_floating_first_then_tiled_in_focus_order() {
        let r = sway_rects(SWAY);
        assert_eq!(titles(&r), ["Calculator", "Mozilla Firefox", "~/code"]);
        let c = r[0].rect;
        assert_eq!((c.x, c.y, c.w, c.h), (700.0, 300.0, 400.0, 500.0));
    }

    #[test]
    fn sway_hidden_workspace_and_scratchpad_are_absent() {
        let r = sway_rects(SWAY);
        assert!(!titles(&r).contains(&"Hidden editor"));
        assert!(!titles(&r).contains(&"scratch terminal"));
    }

    #[test]
    fn hyprland_fullscreen_then_floating_then_tiled_by_focus() {
        let r = hyprland_rects(CLIENTS, MONITORS);
        assert_eq!(
            titles(&r),
            ["movie.mkv", "Calculator", "Mozilla Firefox", "~/code"]
        );
        let c = r[1].rect;
        assert_eq!((c.x, c.y, c.w, c.h), (700.0, 300.0, 400.0, 500.0));
    }

    #[test]
    fn hyprland_hidden_workspace_and_unmapped_are_absent() {
        let r = hyprland_rects(CLIENTS, MONITORS);
        assert!(!titles(&r).contains(&"Hidden editor"));
        assert_eq!(r.len(), 4);
    }

    #[test]
    fn hyprland_special_workspace_counts_when_shown() {
        let monitors = MONITORS.replace(
            r#""specialWorkspace": {"id": 0, "name": ""}"#,
            r#""specialWorkspace": {"id": -98, "name": "special:s"}"#,
        );
        let clients = CLIENTS.replace(
            r#""id": 2, "name": "2""#,
            r#""id": -98, "name": "special:s""#,
        );
        assert!(titles(&hyprland_rects(&clients, &monitors)).contains(&"Hidden editor"));
        assert!(!titles(&hyprland_rects(&clients, MONITORS)).contains(&"Hidden editor"));
    }

    #[test]
    fn malformed_json_is_empty() {
        assert!(sway_rects("not json").is_empty());
        assert!(sway_rects("{}").is_empty());
        assert!(hyprland_rects("[", MONITORS).is_empty());
        assert!(hyprland_rects(CLIENTS, "oops").is_empty());
    }
}
