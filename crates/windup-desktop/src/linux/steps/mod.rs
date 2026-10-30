//! Steps on Wayland: clicks read from the pointer devices themselves
//! (evdev), and the one-time setup that makes them readable where no udev
//! rule does.

// The settings pane's "Capture clicks too" and the hosts' sessions are not
// in yet; remove this once they are.
#![allow(dead_code)]

pub(crate) mod evdev;
pub(crate) mod setup;
