//! A step's marker shapes (`windup_core::steps::marker`) drawn with egui, so
//! a picture on screen shows the same marker the exports burn in with
//! tiny-skia.

use egui::epaint::{Mesh, PathShape};
use egui::{Color32, Painter, Pos2, Stroke};
use windup_core::steps::marker::Prim;

fn colour(c: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

/// Paints `prims` in order. `to_screen` maps an image pixel to the screen
/// and `k` is screen points per image pixel, for radii and widths.
pub fn paint_prims(
    painter: &Painter,
    prims: &[Prim],
    to_screen: impl Fn([f32; 2]) -> Pos2,
    k: f32,
) {
    for prim in prims {
        match prim {
            Prim::Disc { c, r, colour: col } => {
                painter.circle_filled(to_screen(*c), r * k, colour(*col));
            }
            Prim::Ring {
                c,
                r,
                width,
                colour: col,
            } => {
                painter.circle_stroke(to_screen(*c), r * k, Stroke::new(width * k, colour(*col)));
            }
            Prim::Polygon {
                points,
                colour: col,
            } => {
                let points: Vec<Pos2> = points.iter().map(|p| to_screen(*p)).collect();
                painter.add(fill_mesh(&points, colour(*col)));
            }
            Prim::Stroke {
                points,
                width,
                colour: col,
            } => {
                let mut points: Vec<Pos2> = points.iter().map(|p| to_screen(*p)).collect();
                let stroke = Stroke::new(width * k, colour(*col));
                let closed = points.len() > 2 && points.first() == points.last();
                if closed {
                    points.pop();
                    painter.add(PathShape::closed_line(points, stroke));
                } else {
                    // tiny-skia's round caps: the digits' ink is opaque, so
                    // the cap discs never show through the line.
                    for end in [points.first(), points.last()].into_iter().flatten() {
                        painter.circle_filled(*end, width * k / 2.0, stroke.color);
                    }
                    painter.add(PathShape::line(points, stroke));
                }
            }
        }
    }
}

/// A filled simple polygon, which may be concave (the arrow), as triangles.
/// egui fills only convex paths, so this clips ears.
fn fill_mesh(points: &[Pos2], fill: Color32) -> Mesh {
    let mut mesh = Mesh::default();
    for p in points {
        mesh.colored_vertex(*p, fill);
    }
    for [a, b, c] in triangulate(points) {
        mesh.add_triangle(a, b, c);
    }
    mesh
}

/// Twice the signed area. Its sign is the polygon's winding, in the same
/// sense as `cross` below, so a convex corner has a `cross` of that sign.
fn signed_area(points: &[Pos2]) -> f32 {
    let n = points.len();
    (0..n)
        .map(|i| {
            let (p, q) = (points[i], points[(i + 1) % n]);
            p.x * q.y - q.x * p.y
        })
        .sum()
}

fn cross(a: Pos2, b: Pos2, c: Pos2) -> f32 {
    (b - a).x * (c - a).y - (b - a).y * (c - a).x
}

/// Ear clipping: the triangles of a simple polygon, as indices.
fn triangulate(points: &[Pos2]) -> Vec<[u32; 3]> {
    let mut left: Vec<usize> = (0..points.len()).collect();
    let mut out = Vec::new();
    if left.len() < 3 {
        return out;
    }
    let turn = signed_area(points).signum();
    while left.len() > 3 {
        let n = left.len();
        let ear = (0..n).find(|&i| {
            let (a, b, c) = (left[(i + n - 1) % n], left[i], left[(i + 1) % n]);
            let (pa, pb, pc) = (points[a], points[b], points[c]);
            if cross(pa, pb, pc) * turn <= 0.0 {
                return false;
            }
            !left.iter().any(|&o| {
                o != a && o != b && o != c && {
                    let q = points[o];
                    cross(pa, pb, q) * turn >= 0.0
                        && cross(pb, pc, q) * turn >= 0.0
                        && cross(pc, pa, q) * turn >= 0.0
                }
            })
        });
        // A degenerate outline has no ear left; what remains is a fan.
        let i = ear.unwrap_or(1);
        out.push([
            left[(i + n - 1) % n] as u32,
            left[i] as u32,
            left[(i + 1) % n] as u32,
        ]);
        left.remove(i);
    }
    out.push([left[0] as u32, left[1] as u32, left[2] as u32]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;
    use windup_core::steps::marker::marker;
    use windup_core::steps::options::{MarkerKind, MarkerStyle};

    fn area(t: [u32; 3], points: &[Pos2]) -> f32 {
        let [a, b, c] = t.map(|i| points[i as usize]);
        cross(a, b, c).abs() / 2.0
    }

    #[test]
    fn the_arrow_fills_exactly_its_outline() {
        let style = MarkerStyle {
            kind: MarkerKind::Arrow,
            ..MarkerStyle::default()
        };
        let prims = marker(&style, [500.0, 400.0], 1, 1.0, (1920, 1080));
        let Some(Prim::Polygon { points, .. }) = prims.last() else {
            panic!("no polygon");
        };
        let points: Vec<Pos2> = points.iter().map(|p| pos2(p[0], p[1])).collect();
        let tris = triangulate(&points);
        assert_eq!(tris.len(), points.len() - 2);
        let sum: f32 = tris.iter().map(|t| area(*t, &points)).sum();
        let whole = signed_area(&points).abs() / 2.0;
        assert!((sum - whole).abs() < 1e-2, "{sum} vs {whole}");
    }

    #[test]
    fn every_marker_paints_without_panicking() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            for kind in MarkerKind::ALL {
                let style = MarkerStyle {
                    kind,
                    ..MarkerStyle::default()
                };
                let prims = marker(&style, [5.0, 5.0], 12, 1.5, (200, 100));
                paint_prims(&painter, &prims, |p| pos2(p[0] / 4.0, p[1] / 4.0), 0.25);
            }
        });
    }
}
