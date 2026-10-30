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
    /// The ScreenCast portal answers and can share a monitor. Unlike the
    /// screenshot one it is asked, because without it a recording has
    /// nothing to fall back to and the tool should say so before it starts.
    pub portal_screencast: bool,
}

impl Facts {
    /// Read the environment. Trivial on purpose; not unit-tested.
    pub fn from_env(
        globals: Vec<String>,
        portal_screenshot: bool,
        portal_screencast: bool,
    ) -> Facts {
        let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
        Facts {
            windows: cfg!(windows),
            x11_only: set("DISPLAY") && !set("WAYLAND_DISPLAY"),
            globals,
            desktop: std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
            swaysock: set("SWAYSOCK"),
            hyprland: set("HYPRLAND_INSTANCE_SIGNATURE"),
            portal_screenshot,
            portal_screencast,
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

/// How moving pictures of the desktop are taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Moving {
    /// Desktop Duplication through ffmpeg's `ddagrab`.
    Ddagrab,
    Gdigrab,
    X11grab,
    ExtImageCopy,
    WlrScreencopy,
    /// The ScreenCast portal, a PipeWire stream.
    ScreenCast,
    /// Nothing available.
    None,
}

fn cosmic(f: &Facts) -> bool {
    f.desktop
        .split(':')
        .any(|d| d.eq_ignore_ascii_case("COSMIC"))
}

fn gnome(f: &Facts) -> bool {
    f.desktop
        .split(':')
        .any(|d| d.eq_ignore_ascii_case("GNOME"))
}

/// The order to try the video back ends in; never empty. The protocols are
/// told apart as `still_order` tells them: by the globals on offer. COSMIC
/// has both and prefers the standard one, wlroots prefers its own. KDE and
/// GNOME offer neither, so they come out as the portal alone.
pub fn video_order(f: &Facts) -> Vec<Moving> {
    if f.windows {
        return vec![Moving::Ddagrab, Moving::Gdigrab];
    }
    if f.x11_only {
        return vec![Moving::X11grab];
    }
    let ext = f.has("ext_image_copy_capture_manager_v1")
        && f.has("ext_output_image_capture_source_manager_v1");
    let wlr = f.has("zwlr_screencopy_manager_v1");
    let mut order = Vec::new();
    let (ext, wlr) = (
        ext.then_some(Moving::ExtImageCopy),
        wlr.then_some(Moving::WlrScreencopy),
    );
    if cosmic(f) {
        order.extend(ext);
        order.extend(wlr);
    } else {
        order.extend(wlr);
        order.extend(ext);
    }
    if f.portal_screencast {
        order.push(Moving::ScreenCast);
    }
    if order.is_empty() {
        order.push(Moving::None);
    }
    order
}

/// How the recording bar is kept out of the recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarHost {
    /// Windows can exclude one window from capture.
    ExcludedWindow,
    /// The bar sits outside the recorded rectangle.
    Outside,
    /// The bar is hidden while recording and the hotkey stops it.
    Hidden,
}

pub fn record_bar_host(f: &Facts) -> BarHost {
    if f.windows {
        BarHost::ExcludedWindow
    } else if f.x11_only {
        BarHost::Outside
    } else if gnome(f) || !f.has("zwlr_layer_shell_v1") {
        // GNOME has no layer-shell; neither does a window host.
        BarHost::Hidden
    } else {
        BarHost::Outside
    }
}

/// How a Wayland route learns where the pointer was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WlPointer {
    /// The capture protocol's cursor session.
    CursorSession,
    Hyprland,
    /// Sway only says which output has focus.
    SwayFocus,
    Unknown,
}

/// How Steps takes the picture for each click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepRoute {
    Gdi,
    X11,
    Screencopy { ext: bool, pointer: WlPointer },
    KWin,
    ScreenCast,
    None,
}

/// The order to try the Steps back ends in; never empty. Protocols are
/// detected as `video_order` does. The standard one always has a cursor
/// session; the wlroots one needs the compositor's own way to find the pointer.
pub fn steps_order(f: &Facts) -> Vec<StepRoute> {
    if f.windows {
        return vec![StepRoute::Gdi];
    }
    if f.x11_only {
        return vec![StepRoute::X11];
    }
    let mut order = Vec::new();
    if f.has("ext_image_copy_capture_manager_v1")
        && f.has("ext_output_image_capture_source_manager_v1")
    {
        order.push(StepRoute::Screencopy {
            ext: true,
            pointer: WlPointer::CursorSession,
        });
    }
    if f.has("zwlr_screencopy_manager_v1") {
        let pointer = if f.hyprland {
            WlPointer::Hyprland
        } else if f.swaysock {
            WlPointer::SwayFocus
        } else {
            WlPointer::Unknown
        };
        order.push(StepRoute::Screencopy {
            ext: false,
            pointer,
        });
    }
    if f.kde() {
        order.push(StepRoute::KWin);
    }
    if f.portal_screencast {
        order.push(StepRoute::ScreenCast);
    }
    if order.is_empty() {
        order.push(StepRoute::None);
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
            portal_screencast: portal,
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

    fn all_globals(desktop: &str, screencast: bool) -> Facts {
        let mut f = wl(
            &["zwlr_screencopy_manager_v1", "zwlr_layer_shell_v1"],
            desktop,
            true,
        );
        f.globals.extend(EXT.map(String::from));
        f.portal_screencast = screencast;
        f
    }

    #[test]
    fn video_windows_is_ddagrab_then_gdigrab() {
        let f = Facts {
            windows: true,
            ..Facts::default()
        };
        assert_eq!(video_order(&f), [Moving::Ddagrab, Moving::Gdigrab]);
    }

    #[test]
    fn video_x11_is_x11grab() {
        let f = Facts {
            x11_only: true,
            ..Facts::default()
        };
        assert_eq!(video_order(&f), [Moving::X11grab]);
    }

    #[test]
    fn video_cosmic_prefers_ext() {
        assert_eq!(
            video_order(&all_globals("COSMIC", true)),
            [
                Moving::ExtImageCopy,
                Moving::WlrScreencopy,
                Moving::ScreenCast
            ]
        );
    }

    #[test]
    fn video_wlroots_prefers_wlr() {
        assert_eq!(
            video_order(&all_globals("sway", true)),
            [
                Moving::WlrScreencopy,
                Moving::ExtImageCopy,
                Moving::ScreenCast
            ]
        );
    }

    #[test]
    fn video_kde_and_gnome_are_the_portal() {
        for d in ["KDE", "ubuntu:GNOME"] {
            let f = wl(&["wl_compositor"], d, true);
            assert_eq!(video_order(&f), [Moving::ScreenCast]);
        }
    }

    #[test]
    fn video_without_the_portal_is_nothing() {
        let mut f = wl(&["wl_compositor", "xdg_wm_base"], "", true);
        f.portal_screencast = false;
        assert_eq!(video_order(&f), [Moving::None]);
        let mut f = wl(&["wl_compositor"], "GNOME", true);
        f.portal_screencast = false;
        assert_eq!(video_order(&f), [Moving::None]);
        // A protocol alone is enough without the portal.
        assert_eq!(
            video_order(&all_globals("COSMIC", false)),
            [Moving::ExtImageCopy, Moving::WlrScreencopy]
        );
    }

    #[test]
    fn steps_order_per_desktop() {
        let sc = |ext, pointer| StepRoute::Screencopy { ext, pointer };
        let f = Facts {
            windows: true,
            ..Facts::default()
        };
        assert_eq!(steps_order(&f), [StepRoute::Gdi]);
        let f = Facts {
            x11_only: true,
            ..Facts::default()
        };
        assert_eq!(steps_order(&f), [StepRoute::X11]);
        let f = wl(&[EXT[0], EXT[1]], "COSMIC", false);
        assert_eq!(steps_order(&f), [sc(true, WlPointer::CursorSession)]);
        let mut f = wl(
            &[EXT[0], EXT[1], "zwlr_screencopy_manager_v1"],
            "sway",
            false,
        );
        f.swaysock = true;
        assert_eq!(
            steps_order(&f),
            [
                sc(true, WlPointer::CursorSession),
                sc(false, WlPointer::SwayFocus)
            ]
        );
        let mut f = wl(&["zwlr_screencopy_manager_v1"], "sway", false);
        f.swaysock = true;
        assert_eq!(steps_order(&f), [sc(false, WlPointer::SwayFocus)]);
        let mut f = wl(&["zwlr_screencopy_manager_v1"], "Hyprland", false);
        f.hyprland = true;
        assert_eq!(steps_order(&f), [sc(false, WlPointer::Hyprland)]);
        let f = wl(&["zwlr_screencopy_manager_v1"], "river", false);
        assert_eq!(steps_order(&f), [sc(false, WlPointer::Unknown)]);
        let f = wl(&["wl_compositor"], "KDE", true);
        assert_eq!(steps_order(&f), [StepRoute::KWin, StepRoute::ScreenCast]);
        let f = wl(&["wl_compositor"], "ubuntu:GNOME", true);
        assert_eq!(steps_order(&f), [StepRoute::ScreenCast]);
        let f = wl(&["wl_compositor", "xdg_wm_base"], "", false);
        assert_eq!(steps_order(&f), [StepRoute::None]);
    }

    #[test]
    fn bar_host_per_desktop() {
        let f = Facts {
            windows: true,
            ..Facts::default()
        };
        assert_eq!(record_bar_host(&f), BarHost::ExcludedWindow);
        let f = Facts {
            x11_only: true,
            ..Facts::default()
        };
        assert_eq!(record_bar_host(&f), BarHost::Outside);
        assert_eq!(record_bar_host(&all_globals("KDE", true)), BarHost::Outside);
        assert_eq!(
            record_bar_host(&all_globals("COSMIC", true)),
            BarHost::Outside
        );
        // GNOME, even if it ever offered layer-shell, and any window host.
        assert_eq!(
            record_bar_host(&all_globals("ubuntu:GNOME", true)),
            BarHost::Hidden
        );
        let f = wl(&["wl_compositor", "xdg_wm_base"], "", true);
        assert_eq!(record_bar_host(&f), BarHost::Hidden);
    }
}
