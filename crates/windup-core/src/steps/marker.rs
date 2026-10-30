//! Where a step's marker goes and what it is made of, as shapes in image
//! pixels. Nothing here draws; `render` burns the shapes in.

use super::glyphs::{DIGIT_PITCH, DIGIT_WIDTH, digits};
use super::options::{MarkerKind, MarkerStyle};

/// One shape, every coordinate in image pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Prim {
    Disc {
        c: [f32; 2],
        r: f32,
        colour: [u8; 4],
    },
    Ring {
        c: [f32; 2],
        r: f32,
        width: f32,
        colour: [u8; 4],
    },
    Polygon {
        points: Vec<[f32; 2]>,
        colour: [u8; 4],
    },
    Stroke {
        points: Vec<[f32; 2]>,
        width: f32,
        colour: [u8; 4],
    },
}

/// WCAG relative luminance.
fn luminance(c: [u8; 4]) -> f32 {
    let lin = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

/// A colour that stands out from `colour`: black at 70 % on a light one,
/// white at 85 % on a dark one.
pub fn halo(colour: [u8; 4]) -> [u8; 4] {
    if luminance(colour) > 0.5 {
        [0, 0, 0, 179]
    } else {
        [255, 255, 255, 217]
    }
}

/// Black or white, whichever reads on `colour`.
fn ink_on(colour: [u8; 4]) -> [u8; 4] {
    if luminance(colour) > 0.5 {
        [0, 0, 0, 255]
    } else {
        [255, 255, 255, 255]
    }
}

/// How tall the number is inside a badge `diameter` across: half of it, less
/// when several digits would otherwise reach the edge.
pub fn badge_text_height(number: usize, diameter: f32) -> f32 {
    let count = number.to_string().len() as f32;
    let reach = (((count - 1.0) * DIGIT_PITCH + DIGIT_WIDTH) / 2.0).hypot(0.5);
    (0.5 * diameter).min(0.4 * diameter / reach)
}

/// The shapes for one marker, halo first. `at` is the pointer in the image,
/// `scale` the image pixels per point.
pub fn marker(
    style: &MarkerStyle,
    at: [f32; 2],
    number: usize,
    scale: f32,
    image: (u32, u32),
) -> Vec<Prim> {
    let size = style.size.points() * scale;
    let colour = style.colour;
    let halo_colour = halo(colour);
    let edge = size / 16.0;
    let (w, h) = (image.0 as f32, image.1 as f32);
    match style.kind {
        MarkerKind::Ring => {
            let (r, width) = (size / 2.0, size / 8.0);
            vec![
                Prim::Ring {
                    c: at,
                    r,
                    width: width + 2.0 * edge,
                    colour: halo_colour,
                },
                Prim::Ring {
                    c: at,
                    r,
                    width,
                    colour,
                },
            ]
        }
        MarkerKind::Badge => {
            let r = size / 2.0;
            let dot = size / 8.0;
            let mut c = [at[0] + 0.75 * size, at[1] - 0.75 * size];
            if c[0] + r > w {
                c[0] = at[0] - 0.75 * size;
            }
            if c[1] - r < 0.0 {
                c[1] = at[1] + 0.75 * size;
            }
            let text = badge_text_height(number, size);
            let mut prims = vec![
                Prim::Disc {
                    c: at,
                    r: dot + edge,
                    colour: halo_colour,
                },
                Prim::Disc {
                    c,
                    r: r + edge,
                    colour: halo_colour,
                },
                Prim::Disc {
                    c: at,
                    r: dot,
                    colour,
                },
                Prim::Disc { c, r, colour },
            ];
            let ink = ink_on(colour);
            for points in digits(number, c, text) {
                prims.push(Prim::Stroke {
                    points,
                    width: text * 0.14,
                    colour: ink,
                });
            }
            prims
        }
        MarkerKind::Arrow => {
            // The body trails away from the tip along (sx, sy).
            let shape = |sx: f32, sy: f32| -> Vec<[f32; 2]> {
                let k = std::f32::consts::FRAC_1_SQRT_2;
                let (u, v) = ([sx * k, sy * k], [-sy * k, sx * k]);
                let at_uv = |a: f32, b: f32| {
                    [
                        at[0] + u[0] * a * size + v[0] * b * size,
                        at[1] + u[1] * a * size + v[1] * b * size,
                    ]
                };
                vec![
                    at_uv(0.0, 0.0),
                    at_uv(0.42, 0.26),
                    at_uv(0.42, 0.09),
                    at_uv(1.0, 0.09),
                    at_uv(1.0, -0.09),
                    at_uv(0.42, -0.09),
                    at_uv(0.42, -0.26),
                ]
            };
            let mut points = shape(1.0, 1.0);
            let over_x = points.iter().any(|p| p[0] > w);
            let over_y = points.iter().any(|p| p[1] > h);
            if over_x || over_y {
                let sx = if over_x { -1.0 } else { 1.0 };
                let sy = if over_y { -1.0 } else { 1.0 };
                points = shape(sx, sy);
            }
            let mut outline = points.clone();
            outline.push(points[0]);
            vec![
                Prim::Stroke {
                    points: outline,
                    width: 2.0 * edge,
                    colour: halo_colour,
                },
                Prim::Polygon { points, colour },
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steps::options::MarkerSize;

    fn style(kind: MarkerKind) -> MarkerStyle {
        MarkerStyle {
            kind,
            colour: [255, 0, 0, 255],
            size: MarkerSize::Medium,
        }
    }

    fn ring_of(prims: &[Prim]) -> ([f32; 2], f32) {
        match prims.last() {
            Some(Prim::Ring { c, r, .. }) => (*c, *r),
            other => panic!("not a ring: {other:?}"),
        }
    }

    #[test]
    fn a_ring_is_centred_on_the_pointer() {
        let p = marker(&style(MarkerKind::Ring), [100.0, 100.0], 1, 1.0, (400, 300));
        assert_eq!(ring_of(&p), ([100.0, 100.0], 16.0));
    }

    #[test]
    fn marker_size_follows_the_monitor_scale() {
        let p = marker(&style(MarkerKind::Ring), [100.0, 100.0], 1, 2.0, (400, 300));
        assert_eq!(ring_of(&p).1, 32.0);
    }

    #[test]
    fn the_badge_stays_inside_the_picture() {
        let p = marker(
            &style(MarkerKind::Badge),
            [1910.0, 5.0],
            1,
            1.0,
            (1920, 1080),
        );
        let Some(Prim::Disc { c, r, .. }) = p.iter().rev().find(|p| matches!(p, Prim::Disc { .. }))
        else {
            panic!("no disc");
        };
        assert!(c[0] - r >= 0.0 && c[0] + r <= 1920.0);
        assert!(c[1] - r >= 0.0 && c[1] + r <= 1080.0);
        assert!(c[0] < 1910.0 && c[1] > 5.0);
    }

    #[test]
    fn the_arrow_tip_is_at_the_pointer() {
        let arrow = |at: [f32; 2]| {
            let p = marker(&style(MarkerKind::Arrow), at, 1, 1.0, (1920, 1080));
            let Some(Prim::Polygon { points, .. }) = p.last() else {
                panic!("no polygon");
            };
            points.clone()
        };
        let points = arrow([500.0, 400.0]);
        assert_eq!(points[0], [500.0, 400.0]);
        // Pointing up and to the left: the body trails to the lower right.
        assert!(points[3][0] > 500.0 && points[3][1] > 400.0);

        let points = arrow([1915.0, 1075.0]);
        assert_eq!(points[0], [1915.0, 1075.0]);
        assert!(points.iter().all(|p| p[0] <= 1920.0 && p[1] <= 1080.0));
    }

    #[test]
    fn the_halo_contrasts() {
        assert!(halo([255, 255, 255, 255])[0] < 64);
        assert!(halo([120, 20, 20, 255])[0] > 200);
    }

    #[test]
    fn digits_fit_the_badge() {
        for n in [1, 9, 42, 128, 1000] {
            let diameter = 32.0;
            let h = badge_text_height(n, diameter);
            for p in digits(n, [50.0, 50.0], h).iter().flatten() {
                let d = (p[0] - 50.0).hypot(p[1] - 50.0);
                assert!(d <= diameter / 2.0, "{n}: {d}");
            }
        }
    }
}
