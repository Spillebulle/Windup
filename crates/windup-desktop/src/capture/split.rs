//! Cutting one picture of the whole desktop into the monitors that are in it.
//!
//! The screenshot portal (GNOME) hands back a single image of everything. The
//! compositor's logical layout says where each monitor sits in it; the image
//! is a uniform scaling of the layout's bounding box.

use image::RgbaImage;
use image::imageops::crop_imm;
use windup_core::capture::geometry::{DRect, DesktopShot, MonitorShot};

/// Split `img` by `layout` (name, logical rectangle). Pixels per layout unit
/// is the image width over the width of the layout's bounding box. A monitor
/// that falls outside the image (or whose rounded crop would be empty) is
/// left out.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn split_desktop(img: RgbaImage, layout: &[(String, DRect)]) -> DesktopShot {
    let Some(bounds) = layout.iter().map(|(_, r)| *r).reduce(|a, b| a.union(&b)) else {
        return DesktopShot { monitors: vec![] };
    };
    if bounds.w <= 0.0 || bounds.h <= 0.0 {
        return DesktopShot { monitors: vec![] };
    }
    let scale = img.width() as f32 / bounds.w;
    let monitors = layout
        .iter()
        .filter_map(|(name, rect)| {
            let x0 = (((rect.x - bounds.x) * scale).round().max(0.0) as u32).min(img.width());
            let y0 = (((rect.y - bounds.y) * scale).round().max(0.0) as u32).min(img.height());
            let x1 = (((rect.right() - bounds.x) * scale).round().max(0.0) as u32).min(img.width());
            let y1 =
                (((rect.bottom() - bounds.y) * scale).round().max(0.0) as u32).min(img.height());
            (x1 > x0 && y1 > y0).then(|| MonitorShot {
                layout: *rect,
                image: crop_imm(&img, x0, y0, x1 - x0, y1 - y0).to_image(),
                name: name.clone(),
                scale: 1.0,
            })
        })
        .collect();
    DesktopShot { monitors }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn r(x: f32, y: f32, w: f32, h: f32) -> DRect {
        DRect { x, y, w, h }
    }

    type Block = ((u32, u32, u32, u32), [u8; 4]);

    /// Paints each (x0, y0, x1, y1) block of `img` its colour.
    fn paint(img: &mut RgbaImage, blocks: &[Block]) {
        for ((x0, y0, x1, y1), c) in blocks {
            for y in *y0..*y1 {
                for x in *x0..*x1 {
                    img.put_pixel(x, y, Rgba(*c));
                }
            }
        }
    }

    #[test]
    fn two_monitors_side_by_side_in_one_2x_image() {
        // Layout 300x100 units; the image is 600x200 pixels.
        let mut img = RgbaImage::new(600, 200);
        paint(
            &mut img,
            &[
                ((0, 0, 400, 200), [255, 0, 0, 255]),
                ((400, 0, 600, 200), [0, 0, 255, 255]),
            ],
        );
        let layout = vec![
            ("DP-1".to_owned(), r(0.0, 0.0, 200.0, 100.0)),
            ("HDMI-1".to_owned(), r(200.0, 0.0, 100.0, 100.0)),
        ];
        let shot = split_desktop(img, &layout);
        assert_eq!(shot.monitors.len(), 2);
        let (a, b) = (&shot.monitors[0], &shot.monitors[1]);
        assert_eq!(
            (a.name.as_str(), a.image.dimensions()),
            ("DP-1", (400, 200))
        );
        assert_eq!(
            (b.name.as_str(), b.image.dimensions()),
            ("HDMI-1", (200, 200))
        );
        assert_eq!(a.layout, layout[0].1);
        assert_eq!(a.image.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(a.image.get_pixel(399, 199).0, [255, 0, 0, 255]);
        assert_eq!(b.image.get_pixel(0, 0).0, [0, 0, 255, 255]);
        assert_eq!(a.px_per_unit(), 2.0);
        assert_eq!(b.px_per_unit(), 2.0);
    }

    #[test]
    fn a_negative_origin_is_measured_from_the_bounding_box() {
        // A monitor left of and above the primary: layout x from -100.
        let mut img = RgbaImage::new(300, 100);
        paint(
            &mut img,
            &[
                ((0, 0, 100, 100), [1, 2, 3, 255]),
                ((100, 0, 300, 100), [9, 8, 7, 255]),
            ],
        );
        let layout = vec![
            ("left".to_owned(), r(-100.0, -50.0, 100.0, 100.0)),
            ("main".to_owned(), r(0.0, -50.0, 200.0, 100.0)),
        ];
        let shot = split_desktop(img, &layout);
        assert_eq!(shot.monitors[0].image.dimensions(), (100, 100));
        assert_eq!(shot.monitors[0].image.get_pixel(50, 50).0, [1, 2, 3, 255]);
        assert_eq!(shot.monitors[1].image.dimensions(), (200, 100));
        assert_eq!(shot.monitors[1].image.get_pixel(0, 0).0, [9, 8, 7, 255]);
    }

    #[test]
    fn a_vertical_stack_is_cut_by_rows() {
        let mut img = RgbaImage::new(100, 300);
        paint(
            &mut img,
            &[
                ((0, 0, 100, 100), [10, 0, 0, 255]),
                ((0, 100, 100, 300), [0, 10, 0, 255]),
            ],
        );
        let layout = vec![
            ("top".to_owned(), r(0.0, 0.0, 100.0, 100.0)),
            ("bottom".to_owned(), r(0.0, 100.0, 100.0, 200.0)),
        ];
        let shot = split_desktop(img, &layout);
        assert_eq!(shot.monitors[0].image.dimensions(), (100, 100));
        assert_eq!(shot.monitors[1].image.dimensions(), (100, 200));
        assert_eq!(shot.monitors[0].image.get_pixel(5, 99).0, [10, 0, 0, 255]);
        assert_eq!(shot.monitors[1].image.get_pixel(5, 0).0, [0, 10, 0, 255]);
    }

    #[test]
    fn a_fractional_scale_rounds_to_whole_pixels_inside_the_image() {
        // 1.5x: 3 monitors of 100 units each in a 450 pixel wide image.
        let img = RgbaImage::new(450, 150);
        let layout: Vec<_> = (0..3)
            .map(|i| (format!("m{i}"), r(100.0 * i as f32, 0.0, 100.0, 100.0)))
            .collect();
        let shot = split_desktop(img, &layout);
        let widths: Vec<u32> = shot.monitors.iter().map(|m| m.image.width()).collect();
        assert_eq!(widths.iter().sum::<u32>(), 450);
    }

    #[test]
    fn no_layout_or_an_empty_one_gives_no_monitors() {
        assert!(
            split_desktop(RgbaImage::new(10, 10), &[])
                .monitors
                .is_empty()
        );
        let zero = vec![("x".to_owned(), r(0.0, 0.0, 0.0, 0.0))];
        assert!(
            split_desktop(RgbaImage::new(10, 10), &zero)
                .monitors
                .is_empty()
        );
    }
}
