//! Steps on Wayland: clicks read from the pointer devices themselves
//! (evdev), the one-time setup that makes them readable where no udev
//! rule does, and the cameras: screencopy (COSMIC, wlroots), KWin (KDE's
//! installed package) and the ScreenCast portal (GNOME, the KDE AppImage).

// The settings pane's "Capture clicks too" and the hosts' sessions are not
// in yet; remove this once they are.
#![allow(dead_code)]

pub(crate) mod evdev;
pub(crate) mod kwin;
pub(crate) mod screencast;
pub(crate) mod screencopy;
pub(crate) mod setup;
