//! Simulated paste operations across platforms.

use clipboard_rs::common::RustImage;
use clipboard_rs::{Clipboard, ClipboardContext, RustImageData};
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
    if let Ok(ctx) = ClipboardContext::new() {
        ctx.set_text(text.to_string()).is_ok()
    } else {
        false
    }
}

pub fn populate_clipboard(entry: &ClipboardEntry) {
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
            if let Ok(ctx) = ClipboardContext::new() {
                let img = RustImageData::from_dynamic_image(image::DynamicImage::ImageRgba8(
                    image::RgbaImage::from_raw(*width, *height, rgba.clone()).unwrap_or_default(),
                ));
                let _ = ctx.set_image(img);
            }
        }
    }
}

pub fn paste_entry_to_target(target_hwnd: isize, entry: &ClipboardEntry) {
    populate_clipboard(entry);
    dispatch_paste_to_target(target_hwnd);
}

pub fn paste_plain_to_target(target_hwnd: isize, text: &str) {
    set_clipboard_text(text);
    dispatch_paste_to_target(target_hwnd);
}

pub fn paste_current_clipboard_as_plain(target_hwnd: isize) {
    if let Ok(ctx) = ClipboardContext::new()
        && let Ok(text) = ctx.get_text()
    {
        set_clipboard_text(&text);
        dispatch_paste_to_target(target_hwnd);
    }
}

pub fn dispatch_paste_to_target(target_hwnd: isize) {
    thread::spawn(move || {
        #[cfg(target_os = "windows")]
        unsafe {
            use windows_sys::Win32::Foundation::HWND;
            use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
                GetAsyncKeyState, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
                VK_CONTROL, VK_LWIN, VK_RETURN, VK_RWIN, VK_V,
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

            // 2. Wait for physical keys (Enter, Win, V) to be fully released so they don't leak or collide
            let start = std::time::Instant::now();
            loop {
                let enter_down = GetAsyncKeyState(VK_RETURN as i32) as u16 & 0x8000 != 0;
                let win_down = (GetAsyncKeyState(VK_LWIN as i32) as u16 & 0x8000 != 0)
                    || (GetAsyncKeyState(VK_RWIN as i32) as u16 & 0x8000 != 0);
                let v_down = GetAsyncKeyState(VK_V as i32) as u16 & 0x8000 != 0;

                if !enter_down && !win_down && !v_down {
                    break;
                }
                if start.elapsed() > Duration::from_millis(300) {
                    break;
                }
                thread::sleep(Duration::from_millis(15));
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

        #[cfg(not(target_os = "windows"))]
        {
            thread::sleep(Duration::from_millis(80));
            use enigo::{Direction, Enigo, Key, Keyboard, Settings};
            if let Ok(mut enigo) = Enigo::new(&Settings::default()) {
                #[cfg(target_os = "macos")]
                let modifier = Key::Meta;
                #[cfg(not(target_os = "macos"))]
                let modifier = Key::Control;

                let _ = enigo.key(modifier, Direction::Press);
                let _ = enigo.key(Key::Unicode('v'), Direction::Click);
                let _ = enigo.key(modifier, Direction::Release);
            }
        }
    });
}
