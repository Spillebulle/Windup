//! Steps as the app runs it: a camera thread that takes a picture for each
//! click or key press, a writer thread that files them, the cameras, and the
//! review's host.

pub(crate) mod camera;
pub(crate) mod clicks;
pub(crate) mod live;
pub(crate) mod review;
#[cfg(target_os = "windows")]
pub(crate) mod windows;
#[cfg(target_os = "linux")]
pub(crate) mod x11;
