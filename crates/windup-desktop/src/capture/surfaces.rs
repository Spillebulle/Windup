//! Which compositor output shows which monitor of a Wayland shot, and the
//! sizes of the overlay surface on it.
//!
//! On Wayland the shot's desktop units are the compositor's logical
//! coordinates (every back end lays its monitors out from `xdg_output`), and
//! an overlay surface covering an output is that output's logical size, so a
//! point on the surface is a desktop position once the output's logical
//! origin is added. The view's `Mapping` does that from the surface rect and
//! the monitor's layout; this module only pairs monitors with outputs.

use windup_core::capture::DRect;

/// An output as the Wayland thread knows it: its name and logical rectangle.
#[derive(Clone, Debug, PartialEq)]
pub struct Output<'a> {
    pub name: &'a str,
    pub layout: DRect,
}

/// For each monitor of the shot, the output that shows it: the one with the
/// same name, else the one with the same logical rectangle. No output is
/// used twice; `None` when nothing matches (the output went away).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn match_outputs(monitors: &[Output], outputs: &[Output]) -> Vec<Option<usize>> {
    let mut taken = vec![false; outputs.len()];
    let mut found = vec![None; monitors.len()];
    // Names first, for every monitor, so a rectangle never takes an output
    // that a later monitor would have found by name.
    for (m, slot) in monitors.iter().zip(found.iter_mut()) {
        if m.name.is_empty() {
            continue;
        }
        if let Some(i) = (0..outputs.len()).find(|&i| !taken[i] && outputs[i].name == m.name) {
            taken[i] = true;
            *slot = Some(i);
        }
    }
    for (m, slot) in monitors.iter().zip(found.iter_mut()) {
        if slot.is_some() {
            continue;
        }
        let named_elsewhere = !m.name.is_empty() && outputs.iter().any(|o| o.name == m.name);
        if named_elsewhere {
            continue;
        }
        if let Some(i) = (0..outputs.len()).find(|&i| !taken[i] && outputs[i].layout == m.layout) {
            taken[i] = true;
            *slot = Some(i);
        }
    }
    found
}

/// The surface's logical size for a monitor's layout, in whole units.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn logical_size(layout: DRect) -> (u32, u32) {
    (
        layout.w.round().max(1.0) as u32,
        layout.h.round().max(1.0) as u32,
    )
}

/// The buffer's size in pixels for a logical size at `scale`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn buffer_size(logical: (u32, u32), scale: f64) -> (u32, u32) {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    (
        (logical.0 as f64 * scale).round().max(1.0) as u32,
        (logical.1 as f64 * scale).round().max(1.0) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> DRect {
        DRect { x, y, w, h }
    }

    fn out(name: &str, layout: DRect) -> Output<'_> {
        Output { name, layout }
    }

    #[test]
    fn names_pair_monitors_with_outputs_in_any_order() {
        let monitors = [
            out("DP-1", rect(-1280.0, 80.0, 1280.0, 720.0)),
            out("eDP-1", rect(0.0, 0.0, 1440.0, 900.0)),
        ];
        let outputs = [
            out("eDP-1", rect(0.0, 0.0, 1440.0, 900.0)),
            out("DP-1", rect(-1280.0, 80.0, 1280.0, 720.0)),
        ];
        assert_eq!(match_outputs(&monitors, &outputs), vec![Some(1), Some(0)]);
    }

    #[test]
    fn without_a_name_the_same_rectangle_pairs_them() {
        // The portal's split names nothing the compositor knows.
        let monitors = [
            out("", rect(1920.0, 0.0, 1280.0, 1024.0)),
            out("", rect(0.0, 0.0, 1920.0, 1080.0)),
        ];
        let outputs = [
            out("HDMI-A-1", rect(0.0, 0.0, 1920.0, 1080.0)),
            out("DP-2", rect(1920.0, 0.0, 1280.0, 1024.0)),
        ];
        assert_eq!(match_outputs(&monitors, &outputs), vec![Some(1), Some(0)]);
    }

    #[test]
    fn a_gone_output_is_none_and_no_output_serves_twice() {
        let monitors = [
            out("A", rect(0.0, 0.0, 100.0, 100.0)),
            out("A", rect(0.0, 0.0, 100.0, 100.0)),
            out("B", rect(100.0, 0.0, 100.0, 100.0)),
        ];
        let outputs = [out("A", rect(0.0, 0.0, 100.0, 100.0))];
        assert_eq!(
            match_outputs(&monitors, &outputs),
            vec![Some(0), None, None]
        );
    }

    #[test]
    fn a_name_wins_over_a_rectangle_another_output_has() {
        // The output moved since the shot: the name still finds it, and the
        // output now at the old place is not taken instead.
        let monitors = [out("A", rect(0.0, 0.0, 100.0, 100.0))];
        let outputs = [
            out("B", rect(0.0, 0.0, 100.0, 100.0)),
            out("A", rect(100.0, 0.0, 100.0, 100.0)),
        ];
        assert_eq!(match_outputs(&monitors, &outputs), vec![Some(1)]);
    }

    #[test]
    fn sizes_round_and_never_reach_zero() {
        assert_eq!(
            logical_size(rect(-1280.0, 80.0, 1280.0, 720.0)),
            (1280, 720)
        );
        assert_eq!(logical_size(rect(0.0, 0.0, 1706.6, 959.5)), (1707, 960));
        assert_eq!(logical_size(rect(0.0, 0.0, 0.0, 0.2)), (1, 1));
        assert_eq!(buffer_size((1280, 720), 1.0), (1280, 720));
        assert_eq!(buffer_size((1707, 960), 1.5), (2561, 1440));
        assert_eq!(buffer_size((1280, 800), 2.0), (2560, 1600));
        assert_eq!(buffer_size((10, 10), 0.0), (10, 10));
    }
}
