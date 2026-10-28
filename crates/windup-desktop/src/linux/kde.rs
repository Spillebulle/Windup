//! KDE Plasma: a still of each screen from KWin's `ScreenShot2` interface, and
//! the windows' rectangles from a small KWin script.
//!
//! KWin only answers `ScreenShot2` to programs whose desktop entry lists the
//! interface under `X-KDE-DBUS-Restricted-Interfaces`, and it finds that entry
//! from the caller's executable path. A deb/rpm/Arch install (`/usr/bin/windup`
//! beside `/usr/share/applications/...desktop`) matches; the AppImage runs
//! from a mount like `/tmp/.mount_windupXXXX/usr/bin/windup`, which does not,
//! so there KWin refuses and `grab_all` falls through to the portal (which
//! comes after KWin in `still_order`).

use crate::capture::CaptureError;
use crate::capture::kwin_rects::{parse_kwin_rects, script};
use crate::capture::pixels::{qimage_format, to_rgba};
use crate::linux::wayland::screencopy::output_layout_from_env;
use ashpd::zbus::zvariant::{Fd, OwnedValue, Value};
use ashpd::zbus::{self, Connection};
use std::collections::HashMap;
use std::io::Read;
use std::sync::Mutex;
use std::sync::mpsc::{Sender, channel};
use std::time::Duration;
use windup_core::capture::geometry::{DesktopShot, MonitorShot};
use windup_core::capture::pick::WindowRect;

const KWIN: &str = "org.kde.KWin";
const SHOT_PATH: &str = "/org/kde/KWin/ScreenShot2";
const SHOT_IFACE: &str = "org.kde.KWin.ScreenShot2";

/// How long to wait for the script to report. A script that never calls back
/// (an old KWin, a script error) must not hold the overlay up.
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(1);
/// The name the script is loaded under, so it can be unloaded again.
const PLUGIN_NAME: &str = "io.github.spillebulle.windup.rects";
const RECTS_PATH: &str = "/io/github/spillebulle/windup/Rects";
const RECTS_IFACE: &str = "io.github.spillebulle.windup.Rects";
const RECTS_METHOD: &str = "Windows";

fn failed(what: impl std::fmt::Display) -> CaptureError {
    CaptureError::Failed(what.to_string())
}

/// KWin says no with its own error or the bus's `AccessDenied`.
fn is_denied(e: &zbus::Error) -> bool {
    match e {
        zbus::Error::MethodError(name, _, _) => {
            let n = name.as_str();
            n.ends_with("AccessDenied") || n.contains("NoAuthorized") || n.contains("NotAuthorized")
        }
        _ => false,
    }
}

/// Every screen, by `CaptureScreen`, placed by the compositor's layout.
pub fn grab() -> Result<DesktopShot, CaptureError> {
    let layout = output_layout_from_env()?;
    let mut monitors = Vec::new();
    for (name, rect) in layout {
        let image = pollster::block_on(capture_screen(&name))?;
        monitors.push(MonitorShot {
            layout: rect,
            image,
            name,
        });
    }
    Ok(DesktopShot { monitors })
}

async fn capture_screen(name: &str) -> Result<image::RgbaImage, CaptureError> {
    let conn = Connection::session()
        .await
        .map_err(|_| CaptureError::Unsupported)?;
    let (read, write) = rustix::pipe::pipe().map_err(failed)?;
    // KWin writes the pixels as it replies, and a pipe holds 64 KiB of a
    // picture of megabytes: somebody must already be reading.
    let reader = std::thread::Builder::new()
        .name("kwin-shot".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            std::fs::File::from(read)
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        })
        .map_err(failed)?;
    let mut options: HashMap<&str, Value<'_>> = HashMap::new();
    options.insert("include-cursor", Value::from(false));
    options.insert("native-resolution", Value::from(true));
    let reply = conn
        .call_method(
            Some(KWIN),
            SHOT_PATH,
            Some(SHOT_IFACE),
            "CaptureScreen",
            &(name, options, Fd::from(&write)),
        )
        .await;
    // Our end must close for the reader to see the end of the data.
    drop(write);
    let reply = match reply {
        Ok(r) => r,
        Err(e) if is_denied(&e) => return Err(CaptureError::Denied),
        // No KWin on the bus (or no such interface): not this back end.
        Err(zbus::Error::MethodError(n, _, _))
            if n.as_str().ends_with("ServiceUnknown") || n.as_str().ends_with("UnknownObject") =>
        {
            return Err(CaptureError::Unsupported);
        }
        Err(e) => return Err(failed(e)),
    };
    let results: HashMap<String, OwnedValue> = reply.body().deserialize().map_err(failed)?;
    let num = |k: &str| -> Result<u32, CaptureError> {
        results
            .get(k)
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| failed(format!("KWin's reply has no {k}")))
    };
    let (width, height, stride, format) = (
        num("width")?,
        num("height")?,
        num("stride")?,
        num("format")?,
    );
    let format = qimage_format(format)
        .ok_or_else(|| failed(format!("unsupported QImage format {format}")))?;
    let bytes = reader
        .join()
        .map_err(|_| failed("the pipe reader panicked"))?
        .map_err(failed)?;
    to_rgba(format, width, height, stride as usize, &bytes)
}

/// What the script's callback hands to the main thread.
struct Rects(Mutex<Sender<String>>);

#[zbus::interface(name = "io.github.spillebulle.windup.Rects")]
impl Rects {
    /// Called by the KWin script with the windows as JSON.
    fn windows(&self, json: String) {
        if let Ok(tx) = self.0.lock() {
            let _ = tx.send(json);
        }
    }
}

/// The desktop's windows, topmost first; empty if KWin's script does not
/// answer in time (the picker then falls back to edge detection).
pub fn window_rects() -> Vec<WindowRect> {
    match try_window_rects() {
        Ok(json) => parse_kwin_rects(&json),
        Err(e) => {
            log::debug!("kwin window rects: {e}");
            Vec::new()
        }
    }
}

fn try_window_rects() -> Result<String, String> {
    let (tx, rx) = channel();
    let (conn, script_id) = pollster::block_on(async {
        let conn = Connection::session().await.map_err(|e| e.to_string())?;
        conn.object_server()
            .at(RECTS_PATH, Rects(Mutex::new(tx)))
            .await
            .map_err(|e| e.to_string())?;
        let me = conn.unique_name().ok_or("no bus name")?.to_string();
        let file = std::env::temp_dir().join(format!("windup-rects-{}.js", std::process::id()));
        std::fs::write(&file, script(&me, RECTS_PATH, RECTS_IFACE, RECTS_METHOD))
            .map_err(|e| e.to_string())?;
        // A script of that name left by a crashed run would refuse the load.
        let _ = unload(&conn).await;
        let loaded = conn
            .call_method(
                Some(KWIN),
                "/Scripting",
                Some("org.kde.kwin.Scripting"),
                "loadScript",
                &(file.to_string_lossy().as_ref(), PLUGIN_NAME),
            )
            .await;
        let _ = std::fs::remove_file(&file);
        let id: i32 = loaded
            .map_err(|e| e.to_string())?
            .body()
            .deserialize()
            .map_err(|e| e.to_string())?;
        if id < 0 {
            return Err("KWin would not load the script".to_owned());
        }
        // KWin 6 puts script N at /Scripting/ScriptN.
        conn.call_method(
            Some(KWIN),
            format!("/Scripting/Script{id}").as_str(),
            Some("org.kde.kwin.Script"),
            "run",
            &(),
        )
        .await
        .map_err(|e| e.to_string())?;
        Ok::<_, String>((conn, id))
    })?;
    // Not inside the async block: the call arrives on the connection's own
    // executor thread, and this thread only has to wait for it.
    let got = rx.recv_timeout(SCRIPT_TIMEOUT);
    let _ = pollster::block_on(unload(&conn));
    got.map_err(|e| format!("script {script_id} did not report: {e}"))
}

async fn unload(conn: &Connection) -> zbus::Result<()> {
    conn.call_method(
        Some(KWIN),
        "/Scripting",
        Some("org.kde.kwin.Scripting"),
        "unloadScript",
        &(PLUGIN_NAME,),
    )
    .await
    .map(|_| ())
}
