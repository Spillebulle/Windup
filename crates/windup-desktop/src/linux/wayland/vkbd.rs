//! Ctrl+V through `zwp_virtual_keyboard_v1`, the way `wtype` types.
//!
//! The device carries its own keymap holding only the two keys it presses,
//! and sends its modifier state explicitly. A compositor reports a virtual
//! keyboard's modifiers as they are on that device, so a Super still
//! physically held from the shortcut is not added to the Ctrl the target sees.

use super::State;
use std::io::Write;
use std::os::fd::AsFd;
use std::time::Instant;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};

/// Evdev codes: what the protocol's `key` request takes. The keymap names
/// them by XKB keycode, which is evdev + 8.
const KEY_LEFTCTRL: u32 = 29;
const KEY_V: u32 = 47;
/// Bit of Control in the core modifier order (Shift, Lock, Control, …).
const CONTROL_MASK: u32 = 1 << 2;

const KEYMAP: &str = "xkb_keymap {
xkb_keycodes \"windup\" { minimum = 8; maximum = 255; <LCTL> = 37; <AB04> = 55; };
xkb_types \"windup\" { include \"complete\" };
xkb_compatibility \"windup\" { include \"complete\" };
xkb_symbols \"windup\" {
    key <LCTL> { [ Control_L ] };
    key <AB04> { [ v, V ] };
    modifier_map Control { <LCTL> };
};
};
";

const PRESSED: u32 = 1;
const RELEASED: u32 = 0;
/// `wl_keyboard.keymap_format.xkb_v1`.
const XKB_V1: u32 = 1;

pub struct VirtualKeyboard {
    keyboard: ZwpVirtualKeyboardV1,
    epoch: Instant,
}

impl VirtualKeyboard {
    pub fn new(
        manager: &ZwpVirtualKeyboardManagerV1,
        seat: &WlSeat,
        qh: &QueueHandle<State>,
    ) -> Option<Self> {
        let keyboard = manager.create_virtual_keyboard(seat, qh, ());
        let file = keymap_file()?;
        keyboard.keymap(XKB_V1, file.as_fd(), KEYMAP.len() as u32 + 1);
        Some(Self {
            keyboard,
            epoch: Instant::now(),
        })
    }

    /// Presses and releases Ctrl+V.
    pub fn ctrl_v(&self, conn: &Connection) {
        let k = &self.keyboard;
        let t = || self.epoch.elapsed().as_millis() as u32;
        k.key(t(), KEY_LEFTCTRL, PRESSED);
        k.modifiers(CONTROL_MASK, 0, 0, 0);
        k.key(t(), KEY_V, PRESSED);
        k.key(t(), KEY_V, RELEASED);
        k.key(t(), KEY_LEFTCTRL, RELEASED);
        k.modifiers(0, 0, 0, 0);
        let _ = conn.flush();
    }
}

/// The keymap in an anonymous file, NUL-terminated as the protocol expects.
fn keymap_file() -> Option<std::fs::File> {
    use rustix::fs::{MemfdFlags, memfd_create};
    let fd = memfd_create("windup-keymap", MemfdFlags::CLOEXEC).ok()?;
    let mut file = std::fs::File::from(fd);
    file.write_all(KEYMAP.as_bytes()).ok()?;
    file.write_all(&[0]).ok()?;
    Some(file)
}

impl Dispatch<ZwpVirtualKeyboardManagerV1, ()> for State {
    fn event(
        _: &mut State,
        _: &ZwpVirtualKeyboardManagerV1,
        _: <ZwpVirtualKeyboardManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
    }
}

impl Dispatch<ZwpVirtualKeyboardV1, ()> for State {
    fn event(
        _: &mut State,
        _: &ZwpVirtualKeyboardV1,
        _: <ZwpVirtualKeyboardV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
    }
}
