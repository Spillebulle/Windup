//! Per-frame stitching for the frame sources that copy monitors one by one
//! (Wayland screencopy): crops each monitor's frame to the recorded
//! rectangle and places it in one BGRA picture.
//!
//! Like a still (`DesktopShot::compose`), the picture is at the highest
//! scale among the monitors the rectangle crosses, and what no monitor
//! covers is black. Unlike a still it is opaque, nearest neighbour, and the
//! mapping from every output pixel to a monitor pixel is worked out once
//! per recording, so a frame is only table lookups and copies.

use crate::capture::geometry::{DRect, Transform};
use crate::video::even_size;

/// The byte order of a monitor's frame. Alpha is never read: a screen is
/// opaque, and some formats leave that byte undefined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Bgra,
    Rgba,
}

/// How a monitor's frames are laid out, beyond their size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Buffer {
    pub transform: Transform,
    pub order: Order,
    /// Rows bottom to top (wlr-screencopy's `y_invert`), before the
    /// transform is undone.
    pub y_invert: bool,
}

impl Default for Buffer {
    fn default() -> Buffer {
        Buffer {
            transform: Transform::Normal,
            order: Order::Bgra,
            y_invert: false,
        }
    }
}

pub struct Composer {
    rect: DRect,
    scale: f64,
    size: (u32, u32),
    monitors: Vec<Map>,
}

/// One monitor's part of the picture.
struct Map {
    layout: DRect,
    /// The frame's size as it appears on screen (before any transform).
    px: (u32, u32),
    buffer: Buffer,
    /// The output columns and rows the monitor covers start here.
    dx0: usize,
    dy0: usize,
    /// For each covered output column, its buffer point when the row is 0;
    /// for each covered output row, what it adds to that. Each buffer
    /// coordinate depends on one screen axis only (`buffer_point`).
    cols: Vec<(i64, i64)>,
    rows: Vec<(i64, i64)>,
}

impl Composer {
    /// `rect` in desktop units; each monitor as its layout, its scale (frame
    /// pixels per desktop unit) and its frame's on-screen pixel size. Frames
    /// start out as `Buffer::default()`; `set_buffer` says otherwise. A
    /// rectangle under 2x2 pixels still gives a 2x2 picture: callers refuse
    /// those before getting here.
    pub fn new(rect: DRect, monitors: &[(DRect, f32, (u32, u32))]) -> Composer {
        let rect = rect.normalised();
        let scale = monitors
            .iter()
            .filter(|(layout, _, _)| layout.intersect(&rect).is_some())
            .map(|(_, s, _)| f64::from(*s))
            .filter(|s| s.is_finite() && *s > 0.0)
            .fold(0.0_f64, f64::max);
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let side = |v: f32| {
            (f64::from(v) * scale)
                .round()
                .clamp(0.0, f64::from(u32::MAX)) as u32
        };
        let size = even_size(side(rect.w), side(rect.h)).unwrap_or((2, 2));
        let mut c = Composer {
            rect,
            scale,
            size,
            monitors: Vec::new(),
        };
        c.monitors = monitors
            .iter()
            .map(|&(layout, _, px)| c.map(layout, px, Buffer::default()))
            .collect();
        c
    }

    /// The picture's size, even both ways.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// A picture with nothing on it yet: black and opaque. Blit into the
    /// same one every frame and the gaps stay that way.
    pub fn canvas(&self) -> Vec<u8> {
        [0, 0, 0, 255].repeat(self.size.0 as usize * self.size.1 as usize)
    }

    /// Change how `monitor`'s frames are laid out, and its mapping with it.
    pub fn set_buffer(&mut self, monitor: usize, buffer: Buffer) {
        if let Some(m) = self.monitors.get(monitor) {
            let (layout, px) = (m.layout, m.px);
            self.monitors[monitor] = self.map(layout, px, buffer);
        }
    }

    pub fn buffer(&self, monitor: usize) -> Option<Buffer> {
        self.monitors.get(monitor).map(|m| m.buffer)
    }

    /// Copy `monitor`'s part of `frame` (rows of `stride` bytes) into `out`,
    /// a `canvas`. A frame too short for its mapping is skipped, not read
    /// past.
    pub fn blit(&self, monitor: usize, frame: &[u8], stride: usize, out: &mut [u8]) {
        let Some(m) = self.monitors.get(monitor) else {
            return;
        };
        let w = self.size.0 as usize;
        if m.cols.is_empty() || m.rows.is_empty() || out.len() < w * self.size.1 as usize * 4 {
            return;
        }
        let s = stride as i64;
        let offsets =
            |v: &[(i64, i64)]| -> Vec<i64> { v.iter().map(|&(x, y)| y * s + x * 4).collect() };
        let (cols, rows) = (offsets(&m.cols), offsets(&m.rows));
        let range = |v: &[i64]| {
            v.iter()
                .fold((i64::MAX, i64::MIN), |(lo, hi), &o| (lo.min(o), hi.max(o)))
        };
        let ((clo, chi), (rlo, rhi)) = (range(&cols), range(&rows));
        if clo + rlo < 0 || chi + rhi + 4 > frame.len() as i64 {
            return;
        }
        // Same scale, upright, in BGRA: each row is one run of the frame.
        let run = m.buffer.order == Order::Bgra && cols.windows(2).all(|p| p[1] - p[0] == 4);
        for (j, &r) in rows.iter().enumerate() {
            let o = ((m.dy0 + j) * w + m.dx0) * 4;
            let dst = &mut out[o..o + cols.len() * 4];
            if run {
                let i = (r + cols[0]) as usize;
                dst.copy_from_slice(&frame[i..i + dst.len()]);
                for p in dst.as_chunks_mut::<4>().0 {
                    p[3] = 255;
                }
                continue;
            }
            for (p, &c) in dst.as_chunks_mut::<4>().0.iter_mut().zip(&cols) {
                let i = (r + c) as usize;
                let q = &frame[i..i + 4];
                *p = match m.buffer.order {
                    Order::Bgra => [q[0], q[1], q[2], 255],
                    Order::Rgba => [q[2], q[1], q[0], 255],
                };
            }
        }
    }

    /// The output columns and rows whose centres fall on `layout`, and the
    /// frame pixel under each, as buffer points.
    fn map(&self, layout: DRect, px: (u32, u32), buffer: Buffer) -> Map {
        let (dx0, xs) = self.axis(self.rect.x, self.size.0, layout.x, layout.w, px.0);
        let (dy0, ys) = self.axis(self.rect.y, self.size.1, layout.y, layout.h, px.1);
        let (w, h) = px;
        let bh = if buffer.transform.swaps_axes() { w } else { h };
        let point = |x: u32, y: u32| {
            let (bx, by) = buffer.transform.buffer_point(x, y, w, h);
            let by = if buffer.y_invert { bh - 1 - by } else { by };
            (i64::from(bx), i64::from(by))
        };
        let (cols, rows) = if xs.is_empty() || ys.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            let (ox, oy) = point(0, 0);
            (
                xs.iter().map(|&x| point(x, 0)).collect(),
                ys.iter()
                    .map(|&y| {
                        let (bx, by) = point(0, y);
                        (bx - ox, by - oy)
                    })
                    .collect(),
            )
        };
        Map {
            layout,
            px,
            buffer,
            dx0,
            dy0,
            cols,
            rows,
        }
    }

    /// Along one axis: the first output index on the monitor, and for each
    /// one from there the monitor pixel nearest its centre. Monotonic, so
    /// the covered indices are one run.
    fn axis(&self, origin: f32, n: u32, lo: f32, len: f32, pixels: u32) -> (usize, Vec<u32>) {
        let (origin, lo, len) = (f64::from(origin), f64::from(lo), f64::from(len));
        let mut first = None;
        let mut src = Vec::new();
        if len <= 0.0 || pixels == 0 {
            return (0, src);
        }
        for i in 0..n {
            let u = origin + (f64::from(i) + 0.5) / self.scale;
            if u >= lo && u < lo + len {
                first.get_or_insert(i as usize);
                let p = ((u - lo) * f64::from(pixels) / len).floor() as u32;
                src.push(p.min(pixels - 1));
            }
        }
        (first.unwrap_or(0), src)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> DRect {
        DRect { x, y, w, h }
    }

    /// A BGRA frame where pixel (x, y) is [x, y, tag, 0], so every source
    /// pixel can be told apart (alpha 0 checks it is forced opaque).
    fn frame(w: u32, h: u32, tag: u8) -> Vec<u8> {
        let mut out = Vec::new();
        for y in 0..h {
            for x in 0..w {
                out.extend_from_slice(&[x as u8, y as u8, tag, 0]);
            }
        }
        out
    }

    fn px(out: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * w as usize + x as usize) * 4;
        out[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn one_scale_1_monitor_copies_pixels_one_to_one() {
        let c = Composer::new(
            rect(10.0, 20.0, 40.0, 30.0),
            &[(rect(0.0, 0.0, 100.0, 100.0), 1.0, (100, 100))],
        );
        assert_eq!(c.size(), (40, 30));
        let mut out = c.canvas();
        c.blit(0, &frame(100, 100, 7), 400, &mut out);
        for y in 0..30 {
            for x in 0..40 {
                assert_eq!(
                    px(&out, 40, x, y),
                    [10 + x as u8, 20 + y as u8, 7, 255],
                    "{x},{y}"
                );
            }
        }
    }

    #[test]
    fn across_scale_2_and_scale_1_the_output_is_at_scale_2() {
        let c = Composer::new(
            rect(90.0, 0.0, 20.0, 10.0),
            &[
                (rect(0.0, 0.0, 100.0, 100.0), 2.0, (200, 200)),
                (rect(100.0, 0.0, 100.0, 100.0), 1.0, (100, 100)),
            ],
        );
        assert_eq!(c.size(), (40, 20));
        let mut out = c.canvas();
        c.blit(0, &frame(200, 200, 1), 800, &mut out);
        c.blit(1, &frame(100, 100, 2), 400, &mut out);
        // The scale-2 half is copied pixel for pixel from x = 180.
        for x in 0..20 {
            assert_eq!(px(&out, 40, x, 3), [180 + x as u8, 3, 1, 255], "{x}");
        }
        // Each scale-1 pixel becomes 2x2.
        for x in 20..40 {
            for y in 0..20 {
                let (sx, sy) = ((x - 20) / 2, y / 2);
                assert_eq!(px(&out, 40, x, y), [sx as u8, sy as u8, 2, 255], "{x},{y}");
            }
        }
    }

    #[test]
    fn a_gap_between_monitors_is_black_and_opaque() {
        let c = Composer::new(
            rect(90.0, 0.0, 20.0, 100.0),
            &[
                (rect(0.0, 0.0, 100.0, 100.0), 1.0, (100, 100)),
                (rect(100.0, 50.0, 100.0, 100.0), 1.0, (100, 100)),
            ],
        );
        let mut out = c.canvas();
        c.blit(0, &frame(100, 100, 1), 400, &mut out);
        c.blit(1, &frame(100, 100, 2), 400, &mut out);
        assert_eq!(px(&out, 20, 15, 10), [0, 0, 0, 255]);
        assert_eq!(px(&out, 20, 15, 49), [0, 0, 0, 255]);
        assert_eq!(px(&out, 20, 15, 50), [5, 0, 2, 255]);
        assert_eq!(px(&out, 20, 5, 10), [95, 10, 1, 255]);
    }

    #[test]
    fn a_monitor_at_negative_coordinates_lands_at_its_offset() {
        let c = Composer::new(
            rect(-10.0, -5.0, 20.0, 10.0),
            &[
                (rect(-100.0, -50.0, 100.0, 100.0), 1.0, (100, 100)),
                (rect(0.0, -50.0, 100.0, 100.0), 1.0, (100, 100)),
            ],
        );
        let mut out = c.canvas();
        c.blit(0, &frame(100, 100, 1), 400, &mut out);
        c.blit(1, &frame(100, 100, 2), 400, &mut out);
        assert_eq!(px(&out, 20, 0, 0), [90, 45, 1, 255]);
        assert_eq!(px(&out, 20, 9, 9), [99, 54, 1, 255]);
        assert_eq!(px(&out, 20, 10, 0), [0, 45, 2, 255]);
        assert_eq!(px(&out, 20, 19, 9), [9, 54, 2, 255]);
    }

    #[test]
    fn the_size_is_rounded_down_to_even() {
        let c = Composer::new(
            rect(0.0, 0.0, 641.0, 359.0),
            &[(rect(0.0, 0.0, 1920.0, 1080.0), 1.0, (1920, 1080))],
        );
        assert_eq!(c.size(), (640, 358));
        assert_eq!(c.canvas().len(), 640 * 358 * 4);
    }

    #[test]
    fn rgba_frames_are_swapped_to_bgra() {
        let mut c = Composer::new(
            rect(0.0, 0.0, 2.0, 2.0),
            &[(rect(0.0, 0.0, 2.0, 2.0), 1.0, (2, 2))],
        );
        c.set_buffer(
            0,
            Buffer {
                order: Order::Rgba,
                ..Buffer::default()
            },
        );
        let mut out = c.canvas();
        c.blit(0, &frame(2, 2, 9), 8, &mut out);
        assert_eq!(px(&out, 2, 1, 0), [9, 0, 1, 255]);
    }

    #[test]
    fn a_rotated_or_inverted_buffer_is_read_upright() {
        // On screen 4x2; the buffer is the screen turned (2x4), with the
        // stride padded by one pixel.
        let mut c = Composer::new(
            rect(0.0, 0.0, 4.0, 2.0),
            &[(rect(0.0, 0.0, 4.0, 2.0), 1.0, (4, 2))],
        );
        for (t, inv) in [
            (Transform::Rot90, false),
            (Transform::Flipped270, false),
            (Transform::Rot180, true),
        ] {
            c.set_buffer(
                0,
                Buffer {
                    transform: t,
                    y_invert: inv,
                    ..Buffer::default()
                },
            );
            let (bw, bh) = if t.swaps_axes() { (2, 4) } else { (4, 2) };
            let stride = (bw + 1) * 4;
            let mut buf = vec![0u8; stride as usize * bh as usize];
            for by in 0..bh {
                for bx in 0..bw {
                    let i = (by * stride + bx * 4) as usize;
                    buf[i..i + 4].copy_from_slice(&[bx as u8, by as u8, 0, 0]);
                }
            }
            let mut out = c.canvas();
            c.blit(0, &buf, stride as usize, &mut out);
            for y in 0..2 {
                for x in 0..4 {
                    let (bx, mut by) = t.buffer_point(x, y, 4, 2);
                    if inv {
                        by = bh - 1 - by;
                    }
                    assert_eq!(
                        px(&out, 4, x, y),
                        [bx as u8, by as u8, 0, 255],
                        "{t:?} {x},{y}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_short_frame_is_skipped_not_read_past() {
        let c = Composer::new(
            rect(0.0, 0.0, 4.0, 4.0),
            &[(rect(0.0, 0.0, 4.0, 4.0), 1.0, (4, 4))],
        );
        let mut out = c.canvas();
        c.blit(0, &[1; 20], 16, &mut out);
        c.blit(7, &frame(4, 4, 1), 16, &mut out);
        assert!(out.as_chunks::<4>().0.iter().all(|p| *p == [0, 0, 0, 255]));
    }
}
