//! Global hotkey listener.
//! On Windows, uses low-level keyboard hook to intercept Win+V and Win+Shift+V reliably.
//! On X11 and macOS, uses the global-hotkey crate. On Wayland nothing can grab
//! a key, so the events come from the socket instead (`linux::ipc`), sent
//! through [`HotkeyListener::sender`].

use crate::platform::{HotkeysVia, Plan, Target};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyEvent {
    ClipboardPopup {
        target: Target,
    },
    PastePlainText {
        target: Target,
    },
    /// Bring the settings window up (`windup` run a second time).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Show,
    /// config.json changed under us.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Reload,
}

pub struct HotkeyListener {
    rx: Receiver<HotkeyEvent>,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    tx: Sender<HotkeyEvent>,
    _running: Arc<AtomicBool>,
}

impl HotkeyListener {
    pub fn new(plan: &Plan) -> Self {
        let (tx, rx) = channel();
        let running = Arc::new(AtomicBool::new(true));

        match plan.hotkeys {
            #[cfg(target_os = "windows")]
            HotkeysVia::Hook => start_windows_hook(tx.clone(), Arc::clone(&running)),
            #[cfg(not(target_os = "windows"))]
            HotkeysVia::XGrab => start_crossplatform_hotkeys(tx.clone(), Arc::clone(&running)),
            _ => {}
        }

        Self {
            rx,
            tx,
            _running: running,
        }
    }

    /// For events that do not come from a key grab: the socket, the portal.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn sender(&self) -> Sender<HotkeyEvent> {
        self.tx.clone()
    }

    pub fn try_recv(&self) -> Option<HotkeyEvent> {
        self.rx.try_recv().ok()
    }
}

#[cfg(target_os = "windows")]
static EVENT_SENDER: std::sync::Mutex<Option<Sender<HotkeyEvent>>> = std::sync::Mutex::new(None);

#[cfg(target_os = "windows")]
fn start_windows_hook(tx: Sender<HotkeyEvent>, running: Arc<AtomicBool>) {
    *EVENT_SENDER.lock().unwrap() = Some(tx);

    thread::spawn(move || {
        use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GetMessageW, MSG, SetWindowsHookExW,
            UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_SYSKEYDOWN,
        };

        unsafe extern "system" fn low_level_keyboard_proc(
            code: i32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            unsafe {
                use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
                    GetAsyncKeyState, VK_LSHIFT, VK_LWIN, VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_V,
                };
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    GetForegroundWindow, KBDLLHOOKSTRUCT,
                };

                if code >= 0 {
                    let p = *(lparam as *const KBDLLHOOKSTRUCT);
                    // Ignore injected/simulated keystrokes from SendInput to prevent loops
                    if p.flags & 1 != 0 {
                        return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam);
                    }

                    let vk = p.vkCode as u16;
                    let is_down = wparam == WM_KEYDOWN as usize || wparam == WM_SYSKEYDOWN as usize;

                    if vk == VK_V && is_down {
                        // Query real hardware key states directly via GetAsyncKeyState to prevent desync
                        let win_down = (GetAsyncKeyState(VK_LWIN as i32) as u16 & 0x8000 != 0)
                            || (GetAsyncKeyState(VK_RWIN as i32) as u16 & 0x8000 != 0);

                        if win_down {
                            let shift_down = (GetAsyncKeyState(VK_SHIFT as i32) as u16 & 0x8000
                                != 0)
                                || (GetAsyncKeyState(VK_LSHIFT as i32) as u16 & 0x8000 != 0)
                                || (GetAsyncKeyState(VK_RSHIFT as i32) as u16 & 0x8000 != 0);

                            let target = Target::Hwnd(GetForegroundWindow() as isize);

                            // Explicitly permit this process to take foreground window
                            windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
                                windows_sys::Win32::System::Threading::GetCurrentProcessId(),
                            );

                            if let Ok(guard) = EVENT_SENDER.lock()
                                && let Some(sender) = guard.as_ref()
                            {
                                if shift_down {
                                    let _ = sender.send(HotkeyEvent::PastePlainText { target });
                                } else {
                                    let _ = sender.send(HotkeyEvent::ClipboardPopup { target });
                                }
                            }

                            // Send benign key event to satisfy Windows Shell that Win was used with another key
                            use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
                                KEYEVENTF_KEYUP, keybd_event,
                            };
                            keybd_event(0xFF, 0, KEYEVENTF_KEYUP, 0);

                            return 1; // Intercept key event, preventing OS Start Menu / native clipboard
                        }
                    }
                }

                CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
            }
        }

        unsafe {
            let hook = SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(low_level_keyboard_proc),
                std::ptr::null_mut(),
                0,
            );

            if hook.is_null() {
                log::error!("Failed to install low-level keyboard hook on Windows");
                return;
            }

            let mut msg: MSG = std::mem::zeroed();
            while running.load(Ordering::SeqCst)
                && GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0
            {
                DispatchMessageW(&msg);
            }

            UnhookWindowsHookEx(hook);
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn start_crossplatform_hotkeys(tx: Sender<HotkeyEvent>, running: Arc<AtomicBool>) {
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager};

    thread::spawn(move || {
        let Ok(manager) = GlobalHotKeyManager::new() else {
            log::error!("Failed to initialize GlobalHotKeyManager");
            return;
        };

        let hk_v = HotKey::new(Some(Modifiers::SUPER), Code::KeyV);
        let hk_shift_v = HotKey::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyV);

        let id_v = hk_v.id();
        let id_shift_v = hk_shift_v.id();

        // Another program (often the desktop itself) may own Super+V; then
        // the command-line route in docs/linux.md is the way in.
        for hk in [hk_v, hk_shift_v] {
            if let Err(e) = manager.register(hk) {
                log::warn!("could not grab {hk:?}: {e}");
            }
        }

        let receiver = GlobalHotKeyEvent::receiver();

        while running.load(Ordering::SeqCst) {
            if let Ok(event) = receiver.recv_timeout(std::time::Duration::from_millis(100)) {
                // Only key presses: global-hotkey reports the release too.
                if event.state != global_hotkey::HotKeyState::Pressed {
                    continue;
                }
                let target = focused_window();
                if event.id == id_v {
                    let _ = tx.send(HotkeyEvent::ClipboardPopup { target });
                } else if event.id == id_shift_v {
                    let _ = tx.send(HotkeyEvent::PastePlainText { target });
                }
            }
        }
    });
}

#[cfg(target_os = "linux")]
fn focused_window() -> Target {
    crate::linux::x11::active_window().map_or(Target::None, Target::X11)
}

#[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
fn focused_window() -> Target {
    Target::None
}
