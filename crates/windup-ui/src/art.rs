//! The mark as pixels: an accent rounded square with the Lucide wrench on it.
//!
//! One drawing for every place the mark is a picture rather than painted
//! geometry: the window icon (redrawn whenever the accent changes), and the
//! `.ico` and installer bitmaps that `examples/make-art.rs` writes from the
//! default palette. The settings rail paints the same mark with `Painter`
//! instead, because there it is drawn at UI scale every frame.

use crate::Icon;
use crate::lucide;
use crate::theme::Palette;
use egui::Color32;

/// Straight (not premultiplied) RGBA, row-major.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; (width * height * 4) as usize],
        }
    }

    pub fn filled(width: u32, height: u32, colour: Color32) -> Self {
        let mut image = Self::new(width, height);
        for px in image.pixels.as_chunks_mut::<4>().0 {
            *px = [colour.r(), colour.g(), colour.b(), 255];
        }
        image
    }

    /// Composite `other` over this image with its top-left corner at `(x, y)`.
    pub fn blit(&mut self, other: &Image, x: i32, y: i32) {
        for oy in 0..other.height as i32 {
            for ox in 0..other.width as i32 {
                let (tx, ty) = (x + ox, y + oy);
                if tx < 0 || ty < 0 || tx >= self.width as i32 || ty >= self.height as i32 {
                    continue;
                }
                let src = ((oy as u32 * other.width + ox as u32) * 4) as usize;
                let dst = ((ty as u32 * self.width + tx as u32) * 4) as usize;
                let a = other.pixels[src + 3] as f32 / 255.0;
                if a <= 0.0 {
                    continue;
                }
                let da = self.pixels[dst + 3] as f32 / 255.0;
                let out_a = a + da * (1.0 - a);
                for c in 0..3 {
                    let s = other.pixels[src + c] as f32;
                    let d = self.pixels[dst + c] as f32;
                    self.pixels[dst + c] =
                        ((s * a + d * da * (1.0 - a)) / out_a.max(1e-6)).round() as u8;
                }
                self.pixels[dst + 3] = (out_a * 255.0).round() as u8;
            }
        }
    }
}

/// Transparent margin around the square, as a fraction of the side.
const INSET: f32 = 4.0 / 64.0;
/// Corner radius, as a fraction of the side.
const CORNER: f32 = 12.0 / 64.0;
/// The wrench's 24-unit Lucide box, as a fraction of the side.
const GLYPH: f32 = 36.0 / 64.0;
/// Lucide's stroke is 2 units, so its half-width is 1.
const STROKE_HALF: f32 = 1.0;
/// The thinnest half-stroke in pixels. At 16 px Lucide's own width comes out
/// under a pixel and the wrench dissolves into the square.
const MIN_STROKE_HALF_PX: f32 = 0.7;

/// The mark at `side` pixels square.
pub fn mark(side: u32, accent: Color32, ink: Color32) -> Image {
    let mut image = Image::new(side, side);
    let s = side as f32;
    let outlines = lucide::flatten(Icon::Wrench.nodes());

    let glyph_px = s * GLYPH;
    let scale = glyph_px / 24.0;
    let offset = (s - glyph_px) * 0.5;
    let half_w = (s - 2.0 * s * INSET) * 0.5;
    let corner = s * CORNER;
    let centre = s * 0.5;
    let stroke_px = (STROKE_HALF * scale).max(MIN_STROKE_HALF_PX);

    for y in 0..side {
        for x in 0..side {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;

            let rx = (px - centre).abs() - (half_w - corner);
            let ry = (py - centre).abs() - (half_w - corner);
            let outside = ((rx.max(0.0).powi(2) + ry.max(0.0).powi(2)).sqrt() - corner)
                .max(rx.max(ry).min(0.0) - corner);
            let square = (0.5 - outside).clamp(0.0, 1.0);
            if square <= 0.0 {
                continue;
            }

            let bx = (px - offset) / scale;
            let by = (py - offset) / scale;
            let mut nearest = f32::MAX;
            for outline in &outlines {
                let pts = &outline.points;
                let n = pts.len();
                if n < 2 {
                    continue;
                }
                for i in 0..n {
                    if i + 1 == n && !outline.closed {
                        break;
                    }
                    let a = pts[i];
                    let b = pts[(i + 1) % n];
                    nearest = nearest.min(distance_to_segment((bx, by), (a.x, a.y), (b.x, b.y)));
                }
            }
            let wrench = (0.5 - (nearest * scale - stroke_px)).clamp(0.0, 1.0);

            let mix = |bg: u8, fg: u8| (bg as f32 * (1.0 - wrench) + fg as f32 * wrench).round();
            let idx = ((y * side + x) * 4) as usize;
            image.pixels[idx] = mix(accent.r(), ink.r()) as u8;
            image.pixels[idx + 1] = mix(accent.g(), ink.g()) as u8;
            image.pixels[idx + 2] = mix(accent.b(), ink.b()) as u8;
            image.pixels[idx + 3] = (square * 255.0).round() as u8;
        }
    }
    image
}

/// The mark for a window's title bar and taskbar button, in `p`'s accent.
pub fn window_icon(p: &Palette) -> egui::IconData {
    let image = mark(64, p.accent, p.accent_ink);
    egui::IconData {
        rgba: image.pixels,
        width: image.width,
        height: image.height,
    }
}

fn distance_to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let ab = (b.0 - a.0, b.1 - a.1);
    let ap = (p.0 - a.0, p.1 - a.1);
    let len_sq = ab.0 * ab.0 + ab.1 * ab.1;
    if len_sq < 1e-6 {
        return (ap.0 * ap.0 + ap.1 * ap.1).sqrt();
    }
    let t = ((ap.0 * ab.0 + ap.1 * ab.1) / len_sq).clamp(0.0, 1.0);
    let dx = p.0 - (a.0 + t * ab.0);
    let dy = p.1 - (a.1 + t * ab.1);
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The corners are transparent and the middle of the square is opaque, at
    /// every size the `.ico` carries.
    #[test]
    fn the_mark_is_a_rounded_square_at_every_icon_size() {
        let p = Palette::dark(28.0);
        for side in [16, 32, 48, 64, 128, 256] {
            let image = mark(side, p.accent, p.accent_ink);
            assert_eq!(image.pixels.len(), (side * side * 4) as usize);
            assert_eq!(image.pixels[3], 0, "corner of {side} px is not clear");
            let mid = ((side / 2 * side + side / 2) * 4 + 3) as usize;
            assert_eq!(image.pixels[mid], 255, "middle of {side} px is not opaque");
        }
    }

    /// Some pixels are the ink: the wrench did not dissolve, even at 16 px.
    #[test]
    fn the_wrench_survives_the_smallest_size() {
        let p = Palette::dark(28.0);
        let image = mark(16, p.accent, p.accent_ink);
        let inky = image
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[3] == 255 && px[0] == p.accent_ink.r() && px[1] == p.accent_ink.g())
            .count();
        assert!(inky >= 4, "only {inky} ink pixels at 16 px");
    }
}
