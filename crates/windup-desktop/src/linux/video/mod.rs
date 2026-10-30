//! Recording on Linux where Windup reads the frames itself: Wayland
//! screencopy (COSMIC, wlroots) and the ScreenCast portal over PipeWire
//! (KDE, GNOME). X11 needs nothing here, ffmpeg's x11grab reads the screen
//! (`crate::video::x11`). Sound is PipeWire's too (`audio`).

#[cfg(feature = "pipewire")]
pub(crate) mod audio;
#[cfg(feature = "pipewire")]
mod pw_stream;
mod screencast;
mod stream;

pub(crate) use screencast::{ScreenCastFactory, ScreenCastStills, open_screencast_stills};
pub(crate) use stream::WaylandFactory;

/// Built without PipeWire: there is nothing to read the portal's streams
/// with, and `open_screencast` says so before asking.
#[cfg(not(feature = "pipewire"))]
mod pw_stream {
    use std::os::fd::OwnedFd;
    use std::sync::{Arc, Mutex};

    use super::screencast::{PortalStream, Want};
    use super::stream::Latest;

    pub(super) struct PwStream {
        pub(super) size: (u32, u32),
    }

    pub(super) fn spawn(
        fd: OwnedFd,
        streams: &[PortalStream],
        want: Want,
        latest: Arc<Mutex<Latest>>,
    ) -> Result<PwStream, String> {
        let _ = (fd, streams, want, latest);
        Err("Windup was built without PipeWire.".into())
    }
}

/// Built without PipeWire: no sound to record, and settings say why
/// (`windup_core::video::pcm::SOUND_NEEDS_PIPEWIRE`).
#[cfg(not(feature = "pipewire"))]
pub(crate) mod audio {
    use crate::video::audio::AudioDevices;
    use crate::video::run::AudioSource;

    const NO_PIPEWIRE: &str = "Windup was built without PipeWire.";

    pub(crate) fn audio_devices() -> AudioDevices {
        AudioDevices::default()
    }

    pub(crate) fn open_system(device: Option<&str>) -> Result<Box<dyn AudioSource>, String> {
        let _ = device;
        Err(NO_PIPEWIRE.into())
    }

    pub(crate) fn open_mic(device: Option<&str>) -> Result<Box<dyn AudioSource>, String> {
        let _ = device;
        Err(NO_PIPEWIRE.into())
    }
}
