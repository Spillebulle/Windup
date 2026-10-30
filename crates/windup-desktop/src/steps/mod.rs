//! Steps as the app runs it: a camera thread that takes a picture for each
//! click or key press, a writer thread that files them, and the cameras.

// Nothing starts a session until the platform cameras and the hosts' bars
// are in; remove this once they are.
#![allow(dead_code)]

pub(crate) mod camera;
pub(crate) mod live;
