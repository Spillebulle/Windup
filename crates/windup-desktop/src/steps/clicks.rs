//! The click watchers: every button press anywhere on the desktop, as a
//! `Trigger::Click` on the session's trigger channel. Windows has a
//! low-level mouse hook; X11 has XInput2 raw button events. Each runs on a
//! thread of its own and stops when its `ClickWatch` is dropped.

use std::sync::OnceLock;
use std::sync::mpsc::Sender;
use std::time::Instant;

use super::live::Trigger;

/// Seconds since the first press any watcher saw: the steady clock a
/// `Press` is timed by.
#[cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]
fn now() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// Watches clicks for one Steps session; dropping it stops watching.
pub struct ClickWatch {
    #[cfg(target_os = "windows")]
    _inner: windows::Watch,
    #[cfg(target_os = "linux")]
    _inner: x11::Watch,
}

impl ClickWatch {
    /// Starts watching, sending each press to `tx`; `None` where clicks
    /// cannot be seen this way. On Linux this is X11 only: Wayland hosts
    /// read clicks through evdev.
    pub fn start(tx: Sender<Trigger>) -> Option<ClickWatch> {
        #[cfg(target_os = "windows")]
        {
            Some(ClickWatch {
                _inner: windows::Watch::start(tx)?,
            })
        }
        #[cfg(target_os = "linux")]
        {
            if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()) {
                return None;
            }
            Some(ClickWatch {
                _inner: x11::Watch::start(tx)?,
            })
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            let _ = tx;
            None
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) mod windows {
    //! `WH_MOUSE_LL`, installed for the session only, on a thread of its own
    //! with a message loop. It watches and never swallows: every message
    //! goes on to `CallNextHookEx`.

    use std::sync::Mutex;
    use std::sync::mpsc::{Sender, channel};
    use std::thread::JoinHandle;

    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, MSG, MSLLHOOKSTRUCT, PM_NOREMOVE,
        PeekMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_MOUSE_LL,
        WM_QUIT,
    };
    use windup_core::steps::clicks::{Press, windows_button};

    use super::super::live::Trigger;

    /// What the hook does with a message.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum HookReply {
        /// Hand it on to the next hook and the window under the pointer.
        Pass,
    }

    /// The hook only watches: whatever the message, it is passed on.
    pub fn hook_reply(_msg: u32) -> HookReply {
        HookReply::Pass
    }

    /// A press, from a hook message and its point (physical pixels).
    pub fn press_from(msg: u32, pt: (i32, i32), t: f64) -> Option<Press> {
        Some(Press {
            button: windows_button(msg)?,
            at: Some([pt.0 as f32, pt.1 as f32]),
            t,
        })
    }

    /// Where the hook sends presses. The hook has no context of its own, so
    /// there is one watch at a time.
    static SENDER: Mutex<Option<Sender<Trigger>>> = Mutex::new(None);

    unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let msg = wparam as u32;
        if code >= 0 {
            // SAFETY: for a non-negative code, `lparam` is the event's
            // MSLLHOOKSTRUCT.
            let info = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
            // try_lock: the hook must never wait. It misses a press only
            // while a watch starts or stops.
            if let Some(press) = press_from(msg, (info.pt.x, info.pt.y), super::now())
                && let Ok(guard) = SENDER.try_lock()
                && let Some(tx) = guard.as_ref()
            {
                let _ = tx.send(Trigger::Click(press));
            }
        }
        match hook_reply(msg) {
            // SAFETY: the arguments are the ones this hook was called with.
            HookReply::Pass => unsafe {
                CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
            },
        }
    }

    pub struct Watch {
        thread_id: u32,
        thread: Option<JoinHandle<()>>,
    }

    impl Watch {
        pub fn start(tx: Sender<Trigger>) -> Option<Watch> {
            {
                let mut sender = SENDER.lock().unwrap_or_else(|e| e.into_inner());
                if sender.is_some() {
                    log::warn!("steps: a click watch is already running");
                    return None;
                }
                *sender = Some(tx);
            }
            let (ready_tx, ready_rx) = channel();
            let thread = std::thread::Builder::new()
                .name("windup-steps-clicks".into())
                .spawn(move || {
                    // SAFETY: the hook is installed, served and removed on
                    // this thread; `msg` is a local the loop fills.
                    unsafe {
                        let mut msg: MSG = std::mem::zeroed();
                        // Gives the thread its message queue before anyone
                        // can post WM_QUIT to it.
                        PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
                        let hook = SetWindowsHookExW(
                            WH_MOUSE_LL,
                            Some(mouse_proc),
                            std::ptr::null_mut(),
                            0,
                        );
                        if hook.is_null() {
                            let _ = ready_tx.send(None);
                            return;
                        }
                        let _ = ready_tx.send(Some(GetCurrentThreadId()));
                        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                            DispatchMessageW(&msg);
                        }
                        UnhookWindowsHookEx(hook);
                    }
                });
            let started = thread
                .ok()
                .and_then(|thread| Some((ready_rx.recv().ok()??, thread)));
            match started {
                Some((thread_id, thread)) => Some(Watch {
                    thread_id,
                    thread: Some(thread),
                }),
                None => {
                    log::warn!("steps: the mouse hook could not be installed");
                    *SENDER.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    None
                }
            }
        }
    }

    impl Drop for Watch {
        fn drop(&mut self) {
            // SAFETY: plain call; the thread made its queue before it said
            // it was ready.
            unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            *SENDER.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) mod x11 {
    //! XInput2 raw button presses on the root, for every master device, on
    //! a connection of its own. Raw events reach the root whatever window
    //! is under the pointer.

    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::Sender;
    use std::thread::JoinHandle;

    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    use windup_core::steps::clicks::{Press, x11_button};
    use x11rb::connection::{Connection, RequestConnection as _};
    use x11rb::protocol::Event;
    use x11rb::protocol::xinput::{self, ConnectionExt as _, XIEventMask};
    use x11rb::protocol::xproto::{ConnectionExt as _, Window};
    use x11rb::rust_connection::RustConnection;

    use super::super::live::Trigger;

    /// How often the thread looks at the stop flag while no event comes.
    const STOP_CHECK: Timespec = Timespec {
        tv_sec: 0,
        tv_nsec: 100_000_000,
    };

    pub struct Watch {
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    /// A connection with raw button presses selected on its root.
    fn listen() -> Result<(RustConnection, Window), String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| format!("no X display: {e}"))?;
        let root = conn.setup().roots.get(screen).ok_or("no X screen")?.root;
        conn.extension_information(xinput::X11_EXTENSION_NAME)
            .map_err(|e| e.to_string())?
            .ok_or("no XInput extension")?;
        // From 2.1 on, raw events reach the root even while a client has
        // the pointer grabbed (a menu, a drag).
        let version = conn
            .xinput_xi_query_version(2, 2)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        if version.major_version < 2 {
            return Err("XInput 2 is missing".into());
        }
        let masks = [xinput::EventMask {
            deviceid: xinput::Device::ALL_MASTER.into(),
            mask: vec![XIEventMask::RAW_BUTTON_PRESS],
        }];
        conn.xinput_xi_select_events(root, &masks)
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|e| e.to_string())?;
        Ok((conn, root))
    }

    impl Watch {
        pub fn start(tx: Sender<Trigger>) -> Option<Watch> {
            let (conn, root) = listen()
                .map_err(|e| log::warn!("steps: cannot watch clicks: {e}"))
                .ok()?;
            let stop = Arc::new(AtomicBool::new(false));
            let flag = stop.clone();
            let thread = std::thread::Builder::new()
                .name("windup-steps-clicks".into())
                .spawn(move || watch(conn, root, &flag, &tx))
                .ok()?;
            Some(Watch {
                stop,
                thread: Some(thread),
            })
        }
    }

    impl Drop for Watch {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            // The thread sees the flag within `STOP_CHECK`, and its end
            // closes the connection.
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn watch(conn: RustConnection, root: Window, stop: &AtomicBool, tx: &Sender<Trigger>) {
        while !stop.load(Ordering::SeqCst) {
            loop {
                match conn.poll_for_event() {
                    Ok(Some(Event::XinputRawButtonPress(e))) => {
                        let Some(button) = x11_button(e.detail) else {
                            continue;
                        };
                        // Raw events carry no position: the pointer now is
                        // where the press was, a moment ago.
                        let at = conn
                            .query_pointer(root)
                            .ok()
                            .and_then(|c| c.reply().ok())
                            .filter(|r| r.same_screen)
                            .map(|r| [r.root_x as f32, r.root_y as f32]);
                        let press = Press {
                            button,
                            at,
                            t: super::now(),
                        };
                        if tx.send(Trigger::Click(press)).is_err() {
                            return;
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(e) => {
                        log::warn!("steps: the X connection for clicks closed: {e}");
                        return;
                    }
                }
            }
            let mut fds = [PollFd::new(conn.stream(), PollFlags::IN)];
            let _ = poll(&mut fds, Some(&STOP_CHECK));
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "windows")]
    #[test]
    fn the_mouse_hook_never_swallows() {
        use super::windows::{HookReply, hook_reply, press_from};
        use windup_core::steps::clicks::Press;
        use windup_core::steps::model::Button;

        // Down, up, move, wheel, side buttons and nonsense alike.
        for msg in [
            0x200, 0x201, 0x202, 0x203, 0x204, 0x205, 0x207, 0x208, 0x20A, 0x20B, 0x20C, 0x20E, 0,
            0xFFFF,
        ] {
            assert_eq!(hook_reply(msg), HookReply::Pass, "{msg:#x}");
        }
        assert_eq!(
            press_from(0x204, (5, 7), 1.5),
            Some(Press {
                button: Button::Right,
                at: Some([5.0, 7.0]),
                t: 1.5,
            })
        );
        // The release and the wheel are not presses.
        assert_eq!(press_from(0x205, (5, 7), 1.5), None);
        assert_eq!(press_from(0x20A, (5, 7), 1.5), None);
    }
}
