//! Where the recording bar goes: beside the recorded area, never over it,
//! because the bar must not be in the recording.

use crate::capture::geometry::DRect;

/// The bar's rectangle (`bar` is width and height, `gap` the space kept from
/// the area), or `None` when no slot is free. In order: centred above `rect`
/// on its monitor, centred below it, the top centre of another monitor.
/// Every candidate lies wholly on one monitor and does not cover `rect`
/// (touching edges is fine). Desktop units throughout.
pub fn place_bar(rect: DRect, monitors: &[DRect], bar: (f32, f32), gap: f32) -> Option<DRect> {
    let (w, h) = bar;
    let cx = rect.x + rect.w / 2.0;
    let fits = |m: &DRect, c: DRect| {
        c.x >= m.x
            && c.y >= m.y
            && c.right() <= m.right()
            && c.bottom() <= m.bottom()
            && c.intersect(&rect).is_none()
    };
    let centred = |m: &DRect, y: f32| DRect {
        x: (cx - w / 2.0).clamp(m.x, (m.right() - w).max(m.x)),
        y,
        w,
        h,
    };

    // The monitor holding a point of the area's edge, else the one it
    // overlaps most.
    let holding = |x: f32, y: f32| {
        monitors.iter().find(|m| m.contains(x, y)).or_else(|| {
            monitors
                .iter()
                .filter_map(|m| m.intersect(&rect).map(|o| (m, o.w * o.h)))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(m, _)| m)
        })
    };

    if let Some(m) = holding(cx, rect.y) {
        let c = centred(m, rect.y - gap - h);
        if fits(m, c) {
            return Some(c);
        }
    }
    if let Some(m) = holding(cx, rect.bottom() - 0.5) {
        let c = centred(m, rect.bottom() + gap);
        if fits(m, c) {
            return Some(c);
        }
    }
    monitors
        .iter()
        .filter(|m| m.intersect(&rect).is_none())
        .find_map(|m| {
            // The monitor's own centre, not the area's.
            let c = DRect {
                x: m.x + (m.w - w) / 2.0,
                y: m.y + gap,
                w,
                h,
            };
            fits(m, c).then_some(c)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> DRect {
        DRect { x, y, w, h }
    }
    const BAR: (f32, f32) = (300.0, 40.0);
    const GAP: f32 = 8.0;

    #[test]
    fn an_area_in_the_middle_gets_the_slot_above() {
        let m = r(0.0, 0.0, 1920.0, 1080.0);
        let a = r(600.0, 400.0, 400.0, 300.0);
        let b = place_bar(a, &[m], BAR, GAP).unwrap();
        assert_eq!(b, r(650.0, 352.0, 300.0, 40.0));
    }

    #[test]
    fn an_area_touching_the_top_gets_the_slot_below() {
        let m = r(0.0, 0.0, 1920.0, 1080.0);
        let a = r(600.0, 0.0, 400.0, 300.0);
        let b = place_bar(a, &[m], BAR, GAP).unwrap();
        assert_eq!(b, r(650.0, 308.0, 300.0, 40.0));
    }

    #[test]
    fn full_screen_of_the_left_monitor_goes_to_the_right_monitors_top_centre() {
        let l = r(0.0, 0.0, 1920.0, 1080.0);
        let rt = r(1920.0, 0.0, 1920.0, 1080.0);
        let b = place_bar(l, &[l, rt], BAR, GAP).unwrap();
        assert_eq!(b, r(1920.0 + 810.0, 8.0, 300.0, 40.0));
    }

    #[test]
    fn full_screen_of_the_only_monitor_has_no_slot() {
        let m = r(0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(place_bar(m, &[m], BAR, GAP), None);
    }

    #[test]
    fn an_area_across_two_monitors_gets_a_slot_on_one() {
        let l = r(0.0, 0.0, 1920.0, 1080.0);
        let rt = r(1920.0, 0.0, 1920.0, 1080.0);
        let a = r(1700.0, 400.0, 500.0, 300.0);
        let b = place_bar(a, &[l, rt], BAR, GAP).unwrap();
        // The area's centre (1950) is on the right monitor, so the bar is
        // clamped to its left edge there.
        assert_eq!(b, r(1920.0, 352.0, 300.0, 40.0));
    }

    #[test]
    fn the_slot_is_kept_inside_the_monitor_sideways() {
        let m = r(0.0, 0.0, 1920.0, 1080.0);
        let a = r(0.0, 400.0, 100.0, 100.0);
        let b = place_bar(a, &[m], BAR, GAP).unwrap();
        assert_eq!(b.x, 0.0);
    }

    #[test]
    fn monitors_at_negative_coordinates_place_exactly() {
        let l = r(-1920.0, 0.0, 1920.0, 1080.0);
        let m = r(0.0, 0.0, 1920.0, 1080.0);
        let a = r(-1000.0, 400.0, 400.0, 300.0);
        assert_eq!(
            place_bar(a, &[l, m], BAR, GAP),
            Some(r(-950.0, 352.0, 300.0, 40.0))
        );
        let up = r(0.0, -1080.0, 1920.0, 1080.0);
        assert_eq!(
            place_bar(m, &[m, up], BAR, GAP),
            Some(r(810.0, -1072.0, 300.0, 40.0))
        );
    }

    #[test]
    fn no_room_above_or_below_goes_to_the_second_monitor() {
        let l = r(0.0, 0.0, 1920.0, 1080.0);
        let rt = r(1920.0, 0.0, 1920.0, 1080.0);
        let a = r(100.0, 0.0, 500.0, 1080.0);
        assert_eq!(
            place_bar(a, &[l, rt], BAR, GAP),
            Some(r(2730.0, 8.0, 300.0, 40.0))
        );
    }

    #[test]
    fn a_bar_wider_than_every_monitor_has_no_slot() {
        let m = r(0.0, 0.0, 1920.0, 1080.0);
        let a = r(600.0, 400.0, 400.0, 300.0);
        assert_eq!(place_bar(a, &[m], (2000.0, 40.0), GAP), None);
    }

    #[test]
    fn an_area_over_a_gap_uses_the_monitor_it_overlaps_most() {
        let l = r(0.0, 0.0, 1000.0, 1080.0);
        let rt = r(1100.0, 0.0, 1000.0, 1080.0);
        // Its top centre (x = 1000) is on neither monitor; 150 wide of it
        // is on the left one and 50 on the right.
        let a = r(850.0, 400.0, 300.0, 300.0);
        assert_eq!(
            place_bar(a, &[l, rt], BAR, GAP),
            Some(r(700.0, 352.0, 300.0, 40.0))
        );
    }
}
