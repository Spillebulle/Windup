//! Simulated paste operations across platforms.

use crate::platform::Target;
use clipboard_rs::common::RustImage;
use clipboard_rs::{Clipboard, ClipboardContext, RustImageData};
use std::sync::atomic::{AtomicU16, Ordering};
use std::thread;
use std::time::Duration;
use windup_core::clipboard::{ClipboardContent, ClipboardEntry};

#[cfg(target_os = "windows")]
pub fn set_clipboard_text(text: &str) -> bool {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock};

    const GMEM_MOVEABLE: u32 = 0x0002;
    const CF_UNICODETEXT: u32 = 13;

    unsafe {
        // Try up to 10 times to open the clipboard with short backoff in case another process holds it
        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(std::ptr::null_mut()) != 0 {
                opened = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if !opened {
            return false;
        }

        EmptyClipboard();

        let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = utf16.len() * std::mem::size_of::<u16>();
        let h_mem = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if !h_mem.is_null() {
            let ptr = GlobalLock(h_mem);
            if !ptr.is_null() {
                std::ptr::copy_nonoverlapping(utf16.as_ptr() as *const u8, ptr as *mut u8, bytes);
                GlobalUnlock(h_mem);
                SetClipboardData(CF_UNICODETEXT, h_mem);
            }
        }

        CloseClipboard();
        true
    }
}

#[cfg(not(target_os = "windows"))]
pub fn set_clipboard_text(text: &str) -> bool {
    #[cfg(target_os = "linux")]
    if crate::linux::set_text(text, false) {
        return true;
    }
    if let Ok(ctx) = ClipboardContext::new() {
        ctx.set_text(text.to_string()).is_ok()
    } else {
        false
    }
}

pub fn populate_clipboard(entry: &ClipboardEntry) {
    #[cfg(target_os = "linux")]
    if crate::linux::set_entry(entry, false) {
        return;
    }
    match &entry.content {
        ClipboardContent::Text { text, .. } => {
            set_clipboard_text(text);
        }
        ClipboardContent::Image {
            rgba,
            width,
            height,
            ..
        } => {
            set_clipboard_image(
                image::RgbaImage::from_raw(*width, *height, rgba.clone()).unwrap_or_default(),
            );
        }
    }
}

/// Puts a picture on the clipboard. False when the clipboard would not take it.
pub fn set_clipboard_image(img: image::RgbaImage) -> bool {
    let Ok(ctx) = ClipboardContext::new() else {
        return false;
    };
    let img = RustImageData::from_dynamic_image(image::DynamicImage::ImageRgba8(img));
    ctx.set_image(img).is_ok()
}

pub fn paste_entry_to_target(target: Target, entry: &ClipboardEntry) {
    // The Wayland service sets the clipboard and presses Ctrl+V in order on
    // its own thread.
    #[cfg(target_os = "linux")]
    if crate::linux::set_entry(entry, true) {
        return;
    }
    populate_clipboard(entry);
    dispatch_paste_to_target(target);
}

pub fn paste_plain_to_target(target: Target, text: &str) {
    #[cfg(target_os = "linux")]
    if crate::linux::set_text(text, true) {
        return;
    }
    set_clipboard_text(text);
    dispatch_paste_to_target(target);
}

pub fn paste_current_clipboard_as_plain(target: Target) {
    #[cfg(target_os = "linux")]
    if crate::linux::paste_plain_current() {
        return;
    }
    if let Ok(ctx) = ClipboardContext::new()
        && let Ok(text) = ctx.get_text()
    {
        set_clipboard_text(&text);
        dispatch_paste_to_target(target);
    }
}

/// Virtual-key code of the key bound to Paste plain text; the paste waits for
/// it to be physically released before sending Ctrl+V.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
static TRIGGER_VK: AtomicU16 = AtomicU16::new(0x56);

/// Called when the bindings change, with the VK of the Paste plain text key
/// (V if it is not bound).
pub fn set_trigger_vk(vk: Option<u16>) {
    TRIGGER_VK.store(vk.unwrap_or(0x56), Ordering::Relaxed);
}

/// Modifier virtual keys, generic and per side: Shift, Ctrl, Alt, Win.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
const MODIFIER_VKS: [u16; 11] = [
    0x10, 0xA0, 0xA1, 0x11, 0xA2, 0xA3, 0x12, 0xA4, 0xA5, 0x5B, 0x5C,
];

/// Keys that must be up before the simulated Ctrl+V: the shortcut's own key,
/// Enter, and every modifier the shortcut might have used.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn release_wait_keys(trigger_vk: u16) -> Vec<u16> {
    let mut keys = vec![trigger_vk, 0x0D];
    keys.extend(MODIFIER_VKS);
    keys
}

pub fn dispatch_paste_to_target(target: Target) {
    thread::spawn(move || {
        #[cfg(target_os = "windows")]
        let target_hwnd = match target {
            Target::Hwnd(h) => h,
            _ => 0,
        };
        #[cfg(target_os = "windows")]
        unsafe {
            use windows_sys::Win32::Foundation::HWND;
            use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
                GetAsyncKeyState, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
                VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_V,
            };
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId,
                SetForegroundWindow,
            };

            // 1. If target_hwnd is non-zero, reliably restore foreground focus using AttachThreadInput
            if target_hwnd != 0 {
                let hwnd = target_hwnd as HWND;
                let current_thread = GetCurrentThreadId();
                let target_thread = GetWindowThreadProcessId(hwnd, std::ptr::null_mut());
                let foreground_hwnd = GetForegroundWindow();
                let foreground_thread =
                    GetWindowThreadProcessId(foreground_hwnd, std::ptr::null_mut());

                if foreground_thread != 0 && current_thread != foreground_thread {
                    AttachThreadInput(current_thread, foreground_thread, 1);
                }
                if target_thread != 0 && current_thread != target_thread {
                    AttachThreadInput(current_thread, target_thread, 1);
                }

                SetForegroundWindow(hwnd);
                BringWindowToTop(hwnd);

                if foreground_thread != 0 && current_thread != foreground_thread {
                    AttachThreadInput(current_thread, foreground_thread, 0);
                }
                if target_thread != 0 && current_thread != target_thread {
                    AttachThreadInput(current_thread, target_thread, 0);
                }
            }

            // 2. Wait for the shortcut's keys (its key, Enter, Win, Ctrl, Alt, Shift) to be fully released so they don't leak or collide
            let wait_keys = release_wait_keys(TRIGGER_VK.load(Ordering::Relaxed));
            let is_down = |vk: u16| GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0;
            let start = std::time::Instant::now();
            loop {
                if !wait_keys.iter().any(|&vk| is_down(vk)) {
                    break;
                }
                if start.elapsed() > Duration::from_millis(300) {
                    break;
                }
                thread::sleep(Duration::from_millis(15));
            }

            // Still held at the cap: release them for the target, or it would
            // see Ctrl+Alt+V, Ctrl+Shift+V and so on.
            let stuck: Vec<u16> = [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
                .into_iter()
                .filter(|&vk| is_down(vk))
                .collect();
            if !stuck.is_empty() {
                let mut ups: Vec<INPUT> = stuck
                    .iter()
                    .map(|&vk| {
                        let mut i: INPUT = std::mem::zeroed();
                        i.r#type = INPUT_KEYBOARD;
                        i.Anonymous.ki = KEYBDINPUT {
                            wVk: vk,
                            wScan: 0,
                            dwFlags: KEYEVENTF_KEYUP,
                            time: 0,
                            dwExtraInfo: 0,
                        };
                        i
                    })
                    .collect();
                SendInput(
                    ups.len() as u32,
                    ups.as_mut_ptr(),
                    std::mem::size_of::<INPUT>() as i32,
                );
            }

            // Allow the target application message loop to settle its newly acquired focus
            thread::sleep(Duration::from_millis(60));

            // 3. Synthesize clean Ctrl+V
            let mut inputs: [INPUT; 4] = std::mem::zeroed();

            inputs[0].r#type = INPUT_KEYBOARD;
            inputs[0].Anonymous.ki = KEYBDINPUT {
                wVk: VK_CONTROL,
                wScan: 0,
                dwFlags: 0,
                time: 0,
                dwExtraInfo: 0,
            };

            inputs[1].r#type = INPUT_KEYBOARD;
            inputs[1].Anonymous.ki = KEYBDINPUT {
                wVk: VK_V,
                wScan: 0,
                dwFlags: 0,
                time: 0,
                dwExtraInfo: 0,
            };

            inputs[2].r#type = INPUT_KEYBOARD;
            inputs[2].Anonymous.ki = KEYBDINPUT {
                wVk: VK_V,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            };

            inputs[3].r#type = INPUT_KEYBOARD;
            inputs[3].Anonymous.ki = KEYBDINPUT {
                wVk: VK_CONTROL,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            };

            SendInput(4, inputs.as_mut_ptr(), std::mem::size_of::<INPUT>() as i32);
        }

        #[cfg(target_os = "linux")]
        {
            use crate::platform::PasteVia;
            let via = crate::linux::runtime().map_or(PasteVia::XTest, |r| r.plan.paste);
            match via {
                PasteVia::Portal => {
                    crate::linux::portal::paste_ctrl_v();
                    return;
                }
                PasteVia::VirtualKeyboard => {
                    if let Some(service) = crate::linux::runtime().and_then(|r| r.service.as_ref())
                    {
                        service.press_ctrl_v();
                    }
                    return;
                }
                _ => {}
            }
            if let Target::X11(window) = target {
                crate::linux::x11::activate(window);
            }
        }
        #[cfg(target_os = "linux")]
        let held = crate::linux::x11::wait_keys_released(Duration::from_millis(300));

        #[cfg(not(target_os = "windows"))]
        {
            #[cfg(not(target_os = "linux"))]
            let _ = target;
            thread::sleep(Duration::from_millis(80));
            use enigo::{Direction, Enigo, Key, Keyboard, Settings};
            if let Ok(mut enigo) = Enigo::new(&Settings::default()) {
                #[cfg(target_os = "macos")]
                let modifier = Key::Meta;
                #[cfg(not(target_os = "macos"))]
                let modifier = Key::Control;

                // Still held at the cap: release them for the target, or it
                // would see Ctrl+Alt+V, Ctrl+Shift+V and so on.
                #[cfg(target_os = "linux")]
                for (down, key) in [
                    (held.shift, Key::Shift),
                    (held.alt, Key::Alt),
                    (held.super_, Key::Meta),
                ] {
                    if down {
                        let _ = enigo.key(key, Direction::Release);
                    }
                }
                let _ = enigo.key(modifier, Direction::Press);
                let _ = enigo.key(Key::Unicode('v'), Direction::Click);
                let _ = enigo.key(modifier, Direction::Release);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_the_key_and_every_modifier() {
        // Alt+Shift+P
        let keys = release_wait_keys(0x50);
        for vk in [0x50, 0x12, 0x10, 0x11, 0x5B, 0x5C, 0x0D] {
            assert!(keys.contains(&vk), "missing {vk:#x}");
        }
        // Per-side codes too.
        for vk in [0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5] {
            assert!(keys.contains(&vk), "missing {vk:#x}");
        }
        assert!(!keys.contains(&0x56));
    }
}
