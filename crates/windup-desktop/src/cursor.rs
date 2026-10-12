//! Cursor and monitor geometry helpers.

#[derive(Clone, Copy, Debug)]
pub struct MonitorBounds {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

pub fn get_cursor_and_monitor() -> ((f32, f32), MonitorBounds) {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

        let mut pt = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut pt) != 0 {
            let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            if !hmon.is_null() {
                let mut mi: MONITORINFO = std::mem::zeroed();
                mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                if GetMonitorInfoW(hmon, &mut mi) != 0 {
                    return (
                        (pt.x as f32, pt.y as f32),
                        MonitorBounds {
                            left: mi.rcWork.left as f32,
                            top: mi.rcWork.top as f32,
                            right: mi.rcWork.right as f32,
                            bottom: mi.rcWork.bottom as f32,
                        },
                    );
                }
            }
            return (
                (pt.x as f32, pt.y as f32),
                MonitorBounds {
                    left: 0.0,
                    top: 0.0,
                    right: 1920.0,
                    bottom: 1080.0,
                },
            );
        }
    }

    (
        (200.0, 200.0),
        MonitorBounds {
            left: 0.0,
            top: 0.0,
            right: 1920.0,
            bottom: 1080.0,
        },
    )
}

#[allow(dead_code)]
pub fn get_cursor_position() -> (f32, f32) {
    get_cursor_and_monitor().0
}
