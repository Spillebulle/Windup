//! Global hotkey listener.
//! On Windows, uses low-level keyboard hook to intercept the configured
//! shortcuts (Win+V and friends) reliably.
//! On X11 and macOS, uses the global-hotkey crate. On Wayland nothing can grab
//! a key, so the events come from the socket instead (`linux::ipc`), sent
//! through [`HotkeyListener::sender`].
//!
//! While a recording or a Steps session runs, its own keys are live too
//! (`hotkeys::session_bindings`: pause, stop and Esc, plus the step key for
//! Steps), and only then: [`HotkeyListener::set_live`].

use crate::platform::{HotkeysVia, Plan, Target};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use windup_core::hotkeys::{Action, LiveKind, live_bindings_for, windows_vk};
use windup_core::{Config, Shortcut, ShortcutModifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyEvent {
    ClipboardPopup {
        target: Target,
    },
    PastePlainText {
        target: Target,
    },
    Capture {
        target: Target,
    },
    Screenshot {
        target: Target,
    },
    /// The Video key: starts a recording, or stops the one that runs.
    Record {
        target: Target,
    },
    /// The Steps key: starts a Steps session, or stops the one that runs.
    /// No target: Steps opens nothing over the focused window.
    Steps,
    /// Add a step by hand (`windup --step`).
    Step,
    /// Stop a Steps session, and only that (`windup --steps-stop`).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    StepsStop,
    /// Pause or resume the recording (`windup --record-pause`).
    RecordPause,
    /// Stop the recording (`windup --record-stop`).
    RecordStop,
    /// Cancel the recording and discard it: Esc (`windup --record-cancel`).
    RecordCancel,
    /// Bring the settings window up (`windup` run a second time).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Show,
    /// config.json changed under us.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Reload,
}

impl HotkeyEvent {
    fn from_action(action: Action, target: Target) -> Self {
        match action {
            Action::ClipboardPopup => Self::ClipboardPopup { target },
            Action::PastePlain => Self::PastePlainText { target },
            Action::CaptureLauncher => Self::Capture { target },
            Action::Screenshot => Self::Screenshot { target },
            Action::Video => Self::Record { target },
            Action::Steps => Self::Steps,
            Action::Step => Self::Step,
            Action::RecordPause => Self::RecordPause,
            Action::RecordStop => Self::RecordStop,
            Action::RecordCancel => Self::RecordCancel,
        }
    }
}

pub struct HotkeyListener {
    rx: Receiver<HotkeyEvent>,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    tx: Sender<HotkeyEvent>,
    /// New binding lists for the grab thread (X11 and macOS).
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    rebind_tx: Option<Sender<Vec<(Shortcut, Action)>>>,
    /// Which session's keys are bound, as [`live_code`] gives it.
    live: AtomicU8,
    _running: Arc<AtomicBool>,
}

impl HotkeyListener {
    pub fn new(plan: &Plan, config: &Config) -> Self {
        let (tx, rx) = channel();
        let running = Arc::new(AtomicBool::new(true));
        #[allow(unused_mut)]
        let mut rebind_tx = None;
        let initial = live_bindings_for(config, None);

        match plan.hotkeys {
            #[cfg(target_os = "windows")]
            HotkeysVia::Hook => {
                set_windows_tables(config);
                start_windows_hook(tx.clone(), Arc::clone(&running));
            }
            #[cfg(not(target_os = "windows"))]
            HotkeysVia::XGrab => {
                let (btx, brx) = channel();
                let _ = btx.send(initial);
                rebind_tx = Some(btx);
                start_crossplatform_hotkeys(tx.clone(), brx, Arc::clone(&running));
            }
            _ => {
                let _ = initial;
            }
        }

        Self {
            rx,
            tx,
            rebind_tx,
            live: AtomicU8::new(live_code(None)),
            _running: running,
        }
    }

    /// Replace the active shortcuts with what the config now says.
    pub fn rebind(&self, config: &Config) {
        #[cfg(target_os = "windows")]
        set_windows_tables(config);
        if let Some(tx) = &self.rebind_tx {
            let live = live_kind(self.live.load(Ordering::SeqCst));
            let _ = tx.send(live_bindings_for(config, live));
        }
    }

    /// Binds the keys of the session `live` names (pause, stop, Esc, and
    /// the step key for Steps), and lets them go again with `None`: the
    /// Windows hook matches that kind's table only while `LIVE` names it,
    /// and X11 grabs them for that long. On Wayland the keys are commands,
    /// which the background host binds.
    pub fn set_live(&self, live: Option<LiveKind>, config: &Config) {
        let code = live_code(live);
        let was = self.live.swap(code, Ordering::SeqCst);
        if was == code {
            return;
        }
        #[cfg(target_os = "windows")]
        LIVE.store(code, Ordering::SeqCst);
        match (live, live_kind(was)) {
            (Some(LiveKind::Video), _) => log::info!("recording keys bound"),
            (Some(LiveKind::Steps), _) => log::info!("steps keys bound"),
            (None, Some(LiveKind::Steps)) => log::info!("steps keys released"),
            (None, _) => log::info!("recording keys released"),
        }
        if let Some(tx) = &self.rebind_tx {
            let _ = tx.send(live_bindings_for(config, live));
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

/// The shortcuts the hook acts on, as virtual-key code + exact modifiers.
/// Keys `windows_vk` does not know are left out.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn windows_table(bindings: &[(Shortcut, Action)]) -> Vec<(u16, ShortcutModifiers, Action)> {
    bindings
        .iter()
        .filter_map(|(s, a)| Some((windows_vk(&s.key)?, s.modifiers, *a)))
        .collect()
}

/// `HotkeyListener::live` and the hook's `LIVE`: 0 for no session, 1 for
/// a recording, 2 for Steps.
fn live_code(live: Option<LiveKind>) -> u8 {
    match live {
        None => 0,
        Some(LiveKind::Video) => 1,
        Some(LiveKind::Steps) => 2,
    }
}

fn live_kind(code: u8) -> Option<LiveKind> {
    match code {
        1 => Some(LiveKind::Video),
        2 => Some(LiveKind::Steps),
        _ => None,
    }
}

/// Keys as the hook matches them: virtual-key code, exact modifiers, action.
type Table = [(u16, ShortcutModifiers, Action)];

/// What a key press does, as the hook asks it: while a session is `live`,
/// that kind's keys first (`video` or `steps`, Esc among them, which is why
/// they are looked at only then), then the tools'.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn match_key(
    table: &Table,
    video: &Table,
    steps: &Table,
    live: Option<LiveKind>,
    vk: u16,
    held: ShortcutModifiers,
) -> Option<Action> {
    let session = match live {
        Some(LiveKind::Video) => Some(video),
        Some(LiveKind::Steps) => Some(steps),
        None => None,
    };
    session
        .and_then(|t| match_binding(t, vk, held))
        .or_else(|| match_binding(table, vk, held))
}

/// What a key press does: the binding for this key whose modifiers are
/// exactly the ones held.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn match_binding(
    table: &[(u16, ShortcutModifiers, Action)],
    vk: u16,
    held: ShortcutModifiers,
) -> Option<Action> {
    table
        .iter()
        .find(|(k, m, _)| *k == vk && *m == held)
        .map(|(_, _, a)| *a)
}

/// What the hook does with one key event.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// Let it through to the next hook and the focused app.
    Pass,
    /// Stop it, and act on the binding.
    Fire(Action),
    /// Stop it and do nothing: the rest of a press already acted on.
    Swallow,
}

/// The hook's decision for key `vk` going down or up, given the binding a
/// press matched. `held` is the keys whose press was swallowed and whose
/// release is still to come: their release is swallowed too, so the shell
/// never sees half a key (Windows 11 opens Snipping Tool on Print Screen's
/// key-up), and their auto-repeat stays swallowed even once the modifiers
/// no longer match.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn key_verdict(held: &mut Vec<u16>, vk: u16, is_down: bool, matched: Option<Action>) -> Verdict {
    let was_held = held.contains(&vk);
    match (is_down, matched) {
        (true, Some(action)) => {
            if !was_held {
                held.push(vk);
            }
            Verdict::Fire(action)
        }
        (true, None) if was_held => Verdict::Swallow,
        (false, _) if was_held => {
            held.retain(|k| *k != vk);
            Verdict::Swallow
        }
        _ => Verdict::Pass,
    }
}

/// global-hotkey key code for a canonical key name.
#[cfg_attr(all(target_os = "windows", not(test)), allow(dead_code))]
fn code_for(key: &str) -> Option<global_hotkey::hotkey::Code> {
    use global_hotkey::hotkey::Code;
    const LETTERS: [Code; 26] = [
        Code::KeyA,
        Code::KeyB,
        Code::KeyC,
        Code::KeyD,
        Code::KeyE,
        Code::KeyF,
        Code::KeyG,
        Code::KeyH,
        Code::KeyI,
        Code::KeyJ,
        Code::KeyK,
        Code::KeyL,
        Code::KeyM,
        Code::KeyN,
        Code::KeyO,
        Code::KeyP,
        Code::KeyQ,
        Code::KeyR,
        Code::KeyS,
        Code::KeyT,
        Code::KeyU,
        Code::KeyV,
        Code::KeyW,
        Code::KeyX,
        Code::KeyY,
        Code::KeyZ,
    ];
    const DIGITS: [Code; 10] = [
        Code::Digit0,
        Code::Digit1,
        Code::Digit2,
        Code::Digit3,
        Code::Digit4,
        Code::Digit5,
        Code::Digit6,
        Code::Digit7,
        Code::Digit8,
        Code::Digit9,
    ];
    const FUNCTION: [Code; 24] = [
        Code::F1,
        Code::F2,
        Code::F3,
        Code::F4,
        Code::F5,
        Code::F6,
        Code::F7,
        Code::F8,
        Code::F9,
        Code::F10,
        Code::F11,
        Code::F12,
        Code::F13,
        Code::F14,
        Code::F15,
        Code::F16,
        Code::F17,
        Code::F18,
        Code::F19,
        Code::F20,
        Code::F21,
        Code::F22,
        Code::F23,
        Code::F24,
    ];
    let key = key.to_uppercase();
    if let [c @ b'A'..=b'Z'] = key.as_bytes() {
        return Some(LETTERS[(c - b'A') as usize]);
    }
    if let [c @ b'0'..=b'9'] = key.as_bytes() {
        return Some(DIGITS[(c - b'0') as usize]);
    }
    if let Some(n) = key.strip_prefix('F').and_then(|d| d.parse::<usize>().ok())
        && (1..=24).contains(&n)
    {
        return Some(FUNCTION[n - 1]);
    }
    Some(match key.as_str() {
        windup_core::shortcut::PRINT_SCREEN => Code::PrintScreen,
        windup_core::shortcut::SCROLL_LOCK => Code::ScrollLock,
        windup_core::shortcut::PAUSE => Code::Pause,
        windup_core::shortcut::ESCAPE => Code::Escape,
        "SPACE" => Code::Space,
        "ENTER" => Code::Enter,
        "TAB" => Code::Tab,
        "INSERT" => Code::Insert,
        "DELETE" => Code::Delete,
        "HOME" => Code::Home,
        "END" => Code::End,
        "PAGEUP" => Code::PageUp,
        "PAGEDOWN" => Code::PageDown,
        _ => return None,
    })
}

#[cfg(target_os = "windows")]
static EVENT_SENDER: std::sync::Mutex<Option<Sender<HotkeyEvent>>> = std::sync::Mutex::new(None);

/// Read by the hook on every key press; written only when the config changes.
#[cfg(target_os = "windows")]
static BINDINGS: std::sync::RwLock<Vec<(u16, ShortcutModifiers, Action)>> =
    std::sync::RwLock::new(Vec::new());

/// The recording's keys, which the hook matches only while `LIVE` says a
/// recording runs.
#[cfg(target_os = "windows")]
static RECORDING_BINDINGS: std::sync::RwLock<Vec<(u16, ShortcutModifiers, Action)>> =
    std::sync::RwLock::new(Vec::new());

/// Steps' keys, which the hook matches only while `LIVE` says Steps runs.
#[cfg(target_os = "windows")]
static STEPS_BINDINGS: std::sync::RwLock<Vec<(u16, ShortcutModifiers, Action)>> =
    std::sync::RwLock::new(Vec::new());

/// `HotkeyListener::live`, for the hook, which is a plain function. One
/// hook per process, so one flag.
#[cfg(target_os = "windows")]
static LIVE: AtomicU8 = AtomicU8::new(0);

#[cfg(target_os = "windows")]
thread_local! {
    /// Keys whose press the hook swallowed, until their release (`key_verdict`).
    static HELD: std::cell::RefCell<Vec<u16>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(target_os = "windows")]
fn set_windows_tables(config: &Config) {
    use windup_core::hotkeys::session_bindings;
    if let Ok(mut table) = BINDINGS.write() {
        *table = windows_table(&live_bindings_for(config, None));
    }
    if let Ok(mut table) = RECORDING_BINDINGS.write() {
        *table = windows_table(&session_bindings(config, LiveKind::Video));
    }
    if let Ok(mut table) = STEPS_BINDINGS.write() {
        *table = windows_table(&session_bindings(config, LiveKind::Steps));
    }
}

#[cfg(target_os = "windows")]
fn start_windows_hook(tx: Sender<HotkeyEvent>, running: Arc<AtomicBool>) {
    // One hook per process: a second would share the first's `LIVE`.
    let first = EVENT_SENDER.lock().unwrap().replace(tx).is_none();
    debug_assert!(first, "the Windows hook was started twice");

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
                    GetAsyncKeyState, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN,
                    VK_MENU, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
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

                    let mut matched = None;
                    let mut win_down = false;
                    if is_down {
                        // Query real hardware key states directly via GetAsyncKeyState to prevent desync
                        let down = |vk: u16| GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0;
                        win_down = down(VK_LWIN) || down(VK_RWIN);
                        let held = ShortcutModifiers {
                            super_key: win_down,
                            ctrl: down(VK_CONTROL) || down(VK_LCONTROL) || down(VK_RCONTROL),
                            alt: down(VK_MENU) || down(VK_LMENU) || down(VK_RMENU),
                            shift: down(VK_SHIFT) || down(VK_LSHIFT) || down(VK_RSHIFT),
                        };

                        // try_read: the hook must never wait on a lock.
                        let live = live_kind(LIVE.load(Ordering::Relaxed));
                        matched = match (
                            BINDINGS.try_read(),
                            RECORDING_BINDINGS.try_read(),
                            STEPS_BINDINGS.try_read(),
                        ) {
                            (Ok(table), Ok(video), Ok(steps)) => {
                                match_key(&table, &video, &steps, live, vk, held)
                            }
                            (Ok(table), _, _) => match_binding(&table, vk, held),
                            _ => None,
                        };
                    }

                    // Only the hook thread runs this, so a thread-local needs no lock.
                    let verdict =
                        HELD.with_borrow_mut(|keys| key_verdict(keys, vk, is_down, matched));
                    match verdict {
                        Verdict::Pass => {}
                        // The release (or a repeat) of a press already acted on.
                        Verdict::Swallow => return 1,
                        Verdict::Fire(action) => {
                            let target = Target::Hwnd(GetForegroundWindow() as isize);

                            // Explicitly permit this process to take foreground window
                            windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
                                windows_sys::Win32::System::Threading::GetCurrentProcessId(),
                            );

                            if let Ok(guard) = EVENT_SENDER.lock()
                                && let Some(sender) = guard.as_ref()
                            {
                                let _ = sender.send(HotkeyEvent::from_action(action, target));
                            }

                            if win_down {
                                // Send benign key event to satisfy Windows Shell that Win was used with another key
                                use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
                                    KEYEVENTF_KEYUP, keybd_event,
                                };
                                keybd_event(0xFF, 0, KEYEVENTF_KEYUP, 0);
                            }

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
fn start_crossplatform_hotkeys(
    tx: Sender<HotkeyEvent>,
    rebinds: Receiver<Vec<(Shortcut, Action)>>,
    running: Arc<AtomicBool>,
) {
    use global_hotkey::hotkey::{HotKey, Modifiers};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager};

    thread::spawn(move || {
        let Ok(manager) = GlobalHotKeyManager::new() else {
            log::error!("Failed to initialize GlobalHotKeyManager");
            return;
        };

        let mut registered: Vec<HotKey> = Vec::new();
        let mut actions: Vec<(u32, Action)> = Vec::new();

        let receiver = GlobalHotKeyEvent::receiver();

        while running.load(Ordering::SeqCst) {
            // Only the newest list matters.
            if let Some(list) = rebinds.try_iter().last() {
                for hk in registered.drain(..) {
                    let _ = manager.unregister(hk);
                }
                actions.clear();
                for (shortcut, action) in list {
                    let Some(code) = code_for(&shortcut.key) else {
                        log::warn!("no X key for {}", shortcut.key);
                        continue;
                    };
                    let m = shortcut.modifiers;
                    let mut mods = Modifiers::empty();
                    mods.set(Modifiers::SUPER, m.super_key);
                    mods.set(Modifiers::CONTROL, m.ctrl);
                    mods.set(Modifiers::ALT, m.alt);
                    mods.set(Modifiers::SHIFT, m.shift);
                    let hk = HotKey::new((!mods.is_empty()).then_some(mods), code);
                    // Another program (often the desktop itself) may own the
                    // shortcut; then the command-line route in docs/linux.md
                    // is the way in.
                    match manager.register(hk) {
                        Ok(()) => {
                            actions.push((hk.id(), action));
                            registered.push(hk);
                        }
                        Err(e) => log::warn!("could not grab {hk:?}: {e}"),
                    }
                }
            }

            if let Ok(event) = receiver.recv_timeout(std::time::Duration::from_millis(100)) {
                // Only key presses: global-hotkey reports the release too.
                if event.state != global_hotkey::HotKeyState::Pressed {
                    continue;
                }
                if let Some((_, action)) = actions.iter().find(|(id, _)| *id == event.id) {
                    let _ = tx.send(HotkeyEvent::from_action(*action, focused_window()));
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

#[cfg(test)]
mod tests {
    use super::*;
    use windup_core::ToolId;
    use windup_core::hotkeys::{bindings, session_bindings};
    use windup_core::{Config, Shortcut, ShortcutModifiers};

    const WIN: ShortcutModifiers = ShortcutModifiers {
        super_key: true,
        ..ShortcutModifiers::NONE
    };
    const WIN_SHIFT: ShortcutModifiers = ShortcutModifiers {
        super_key: true,
        shift: true,
        ..ShortcutModifiers::NONE
    };
    const WIN_CTRL: ShortcutModifiers = ShortcutModifiers {
        super_key: true,
        ctrl: true,
        ..ShortcutModifiers::NONE
    };

    fn default_table() -> Vec<(u16, ShortcutModifiers, Action)> {
        windows_table(&bindings(&Config::default()))
    }

    #[test]
    fn defaults_match_their_actions() {
        let t = default_table();
        assert_eq!(match_binding(&t, 0x56, WIN), Some(Action::ClipboardPopup));
        assert_eq!(match_binding(&t, 0x56, WIN_SHIFT), Some(Action::PastePlain));
        assert_eq!(
            match_binding(&t, 0x2C, ShortcutModifiers::NONE),
            Some(Action::CaptureLauncher)
        );
        assert_eq!(
            match_binding(&t, 0x53, ShortcutModifiers::CTRL_SHIFT),
            Some(Action::Screenshot)
        );
        assert_eq!(match_binding(&t, 0x52, WIN_SHIFT), Some(Action::Video));
        // Windows keeps Win+Shift+S for its own Snipping Tool.
        assert_eq!(match_binding(&t, 0x53, WIN_SHIFT), None);
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let t = default_table();
        assert_eq!(match_binding(&t, 0x56, WIN_CTRL), None);
        assert_eq!(match_binding(&t, 0x56, ShortcutModifiers::NONE), None);
        assert_eq!(match_binding(&t, 0x2C, WIN), None);
    }

    #[test]
    fn a_changed_shortcut_replaces_the_default() {
        let mut config = Config::default();
        config.set_tool_shortcut(
            ToolId::ClipboardHistory,
            Shortcut::new(
                ShortcutModifiers {
                    ctrl: true,
                    alt: true,
                    ..ShortcutModifiers::NONE
                },
                "H",
            ),
        );
        let t = windows_table(&bindings(&config));
        let ctrl_alt = ShortcutModifiers {
            ctrl: true,
            alt: true,
            ..ShortcutModifiers::NONE
        };
        assert_eq!(
            match_binding(&t, 0x48, ctrl_alt),
            Some(Action::ClipboardPopup)
        );
        assert_eq!(match_binding(&t, 0x56, WIN), None);
        assert_eq!(match_binding(&t, 0x56, WIN_SHIFT), Some(Action::PastePlain));
    }

    const ESC: u16 = 0x1B;
    const KEY_G: u16 = 0x47;
    const KEY_X: u16 = 0x58;
    const VIDEO: Option<LiveKind> = Some(LiveKind::Video);
    const STEPS: Option<LiveKind> = Some(LiveKind::Steps);

    /// The tools' table and each session's, from the default config.
    fn tables() -> [Vec<(u16, ShortcutModifiers, Action)>; 3] {
        let c = Config::default();
        [
            default_table(),
            windows_table(&session_bindings(&c, LiveKind::Video)),
            windows_table(&session_bindings(&c, LiveKind::Steps)),
        ]
    }

    #[test]
    fn esc_is_matched_only_while_recording() {
        let [t, v, s] = tables();
        let none = ShortcutModifiers::NONE;
        let key = |live, vk, held| match_key(&t, &v, &s, live, vk, held);
        // Outside a recording Esc is the focused app's.
        assert_eq!(key(None, ESC, none), None);
        assert_eq!(key(VIDEO, ESC, none), Some(Action::RecordCancel));
        // Only bare Esc: Ctrl+Shift+Esc is still Task Manager's.
        assert_eq!(key(VIDEO, ESC, ShortcutModifiers::CTRL_SHIFT), None);
        // Pause is live only while recording; the Video key stops.
        assert_eq!(key(None, 0x50, WIN_SHIFT), None);
        assert_eq!(key(VIDEO, 0x50, WIN_SHIFT), Some(Action::RecordPause));
        assert_eq!(key(None, 0x52, WIN_SHIFT), Some(Action::Video));
        assert_eq!(key(VIDEO, 0x52, WIN_SHIFT), Some(Action::RecordStop));
        // The other tools keep working during a recording.
        assert_eq!(key(VIDEO, 0x56, WIN), Some(Action::ClipboardPopup));
    }

    #[test]
    fn the_step_key_is_matched_only_during_steps() {
        let [t, v, s] = tables();
        let key = |live, vk, held| match_key(&t, &v, &s, live, vk, held);
        assert_eq!(key(None, KEY_X, WIN_SHIFT), None);
        assert_eq!(key(STEPS, KEY_X, WIN_SHIFT), Some(Action::Step));
        // Matched means swallowed: the focused app never sees it.
        let mut held = Vec::new();
        assert_eq!(
            key_verdict(&mut held, KEY_X, true, key(STEPS, KEY_X, WIN_SHIFT)),
            Verdict::Fire(Action::Step)
        );
        // A recording has no step key: it passes.
        assert_eq!(key(VIDEO, KEY_X, WIN_SHIFT), None);
        let mut held = Vec::new();
        assert_eq!(
            key_verdict(&mut held, KEY_X, true, key(VIDEO, KEY_X, WIN_SHIFT)),
            Verdict::Pass
        );
        assert_eq!(
            key(VIDEO, ESC, ShortcutModifiers::NONE),
            Some(Action::RecordCancel)
        );
    }

    #[test]
    fn during_steps_esc_cancels_and_the_steps_key_stops() {
        let [t, v, s] = tables();
        let none = ShortcutModifiers::NONE;
        let key = |live, vk, held| match_key(&t, &v, &s, live, vk, held);
        assert_eq!(key(None, ESC, none), None);
        assert_eq!(key(STEPS, ESC, none), Some(Action::RecordCancel));
        assert_eq!(key(None, KEY_G, WIN_SHIFT), Some(Action::Steps));
        assert_eq!(key(STEPS, KEY_G, WIN_SHIFT), Some(Action::RecordStop));
        // Pause is video's pause key.
        assert_eq!(key(STEPS, 0x50, WIN_SHIFT), Some(Action::RecordPause));
        // The Video key stays the tool's: the host ignores it during Steps.
        assert_eq!(key(STEPS, 0x52, WIN_SHIFT), Some(Action::Video));
    }

    #[test]
    fn the_live_code_round_trips() {
        for live in [None, VIDEO, STEPS] {
            assert_eq!(live_kind(live_code(live)), live);
        }
    }

    #[test]
    fn each_recording_key_reaches_the_recording_as_its_own_event() {
        let t = Target::None;
        assert_eq!(
            HotkeyEvent::from_action(Action::Steps, t),
            HotkeyEvent::Steps
        );
        assert_eq!(HotkeyEvent::from_action(Action::Step, t), HotkeyEvent::Step);
        assert_eq!(
            HotkeyEvent::from_action(Action::Video, t),
            HotkeyEvent::Record { target: t }
        );
        assert_eq!(
            HotkeyEvent::from_action(Action::RecordPause, t),
            HotkeyEvent::RecordPause
        );
        assert_eq!(
            HotkeyEvent::from_action(Action::RecordStop, t),
            HotkeyEvent::RecordStop
        );
        assert_eq!(
            HotkeyEvent::from_action(Action::RecordCancel, t),
            HotkeyEvent::RecordCancel
        );
    }

    const PRINT: u16 = 0x2C;

    #[test]
    fn a_swallowed_press_swallows_its_release() {
        let mut held = Vec::new();
        assert_eq!(
            key_verdict(&mut held, PRINT, true, Some(Action::Screenshot)),
            Verdict::Fire(Action::Screenshot)
        );
        // Windows 11 opens Snipping Tool on Print Screen's key-up.
        assert_eq!(key_verdict(&mut held, PRINT, false, None), Verdict::Swallow);
        assert!(held.is_empty());
        // Only that once.
        assert_eq!(key_verdict(&mut held, PRINT, false, None), Verdict::Pass);
    }

    #[test]
    fn unrelated_keys_pass() {
        let mut held = Vec::new();
        key_verdict(&mut held, PRINT, true, Some(Action::Screenshot));
        assert_eq!(key_verdict(&mut held, 0x41, true, None), Verdict::Pass);
        assert_eq!(key_verdict(&mut held, 0x41, false, None), Verdict::Pass);
        assert_eq!(key_verdict(&mut held, PRINT, false, None), Verdict::Swallow);
    }

    #[test]
    fn a_release_whose_press_was_not_swallowed_passes() {
        // Pressed before the hook saw it, or with other modifiers, so the
        // press went to whichever app had focus: its release goes there too.
        let mut held = Vec::new();
        assert_eq!(key_verdict(&mut held, PRINT, true, None), Verdict::Pass);
        assert_eq!(key_verdict(&mut held, PRINT, false, None), Verdict::Pass);
        assert_eq!(key_verdict(&mut held, 0x56, false, None), Verdict::Pass);
    }

    #[test]
    fn auto_repeat_of_a_swallowed_key_stays_swallowed() {
        let mut held = Vec::new();
        key_verdict(&mut held, 0x56, true, Some(Action::ClipboardPopup));
        // Win let go while V repeats: no binding matches any more.
        assert_eq!(key_verdict(&mut held, 0x56, true, None), Verdict::Swallow);
        assert_eq!(key_verdict(&mut held, 0x56, false, None), Verdict::Swallow);
        assert!(held.is_empty());
    }

    #[test]
    fn unknown_keys_are_skipped() {
        let b = vec![(Shortcut::new(WIN, "NOSUCHKEY"), Action::ClipboardPopup)];
        assert!(windows_table(&b).is_empty());
    }

    #[test]
    fn x11_key_names() {
        assert!(code_for("V").is_some());
        assert!(code_for("7").is_some());
        assert!(code_for("F12").is_some());
        for key in windup_core::shortcut::UNHEARD_KEYS {
            assert!(code_for(key).is_some(), "{key}");
        }
        assert!(code_for("ESCAPE").is_some(), "the recording's cancel key");
        assert!(code_for("NOSUCHKEY").is_none());
    }
}
