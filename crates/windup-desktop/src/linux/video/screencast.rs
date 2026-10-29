//! A recording's frames through the ScreenCast portal (KDE, GNOME), whose
//! compositors keep screen copying to themselves.
//!
//! The portal asks the user once which screens to share; Windup asks for
//! every monitor (`multiple`, `Monitor`) with the pointer drawn in, and for
//! the grant to be remembered (`ExplicitlyRevoked`). The answer is a set of
//! PipeWire streams, one per monitor, with where each monitor sits; those
//! are read on a thread of their own (`pw_stream`) and placed in one
//! picture by the same `Composer` the screencopy streams use.
//!
//! The portal session lives on a thread of its own too: it is held open for
//! as long as the recording reads it, and closed when the stream is dropped.
//! The desktop closing it (the user pressing "Stop sharing") ends the
//! recording with a reason.
//!
//! A restore token lets the next session skip the question. Each token is
//! good once; the session returns the next, which `ScreenCastFactory` keeps
//! for the next segment and hands to whoever saves it.

use std::os::fd::OwnedFd;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Poll, Waker};
use std::time::Duration;

use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};
use futures_util::StreamExt;
use futures_util::future::{Either, select};
use windup_core::capture::geometry::DRect;
use windup_core::video::screencast::{TokenChange, token_change};
use windup_core::video::{Source, even_size};

use super::pw_stream::{self, PwStream};
use super::stream::Latest;
use crate::capture::CaptureError;
use crate::video::run::{FrameSource, SourceFactory};

/// How long the portal may take, including the user answering its dialog
/// the first time: long enough to read it, short enough that a portal that
/// never answers does not hold a recording up for good.
const PORTAL_WAIT: Duration = Duration::from_secs(120);

/// Why a recording ends when the desktop stops the sharing.
pub(super) const STOPPED: &str = "The screen sharing was stopped.";

/// One stream the portal handed over: its PipeWire node, and where its
/// monitor sits and how big it is in desktop units, when the portal says.
#[derive(Clone, Debug)]
#[cfg_attr(not(feature = "pipewire"), allow(dead_code))]
pub(super) struct PortalStream {
    pub(super) node: u32,
    pub(super) position: Option<(i32, i32)>,
    pub(super) size: Option<(i32, i32)>,
}

/// The recorded rectangle through the ScreenCast portal: what `--record-test`
/// and the recording use on KDE and GNOME.
pub(crate) struct ScreenCastFactory {
    rect: DRect,
    /// The logical size, for `source` before a stream knows the real one.
    size: (u32, u32),
    /// The restore token for the next session.
    token: Mutex<Option<String>>,
}

impl ScreenCastFactory {
    /// `rect` in desktop (logical) units; `token` the saved restore token.
    /// `None` when the rectangle is too small.
    pub(crate) fn new(rect: DRect, token: Option<String>) -> Option<ScreenCastFactory> {
        let rect = rect.normalised();
        let size = even_size(rect.w.round() as u32, rect.h.round() as u32)?;
        Some(ScreenCastFactory {
            rect,
            size,
            token: Mutex::new(token),
        })
    }

    /// The token to save to `VideoConfig.screencast_token` once the
    /// recording has started.
    // Task 13 saves it through `change_config`; `--record-test` must not.
    #[allow(dead_code)]
    pub(crate) fn token(&self) -> Option<String> {
        self.token.lock().ok().and_then(|t| t.clone())
    }
}

impl SourceFactory for ScreenCastFactory {
    fn source(&self) -> Source {
        Source::RawPipe {
            path: String::new(),
            w: self.size.0,
            h: self.size.1,
        }
    }

    /// A session per segment: after a pause the token restores the grant,
    /// so the user is not asked again.
    fn open_pipe(&self) -> Option<Box<dyn FrameSource>> {
        let saved = self.token();
        match open_screencast(self.rect, saved.clone()) {
            Ok((stream, returned)) => {
                log::info!(
                    "ScreenCast: the portal {} a restore token",
                    if returned.is_some() {
                        "gave"
                    } else {
                        "gave no"
                    }
                );
                if let Ok(mut t) = self.token.lock() {
                    match token_change(saved.as_deref(), returned.as_deref()) {
                        TokenChange::Keep => {}
                        TokenChange::Set(new) => *t = Some(new),
                        TokenChange::Clear => *t = None,
                    }
                }
                Some(Box::new(stream))
            }
            Err(e) => {
                log::warn!("ScreenCast: {e:?}");
                None
            }
        }
    }
}

/// Asks the portal for every monitor and starts reading them, for `rect`
/// (desktop units). Gives the stream and the session's new restore token,
/// which the caller saves. A refused dialog is `Denied`, no portal (or a
/// build without PipeWire) `Unsupported`.
///
/// Blocks until the portal answers, which includes the user answering the
/// dialog: call it from a thread of its own, never the event thread.
pub(crate) fn open_screencast(
    rect: DRect,
    token: Option<String>,
) -> Result<(ScreenCastStream, Option<String>), CaptureError> {
    if !cfg!(feature = "pipewire") {
        // Asking would show the dialog for frames nothing could read.
        return Err(CaptureError::Unsupported);
    }
    let latest = Arc::new(Mutex::new(Latest::default()));
    let stop = Stop::default();
    let (tx, rx) = mpsc::channel();
    {
        let (stop, latest) = (stop.clone(), Arc::clone(&latest));
        std::thread::Builder::new()
            .name("windup-screencast-portal".into())
            .spawn(move || pollster::block_on(session(token, tx, stop, latest)))
            .map_err(|e| CaptureError::Failed(e.to_string()))?;
    }
    let started = match rx.recv_timeout(PORTAL_WAIT) {
        Ok(started) => started?,
        Err(_) => {
            stop.set();
            return Err(CaptureError::Failed(
                "the ScreenCast portal did not answer".into(),
            ));
        }
    };
    log::debug!("ScreenCast streams: {:?}", started.streams);
    match pw_stream::spawn(started.fd, &started.streams, rect, Arc::clone(&latest)) {
        Ok(pw) => Ok((
            ScreenCastStream {
                size: pw.size,
                latest,
                pw: Some(pw),
                stop,
            },
            started.token,
        )),
        Err(e) => {
            stop.set();
            Err(CaptureError::Failed(e))
        }
    }
}

/// What a started session hands over.
struct Started {
    fd: OwnedFd,
    streams: Vec<PortalStream>,
    token: Option<String>,
}

/// The portal thread: start the session, say what it gave, then hold it
/// open until `stop` or until the desktop closes it.
async fn session(
    token: Option<String>,
    ready: mpsc::Sender<Result<Started, CaptureError>>,
    stop: Stop,
    latest: Arc<Mutex<Latest>>,
) {
    let opened = async {
        let portal = Screencast::new().await?;
        let session = portal.create_session().await?;
        portal
            .select_sources(
                &session,
                CursorMode::Embedded,
                SourceType::Monitor.into(),
                true,
                token.as_deref(),
                PersistMode::ExplicitlyRevoked,
            )
            .await?
            .response()?;
        let started = portal.start(&session, None).await?.response()?;
        let fd = portal.open_pipe_wire_remote(&session).await?;
        ashpd::Result::Ok((session, started, fd))
    }
    .await;
    let (session, started, fd) = match opened {
        Ok(o) => o,
        Err(e) => {
            let _ = ready.send(Err(portal_error(e)));
            return;
        }
    };
    let streams = started
        .streams()
        .iter()
        .map(|s| PortalStream {
            node: s.pipe_wire_node_id(),
            position: s.position(),
            size: s.size(),
        })
        .collect();
    let token = started.restore_token().map(str::to_owned);
    // Listen before saying it started, so a close right after is not missed.
    let closed = session.receive_closed().await;
    if ready.send(Ok(Started { fd, streams, token })).is_err() {
        // Nobody waits any more (the wait timed out).
        let _ = session.close().await;
        return;
    }
    match closed {
        Ok(closed) => {
            let mut closed = pin!(closed);
            let stopped = pin!(stop.wait());
            if let Either::Left(_) = select(closed.next(), stopped).await {
                if let Ok(mut l) = latest.lock() {
                    l.fail(STOPPED.into());
                }
                return;
            }
        }
        Err(e) => {
            log::debug!("cannot watch the ScreenCast session: {e}");
            stop.wait().await;
        }
    }
    if let Err(e) = session.close().await {
        log::debug!("closing the ScreenCast session: {e}");
    }
}

fn portal_error(e: ashpd::Error) -> CaptureError {
    match e {
        ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)
        | ashpd::Error::Portal(ashpd::PortalError::NotAllowed(_)) => CaptureError::Denied,
        e if crate::linux::portal::absent(&e) => {
            log::debug!("no ScreenCast portal: {e}");
            CaptureError::Unsupported
        }
        e => CaptureError::Failed(e.to_string()),
    }
}

/// Tells the portal thread to close the session; a future it can wait on
/// alongside the session's own end.
#[derive(Clone, Default)]
struct Stop(Arc<StopInner>);

#[derive(Default)]
struct StopInner {
    set: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

impl Stop {
    fn set(&self) {
        self.0.set.store(true, Ordering::SeqCst);
        if let Some(w) = self.0.waker.lock().ok().and_then(|mut w| w.take()) {
            w.wake();
        }
    }

    async fn wait(&self) {
        std::future::poll_fn(|cx| {
            if self.0.set.load(Ordering::SeqCst) {
                return Poll::Ready(());
            }
            if let Ok(mut w) = self.0.waker.lock() {
                *w = Some(cx.waker().clone());
            }
            // Set between the first look and the waker being stored.
            if self.0.set.load(Ordering::SeqCst) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }
}

pub(crate) struct ScreenCastStream {
    size: (u32, u32),
    latest: Arc<Mutex<Latest>>,
    pw: Option<PwStream>,
    stop: Stop,
}

impl FrameSource for ScreenCastStream {
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn next_frame(&mut self, buf: &mut Vec<u8>) -> Result<bool, String> {
        self.latest.lock().map_err(|_| STOPPED)?.take(buf)
    }
}

impl Drop for ScreenCastStream {
    /// Stops reading first, then closes the session.
    fn drop(&mut self) {
        self.pw = None;
        self.stop.set();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_wakes_a_waiting_portal_thread() {
        let stop = Stop::default();
        let waiter = {
            let stop = stop.clone();
            std::thread::spawn(move || pollster::block_on(stop.wait()))
        };
        std::thread::sleep(Duration::from_millis(50));
        stop.set();
        waiter.join().unwrap();
        // Set before waiting: done at once.
        pollster::block_on(stop.wait());
    }
}
