//! The KDE camera with the installed package: a still of the pointer's
//! screen from KWin's `ScreenShot2`, and the pointer from a KWin script.
//!
//! KWin answers `ScreenShot2` only to the installed `/usr/bin/windup`
//! (`linux/kde.rs` says why). The AppImage is refused, which fails
//! `open_camera`'s first shot, so the ScreenCast portal is tried next.

use std::time::{Duration, Instant};

use ashpd::zbus::Connection;
use windup_core::capture::geometry::{DRect, MonitorShot};

use crate::capture::CaptureError;
use crate::linux::kde;
use crate::linux::wayland::screencopy::output_layout_from_env;
use crate::steps::camera::{Camera, Shot};

/// How long the screen `monitor` found stays good for the shot after it:
/// the camera asks one and then the other for every step, and the pointer
/// does not move between them.
const AIM_KEEPS: Duration = Duration::from_millis(500);

/// The screen to picture and where the pointer is on it.
#[derive(Clone, Debug, PartialEq)]
struct Aim {
    name: String,
    layout: DRect,
    /// Desktop units, when the script said.
    point: Option<(f32, f32)>,
}

/// The screen holding the pointer at `cursor` (logical units, and KWin's
/// active screen): the one containing the point, else the one of that name
/// (the active screen can follow focus rather than the mouse), else the
/// first, without a point. `None` when there are no screens.
fn aim_at(layout: &[(String, DRect)], cursor: Option<(f32, f32, String)>) -> Option<Aim> {
    let aim = |(name, rect): &(String, DRect), point| Aim {
        name: name.clone(),
        layout: *rect,
        point,
    };
    if let Some((x, y, screen)) = cursor {
        if let Some(on) = layout.iter().find(|(_, r)| r.contains(x, y)) {
            return Some(aim(on, Some((x, y))));
        }
        if let Some(named) = layout.iter().find(|(n, _)| *n == screen) {
            return Some(aim(named, None));
        }
    }
    layout.first().map(|first| aim(first, None))
}

/// KWin's `ScreenShot2` with the pointer from its scripting.
pub struct KWinCamera {
    conn: Connection,
    /// The last screen `monitor` found, and when.
    aimed: Option<(Instant, Aim)>,
}

impl KWinCamera {
    pub fn open() -> Result<KWinCamera, String> {
        let conn = pollster::block_on(kde::session()).map_err(|e| e.to_string())?;
        Ok(KWinCamera { conn, aimed: None })
    }

    /// Where the pointer is now. A script that fails is logged and the
    /// first screen pictured without a marker, rather than no step at all.
    fn aim(&self) -> Result<Aim, CaptureError> {
        let layout = output_layout_from_env()?;
        let cursor = kde::cursor()
            .map_err(|why| log::warn!("steps: no pointer from KWin: {why}"))
            .ok();
        aim_at(&layout, cursor)
            .ok_or_else(|| CaptureError::Failed("the compositor reports no outputs".into()))
    }
}

impl Camera for KWinCamera {
    fn monitor(&mut self, _at: Option<[f32; 2]>) -> Option<DRect> {
        let aim = self.aim().ok()?;
        let layout = aim.layout;
        self.aimed = Some((Instant::now(), aim));
        Some(layout)
    }

    /// The press point the click watcher passes is never known on Wayland,
    /// so KWin is asked.
    fn shoot(&mut self, _at: Option<[f32; 2]>) -> Result<Shot, CaptureError> {
        let aim = match self.aimed.take() {
            Some((when, aim)) if when.elapsed() < AIM_KEEPS => aim,
            _ => self.aim()?,
        };
        let image = pollster::block_on(kde::capture_screen(&self.conn, &aim.name))?;
        let m = MonitorShot {
            layout: aim.layout,
            image,
            name: aim.name,
            scale: 1.0,
            refresh_hz: None,
        };
        Ok(Shot::of(m, aim.point))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Vec<(String, DRect)> {
        let r = |x, w| DRect {
            x,
            y: 0.0,
            w,
            h: 1080.0,
        };
        vec![
            ("DP-1".into(), r(0.0, 1920.0)),
            ("HDMI-A-1".into(), r(1920.0, 1280.0)),
        ]
    }

    #[test]
    fn the_screen_holding_the_pointer_is_pictured() {
        let aim = aim_at(&layout(), Some((2000.0, 10.0, "DP-1".into()))).unwrap();
        assert_eq!(
            aim.name, "HDMI-A-1",
            "the point wins over the active screen"
        );
        assert_eq!(aim.point, Some((2000.0, 10.0)));
    }

    #[test]
    fn without_a_usable_point_the_active_screen_then_the_first() {
        let off = aim_at(&layout(), Some((-50.0, 10.0, "HDMI-A-1".into()))).unwrap();
        assert_eq!((off.name.as_str(), off.point), ("HDMI-A-1", None));
        let none = aim_at(&layout(), None).unwrap();
        assert_eq!((none.name.as_str(), none.point), ("DP-1", None));
        assert_eq!(aim_at(&[], None), None);
    }
}
