//! The X11 frame source: ffmpeg's `x11grab` reads the screen itself.

use windup_core::capture::geometry::DRect;
use windup_core::video::{Source, even_size};

use super::run::{FrameSource, SourceFactory};

pub(crate) struct X11Factory {
    source: Source,
}

impl X11Factory {
    /// `rect` is in root pixels, which are physical on X11 (scale 1).
    /// `None` when it is too small to record.
    pub(crate) fn new(display: &str, rect: DRect) -> Option<X11Factory> {
        let rect = rect.normalised();
        let (w, h) = even_size(rect.w.round() as u32, rect.h.round() as u32)?;
        Some(X11Factory {
            source: Source::X11grab {
                display: display.to_owned(),
                x: rect.x.round() as i32,
                y: rect.y.round() as i32,
                w,
                h,
            },
        })
    }

    /// The factory for `$DISPLAY`.
    pub(crate) fn from_env(rect: DRect) -> Option<X11Factory> {
        let display = std::env::var("DISPLAY").ok().filter(|d| !d.is_empty())?;
        Self::new(&display, rect)
    }
}

impl SourceFactory for X11Factory {
    fn source(&self) -> Source {
        self.source.clone()
    }

    fn open_pipe(&self) -> Option<Box<dyn FrameSource>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rectangle_and_display_go_to_x11grab_with_an_even_size() {
        let rect = DRect {
            x: 10.0,
            y: 20.0,
            w: 321.0,
            h: 241.0,
        };
        let f = X11Factory::new(":1", rect).unwrap();
        assert_eq!(
            f.source(),
            Source::X11grab {
                display: ":1".into(),
                x: 10,
                y: 20,
                w: 320,
                h: 240
            }
        );
        assert!(f.open_pipe().is_none());
        assert!(f.fallback().is_none());
    }
}
