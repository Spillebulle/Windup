//! The pipes we write frames and sound into, which ffmpeg opens by path.
//!
//! A named pipe on Windows and a FIFO on Linux. Either way the writer waits
//! until ffmpeg opens its end, and a pipe ffmpeg never opens (it died first)
//! must not hold the writer for ever: on Linux the wait polls the stop flag,
//! on Windows `unblock` opens the other end ourselves to let it go.

use std::fs::File;
use std::io;
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Numbers the pipes of this process, so no two share a name.
static NEXT: AtomicU32 = AtomicU32::new(0);

/// Our end of a pipe, before ffmpeg has opened the other.
pub(crate) struct PipeServer {
    /// What ffmpeg is given as its input.
    path: String,
    #[cfg(windows)]
    handle: std::os::windows::io::OwnedHandle,
    #[cfg(target_os = "linux")]
    fifo: PathBuf,
}

impl PipeServer {
    /// A new pipe tagged `tag` (`video`, `audio0`). On Linux the FIFO is made
    /// in `dir`, which goes when the recording's folder does.
    #[cfg(windows)]
    pub(crate) fn create(_dir: &Path, tag: &str) -> io::Result<Self> {
        use std::os::windows::io::{FromRawHandle, OwnedHandle};
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_OUTBOUND;
        use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_TYPE_BYTE, PIPE_WAIT};

        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = format!(r"\\.\pipe\windup-{}-{n}-{tag}", std::process::id());
        let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        // A megabyte of buffer: a few small frames, or a slice of a big one,
        // so the writer is rarely held up by ffmpeg's reading.
        let buffer = 1 << 20;
        // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the
        // call; no security attributes.
        let handle = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                PIPE_ACCESS_OUTBOUND,
                PIPE_TYPE_BYTE | PIPE_WAIT,
                1,
                buffer,
                0,
                0,
                std::ptr::null(),
            )
        };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a fresh handle we own and nothing else closes.
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        Ok(Self { path, handle })
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn create(dir: &Path, tag: &str) -> io::Result<Self> {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let fifo = dir.join(format!("{tag}-{n}.pipe"));
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )?;
        Ok(Self {
            path: fifo.to_string_lossy().into_owned(),
            fifo,
        })
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    pub(crate) fn create(_dir: &Path, _tag: &str) -> io::Result<Self> {
        let _ = &NEXT;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Recording through a pipe is not supported on this system.",
        ))
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    /// Waits for ffmpeg to open the pipe, then gives the end to write to.
    /// Gives up with `Interrupted` once `stop` is set.
    #[cfg(windows)]
    pub(crate) fn connect(self, _stop: &AtomicBool) -> io::Result<File> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::ERROR_PIPE_CONNECTED;
        use windows_sys::Win32::System::Pipes::ConnectNamedPipe;

        // SAFETY: a valid pipe handle, and no overlapped structure because
        // the pipe was not made for overlapped use.
        let ok = unsafe { ConnectNamedPipe(self.handle.as_raw_handle(), std::ptr::null_mut()) };
        if ok == 0 {
            let e = io::Error::last_os_error();
            // ffmpeg got there between create and connect: that is fine.
            if e.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) {
                return Err(e);
            }
        }
        Ok(File::from(self.handle))
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn connect(self, stop: &AtomicBool) -> io::Result<File> {
        use rustix::fs::OFlags;
        use std::os::unix::fs::OpenOptionsExt;
        // A blocking open would wait for a reader with no way to give up, so
        // open without blocking, which fails with ENXIO until ffmpeg has its
        // end, and only then make the writes block.
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(OFlags::NONBLOCK.bits() as i32)
                .open(&self.fifo)
            {
                Ok(file) => {
                    rustix::fs::fcntl_setfl(&file, OFlags::empty())?;
                    return Ok(file);
                }
                Err(e) if e.raw_os_error() == Some(rustix::io::Errno::NXIO.raw_os_error()) => {
                    if stop.load(Ordering::Relaxed) {
                        return Err(io::ErrorKind::Interrupted.into());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(e) => return Err(e),
            }
        }
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    pub(crate) fn connect(self, _stop: &AtomicBool) -> io::Result<File> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

/// Lets a writer stuck waiting for ffmpeg go, by opening the reading end
/// ourselves and closing it at once. Harmless if the writer is past that.
#[cfg(windows)]
pub(crate) fn unblock(path: &str) {
    let _ = std::fs::OpenOptions::new().read(true).open(path);
}

/// Elsewhere the writer's wait watches the stop flag itself.
#[cfg(not(windows))]
pub(crate) fn unblock(_path: &str) {}

/// Closes our end once ffmpeg has read everything in it. On Windows a pipe
/// closed with data still in it can lose that data, so this waits for the
/// reader first; when ffmpeg is gone the wait fails at once.
pub(crate) fn close(file: File) {
    #[cfg(windows)]
    let _ = file.sync_all();
    drop(file);
}
