//! Which mechanism takes the picture and finds the windows on this machine.
//!
//! Like the clipboard's `platform::detect`, a pure function of the
//! environment and of the Wayland globals the compositor advertises. The
//! back ends themselves live elsewhere and are tried in the order given.

/// How the still picture of the desktop is taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Still {
    Gdi,
    X11,
    /// `ext_image_copy_capture_v1`, the standardised one.
    ExtImageCopy,
    /// `zwlr_screencopy_v1`, wlroots.
    WlrScreencopy,
    KWin,
    Portal,
    /// Nothing available.
    None,
}

/// How window rectangles are found for the picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rects {
    Win32,
    X11,
    Sway,
    Hyprland,
    KWinScript,
    /// Detect edges in the picture itself.
    Edges,
}

/// What the machine offers.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub windows: bool,
    /// `DISPLAY` is set and `WAYLAND_DISPLAY` is not.
    pub x11_only: bool,
    /// Wayland interface names from the registry.
    pub globals: Vec<String>,
    /// Raw `XDG_CURRENT_DESKTOP` (colon-separated).
    pub desktop: String,
    pub swaysock: bool,
    pub hyprland: bool,
    /// The screenshot portal answers.
    pub portal_screenshot: bool,
}

impl Facts {
    /// Read the environment. Trivial on purpose; not unit-tested.
    pub fn from_env(globals: Vec<String>, portal_screenshot: bool) -> Facts {
        let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
        Facts {
            windows: cfg!(windows),
            x11_only: set("DISPLAY") && !set("WAYLAND_DISPLAY"),
            globals,
            desktop: std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
            swaysock: set("SWAYSOCK"),
            hyprland: set("HYPRLAND_INSTANCE_SIGNATURE"),
            portal_screenshot,
        }
    }

    fn has(&self, name: &str) -> bool {
        self.globals.iter().any(|g| g == name)
    }

    fn kde(&self) -> bool {
        self.desktop
            .split(':')
            .any(|d| d.eq_ignore_ascii_case("KDE"))
    }
}

/// The order to try the still back ends in; never empty.
pub fn still_order(f: &Facts) -> Vec<Still> {
    if f.windows {
        return vec![Still::Gdi];
    }
    if f.x11_only {
        return vec![Still::X11];
    }
    let mut order = Vec::new();
    if f.has("ext_image_copy_capture_manager_v1")
        && f.has("ext_output_image_capture_source_manager_v1")
    {
        order.push(Still::ExtImageCopy);
    }
    if f.has("zwlr_screencopy_manager_v1") {
        order.push(Still::WlrScreencopy);
    }
    if f.kde() {
        order.push(Still::KWin);
    }
    if f.portal_screenshot {
        order.push(Still::Portal);
    }
    if order.is_empty() {
        order.push(Still::None);
    }
    order
}

/// How windows are found.
pub fn rects(f: &Facts) -> Rects {
    if f.windows {
        Rects::Win32
    } else if f.x11_only {
        Rects::X11
    } else if f.swaysock {
        Rects::Sway
    } else if f.hyprland {
        Rects::Hyprland
    } else if f.kde() {
        Rects::KWinScript
    } else {
        Rects::Edges
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wl(globals: &[&str], desktop: &str, portal: bool) -> Facts {
        Facts {
            globals: globals.iter().map(|s| s.to_string()).collect(),
            desktop: desktop.into(),
            portal_screenshot: portal,
            ..Facts::default()
        }
    }

    const EXT: [&str; 2] = [
        "ext_image_copy_capture_manager_v1",
        "ext_output_image_capture_source_manager_v1",
    ];

    #[test]
    fn cosmic_prefers_ext_then_portal() {
        let f = wl(&[EXT[0], EXT[1], "wl_compositor"], "COSMIC", true);
        assert_eq!(still_order(&f), [Still::ExtImageCopy, Still::Portal]);
        assert_eq!(rects(&f), Rects::Edges);
    }

    #[test]
    fn ext_needs_both_globals() {
        let f = wl(&[EXT[0]], "COSMIC", false);
        assert_eq!(still_order(&f), [Still::None]);
    }

    #[test]
    fn sway_orders_ext_wlr_portal() {
        let mut f = wl(&["zwlr_screencopy_manager_v1"], "sway", true);
        f.swaysock = true;
        assert_eq!(still_order(&f), [Still::WlrScreencopy, Still::Portal]);
        assert_eq!(rects(&f), Rects::Sway);
        f.globals.extend(EXT.map(String::from));
        assert_eq!(
            still_order(&f),
            [Still::ExtImageCopy, Still::WlrScreencopy, Still::Portal]
        );
    }

    #[test]
    fn hyprland_rects() {
        let mut f = wl(&["zwlr_screencopy_manager_v1"], "Hyprland", false);
        f.hyprland = true;
        assert_eq!(still_order(&f), [Still::WlrScreencopy]);
        assert_eq!(rects(&f), Rects::Hyprland);
    }

    #[test]
    fn sway_wins_over_hyprland_and_kde() {
        let mut f = wl(&[], "KDE", false);
        f.swaysock = true;
        f.hyprland = true;
        assert_eq!(rects(&f), Rects::Sway);
    }

    #[test]
    fn kde_without_screencopy_uses_kwin_then_portal() {
        let f = wl(&["wl_compositor"], "KDE", true);
        assert_eq!(still_order(&f), [Still::KWin, Still::Portal]);
        assert_eq!(rects(&f), Rects::KWinScript);
    }

    #[test]
    fn desktop_is_colon_separated_and_case_insensitive() {
        let f = wl(&[], "ubuntu:kde", true);
        assert_eq!(still_order(&f), [Still::KWin, Still::Portal]);
        assert_eq!(rects(&f), Rects::KWinScript);
        let f = wl(&[], "NotKDE", false);
        assert_eq!(still_order(&f), [Still::None]);
    }

    #[test]
    fn gnome_is_portal_with_edges() {
        let f = wl(&["wl_compositor"], "ubuntu:GNOME", true);
        assert_eq!(still_order(&f), [Still::Portal]);
        assert_eq!(rects(&f), Rects::Edges);
    }

    #[test]
    fn weston_has_nothing() {
        let f = wl(&["wl_compositor", "xdg_wm_base"], "", false);
        assert_eq!(still_order(&f), [Still::None]);
        assert_eq!(rects(&f), Rects::Edges);
    }

    #[test]
    fn windows_and_x11() {
        let f = Facts {
            windows: true,
            ..Facts::default()
        };
        assert_eq!(still_order(&f), [Still::Gdi]);
        assert_eq!(rects(&f), Rects::Win32);
        let f = Facts {
            x11_only: true,
            swaysock: true,
            ..Facts::default()
        };
        assert_eq!(still_order(&f), [Still::X11]);
        assert_eq!(rects(&f), Rects::X11);
    }
}
