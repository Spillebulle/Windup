//! Windows: "Run in background". Closing the settings window keeps Windup
//! running, with an icon in the notification area to bring the window back
//! or quit.
//!
//! The window is cloaked (DWM) rather than hidden. Windows sends no
//! `WM_PAINT` to a hidden window, and eframe runs a frame only on one, so a
//! hidden window would stop `update`, and with it the hotkeys and the
//! clipboard popup. A cloaked window is still "visible" to the system: it
//! keeps painting but is not drawn, and taking it out of the taskbar and
//! Alt+Tab is a tool-window style.
//!
//! The icon belongs to a hidden window of its own class on its own thread.
//! A second `windup` finds that window and posts it `WM_SHOW_RUNNING`.

use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, Sender, channel};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
    FindWindowW, GWL_EXSTYLE, GetCursorPos, GetMessageW, GetSystemMetrics, GetWindowLongPtrW,
    GetWindowThreadProcessId, HICON, IMAGE_ICON, LR_DEFAULTCOLOR, LoadIconW, LoadImageW,
    MF_SEPARATOR, MF_STRING, MSG, PostMessageW, RegisterClassW, RegisterWindowMessageW,
    SM_CXSMICON, SM_CYSMICON, SW_HIDE, SW_SHOWNA, SetForegroundWindow, SetWindowLongPtrW,
    ShowWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WM_APP,
    WM_COMMAND, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

/// What the icon (or a second `windup`) asks of the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    Show,
    Quit,
}

const CLASS: &str = "io.github.spillebulle.windup.tray";
/// The icon's callback message; `lparam` is the mouse message.
const WM_TRAY: u32 = WM_APP + 1;
/// Posted by a second `windup`: bring the window up.
const WM_SHOW_RUNNING: u32 = WM_APP + 2;
const ICON_ID: u32 = 1;
const MENU_OPEN: usize = 1;
const MENU_QUIT: usize = 2;

/// The tray window, once its thread made it, and where its events go.
struct Shared {
    events: Sender<TrayEvent>,
    wake: Box<dyn Fn() + Send + Sync>,
}
static SHARED: OnceLock<Shared> = OnceLock::new();
static TRAY_HWND: OnceLock<isize> = OnceLock::new();

/// The notification-area icon. Dropping it removes the icon.
pub struct Tray {
    events: Receiver<TrayEvent>,
}

impl Tray {
    /// Starts the icon's thread. `wake` asks for a frame, so an event is
    /// seen at once.
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> Option<Self> {
        let (tx, rx) = channel();
        SHARED
            .set(Shared {
                events: tx,
                wake: Box::new(wake),
            })
            .ok()?;
        std::thread::Builder::new()
            .name("windup-tray".into())
            .spawn(run)
            .ok()?;
        Some(Self { events: rx })
    }

    pub fn try_recv(&self) -> Option<TrayEvent> {
        self.events.try_recv().ok()
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        if let Some(&hwnd) = TRAY_HWND.get() {
            let data = icon_data(hwnd as HWND);
            // SAFETY: plain struct; the icon may already be gone.
            unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
        }
    }
}

/// Asks a Windup already running to bring its window up. False when none
/// has a tray window (an older version: the caller looks for its window).
pub fn show_running() -> bool {
    let class = wide(CLASS);
    // SAFETY: FFI with a NUL-terminated class name; the window may be gone
    // by the time the message is posted, which only fails the post.
    unsafe {
        let hwnd = FindWindowW(class.as_ptr(), std::ptr::null());
        if hwnd.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        // Let it take the foreground, which this launch has.
        windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid);
        PostMessageW(hwnd, WM_SHOW_RUNNING, 0, 0) != 0
    }
}

/// Cloaks the main window and takes it out of the taskbar and Alt+Tab, or
/// undoes that. It keeps painting either way.
pub fn conceal(hwnd: isize, hidden: bool) {
    let hwnd = hwnd as HWND;
    if hwnd.is_null() {
        return;
    }
    let cloak: i32 = hidden.into();
    // SAFETY: FFI on our own window from its own thread.
    unsafe {
        if hidden {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_CLOAK as u32,
                &cloak as *const i32 as *const _,
                4,
            );
        }
        // The taskbar looks at the style when a window is shown, so hide,
        // restyle, show again (without activating it). Cloaked, none of it
        // is seen.
        ShowWindow(hwnd, SW_HIDE);
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let ex = if hidden {
            (ex | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW
        } else {
            ex & !WS_EX_TOOLWINDOW
        };
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex as isize);
        ShowWindow(hwnd, SW_SHOWNA);
        if !hidden {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_CLOAK as u32,
                &cloak as *const i32 as *const _,
                4,
            );
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn send(event: TrayEvent) {
    if let Some(shared) = SHARED.get() {
        let _ = shared.events.send(event);
        (shared.wake)();
    }
}

/// The icon as `windup.exe` carries it (resource 1, see `build.rs`).
fn app_icon() -> HICON {
    // SAFETY: FFI; resource 1 is the application icon, falling back to the
    // stock one if the build had no `.ico`.
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        let icon = LoadImageW(
            module,
            ICON_ID as usize as *const u16,
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            LR_DEFAULTCOLOR,
        );
        if icon.is_null() {
            LoadIconW(
                std::ptr::null_mut(),
                windows_sys::Win32::UI::WindowsAndMessaging::IDI_APPLICATION,
            )
        } else {
            icon as HICON
        }
    }
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    // SAFETY: NOTIFYICONDATAW is plain data; all-zero is its empty value.
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = ICON_ID;
    data
}

fn add_icon(hwnd: HWND) {
    let mut data = icon_data(hwnd);
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = app_icon();
    for (slot, c) in data.szTip.iter_mut().zip("Windup".encode_utf16()) {
        *slot = c;
    }
    // SAFETY: `data` is filled in and outlives the call.
    if unsafe { Shell_NotifyIconW(NIM_ADD, &data) } == 0 {
        log::warn!("tray: the notification-area icon could not be added");
    }
}

/// The right-click menu; returns the chosen item, 0 for none.
fn menu(hwnd: HWND) -> usize {
    // SAFETY: FFI; the menu is destroyed before returning.
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return 0;
        }
        let open = wide("Open Windup");
        let quit = wide("Quit Windup");
        AppendMenuW(menu, MF_STRING, MENU_OPEN, open.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(menu, MF_STRING, MENU_QUIT, quit.as_ptr());
        let mut at = POINT { x: 0, y: 0 };
        GetCursorPos(&mut at);
        // Without the foreground the menu would not close on a click
        // elsewhere (KB135788), and without WM_NULL after it a second
        // right-click would be ignored.
        SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            at.x,
            at.y,
            0,
            hwnd,
            std::ptr::null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
        chosen.max(0) as usize
    }
}

/// Explorer's broadcast when the taskbar is (re)created: icons are added again.
fn taskbar_created() -> u32 {
    static MSG: OnceLock<u32> = OnceLock::new();
    // SAFETY: FFI with a NUL-terminated name.
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) })
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => match lparam as u32 {
            WM_LBUTTONUP => send(TrayEvent::Show),
            WM_RBUTTONUP => match menu(hwnd) {
                MENU_OPEN => send(TrayEvent::Show),
                MENU_QUIT => send(TrayEvent::Quit),
                _ => {}
            },
            _ => {}
        },
        WM_SHOW_RUNNING => send(TrayEvent::Show),
        WM_COMMAND => {}
        m if m == taskbar_created() && m != 0 => add_icon(hwnd),
        // SAFETY: the default handling for everything else.
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    0
}

/// The tray thread: a hidden top-level window (a message-only one would
/// not hear `TaskbarCreated`), the icon, and its message loop.
fn run() {
    let class = wide(CLASS);
    let title = wide("Windup tray");
    // SAFETY: FFI; the class and window live as long as the process.
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        // SAFETY: WNDCLASSW is plain data; zero is "none" for every field.
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = module;
        wc.lpszClassName = class.as_ptr();
        if RegisterClassW(&wc) == 0 {
            log::warn!("tray: could not register the window class");
            return;
        }
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class.as_ptr(),
            title.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            module,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            log::warn!("tray: could not create its window");
            return;
        }
        let _ = TRAY_HWND.set(hwnd as isize);
        taskbar_created();
        add_icon(hwnd);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
