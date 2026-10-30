//! Where the pointer was, in the picture's own pixels.

use crate::capture::geometry::DRect;

/// The pointer's hot spot from the cursor image's corner.
pub fn cursor_point(position: (i32, i32), hotspot: (i32, i32)) -> [f32; 2] {
    [
        (position.0 + hotspot.0) as f32,
        (position.1 + hotspot.1) as f32,
    ]
}

/// A desktop point in the pixels of the picture of the monitor at `layout`;
/// none when the point is on another monitor.
pub fn to_image(global: (f32, f32), layout: DRect, image: (u32, u32)) -> Option<[f32; 2]> {
    if !layout.contains(global.0, global.1) || layout.w <= 0.0 || layout.h <= 0.0 {
        return None;
    }
    Some([
        (global.0 - layout.x) / layout.w * image.0 as f32,
        (global.1 - layout.y) / layout.h * image.1 as f32,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_point_is_the_image_corner_plus_the_hotspot() {
        assert_eq!(cursor_point((100, 50), (4, 2)), [104.0, 52.0]);
    }

    #[test]
    fn a_global_point_lands_in_the_monitors_pixels() {
        let layout = DRect {
            x: 1920.0,
            y: 0.0,
            w: 1280.0,
            h: 720.0,
        };
        assert_eq!(
            to_image((2560.0, 360.0), layout, (2560, 1440)),
            Some([1280.0, 720.0])
        );
        assert_eq!(to_image((100.0, 100.0), layout, (2560, 1440)), None);
    }
}
