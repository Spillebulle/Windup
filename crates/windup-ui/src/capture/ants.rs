//! The marching-ants outline: ported from Umber's selection outline, which is
//! the same dark-under-light dashed line on a surface of arbitrary pixels.

use crate::theme::{Palette, metrics};
use egui::{Painter, Pos2, Shape, Stroke};

/// The outline's width, in points, on a display at `pixels_per_point`.
///
/// **A whole number of device pixels, never one point.** egui already snaps an
/// axis-aligned line's *position* to the pixel grid — `tessellate_line_segment`
/// does it, and the dashes are `Shape::LineSegment`s so they get it too — but it
/// can only snap what it is given, and a one-point stroke on a display at 150%
/// is one and a half device pixels. epaint draws a line that wide as a ridge
/// whose fully opaque core is half a pixel across, straddling a pixel boundary,
/// so no pixel centre is ever fully covered: the ants come out two device pixels
/// wide at three quarters opacity each.
///
/// **It bites at 150% and at 175% and nowhere else**, which is worth being exact
/// about rather than saying "any fractional scale". The core is under-covered
/// only where the nearest whole number of device pixels is *even* while the
/// scale is not, and that is the interval from 150% up to 200%; 125% rounds to
/// one, lands on a pixel centre and was already crisp. So it is invisible at
/// 100% and 200%, which are the two scales anybody developing would be at.
/// Measured over white paper:
/// the darkest pixel of the dark half is 0 at 100%, 125% and 200%, and **64** at
/// 150%, where the accent reads 160 against its own 192.
///
/// Rounding the *width* to whole device pixels puts the opaque core back on a
/// covered pixel at every scale, because epaint then rounds the position to a
/// centre for an odd count and to a boundary for an even one, and the two agree.
/// It changes nothing at 100% or 200%, where one point already is a whole number
/// of pixels; at 125% it narrows 1.25 device pixels to 1, dropping a faint
/// fringe rather than fixing a softness.
///
/// The caller asks the *context* for `pixels_per_point` on the frame it draws,
/// not a value it stored last frame: a marching outline a frame behind the scale
/// is exactly the soft line this exists to prevent, on the frame somebody drags
/// the overlay onto their second monitor.
///
/// **One thing this does not reach.** It only helps a line egui *snapped*, which
/// is one that is axis-aligned: `tessellate_line_segment` tests `a.x == b.x` and
/// `a.y == b.y` and does nothing otherwise, so a diagonal edge, or the outline
/// of a window with rounded corners, is as soft as it ever was.
pub fn ant_width(pixels_per_point: f32) -> f32 {
    let ppp = pixels_per_point.max(1e-3);
    ppp.round().max(1.0) / ppp
}

/// Draws `ring` as marching ants: a solid underlay, then the accent dashes
/// travelling along it. `closed` joins the last point back to the first.
///
/// Two passes, underlay then accent, so the outline reads over both a white
/// screenshot and a black one. Neither colour is a literal, and the under-pass
/// is [`Palette::accent_underlay`] rather than `backdrop`: the surface under
/// the overlay is whatever is on the user's screen, which no theme token
/// describes. **Only the accent dashes move.** The line under them stays solid,
/// so the pair still reads on any pixels at every instant of the animation
/// rather than only when a dash happens to be over a dark one.
///
/// `time` is egui's own clock (`ctx.input(|i| i.time)`), so the ants keep step
/// with the frames actually painted rather than with how many of them there
/// were: a dropped frame slides the pattern further, it does not make it fall
/// behind. `ppp` is `ctx.pixels_per_point()`, see [`ant_width`].
///
/// This does not request repaints. The caller does, once per
/// [`metrics::ANT_FRAME_MS`], and only while ants are on screen: marching at the
/// display's rate would be a frame every 16 ms for as long as the overlay is up.
/// Below about ten frames a second the dashes hop rather than slide, and above
/// about twenty nothing is gained that anybody can see in a four-point dash.
pub fn ants(painter: &Painter, ring: &[Pos2], closed: bool, p: &Palette, time: f64, ppp: f32) {
    if ring.len() < 2 {
        return;
    }

    // The offset counts *backwards* through the period so it stays positive:
    // `dashes_from_line` starts its walk at `dash_offset` along the path, so a
    // negative one would place the first dash before the path begins and draw it
    // hanging off the end.
    //
    // Wrapped in f64 and only then narrowed: egui's clock counts from when the
    // application started, and an f32 holding a day's worth of seconds has steps
    // coarser than the dash is long, so the ants would end a long session
    // hopping between two positions.
    let period = f64::from(2.0 * metrics::ANT_DASH);
    let travelled = time * f64::from(metrics::ANT_SPEED);
    let phase = (period - travelled.rem_euclid(period)) as f32;

    let width = ant_width(ppp);
    let mut points = ring.to_vec();
    if closed {
        points.push(ring[0]);
    }

    // Segment by segment: the underlay is solid, so there is no dash state to
    // carry across joins.
    let underlay = Stroke::new(width, p.accent_underlay());
    for pair in points.windows(2) {
        painter.line_segment([pair[0], pair[1]], underlay);
    }

    let mut dashes = Vec::new();
    Shape::dashed_line_many_with_offset(
        &points,
        Stroke::new(width, p.accent),
        &[metrics::ANT_DASH],
        &[metrics::ANT_DASH],
        phase,
        &mut dashes,
    );
    painter.extend(dashes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ant_width_is_whole_device_pixels() {
        for ppp in [1.0f32, 1.25, 1.5, 1.75, 2.0] {
            let device = ant_width(ppp) * ppp;
            assert_eq!(device.fract(), 0.0, "ppp {ppp}: {device} device pixels");
            assert!(device >= 1.0, "ppp {ppp}");
        }
    }
}
