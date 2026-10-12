//! Where the clipboard popup goes, given the cursor and the usable area of the
//! screen it is on. Every platform calls this; only how the cursor and the
//! area are found differs.
//!
//! Everything is in logical points.

/// A rectangle by its edges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// Distance kept from the edges of the area.
pub const MARGIN: f32 = 8.0;
/// Offset from the cursor to the popup's near corner.
pub const GAP_X: f32 = 4.0;
pub const GAP_Y: f32 = 12.0;

/// The popup's top-left corner. It opens below and to the right of the
/// cursor, flips to the left or above when it would overflow, and is never
/// placed closer than [`MARGIN`] to the left or top edge.
pub fn place(cursor: (f32, f32), area: Area, size: (f32, f32)) -> (f32, f32) {
    let (cx, cy) = cursor;
    let (w, h) = size;

    let mut x = cx + GAP_X;
    if x + w > area.right - MARGIN {
        x = (cx - w - GAP_X).max(area.left + MARGIN);
    }
    if x < area.left + MARGIN {
        x = area.left + MARGIN;
    }

    let mut y = cy + GAP_Y;
    if y + h > area.bottom - MARGIN {
        y = cy - h - GAP_Y;
    }
    if y < area.top + MARGIN {
        y = area.top + MARGIN;
    }

    (x, y)
}

/// The popup centred in the area, for when the cursor cannot be known.
pub fn centre(area: Area, size: (f32, f32)) -> (f32, f32) {
    let x = area.left + ((area.right - area.left) - size.0) / 2.0;
    let y = area.top + ((area.bottom - area.top) - size.1) / 2.0;
    (x.max(area.left), y.max(area.top))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Area = Area {
        left: 0.0,
        top: 0.0,
        right: 1920.0,
        bottom: 1080.0,
    };
    const SIZE: (f32, f32) = (380.0, 480.0);

    #[test]
    fn opens_below_right_of_cursor() {
        assert_eq!(place((100.0, 100.0), SCREEN, SIZE), (104.0, 112.0));
    }

    #[test]
    fn flips_left_at_right_edge() {
        let (x, _) = place((1900.0, 100.0), SCREEN, SIZE);
        assert_eq!(x, 1900.0 - 380.0 - GAP_X);
    }

    #[test]
    fn flips_up_at_bottom_edge() {
        let (_, y) = place((100.0, 1000.0), SCREEN, SIZE);
        assert_eq!(y, 1000.0 - 480.0 - GAP_Y);
    }

    #[test]
    fn never_past_the_top_left_margin() {
        let tiny = Area {
            left: 0.0,
            top: 0.0,
            right: 300.0,
            bottom: 300.0,
        };
        assert_eq!(place((150.0, 150.0), tiny, SIZE), (MARGIN, MARGIN));
    }

    #[test]
    fn respects_an_offset_work_area() {
        // A second monitor to the left of the primary one, with a top panel.
        let area = Area {
            left: -1920.0,
            top: 32.0,
            right: 0.0,
            bottom: 1080.0,
        };
        assert_eq!(place((-1910.0, 33.0), area, SIZE), (-1906.0, 45.0));
    }

    #[test]
    fn centre_is_centred() {
        assert_eq!(centre(SCREEN, SIZE), (770.0, 300.0));
    }
}
