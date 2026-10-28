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

/// Windows from Sway's `GET_TREE` reply, topmost first. Checked against
/// sway/ipc-json.c: workspace nodes carry no `visible`, so a workspace is
/// showing when its output's `current_workspace` names it (or, without that
/// key, when it is first in the output's `focus`); the scratchpad's `__i3`
/// output never counts. On each workspace floating windows come before tiled
/// ones, and within each group the `focus` arrays (child ids, most recently
/// focused first) order them, the closest Sway gives to a stacking order.
///
/// A window is a container with a `pid` (only views have one); the title is
/// `name`. Its `rect` excludes the title bar: `ipc_json_describe_node` moves
/// `y` down and shrinks `height` by `deco_rect.height * count`, where `count`
/// is the number of siblings when the parent's layout is `stacked` and 1
/// otherwise. That is undone here, so the rectangle is the whole window,
/// borders and title bar included.
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
    if node.get("type").and_then(Value::as_str) != Some("output") {
        for k in children(node, "nodes") {
            find_workspaces(k, out);
        }
        return;
    }
    if node.get("name").and_then(Value::as_str) == Some("__i3") {
        return;
    }
    let current = node.get("current_workspace").and_then(Value::as_str);
    let first_focus = node
        .get("focus")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_i64);
    for ws in children(node, "nodes") {
        if ws.get("type").and_then(Value::as_str) != Some("workspace") {
            continue;
        }
        let showing = match current {
            Some(name) => ws.get("name").and_then(Value::as_str) == Some(name),
            None => first_focus.is_some() && ws.get("id").and_then(Value::as_i64) == first_focus,
        };
        if showing {
            let stacked = ws.get("layout").and_then(Value::as_str) == Some("stacked");
            let floating = children(ws, "floating_nodes");
            let count = if stacked { floating.len() } else { 1 };
            for k in floating {
                sway_windows(k, count, out);
            }
            let tiled = children(ws, "nodes");
            let count = if stacked { tiled.len() } else { 1 };
            for k in tiled {
                sway_windows(k, count, out);
            }
        }
    }
}

/// `count` is how many title bars the compositor took off this container's
/// `rect` (see `sway_rects`).
fn sway_windows(node: &Value, count: usize, out: &mut Vec<WindowRect>) {
    let kids = children(node, "nodes");
    let floating = children(node, "floating_nodes");
    if !kids.is_empty() || !floating.is_empty() {
        let stacked = node.get("layout").and_then(Value::as_str) == Some("stacked");
        let n = if stacked { kids.len() } else { 1 };
        for k in kids {
            sway_windows(k, n, out);
        }
        for k in floating {
            sway_windows(k, 1, out);
        }
        return;
    }
    // Only views have a pid. `visible` is false for a hidden tab or stacked
    // sibling.
    if node.get("pid").is_none() || node.get("visible").and_then(Value::as_bool) == Some(false) {
        return;
    }
    let r = node.get("rect");
    let field = |k: &str| r.and_then(|r| r.get(k)).and_then(num);
    let deco = node
        .get("deco_rect")
        .and_then(|d| d.get("height"))
        .and_then(num)
        .unwrap_or(0.0)
        * count as f32;
    let (y, h) = (
        field("y").map(|y| y - deco),
        field("height").map(|h| h + deco),
    );
    if let Some(rect) = rect(field("x"), y, field("width"), h) {
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
/// group `focusHistoryID` (0 is the most recent) orders them. Checked against
/// hyprland's src/ipc/s1/Commands.cpp: `at` and `size` are the window's
/// geometric goal position and size in the global layout (monitors are placed
/// by `x`/`y` there), not pixels of a monitor.
pub(crate) fn hyprland_rects(clients_json: &str, monitors_json: &str) -> Vec<WindowRect> {
    let (Ok(Value::Array(clients)), Ok(Value::Array(monitors))) = (
        serde_json::from_str::<Value>(clients_json),
        serde_json::from_str::<Value>(monitors_json),
    ) else {
        return Vec::new();
    };
    // Workspaces are matched by name: `id` is only written for numbered
    // workspaces, while both commands write `name` (HyprCtl's displayName).
    // A monitor with no special workspace open reports an empty name.
    let mut shown: Vec<&str> = Vec::new();
    for m in &monitors {
        for key in ["activeWorkspace", "specialWorkspace"] {
            if let Some(name) = m
                .get(key)
                .and_then(|w| w.get("name"))
                .and_then(Value::as_str)
                && !name.is_empty()
            {
                shown.push(name);
            }
        }
    }
    let mut found: Vec<((bool, bool, i64), WindowRect)> = Vec::new();
    for c in &clients {
        let flag = |k: &str| c.get(k).and_then(Value::as_bool).unwrap_or(false);
        let ws = c
            .get("workspace")
            .and_then(|w| w.get("name"))
            .and_then(Value::as_str);
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
    // Hyprland's own hyprctl uses `$XDG_RUNTIME_DIR/hypr/<sig>/.socket.sock`
    // and sends `j/clients` for JSON. Older releases kept sockets in /tmp.
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(run) = std::env::var_os("XDG_RUNTIME_DIR") {
        dirs.push(PathBuf::from(run).join("hypr").join(&sig));
    }
    dirs.push(PathBuf::from("/tmp/hypr").join(&sig));
    let mut last = Error::new(ErrorKind::NotFound, "no Hyprland socket");
    for dir in dirs {
        // One attempt per directory: a socket that connects but fails the
        // request falls through to the next directory too.
        let attempt = || -> std::io::Result<String> {
            let mut s = UnixStream::connect(dir.join(".socket.sock"))?;
            s.set_read_timeout(Some(READ_TIMEOUT))?;
            s.set_write_timeout(Some(READ_TIMEOUT))?;
            s.write_all(command.as_bytes())?;
            let mut reply = String::new();
            s.read_to_string(&mut reply)?;
            Ok(reply)
        };
        match attempt() {
            Ok(reply) => return Ok(reply),
            Err(e) => last = e,
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures are hand-made, not recordings, but every key is one the source
    // writes (trimmed of the rest): Sway's from sway/ipc-json.c,
    // https://raw.githubusercontent.com/swaywm/sway/master/sway/ipc-json.c
    // (workspaces carry no `visible`; outputs carry `current_workspace`;
    // `rect` excludes the title bar), and Hyprland's from src/ipc/s1/Commands.cpp,
    // https://raw.githubusercontent.com/hyprwm/Hyprland/main/src/ipc/s1/Commands.cpp.
    // Values are invented. Replace with recordings when a session is at hand.
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
        let mut monitors: serde_json::Value = serde_json::from_str(MONITORS).unwrap();
        monitors[0]["specialWorkspace"]["name"] = "special:s".into();
        let mut clients: serde_json::Value = serde_json::from_str(CLIENTS).unwrap();
        for c in clients.as_array_mut().unwrap() {
            if c["title"] == "Hidden editor" {
                c["workspace"]["name"] = "special:s".into();
            }
        }
        let (clients, monitors) = (clients.to_string(), monitors.to_string());
        assert!(titles(&hyprland_rects(&clients, &monitors)).contains(&"Hidden editor"));
        assert!(!titles(&hyprland_rects(&clients, MONITORS)).contains(&"Hidden editor"));
    }

    #[test]
    fn sway_title_bar_is_added_back() {
        let r = sway_rects(SWAY);
        let rect = |t: &str| {
            let d = r.iter().find(|w| w.title == t).unwrap().rect;
            (d.x, d.y, d.w, d.h)
        };
        // Reported y=20 h=1060 under a 20 px bar; the window is 0..1080.
        assert_eq!(rect("~/code"), (0.0, 0.0, 960.0, 1080.0));
        assert_eq!(rect("Calculator"), (700.0, 300.0, 400.0, 500.0));
        // No bar (deco_rect empty): unchanged.
        assert_eq!(rect("Mozilla Firefox"), (960.0, 0.0, 960.0, 1080.0));
    }

    #[test]
    fn sway_stacked_siblings_add_back_every_bar() {
        let tree = r#"{"type":"root","nodes":[{"type":"output","name":"X","current_workspace":"1",
          "nodes":[{"type":"workspace","name":"1","id":2,"layout":"stacked","focus":[3,4],
            "nodes":[
              {"id":3,"type":"con","name":"a","pid":1,"visible":true,
               "rect":{"x":0,"y":40,"width":100,"height":160},
               "deco_rect":{"x":0,"y":0,"width":100,"height":20}},
              {"id":4,"type":"con","name":"b","pid":2,"visible":false,
               "rect":{"x":0,"y":40,"width":100,"height":160},
               "deco_rect":{"x":0,"y":20,"width":100,"height":20}}],
            "floating_nodes":[]}]}]}"#;
        let r = sway_rects(tree);
        assert_eq!(titles(&r), ["a"]);
        assert_eq!((r[0].rect.y, r[0].rect.h), (0.0, 200.0));
    }

    #[test]
    fn malformed_json_is_empty() {
        assert!(sway_rects("not json").is_empty());
        assert!(sway_rects("{}").is_empty());
        assert!(hyprland_rects("[", MONITORS).is_empty());
        assert!(hyprland_rects(CLIENTS, "oops").is_empty());
    }
}
