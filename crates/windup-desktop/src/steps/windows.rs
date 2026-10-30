//! The Windows camera: a GDI still of the monitor the press was on, and the
//! pointer from the hook's point or `GetCursorPos`, all in physical pixels.
//! The bar is excluded from capture, so nothing is ever concealed here.

use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
use windup_core::capture::geometry::DRect;

use super::camera::{Camera, Shot};
use crate::capture::CaptureError;
use crate::capture::windows::{DpiScope, grab_monitor_at, monitor_at};

pub struct GdiCamera;

/// The pointer in physical pixels; `None` when Windows will not say (the
/// secure desktop, a locked screen).
fn cursor() -> Option<(f32, f32)> {
    let _dpi = DpiScope::enter();
    let mut p = POINT { x: 0, y: 0 };
    // SAFETY: plain call with an out-pointer to a local.
    (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x as f32, p.y as f32))
}

/// The press point when there is one, else the pointer now.
fn point(at: Option<[f32; 2]>) -> Option<(f32, f32)> {
    at.map(|[x, y]| (x, y)).or_else(cursor)
}

impl Camera for GdiCamera {
    fn monitor(&mut self, at: Option<[f32; 2]>) -> Option<DRect> {
        let (x, y) = point(at)?;
        monitor_at(x, y)
    }

    fn shoot(&mut self, at: Option<[f32; 2]>) -> Result<Shot, CaptureError> {
        let point = point(at);
        // With no pointer at all, the monitor at the desktop's origin: the
        // primary one.
        let (x, y) = point.unwrap_or((0.0, 0.0));
        Ok(Shot::of(grab_monitor_at(x, y)?, point))
    }
}
