//! What takes a step's picture, and what keeps the bar out of it. Each
//! platform brings its own camera; this module only names the shapes and
//! opens the first route that works.

use image::RgbaImage;
use windup_core::capture::backend::StepRoute;
use windup_core::capture::geometry::{DRect, MonitorShot};
use windup_core::steps::pointer::to_image;

use crate::capture::CaptureError;
use crate::video::live::TokenSlot;

/// One step's picture: the monitor the press was on.
pub struct Shot {
    pub image: RgbaImage,
    /// In the image's pixels, when the pointer was found.
    pub pointer: Option<[f32; 2]>,
    /// The monitor pictured, desktop units.
    pub monitor: DRect,
    pub monitor_name: String,
    /// Image pixels per point.
    pub scale: f32,
}

impl Shot {
    /// A monitor's picture, with the pointer at `point` (desktop units) when
    /// it was found on that monitor.
    pub fn of(m: MonitorShot, point: Option<(f32, f32)>) -> Self {
        let size = (m.image.width(), m.image.height());
        Shot {
            pointer: point.and_then(|p| to_image(p, m.layout, size)),
            scale: m.scale * m.px_per_unit(),
            monitor: m.layout,
            monitor_name: m.name,
            image: m.image,
        }
    }
}

/// Takes pictures on the camera thread.
pub trait Camera: Send {
    /// The monitor a shot at `at` (desktop units) would picture; `None` when
    /// it cannot be told, which conceals the bar wherever it is. Asked
    /// before each shot, so the bar is hidden only when it would be in it.
    fn monitor(&mut self, at: Option<[f32; 2]>) -> Option<DRect>;
    /// A picture of the monitor at `at`, the press point when the watcher
    /// knew it, else of the pointer's monitor.
    fn shoot(&mut self, at: Option<[f32; 2]>) -> Result<Shot, CaptureError>;
}

/// Hides the bar for the instant of a shot, where nothing else keeps it out.
pub trait Conceal: Send + Sync {
    /// Hides the bar for a shot of `shot_monitor`; says whether it hid
    /// anything (and so whether `show` has work to do).
    fn hide(&self, shot_monitor: DRect) -> bool;
    fn show(&self);
}

/// For hosts whose bar is never in a picture.
pub struct NoConceal;

impl Conceal for NoConceal {
    fn hide(&self, _shot_monitor: DRect) -> bool {
        false
    }

    fn show(&self) {}
}

/// The first route in `route_order` that opens and takes a picture. That
/// first picture is thrown away: it is taken so that a missing permission
/// fails now, before the user starts clicking.
///
/// `token` holds the saved ScreenCast restore token (video's); once the
/// portal has answered it holds the next, whether or not a camera opened,
/// and the caller saves it (`Config::save_screencast_token`) as video does.
/// The session, and so the token, lasts as long as the camera.
pub fn open_camera(
    route_order: &[StepRoute],
    token: &TokenSlot,
) -> Result<(Box<dyn Camera>, StepRoute), String> {
    let mut why = Vec::new();
    for &route in route_order {
        match open(route, token) {
            Ok(mut camera) => match camera.shoot(None) {
                Ok(_) => return Ok((camera, route)),
                Err(e) => why.push(format!("{route:?}: {}", describe(&e))),
            },
            Err(e) => why.push(format!("{route:?}: {e}")),
        }
    }
    Err(if why.is_empty() {
        "no way to take a step's picture".to_string()
    } else {
        why.join("; ")
    })
}

/// The camera for one route.
// Only the ScreenCast portal (Linux) takes a token.
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
fn open(route: StepRoute, token: &TokenSlot) -> Result<Box<dyn Camera>, String> {
    match route {
        #[cfg(target_os = "windows")]
        StepRoute::Gdi => Ok(Box::new(super::windows::GdiCamera)),
        #[cfg(target_os = "linux")]
        StepRoute::X11 => Ok(Box::new(super::x11::X11Camera::open()?)),
        #[cfg(target_os = "linux")]
        StepRoute::Screencopy { ext, pointer } => Ok(Box::new(
            crate::linux::steps::screencopy::ScreencopyCamera::new(ext, pointer),
        )),
        #[cfg(target_os = "linux")]
        StepRoute::KWin => Ok(Box::new(crate::linux::steps::kwin::KWinCamera::open()?)),
        #[cfg(target_os = "linux")]
        StepRoute::ScreenCast => Ok(Box::new(
            crate::linux::steps::screencast::PortalCamera::open(token)?,
        )),
        other => Err(format!("no camera exists for {other:?}")),
    }
}

pub(crate) fn describe(e: &CaptureError) -> String {
    match e {
        CaptureError::Failed(why) => why.clone(),
        other => other.message().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_route_without_a_camera_is_named_in_the_error() {
        // Routes of another platform, so no test here takes a picture.
        let foreign = if cfg!(target_os = "windows") {
            StepRoute::X11
        } else {
            StepRoute::Gdi
        };
        let token = TokenSlot::default();
        let err = open_camera(&[foreign, StepRoute::None], &token)
            .err()
            .expect("no camera");
        assert!(
            err.contains(&format!("{foreign:?}")) && err.contains("None"),
            "{err}"
        );
        assert!(open_camera(&[], &token).is_err());
        assert_eq!(*token.lock().unwrap(), None, "no portal was asked");
    }

    #[test]
    fn a_shot_finds_the_pointer_on_its_own_monitor_only() {
        let m = || MonitorShot {
            layout: DRect {
                x: 1920.0,
                y: 0.0,
                w: 1280.0,
                h: 1024.0,
            },
            image: RgbaImage::new(1280, 1024),
            name: "DP-2".into(),
            scale: 1.5,
            refresh_hz: None,
        };
        let shot = Shot::of(m(), Some((2000.0, 512.0)));
        assert_eq!(shot.pointer, Some([80.0, 512.0]));
        assert_eq!(shot.scale, 1.5);
        assert_eq!(shot.monitor_name, "DP-2");
        assert_eq!(Shot::of(m(), Some((10.0, 100.0))).pointer, None);
        assert_eq!(Shot::of(m(), None).pointer, None);
    }
}
