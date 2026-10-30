//! Digits as stroke polylines, so the badge numbers look the same in the
//! review and in every export, whatever fonts the machine has.

/// Each digit's strokes on a 0..1 grid, y down.
fn glyph(d: u8) -> Vec<Vec<[f32; 2]>> {
    let one = |v: &[(f32, f32)]| v.iter().map(|&(x, y)| [x, y]).collect::<Vec<_>>();
    match d {
        0 => vec![one(&[
            (0.2, 0.0),
            (0.8, 0.0),
            (1.0, 0.2),
            (1.0, 0.8),
            (0.8, 1.0),
            (0.2, 1.0),
            (0.0, 0.8),
            (0.0, 0.2),
            (0.2, 0.0),
        ])],
        1 => vec![one(&[(0.2, 0.2), (0.55, 0.0), (0.55, 1.0)])],
        2 => vec![one(&[
            (0.0, 0.2),
            (0.2, 0.0),
            (0.8, 0.0),
            (1.0, 0.2),
            (1.0, 0.4),
            (0.0, 1.0),
            (1.0, 1.0),
        ])],
        3 => vec![
            one(&[
                (0.0, 0.1),
                (0.15, 0.0),
                (0.85, 0.0),
                (1.0, 0.15),
                (1.0, 0.35),
                (0.8, 0.5),
                (0.35, 0.5),
            ]),
            one(&[
                (0.8, 0.5),
                (1.0, 0.65),
                (1.0, 0.85),
                (0.85, 1.0),
                (0.15, 1.0),
                (0.0, 0.9),
            ]),
        ],
        4 => vec![one(&[(0.75, 1.0), (0.75, 0.0), (0.0, 0.7), (1.0, 0.7)])],
        5 => vec![one(&[
            (1.0, 0.0),
            (0.1, 0.0),
            (0.0, 0.5),
            (0.8, 0.45),
            (1.0, 0.6),
            (1.0, 0.85),
            (0.8, 1.0),
            (0.2, 1.0),
            (0.0, 0.85),
        ])],
        6 => vec![one(&[
            (0.9, 0.0),
            (0.3, 0.0),
            (0.0, 0.3),
            (0.0, 0.8),
            (0.2, 1.0),
            (0.8, 1.0),
            (1.0, 0.8),
            (1.0, 0.6),
            (0.8, 0.45),
            (0.2, 0.45),
            (0.0, 0.6),
        ])],
        7 => vec![one(&[(0.0, 0.0), (1.0, 0.0), (0.35, 1.0)])],
        8 => vec![
            one(&[
                (0.2, 0.0),
                (0.8, 0.0),
                (1.0, 0.12),
                (1.0, 0.38),
                (0.8, 0.5),
                (0.2, 0.5),
                (0.0, 0.38),
                (0.0, 0.12),
                (0.2, 0.0),
            ]),
            one(&[
                (0.2, 0.5),
                (0.8, 0.5),
                (1.0, 0.62),
                (1.0, 0.88),
                (0.8, 1.0),
                (0.2, 1.0),
                (0.0, 0.88),
                (0.0, 0.62),
                (0.2, 0.5),
            ]),
        ],
        // A nine is a six turned half a turn.
        _ => glyph(6)
            .into_iter()
            .map(|s| s.into_iter().map(|[x, y]| [1.0 - x, 1.0 - y]).collect())
            .collect(),
    }
}

/// How wide a digit is, as a share of its height.
pub const DIGIT_WIDTH: f32 = 0.5;
/// How far apart digit centres are, as a share of the height.
pub const DIGIT_PITCH: f32 = 0.6;

/// The strokes of `n`, centred on `centre`, `height` tall, in the same units.
pub fn digits(n: usize, centre: [f32; 2], height: f32) -> Vec<Vec<[f32; 2]>> {
    let text = n.to_string();
    let count = text.len() as f32;
    let mut out = Vec::new();
    for (i, ch) in text.bytes().enumerate() {
        let mid = centre[0] + (i as f32 - (count - 1.0) / 2.0) * DIGIT_PITCH * height;
        for stroke in glyph(ch - b'0') {
            out.push(
                stroke
                    .into_iter()
                    .map(|[x, y]| {
                        [
                            mid + (x - 0.5) * DIGIT_WIDTH * height,
                            centre[1] + (y - 0.5) * height,
                        ]
                    })
                    .collect(),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_digit_has_strokes_inside_its_grid() {
        for d in 0..10u8 {
            let g = glyph(d);
            assert!(!g.is_empty());
            for [x, y] in g.iter().flatten() {
                assert!((0.0..=1.0).contains(x) && (0.0..=1.0).contains(y), "{d}");
            }
        }
    }

    #[test]
    fn digits_are_spaced_and_centred() {
        let two = digits(11, [100.0, 50.0], 10.0);
        assert_eq!(two.len(), 2);
        let xs = |s: &Vec<[f32; 2]>| s.iter().map(|p| p[0]).sum::<f32>() / s.len() as f32;
        assert!(xs(&two[0]) < 100.0 && xs(&two[1]) > 100.0);
        assert!(two.iter().flatten().all(|p| (45.0..=55.0).contains(&p[1])));
    }
}
