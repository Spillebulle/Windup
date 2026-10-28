//! The KWin script that reports window rectangles, and the reading of what it
//! says. The D-Bus plumbing is in `linux/kde.rs`; this half is pure so it is
//! tested everywhere.

use serde_json::Value;
use windup_core::capture::geometry::DRect;
use windup_core::capture::pick::WindowRect;

/// The script KWin runs. `workspace.stackingOrder` is KWin 6's list, bottom
/// to top; geometry is in the compositor's logical coordinates, which is the
/// layout the screenshots use. Placeholders are filled by `script`.
const SCRIPT: &str = r#"
const out = [];
for (const w of workspace.stackingOrder) {
    if (!w.normalWindow || w.minimized || w.desktopWindow || w.dock) continue;
    const g = w.frameGeometry;
    out.push({ x: g.x, y: g.y, w: g.width, h: g.height, title: String(w.caption) });
}
callDBus("@SERVICE@", "@PATH@", "@IFACE@", "@METHOD@", JSON.stringify(out));
"#;

/// The script text, calling `method` on `service` at `path` of `iface` with
/// one JSON string.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn script(service: &str, path: &str, iface: &str, method: &str) -> String {
    SCRIPT
        .replace("@SERVICE@", service)
        .replace("@PATH@", path)
        .replace("@IFACE@", iface)
        .replace("@METHOD@", method)
}

/// Windows from the script's JSON (bottom to top, as KWin stacks them),
/// returned topmost first. Malformed input, and rectangles with no area or
/// non-finite numbers, are dropped rather than failing the lot.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_kwin_rects(json: &str) -> Vec<WindowRect> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    let mut out: Vec<WindowRect> = items
        .iter()
        .filter_map(|v| {
            let n = |k: &str| v.get(k)?.as_f64().map(|n| n as f32);
            let rect = DRect {
                x: n("x")?,
                y: n("y")?,
                w: n("w")?,
                h: n("h")?,
            };
            let sane = [rect.x, rect.y, rect.w, rect.h]
                .iter()
                .all(|v| v.is_finite());
            (sane && rect.w > 0.0 && rect.h > 0.0).then(|| WindowRect {
                rect,
                title: v
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            })
        })
        .collect();
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_topmost_window_comes_first() {
        let json = r#"[
            {"x":0,"y":0,"w":1920,"h":1080,"title":"bottom"},
            {"x":100,"y":50.5,"w":800,"h":600,"title":"top"}
        ]"#;
        let rects = parse_kwin_rects(json);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].title, "top");
        assert_eq!(
            rects[0].rect,
            DRect {
                x: 100.0,
                y: 50.5,
                w: 800.0,
                h: 600.0
            }
        );
        assert_eq!(rects[1].title, "bottom");
    }

    #[test]
    fn negative_origins_and_a_missing_title_are_fine() {
        let rects = parse_kwin_rects(r#"[{"x":-1920,"y":-10,"w":10,"h":20}]"#);
        assert_eq!(rects[0].rect.x, -1920.0);
        assert_eq!(rects[0].title, "");
    }

    #[test]
    fn bad_entries_are_skipped_and_bad_json_is_empty() {
        let json = r#"[{"x":0,"y":0,"w":0,"h":10},{"x":"a","y":0,"w":1,"h":1},
                       5, {"x":1,"y":2,"w":3,"h":4,"title":"ok"}]"#;
        let rects = parse_kwin_rects(json);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].title, "ok");
        assert!(parse_kwin_rects("not json").is_empty());
        assert!(parse_kwin_rects("{}").is_empty());
        assert!(parse_kwin_rects("").is_empty());
    }

    #[test]
    fn the_script_names_its_target_and_leaves_no_placeholder() {
        let s = script(":1.42", "/p", "org.x.I", "Windows");
        assert!(s.contains(r#"callDBus(":1.42", "/p", "org.x.I", "Windows""#));
        assert!(!s.contains('@'));
    }
}
