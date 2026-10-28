//! The zoom loupe: a magnified pixel grid beside the pointer, shown (when it
//! is on) while choosing a target, and while editing only for the pipette or
//! while an edge of the selection is dragged (`super::loupe_wanted`).
//!
//! Ported from Umber's `umber-app/src/loupe.rs` (GPL-3.0-or-later like
//! Windup): [`place`] and [`Patch`] are its model, pure and tested, and
//! [`paint`] draws what they decide. Two things differ, both because the
//! loupe here is the square box of the owner's mockup rather than Umber's
//! round lens:
//!
//! * **Clearance is measured to the box, not to a circle.** The gap between
//!   the pointer and the nearest point of the box is the Chebyshev distance
//!   less the half-extent on each axis, and the eight candidates are placed so
//!   that gap is exactly [`metrics::LOUPE_CLEARANCE`] by construction.
//! * **The picture is the frozen screenshot, not the live screen.** Umber's
//!   loupe read the screen it was drawn on, so covering the pointer would have
//!   made it magnify itself. Here nothing is read back, so the clearance is
//!   about the hand: the box never sits on the thing it is aimed at.

use crate::theme::{Palette, metrics, shadow, text};
use egui::epaint::RectShape;
use egui::{Align2, Color32, ColorImage, FontId, Painter, Pos2, Rect, Stroke, Vec2, pos2, vec2};
use image::RgbaImage;

/// How many image pixels across the grid is: the loupe's side over the zoom,
/// rounded **up to an odd number**, so there is a middle texel and the mark
/// sits on the pixel under the pointer rather than between two. The outer
/// cells are clipped by the box, which is the safe direction: a magnifier
/// showing fewer pixels than it read, never ones it did not.
pub const CELLS: u32 = odd_ceil(metrics::LOUPE / metrics::LOUPE_ZOOM);

const fn odd_ceil(v: f32) -> u32 {
    let n = v as u32;
    let n = if (n as f32) < v { n + 1 } else { n };
    if n % 2 == 0 { n + 1 } else { n }
}

/// The whole loupe, grid and caption, in points.
pub fn size() -> Vec2 {
    vec2(metrics::LOUPE, metrics::LOUPE + metrics::LOUPE_CAPTION)
}

/// A rectangle to keep the loupe inside, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub min: Vec2,
    pub max: Vec2,
}

impl View {
    /// Whether a box of `half` extents about `at` lies wholly inside.
    fn holds(&self, at: Vec2, half: Vec2) -> bool {
        at.x - half.x >= self.min.x
            && at.y - half.y >= self.min.y
            && at.x + half.x <= self.max.x
            && at.y + half.y <= self.max.y
    }

    /// The nearest position a box of `half` may sit at, or `None` where the
    /// view is too small to hold one at all.
    fn nearest(&self, at: Vec2, half: Vec2) -> Option<Vec2> {
        let (lo, hi) = (self.min + half, self.max - half);
        (lo.x <= hi.x && lo.y <= hi.y).then(|| vec2(at.x.clamp(lo.x, hi.x), at.y.clamp(lo.y, hi.y)))
    }
}

/// The gap between `pointer` and the nearest point of a box of `half` about
/// `centre`: negative when the pointer is inside it.
pub fn clearance(centre: Vec2, pointer: Vec2, half: Vec2) -> f32 {
    let d = centre - pointer;
    (d.x.abs() - half.x).max(d.y.abs() - half.y)
}

/// Where the box's centre goes, in points, or `None` for a view too small.
///
/// **Above the pointer, then below, then to either side, then diagonally, then
/// wherever the view will take it.** Each candidate leaves exactly
/// [`metrics::LOUPE_CLEARANCE`] between the pointer and the box, so the rule
/// holds by construction; the diagonals are what a corner of the screen needs,
/// where every straight candidate fails on one axis or the other. The
/// fallback is refused where it would put the box on the pointer: a control
/// drawn where it cannot work is worse than one that is not drawn.
pub fn place(pointer: Vec2, view: View, half: Vec2) -> Option<Vec2> {
    let reach = half + Vec2::splat(metrics::LOUPE_CLEARANCE);
    for dir in [
        vec2(0.0, -1.0),
        vec2(0.0, 1.0),
        vec2(-1.0, 0.0),
        vec2(1.0, 0.0),
        vec2(1.0, 1.0),
        vec2(-1.0, 1.0),
        vec2(1.0, -1.0),
        vec2(-1.0, -1.0),
    ] {
        let at = pointer + dir * reach;
        if view.holds(at, half) {
            return Some(at);
        }
    }
    let at = view.nearest(pointer, half)?;
    (clearance(at, pointer, half) >= metrics::LOUPE_CLEARANCE).then_some(at)
}

/// A magnified neighbourhood: `size`² texels, row-major, top-left first.
///
/// A texel is `None` where there is nothing to show (off the monitor's
/// image), which the painter leaves as the loupe's own surface rather than
/// filling with black: a loupe showing a neighbourhood it had not read would
/// be a control that lies.
#[derive(Clone, Debug, PartialEq)]
pub struct Patch {
    size: u32,
    texels: Vec<Option<[u8; 3]>>,
}

impl Patch {
    /// Build one, refusing a run of texels that is not `size` squared.
    pub fn new(size: u32, texels: Vec<Option<[u8; 3]>>) -> Option<Self> {
        let want = (size as usize).checked_mul(size as usize)?;
        (size > 0 && texels.len() == want).then_some(Self { size, texels })
    }

    /// From straight-alpha RGBA, where `first` is the image pixel the top-left
    /// texel stands for and `dims` the image's size. An alpha of exactly zero
    /// and a texel outside the image are both nothing.
    pub fn from_rgba(size: u32, rgba: &[u8], first: (i32, i32), dims: (i32, i32)) -> Option<Self> {
        let want = (size as usize).checked_mul(size as usize)?;
        if rgba.len() < want * 4 {
            return None;
        }
        Self::new(
            size,
            rgba[..want * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let x = first.0 + (i % size as usize) as i32;
                    let y = first.1 + (i / size as usize) as i32;
                    let inside = x >= 0 && y >= 0 && x < dims.0 && y < dims.1;
                    (inside && c[3] != 0).then_some([c[0], c[1], c[2]])
                })
                .collect(),
        )
    }

    /// The `size`-wide block of `img` centred on pixel `centre`.
    pub fn sample(img: &RgbaImage, centre: (i32, i32), size: u32) -> Option<Self> {
        let half = (size / 2) as i32;
        let first = (centre.0 - half, centre.1 - half);
        let (w, h) = (img.width() as i32, img.height() as i32);
        let mut rgba = Vec::with_capacity((size * size * 4) as usize);
        for row in 0..size as i32 {
            for col in 0..size as i32 {
                let (x, y) = (first.0 + col, first.1 + row);
                if x >= 0 && y >= 0 && x < w && y < h {
                    rgba.extend_from_slice(&img.get_pixel(x as u32, y as u32).0);
                } else {
                    rgba.extend_from_slice(&[0, 0, 0, 0]);
                }
            }
        }
        Self::from_rgba(size, &rgba, first, (w, h))
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    /// The texel at a column and a row, or `None` for nothing there and for
    /// an index off the grid.
    pub fn at(&self, col: u32, row: u32) -> Option<[u8; 3]> {
        if col >= self.size || row >= self.size {
            return None;
        }
        self.texels[(row * self.size + col) as usize]
    }

    /// The middle column and row: the pixel under the pointer.
    pub fn middle(&self) -> u32 {
        self.size / 2
    }

    /// As a picture for a texture, nothing as transparent.
    pub fn to_image(&self) -> ColorImage {
        let n = self.size as usize;
        let mut img = ColorImage::filled([n, n], Color32::TRANSPARENT);
        for row in 0..self.size {
            for col in 0..self.size {
                if let Some([r, g, b]) = self.at(col, row) {
                    img.pixels[(row * self.size + col) as usize] = Color32::from_rgb(r, g, b);
                }
            }
        }
        img
    }
}

/// Paints the loupe at `centre` (from [`place`]): the grid from `texture`
/// (the [`Patch`] uploaded with nearest filtering), a hairline grid, the
/// crosshair on the middle texel and the pointer's position underneath.
///
/// The box is a floating thing (radius 8, `shadow::menu`, `popover` and
/// `line-popover`, §5); the crosshair and grid are marks over the user's
/// pixels, so they take derived ink, white over black, never a token (§2.6).
pub fn paint(
    painter: &Painter,
    p: &Palette,
    mode: crate::theme::Mode,
    centre: Pos2,
    texture: egui::TextureId,
    position: &str,
) {
    let outer = Rect::from_center_size(centre, size());
    let radius = metrics::RADIUS_CARD;
    painter.add(shadow::menu(mode).as_shape(outer, radius));
    painter.rect_filled(outer, radius, p.popover);

    // The grid: CELLS texels at LOUPE_ZOOM points each, centred on the box so
    // the middle texel is in the middle, and clipped by it. The texture's uv
    // is widened to the part of the grid the box shows.
    let grid = Rect::from_min_size(outer.min, Vec2::splat(metrics::LOUPE));
    let cell = metrics::LOUPE_ZOOM;
    let span = CELLS as f32 * cell;
    let inset = (span - metrics::LOUPE) / 2.0 / span;
    let uv = Rect::from_min_max(pos2(inset, inset), pos2(1.0 - inset, 1.0 - inset));
    let top = egui::CornerRadius {
        nw: radius as u8,
        ne: radius as u8,
        sw: 0,
        se: 0,
    };
    painter.add(RectShape::filled(grid, top, Color32::WHITE).with_texture(texture, uv));

    // Hairlines between texels, faint black: they read on light pixels and
    // vanish on dark ones, which is where a grid is least needed anyway.
    let grid_ink = Color32::from_black_alpha(metrics::LOUPE_GRID_ALPHA);
    let first = grid.center() - Vec2::splat(span / 2.0);
    let clip = painter.with_clip_rect(grid);
    let hairline = Stroke::new(metrics::HAIRLINE, grid_ink);
    for i in 0..=CELLS {
        let o = first + Vec2::splat(i as f32 * cell);
        clip.line_segment([pos2(o.x, grid.top()), pos2(o.x, grid.bottom())], hairline);
        clip.line_segment([pos2(grid.left(), o.y), pos2(grid.right(), o.y)], hairline);
    }

    // The crosshair: arms from the edges to the middle texel, and its box.
    // Black first and wider, white on top: one of the two reads on any pixel.
    let mid = Rect::from_center_size(grid.center(), Vec2::splat(cell));
    let c = grid.center();
    let arms = [
        [pos2(c.x, grid.top()), pos2(c.x, mid.top())],
        [pos2(c.x, mid.bottom()), pos2(c.x, grid.bottom())],
        [pos2(grid.left(), c.y), pos2(mid.left(), c.y)],
        [pos2(mid.right(), c.y), pos2(grid.right(), c.y)],
    ];
    for (width, ink) in [
        (metrics::HAIRLINE * 3.0, Color32::BLACK),
        (metrics::HAIRLINE, Color32::WHITE),
    ] {
        for arm in arms {
            clip.line_segment(arm, Stroke::new(width, ink));
        }
        clip.rect_stroke(mid, 0.0, Stroke::new(width, ink), egui::StrokeKind::Middle);
    }

    // The caption: the pointer's position, mono, text-dim.
    let caption = Rect::from_min_max(pos2(outer.left(), grid.bottom()), outer.max);
    painter.line_segment(
        [caption.left_top(), caption.right_top()],
        Stroke::new(metrics::HAIRLINE, p.line_popover),
    );
    painter.text(
        caption.center(),
        Align2::CENTER_CENTER,
        position,
        FontId::monospace(text::TINY),
        p.text_dim,
    );
    painter.rect_stroke(
        outer,
        radius,
        Stroke::new(metrics::HAIRLINE, p.line_popover),
        egui::StrokeKind::Inside,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: View = View {
        min: Vec2::new(0.0, 0.0),
        max: Vec2::new(1280.0, 800.0),
    };
    const C: f32 = metrics::LOUPE_CLEARANCE;

    fn half() -> Vec2 {
        size() / 2.0
    }

    #[test]
    fn cells_are_odd_and_cover_the_box() {
        assert_eq!(CELLS % 2, 1);
        assert!(CELLS as f32 * metrics::LOUPE_ZOOM >= metrics::LOUPE);
        assert_eq!(odd_ceil(14.0), 15);
        assert_eq!(odd_ceil(13.0), 13);
        assert_eq!(odd_ceil(12.5), 13);
    }

    #[test]
    fn it_sits_above_the_pointer_where_there_is_room() {
        let at = place(vec2(640.0, 400.0), VIEW, half()).expect("room");
        assert_eq!(at, vec2(640.0, 400.0 - half().y - C));
    }

    #[test]
    fn it_goes_below_rather_than_being_squashed_against_the_top() {
        let pointer = vec2(640.0, 8.0);
        let at = place(pointer, VIEW, half()).expect("room");
        assert_eq!(at, pointer + vec2(0.0, half().y + C));
    }

    #[test]
    fn a_corner_still_gets_a_loupe() {
        // All four straight candidates fail in a corner: above and to the left
        // there is no room for the offset, below and to the right the offset
        // fits on one axis and the box does not fit on the other.
        let pointer = vec2(6.0, 6.0);
        let at = place(pointer, VIEW, half()).expect("a corner still gets one");
        assert!(
            at.x > pointer.x && at.y > pointer.y,
            "down and to the right"
        );
        assert!((clearance(at, pointer, half()) - C).abs() < 1e-3);
        for corner in [
            vec2(0.0, 0.0),
            vec2(1280.0, 0.0),
            vec2(0.0, 800.0),
            vec2(1280.0, 800.0),
        ] {
            assert!(place(corner, VIEW, half()).is_some(), "at {corner:?}");
        }
    }

    #[test]
    fn the_loupe_never_covers_the_pointer() {
        // Swept rather than argued, over positions outside the view too (the
        // clamp's half) and over views barely larger than the box, which are
        // the ones that reach the clamp's refusal.
        let h = half();
        let views = [
            VIEW,
            View {
                min: Vec2::ZERO,
                max: 2.0 * h + Vec2::splat(6.0),
            },
            View {
                min: Vec2::ZERO,
                max: vec2(2.0 * (h.x + C) - 4.0, 400.0),
            },
            View {
                min: vec2(-90.0, -40.0),
                max: vec2(180.0, 220.0),
            },
        ];
        for view in views {
            let (lo, hi) = (view.min - Vec2::splat(200.0), view.max + Vec2::splat(200.0));
            let mut x = lo.x;
            while x <= hi.x {
                let mut y = lo.y;
                while y <= hi.y {
                    let pointer = vec2(x, y);
                    y += 7.0;
                    let Some(at) = place(pointer, view, h) else {
                        continue;
                    };
                    let clear = clearance(at, pointer, h);
                    assert!(
                        clear >= C - 1e-3,
                        "at {pointer:?} in {view:?}: {clear} of clearance"
                    );
                    assert!(view.holds(at, h), "inside the view");
                }
                x += 5.0;
            }
        }
    }

    #[test]
    fn a_pointer_outside_the_view_keeps_the_loupe_inside_it() {
        let h = half();
        let at = place(vec2(1600.0, 400.0), VIEW, h).expect("room");
        assert_eq!(at, vec2(1280.0 - h.x, 400.0));
        let at = place(vec2(-300.0, -300.0), VIEW, h).expect("room");
        assert_eq!(at, h);
    }

    #[test]
    fn a_view_too_small_draws_nothing_rather_than_something_wrong() {
        let h = half();
        let tiny = View {
            min: Vec2::ZERO,
            max: h,
        };
        assert_eq!(place(Vec2::splat(10.0), tiny, h), None);
        let snug = View {
            min: Vec2::ZERO,
            max: 2.0 * h + Vec2::splat(4.0),
        };
        assert_eq!(place(h + Vec2::splat(2.0), snug, h), None);
    }

    #[test]
    fn the_middle_texel_is_the_pointers_pixel() {
        let patch = Patch::new(CELLS, vec![Some([1, 2, 3]); (CELLS * CELLS) as usize]).expect("ok");
        assert_eq!(patch.middle(), CELLS / 2);
        assert_eq!(patch.at(patch.middle(), patch.middle()), Some([1, 2, 3]));

        let mut img = RgbaImage::from_pixel(40, 30, image::Rgba([0, 0, 0, 255]));
        img.put_pixel(17, 9, image::Rgba([200, 100, 50, 255]));
        let patch = Patch::sample(&img, (17, 9), 5).expect("ok");
        assert_eq!(patch.at(2, 2), Some([200, 100, 50]));
        assert_eq!(patch.at(1, 2), Some([0, 0, 0]));
    }

    #[test]
    fn a_patch_of_the_wrong_length_is_refused_rather_than_padded() {
        assert_eq!(Patch::new(3, vec![None; 8]), None);
        assert_eq!(Patch::new(3, vec![None; 10]), None);
        assert_eq!(Patch::new(0, vec![]), None);
        assert!(Patch::new(3, vec![None; 9]).is_some());
    }

    #[test]
    fn a_texel_off_the_grid_is_nothing_rather_than_a_panic() {
        let patch = Patch::new(2, vec![Some([9, 9, 9]); 4]).expect("ok");
        assert_eq!(patch.at(2, 0), None);
        assert_eq!(patch.at(0, 2), None);
        assert_eq!(patch.at(u32::MAX, u32::MAX), None);
    }

    #[test]
    fn transparent_pixels_are_nothing_and_faint_ones_are_a_colour() {
        let rgba = [
            10, 20, 30, 0, // nothing
            40, 50, 60, 1, // a colour, only just
            70, 80, 90, 255, //
            1, 2, 3, 128, //
        ];
        let patch = Patch::from_rgba(2, &rgba, (0, 0), (2, 2)).expect("ok");
        assert_eq!(patch.at(0, 0), None);
        assert_eq!(patch.at(1, 0), Some([40, 50, 60]));
        assert_eq!(patch.at(0, 1), Some([70, 80, 90]));
        assert_eq!(patch.at(1, 1), Some([1, 2, 3]));
    }

    #[test]
    fn texels_off_the_image_are_nothing() {
        // At the image's first pixel a 3-wide block reaches one pixel left of
        // and above it; every byte is opaque, so a mask reading alpha alone
        // would pass this and still magnify a smeared edge.
        let rgba = [255u8; 3 * 3 * 4];
        let patch = Patch::from_rgba(3, &rgba, (-1, -1), (8, 8)).expect("ok");
        for row in 0..3 {
            assert_eq!(patch.at(0, row), None, "left column, row {row}");
            assert_eq!(patch.at(row, 0), None, "top row, column {row}");
        }
        assert_eq!(patch.at(1, 1), Some([255, 255, 255]));
        let patch = Patch::from_rgba(3, &rgba, (0, 0), (2, 2)).expect("ok");
        assert_eq!(patch.at(2, 1), None);
        assert_eq!(patch.at(1, 2), None);

        // The same through `sample` at the image's corner.
        let img = RgbaImage::from_pixel(4, 4, image::Rgba([7, 7, 7, 255]));
        let patch = Patch::sample(&img, (0, 0), 3).expect("ok");
        assert_eq!(patch.at(0, 0), None);
        assert_eq!(patch.at(1, 1), Some([7, 7, 7]));
    }

    #[test]
    fn a_short_buffer_is_refused_rather_than_read_off_the_end() {
        let doc = (99, 99);
        assert_eq!(Patch::from_rgba(3, &[0u8; 35], (0, 0), doc), None);
        assert!(Patch::from_rgba(3, &[0u8; 36], (0, 0), doc).is_some());
        assert!(Patch::from_rgba(3, &[0u8; 64], (0, 0), doc).is_some());
    }
}
