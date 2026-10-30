//! Burning a step's marker into its picture. The pictures on disk stay
//! clean; this runs for the review, the thumbnails and every export.

use image::RgbaImage;
use image::imageops::{self, FilterType};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

use super::marker::{Prim, marker};
use super::model::Step;
use super::options::MarkerStyle;
use crate::capture::raster::{from_pixmap, to_pixmap};

fn paint(colour: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    let [r, g, b, a] = colour;
    p.set_color_rgba8(r, g, b, a);
    p.anti_alias = true;
    p
}

fn polyline(points: &[[f32; 2]]) -> PathBuilder {
    let mut pb = PathBuilder::new();
    for (i, p) in points.iter().enumerate() {
        if i == 0 {
            pb.move_to(p[0], p[1]);
        } else {
            pb.line_to(p[0], p[1]);
        }
    }
    pb
}

fn draw(pm: &mut Pixmap, prim: &Prim) {
    let round = |width: f32| Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    let id = Transform::identity();
    match prim {
        Prim::Disc { c, r, colour } => {
            if let Some(path) = PathBuilder::from_circle(c[0], c[1], *r) {
                pm.fill_path(&path, &paint(*colour), FillRule::Winding, id, None);
            }
        }
        Prim::Ring {
            c,
            r,
            width,
            colour,
        } => {
            if let Some(path) = PathBuilder::from_circle(c[0], c[1], *r) {
                pm.stroke_path(&path, &paint(*colour), &round(*width), id, None);
            }
        }
        Prim::Polygon { points, colour } => {
            let mut pb = polyline(points);
            pb.close();
            if let Some(path) = pb.finish() {
                pm.fill_path(&path, &paint(*colour), FillRule::Winding, id, None);
            }
        }
        Prim::Stroke {
            points,
            width,
            colour,
        } => {
            if let Some(path) = polyline(points).finish() {
                pm.stroke_path(&path, &paint(*colour), &round(*width), id, None);
            }
        }
    }
}

/// Draws `prims` over `img`, anti-aliased, in list order.
pub fn burn(img: &mut RgbaImage, prims: &[Prim]) {
    if prims.is_empty() {
        return;
    }
    let Some(mut pm) = to_pixmap(img) else { return };
    for prim in prims {
        draw(&mut pm, prim);
    }
    *img = from_pixmap(&pm);
}

/// The step's picture with its marker, numbered `number` (the position in the
/// list now, not the file's name). A step without a pointer is left alone.
pub fn render_step(img: &RgbaImage, step: &Step, number: usize, style: &MarkerStyle) -> RgbaImage {
    let mut out = img.clone();
    if let Some(at) = step.pointer {
        let prims = marker(style, at, number, step.scale, (out.width(), out.height()));
        burn(&mut out, &prims);
    }
    out
}

/// A picture at most `max_w` wide, the marker burned in at its size.
pub fn thumbnail(
    img: &RgbaImage,
    step: &Step,
    number: usize,
    style: &MarkerStyle,
    max_w: u32,
) -> RgbaImage {
    if max_w == 0 || img.width() <= max_w {
        return render_step(img, step, number, style);
    }
    let f = max_w as f32 / img.width() as f32;
    let h = ((img.height() as f32 * f).round() as u32).max(1);
    let small = imageops::resize(img, max_w, h, FilterType::Triangle);
    let mut scaled = step.clone();
    scaled.pointer = step.pointer.map(|p| [p[0] * f, p[1] * f]);
    scaled.scale = step.scale * f;
    render_step(&small, &scaled, number, style)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steps::model::{Button, StepCause};
    use crate::steps::options::{MarkerKind, MarkerSize};
    use image::Rgba;

    fn step(file: &str, pointer: Option<[f32; 2]>) -> Step {
        Step {
            file: file.into(),
            cause: StepCause::Click(Button::Left),
            pointer,
            scale: 1.0,
            monitor: String::new(),
            caption: String::new(),
            at_ms: 0,
        }
    }

    fn style(kind: MarkerKind) -> MarkerStyle {
        MarkerStyle {
            kind,
            colour: [255, 0, 0, 255],
            size: MarkerSize::Medium,
        }
    }

    fn black(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 255]))
    }

    #[test]
    fn render_step_without_a_pointer_leaves_the_picture_alone() {
        let img = black(50, 40);
        let out = render_step(&img, &step("01.png", None), 1, &style(MarkerKind::Badge));
        assert_eq!(out.as_raw(), img.as_raw());
    }

    #[test]
    fn render_step_burns_the_marker_colour() {
        let img = black(200, 200);
        let out = render_step(
            &img,
            &step("01.png", Some([100.0, 100.0])),
            1,
            &style(MarkerKind::Ring),
        );
        assert!(out.get_pixel(116, 100)[0] > 200);
        assert_eq!(out.get_pixel(5, 5).0, [0, 0, 0, 255]);
    }

    #[test]
    fn the_badge_number_is_the_one_given_not_the_file() {
        let img = black(200, 200);
        let s = style(MarkerKind::Badge);
        let a = render_step(&img, &step("03.png", Some([50.0, 150.0])), 2, &s);
        let b = render_step(&img, &step("02.png", Some([50.0, 150.0])), 2, &s);
        assert_eq!(a.as_raw(), b.as_raw());
        let c = render_step(&img, &step("02.png", Some([50.0, 150.0])), 3, &s);
        assert_ne!(a.as_raw(), c.as_raw());
    }

    #[test]
    fn a_thumbnail_is_scaled_before_the_marker_is_burned() {
        let img = black(400, 400);
        let s = step("01.png", Some([200.0, 200.0]));
        let t = thumbnail(&img, &s, 1, &style(MarkerKind::Ring), 200);
        assert_eq!((t.width(), t.height()), (200, 200));
        // Medium ring radius 16 at half scale is 8, centred at (100, 100).
        assert!(t.get_pixel(108, 100)[0] > 200);
        assert_eq!(t.get_pixel(116, 100)[0], 0);
    }
}
