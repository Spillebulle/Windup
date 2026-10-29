//! Pixel arithmetic for the Wayland screen-copy back ends, kept out of the
//! Linux-only module so it is tested on every platform.

use super::CaptureError;
use image::RgbaImage;
use windup_core::video::compose::Order;

/// The `wl_shm` formats a compositor may offer for a screen copy, and that we
/// can read. Byte order is as in memory: the protocol names them as
/// little-endian words, so `Argb8888` is B, G, R, A.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShmFormat {
    Argb8888,
    Xrgb8888,
    Abgr8888,
    Xbgr8888,
}

pub(crate) use windup_core::capture::geometry::Transform;

impl ShmFormat {
    /// The byte order as the video compositor reads it. Alpha is never
    /// read: a screen is opaque.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn order(self) -> Order {
        match self {
            ShmFormat::Argb8888 | ShmFormat::Xrgb8888 => Order::Bgra,
            ShmFormat::Abgr8888 | ShmFormat::Xbgr8888 => Order::Rgba,
        }
    }
}

/// Convert `height` rows of `stride` bytes to RGBA. `X` formats are opaque.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn to_rgba(
    format: ShmFormat,
    width: u32,
    height: u32,
    stride: usize,
    bytes: &[u8],
) -> Result<RgbaImage, CaptureError> {
    let (w, h) = (width as usize, height as usize);
    if stride < w * 4 || bytes.len() < stride * h.saturating_sub(1) + w * 4 {
        return Err(CaptureError::Failed(format!(
            "the compositor sent {} bytes for {width}x{height} pixels at stride {stride}",
            bytes.len()
        )));
    }
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = &bytes[y * stride..y * stride + w * 4];
        for px in row.as_chunks::<4>().0 {
            let [p0, p1, p2, p3] = *px;
            out.extend_from_slice(&match format {
                ShmFormat::Argb8888 => [p2, p1, p0, p3],
                ShmFormat::Xrgb8888 => [p2, p1, p0, 255],
                ShmFormat::Abgr8888 => [p0, p1, p2, p3],
                ShmFormat::Xbgr8888 => [p0, p1, p2, 255],
            });
        }
    }
    RgbaImage::from_raw(width, height, out)
        .ok_or_else(|| CaptureError::Failed("image size overflow".to_owned()))
}

/// Undo `transform`, giving the picture as it appears on screen. Per
/// wl_output.transform the compositor applies it to content to compensate for
/// the panel (90 is counter-clockwise, flips come first), so the inverse is
/// applied here.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn untransform(img: RgbaImage, transform: Transform) -> RgbaImage {
    use image::imageops::{flip_horizontal, rotate90, rotate180, rotate270};
    match transform {
        Transform::Normal => img,
        // `rotate90` is clockwise, the inverse of the compositor's quarter turn.
        Transform::Rot90 => rotate90(&img),
        Transform::Rot180 => rotate180(&img),
        Transform::Rot270 => rotate270(&img),
        Transform::Flipped => flip_horizontal(&img),
        Transform::Flipped90 => flip_horizontal(&rotate90(&img)),
        Transform::Flipped180 => flip_horizontal(&rotate180(&img)),
        Transform::Flipped270 => flip_horizontal(&rotate270(&img)),
    }
}

/// The `QImage::Format` numbers KWin's ScreenShot2 reports, as the byte order
/// they have in memory on a little-endian machine. Alpha is ignored by the
/// callers (a screen is opaque), which also sidesteps premultiplication.
/// `None` for anything else (16-bit, 30-bit, float, indexed).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn qimage_format(format: u32) -> Option<ShmFormat> {
    match format {
        // RGB32 (0xffRRGGBB), ARGB32, ARGB32_Premultiplied: B, G, R, A.
        4..=6 => Some(ShmFormat::Xrgb8888),
        // RGBX8888, RGBA8888, RGBA8888_Premultiplied: R, G, B, A.
        16..=18 => Some(ShmFormat::Xbgr8888),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn qimage_formats_map_to_byte_orders() {
        for f in [4, 5, 6] {
            assert_eq!(qimage_format(f), Some(ShmFormat::Xrgb8888), "{f}");
        }
        for f in [16, 17, 18] {
            assert_eq!(qimage_format(f), Some(ShmFormat::Xbgr8888), "{f}");
        }
        // RGB16, RGB30, Invalid.
        for f in [0, 7, 20, 99] {
            assert_eq!(qimage_format(f), None, "{f}");
        }
    }

    #[test]
    fn argb_is_bgra_in_memory_and_keeps_alpha() {
        // Two pixels, stride 12 (4 bytes padding), two rows.
        let bytes = [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 9, 9, 9, //
            10, 11, 12, 13, 14, 15, 16, 17, 9, 9, 9, 9,
        ];
        let img = to_rgba(ShmFormat::Argb8888, 2, 2, 12, &bytes).unwrap();
        assert_eq!(img.get_pixel(0, 0).0, [3, 2, 1, 4]);
        assert_eq!(img.get_pixel(1, 0).0, [7, 6, 5, 8]);
        assert_eq!(img.get_pixel(0, 1).0, [12, 11, 10, 13]);
        assert_eq!(img.get_pixel(1, 1).0, [16, 15, 14, 17]);
    }

    #[test]
    fn xrgb_forces_opaque() {
        let img = to_rgba(ShmFormat::Xrgb8888, 1, 1, 4, &[1, 2, 3, 0]).unwrap();
        assert_eq!(img.get_pixel(0, 0).0, [3, 2, 1, 255]);
    }

    #[test]
    fn abgr_is_rgba_in_memory() {
        let img = to_rgba(ShmFormat::Abgr8888, 1, 1, 4, &[1, 2, 3, 4]).unwrap();
        assert_eq!(img.get_pixel(0, 0).0, [1, 2, 3, 4]);
    }

    #[test]
    fn xbgr_forces_opaque() {
        let img = to_rgba(ShmFormat::Xbgr8888, 1, 1, 4, &[1, 2, 3, 0]).unwrap();
        assert_eq!(img.get_pixel(0, 0).0, [1, 2, 3, 255]);
    }

    #[test]
    fn short_or_impossible_buffers_are_errors_not_panics() {
        assert!(matches!(
            to_rgba(ShmFormat::Xrgb8888, 2, 2, 8, &[0; 15]),
            Err(CaptureError::Failed(_))
        ));
        // A stride smaller than a row cannot hold the pixels.
        assert!(matches!(
            to_rgba(ShmFormat::Xrgb8888, 2, 1, 4, &[0; 8]),
            Err(CaptureError::Failed(_))
        ));
    }

    /// 3x2, every pixel distinct: a b c / d e f, as the letters' codes.
    fn upright() -> RgbaImage {
        from_rows(&[b"abc", b"def"])
    }

    fn from_rows(rows: &[&[u8]]) -> RgbaImage {
        let (h, w) = (rows.len() as u32, rows[0].len() as u32);
        RgbaImage::from_fn(w, h, |x, y| {
            let v = rows[y as usize][x as usize];
            Rgba([v, 0, 0, 255])
        })
    }

    /// Each buffer is `upright()` as the compositor transformed it (90 is
    /// counter-clockwise, flips are about the vertical axis first), written
    /// out by hand; undoing it must give `upright()` back.
    #[test]
    fn every_transform_is_undone() {
        let cases: [(Transform, &[&[u8]]); 8] = [
            (Transform::Normal, &[b"abc", b"def"]),
            (Transform::Rot90, &[b"cf", b"be", b"ad"]),
            (Transform::Rot180, &[b"fed", b"cba"]),
            (Transform::Rot270, &[b"da", b"eb", b"fc"]),
            (Transform::Flipped, &[b"cba", b"fed"]),
            (Transform::Flipped90, &[b"ad", b"be", b"cf"]),
            (Transform::Flipped180, &[b"def", b"abc"]),
            (Transform::Flipped270, &[b"fc", b"eb", b"da"]),
        ];
        for (t, rows) in cases {
            assert_eq!(untransform(from_rows(rows), t), upright(), "{t:?}");
        }
    }

    /// The video compositor reads buffers through `Transform::buffer_point`
    /// instead of undoing the transform: both must agree, for every pixel.
    #[test]
    fn buffer_point_agrees_with_untransform() {
        let all = [
            Transform::Normal,
            Transform::Rot90,
            Transform::Rot180,
            Transform::Rot270,
            Transform::Flipped,
            Transform::Flipped90,
            Transform::Flipped180,
            Transform::Flipped270,
        ];
        let (w, h) = (3, 2);
        for t in all {
            let (bw, bh) = if t.swaps_axes() { (h, w) } else { (w, h) };
            let buffer = RgbaImage::from_fn(bw, bh, |x, y| Rgba([x as u8, y as u8, 0, 255]));
            let screen = untransform(buffer.clone(), t);
            assert_eq!(screen.dimensions(), (w, h), "{t:?}");
            for y in 0..h {
                for x in 0..w {
                    let (bx, by) = t.buffer_point(x, y, w, h);
                    assert_eq!(screen.get_pixel(x, y), buffer.get_pixel(bx, by), "{t:?}");
                }
            }
        }
    }
}
