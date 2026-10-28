//! Screenshots: desktop geometry and stitching. No UI, no platform code.

pub mod annotate;
pub mod backend;
pub mod geometry;
pub mod output;
pub mod pick;
pub mod raster;
pub mod selection;

pub use geometry::{DRect, DesktopShot, MonitorShot};
pub use pick::{TWEEN_SECS, Tween, WindowRect, edge_rect, window_at};
pub use selection::{Handle, MIN_DRAG, Selection};
