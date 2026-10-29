//! Windows frame sources: `ddagrab` for a rectangle on one monitor, `gdigrab`
//! for one that spans several.

use windup_core::capture::geometry::DRect;
use windup_core::video::{Source, even_size};

use super::run::{FrameSource, SourceFactory};

/// Where a recording's picture comes from on Windows.
pub(crate) struct WindowsFactory {
    primary: Source,
    fallback: Source,
}

impl WindowsFactory {
    /// `rect` is in desktop units (physical pixels of the virtual screen) and
    /// `monitors` are the monitors' layouts in the order Windows lists them.
    /// `None` when the rectangle is too small to record.
    pub(crate) fn new(rect: DRect, monitors: &[DRect]) -> Option<WindowsFactory> {
        let rect = rect.normalised();
        let (w, h) = even_size(rect.w.round() as u32, rect.h.round() as u32)?;
        let (x, y) = (rect.x.round() as i32, rect.y.round() as i32);
        let gdigrab = Source::Gdigrab { x, y, w, h };
        let primary = match monitor_holding(x, y, w, h, monitors) {
            Some((output, m)) => Source::Ddagrab {
                output,
                x: x - m.x.round() as i32,
                y: y - m.y.round() as i32,
                w,
                h,
            },
            None => gdigrab.clone(),
        };
        Some(WindowsFactory {
            primary,
            fallback: gdigrab,
        })
    }
}

/// The monitor that wholly holds the rectangle, with its ddagrab output
/// index. ddagrab numbers outputs the way DXGI does; this takes that to be
/// the order `EnumDisplayMonitors` gives, which agrees on the usual single
/// graphics card but is not promised. When it does not, ddagrab either fails
/// at once and the recording falls back to gdigrab.
fn monitor_holding(x: i32, y: i32, w: u32, h: u32, monitors: &[DRect]) -> Option<(u32, DRect)> {
    let (r, b) = (x + w as i32, y + h as i32);
    monitors.iter().enumerate().find_map(|(i, m)| {
        let inside = x >= m.x.round() as i32
            && y >= m.y.round() as i32
            && r <= m.right().round() as i32
            && b <= m.bottom().round() as i32;
        inside.then_some((i as u32, *m))
    })
}

impl SourceFactory for WindowsFactory {
    fn source(&self) -> Source {
        self.primary.clone()
    }

    fn open_pipe(&self) -> Option<Box<dyn FrameSource>> {
        None
    }

    fn fallback(&self) -> Option<Source> {
        matches!(self.primary, Source::Ddagrab { .. }).then(|| self.fallback.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> DRect {
        DRect { x, y, w, h }
    }

    /// A 1920x1080 monitor at the origin, a second to its right, and a third
    /// to the left of the first (negative coordinates).
    fn desk() -> Vec<DRect> {
        vec![
            r(0.0, 0.0, 1920.0, 1080.0),
            r(1920.0, 0.0, 2560.0, 1440.0),
            r(-1280.0, 100.0, 1280.0, 1024.0),
        ]
    }

    #[test]
    fn a_rectangle_on_one_monitor_is_ddagrab_relative_to_it() {
        let f = WindowsFactory::new(r(2000.0, 50.0, 801.0, 600.0), &desk()).unwrap();
        assert_eq!(
            f.source(),
            Source::Ddagrab {
                output: 1,
                x: 80,
                y: 50,
                w: 800,
                h: 600
            }
        );
        assert_eq!(
            f.fallback(),
            Some(Source::Gdigrab {
                x: 2000,
                y: 50,
                w: 800,
                h: 600
            })
        );
    }

    #[test]
    fn negative_origins_work() {
        let f = WindowsFactory::new(r(-1200.0, 200.0, 400.0, 300.0), &desk()).unwrap();
        assert_eq!(
            f.source(),
            Source::Ddagrab {
                output: 2,
                x: 80,
                y: 100,
                w: 400,
                h: 300
            }
        );
    }

    #[test]
    fn a_rectangle_across_monitors_is_gdigrab_in_virtual_desktop_pixels() {
        let f = WindowsFactory::new(r(1800.0, 10.0, 400.0, 300.0), &desk()).unwrap();
        assert_eq!(
            f.source(),
            Source::Gdigrab {
                x: 1800,
                y: 10,
                w: 400,
                h: 300
            }
        );
        assert_eq!(f.fallback(), None, "nothing to fall back to");
    }

    #[test]
    fn a_whole_monitor_fits_and_a_sliver_too_small_is_refused() {
        let f = WindowsFactory::new(desk()[1], &desk()).unwrap();
        assert!(matches!(
            f.source(),
            Source::Ddagrab {
                output: 1,
                x: 0,
                y: 0,
                w: 2560,
                h: 1440
            }
        ));
        assert!(WindowsFactory::new(r(0.0, 0.0, 1.0, 100.0), &desk()).is_none());
    }
}
