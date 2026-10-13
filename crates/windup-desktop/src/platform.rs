//! Which mechanism does each job on this machine.
//!
//! One Linux binary runs under X11 and under every Wayland compositor, and
//! they differ in exactly the things Windup needs, so the choice is made at
//! startup by [`detect`], a pure function of the environment and of the
//! Wayland globals the compositor advertises. Windows has one answer.

/// The window a paste goes back to: whatever had focus when the hotkey fired.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Target {
    /// Unknown, or the platform hands focus back by itself (Wayland).
    #[default]
    None,
    /// A Win32 `HWND`.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    Hwnd(isize),
    /// An X11 window id.
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    X11(u32),
}

/// Where the clipboard popup is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    /// A secondary egui viewport of the eframe app: Windows and X11, which
    /// can keep the app's own window hidden while it runs.
    Viewport,
    /// A layer-shell surface on the overlay layer, drawn by our own Wayland
    /// connection. No eframe window exists until settings are opened.
    Layer,
    /// An ordinary window opened for each popup, with no window in between:
    /// Wayland compositors without layer-shell (GNOME). Wayland cannot hide a
    /// window, and one that is not shown never draws, so a hidden eframe app
    /// would never see the shortcut. The compositor places the popup.
    Window,
}

/// Which data-control protocol the compositor speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataControl {
    /// `ext_data_control_manager_v1`, the standardised one.
    Ext,
    /// `zwlr_data_control_manager_v1`, its wlroots predecessor.
    Wlr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Watch {
    /// Read the clipboard every 250 ms (`clipboard-rs`; Windows, X11, XWayland).
    Poll,
    /// Told about every copy by the compositor.
    DataControl(DataControl),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteVia {
    SendInput,
    /// XTest, through `enigo`.
    XTest,
    /// `zwp_virtual_keyboard_v1`.
    VirtualKeyboard,
    /// The RemoteDesktop portal, which asks the user once.
    Portal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeysVia {
    /// The `WH_KEYBOARD_LL` hook.
    Hook,
    /// `XGrabKey`, through `global-hotkey`, plus the command line.
    XGrab,
    /// Only `windup --popup` bound in the desktop's own shortcut settings,
    /// plus the GlobalShortcuts portal where the desktop has one.
    Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub host: Host,
    pub watch: Watch,
    pub paste: PasteVia,
    pub hotkeys: HotkeysVia,
}

impl Plan {
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub const WINDOWS: Plan = Plan {
        host: Host::Viewport,
        watch: Watch::Poll,
        paste: PasteVia::SendInput,
        hotkeys: HotkeysVia::Hook,
    };

    /// macOS, and the settings-only window, which needs no backend at all.
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    pub const OTHER: Plan = Plan {
        host: Host::Viewport,
        watch: Watch::Poll,
        paste: PasteVia::XTest,
        hotkeys: HotkeysVia::XGrab,
    };

    /// Whether a Wayland connection of our own is needed.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn needs_wayland(&self) -> bool {
        self.host == Host::Layer
            || matches!(self.watch, Watch::DataControl(_))
            || self.paste == PasteVia::VirtualKeyboard
    }
}

/// The parts of the environment that decide the backend.
#[derive(Clone, Debug, Default)]
pub struct Env {
    pub wayland_display: Option<String>,
    pub display: Option<String>,
}

impl Env {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn from_process() -> Self {
        let var = |k| std::env::var(k).ok().filter(|v: &String| !v.is_empty());
        Self {
            wayland_display: var("WAYLAND_DISPLAY"),
            display: var("DISPLAY"),
        }
    }
}

pub const LAYER_SHELL: &str = "zwlr_layer_shell_v1";
pub const EXT_DATA_CONTROL: &str = "ext_data_control_manager_v1";
pub const WLR_DATA_CONTROL: &str = "zwlr_data_control_manager_v1";
pub const VIRTUAL_KEYBOARD: &str = "zwp_virtual_keyboard_manager_v1";

/// `globals` is the interface names the Wayland compositor advertised, or
/// `None` when there is no Wayland session or connecting failed.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn detect(env: &Env, globals: Option<&[String]>) -> Plan {
    if env.wayland_display.is_some()
        && let Some(globals) = globals
    {
        let has = |name: &str| globals.iter().any(|g| g == name);
        let data_control = if has(EXT_DATA_CONTROL) {
            Some(DataControl::Ext)
        } else if has(WLR_DATA_CONTROL) {
            Some(DataControl::Wlr)
        } else {
            None
        };
        let watch = data_control.map_or(Watch::Poll, Watch::DataControl);
        let host = if has(LAYER_SHELL) && data_control.is_some() {
            Host::Layer
        } else {
            Host::Window
        };
        let paste = if has(VIRTUAL_KEYBOARD) {
            PasteVia::VirtualKeyboard
        } else {
            PasteVia::Portal
        };
        return Plan {
            host,
            watch,
            paste,
            hotkeys: HotkeysVia::Command,
        };
    }

    Plan {
        host: Host::Viewport,
        watch: Watch::Poll,
        paste: PasteVia::XTest,
        hotkeys: if env.display.is_some() {
            HotkeysVia::XGrab
        } else {
            HotkeysVia::Command
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wayland() -> Env {
        Env {
            wayland_display: Some("wayland-1".into()),
            display: Some(":0".into()),
        }
    }

    fn globals(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn cosmic_gets_everything_native() {
        // What COSMIC 1.9 advertised on the owner's machine.
        let g = globals(&[
            "wl_compositor",
            "zwp_virtual_keyboard_manager_v1",
            "ext_data_control_manager_v1",
            "zwlr_data_control_manager_v1",
            "zwlr_layer_shell_v1",
            "zcosmic_toplevel_info_v1",
        ]);
        let plan = detect(&wayland(), Some(&g));
        assert_eq!(
            plan,
            Plan {
                host: Host::Layer,
                watch: Watch::DataControl(DataControl::Ext),
                paste: PasteVia::VirtualKeyboard,
                hotkeys: HotkeysVia::Command,
            }
        );
        assert!(plan.needs_wayland());
    }

    #[test]
    fn older_wlroots_uses_the_wlr_protocol() {
        let g = globals(&[LAYER_SHELL, WLR_DATA_CONTROL, VIRTUAL_KEYBOARD]);
        let plan = detect(&wayland(), Some(&g));
        assert_eq!(plan.watch, Watch::DataControl(DataControl::Wlr));
        assert_eq!(plan.host, Host::Layer);
    }

    #[test]
    fn kde_without_virtual_keyboard_pastes_through_the_portal() {
        let g = globals(&[LAYER_SHELL, EXT_DATA_CONTROL]);
        let plan = detect(&wayland(), Some(&g));
        assert_eq!(plan.host, Host::Layer);
        assert_eq!(plan.paste, PasteVia::Portal);
    }

    #[test]
    fn gnome_falls_back_to_a_window_and_polling() {
        let g = globals(&["wl_compositor", "xdg_wm_base", "gtk_shell1"]);
        let plan = detect(&wayland(), Some(&g));
        assert_eq!(
            plan,
            Plan {
                host: Host::Window,
                watch: Watch::Poll,
                paste: PasteVia::Portal,
                hotkeys: HotkeysVia::Command,
            }
        );
        assert!(!plan.needs_wayland());
    }

    #[test]
    fn data_control_without_layer_shell_still_watches_natively() {
        let g = globals(&[EXT_DATA_CONTROL]);
        let plan = detect(&wayland(), Some(&g));
        assert_eq!(plan.host, Host::Window);
        assert_eq!(plan.watch, Watch::DataControl(DataControl::Ext));
        assert!(plan.needs_wayland());
    }

    #[test]
    fn x11_session() {
        let env = Env {
            wayland_display: None,
            display: Some(":0".into()),
        };
        assert_eq!(
            detect(&env, None),
            Plan {
                host: Host::Viewport,
                watch: Watch::Poll,
                paste: PasteVia::XTest,
                hotkeys: HotkeysVia::XGrab,
            }
        );
    }

    #[test]
    fn wayland_that_would_not_connect_is_treated_as_x11() {
        let plan = detect(&wayland(), None);
        assert_eq!(plan.hotkeys, HotkeysVia::XGrab);
        assert_eq!(plan.paste, PasteVia::XTest);
    }

    #[test]
    fn nothing_at_all() {
        let plan = detect(&Env::default(), None);
        assert_eq!(plan.hotkeys, HotkeysVia::Command);
    }
}
