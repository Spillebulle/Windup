//! Choosing a window to capture, and the highlight that glides to it.
//!
//! Where the OS reports window rectangles they come in as [`WindowRect`]s,
//! topmost first. Where it does not, [`edge_rect`] finds a rectangle from the
//! frozen screenshot instead.
//!
//! ## The edge heuristic
//!
//! Work in the pixels of the monitor under the pointer. A pixel sits on a
//! vertical edge when its luminance differs from its left neighbour by at
//! least `EDGE_STEP`, on a horizontal edge likewise against the pixel above.
//! Nothing is computed up front: edges are tested lazily, per pixel.
//!
//! 1. Scan the row and the column through the pointer outwards and note the
//!    nearest edges in each direction (adjacent edge pixels count once, so a
//!    1-px outline is one edge).
//! 2. Start with the nearest edge on every side. A side holds when edges
//!    cover at least `COVERAGE` of its length along the current rectangle. A
//!    side that fails (text, an icon, a gradient) moves to its next-nearest
//!    candidate, and the sides are checked again. A real window edge covers
//!    any part of its length, so it holds however small the rectangle is yet.
//!    The monitor's border is each side's last candidate, so a maximised or
//!    snapped window, which has no edge where it meets the border, is found.
//! 3. Reject a rectangle under `MIN_SIDE` px either way or as large as the
//!    monitor; give up (`None`) when a side runs out of candidates.
//!
//! The work is a handful of line scans, never a pass over the image.

use super::geometry::{DRect, DesktopShot};

#[derive(Clone, Debug, PartialEq)]
pub struct WindowRect {
    pub rect: DRect,
    pub title: String,
}

/// The first (topmost) window containing `at`.
pub fn window_at(rects: &[WindowRect], at: (f32, f32)) -> Option<DRect> {
    rects
        .iter()
        .find(|w| w.rect.contains(at.0, at.1))
        .map(|w| w.rect)
}

/// Luminance step (0..=255) that counts as an edge.
const EDGE_STEP: i32 = 24;
/// Share of a side that must be edge for the side to hold.
const COVERAGE: f32 = 0.8;
/// Smallest side of a window, in image pixels.
const MIN_SIDE: usize = 40;
/// Candidate edges kept per direction.
const MAX_CANDIDATES: usize = 96;

struct Luma<'a> {
    raw: &'a [u8],
    w: usize,
}

impl Luma<'_> {
    fn at(&self, x: usize, y: usize) -> i32 {
        let i = (y * self.w + x) * 4;
        (self.raw[i] as i32 * 299 + self.raw[i + 1] as i32 * 587 + self.raw[i + 2] as i32 * 114)
            / 1000
    }

    /// Edge between column `x - 1` and `x` at row `y`.
    fn v_edge(&self, x: usize, y: usize) -> bool {
        x > 0 && (self.at(x, y) - self.at(x - 1, y)).abs() >= EDGE_STEP
    }

    /// Edge between row `y - 1` and `y` at column `x`.
    fn h_edge(&self, x: usize, y: usize) -> bool {
        y > 0 && (self.at(x, y) - self.at(x, y - 1)).abs() >= EDGE_STEP
    }
}

/// Positions along a scan, nearest first, where `edge` holds; a run of
/// adjacent positions counts once, by its nearest one.
fn candidates(positions: impl Iterator<Item = usize>, edge: impl Fn(usize) -> bool) -> Vec<usize> {
    let mut out = Vec::new();
    let mut prev: Option<usize> = None;
    for p in positions {
        if !edge(p) {
            continue;
        }
        if prev.is_none_or(|q| q.abs_diff(p) > 1) {
            out.push(p);
            if out.len() == MAX_CANDIDATES {
                break;
            }
        }
        prev = Some(p);
    }
    out
}

/// Whether `edge` holds on enough of `lo..hi`. Stops as soon as the share
/// cannot be reached any more.
fn covered(lo: usize, hi: usize, edge: impl Fn(usize) -> bool) -> bool {
    let len = hi.saturating_sub(lo);
    if len == 0 {
        return false;
    }
    let allowed_misses = len - (len as f32 * COVERAGE).ceil() as usize;
    let mut misses = 0;
    for p in lo..hi {
        if !edge(p) {
            misses += 1;
            if misses > allowed_misses {
                return false;
            }
        }
    }
    true
}

/// The smallest rectangle around `at`, within its monitor, bounded on all
/// four sides by long straight luminance edges. `None` when there is none;
/// the caller then highlights the monitor.
pub fn edge_rect(shot: &DesktopShot, at: (f32, f32)) -> Option<DRect> {
    let mon = &shot.monitors[shot.monitor_at(at.0, at.1)?];
    let ppu = mon.px_per_unit();
    let (w, h) = (mon.image.width() as usize, mon.image.height() as usize);
    let px = ((at.0 - mon.layout.x) * ppu) as usize;
    let py = ((at.1 - mon.layout.y) * ppu) as usize;
    if px >= w || py >= h {
        return None;
    }
    let luma = Luma {
        raw: mon.image.as_raw(),
        w,
    };

    // A left/top boundary is the first pixel inside, a right/bottom one the
    // first pixel outside; an edge at `x` lies between `x - 1` and `x`, so
    // the edge position is the boundary either way.
    //
    // The monitor's own border is every side's last candidate, and always
    // covered: a maximised or snapped window has no edge where it meets it.
    // A rectangle that ends up as the whole monitor is still rejected below.
    let mut lefts = candidates((1..=px).rev(), |x| luma.v_edge(x, py));
    let mut rights = candidates(px + 1..w, |x| luma.v_edge(x, py));
    let mut tops = candidates((1..=py).rev(), |y| luma.h_edge(px, y));
    let mut bottoms = candidates(py + 1..h, |y| luma.h_edge(px, y));
    lefts.push(0);
    rights.push(w);
    tops.push(0);
    bottoms.push(h);
    let v_side = |x: usize, y: usize| x == 0 || x == w || luma.v_edge(x, y);
    let h_side = |x: usize, y: usize| y == 0 || y == h || luma.h_edge(x, y);

    let (mut il, mut ir, mut it, mut ib) = (0, 0, 0, 0);
    let (l, r, t, b) = loop {
        let (l, r, t, b) = (lefts[il], rights[ir], tops[it], bottoms[ib]);
        let mut ok = true;
        if !covered(t, b, |y| v_side(l, y)) {
            il += 1;
            ok = false;
        }
        if !covered(t, b, |y| v_side(r, y)) {
            ir += 1;
            ok = false;
        }
        if !covered(l, r, |x| h_side(x, t)) {
            it += 1;
            ok = false;
        }
        if !covered(l, r, |x| h_side(x, b)) {
            ib += 1;
            ok = false;
        }
        if ok {
            // Two letters of text can satisfy every side of a sliver; look
            // further out on the nearer side of whichever way is too short.
            let (short_w, short_h) = (r - l < MIN_SIDE, b - t < MIN_SIDE);
            if !short_w && !short_h {
                break (l, r, t, b);
            }
            if short_w {
                if px - l <= r - px {
                    il += 1;
                } else {
                    ir += 1;
                }
            }
            if short_h {
                if py - t <= b - py {
                    it += 1;
                } else {
                    ib += 1;
                }
            }
        }
        if il >= lefts.len() || ir >= rights.len() || it >= tops.len() || ib >= bottoms.len() {
            return None;
        }
    };

    let (rw, rh) = (r - l, b - t);
    if rw < MIN_SIDE || rh < MIN_SIDE || (rw + 4 >= w && rh + 4 >= h) {
        return None;
    }
    Some(DRect {
        x: mon.layout.x + l as f32 / ppu,
        y: mon.layout.y + t as f32 / ppu,
        w: rw as f32 / ppu,
        h: rh as f32 / ppu,
    })
}

/// How long the highlight takes to glide to a new window.
pub const TWEEN_SECS: f64 = 0.16;

/// The highlight rectangle gliding from one window to the next.
#[derive(Clone, Copy, Debug)]
pub struct Tween {
    from: DRect,
    to: DRect,
    started: f64,
}

impl Tween {
    pub fn new(at: DRect, now: f64) -> Self {
        Self {
            from: at,
            to: at,
            started: now,
        }
    }

    /// Glide on towards `to`, from wherever the highlight is right now.
    pub fn retarget(&mut self, to: DRect, now: f64) {
        self.from = self.at(now, false);
        self.to = to;
        self.started = now;
    }

    /// The rectangle at `now`; reduced motion has no glide.
    pub fn at(&self, now: f64, reduced_motion: bool) -> DRect {
        if reduced_motion {
            return self.to;
        }
        let k = ease(((now - self.started) / TWEEN_SECS).clamp(0.0, 1.0)) as f32;
        let mix = |a: f32, b: f32| a + (b - a) * k;
        DRect {
            x: mix(self.from.x, self.to.x),
            y: mix(self.from.y, self.to.y),
            w: mix(self.from.w, self.to.w),
            h: mix(self.from.h, self.to.h),
        }
    }
}

/// `cubic-bezier(.22, 1, .36, 1)`: solve x(s) = t for s, return y(s).
fn ease(t: f64) -> f64 {
    const X1: f64 = 0.22;
    const X2: f64 = 0.36;
    // One coordinate of the curve with control values a, b (ends 0 and 1).
    let bez = |a: f64, b: f64, s: f64| {
        let m = 1.0 - s;
        3.0 * m * m * s * a + 3.0 * m * s * s * b + s * s * s
    };
    let dbez = |a: f64, b: f64, s: f64| {
        let m = 1.0 - s;
        3.0 * m * m * a + 6.0 * m * s * (b - a) + 3.0 * s * s * (1.0 - b)
    };
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let mut s = t;
    for _ in 0..8 {
        let err = bez(X1, X2, s) - t;
        let d = dbez(X1, X2, s);
        if err.abs() < 1e-7 || d.abs() < 1e-6 {
            break;
        }
        s = (s - err / d).clamp(0.0, 1.0);
    }
    // Newton can stall where the slope is flat; bisect if it did not land.
    if (bez(X1, X2, s) - t).abs() > 1e-4 {
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..40 {
            s = (lo + hi) / 2.0;
            if bez(X1, X2, s) < t {
                lo = s;
            } else {
                hi = s;
            }
        }
    }
    bez(1.0, 1.0, s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::geometry::MonitorShot;
    use image::{Rgba, RgbaImage};

    fn r(x: f32, y: f32, w: f32, h: f32) -> DRect {
        DRect { x, y, w, h }
    }

    fn shot(img: RgbaImage, layout: DRect) -> DesktopShot {
        DesktopShot {
            monitors: vec![MonitorShot {
                layout,
                image: img,
                name: "t".into(),
                scale: 1.0,
            }],
        }
    }

    fn near(a: DRect, b: DRect, tol: f32) {
        assert!(
            (a.x - b.x).abs() <= tol
                && (a.y - b.y).abs() <= tol
                && (a.w - b.w).abs() <= tol
                && (a.h - b.h).abs() <= tol,
            "{a:?} vs {b:?}"
        );
    }

    fn boxed(w: u32, h: u32, bx: DRect) -> RgbaImage {
        let mut img = RgbaImage::from_pixel(w, h, Rgba([180, 180, 180, 255]));
        let (x0, y0, x1, y1) = (
            bx.x as u32,
            bx.y as u32,
            bx.right() as u32 - 1,
            bx.bottom() as u32 - 1,
        );
        for x in x0..=x1 {
            img.put_pixel(x, y0, Rgba([20, 20, 20, 255]));
            img.put_pixel(x, y1, Rgba([20, 20, 20, 255]));
        }
        for y in y0..=y1 {
            img.put_pixel(x0, y, Rgba([20, 20, 20, 255]));
            img.put_pixel(x1, y, Rgba([20, 20, 20, 255]));
        }
        img
    }

    #[test]
    fn the_topmost_window_wins() {
        let rects = [
            WindowRect {
                rect: r(10.0, 10.0, 50.0, 50.0),
                title: "top".into(),
            },
            WindowRect {
                rect: r(0.0, 0.0, 200.0, 200.0),
                title: "below".into(),
            },
        ];
        assert_eq!(window_at(&rects, (20.0, 20.0)), Some(rects[0].rect));
        assert_eq!(window_at(&rects, (100.0, 100.0)), Some(rects[1].rect));
        assert_eq!(window_at(&rects, (300.0, 100.0)), None);
    }

    #[test]
    fn edges_find_a_drawn_window() {
        let b = r(100.0, 80.0, 300.0, 200.0);
        let s = shot(boxed(800, 600, b), r(0.0, 0.0, 800.0, 600.0));
        near(edge_rect(&s, (250.0, 180.0)).unwrap(), b, 2.0);
    }

    #[test]
    fn edges_work_on_a_scaled_monitor_with_an_offset() {
        let b = r(200.0, 160.0, 600.0, 400.0); // image pixels
        let s = shot(boxed(1600, 1200, b), r(1000.0, 50.0, 800.0, 600.0));
        let want = r(1100.0, 130.0, 300.0, 200.0);
        near(edge_rect(&s, (1250.0, 230.0)).unwrap(), want, 2.0);
    }

    #[test]
    fn edges_skip_text_inside_the_window() {
        let b = r(100.0, 80.0, 300.0, 200.0);
        let mut img = boxed(800, 600, b);
        for x in 150..230 {
            for y in 150..162 {
                if (x / 2 + y / 3) % 2 == 0 {
                    img.put_pixel(x, y, Rgba([30, 30, 30, 255]));
                }
            }
        }
        let s = shot(img, r(0.0, 0.0, 800.0, 600.0));
        near(edge_rect(&s, (180.0, 155.0)).unwrap(), b, 2.0);
    }

    #[test]
    fn edges_give_none_on_a_flat_image() {
        let img = RgbaImage::from_pixel(800, 600, Rgba([120, 120, 120, 255]));
        let s = shot(img, r(0.0, 0.0, 800.0, 600.0));
        assert_eq!(edge_rect(&s, (400.0, 300.0)), None);
    }

    #[test]
    fn edges_give_none_for_a_box_that_is_the_whole_monitor() {
        let b = r(0.0, 0.0, 800.0, 600.0);
        let s = shot(boxed(800, 600, b), r(0.0, 0.0, 800.0, 600.0));
        assert_eq!(edge_rect(&s, (400.0, 300.0)), None);
    }

    #[test]
    fn edges_find_a_half_snapped_window_touching_three_borders() {
        // The window fills the left half, edge to edge; wallpaper on the right.
        let mut img = RgbaImage::from_pixel(800, 600, Rgba([180, 180, 180, 255]));
        for x in 400..800 {
            for y in 0..600 {
                img.put_pixel(x, y, Rgba([40, 40, 40, 255]));
            }
        }
        let s = shot(img, r(0.0, 0.0, 800.0, 600.0));
        near(
            edge_rect(&s, (200.0, 300.0)).unwrap(),
            r(0.0, 0.0, 400.0, 600.0),
            2.0,
        );
    }

    #[test]
    fn edges_find_a_maximised_window_below_a_top_panel() {
        let mut img = RgbaImage::from_pixel(800, 600, Rgba([180, 180, 180, 255]));
        for x in 0..800 {
            for y in 0..30 {
                img.put_pixel(x, y, Rgba([30, 30, 30, 255]));
            }
        }
        let s = shot(img, r(0.0, 0.0, 800.0, 600.0));
        near(
            edge_rect(&s, (400.0, 300.0)).unwrap(),
            r(0.0, 30.0, 800.0, 570.0),
            2.0,
        );
    }

    #[test]
    fn edges_give_none_off_every_monitor() {
        let img = RgbaImage::from_pixel(100, 100, Rgba([0, 0, 0, 255]));
        let s = shot(img, r(0.0, 0.0, 100.0, 100.0));
        assert_eq!(edge_rect(&s, (500.0, 500.0)), None);
    }

    #[test]
    fn edge_search_is_fast_on_4k() {
        let b = r(400.0, 300.0, 1600.0, 1000.0);
        let s = shot(boxed(3840, 2160, b), r(0.0, 0.0, 3840.0, 2160.0));
        let t = std::time::Instant::now();
        for _ in 0..10 {
            assert!(edge_rect(&s, (1000.0, 800.0)).is_some());
        }
        let per = t.elapsed().as_secs_f64() / 10.0;
        eprintln!("edge_rect 4K: {:.3} ms", per * 1000.0);
        assert!(per < 0.016, "{per}");
    }

    const A: DRect = DRect {
        x: 0.0,
        y: 0.0,
        w: 100.0,
        h: 100.0,
    };
    const B: DRect = DRect {
        x: 200.0,
        y: 100.0,
        w: 300.0,
        h: 50.0,
    };

    #[test]
    fn tween_ends_at_the_target_and_starts_where_it_was() {
        let mut t = Tween::new(A, 5.0);
        t.retarget(B, 10.0);
        assert_eq!(t.at(10.0, false), A);
        assert_eq!(t.at(10.0 + TWEEN_SECS, false), B);
        assert_eq!(t.at(99.0, false), B);
        let mid = t.at(10.0 + TWEEN_SECS / 2.0, false);
        assert!(mid.x > 100.0, "ease-out is past halfway at the midpoint");
        t.retarget(A, 10.0 + TWEEN_SECS / 2.0);
        near(t.at(10.0 + TWEEN_SECS / 2.0, false), mid, 1e-3);
    }

    #[test]
    fn the_curve_is_cubic_bezier_22_1_36_1() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        // Reference value of cubic-bezier(.22,1,.36,1) at t = 0.5.
        assert!((ease(0.5) - 0.9614).abs() < 2e-3, "{}", ease(0.5));
        let mut last = 0.0;
        for i in 1..=100 {
            let v = ease(i as f64 / 100.0);
            assert!(v >= last);
            last = v;
        }
    }

    #[test]
    fn reduced_motion_jumps() {
        let mut t = Tween::new(A, 0.0);
        t.retarget(B, 1.0);
        assert_eq!(t.at(1.0, true), B);
    }
}
