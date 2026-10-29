//! The GDI back end: `BitBlt` each monitor from the screen into a DIB, and
//! `EnumWindows` for the picker.
//!
//! Everything runs with the thread made per-monitor DPI aware for the
//! duration, so monitor rectangles and window frames come back in physical
//! pixels whether or not the process is aware (`--capture-test` runs without
//! eframe, which would have set it). The layout is therefore the virtual
//! screen in physical pixels and every image is exactly its layout's size.

use super::{CaptureError, bgra_to_rgba};
use std::ffi::c_void;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, TRUE};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection,
    DEVMODEW, DIB_RGB_COLORS, DeleteDC, DeleteObject, ENUM_CURRENT_SETTINGS, EnumDisplayMonitors,
    EnumDisplaySettingsW, GetDC, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    ReleaseDC, SRCCOPY, SelectObject,
};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
    SetThreadDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
};
use windup_core::capture::geometry::{DRect, DesktopShot, MonitorShot};
use windup_core::capture::pick::WindowRect;

/// Per-monitor-aware for this thread until dropped, then whatever it was.
struct DpiScope(*mut c_void);

impl DpiScope {
    fn enter() -> Self {
        // SAFETY: plain call; a null return (old Windows) is restored as null,
        // which `SetThreadDpiAwarenessContext` ignores.
        Self(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
    }
}

impl Drop for DpiScope {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: restores the context this thread had.
            unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }
}

struct Monitor {
    rect: RECT,
    name: String,
    /// Physical pixels (desktop units) per point: the monitor's DPI / 96.
    scale: f32,
    refresh_hz: Option<u32>,
}

/// The rate a display runs at now. 0 and 1 are the documented "hardware
/// default" values, so they mean unknown.
fn refresh_of(device: &[u16]) -> Option<u32> {
    // SAFETY: `device` is the NUL-terminated device name `GetMonitorInfoW`
    // gave, and an all-zero DEVMODEW with dmSize set is the documented input.
    let hz = unsafe {
        let mut mode: DEVMODEW = std::mem::zeroed();
        mode.dmSize = size_of::<DEVMODEW>() as u16;
        (EnumDisplaySettingsW(device.as_ptr(), ENUM_CURRENT_SETTINGS, &mut mode) != 0)
            .then_some(mode.dmDisplayFrequency)
    };
    hz.filter(|&hz| hz > 1)
}

unsafe extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _dc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> i32 {
    // SAFETY: `data` is the `Vec<Monitor>` `monitors` passed, alive for the call.
    let list = unsafe { &mut *(data as *mut Vec<Monitor>) };
    // SAFETY: an all-zero MONITORINFOEXW with cbSize set is the documented input.
    let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: `info` is a MONITORINFOEXW whose first field is a MONITORINFO.
    if unsafe { GetMonitorInfoW(monitor, &mut info as *mut _ as *mut MONITORINFO) } != 0 {
        let len = info
            .szDevice
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.szDevice.len());
        let (mut dx, mut dy) = (0u32, 0u32);
        // SAFETY: plain call with out-pointers to locals. The thread is
        // per-monitor aware (`DpiScope`), so this is the monitor's own DPI.
        let dpi = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
        list.push(Monitor {
            rect: info.monitorInfo.rcMonitor,
            name: String::from_utf16_lossy(&info.szDevice[..len]),
            scale: if dpi == 0 && dx > 0 {
                dx as f32 / 96.0
            } else {
                1.0
            },
            refresh_hz: refresh_of(&info.szDevice),
        });
    }
    TRUE
}

fn monitors() -> Vec<Monitor> {
    let mut list: Vec<Monitor> = Vec::new();
    // SAFETY: the callback only runs during this call and `list` outlives it.
    unsafe {
        EnumDisplayMonitors(
            null_mut(),
            std::ptr::null(),
            Some(collect_monitor),
            &mut list as *mut _ as LPARAM,
        );
    }
    list
}

/// Copy `rect` of the screen into an RGBA image.
fn blit(screen: HDC, rect: &RECT) -> Result<image::RgbaImage, CaptureError> {
    let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
    if w <= 0 || h <= 0 {
        return Err(CaptureError::Failed("a monitor has no area".into()));
    }
    // SAFETY: GDI objects are created, used and deleted in this block; every
    // failure path releases what was made before it.
    unsafe {
        let mem = CreateCompatibleDC(screen);
        if mem.is_null() {
            return Err(CaptureError::Failed("CreateCompatibleDC failed".into()));
        }
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            // Negative: rows run top to bottom.
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bits: *mut c_void = null_mut();
        let dib = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if dib.is_null() || bits.is_null() {
            DeleteDC(mem);
            return Err(CaptureError::Failed("CreateDIBSection failed".into()));
        }
        let old = SelectObject(mem, dib);
        // CAPTUREBLT includes layered (transparent, topmost) windows.
        let ok = BitBlt(
            mem,
            0,
            0,
            w,
            h,
            screen,
            rect.left,
            rect.top,
            SRCCOPY | CAPTUREBLT,
        );
        let image = (ok != 0).then(|| {
            let len = w as usize * h as usize * 4;
            bgra_to_rgba(
                std::slice::from_raw_parts(bits as *const u8, len),
                w as u32,
                h as u32,
                w as usize * 4,
            )
        });
        SelectObject(mem, old);
        DeleteObject(dib);
        DeleteDC(mem);
        image.ok_or_else(|| CaptureError::Failed("BitBlt failed".into()))
    }
}

pub fn grab() -> Result<DesktopShot, CaptureError> {
    let _dpi = DpiScope::enter();
    let list = monitors();
    if list.is_empty() {
        return Err(CaptureError::Failed("no monitors found".into()));
    }
    // SAFETY: the screen DC is released below on every path.
    let screen = unsafe { GetDC(null_mut()) };
    if screen.is_null() {
        return Err(CaptureError::Failed("GetDC failed".into()));
    }
    let shots: Result<Vec<MonitorShot>, CaptureError> = list
        .iter()
        .map(|m| {
            Ok(MonitorShot {
                layout: DRect {
                    x: m.rect.left as f32,
                    y: m.rect.top as f32,
                    w: (m.rect.right - m.rect.left) as f32,
                    h: (m.rect.bottom - m.rect.top) as f32,
                },
                image: blit(screen, &m.rect)?,
                name: m.name.clone(),
                scale: m.scale,
                refresh_hz: m.refresh_hz,
            })
        })
        .collect();
    // SAFETY: releases the DC taken above.
    unsafe { ReleaseDC(null_mut(), screen) };
    Ok(DesktopShot { monitors: shots? })
}

/// Each monitor's layout (physical pixels) and refresh rate, in the order
/// `EnumDisplayMonitors` gives, without taking a picture.
pub(crate) fn monitor_list() -> Vec<(DRect, Option<u32>)> {
    let _dpi = DpiScope::enter();
    monitors()
        .into_iter()
        .map(|m| {
            let layout = DRect {
                x: m.rect.left as f32,
                y: m.rect.top as f32,
                w: (m.rect.right - m.rect.left) as f32,
                h: (m.rect.bottom - m.rect.top) as f32,
            };
            (layout, m.refresh_hz)
        })
        .collect()
}

unsafe extern "system" fn collect_window(hwnd: HWND, data: LPARAM) -> i32 {
    // SAFETY: `data` is the `Vec<WindowRect>` `window_rects` passed.
    let list = unsafe { &mut *(data as *mut Vec<WindowRect>) };
    if let Some(w) = unsafe { pickable(hwnd) } {
        list.push(w);
    }
    TRUE
}

/// The window as the picker should offer it, or `None` for anything a person
/// would not think of as a window on screen.
///
/// # Safety
/// `hwnd` is a handle from `EnumWindows`.
unsafe fn pickable(hwnd: HWND) -> Option<WindowRect> {
    unsafe {
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
            return None;
        }
        // Windows on another virtual desktop and suspended UWP shells are
        // "visible" yet cloaked.
        let mut cloaked = 0u32;
        let hr = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED as u32,
            &mut cloaked as *mut _ as *mut c_void,
            size_of::<u32>() as u32,
        );
        if hr == 0 && cloaked != 0 {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == GetCurrentProcessId() {
            return None;
        }
        let mut class = [0u16; 64];
        let n = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        // The desktop is the background, not a window to pick.
        if class == "Progman" || class == "WorkerW" {
            return None;
        }
        // The visible frame, without the invisible resize border.
        let mut r: RECT = std::mem::zeroed();
        let hr = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            &mut r as *mut _ as *mut c_void,
            size_of::<RECT>() as u32,
        );
        if hr != 0 || r.right <= r.left || r.bottom <= r.top {
            return None;
        }
        let mut title = [0u16; 256];
        let n = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32).max(0) as usize;
        Some(WindowRect {
            rect: DRect {
                x: r.left as f32,
                y: r.top as f32,
                w: (r.right - r.left) as f32,
                h: (r.bottom - r.top) as f32,
            },
            title: String::from_utf16_lossy(&title[..n]),
        })
    }
}

/// Top-level windows, topmost first (`EnumWindows` walks the z-order down).
pub fn window_rects(_shot: &DesktopShot) -> Vec<WindowRect> {
    let _dpi = DpiScope::enter();
    let mut list: Vec<WindowRect> = Vec::new();
    // SAFETY: the callback only runs during this call and `list` outlives it.
    unsafe { EnumWindows(Some(collect_window), &mut list as *mut _ as LPARAM) };
    list
}
