//! Turning the frozen desktop and its annotations into the final picture.
//!
//! Blur and mosaic read the composed original, never earlier annotations, so
//! a region hides what is on the screen and not what was drawn over it.
//! Strokes are anti-aliased by tiny-skia, which works in premultiplied alpha;
//! the conversion in and out keeps transparent gaps fully transparent.

use image::{Rgba, RgbaImage};
use tiny_skia::{
    ColorU8, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Rect, Stroke,
    Transform,
};

use super::annotate::{Annotation, Doc, Shape};
use super::geometry::{DRect, DesktopShot};

/// `area` (x, y, w, h) clipped to the image; `None` when nothing is left.
fn clip_area(img: &RgbaImage, area: (u32, u32, u32, u32)) -> Option<(u32, u32, u32, u32)> {
    let (x, y, w, h) = area;
    let x1 = x.saturating_add(w).min(img.width());
    let y1 = y.saturating_add(h).min(img.height());
    (x < x1 && y < y1).then(|| (x, y, x1 - x, y1 - y))
}

/// One box-blur pass over a line: each sample becomes the mean of the window
/// `i - r ..= i + r`, with the edge sample repeated beyond the ends.
fn box_line(line: &mut [[u32; 4]], r: usize) {
    let n = line.len() as isize;
    let src = line.to_vec();
    let at = |i: isize| src[i.clamp(0, n - 1) as usize];
    let r = r as isize;
    let width = (2 * r + 1) as u32;
    let mut sum = [0u32; 4];
    for i in -r..=r {
        let p = at(i);
        for c in 0..4 {
            sum[c] += p[c];
        }
    }
    for (i, out) in line.iter_mut().enumerate() {
        for c in 0..4 {
            out[c] = (sum[c] + width / 2) / width;
        }
        let (add, sub) = (at(i as isize + r + 1), at(i as isize - r));
        for c in 0..4 {
            sum[c] = sum[c] + add[c] - sub[c];
        }
    }
}

/// Box-blurs `area` (x, y, w, h in pixels, clipped to the image) with three passes.
pub fn blur_region(img: &mut RgbaImage, area: (u32, u32, u32, u32), radius: u32) {
    let Some((x0, y0, w, h)) = clip_area(img, area) else {
        return;
    };
    if radius == 0 {
        return;
    }
    let r = radius as usize;
    let (w, h) = (w as usize, h as usize);
    let mut buf: Vec<[u32; 4]> = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let p = img.get_pixel(x0 + x as u32, y0 + y as u32).0;
            buf.push(p.map(u32::from));
        }
    }
    for _ in 0..3 {
        for row in buf.chunks_mut(w) {
            box_line(row, r);
        }
        let mut col = vec![[0u32; 4]; h];
        for x in 0..w {
            for y in 0..h {
                col[y] = buf[y * w + x];
            }
            box_line(&mut col, r);
            for y in 0..h {
                buf[y * w + x] = col[y];
            }
        }
    }
    for y in 0..h {
        for x in 0..w {
            let p = buf[y * w + x].map(|v| v.min(255) as u8);
            img.put_pixel(x0 + x as u32, y0 + y as u32, Rgba(p));
        }
    }
}

/// Replaces each `block` x `block` cell of `area` with its average colour.
pub fn mosaic_region(img: &mut RgbaImage, area: (u32, u32, u32, u32), block: u32) {
    let Some((x0, y0, w, h)) = clip_area(img, area) else {
        return;
    };
    let block = block.max(1);
    for by in (0..h).step_by(block as usize) {
        for bx in (0..w).step_by(block as usize) {
            let (cw, ch) = (block.min(w - bx), block.min(h - by));
            let mut sum = [0u64; 4];
            for y in 0..ch {
                for x in 0..cw {
                    let p = img.get_pixel(x0 + bx + x, y0 + by + y).0;
                    for c in 0..4 {
                        sum[c] += u64::from(p[c]);
                    }
                }
            }
            let n = u64::from(cw * ch);
            let avg = Rgba(sum.map(|s| ((s + n / 2) / n) as u8));
            for y in 0..ch {
                for x in 0..cw {
                    img.put_pixel(x0 + bx + x, y0 + by + y, avg);
                }
            }
        }
    }
}

/// A size in desktop units as whole output pixels, at least one.
fn px(size: f32, scale: f32) -> u32 {
    (size * scale).round().max(1.0) as u32
}

/// The pixel box of `r` (desktop units) in an output whose origin is
/// `origin`, clipped to `dims`; `None` when it misses the picture.
fn pixel_area(
    r: DRect,
    origin: (f32, f32),
    scale: f32,
    dims: (u32, u32),
) -> Option<(u32, u32, u32, u32)> {
    let r = r.normalised();
    let edge = |v: f32, o: f32, max: u32| (((v - o) * scale).round().max(0.0) as u32).min(max);
    let (x0, x1) = (
        edge(r.x, origin.0, dims.0),
        edge(r.right(), origin.0, dims.0),
    );
    let (y0, y1) = (
        edge(r.y, origin.1, dims.1),
        edge(r.bottom(), origin.1, dims.1),
    );
    (x1 > x0 && y1 > y0).then(|| (x0, y0, x1 - x0, y1 - y0))
}

fn process(img: &mut RgbaImage, shape: &Shape, size: f32, scale: f32) {
    let area = (0, 0, img.width(), img.height());
    match shape {
        Shape::Blur(_) => blur_region(img, area, px(size, scale)),
        Shape::Mosaic(_) => mosaic_region(img, area, px(size, scale)),
        _ => {}
    }
}

fn premultiply(p: Rgba<u8>) -> PremultipliedColorU8 {
    let [r, g, b, a] = p.0;
    ColorU8::from_rgba(r, g, b, a).premultiply()
}

fn to_pixmap(img: &RgbaImage) -> Option<Pixmap> {
    let mut pm = Pixmap::new(img.width(), img.height())?;
    for (dst, src) in pm.pixels_mut().iter_mut().zip(img.pixels()) {
        *dst = premultiply(*src);
    }
    Some(pm)
}

fn from_pixmap(pm: &Pixmap) -> RgbaImage {
    let mut img = RgbaImage::new(pm.width(), pm.height());
    for (dst, src) in img.pixels_mut().zip(pm.pixels()) {
        let c = src.demultiply();
        *dst = Rgba([c.red(), c.green(), c.blue(), c.alpha()]);
    }
    img
}

fn stroke_shape(pm: &mut Pixmap, a: &Annotation, origin: (f32, f32), scale: f32) {
    let to_px = |(x, y): (f32, f32)| ((x - origin.0) * scale, (y - origin.1) * scale);
    let mut pb = PathBuilder::new();
    match &a.shape {
        Shape::Pen(points) => {
            let mut it = points.iter().map(|&p| to_px(p));
            let Some(first) = it.next() else { return };
            pb.move_to(first.0, first.1);
            for p in it {
                pb.line_to(p.0, p.1);
            }
            if points.len() == 1 {
                // A click: a zero-length segment, which a round cap makes a dot.
                pb.line_to(first.0, first.1);
            }
        }
        Shape::Rect(r) | Shape::Ellipse(r) => {
            let r = r.normalised();
            let (l, t) = to_px((r.x, r.y));
            let (rt, b) = to_px((r.right(), r.bottom()));
            let Some(rc) = Rect::from_ltrb(l, t, rt, b) else {
                return;
            };
            if matches!(a.shape, Shape::Rect(_)) {
                pb.push_rect(rc);
            } else {
                pb.push_oval(rc);
            }
        }
        Shape::Blur(_) | Shape::Mosaic(_) => return,
    }
    let Some(path) = pb.finish() else { return };
    let mut paint = Paint::default();
    let [r, g, b, al] = a.colour;
    paint.set_color_rgba8(r, g, b, al);
    paint.anti_alias = true;
    let stroke = Stroke {
        width: px(a.size, scale) as f32,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

/// The final picture of `crop` with every annotation applied in list order.
/// Annotations partly outside `crop` are clipped; `None` if `crop` is empty.
pub fn render(shot: &DesktopShot, crop: DRect, doc: &Doc) -> Option<RgbaImage> {
    let out = shot.compose(crop)?;
    let clipped = crop.normalised().intersect(&shot.bounds())?;
    let scale = out.width() as f32 / clipped.w;
    let origin = (clipped.x, clipped.y);
    let mut canvas = to_pixmap(&out)?;
    let width = canvas.width();
    for a in &doc.items {
        match &a.shape {
            Shape::Blur(r) | Shape::Mosaic(r) => {
                let Some((x, y, w, h)) = pixel_area(*r, origin, scale, out.dimensions()) else {
                    continue;
                };
                // From the composed original, not from what was drawn so far.
                let mut part = image::imageops::crop_imm(&out, x, y, w, h).to_image();
                process(&mut part, &a.shape, a.size, scale);
                for (dx, dy, p) in part.enumerate_pixels() {
                    let i = ((y + dy) * width + x + dx) as usize;
                    canvas.pixels_mut()[i] = premultiply(*p);
                }
            }
            _ => stroke_shape(&mut canvas, a, origin, scale),
        }
    }
    Some(from_pixmap(&canvas))
}

/// The blurred or mosaicked pixels of one region, at the region's own scale,
/// for showing while it is edited. `None` for any other shape.
pub fn region_preview(shot: &DesktopShot, a: &Annotation) -> Option<RgbaImage> {
    if !matches!(a.shape, Shape::Blur(_) | Shape::Mosaic(_)) {
        return None;
    }
    let bounds = a.shape.bounds();
    let mut img = shot.compose(bounds)?;
    let clipped = bounds.normalised().intersect(&shot.bounds())?;
    let scale = img.width() as f32 / clipped.w;
    process(&mut img, &a.shape, a.size, scale);
    Some(img)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::geometry::MonitorShot;

    const RED: [u8; 4] = [255, 0, 0, 255];

    fn rect(x: f32, y: f32, w: f32, h: f32) -> DRect {
        DRect { x, y, w, h }
    }

    fn shot(w: u32, h: u32, px: (u32, u32), c: [u8; 4]) -> DesktopShot {
        DesktopShot {
            monitors: vec![MonitorShot {
                layout: rect(0.0, 0.0, w as f32, h as f32),
                image: RgbaImage::from_pixel(px.0, px.1, Rgba(c)),
                name: String::new(),
                scale: 1.0,
            }],
        }
    }

    fn doc_with(shapes: Vec<(Shape, [u8; 4], f32)>) -> Doc {
        let mut d = Doc::default();
        for (shape, colour, size) in shapes {
            d.add(Annotation {
                id: 0,
                shape,
                colour,
                size,
            });
        }
        d
    }

    #[test]
    fn a_rect_outline_paints_its_edge_not_its_inside() {
        let s = shot(100, 100, (100, 100), [0, 0, 255, 255]);
        let d = doc_with(vec![(Shape::Rect(rect(20.0, 20.0, 60.0, 60.0)), RED, 4.0)]);
        let out = render(&s, rect(0.0, 0.0, 100.0, 100.0), &d).unwrap();
        assert_eq!(out.get_pixel(50, 20).0, RED); // on the top edge
        assert_eq!(out.get_pixel(50, 50).0, [0, 0, 255, 255]); // inside
        assert_eq!(out.get_pixel(5, 5).0, [0, 0, 255, 255]); // outside
    }

    #[test]
    fn mosaic_makes_each_block_one_colour() {
        let mut img = RgbaImage::new(32, 32);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = if (x + y) % 2 == 0 {
                Rgba([255, 255, 255, 255])
            } else {
                Rgba([0, 0, 0, 255])
            };
        }
        mosaic_region(&mut img, (0, 0, 32, 32), 8);
        for by in 0..4 {
            for bx in 0..4 {
                let first = *img.get_pixel(bx * 8, by * 8);
                for y in 0..8 {
                    for x in 0..8 {
                        assert_eq!(*img.get_pixel(bx * 8 + x, by * 8 + y), first);
                    }
                }
                assert!((120..=135).contains(&first.0[0]), "{first:?}");
            }
        }
    }

    #[test]
    fn blur_reads_the_original_not_a_rect_drawn_before_it() {
        let s = shot(100, 100, (100, 100), [0, 255, 0, 255]);
        let d = doc_with(vec![
            (Shape::Rect(rect(30.0, 30.0, 40.0, 40.0)), RED, 6.0),
            (
                Shape::Blur(rect(10.0, 10.0, 80.0, 80.0)),
                [0, 0, 0, 255],
                4.0,
            ),
        ]);
        let out = render(&s, rect(0.0, 0.0, 100.0, 100.0), &d).unwrap();
        assert!(out.pixels().all(|p| p.0 == [0, 255, 0, 255]));
    }

    #[test]
    fn an_annotation_outside_the_crop_does_not_change_it() {
        let s = shot(100, 100, (100, 100), [0, 0, 255, 255]);
        let d = doc_with(vec![
            (Shape::Rect(rect(70.0, 70.0, 20.0, 20.0)), RED, 4.0),
            (Shape::Mosaic(rect(60.0, 60.0, 30.0, 30.0)), RED, 8.0),
        ]);
        let crop = rect(0.0, 0.0, 50.0, 50.0);
        let with = render(&s, crop, &d).unwrap();
        let without = render(&s, crop, &Doc::default()).unwrap();
        assert_eq!(with, without);
    }

    #[test]
    fn strokes_scale_with_a_2x_monitor() {
        let s = shot(100, 100, (200, 200), [0, 0, 255, 255]);
        let d = doc_with(vec![(
            Shape::Pen(vec![(10.0, 50.0), (90.0, 50.0)]),
            RED,
            4.0,
        )]);
        let out = render(&s, rect(0.0, 0.0, 100.0, 100.0), &d).unwrap();
        assert_eq!(out.dimensions(), (200, 200));
        let red_rows = (0..200).filter(|&y| out.get_pixel(100, y).0 == RED).count();
        assert!((7..=9).contains(&red_rows), "{red_rows}");
    }

    #[test]
    fn transparent_gaps_stay_transparent_and_alpha_colours_blend() {
        let s = DesktopShot {
            monitors: vec![MonitorShot {
                layout: rect(0.0, 0.0, 10.0, 10.0),
                image: RgbaImage::from_pixel(10, 10, Rgba([0, 0, 255, 255])),
                name: String::new(),
                scale: 1.0,
            }],
        };
        let d = doc_with(vec![(
            Shape::Rect(rect(0.0, 0.0, 10.0, 10.0)),
            [255, 0, 0, 128],
            2.0,
        )]);
        // Crop reaches past the monitor: the clipped result is the monitor only.
        let out = render(&s, rect(0.0, 0.0, 20.0, 20.0), &d).unwrap();
        assert_eq!(out.dimensions(), (10, 10));
        let p = out.get_pixel(5, 0).0;
        assert!(p[0] > 100 && p[2] > 100 && p[3] == 255, "{p:?}");
    }

    #[test]
    fn a_gap_between_monitors_keeps_alpha_zero() {
        let mk = |x: f32| MonitorShot {
            layout: rect(x, 0.0, 10.0, 10.0),
            image: RgbaImage::from_pixel(10, 10, Rgba([0, 0, 255, 255])),
            name: String::new(),
            scale: 1.0,
        };
        let s = DesktopShot {
            monitors: vec![mk(0.0), mk(20.0)],
        };
        let d = doc_with(vec![(Shape::Pen(vec![(0.0, 0.0), (1.0, 1.0)]), RED, 1.0)]);
        let out = render(&s, rect(0.0, 0.0, 30.0, 10.0), &d).unwrap();
        assert_eq!(out.get_pixel(15, 5).0, [0, 0, 0, 0]);
    }

    #[test]
    fn region_preview_is_only_for_blur_and_mosaic() {
        let s = shot(100, 100, (100, 100), [0, 0, 255, 255]);
        let mk = |shape| Annotation {
            id: 1,
            shape,
            colour: RED,
            size: 4.0,
        };
        let m = region_preview(&s, &mk(Shape::Mosaic(rect(10.0, 10.0, 20.0, 20.0)))).unwrap();
        assert_eq!(m.dimensions(), (20, 20));
        assert!(region_preview(&s, &mk(Shape::Rect(rect(10.0, 10.0, 20.0, 20.0)))).is_none());
    }
}
