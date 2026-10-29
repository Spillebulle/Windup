//! Recording on Linux where Windup reads the frames itself: Wayland
//! screencopy (COSMIC, wlroots). X11 needs nothing here, ffmpeg's x11grab
//! reads the screen (`crate::video::x11`).

mod stream;

pub(crate) use stream::WaylandFactory;
