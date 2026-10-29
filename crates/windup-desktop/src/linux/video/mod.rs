//! Recording on Linux where Windup reads the frames itself: Wayland
//! screencopy (COSMIC, wlroots) and the ScreenCast portal over PipeWire
//! (KDE, GNOME). X11 needs nothing here, ffmpeg's x11grab reads the screen
//! (`crate::video::x11`).

#[cfg(feature = "pipewire")]
mod pw_stream;
mod screencast;
mod stream;

pub(crate) use screencast::ScreenCastFactory;
pub(crate) use stream::WaylandFactory;

/// Built without PipeWire: there is nothing to read the portal's streams
/// with, and `open_screencast` says so before asking.
#[cfg(not(feature = "pipewire"))]
mod pw_stream {
    use std::os::fd::OwnedFd;
    use std::sync::{Arc, Mutex};

    use windup_core::capture::geometry::DRect;

    use super::screencast::PortalStream;
    use super::stream::Latest;

    pub(super) struct PwStream {
        pub(super) size: (u32, u32),
    }

    pub(super) fn spawn(
        fd: OwnedFd,
        streams: &[PortalStream],
        rect: DRect,
        latest: Arc<Mutex<Latest>>,
    ) -> Result<PwStream, String> {
        let _ = (fd, streams, rect, latest);
        Err("Windup was built without PipeWire.".into())
    }
}
