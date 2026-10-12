//! The XDG desktop portals, for desktops whose compositor keeps the
//! protocols Windup would otherwise use to itself: GlobalShortcuts to bind
//! Super+V without a trip to the desktop's settings, and RemoteDesktop to
//! press Ctrl+V where there is no virtual keyboard (GNOME, KDE).

use crate::platform::Plan;

pub fn start_shortcuts(_plan: &Plan) {}

pub fn paste_ctrl_v() {
    log::warn!("no way to press Ctrl+V on this desktop; the entry is on the clipboard");
}
