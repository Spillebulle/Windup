//! Rectangles in one desktop coordinate space, and monitor images stitched
//! into a single picture.
//!
//! Desktop units are whatever the platform lays monitors out in: Windows
//! virtual-screen physical pixels, X11 root pixels, Wayland logical layout.
//! A monitor's image may hold more pixels than its layout has units (HiDPI).
//! Screen points are what the user sees and sizes are set in; each monitor
//! says how many desktop units one point is (`MonitorShot::scale`).

use image::imageops::{self, FilterType};
use image::{Rgba, RgbaImage};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl DRect {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// Half-open: the left/top edge is inside, the right/bottom edge is not.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// The overlap, or `None` when the rectangles only touch or are apart.
    pub fn intersect(&self, other: &DRect) -> Option<DRect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let r = self.right().min(other.right());
        let b = self.bottom().min(other.bottom());
        (r > x && b > y).then_some(DRect {
            x,
            y,
            w: r - x,
            h: b - y,
        })
    }

    pub fn union(&self, other: &DRect) -> DRect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        DRect {
            x,
            y,
            w: self.right().max(other.right()) - x,
            h: self.bottom().max(other.bottom()) - y,
        }
    }

    /// Positive width and height, whichever way round the corners were.
    pub fn normalised(&self) -> DRect {
        DRect::from_corners((self.x, self.y), (self.right(), self.bottom()))
    }

    pub fn from_corners(a: (f32, f32), b: (f32, f32)) -> DRect {
        DRect {
            x: a.0.min(b.0),
            y: a.1.min(b.1),
            w: (a.0 - b.0).abs(),
            h: (a.1 - b.1).abs(),
        }
    }
}

/// `wl_output.transform`, in the protocol's order: what the compositor did
/// to a monitor's content to suit the panel (90 is counter-clockwise, flips
/// are about the vertical axis and come first). A copied buffer arrives
/// transformed; undoing it gives the picture as it appears on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transform {
    Normal,
    Rot90,
    Rot180,
    Rot270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

impl Transform {
    /// Whether the buffer is the screen on its side (width and height swap).
    pub fn swaps_axes(self) -> bool {
        matches!(
            self,
            Transform::Rot90 | Transform::Rot270 | Transform::Flipped90 | Transform::Flipped270
        )
    }

    /// Where on-screen pixel `(x, y)` of a `w` x `h` screen lies in the
    /// transformed buffer. Each buffer coordinate depends on one screen
    /// coordinate only, which is what lets a per-frame copy use one table
    /// per axis.
    pub fn buffer_point(self, x: u32, y: u32, w: u32, h: u32) -> (u32, u32) {
        let (rx, ry) = (w - 1 - x, h - 1 - y);
        match self {
            Transform::Normal => (x, y),
            Transform::Rot90 => (y, rx),
            Transform::Rot180 => (rx, ry),
            Transform::Rot270 => (ry, x),
            Transform::Flipped => (rx, y),
            Transform::Flipped90 => (y, x),
            Transform::Flipped180 => (x, ry),
            Transform::Flipped270 => (ry, rx),
        }
    }
}

pub struct MonitorShot {
    pub layout: DRect,
    pub image: RgbaImage,
    pub name: String,
    /// Desktop units per screen point on this monitor, so sizes the user
    /// sees (handles, pen widths, a nudge) are alike everywhere: the OS
    /// scale where desktop units are physical pixels (Windows DPI / 96,
    /// X11 Xft.dpi / 96), and 1 on Wayland, whose layout is in points.
    pub scale: f32,
    /// The monitor's refresh rate when the platform says; the recording
    /// caps its frame rate at it. Wayland back ends leave it unknown.
    pub refresh_hz: Option<u32>,
}

impl MonitorShot {
    pub fn px_per_unit(&self) -> f32 {
        self.image.width() as f32 / self.layout.w
    }
}

pub struct DesktopShot {
    pub monitors: Vec<MonitorShot>,
}

impl DesktopShot {
    /// The union of every monitor's layout (all zero when there are none).
    pub fn bounds(&self) -> DRect {
        self.monitors
            .iter()
            .map(|m| m.layout)
            .reduce(|a, b| a.union(&b))
            .unwrap_or(DRect {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            })
    }

    pub fn monitor_at(&self, x: f32, y: f32) -> Option<usize> {
        self.monitors.iter().position(|m| m.layout.contains(x, y))
    }

    /// The picture of `rect`, at the sharpest scale of the monitors it
    /// touches. Monitors at a lower scale are upsampled; anything no monitor
    /// covers stays transparent. `None` when the result would be under one
    /// pixel either way.
    pub fn compose(&self, rect: DRect) -> Option<RgbaImage> {
        let rect = rect.normalised();
        if ![rect.x, rect.y, rect.w, rect.h]
            .iter()
            .all(|v| v.is_finite())
        {
            return None;
        }
        // Never larger than the desktop, however far the rect reaches.
        let rect = rect.intersect(&self.bounds())?;
        let touched: Vec<&MonitorShot> = self
            .monitors
            .iter()
            .filter(|m| m.layout.intersect(&rect).is_some())
            .collect();
        let scale = touched
            .iter()
            .map(|m| m.px_per_unit())
            .fold(0.0_f32, f32::max);
        let out_w = (rect.w * scale).round() as u32;
        let out_h = (rect.h * scale).round() as u32;
        if out_w == 0 || out_h == 0 {
            return None;
        }
        let mut out = RgbaImage::from_pixel(out_w, out_h, Rgba([0, 0, 0, 0]));
        for m in touched {
            let Some(part) = m.layout.intersect(&rect) else {
                continue;
            };
            // Round edges, not sizes, so neighbours meet without a seam.
            let edge = |v: f32, origin: f32, s: f32, max: u32| {
                (((v - origin) * s).round().max(0.0) as u32).min(max)
            };
            let (dx0, dx1) = (
                edge(part.x, rect.x, scale, out_w),
                edge(part.right(), rect.x, scale, out_w),
            );
            let (dy0, dy1) = (
                edge(part.y, rect.y, scale, out_h),
                edge(part.bottom(), rect.y, scale, out_h),
            );
            let ppu = m.px_per_unit();
            let (iw, ih) = m.image.dimensions();
            let (sx0, sx1) = (
                edge(part.x, m.layout.x, ppu, iw),
                edge(part.right(), m.layout.x, ppu, iw),
            );
            let (sy0, sy1) = (
                edge(part.y, m.layout.y, ppu, ih),
                edge(part.bottom(), m.layout.y, ppu, ih),
            );
            if dx1 <= dx0 || dy1 <= dy0 || sx1 <= sx0 || sy1 <= sy0 {
                continue;
            }
            let src = imageops::crop_imm(&m.image, sx0, sy0, sx1 - sx0, sy1 - sy0).to_image();
            let (dw, dh) = (dx1 - dx0, dy1 - dy0);
            // Same scale: copy pixel for pixel. Only a lower scale is resampled.
            let src = if src.dimensions() == (dw, dh) {
                src
            } else {
                imageops::resize(&src, dw, dh, FilterType::Triangle)
            };
            imageops::replace(&mut out, &src, dx0 as i64, dy0 as i64);
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);
    const BLUE: Rgba<u8> = Rgba([0, 0, 255, 255]);

    fn rect(x: f32, y: f32, w: f32, h: f32) -> DRect {
        DRect { x, y, w, h }
    }

    fn mon(layout: DRect, px: (u32, u32), c: Rgba<u8>) -> MonitorShot {
        MonitorShot {
            layout,
            image: RgbaImage::from_pixel(px.0, px.1, c),
            name: String::new(),
            scale: 1.0,
            refresh_hz: None,
        }
    }

    #[test]
    fn compose_inside_one_monitor_is_a_crop() {
        let shot = DesktopShot {
            monitors: vec![mon(rect(0.0, 0.0, 1920.0, 1080.0), (1920, 1080), RED)],
        };
        let img = shot.compose(rect(10.0, 10.0, 100.0, 50.0)).unwrap();
        assert_eq!(img.dimensions(), (100, 50));
        assert!(img.pixels().all(|p| *p == RED));
    }

    #[test]
    fn compose_across_a_left_monitor_with_negative_x() {
        let shot = DesktopShot {
            monitors: vec![
                mon(rect(-1920.0, 0.0, 1920.0, 1080.0), (1920, 1080), RED),
                mon(rect(0.0, 0.0, 1920.0, 1080.0), (1920, 1080), BLUE),
            ],
        };
        let img = shot.compose(rect(-50.0, 0.0, 100.0, 10.0)).unwrap();
        assert_eq!(img.dimensions(), (100, 10));
        assert_eq!(*img.get_pixel(0, 5), RED);
        assert_eq!(*img.get_pixel(49, 5), RED);
        assert_eq!(*img.get_pixel(50, 5), BLUE);
        assert_eq!(*img.get_pixel(99, 5), BLUE);
    }

    #[test]
    fn mixed_scale_uses_the_highest() {
        let shot = DesktopShot {
            monitors: vec![
                mon(rect(0.0, 0.0, 1000.0, 500.0), (1000, 500), RED),
                mon(rect(1000.0, 0.0, 1000.0, 500.0), (2000, 1000), BLUE),
            ],
        };
        let img = shot.compose(rect(900.0, 0.0, 200.0, 100.0)).unwrap();
        assert_eq!(img.dimensions(), (400, 200));
        assert_eq!(*img.get_pixel(0, 100), RED);
        assert_eq!(*img.get_pixel(199, 100), RED);
        assert_eq!(*img.get_pixel(200, 100), BLUE);
        assert_eq!(*img.get_pixel(399, 199), BLUE);
    }

    #[test]
    fn gaps_are_transparent() {
        let shot = DesktopShot {
            monitors: vec![
                mon(rect(0.0, 0.0, 1920.0, 1080.0), (1920, 1080), RED),
                mon(rect(1920.0, 200.0, 1920.0, 1080.0), (1920, 1080), BLUE),
            ],
        };
        let img = shot.compose(rect(1910.0, 0.0, 20.0, 300.0)).unwrap();
        assert_eq!(img.dimensions(), (20, 300));
        assert_eq!(*img.get_pixel(15, 100), Rgba([0, 0, 0, 0]));
        assert_eq!(*img.get_pixel(15, 250), BLUE);
        assert_eq!(*img.get_pixel(5, 100), RED);
    }

    #[test]
    fn a_degenerate_rect_composes_to_none() {
        let shot = DesktopShot {
            monitors: vec![mon(rect(0.0, 0.0, 100.0, 100.0), (100, 100), RED)],
        };
        assert!(shot.compose(rect(5.0, 5.0, 0.3, 40.0)).is_none());
        assert!(shot.compose(rect(500.0, 5.0, 40.0, 40.0)).is_none());
    }

    #[test]
    fn a_non_finite_rect_composes_to_none() {
        let shot = DesktopShot {
            monitors: vec![mon(rect(0.0, 0.0, 100.0, 100.0), (100, 100), RED)],
        };
        assert!(shot.compose(rect(0.0, 0.0, f32::INFINITY, 10.0)).is_none());
        assert!(shot.compose(rect(f32::NAN, 0.0, 10.0, 10.0)).is_none());
    }

    #[test]
    fn a_huge_rect_is_clipped_to_the_desktop() {
        let shot = DesktopShot {
            monitors: vec![mon(rect(0.0, 0.0, 100.0, 50.0), (100, 50), RED)],
        };
        let img = shot.compose(rect(-1e9, -1e9, 2e9, 2e9)).unwrap();
        assert_eq!(img.dimensions(), (100, 50));
        assert!(img.pixels().all(|p| *p == RED));
    }

    #[test]
    fn rect_helpers() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        assert!(a.contains(0.0, 0.0) && !a.contains(10.0, 5.0));
        assert_eq!(a.intersect(&rect(10.0, 0.0, 5.0, 5.0)), None);
        assert_eq!(
            a.intersect(&rect(5.0, 5.0, 10.0, 10.0)),
            Some(rect(5.0, 5.0, 5.0, 5.0))
        );
        assert_eq!(
            a.union(&rect(5.0, 5.0, 10.0, 10.0)),
            rect(0.0, 0.0, 15.0, 15.0)
        );
        assert_eq!(
            DRect::from_corners((9.0, 8.0), (3.0, 2.0)),
            rect(3.0, 2.0, 6.0, 6.0)
        );
    }
}
