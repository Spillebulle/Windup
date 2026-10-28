//! Screenshots: desktop geometry and stitching. No UI, no platform code.

pub mod annotate;
pub mod geometry;
pub mod raster;
pub mod selection;

pub use geometry::{DRect, DesktopShot, MonitorShot};
pub use selection::{Handle, MIN_DRAG, Selection};
