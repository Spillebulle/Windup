//! Clicks on Wayland, where no app sees another's: read from the mouse and
//! touchpad event nodes themselves. They are readable when the packages'
//! udev rule gave them to the seat's user, or when the user is in the
//! `input` group (`setup::join_input_group`). A node that is also a
//! keyboard is never opened, whatever made it readable.
//!
//! What a node is comes from sysfs (`capabilities/{key,rel,abs}`), which
//! anyone may read, so the pointers that cannot be opened are counted too:
//! that is what tells "not set up" from "no mouse at all".

use std::os::fd::OwnedFd;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;

use evdev::AbsoluteAxisCode;
use evdev::raw_stream::RawDevice;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fs::{Access, Mode, OFlags, inotify};
use windup_core::steps::clicks::{
    ClickFacts, ClickRoute, PointerKind, Press, TapDetector, click_route, clicks_reason,
    evdev_button, listed_in_group, pointer_kind,
};
use windup_core::steps::model::Button;

use super::setup::{INPUT_GROUP, user_name};
use crate::steps::clicks::now;
use crate::steps::live::Trigger;

const DEV_INPUT: &str = "/dev/input";
const SYS_INPUT: &str = "/sys/class/input";

/// How often the thread looks at the stop flag while nothing happens.
const STOP_CHECK: Timespec = Timespec {
    tv_sec: 0,
    tv_nsec: 100_000_000,
};

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const SYN_REPORT: u16 = 0;
const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
const BTN_TOUCH: u16 = 0x14a;
const BTN_TOOL_FINGER: u16 = 0x145;
const BTN_TOOL_QUINTTAP: u16 = 0x148;
const BTN_TOOL_DOUBLETAP: u16 = 0x14d;
const BTN_TOOL_TRIPLETAP: u16 = 0x14e;
const BTN_TOOL_QUADTAP: u16 = 0x14f;

/// The facts `click_route` decides from, given each event node as
/// (readable, what it is). Only pointers count, readable or not.
pub fn facts_from(
    nodes: &[(bool, Option<PointerKind>)],
    in_group_now: bool,
    listed_in_group: bool,
) -> ClickFacts {
    let pointers = || nodes.iter().filter(|(_, kind)| kind.is_some());
    ClickFacts {
        native: false,
        pointer_nodes: pointers().count(),
        readable: pointers().filter(|(readable, _)| *readable).count(),
        in_group_now,
        listed_in_group,
    }
}

/// A sysfs capability bitmap as the codes it sets: hexadecimal words of
/// `word_bits` (the kernel's `long`), the highest first.
pub fn bitmap_codes(text: &str, word_bits: u32) -> Vec<u16> {
    text.split_whitespace()
        .rev()
        .enumerate()
        .flat_map(|(i, word)| {
            let word = u64::from_str_radix(word, 16).unwrap_or(0);
            (0..word_bits.min(64))
                .filter(move |b| word >> b & 1 == 1)
                .map(move |b| (i as u32 * word_bits + b) as u16)
        })
        .collect()
}

/// The id of `group` in the text of `/etc/group`.
pub fn group_gid(etc_group: &str, group: &str) -> Option<u32> {
    etc_group.lines().find_map(|line| {
        let mut fields = line.split(':');
        if fields.next()? != group {
            return None;
        }
        fields.nth(1)?.parse().ok()
    })
}

/// A touchpad axis's units per millimetre. A pad that does not say is taken
/// as 100 mm across, which is near enough for telling a tap from a swipe.
pub fn units_per_mm(resolution: i32, min: i32, max: i32) -> f32 {
    if resolution > 0 {
        resolution as f32
    } else {
        ((max - min) as f32 / 100.0).max(1.0)
    }
}

/// How many fingers a `BTN_TOOL_*` code says are down.
fn finger_count(code: u16) -> Option<u8> {
    match code {
        BTN_TOOL_FINGER => Some(1),
        BTN_TOOL_DOUBLETAP => Some(2),
        BTN_TOOL_TRIPLETAP => Some(3),
        BTN_TOOL_QUADTAP => Some(4),
        BTN_TOOL_QUINTTAP => Some(5),
        _ => None,
    }
}

/// A touchpad's state between reports.
#[derive(Default)]
struct Pad {
    per_mm: [f32; 2],
    raw: [i32; 2],
    touching: bool,
    /// Whether the last report had a finger down.
    was_touching: bool,
    fingers: u8,
    /// A button was pressed during this touch: the press is the click, and
    /// the touch is not a tap as well.
    clicked: bool,
    tap: TapDetector,
}

impl Pad {
    /// A report ends: true when it ended a tap.
    fn report(&mut self, t: f64) -> bool {
        if !self.touching && !self.was_touching {
            return false;
        }
        let pos = [
            self.raw[0] as f32 / self.per_mm[0],
            self.raw[1] as f32 / self.per_mm[1],
        ];
        let tapped = self.tap.touch(self.touching, self.fingers, pos, t);
        self.was_touching = self.touching;
        if self.touching {
            return false;
        }
        let clicked = std::mem::take(&mut self.clicked);
        tapped && !clicked
    }
}

/// Turns one device's events into presses.
#[derive(Default)]
pub struct Reader {
    pad: Option<Pad>,
}

impl Reader {
    /// For a touchpad, whose taps are presses too.
    pub fn touchpad(per_mm: [f32; 2]) -> Reader {
        Reader {
            pad: Some(Pad {
                per_mm,
                ..Pad::default()
            }),
        }
    }

    /// One input event; a button when it is a press or ends a tap.
    pub fn feed(&mut self, ty: u16, code: u16, value: i32, t: f64) -> Option<Button> {
        if ty == EV_KEY
            && value == 1
            && let Some(button) = evdev_button(code)
        {
            if let Some(pad) = &mut self.pad
                && (pad.touching || pad.was_touching)
            {
                pad.clicked = true;
            }
            return Some(button);
        }
        let pad = self.pad.as_mut()?;
        match (ty, code) {
            (EV_KEY, BTN_TOUCH) => pad.touching = value != 0,
            (EV_KEY, code) => {
                if let Some(n) = finger_count(code) {
                    if value != 0 {
                        pad.fingers = n;
                    } else if pad.fingers == n {
                        pad.fingers = 0;
                    }
                }
            }
            (EV_ABS, ABS_X) => pad.raw[0] = value,
            (EV_ABS, ABS_Y) => pad.raw[1] = value,
            (EV_SYN, SYN_REPORT) => return pad.report(t).then_some(Button::Left),
            _ => {}
        }
        None
    }
}

/// The names of the event nodes (`event3`), from `/dev/input`.
fn event_names() -> Vec<String> {
    let Ok(dir) = std::fs::read_dir(DEV_INPUT) else {
        return Vec::new();
    };
    dir.filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with("event"))
        .collect()
}

/// What a node is, from sysfs: a keyboard (or anything else) is `None`.
fn kind_of(name: &str) -> Option<PointerKind> {
    let caps = Path::new(SYS_INPUT).join(name).join("device/capabilities");
    let codes = |file: &str| {
        std::fs::read_to_string(caps.join(file))
            .map(|text| bitmap_codes(&text, usize::BITS))
            .unwrap_or_default()
    };
    pointer_kind(&codes("key"), &codes("rel"), &codes("abs"))
}

fn readable(name: &str) -> bool {
    rustix::fs::access(Path::new(DEV_INPUT).join(name), Access::READ_OK).is_ok()
}

/// Whether this process has the input group now, and whether `/etc/group`
/// lists the user in it (which a login before the change does not see).
fn input_group() -> (bool, bool) {
    let etc_group = std::fs::read_to_string("/etc/group").unwrap_or_default();
    let Some(gid) = group_gid(&etc_group, INPUT_GROUP) else {
        return (false, false);
    };
    let now = rustix::process::getegid().as_raw() == gid
        || rustix::process::getgroups().is_ok_and(|gs| gs.iter().any(|g| g.as_raw() == gid));
    let listed = std::fs::read_to_string("/etc/passwd")
        .ok()
        .and_then(|p| user_name(&p, rustix::process::getuid().as_raw()))
        .is_some_and(|user| listed_in_group(&etc_group, &user, INPUT_GROUP));
    (now, listed)
}

/// What decides whether clicks can be read on this Wayland session.
pub fn click_facts() -> ClickFacts {
    let nodes: Vec<_> = event_names()
        .iter()
        .map(|name| (readable(name), kind_of(name)))
        .collect();
    let (now, listed) = input_group();
    facts_from(&nodes, now, listed)
}

/// One open pointer node.
struct Pointer {
    name: String,
    dev: RawDevice,
    reader: Reader,
}

/// Opens `name` read-only when it is a pointer. What the open device says
/// it can send is checked again, so a keyboard is never read even if sysfs
/// and the device disagreed.
fn open(name: &str) -> Option<Pointer> {
    kind_of(name)?;
    let path = Path::new(DEV_INPUT).join(name);
    let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let fd: OwnedFd = rustix::fs::open(&path, flags, Mode::empty()).ok()?;
    let dev = RawDevice::from_fd(fd).ok()?;
    let keys: Vec<u16> = dev
        .supported_keys()
        .map(|s| s.iter().map(|k| k.0).collect())
        .unwrap_or_default();
    let rel: Vec<u16> = dev
        .supported_relative_axes()
        .map(|s| s.iter().map(|a| a.0).collect())
        .unwrap_or_default();
    let abs: Vec<u16> = dev
        .supported_absolute_axes()
        .map(|s| s.iter().map(|a| a.0).collect())
        .unwrap_or_default();
    let reader = match pointer_kind(&keys, &rel, &abs)? {
        PointerKind::Mouse => Reader::default(),
        PointerKind::Touchpad => {
            let mut per_mm = [1.0; 2];
            if let Ok(info) = dev.get_absinfo() {
                for (axis, info) in info {
                    let i = match axis {
                        AbsoluteAxisCode::ABS_X => 0,
                        AbsoluteAxisCode::ABS_Y => 1,
                        _ => continue,
                    };
                    per_mm[i] = units_per_mm(info.resolution(), info.minimum(), info.maximum());
                }
            }
            Reader::touchpad(per_mm)
        }
    };
    log::debug!("steps: reading clicks from {name} ({:?})", dev.name());
    Some(Pointer {
        name: name.to_owned(),
        dev,
        reader,
    })
}

/// Reads clicks from every readable pointer for one Steps session;
/// dropping it stops reading.
pub struct EvdevWatch {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl EvdevWatch {
    /// Starts reading, sending each press to `tx`. Fails with the route
    /// that says why when no pointer can be read.
    pub fn start(tx: Sender<Trigger>) -> Result<EvdevWatch, ClickRoute> {
        let route = click_route(&click_facts());
        if clicks_reason(route).is_err() {
            return Err(route);
        }
        let pointers: Vec<Pointer> = event_names().iter().filter_map(|n| open(n)).collect();
        if pointers.is_empty() {
            return Err(ClickRoute::NotSetUp);
        }
        // A mouse plugged in later: its node appears (IN_CREATE), and udev
        // gives it to the user a moment after (IN_ATTRIB, the ACL).
        let watch = inotify::init(inotify::CreateFlags::NONBLOCK | inotify::CreateFlags::CLOEXEC)
            .and_then(|fd| {
                let flags = inotify::WatchFlags::CREATE | inotify::WatchFlags::ATTRIB;
                inotify::add_watch(&fd, DEV_INPUT, flags).map(|_| fd)
            })
            .map_err(|e| log::warn!("steps: new mice will not be seen: {e}"))
            .ok();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("windup-steps-evdev".into())
            .spawn(move || run(pointers, watch, &flag, &tx))
            .map_err(|_| ClickRoute::NotSetUp)?;
        Ok(EvdevWatch {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for EvdevWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // The thread sees the flag within `STOP_CHECK`, and its end closes
        // the devices.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(
    mut pointers: Vec<Pointer>,
    watch: Option<OwnedFd>,
    stop: &AtomicBool,
    tx: &Sender<Trigger>,
) {
    while !stop.load(Ordering::SeqCst) {
        let (ready, added) = {
            let mut fds: Vec<PollFd> = pointers
                .iter()
                .map(|p| PollFd::new(&p.dev, PollFlags::IN))
                .collect();
            if let Some(w) = &watch {
                fds.push(PollFd::new(w, PollFlags::IN));
            }
            if poll(&mut fds, Some(&STOP_CHECK)).is_err() {
                continue;
            }
            let ready: Vec<bool> = fds.iter().map(|f| !f.revents().is_empty()).collect();
            let added = watch.is_some() && ready.last() == Some(&true);
            (ready, added)
        };
        let mut gone = Vec::new();
        for (i, p) in pointers.iter_mut().enumerate() {
            if !ready[i] {
                continue;
            }
            match p.dev.fetch_events() {
                Ok(events) => {
                    let t = now();
                    for e in events {
                        let Some(button) = p.reader.feed(e.event_type().0, e.code(), e.value(), t)
                        else {
                            continue;
                        };
                        // Nothing says where: the session puts no marker.
                        let press = Press {
                            button,
                            at: None,
                            t,
                        };
                        if tx.send(Trigger::Click(press)).is_err() {
                            return;
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                // Unplugged (ENODEV) or taken away.
                Err(e) => {
                    log::debug!("steps: {} is gone: {e}", p.name);
                    gone.push(i);
                }
            }
        }
        for i in gone.into_iter().rev() {
            pointers.remove(i);
        }
        if added && let Some(w) = &watch {
            drain(w);
            for name in event_names() {
                if !pointers.iter().any(|p| p.name == name)
                    && let Some(p) = open(&name)
                {
                    pointers.push(p);
                }
            }
        }
    }
}

/// Empties the inotify queue: what changed does not matter, the nodes are
/// looked at again.
fn drain(watch: &OwnedFd) {
    let mut buf = [std::mem::MaybeUninit::uninit(); 4096];
    let mut events = inotify::Reader::new(watch, &mut buf);
    while events.next().is_ok() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_facts_count_pointers_and_never_keyboards() {
        let mouse = Some(PointerKind::Mouse);
        let pad = Some(PointerKind::Touchpad);
        // A readable keyboard (no kind) is neither a pointer nor readable.
        let nodes = [(true, mouse), (false, pad), (true, None), (false, None)];
        let f = facts_from(&nodes, false, true);
        assert_eq!(f.pointer_nodes, 2);
        assert_eq!(f.readable, 1);
        assert!(!f.native);
        assert!(!f.in_group_now);
        assert!(f.listed_in_group);
        assert_eq!(click_route(&f), ClickRoute::Udev);
        let f = facts_from(&[(true, None)], false, false);
        assert_eq!((f.pointer_nodes, f.readable), (0, 0));
        assert_eq!(click_route(&f), ClickRoute::NoPointer);
        let f = facts_from(&[(false, mouse)], true, true);
        assert_eq!(click_route(&f), ClickRoute::NotSetUp);
    }

    #[test]
    fn a_sysfs_bitmap_is_read_lowest_word_last() {
        // A mouse's buttons: BTN_LEFT to BTN_EXTRA, in the fifth 64-bit word.
        assert_eq!(
            bitmap_codes("1f0000 0 0 0 0\n", 64),
            vec![0x110, 0x111, 0x112, 0x113, 0x114]
        );
        assert_eq!(bitmap_codes("3", 64), vec![0, 1]);
        assert_eq!(bitmap_codes("1 0", 32), vec![32]);
        assert_eq!(bitmap_codes("0", 64), Vec::<u16>::new());
        assert_eq!(bitmap_codes("", 64), Vec::<u16>::new());
        assert_eq!(bitmap_codes("zz 3", 64), vec![0, 1]);
    }

    #[test]
    fn the_input_groups_id_comes_from_etc_group() {
        let group = "wheel:x:10:anna\ninput:x:104:\nvideo:x:44:anna\n";
        assert_eq!(group_gid(group, "input"), Some(104));
        assert_eq!(group_gid(group, "audio"), None);
        assert_eq!(group_gid("input:x:nope:\n", "input"), None);
    }

    #[test]
    fn a_pad_without_a_resolution_is_taken_as_100_mm_wide() {
        assert_eq!(units_per_mm(40, 0, 4000), 40.0);
        assert_eq!(units_per_mm(0, 1000, 5000), 40.0);
        assert_eq!(units_per_mm(0, 0, 0), 1.0);
    }

    fn tap(r: &mut Reader, fingers: u16, t0: f64, t1: f64, moved: i32) -> Vec<Button> {
        let mut out = Vec::new();
        let mut feed = |ty, code, value, t| out.extend(r.feed(ty, code, value, t));
        feed(EV_KEY, BTN_TOUCH, 1, t0);
        feed(EV_KEY, fingers, 1, t0);
        feed(EV_ABS, 0, 1000, t0);
        feed(EV_ABS, 1, 1000, t0);
        feed(EV_SYN, 0, 0, t0);
        feed(EV_ABS, 0, 1000 + moved, t1);
        feed(EV_SYN, 0, 0, t1);
        feed(EV_KEY, BTN_TOUCH, 0, t1);
        feed(EV_KEY, fingers, 0, t1);
        feed(EV_SYN, 0, 0, t1);
        out
    }

    #[test]
    fn a_touchpad_tap_is_a_left_press() {
        let mut r = Reader::touchpad([10.0, 10.0]);
        assert_eq!(
            tap(&mut r, BTN_TOOL_FINGER, 0.0, 0.1, 5),
            vec![Button::Left]
        );
        // Too slow, moved too far, two fingers.
        assert!(tap(&mut r, BTN_TOOL_FINGER, 1.0, 1.5, 0).is_empty());
        assert!(tap(&mut r, BTN_TOOL_FINGER, 2.0, 2.1, 100).is_empty());
        assert!(tap(&mut r, BTN_TOOL_DOUBLETAP, 3.0, 3.1, 0).is_empty());
        assert_eq!(
            tap(&mut r, BTN_TOOL_FINGER, 4.0, 4.1, 0),
            vec![Button::Left]
        );
    }

    #[test]
    fn a_pressed_clickpad_is_one_press_not_a_press_and_a_tap() {
        let mut r = Reader::touchpad([10.0, 10.0]);
        let mut out = Vec::new();
        let mut feed = |ty, code, value, t| out.extend(r.feed(ty, code, value, t));
        feed(EV_KEY, BTN_TOUCH, 1, 0.0);
        feed(EV_KEY, BTN_TOOL_FINGER, 1, 0.0);
        feed(EV_SYN, 0, 0, 0.0);
        feed(EV_KEY, 0x110, 1, 0.05);
        feed(EV_SYN, 0, 0, 0.05);
        feed(EV_KEY, 0x110, 0, 0.1);
        feed(EV_KEY, BTN_TOUCH, 0, 0.1);
        feed(EV_KEY, BTN_TOOL_FINGER, 0, 0.1);
        feed(EV_SYN, 0, 0, 0.1);
        assert_eq!(out, vec![Button::Left]);
        // The next tap still counts.
        assert_eq!(
            tap(&mut r, BTN_TOOL_FINGER, 1.0, 1.1, 0),
            vec![Button::Left]
        );
    }

    #[test]
    fn a_mouse_sends_its_three_buttons_and_nothing_else() {
        let mut r = Reader::default();
        assert_eq!(r.feed(EV_KEY, 0x111, 1, 0.0), Some(Button::Right));
        // The release, a side button, a wheel turn, a touch.
        assert_eq!(r.feed(EV_KEY, 0x111, 0, 0.1), None);
        assert_eq!(r.feed(EV_KEY, 0x113, 1, 0.2), None);
        assert_eq!(r.feed(2, 8, -1, 0.3), None);
        assert_eq!(r.feed(EV_KEY, BTN_TOUCH, 1, 0.4), None);
        assert_eq!(r.feed(EV_SYN, 0, 0, 0.4), None);
    }
}
