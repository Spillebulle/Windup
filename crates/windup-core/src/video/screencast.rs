//! The decisions behind recording through the ScreenCast portal (KDE,
//! GNOME), kept apart from PipeWire so they are tested everywhere: where
//! each shared monitor sits, which pixel formats to ask for and how each is
//! read, which part of a buffer holds the picture, and what becomes of the
//! restore token. Steps reads the same streams for stills: how often a
//! frame is copied, where the pointer is in it, and its pixels as RGBA.

use std::time::{Duration, Instant};

use crate::capture::geometry::DRect;
use crate::video::compose::Order;

/// What the portal says about a stream: its PipeWire node, and its
/// monitor's position and size in desktop units, each when known.
pub type StreamFacts = (u32, Option<(i32, i32)>, Option<(i32, i32)>);

/// Where each stream's monitor sits in desktop (logical) units, from what
/// the portal said about it: `(node id, position, size)`.
///
/// When every stream has a position, those are the layout. GNOME before 46
/// gives none, and then the monitors are laid out left to right in the
/// order given, tops aligned, which is the most a recording can assume. A
/// stream with no size takes no room; callers fill a missing size from the
/// stream's buffer first.
pub fn layout_from_streams(streams: &[StreamFacts]) -> Vec<DRect> {
    let size = |s: Option<(i32, i32)>| {
        let (w, h) = s.unwrap_or((0, 0));
        (w.max(0) as f32, h.max(0) as f32)
    };
    let placed = streams.iter().all(|(_, pos, _)| pos.is_some());
    let mut x = 0.0;
    streams
        .iter()
        .map(|&(_, pos, s)| {
            let (w, h) = size(s);
            let (px, py) = match pos {
                Some((px, py)) if placed => (px as f32, py as f32),
                _ => {
                    let at = x;
                    x += w;
                    (at, 0.0)
                }
            };
            DRect { x: px, y: py, w, h }
        })
        .collect()
}

/// Seams narrower than this (in desktop units) between shared screens are
/// rounding in a scaled layout, not a gap.
const SEAM: f32 = 1.0;

/// Whether the shared `screens` hold all of `area` (desktop units). An area
/// partly off them would record black there, so the recording refuses it.
pub fn covered_by(area: DRect, screens: &[DRect]) -> bool {
    let area = area.normalised();
    // Cut the area along every screen edge that crosses it; each cell is
    // then wholly on a screen or wholly off all of them.
    let cuts = |lo: f32, hi: f32, edges: &mut dyn Iterator<Item = f32>| {
        let mut v: Vec<f32> = edges.filter(|e| *e > lo && *e < hi).collect();
        v.push(lo);
        v.push(hi);
        v.sort_by(f32::total_cmp);
        v.dedup();
        v
    };
    let xs = cuts(
        area.x,
        area.right(),
        &mut screens.iter().flat_map(|s| [s.x, s.right()]),
    );
    let ys = cuts(
        area.y,
        area.bottom(),
        &mut screens.iter().flat_map(|s| [s.y, s.bottom()]),
    );
    !screens.is_empty()
        && xs.windows(2).all(|x| {
            ys.windows(2).all(|y| {
                let (cx, cy) = ((x[0] + x[1]) / 2.0, (y[0] + y[1]) / 2.0);
                x[1] - x[0] < SEAM
                    || y[1] - y[0] < SEAM
                    || screens.iter().any(|s| s.contains(cx, cy))
            })
        })
}

/// The shared-memory pixel formats a recording accepts from PipeWire, each
/// four bytes a pixel with the fourth ignored. Named in memory order, as
/// PipeWire's (SPA's) formats are. No DMA-BUF: without modifiers on offer
/// the compositor hands over plain memory, which is what the Composer reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShmVideoFormat {
    Bgrx,
    Bgra,
    Rgbx,
    Rgba,
}

impl ShmVideoFormat {
    /// What to offer, most wanted first: BGR order is what the encoder
    /// takes, so those frames are copied without swapping.
    pub const OFFERED: [ShmVideoFormat; 4] = [
        ShmVideoFormat::Bgrx,
        ShmVideoFormat::Bgra,
        ShmVideoFormat::Rgbx,
        ShmVideoFormat::Rgba,
    ];

    /// How the Composer reads it.
    pub fn order(self) -> Order {
        match self {
            ShmVideoFormat::Bgrx | ShmVideoFormat::Bgra => Order::Bgra,
            ShmVideoFormat::Rgbx | ShmVideoFormat::Rgba => Order::Rgba,
        }
    }
}

/// Frame pixels per desktop unit for a monitor whose buffer is `px_w` wide
/// and whose logical width is `logical_w`. 1 when either is unusable.
pub fn stream_scale(px_w: u32, logical_w: f32) -> f32 {
    let s = px_w as f32 / logical_w;
    if px_w > 0 && s.is_finite() && s > 0.0 {
        s
    } else {
        1.0
    }
}

/// Where the picture starts in a buffer of `len` bytes and how long its
/// rows are, from the buffer's chunk (`offset`, `stride`, `size`) for a
/// `w` x `h` frame. `None` means there is no new picture in it: an empty
/// or corrupted chunk (a compositor sends those when only the cursor moved
/// or nothing changed), or one too short for the frame, which is skipped
/// rather than read past.
pub fn frame_view(
    offset: u32,
    stride: i32,
    size: u32,
    corrupted: bool,
    len: usize,
    w: u32,
    h: u32,
) -> Option<(usize, usize)> {
    if corrupted || size == 0 || w == 0 || h == 0 {
        return None;
    }
    let row = w as usize * 4;
    let stride = if stride > 0 { stride as usize } else { row };
    if stride < row {
        return None;
    }
    let start = offset as usize;
    let needed = stride.checked_mul(h as usize - 1)?.checked_add(row)?;
    (start.checked_add(needed)? <= len).then_some((start, stride))
}

/// What to do with the saved restore token once a session has started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenChange {
    Keep,
    Set(String),
    Clear,
}

/// A token is good for one session: the one sent is spent, and the portal
/// returns the next. So a new one replaces the saved one, and none at all
/// (the user did not let it remember, or the desktop cannot) clears it, so
/// a spent token is not sent again. Only for a session that started: a
/// refusal or a missing portal leaves the saved token as it was.
pub fn token_change(saved: Option<&str>, returned: Option<&str>) -> TokenChange {
    match (saved, returned) {
        (Some(s), Some(r)) if s == r => TokenChange::Keep,
        (_, Some(r)) => TokenChange::Set(r.to_owned()),
        (Some(_), None) => TokenChange::Clear,
        (None, None) => TokenChange::Keep,
    }
}

/// How often a Steps stream copies a whole frame while nobody asks for one,
/// so a shot of a screen that has stopped changing (whose compositor sends
/// no new frames) is at most this old.
pub const STILL_EVERY: Duration = Duration::from_millis(100);

/// Whether a stream copies the frame that just arrived: always the first one
/// after a shot asked (`wanted`), else at most once per `STILL_EVERY` since
/// the last copy (`last`).
pub fn copy_still(last: Option<Instant>, now: Instant, wanted: bool) -> bool {
    wanted || last.is_none_or(|t| now.saturating_duration_since(t) >= STILL_EVERY)
}

/// The pointer in a stream's frame of `size` pixels, from the stream's
/// cursor metadata: `position` is already the pointer's tip (the hotspot),
/// in the frame's pixels. `None` for an `id` of 0 (no cursor data) or a
/// point outside the frame (the pointer is on another screen).
pub fn cursor_in_stream(id: u32, position: (i32, i32), size: (u32, u32)) -> Option<[f32; 2]> {
    let (x, y) = position;
    let inside = x >= 0 && y >= 0 && (x as i64) < size.0 as i64 && (y as i64) < size.1 as i64;
    (id != 0 && inside).then_some([x as f32, y as f32])
}

/// A `w` x `h` frame whose rows start every `stride` bytes of `bytes`, as
/// opaque RGBA (the fourth byte of every format is ignored). `bytes` must
/// hold the frame (`frame_view` says so).
pub fn still_rgba(bytes: &[u8], stride: usize, w: u32, h: u32, format: ShmVideoFormat) -> Vec<u8> {
    let row = w as usize * 4;
    let mut out = Vec::with_capacity(row * h as usize);
    for y in 0..h as usize {
        let line = &bytes[y * stride..y * stride + row];
        for px in line.as_chunks::<4>().0 {
            match format.order() {
                Order::Bgra => out.extend_from_slice(&[px[2], px[1], px[0], 255]),
                Order::Rgba => out.extend_from_slice(&[px[0], px[1], px[2], 255]),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> DRect {
        DRect { x, y, w, h }
    }

    #[test]
    fn an_area_is_covered_only_when_shared_screens_hold_all_of_it() {
        let left = rect(0.0, 0.0, 1920.0, 1080.0);
        let right = rect(1920.0, 0.0, 1280.0, 1024.0);
        // Inside one screen, and across two that touch.
        assert!(covered_by(rect(100.0, 100.0, 400.0, 300.0), &[left]));
        assert!(covered_by(
            rect(1800.0, 100.0, 400.0, 300.0),
            &[left, right]
        ));
        // Half on a screen that was not shared.
        assert!(!covered_by(rect(1800.0, 100.0, 400.0, 300.0), &[left]));
        // On none, and on the gap below the shorter screen.
        assert!(!covered_by(rect(4000.0, 0.0, 100.0, 100.0), &[left, right]));
        assert!(!covered_by(
            rect(1900.0, 1000.0, 100.0, 60.0),
            &[left, right]
        ));
        // No screens at all.
        assert!(!covered_by(rect(0.0, 0.0, 10.0, 10.0), &[]));
        // Rounding in scaled layouts (a 0.4 unit seam) is not a gap.
        let scaled = rect(0.0, 0.0, 1535.6, 864.0);
        let next = rect(1536.0, 0.0, 1920.0, 1080.0);
        assert!(covered_by(rect(1500.0, 0.0, 100.0, 100.0), &[scaled, next]));
    }

    #[test]
    fn real_positions_are_the_layout() {
        let streams = [
            (40, Some((1920, -200)), Some((1280, 1024))),
            (41, Some((0, 0)), Some((1920, 1080))),
        ];
        assert_eq!(
            layout_from_streams(&streams),
            [
                rect(1920.0, -200.0, 1280.0, 1024.0),
                rect(0.0, 0.0, 1920.0, 1080.0)
            ]
        );
    }

    #[test]
    fn without_positions_the_streams_go_left_to_right() {
        let streams = [
            (40, None, Some((1920, 1080))),
            (41, None, Some((1280, 1024))),
            (42, None, Some((800, 600))),
        ];
        assert_eq!(
            layout_from_streams(&streams),
            [
                rect(0.0, 0.0, 1920.0, 1080.0),
                rect(1920.0, 0.0, 1280.0, 1024.0),
                rect(3200.0, 0.0, 800.0, 600.0)
            ]
        );
    }

    #[test]
    fn one_missing_position_lays_them_all_out() {
        // Mixing given and made-up positions could overlap them.
        let streams = [
            (40, Some((500, 500)), Some((100, 100))),
            (41, None, Some((200, 100))),
        ];
        assert_eq!(
            layout_from_streams(&streams),
            [rect(0.0, 0.0, 100.0, 100.0), rect(100.0, 0.0, 200.0, 100.0)]
        );
    }

    #[test]
    fn a_stream_without_a_size_takes_no_room() {
        let streams = [(40, None, None), (41, None, Some((100, 50)))];
        assert_eq!(
            layout_from_streams(&streams),
            [rect(0.0, 0.0, 0.0, 0.0), rect(0.0, 0.0, 100.0, 50.0)]
        );
        assert!(layout_from_streams(&[]).is_empty());
    }

    #[test]
    fn bgr_formats_come_first_and_read_without_swapping() {
        assert_eq!(ShmVideoFormat::OFFERED[0], ShmVideoFormat::Bgrx);
        assert_eq!(ShmVideoFormat::Bgrx.order(), Order::Bgra);
        assert_eq!(ShmVideoFormat::Bgra.order(), Order::Bgra);
        assert_eq!(ShmVideoFormat::Rgbx.order(), Order::Rgba);
        assert_eq!(ShmVideoFormat::Rgba.order(), Order::Rgba);
        // Every offered format is distinct.
        for (i, a) in ShmVideoFormat::OFFERED.iter().enumerate() {
            assert!(!ShmVideoFormat::OFFERED[i + 1..].contains(a));
        }
    }

    #[test]
    fn the_scale_is_buffer_pixels_over_logical_width() {
        assert_eq!(stream_scale(3840, 1920.0), 2.0);
        assert_eq!(stream_scale(2880, 1920.0), 1.5);
        assert_eq!(stream_scale(1920, 1920.0), 1.0);
        assert_eq!(stream_scale(1920, 0.0), 1.0);
        assert_eq!(stream_scale(0, 1920.0), 1.0);
    }

    #[test]
    fn the_frame_view_follows_the_chunk() {
        // 4x2, padded rows of 20 bytes, starting 8 bytes in.
        assert_eq!(
            frame_view(8, 20, 40, false, 8 + 20 + 16, 4, 2),
            Some((8, 20))
        );
        // No stride given: tightly packed.
        assert_eq!(frame_view(0, 0, 32, false, 32, 4, 2), Some((0, 16)));
        assert_eq!(frame_view(0, -16, 32, false, 32, 4, 2), Some((0, 16)));
    }

    #[test]
    fn an_empty_corrupt_or_short_chunk_is_no_new_picture() {
        assert_eq!(frame_view(0, 16, 0, false, 32, 4, 2), None);
        assert_eq!(frame_view(0, 16, 32, true, 32, 4, 2), None);
        assert_eq!(frame_view(0, 16, 32, false, 31, 4, 2), None);
        assert_eq!(frame_view(4, 16, 32, false, 32, 4, 2), None);
        // A stride shorter than a row cannot be this frame.
        assert_eq!(frame_view(0, 8, 32, false, 32, 4, 2), None);
        assert_eq!(frame_view(0, 16, 32, false, 32, 0, 2), None);
        assert_eq!(frame_view(u32::MAX, 16, 32, false, 32, 4, 2), None);
    }

    #[test]
    fn a_new_token_replaces_the_spent_one() {
        assert_eq!(token_change(None, Some("b")), TokenChange::Set("b".into()));
        assert_eq!(
            token_change(Some("a"), Some("b")),
            TokenChange::Set("b".into())
        );
        assert_eq!(token_change(Some("a"), Some("a")), TokenChange::Keep);
    }

    #[test]
    fn no_token_back_clears_the_saved_one() {
        assert_eq!(token_change(Some("a"), None), TokenChange::Clear);
        assert_eq!(token_change(None, None), TokenChange::Keep);
    }

    #[test]
    fn cursor_meta_maps_into_the_frame() {
        // The position is the pointer's tip already.
        assert_eq!(
            cursor_in_stream(7, (100, 50), (1920, 1080)),
            Some([100.0, 50.0])
        );
        assert_eq!(cursor_in_stream(0, (100, 50), (1920, 1080)), None);
        assert_eq!(cursor_in_stream(7, (1920, 50), (1920, 1080)), None);
        assert_eq!(
            cursor_in_stream(7, (1919, 1079), (1920, 1080)),
            Some([1919.0, 1079.0])
        );
        assert_eq!(cursor_in_stream(7, (-1, 50), (1920, 1080)), None);
        assert_eq!(cursor_in_stream(7, (0, 0), (0, 0)), None);
    }

    #[test]
    fn a_still_is_copied_when_asked_and_otherwise_every_100_ms() {
        let t = Instant::now();
        assert_eq!(STILL_EVERY, Duration::from_millis(100));
        assert!(copy_still(None, t, false), "the first frame");
        assert!(!copy_still(Some(t), t + Duration::from_millis(99), false));
        assert!(copy_still(Some(t), t + STILL_EVERY, false));
        assert!(copy_still(Some(t), t + Duration::from_millis(1), true));
    }

    #[test]
    fn a_still_is_opaque_rgba_whatever_the_order() {
        // 2x2 with padded rows of 12 bytes.
        let mut bytes = vec![0u8; 24];
        bytes[0..8].copy_from_slice(&[1, 2, 3, 0, 4, 5, 6, 0]);
        bytes[12..20].copy_from_slice(&[7, 8, 9, 0, 10, 11, 12, 0]);
        assert_eq!(
            still_rgba(&bytes, 12, 2, 2, ShmVideoFormat::Bgrx),
            [3, 2, 1, 255, 6, 5, 4, 255, 9, 8, 7, 255, 12, 11, 10, 255]
        );
        assert_eq!(
            still_rgba(&bytes, 12, 2, 2, ShmVideoFormat::Rgba),
            [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]
        );
    }
}
