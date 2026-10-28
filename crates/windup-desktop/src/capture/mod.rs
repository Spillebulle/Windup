//! A frozen picture of every monitor, taken before the capture overlay opens.
//!
//! `grab_all` asks the back ends `still_order` names, in turn; the first that
//! works wins. The back ends are one file each; the arithmetic that does not
//! touch the OS (pixel formats, which error to report) is here so it is tested
//! on every platform.

use image::RgbaImage;
use windup_core::capture::backend::{Facts, Still, still_order};
use windup_core::capture::geometry::DesktopShot;
use windup_core::capture::pick::WindowRect;

pub(crate) mod finish;
pub(crate) mod gate;
pub(crate) mod ipcrects;
pub(crate) mod kwin_rects;
pub(crate) mod pixels;
pub(crate) mod split;
pub(crate) mod surfaces;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "linux")]
mod x11;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureError {
    /// The system said no (a permission dialog was refused).
    #[allow(dead_code)] // the Wayland portal back end (a later task) refuses this way
    Denied,
    /// No way of reading the screen exists here.
    Unsupported,
    /// There was a way, and it broke; the text is for the log.
    Failed(String),
}

impl CaptureError {
    pub fn message(&self) -> &'static str {
        match self {
            CaptureError::Denied => {
                "Windup was not allowed to read the screen. Allow it in the dialog, or in your desktop's privacy settings."
            }
            CaptureError::Unsupported => "This desktop does not let applications read the screen.",
            CaptureError::Failed(_) => "The screen could not be captured.",
        }
    }

    /// How informative the error is: a refusal beats a breakage beats "no".
    fn rank(&self) -> u8 {
        match self {
            CaptureError::Denied => 2,
            CaptureError::Failed(_) => 1,
            CaptureError::Unsupported => 0,
        }
    }
}

/// The error worth telling the user about out of those the back ends gave
/// (the first of equals).
fn most_specific(errors: Vec<CaptureError>) -> CaptureError {
    let mut best: Option<CaptureError> = None;
    for e in errors {
        if best.as_ref().is_none_or(|b| e.rank() > b.rank()) {
            best = Some(e);
        }
    }
    best.unwrap_or(CaptureError::Unsupported)
}

/// Whether a D-Bus error by this name means there is nothing to ask: no
/// portal service, or a portal without the interface. The screenshot portal
/// then reports `Unsupported` rather than a breakage.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn dbus_absent(error_name: &str) -> bool {
    matches!(
        error_name,
        "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner"
            | "org.freedesktop.DBus.Error.UnknownMethod"
            | "org.freedesktop.DBus.Error.UnknownInterface"
            | "org.freedesktop.DBus.Error.UnknownObject"
    )
}

fn facts() -> Facts {
    #[cfg(target_os = "windows")]
    {
        Facts {
            windows: true,
            ..Facts::default()
        }
    }
    #[cfg(target_os = "linux")]
    {
        let globals = crate::linux::wayland::probe().unwrap_or_default();
        // The screenshot portal is always tried last; where it is missing the
        // call fails with `Unsupported`, which is what asking first would say.
        Facts::from_env(globals, true)
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        Facts::default()
    }
}

fn grab_with(still: Still) -> Result<DesktopShot, CaptureError> {
    match still {
        #[cfg(target_os = "windows")]
        Still::Gdi => windows::grab(),
        #[cfg(target_os = "linux")]
        Still::X11 => x11::grab(),
        #[cfg(target_os = "linux")]
        Still::ExtImageCopy => {
            use crate::linux::wayland::screencopy::{Method, grab_from_env};
            grab_from_env(Method::ExtImageCopy)
        }
        #[cfg(target_os = "linux")]
        Still::WlrScreencopy => {
            use crate::linux::wayland::screencopy::{Method, grab_from_env};
            grab_from_env(Method::WlrScreencopy)
        }
        #[cfg(target_os = "linux")]
        Still::KWin => crate::linux::kde::grab(),
        #[cfg(target_os = "linux")]
        Still::Portal => crate::linux::portal::screenshot(),
        // A back end for another OS, or nothing at all.
        #[allow(unreachable_patterns)]
        Still::Gdi
        | Still::X11
        | Still::ExtImageCopy
        | Still::WlrScreencopy
        | Still::KWin
        | Still::Portal
        | Still::None => Err(CaptureError::Unsupported),
    }
}

/// Take the picture, by the first back end that can.
pub fn grab_all() -> Result<DesktopShot, CaptureError> {
    let mut errors = Vec::new();
    for still in still_order(&facts()) {
        match grab_with(still) {
            Ok(shot) => return Ok(shot),
            Err(e) => {
                log::debug!("capture via {still:?}: {e:?}");
                errors.push(e);
            }
        }
    }
    Err(most_specific(errors))
}

/// The windows on the desktop, topmost first, in the layout's coordinates.
/// Empty when the desktop cannot say; the picker then looks for edges.
pub fn window_rects(shot: &DesktopShot) -> Vec<WindowRect> {
    #[cfg(target_os = "windows")]
    {
        windows::window_rects(shot)
    }
    #[cfg(target_os = "linux")]
    {
        use windup_core::capture::backend::{Rects, rects};
        match rects(&facts()) {
            Rects::X11 => x11::window_rects(shot),
            Rects::KWinScript => crate::linux::kde::window_rects(),
            Rects::Sway => ipcrects::sway_live(),
            Rects::Hyprland => ipcrects::hyprland_live(),
            // Edge detection, per hover.
            _ => Vec::new(),
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = shot;
        Vec::new()
    }
}

/// GDI hands over BGRA rows with the alpha byte unset; the picture is opaque.
/// `stride` is bytes per row.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn bgra_to_rgba(bytes: &[u8], width: u32, height: u32, stride: usize) -> RgbaImage {
    let mut out = Vec::with_capacity(width as usize * height as usize * 4);
    for row in bytes.chunks(stride.max(1)).take(height as usize) {
        for px in row.as_chunks::<4>().0.iter().take(width as usize) {
            out.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    // A short buffer leaves the rest transparent black rather than panicking.
    out.resize(width as usize * height as usize * 4, 0);
    RgbaImage::from_raw(width, height, out).expect("buffer sized to the image")
}

/// How an X server lays out the root window's pixels.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct XFormat {
    pub bits_per_pixel: u8,
    /// Least significant byte first.
    pub lsb_first: bool,
    pub red_mask: u32,
    pub green_mask: u32,
    pub blue_mask: u32,
}

/// Convert a `GetImage` ZPixmap of `width` x `height` to RGBA. Handles the
/// 32-bits-per-pixel TrueColor layouts with eight bits per channel (24-bit
/// colour in a 32-bit pixel, the common one); anything else is an error whose
/// text says what was found.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn x_pixels_to_rgba(
    data: &[u8],
    width: u32,
    height: u32,
    format: XFormat,
) -> Result<RgbaImage, String> {
    if format.bits_per_pixel != 32 {
        return Err(format!(
            "unsupported pixel size: {} bits per pixel",
            format.bits_per_pixel
        ));
    }
    let channel = |mask: u32| -> Result<(u32, u32), String> {
        if mask.count_ones() == 8 && (mask >> mask.trailing_zeros()) == 0xff {
            Ok((mask, mask.trailing_zeros()))
        } else {
            Err(format!("unsupported pixel layout: channel mask {mask:#x}"))
        }
    };
    let (r, g, b) = (
        channel(format.red_mask)?,
        channel(format.green_mask)?,
        channel(format.blue_mask)?,
    );
    let pixels = width as usize * height as usize;
    if data.len() < pixels * 4 {
        return Err(format!(
            "the server sent {} bytes for {width}x{height} pixels",
            data.len()
        ));
    }
    let mut out = Vec::with_capacity(pixels * 4);
    for word in data.as_chunks::<4>().0.iter().take(pixels) {
        let v = if format.lsb_first {
            u32::from_le_bytes(*word)
        } else {
            u32::from_be_bytes(*word)
        };
        let get = |(mask, shift): (u32, u32)| ((v & mask) >> shift) as u8;
        out.extend_from_slice(&[get(r), get(g), get(b), 255]);
    }
    RgbaImage::from_raw(width, height, out).ok_or_else(|| "image size overflow".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_swaps_channels_and_forces_opaque() {
        // Two pixels per row, stride 12 (4 bytes of padding), two rows.
        let bytes = [
            1, 2, 3, 0, 4, 5, 6, 0, 9, 9, 9, 9, //
            7, 8, 9, 0, 10, 11, 12, 77, 9, 9, 9, 9,
        ];
        let img = bgra_to_rgba(&bytes, 2, 2, 12);
        assert_eq!(img.dimensions(), (2, 2));
        assert_eq!(img.get_pixel(0, 0).0, [3, 2, 1, 255]);
        assert_eq!(img.get_pixel(1, 0).0, [6, 5, 4, 255]);
        assert_eq!(img.get_pixel(0, 1).0, [9, 8, 7, 255]);
        assert_eq!(img.get_pixel(1, 1).0, [12, 11, 10, 255]);
    }

    const BGRX_LE: XFormat = XFormat {
        bits_per_pixel: 32,
        lsb_first: true,
        red_mask: 0xff0000,
        green_mask: 0x00ff00,
        blue_mask: 0x0000ff,
    };

    #[test]
    fn x_bgrx_little_endian() {
        let data = [10, 20, 30, 0, 40, 50, 60, 0xAB];
        let img = x_pixels_to_rgba(&data, 2, 1, BGRX_LE).unwrap();
        assert_eq!(img.get_pixel(0, 0).0, [30, 20, 10, 255]);
        assert_eq!(img.get_pixel(1, 0).0, [60, 50, 40, 255]);
    }

    #[test]
    fn x_big_endian_reads_the_other_way() {
        let f = XFormat {
            lsb_first: false,
            ..BGRX_LE
        };
        // XRGB in memory order.
        let data = [0, 30, 20, 10];
        let img = x_pixels_to_rgba(&data, 1, 1, f).unwrap();
        assert_eq!(img.get_pixel(0, 0).0, [30, 20, 10, 255]);
    }

    #[test]
    fn x_unusual_formats_are_refused_with_a_reason() {
        let f = XFormat {
            bits_per_pixel: 16,
            ..BGRX_LE
        };
        let e = x_pixels_to_rgba(&[0; 8], 2, 2, f).unwrap_err();
        assert!(e.contains("16"), "{e}");
        let f = XFormat {
            red_mask: 0xf800,
            ..BGRX_LE
        };
        assert!(x_pixels_to_rgba(&[0; 4], 1, 1, f).is_err());
    }

    #[test]
    fn x_short_data_is_an_error_not_a_panic() {
        assert!(x_pixels_to_rgba(&[0; 7], 2, 1, BGRX_LE).is_err());
    }

    #[test]
    fn denied_beats_failed_beats_unsupported() {
        use CaptureError::*;
        let failed = || Failed("x".into());
        assert_eq!(most_specific(vec![Unsupported, failed(), Denied]), Denied);
        assert_eq!(most_specific(vec![Unsupported, failed()]), failed());
        assert_eq!(most_specific(vec![Unsupported, Unsupported]), Unsupported);
        assert_eq!(most_specific(vec![]), Unsupported);
        // The first of equals.
        assert_eq!(
            most_specific(vec![Failed("a".into()), Failed("b".into())]),
            Failed("a".into())
        );
    }

    #[test]
    fn a_missing_portal_service_is_absent_and_a_refusal_is_not() {
        for name in [
            "org.freedesktop.DBus.Error.ServiceUnknown",
            "org.freedesktop.DBus.Error.NameHasNoOwner",
            "org.freedesktop.DBus.Error.UnknownMethod",
            "org.freedesktop.DBus.Error.UnknownInterface",
            "org.freedesktop.DBus.Error.UnknownObject",
        ] {
            assert!(dbus_absent(name), "{name}");
        }
        for name in [
            "org.freedesktop.portal.Error.NotAllowed",
            "org.freedesktop.DBus.Error.Failed",
            "org.freedesktop.DBus.Error.NoReply",
            "",
        ] {
            assert!(!dbus_absent(name), "{name}");
        }
    }

    #[test]
    fn messages_match_the_copy() {
        assert_eq!(
            CaptureError::Unsupported.message(),
            "This desktop does not let applications read the screen."
        );
        assert_eq!(
            CaptureError::Failed("x".into()).message(),
            "The screen could not be captured."
        );
        assert!(
            CaptureError::Denied
                .message()
                .starts_with("Windup was not allowed")
        );
    }
}
