//! Which presses become steps, and which way clicks reach Windup. Pure: the
//! desktop feeds in presses and facts, these decide.

use super::model::Button;
use super::options::StepTrigger;
use crate::capture::geometry::DRect;

/// Two presses closer than this are one double click.
pub const DOUBLE_CLICK_SECS: f64 = 0.4;
/// ... and nearer than this, in points.
pub const DOUBLE_CLICK_POINTS: f32 = 4.0;

/// A touch is a tap when it is lifted within this long ...
const TAP_SECS: f64 = 0.25;
/// ... having moved no further than this.
const TAP_MM: f32 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Press {
    pub button: Button,
    /// In desktop units when known.
    pub at: Option<[f32; 2]>,
    /// Seconds, from any steady clock.
    pub t: f64,
}

/// Turns presses into steps: a double click is one step.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClickFilter {
    last: Option<Press>,
}

impl ClickFilter {
    /// Whether this press adds a step. Paused presses and presses on
    /// Windup's own bar never do, and do not count towards a double click.
    pub fn accept(&mut self, p: Press, units_per_point: f32, paused: bool, own: bool) -> bool {
        if paused || own {
            return false;
        }
        if let Some(last) = self.last
            && last.button == p.button
            && p.t - last.t <= DOUBLE_CLICK_SECS
        {
            let near = match (last.at, p.at) {
                (Some(a), Some(b)) => {
                    let reach = DOUBLE_CLICK_POINTS * units_per_point;
                    (a[0] - b[0]).hypot(a[1] - b[1]) <= reach
                }
                _ => true,
            };
            if near {
                return false;
            }
        }
        self.last = Some(p);
        true
    }
}

/// Whether a press landed on Windup's own bar. `pointer_on_bar` is for
/// Wayland, where no position is known but the bar's surface has the pointer.
pub fn is_own_click(at: Option<[f32; 2]>, bar: Option<DRect>, pointer_on_bar: bool) -> bool {
    pointer_on_bar || at.zip(bar).is_some_and(|(a, b)| b.contains(a[0], a[1]))
}

/// A Win32 mouse message: only the three button-down ones count.
pub fn windows_button(msg: u32) -> Option<Button> {
    match msg {
        0x201 => Some(Button::Left),
        0x204 => Some(Button::Right),
        0x207 => Some(Button::Middle),
        _ => None,
    }
}

/// An X11 button detail: 4 to 7 are the wheel, 8 and 9 the side buttons.
pub fn x11_button(detail: u32) -> Option<Button> {
    match detail {
        1 => Some(Button::Left),
        2 => Some(Button::Middle),
        3 => Some(Button::Right),
        _ => None,
    }
}

/// A Linux input event code (`BTN_LEFT` and its neighbours).
pub fn evdev_button(code: u16) -> Option<Button> {
    match code {
        0x110 => Some(Button::Left),
        0x111 => Some(Button::Right),
        0x112 => Some(Button::Middle),
        _ => None,
    }
}

/// Recognises a tap on a touchpad: one finger, short, and still.
#[derive(Clone, Copy, Debug, Default)]
pub struct TapDetector {
    start: Option<([f32; 2], f64)>,
    spoiled: bool,
}

impl TapDetector {
    /// Feed each touch event; true once, when a tap ends. `fingers` is how
    /// many are on the pad; `pos_mm` is the position in millimetres.
    pub fn touch(&mut self, down: bool, fingers: u8, pos_mm: [f32; 2], t: f64) -> bool {
        if down {
            match self.start {
                None => {
                    self.start = Some((pos_mm, t));
                    self.spoiled = fingers != 1;
                }
                Some((from, _)) => {
                    if fingers > 1 || (pos_mm[0] - from[0]).hypot(pos_mm[1] - from[1]) > TAP_MM {
                        self.spoiled = true;
                    }
                }
            }
            return false;
        }
        let Some((from, t0)) = self.start.take() else {
            return false;
        };
        let spoiled = std::mem::take(&mut self.spoiled) || fingers > 1;
        let travel = (pos_mm[0] - from[0]).hypot(pos_mm[1] - from[1]);
        !spoiled && t - t0 <= TAP_SECS && travel <= TAP_MM
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Takes {
    pub clicks: bool,
    pub hotkey: bool,
}

/// What adds steps. When clicks cannot be seen, the key always works, even
/// for "Clicks only", so the tool is never left with no way to add a step.
pub fn takes(trigger: StepTrigger, clicks: Result<(), &'static str>) -> Takes {
    let can = clicks.is_ok();
    match trigger {
        StepTrigger::ClicksAndHotkey => Takes {
            clicks: can,
            hotkey: true,
        },
        StepTrigger::HotkeyOnly => Takes {
            clicks: false,
            hotkey: true,
        },
        StepTrigger::ClicksOnly => Takes {
            clicks: can,
            hotkey: !can,
        },
    }
}

/// How clicks reach Windup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickRoute {
    /// The OS says (Windows hook, X11 events).
    Native,
    /// Readable thanks to a udev rule for the active seat.
    Udev,
    /// Readable because the user is in the `input` group.
    InputGroup,
    /// In the group, but this session began before.
    AfterLogout,
    /// Pointer devices exist and cannot be read.
    NotSetUp,
    /// No pointer device at all.
    NoPointer,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClickFacts {
    pub native: bool,
    /// Pointer device nodes found.
    pub pointer_nodes: usize,
    /// ... of which this process can open.
    pub readable: usize,
    pub in_group_now: bool,
    pub listed_in_group: bool,
}

pub const CLICKS_AFTER_LOGOUT: &str =
    "Windup was added to the input group. Log out and in again to see clicks.";
pub const CLICKS_NOT_SET_UP: &str =
    "Windup cannot read the mouse. Add yourself to the input group to see clicks.";
pub const CLICKS_NO_POINTER: &str = "No mouse or touchpad was found.";

pub fn click_route(f: &ClickFacts) -> ClickRoute {
    if f.native {
        ClickRoute::Native
    } else if f.pointer_nodes == 0 {
        ClickRoute::NoPointer
    } else if f.readable > 0 {
        if f.in_group_now {
            ClickRoute::InputGroup
        } else {
            ClickRoute::Udev
        }
    } else if f.listed_in_group && !f.in_group_now {
        ClickRoute::AfterLogout
    } else {
        ClickRoute::NotSetUp
    }
}

/// Whether clicks can be seen, and if not, why.
pub fn clicks_reason(r: ClickRoute) -> Result<(), &'static str> {
    match r {
        ClickRoute::Native | ClickRoute::Udev | ClickRoute::InputGroup => Ok(()),
        ClickRoute::AfterLogout => Err(CLICKS_AFTER_LOGOUT),
        ClickRoute::NotSetUp => Err(CLICKS_NOT_SET_UP),
        ClickRoute::NoPointer => Err(CLICKS_NO_POINTER),
    }
}

/// Whether `user` is a member of `group` in the text of `/etc/group`.
pub fn listed_in_group(etc_group: &str, user: &str, group: &str) -> bool {
    etc_group.lines().any(|l| {
        let mut f = l.split(':');
        f.next() == Some(group) && f.nth(2).is_some_and(|m| m.split(',').any(|u| u == user))
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    Mouse,
    Touchpad,
}

/// What an input device is, from the event codes it can send. Anything that
/// has letter keys is a keyboard, whatever else it claims to be.
pub fn pointer_kind(keys: &[u16], rel: &[u16], abs: &[u16]) -> Option<PointerKind> {
    const KEY_A: u16 = 30;
    const BTN_LEFT: u16 = 0x110;
    const BTN_TOOL_FINGER: u16 = 0x145;
    const AXES: [u16; 2] = [0, 1];
    let has = |l: &[u16], c: u16| l.contains(&c);
    if has(keys, KEY_A) {
        None
    } else if has(keys, BTN_LEFT) && AXES.iter().all(|&a| has(rel, a)) {
        Some(PointerKind::Mouse)
    } else if has(keys, BTN_TOOL_FINGER) && AXES.iter().all(|&a| has(abs, a)) {
        Some(PointerKind::Touchpad)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_on_the_bar_is_not_a_step() {
        let bar = Some(DRect {
            x: 100.0,
            y: 10.0,
            w: 200.0,
            h: 40.0,
        });
        assert!(is_own_click(Some([105.0, 15.0]), bar, false));
        assert!(!is_own_click(Some([50.0, 15.0]), bar, false));
        assert!(is_own_click(None, None, true));
        assert!(!is_own_click(None, bar, false));
    }

    #[test]
    fn a_double_click_is_one_step() {
        let mut f = ClickFilter::default();
        let p = |t| Press {
            button: Button::Left,
            at: Some([10.0, 10.0]),
            t,
        };
        assert!(f.accept(p(0.0), 1.0, false, false));
        assert!(!f.accept(p(0.3), 1.0, false, false));
        assert!(f.accept(p(0.9), 1.0, false, false));
        assert!(f.accept(
            Press {
                at: Some([200.0, 10.0]),
                ..p(1.0)
            },
            1.0,
            false,
            false
        ));
        assert!(!f.accept(p(5.0), 1.0, true, false));
        assert!(!f.accept(p(6.0), 1.0, false, true));
    }

    #[test]
    fn the_double_click_distance_scales_and_unknown_positions_use_time() {
        let mut f = ClickFilter::default();
        let at = |x| Press {
            button: Button::Left,
            at: Some([x, 0.0]),
            t: 0.0,
        };
        assert!(f.accept(at(0.0), 2.0, false, false));
        // 6 units is within 4 points at 2 units per point.
        assert!(!f.accept(Press { t: 0.1, ..at(6.0) }, 2.0, false, false));
        let mut f = ClickFilter::default();
        let blind = |t| Press {
            button: Button::Left,
            at: None,
            t,
        };
        assert!(f.accept(blind(0.0), 1.0, false, false));
        assert!(!f.accept(blind(0.2), 1.0, false, false));
        // Another button is its own step.
        let right = Press {
            button: Button::Right,
            ..blind(0.3)
        };
        assert!(f.accept(right, 1.0, false, false));
    }

    #[test]
    fn the_wheel_and_side_buttons_are_not_clicks() {
        assert_eq!(windows_button(0x20A), None);
        assert_eq!(windows_button(0x20B), None);
        assert_eq!(windows_button(0x201), Some(Button::Left));
        assert_eq!(windows_button(0x204), Some(Button::Right));
        assert_eq!(windows_button(0x207), Some(Button::Middle));
        assert_eq!(x11_button(4), None);
        assert_eq!(x11_button(7), None);
        assert_eq!(x11_button(9), None);
        assert_eq!(x11_button(3), Some(Button::Right));
        assert_eq!(evdev_button(0x113), None);
        assert_eq!(evdev_button(0x114), None);
        assert_eq!(evdev_button(0x112), Some(Button::Middle));
    }

    #[test]
    fn a_short_still_one_finger_touch_is_a_tap() {
        let mut t = TapDetector::default();
        assert!(!t.touch(true, 1, [10.0, 10.0], 0.0));
        assert!(t.touch(false, 0, [12.0, 10.0], 0.12));
        // Too slow.
        t.touch(true, 1, [10.0, 10.0], 1.0);
        assert!(!t.touch(false, 0, [10.0, 10.0], 1.3));
        // Moved too far, even if it came back.
        t.touch(true, 1, [10.0, 10.0], 2.0);
        t.touch(true, 1, [15.0, 10.0], 2.05);
        assert!(!t.touch(false, 0, [10.0, 10.0], 2.1));
        // Two fingers.
        t.touch(true, 2, [10.0, 10.0], 3.0);
        assert!(!t.touch(false, 0, [10.0, 10.0], 3.1));
        // A lift with nothing down says nothing, and the next tap works.
        assert!(!t.touch(false, 0, [0.0, 0.0], 4.0));
        t.touch(true, 1, [1.0, 1.0], 5.0);
        assert!(t.touch(false, 0, [2.0, 1.0], 5.1));
    }

    #[test]
    fn clicks_only_without_click_capture_falls_back_to_the_key() {
        let both = |clicks, hotkey| Takes { clicks, hotkey };
        assert_eq!(takes(StepTrigger::ClicksOnly, Err("x")), both(false, true));
        assert_eq!(takes(StepTrigger::ClicksOnly, Ok(())), both(true, false));
        assert_eq!(
            takes(StepTrigger::ClicksAndHotkey, Err("x")),
            both(false, true)
        );
        assert_eq!(
            takes(StepTrigger::ClicksAndHotkey, Ok(())),
            both(true, true)
        );
        assert_eq!(takes(StepTrigger::HotkeyOnly, Ok(())), both(false, true));
    }

    #[test]
    fn the_click_route_says_which_way_clicks_come() {
        let f = ClickFacts::default();
        let route = |f: ClickFacts| click_route(&f);
        assert_eq!(route(ClickFacts { native: true, ..f }), ClickRoute::Native);
        assert_eq!(route(f), ClickRoute::NoPointer);
        let nodes = ClickFacts {
            pointer_nodes: 2,
            ..f
        };
        assert_eq!(
            route(ClickFacts {
                readable: 2,
                in_group_now: true,
                ..nodes
            }),
            ClickRoute::InputGroup
        );
        assert_eq!(
            route(ClickFacts {
                readable: 2,
                ..nodes
            }),
            ClickRoute::Udev
        );
        assert_eq!(
            route(ClickFacts {
                listed_in_group: true,
                ..nodes
            }),
            ClickRoute::AfterLogout
        );
        assert_eq!(route(nodes), ClickRoute::NotSetUp);
        assert!(clicks_reason(ClickRoute::Udev).is_ok());
        assert!(clicks_reason(ClickRoute::InputGroup).is_ok());
        assert!(clicks_reason(ClickRoute::Native).is_ok());
        for r in [
            ClickRoute::AfterLogout,
            ClickRoute::NotSetUp,
            ClickRoute::NoPointer,
        ] {
            assert!(clicks_reason(r).is_err());
        }
    }

    #[test]
    fn group_membership_is_read_from_etc_group() {
        assert!(listed_in_group("input:x:104:anna,bo\n", "bo", "input"));
        assert!(!listed_in_group("input:x:104:bob\n", "bo", "input"));
        assert!(!listed_in_group("video:x:44:bo\n", "bo", "input"));
        assert!(!listed_in_group("input:x:104:\n", "bo", "input"));
    }

    #[test]
    fn keyboards_are_never_pointers() {
        assert_eq!(pointer_kind(&[0x110, 30], &[0, 1], &[]), None);
        assert_eq!(
            pointer_kind(&[0x110], &[0, 1], &[]),
            Some(PointerKind::Mouse)
        );
        assert_eq!(
            pointer_kind(&[0x14a, 0x145], &[], &[0, 1]),
            Some(PointerKind::Touchpad)
        );
        assert_eq!(pointer_kind(&[0x110], &[], &[]), None);
    }
}
