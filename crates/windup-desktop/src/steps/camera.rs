//! What takes a step's picture, and what keeps the bar out of it. Each
//! platform brings its own camera; this module only names the shapes and
//! opens the first route that works.

use image::RgbaImage;
use windup_core::capture::backend::StepRoute;
use windup_core::capture::geometry::DRect;

use crate::capture::CaptureError;

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
pub fn open_camera(route_order: &[StepRoute]) -> Result<(Box<dyn Camera>, StepRoute), String> {
    let mut why = Vec::new();
    for &route in route_order {
        match open(route) {
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
fn open(route: StepRoute) -> Result<Box<dyn Camera>, String> {
    Err(format!("no camera exists for {route:?}"))
}

fn describe(e: &CaptureError) -> String {
    match e {
        CaptureError::Failed(why) => why.clone(),
        other => other.message().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_camera_exists_yet_and_the_error_names_the_routes() {
        let err = open_camera(&[StepRoute::Gdi, StepRoute::X11])
            .err()
            .expect("no camera");
        assert!(err.contains("Gdi") && err.contains("X11"), "{err}");
        assert!(open_camera(&[]).is_err());
    }
}
