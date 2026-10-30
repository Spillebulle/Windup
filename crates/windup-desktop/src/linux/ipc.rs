//! The Unix socket behind `windup --popup` and single-instance launching.
//! The protocol is `windup_core::ipc`; this is the plumbing.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Command as Process, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use windup_core::ipc::{self, Command, Request};

pub fn socket_path() -> PathBuf {
    let var = |k| std::env::var(k).ok();
    let display = var("WAYLAND_DISPLAY")
        .filter(|v| !v.is_empty())
        .or_else(|| var("DISPLAY"));
    ipc::socket_path(
        var("XDG_RUNTIME_DIR").as_deref(),
        rustix::process::getuid().as_raw(),
        display.as_deref(),
    )
}

/// Delivers `command` to the running Windup. Fails when none is listening.
pub fn send(command: Command) -> std::io::Result<()> {
    send_request(&Request::Do(command))
}

/// Delivers `request` to the running Windup: a command, or the Steps
/// review's `EditStep`. Fails when none is listening, or for a path that
/// cannot go on the socket's one line.
pub fn send_request(request: &Request) -> std::io::Result<()> {
    let line = request.encode().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the path cannot be sent on the socket",
        )
    })?;
    let mut stream = UnixStream::connect(socket_path())?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    stream.write_all(line.as_bytes())
}

/// Delivers `command`, first starting Windup in the background if it is not
/// running, the way a desktop shortcut expects `windup --popup` to behave.
pub fn send_or_start(command: Command) -> std::io::Result<()> {
    if send(command).is_ok() {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so the shortcut daemon or terminal that ran
        // us does not take it down when it cleans up.
        Process::new(exe)
            .arg("--background")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match send(command) {
            Ok(()) => return Ok(()),
            Err(e) if Instant::now() > deadline => return Err(e),
            Err(_) => thread::sleep(Duration::from_millis(40)),
        }
    }
}

/// The new executable, when a package update has replaced the one this
/// process runs from and put a new one at the same path.
pub fn upgraded_exe() -> Option<PathBuf> {
    let link = std::fs::read_link("/proc/self/exe").ok()?;
    let exe = ipc::replaced_exe(link.to_str()?)?;
    exe.is_file().then_some(exe)
}

/// Hands the socket to the new Windup and exits: without this, every
/// `windup --popup` after an update reaches the old process, which is still
/// running and may not know the new commands.
pub fn hand_over(exe: &std::path::Path) -> ! {
    log::info!("{} was updated; starting the new one", exe.display());
    let _ = std::fs::remove_file(socket_path());
    {
        use std::os::unix::process::CommandExt;
        let _ = Process::new(exe)
            .arg("--background")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn();
    }
    std::process::exit(0)
}

pub enum Bound {
    /// We own the socket and are the running Windup.
    Listening(UnixListener),
    /// Another Windup already answers on it.
    AlreadyRunning,
}

/// Takes the socket, or reports that another Windup holds it. A socket file
/// with nobody behind it is left over from a crash and is replaced.
pub fn bind() -> std::io::Result<Bound> {
    let path = socket_path();
    if UnixStream::connect(&path).is_ok() {
        return Ok(Bound::AlreadyRunning);
    }
    if let Some(dir) = path.parent()
        && !dir.exists()
    {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let _ = std::fs::remove_file(&path);
    match UnixListener::bind(&path) {
        Ok(listener) => {
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            Ok(Bound::Listening(listener))
        }
        // Lost a race with a Windup starting at the same moment.
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => Ok(Bound::AlreadyRunning),
        Err(e) => Err(e),
    }
}

/// Accepts connections for ever, one request per connection.
pub fn serve(listener: UnixListener, mut on_request: impl FnMut(Request) + Send + 'static) {
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).is_ok() {
                match Request::parse(&line) {
                    Some(request) => on_request(request),
                    // `bind` in another Windup checking whether we are alive.
                    None if line.is_empty() => {}
                    None => log::warn!("ignoring unknown command {line:?} on the socket"),
                }
            }
        }
    });
}
