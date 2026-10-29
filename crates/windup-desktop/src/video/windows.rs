//! Windows frame sources: `ddagrab` for a rectangle on one monitor, `gdigrab`
//! for one that spans several.

use windup_core::capture::geometry::DRect;
use windup_core::video::{Source, even_size};

use super::run::{FrameSource, SourceFactory};

/// Where a recording's picture comes from on Windows.
pub(crate) struct WindowsFactory {
    primary: Source,
    fallback: Source,
}

impl WindowsFactory {
    /// `rect` is in desktop units (physical pixels of the virtual screen);
    /// `monitors` are the monitors' layouts and `outputs` the desktop
    /// coordinates of the default DXGI adapter's outputs, in DXGI's order.
    /// `None` when the rectangle is too small to record.
    pub(crate) fn new(
        rect: DRect,
        monitors: &[DRect],
        outputs: &[DRect],
    ) -> Option<WindowsFactory> {
        let rect = rect.normalised();
        let (w, h) = even_size(rect.w.round() as u32, rect.h.round() as u32)?;
        let (x, y) = (rect.x.round() as i32, rect.y.round() as i32);
        let gdigrab = Source::Gdigrab { x, y, w, h };
        // ddagrab only reads outputs of the default adapter. A monitor that
        // is not one of them is recorded through gdigrab straight away.
        let primary = monitor_holding(x, y, w, h, monitors)
            .and_then(|m| Some((ddagrab_index(outputs, m)?, m)))
            .map_or_else(
                || gdigrab.clone(),
                |(output, m)| Source::Ddagrab {
                    output,
                    x: x - m.x.round() as i32,
                    y: y - m.y.round() as i32,
                    w,
                    h,
                },
            );
        Some(WindowsFactory {
            primary,
            fallback: gdigrab,
        })
    }
}

/// The monitor that wholly holds the rectangle.
fn monitor_holding(x: i32, y: i32, w: u32, h: u32, monitors: &[DRect]) -> Option<DRect> {
    let (r, b) = (x + w as i32, y + h as i32);
    monitors.iter().copied().find(|m| {
        x >= m.x.round() as i32
            && y >= m.y.round() as i32
            && r <= m.right().round() as i32
            && b <= m.bottom().round() as i32
    })
}

/// ddagrab's `output_idx` for a monitor: the position of the DXGI output
/// covering the same desktop rectangle, if the default adapter has one.
fn ddagrab_index(outputs: &[DRect], monitor: DRect) -> Option<u32> {
    let key = |r: &DRect| {
        (
            r.x.round() as i32,
            r.y.round() as i32,
            r.w.round() as i32,
            r.h.round() as i32,
        )
    };
    outputs
        .iter()
        .position(|o| key(o) == key(&monitor))
        .map(|i| i as u32)
}

/// The desktop rectangles of the default DXGI adapter's outputs, in the order
/// ddagrab's `output_idx` counts them. Empty when DXGI will not say.
pub(crate) fn dxgi_outputs() -> Vec<DRect> {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::core::GUID;

    // windows-sys 0.61 has no DXGI module, so the one call and the one struct
    // are declared here.
    #[link(name = "dxgi", kind = "raw-dylib")]
    unsafe extern "system" {
        fn CreateDXGIFactory1(riid: *const GUID, factory: *mut *mut c_void) -> i32;
    }
    #[repr(C)]
    struct DXGI_OUTPUT_DESC {
        device_name: [u16; 32],
        desktop_coordinates: RECT,
        attached_to_desktop: i32,
        rotation: u32,
        monitor: isize,
    }

    type Call = unsafe extern "system" fn();
    // A COM object is a pointer to a pointer to a vtable; only the slots used
    // here are taken, by their index.
    unsafe fn slot(obj: *mut c_void, index: usize) -> Call {
        // SAFETY: the caller passes a live COM interface, whose first word
        // points at a vtable at least `index + 1` entries long.
        unsafe { *(*(obj as *const *const Call)).add(index) }
    }
    unsafe fn release(obj: *mut c_void) {
        // SAFETY: IUnknown::Release is slot 2 of every interface.
        unsafe {
            let f: unsafe extern "system" fn(*mut c_void) -> u32 =
                std::mem::transmute(slot(obj, 2));
            f(obj);
        }
    }
    type Enum = unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32;

    const IID_IDXGIFACTORY1: GUID = GUID::from_u128(0x770aae78_f26f_4dba_a829_253c83d1b387);
    // Vtable slots: IDXGIFactory1::EnumAdapters1, IDXGIAdapter::EnumOutputs,
    // IDXGIOutput::GetDesc.
    const ENUM_ADAPTERS1: usize = 12;
    const ENUM_OUTPUTS: usize = 7;
    const OUTPUT_GET_DESC: usize = 7;

    // Per-monitor aware, so the coordinates are physical like the layouts.
    let _dpi = crate::capture::windows::DpiScope::enter();
    let mut list = Vec::new();
    // SAFETY: every interface obtained below is released before returning,
    // and each call matches the signature of the vtable slot it is taken from.
    unsafe {
        let mut factory: *mut c_void = std::ptr::null_mut();
        if CreateDXGIFactory1(&IID_IDXGIFACTORY1, &mut factory) < 0 || factory.is_null() {
            return list;
        }
        let enum_adapters: Enum = std::mem::transmute(slot(factory, ENUM_ADAPTERS1));
        let mut adapter: *mut c_void = std::ptr::null_mut();
        if enum_adapters(factory, 0, &mut adapter) >= 0 && !adapter.is_null() {
            let enum_outputs: Enum = std::mem::transmute(slot(adapter, ENUM_OUTPUTS));
            for i in 0.. {
                let mut output: *mut c_void = std::ptr::null_mut();
                if enum_outputs(adapter, i, &mut output) < 0 || output.is_null() {
                    break;
                }
                let get_desc: unsafe extern "system" fn(*mut c_void, *mut DXGI_OUTPUT_DESC) -> i32 =
                    std::mem::transmute(slot(output, OUTPUT_GET_DESC));
                let mut desc: DXGI_OUTPUT_DESC = std::mem::zeroed();
                // An output we cannot describe still takes its index.
                let r = if get_desc(output, &mut desc) >= 0 {
                    desc.desktop_coordinates
                } else {
                    std::mem::zeroed()
                };
                list.push(DRect {
                    x: r.left as f32,
                    y: r.top as f32,
                    w: (r.right - r.left) as f32,
                    h: (r.bottom - r.top) as f32,
                });
                release(output);
            }
            release(adapter);
        }
        release(factory);
    }
    list
}

/// The value for `__COMPAT_LAYER` that makes a gdigrab ffmpeg DPI aware,
/// keeping any layers already set. ffmpeg.exe has no DPI manifest, so on a
/// scaled display gdigrab would otherwise see virtualised coordinates.
pub(crate) fn compat_layer(existing: Option<&str>) -> String {
    match existing.map(str::trim).filter(|e| !e.is_empty()) {
        Some(e) if e.to_ascii_uppercase().contains("HIGHDPIAWARE") => e.to_owned(),
        Some(e) => format!("{e} HIGHDPIAWARE"),
        None => "HIGHDPIAWARE".to_owned(),
    }
}

impl SourceFactory for WindowsFactory {
    fn source(&self) -> Source {
        self.primary.clone()
    }

    fn open_pipe(&self) -> Option<Box<dyn FrameSource>> {
        None
    }

    fn fallback(&self) -> Option<Source> {
        matches!(self.primary, Source::Ddagrab { .. }).then(|| self.fallback.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> DRect {
        DRect { x, y, w, h }
    }

    /// A 1920x1080 monitor at the origin, a second to its right, and a third
    /// to the left of the first (negative coordinates).
    fn desk() -> Vec<DRect> {
        vec![
            r(0.0, 0.0, 1920.0, 1080.0),
            r(1920.0, 0.0, 2560.0, 1440.0),
            r(-1280.0, 100.0, 1280.0, 1024.0),
        ]
    }

    /// DXGI's order differs from the monitors' here: the left monitor is its
    /// first output and the origin one its second.
    fn dxgi() -> Vec<DRect> {
        let d = desk();
        vec![d[2], d[0], d[1]]
    }

    #[test]
    fn dxgi_order_decides_the_output_index() {
        assert_eq!(ddagrab_index(&dxgi(), desk()[0]), Some(1));
        assert_eq!(ddagrab_index(&dxgi(), desk()[2]), Some(0));
        assert_eq!(ddagrab_index(&dxgi(), r(0.0, 0.0, 10.0, 10.0)), None);
    }

    #[test]
    fn a_monitor_on_another_adapter_is_gdigrab_at_once() {
        // Only the origin monitor is on the default adapter.
        let one = [desk()[0]];
        let f = WindowsFactory::new(r(2000.0, 50.0, 800.0, 600.0), &desk(), &one).unwrap();
        assert!(matches!(f.source(), Source::Gdigrab { .. }));
        assert_eq!(f.fallback(), None);
        let f = WindowsFactory::new(r(2000.0, 50.0, 800.0, 600.0), &desk(), &[]).unwrap();
        assert!(matches!(f.source(), Source::Gdigrab { .. }));
    }

    #[test]
    fn the_dpi_layer_is_added_once_and_keeps_others() {
        assert_eq!(compat_layer(None), "HIGHDPIAWARE");
        assert_eq!(compat_layer(Some("")), "HIGHDPIAWARE");
        assert_eq!(
            compat_layer(Some("~ RUNASADMIN")),
            "~ RUNASADMIN HIGHDPIAWARE"
        );
        assert_eq!(compat_layer(Some("HighDpiAware")), "HighDpiAware");
    }

    #[test]
    fn a_rectangle_on_one_monitor_is_ddagrab_relative_to_it() {
        let f = WindowsFactory::new(r(2000.0, 50.0, 801.0, 600.0), &desk(), &dxgi()).unwrap();
        assert_eq!(
            f.source(),
            Source::Ddagrab {
                output: 2,
                x: 80,
                y: 50,
                w: 800,
                h: 600
            }
        );
        assert_eq!(
            f.fallback(),
            Some(Source::Gdigrab {
                x: 2000,
                y: 50,
                w: 800,
                h: 600
            })
        );
    }

    #[test]
    fn negative_origins_work() {
        let f = WindowsFactory::new(r(-1200.0, 200.0, 400.0, 300.0), &desk(), &dxgi()).unwrap();
        assert_eq!(
            f.source(),
            Source::Ddagrab {
                output: 0,
                x: 80,
                y: 100,
                w: 400,
                h: 300
            }
        );
    }

    #[test]
    fn a_rectangle_across_monitors_is_gdigrab_in_virtual_desktop_pixels() {
        let f = WindowsFactory::new(r(1800.0, 10.0, 400.0, 300.0), &desk(), &dxgi()).unwrap();
        assert_eq!(
            f.source(),
            Source::Gdigrab {
                x: 1800,
                y: 10,
                w: 400,
                h: 300
            }
        );
        assert_eq!(f.fallback(), None, "nothing to fall back to");
    }

    #[test]
    fn a_whole_monitor_fits_and_a_sliver_too_small_is_refused() {
        let f = WindowsFactory::new(desk()[1], &desk(), &dxgi()).unwrap();
        assert!(matches!(
            f.source(),
            Source::Ddagrab {
                output: 2,
                x: 0,
                y: 0,
                w: 2560,
                h: 1440
            }
        ));
        assert!(WindowsFactory::new(r(0.0, 0.0, 1.0, 100.0), &desk(), &dxgi()).is_none());
    }
}
