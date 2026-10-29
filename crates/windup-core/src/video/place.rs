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
            let c = centred(m, m.y + gap);
            // The centre is the monitor's own, not the area's.
            let c = DRect {
                x: m.x + (m.w - w) / 2.0,
                ..c
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
        assert!([l, rt].iter().any(|m| {
            b.x >= m.x && b.right() <= m.right() && b.y >= m.y && b.bottom() <= m.bottom()
        }));
        assert!(b.intersect(&a).is_none());
    }

    #[test]
    fn the_slot_is_kept_inside_the_monitor_sideways() {
        let m = r(0.0, 0.0, 1920.0, 1080.0);
        let a = r(0.0, 400.0, 100.0, 100.0);
        let b = place_bar(a, &[m], BAR, GAP).unwrap();
        assert_eq!(b.x, 0.0);
    }
}
