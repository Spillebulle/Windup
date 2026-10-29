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
use crate::video::outcome::SCREENS_NOT_SHARED;
use crate::video::run::{FrameSource, SourceFactory, UNREADABLE};

/// How long the portal may take, including the user answering its dialog
/// the first time: long enough to read it, short enough that a portal that
/// never answers does not hold a recording up for good.
const PORTAL_WAIT: Duration = Duration::from_secs(120);

/// Why a recording ends when the desktop stops the sharing.
pub(super) const STOPPED: &str = crate::video::outcome::SHARING_STOPPED;

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
    /// The restore token for the next session, shared with whoever saves
    /// it (`token_slot`).
    token: Arc<Mutex<Option<String>>>,
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
            token: Arc::new(Mutex::new(token)),
        })
    }

    /// Where the next token lands, for saving to its own file
    /// (`Config::save_screencast_token`) once the recording has started (and
    /// again at its end: a pause starts a new session, which spends it).
    /// `--record-test` never saves it.
    pub(crate) fn token_slot(&self) -> Arc<Mutex<Option<String>>> {
        Arc::clone(&self.token)
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
    fn open_pipe(&self) -> Result<Box<dyn FrameSource>, String> {
        match open_screencast(self.rect, &self.token) {
            Ok(stream) => Ok(Box::new(stream)),
            Err(e) => {
                log::warn!("ScreenCast: {e:?}");
                Err(after_failure(&self.token, &e))
            }
        }
    }
}

/// Asks the portal for every monitor and starts reading them, for `rect`
/// (desktop units), with the restore token in `token`. As soon as the
/// session has started, `token` holds the next one (`token_change`), even if
/// reading the streams then fails: the one sent is spent either way. The
/// caller saves it. A refused dialog is `Denied` and no portal (or a build
/// without PipeWire) `Unsupported`; both leave `token` as it was.
///
/// Blocks until the portal answers, which includes the user answering the
/// dialog: call it from a thread of its own, never the event thread.
pub(crate) fn open_screencast(
    rect: DRect,
    token: &Mutex<Option<String>>,
) -> Result<ScreenCastStream, CaptureError> {
    if !cfg!(feature = "pipewire") {
        // Asking would show the dialog for frames nothing could read.
        return Err(CaptureError::Unsupported);
    }
    let saved = token.lock().ok().and_then(|t| t.clone());
    let latest = Arc::new(Mutex::new(Latest::default()));
    let stop = Stop::default();
    let (tx, rx) = mpsc::channel();
    {
        let (stop, latest, sent) = (stop.clone(), Arc::clone(&latest), saved.clone());
        std::thread::Builder::new()
            .name("windup-screencast-portal".into())
            .spawn(move || pollster::block_on(session(sent, tx, stop, latest)))
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
    log::info!(
        "ScreenCast: the portal {} a restore token",
        if started.token.is_some() {
            "gave"
        } else {
            "gave no"
        }
    );
    apply_token(token, saved.as_deref(), started.token.as_deref());
    match pw_stream::spawn(started.fd, &started.streams, rect, Arc::clone(&latest)) {
        Ok(pw) => Ok(ScreenCastStream {
            size: pw.size,
            latest,
            pw: Some(pw),
            stop,
        }),
        Err(e) => {
            stop.set();
            Err(CaptureError::Failed(e))
        }
    }
}

/// Puts the next restore token in `slot` once a session started with
/// `saved` and the portal `returned` one (or none).
fn apply_token(slot: &Mutex<Option<String>>, saved: Option<&str>, returned: Option<&str>) {
    if let Ok(mut t) = slot.lock() {
        match token_change(saved, returned) {
            TokenChange::Keep => {}
            TokenChange::Set(new) => *t = Some(new),
            TokenChange::Clear => *t = None,
        }
    }
}

/// What a session that failed leaves: the reason for the notice, and the
/// token slot emptied when the grant shares too few screens for the area,
/// so the next recording asks which to share instead of restoring the same
/// grant and failing again.
fn after_failure(slot: &Mutex<Option<String>>, e: &CaptureError) -> String {
    match e {
        CaptureError::Failed(why) if why == SCREENS_NOT_SHARED => {
            if let Ok(mut t) = slot.lock() {
                *t = None;
            }
            SCREENS_NOT_SHARED.into()
        }
        _ => UNREADABLE.into(),
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
    fn a_started_session_leaves_the_next_token_in_the_slot() {
        let slot = Mutex::new(Some("a".to_owned()));
        apply_token(&slot, Some("a"), Some("b"));
        assert_eq!(slot.lock().unwrap().as_deref(), Some("b"));
        // No token back: the spent one is not kept.
        apply_token(&slot, Some("b"), None);
        assert_eq!(*slot.lock().unwrap(), None);
    }

    #[test]
    fn too_few_shared_screens_forget_the_grant() {
        let slot = Mutex::new(Some("next".to_owned()));
        let why = after_failure(&slot, &CaptureError::Failed(SCREENS_NOT_SHARED.into()));
        assert_eq!(why, SCREENS_NOT_SHARED);
        assert_eq!(*slot.lock().unwrap(), None, "the next recording asks");
        // Any other failure keeps the token.
        let slot = Mutex::new(Some("next".to_owned()));
        let why = after_failure(&slot, &CaptureError::Failed("PipeWire: gone".into()));
        assert_eq!(why, UNREADABLE);
        assert_eq!(slot.lock().unwrap().as_deref(), Some("next"));
        let _ = after_failure(&slot, &CaptureError::Denied);
        assert_eq!(slot.lock().unwrap().as_deref(), Some("next"));
    }

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
