//! KDE Plasma: a still of each screen from KWin's `ScreenShot2` interface, and
//! the windows' rectangles and the pointer from small KWin scripts.
//!
//! KWin only answers `ScreenShot2` to programs whose desktop entry lists the
//! interface under `X-KDE-DBUS-Restricted-Interfaces`, and it finds that entry
//! by comparing the canonical path of the entry's `Exec` program with the
//! caller's `/proc/<pid>/exe`. The deb, rpm and Arch packages install
//! `/usr/bin/windup` and ship `Exec=/usr/bin/windup` (rewritten by
//! `packaging/linux/package.sh`), so they match. The AppImage runs from a mount
//! like `/tmp/.mount_windupXXXX/usr/bin/windup`, which never does, so there
//! KWin refuses and `grab_all` falls through to the portal (which comes after
//! KWin in `still_order`).

use crate::capture::CaptureError;
use crate::capture::kwin_rects::{cursor_script, parse_cursor, parse_kwin_rects, script};
use crate::capture::pixels::{qimage_format, to_rgba};
use crate::linux::wayland::screencopy::output_layout_from_env;
use ashpd::zbus::zvariant::{Fd, OwnedValue, Value};
use ashpd::zbus::{self, Connection};
use std::collections::HashMap;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{Sender, channel};
use std::time::Duration;
use windup_core::capture::geometry::{DesktopShot, MonitorShot};
use windup_core::capture::pick::WindowRect;

const KWIN: &str = "org.kde.KWin";
const SHOT_PATH: &str = "/org/kde/KWin/ScreenShot2";
const SHOT_IFACE: &str = "org.kde.KWin.ScreenShot2";

/// How long to wait for a script to report. A script that never calls back
/// (an old KWin, a script error) must not hold the overlay up.
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(1);
/// How long any one D-Bus call may take; a compositor that never answers must
/// not hang the capture.
const METHOD_TIMEOUT: Duration = Duration::from_secs(5);
/// How long to wait for a screen's pixels after KWin has replied.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// The names the scripts are loaded under, so they can be unloaded again.
const RECTS_PLUGIN: &str = "io.github.spillebulle.windup.rects";
const CURSOR_PLUGIN: &str = "io.github.spillebulle.windup.cursor";
/// Where a script calls back with what it found.
const REPLY_PATH: &str = "/io/github/spillebulle/windup/Reply";
const REPLY_IFACE: &str = "io.github.spillebulle.windup.Reply";
const REPLY_METHOD: &str = "Report";

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

/// A session-bus connection whose calls give up.
pub(crate) async fn session() -> zbus::Result<Connection> {
    zbus::connection::Builder::session()?
        .method_timeout(METHOD_TIMEOUT)
        .build()
        .await
}

/// Every screen, by `CaptureScreen`, placed by the compositor's layout.
pub fn grab() -> Result<DesktopShot, CaptureError> {
    let layout = output_layout_from_env()?;
    let conn = pollster::block_on(session()).map_err(|_| CaptureError::Unsupported)?;
    let mut monitors = Vec::new();
    for (name, rect) in layout {
        let image = pollster::block_on(capture_screen(&conn, &name))?;
        monitors.push(MonitorShot {
            layout: rect,
            image,
            name,
            scale: 1.0,
            refresh_hz: None,
        });
    }
    Ok(DesktopShot { monitors })
}

/// One screen, by name, at its native resolution and without the cursor.
/// `Denied` when KWin will not answer us (the AppImage).
pub(crate) async fn capture_screen(
    conn: &Connection,
    name: &str,
) -> Result<image::RgbaImage, CaptureError> {
    // Close-on-exec, so a child started meanwhile does not keep the write end
    // open and the reader waiting.
    let (read, write) =
        rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).map_err(failed)?;
    // KWin writes the pixels as it replies, and a pipe holds 64 KiB of a
    // picture of megabytes: somebody must already be reading.
    let (tx, pixels) = channel();
    std::thread::Builder::new()
        .name("kwin-shot".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            let done = std::fs::File::from(read)
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            let _ = tx.send(done);
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
    let Some(format) = qimage_format(format) else {
        log::debug!("KWin sent QImage format {format}");
        return Err(failed(format!("unsupported QImage format {format}")));
    };
    // Not a join: a KWin that never closes the pipe must not hang us (the
    // reader then ends whenever it does).
    let bytes = pixels
        .recv_timeout(READ_TIMEOUT)
        .map_err(|_| failed("KWin did not finish sending the picture"))?
        .map_err(failed)?;
    to_rgba(format, width, height, stride as usize, &bytes)
}

/// What a script's callback hands to the waiting thread.
struct Reply(Mutex<Sender<String>>);

#[zbus::interface(name = "io.github.spillebulle.windup.Reply")]
impl Reply {
    /// Called by the KWin script with what it found, as JSON.
    fn report(&self, json: String) {
        if let Ok(tx) = self.0.lock() {
            let _ = tx.send(json);
        }
    }
}

/// The desktop's windows, topmost first; empty if KWin's script does not
/// answer in time (the picker then falls back to edge detection).
pub fn window_rects() -> Vec<WindowRect> {
    match run_script(RECTS_PLUGIN, script) {
        Ok(json) => parse_kwin_rects(&json),
        Err(e) => {
            log::debug!("kwin window rects: {e}");
            Vec::new()
        }
    }
}

/// Where the pointer is (logical units) and the name of the screen it is
/// on, as KWin's script says.
pub(crate) fn cursor() -> Result<(f32, f32, String), String> {
    let json = run_script(CURSOR_PLUGIN, cursor_script)?;
    parse_cursor(&json).ok_or_else(|| format!("KWin's cursor script said {json}"))
}

/// A script file only we can have written, under the 0700 runtime directory.
/// `create_new` refuses a file or symlink somebody planted there, since KWin
/// runs whatever it finds in it.
fn write_script(text: &str) -> Result<PathBuf, String> {
    use std::io::Write;
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .ok_or("no XDG_RUNTIME_DIR")?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let path = PathBuf::from(dir).join(format!("windup-script-{}-{nanos}.js", std::process::id()));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .map_err(|e| e.to_string())?;
    Ok(path)
}

/// Unloads the script and deletes its file however the wait ends. KWin reads
/// the file when the script runs, on another thread, so it must outlive `run`.
struct Loaded {
    conn: Connection,
    file: PathBuf,
    plugin: &'static str,
}

impl Drop for Loaded {
    fn drop(&mut self) {
        let _ = pollster::block_on(unload(&self.conn, self.plugin));
        let _ = std::fs::remove_file(&self.file);
    }
}

/// Loads the script `make` writes (given our bus name, then the path,
/// interface and method it calls back on) under `plugin`, runs it, and
/// returns the one JSON string it reports, within `SCRIPT_TIMEOUT`. The
/// script is unloaded and its file deleted however it ends.
fn run_script(
    plugin: &'static str,
    make: fn(&str, &str, &str, &str) -> String,
) -> Result<String, String> {
    let (tx, rx) = channel();
    let conn = pollster::block_on(session()).map_err(|e| e.to_string())?;
    pollster::block_on(async {
        conn.object_server()
            .at(REPLY_PATH, Reply(Mutex::new(tx)))
            .await
            .map_err(|e| e.to_string())
    })?;
    let me = conn.unique_name().ok_or("no bus name")?.to_string();
    let file = write_script(&make(&me, REPLY_PATH, REPLY_IFACE, REPLY_METHOD))?;
    // From here on every exit unloads and cleans up.
    let guard = Loaded { conn, file, plugin };
    let id = pollster::block_on(async {
        // A script of that name left by a crashed run would refuse the load.
        let _ = unload(&guard.conn, plugin).await;
        let id: i32 = guard
            .conn
            .call_method(
                Some(KWIN),
                "/Scripting",
                Some("org.kde.kwin.Scripting"),
                "loadScript",
                &(guard.file.to_string_lossy().as_ref(), plugin),
            )
            .await
            .map_err(|e| e.to_string())?
            .body()
            .deserialize()
            .map_err(|e| e.to_string())?;
        if id < 0 {
            return Err("KWin would not load the script".to_owned());
        }
        // KWin 6 puts script N at /Scripting/ScriptN.
        guard
            .conn
            .call_method(
                Some(KWIN),
                format!("/Scripting/Script{id}").as_str(),
                Some("org.kde.kwin.Script"),
                "run",
                &(),
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok(id)
    })?;
    // Not inside the async block: the call arrives on the connection's own
    // executor thread, and this thread only has to wait for it.
    rx.recv_timeout(SCRIPT_TIMEOUT)
        .map_err(|e| format!("script {id} did not report: {e}"))
}

async fn unload(conn: &Connection, plugin: &str) -> zbus::Result<()> {
    conn.call_method(
        Some(KWIN),
        "/Scripting",
        Some("org.kde.kwin.Scripting"),
        "unloadScript",
        &(plugin,),
    )
    .await
    .map(|_| ())
}
