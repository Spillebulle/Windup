//! Where Steps' bar goes, so that it is not in the pictures.

use crate::capture::backend::BarHost;
use crate::capture::geometry::DRect;
use crate::video::place::top_centre;

/// The bar's spot, desktop units. Off the monitor the pointer is on when
/// there is another, since that is where the clicks are; `Hidden` has none.
pub fn steps_bar_slot(
    host: BarHost,
    pointer_monitor: DRect,
    monitors: &[DRect],
    bar: (f32, f32),
    top: f32,
) -> Option<DRect> {
    match host {
        BarHost::Hidden => None,
        BarHost::ExcludedWindow | BarHost::Outside => {
            let m = monitors
                .iter()
                .copied()
                .find(|m| *m != pointer_monitor)
                .unwrap_or(pointer_monitor);
            Some(top_centre(m, bar, top))
        }
    }
}

/// Whether the bar must be hidden for a picture of `shot`: it is on that
/// monitor and nothing keeps it out of the picture.
pub fn must_conceal(host: BarHost, excluded: bool, bar: Option<DRect>, shot: DRect) -> bool {
    if host == BarHost::ExcludedWindow && excluded {
        return false;
    }
    bar.is_some_and(|b| b.intersect(&shot).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32) -> DRect {
        DRect {
            x,
            y,
            w: 1920.0,
            h: 1080.0,
        }
    }

    #[test]
    fn the_steps_bar_goes_to_a_monitor_the_pointer_is_not_on() {
        let (a, b) = (r(0.0, 0.0), r(1920.0, 0.0));
        for host in [BarHost::Outside, BarHost::ExcludedWindow] {
            let s = steps_bar_slot(host, a, &[a, b], (400.0, 60.0), 24.0).unwrap();
            assert!(s.x >= b.x && s.right() <= b.right());
            assert_eq!(s.y, b.y + 24.0);
            let s = steps_bar_slot(host, b, &[a, b], (400.0, 60.0), 24.0).unwrap();
            assert!(s.right() <= a.right());
        }
        assert_eq!(
            steps_bar_slot(BarHost::Hidden, a, &[a, b], (1.0, 1.0), 0.0),
            None
        );
    }

    #[test]
    fn must_conceal_on_one_monitor_for_every_shot() {
        let a = r(0.0, 0.0);
        let slot = steps_bar_slot(BarHost::Outside, a, &[a], (400.0, 60.0), 24.0);
        assert_eq!(slot.map(|s| a.intersect(&s).is_some()), Some(true));
        assert!(must_conceal(BarHost::Outside, false, slot, a));
    }

    #[test]
    fn must_conceal_not_when_the_shot_is_of_the_other_monitor() {
        let (a, b) = (r(0.0, 0.0), r(1920.0, 0.0));
        let slot = steps_bar_slot(BarHost::Outside, a, &[a, b], (400.0, 60.0), 24.0);
        assert!(!must_conceal(BarHost::Outside, false, slot, a));
        assert!(must_conceal(BarHost::Outside, false, slot, b));
    }

    #[test]
    fn must_conceal_never_on_windows_with_exclusion_or_with_no_bar() {
        let a = r(0.0, 0.0);
        let slot = steps_bar_slot(BarHost::ExcludedWindow, a, &[a], (400.0, 60.0), 24.0);
        assert!(!must_conceal(BarHost::ExcludedWindow, true, slot, a));
        assert!(must_conceal(BarHost::ExcludedWindow, false, slot, a));
        assert!(!must_conceal(BarHost::Hidden, false, None, a));
    }
}
