//! The capture bar: the launcher while choosing (with Steps' choices and
//! Start when Steps is the mode), the tool bar while editing (Done instead of
//! Save and Copy on a step's picture), the recording options while Video is
//! ready.
//!
//! One floating panel centred at the top of the monitor under the pointer
//! that **changes in place** between stages (spec "The launcher bar"). Laid
//! out from the owner's mockups `launcher-c2.html` (option C, accent icon) and
//! `editor-placement.html` (option 1). Painted, like the rest of the UI: every
//! control allocates a rect and is drawn with the `Painter`.
//!
//! The bar paints on its own `Order::Middle` layer so it is above the overlay
//! whatever order the view draws in, which lets the view ask the bar where it
//! is *before* it hands the pointer to the selection. Menus, the colour
//! popover and tooltips are higher still.

use crate::colour::{self, Hsv};
use crate::icons::Icon;
use crate::theme::{Mode, Palette, metrics, shadow, strong, text};
use crate::widgets::badge::{keycap_size, paint_keycap};
use egui::{
    Align2, Color32, FontId, Id, LayerId, Order, Painter, Pos2, Rect, Sense, Stroke, StrokeKind,
    Ui, UiBuilder, pos2, vec2,
};
use std::time::Duration;
use windup_core::capture::annotate::Tool;
use windup_core::capture::session::{CaptureMode, Purpose, Stage, StepsChoice, VideoChoice};
use windup_core::config::{EditorKeys, TargetKind};
use windup_core::shortcut::{Shortcut, ShortcutModifiers};
use windup_core::steps::options::{MarkerKind, StepTrigger};
use windup_core::video::{Bitrate, FIXED_BITRATES, GIF_COLOURS};

/// What a click on the bar asks for. The view turns it into a session action.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BarCommand {
    Mode(CaptureMode),
    Target(TargetKind),
    Tool(Tool),
    Colour([u8; 4]),
    Undo,
    Save,
    Copy,
    Close,
    Settings,
    Record,
    Fps(u32),
    Bitrate(Bitrate),
    GifColours(u16),
    ToggleSystemAudio,
    ToggleMic,
    StartSteps,
    StepTrigger(StepTrigger),
    Marker(MarkerKind),
}

/// What the bar shows, read from the session each frame.
pub struct BarState<'a> {
    pub stage: Stage,
    pub mode: CaptureMode,
    pub target: TargetKind,
    pub tool: Tool,
    pub colour: [u8; 4],
    pub keys: &'a EditorKeys,
    /// Whether the Video mode can be chosen, and why not.
    pub video: Result<(), &'static str>,
    /// What Record would ask for.
    pub choice: VideoChoice,
    /// The frame rates on offer for the selection.
    pub rates: Vec<u32>,
    /// A GIF: colours instead of a bitrate.
    pub gif: bool,
    pub system_audio: Result<(), &'static str>,
    pub mic: Result<(), &'static str>,
    /// What Start would ask for.
    pub steps: StepsChoice,
    /// Whether the Steps mode can be chosen, and why not.
    pub steps_mode: Result<(), &'static str>,
    /// Whether clicks can be seen, and why not: the triggers that use them
    /// are disabled without.
    pub clicks: Result<(), &'static str>,
    pub purpose: Purpose,
}

#[derive(Default)]
pub struct BarOutput {
    /// Everything the bar has on screen this frame that takes the pointer:
    /// the panel, an open menu or popover.
    pub owns: Vec<Rect>,
    /// A menu or popover was open as the frame began, so a press outside it
    /// closes it and is not handed on to the selection.
    pub busy: bool,
    pub command: Option<BarCommand>,
}

#[derive(Default)]
pub struct Bar {
    /// The dropdown whose list is open, by its place in the bar, and the
    /// stage it belongs to (another stage has other items).
    menu_open: Option<(Stage, usize)>,
    swatch_open: bool,
    hsv: Option<Hsv>,
    /// The wheel moved and the colour is not yet sent: one colour change per
    /// gesture, so a drag round the wheel is one undo step, not hundreds.
    wheel_pending: bool,
    /// The control the pointer rests on and since when, for the tooltip delay.
    tip: Option<(Id, f64)>,
}

/// One control in the bar, measured before anything is painted.
enum Item {
    /// A segmented box of icon-only buttons.
    Seg(Vec<Seg>),
    Divider,
    Menu(Menu),
    Swatch,
    Icon(IconButton),
    /// An icon button that stays on (§7.6 tool look).
    Toggle(Seg),
    Primary(Primary),
}

/// A dropdown (§7.7): the trigger shows the current entry.
struct Menu {
    entries: Vec<Entry>,
    current: usize,
    tip: Option<&'static str>,
}

struct Entry {
    icon: Option<Icon>,
    label: String,
    command: BarCommand,
    /// Why it cannot be picked, when it cannot (§7.6): drawn faded, with
    /// this as its tooltip.
    why: Option<&'static str>,
}

/// The one accent button (§7.6 primary): Copy, Record, Start or Done.
struct Primary {
    icon: Icon,
    label: &'static str,
    tip: &'static str,
    key: String,
    command: BarCommand,
}

struct Seg {
    icon: Icon,
    tip: &'static str,
    on: bool,
    enabled: bool,
    command: BarCommand,
}

struct IconButton {
    icon: Icon,
    tip: &'static str,
    key: Option<String>,
    command: BarCommand,
}

const TARGETS: [(TargetKind, Icon, &str); 3] = [
    (TargetKind::Area, Icon::SquareDashed, "Area"),
    (TargetKind::Window, Icon::AppWindow, "Window"),
    (TargetKind::FullScreen, Icon::Monitor, "Full screen"),
];

const TOOLS: [(Tool, Icon, &str); 7] = [
    (Tool::Select, Icon::MousePointer2, "Select"),
    (Tool::Pen, Icon::Brush, "Pen"),
    (Tool::Rect, Icon::Square, "Rectangle"),
    (Tool::Ellipse, Icon::Circle, "Ellipse"),
    (Tool::Blur, Icon::EyeOff, "Blur"),
    (Tool::Mosaic, Icon::Grid3x3, "Mosaic"),
    (Tool::Pipette, Icon::Pipette, "Pick a colour"),
];

const TRIGGERS: [(StepTrigger, Icon); 3] = [
    (StepTrigger::ClicksAndHotkey, Icon::MousePointerClick),
    (StepTrigger::HotkeyOnly, Icon::Keyboard),
    (StepTrigger::ClicksOnly, Icon::Mouse),
];

const MARKERS: [(MarkerKind, Icon); 3] = [
    (MarkerKind::Ring, Icon::Circle),
    (MarkerKind::Badge, Icon::Hash),
    (MarkerKind::Arrow, Icon::MousePointer2),
];

/// A bitrate as the dropdown reads it: `Auto`, `8 Mb/s`.
pub fn bitrate_label(b: Bitrate) -> String {
    match b {
        Bitrate::Auto => "Auto".to_string(),
        Bitrate::Kbps(k) if k % 1000 == 0 => format!("{} Mb/s", k / 1000),
        Bitrate::Kbps(k) => format!("{:.1} Mb/s", k as f32 / 1000.0),
    }
}

/// `30 fps`.
pub fn fps_label(fps: u32) -> String {
    format!("{fps} fps")
}

/// `256 colours`.
pub fn colours_label(n: u16) -> String {
    format!("{n} colours")
}

/// The entry of `entries` whose command is `current`, else the first.
fn menu(entries: Vec<Entry>, current: BarCommand, tip: Option<&'static str>) -> Menu {
    let current = entries
        .iter()
        .position(|e| e.command == current)
        .unwrap_or(0);
    Menu {
        entries,
        current,
        tip,
    }
}

/// Area, Window or Full screen: what the next selection is made of.
fn target_menu(s: &BarState) -> Item {
    Item::Menu(menu(
        TARGETS
            .iter()
            .map(|&(kind, icon, label)| Entry {
                icon: Some(icon),
                label: label.to_string(),
                command: BarCommand::Target(kind),
                why: None,
            })
            .collect(),
        BarCommand::Target(s.target),
        None,
    ))
}

/// What takes a step. The triggers that use clicks are disabled where
/// clicks cannot be seen; one already chosen says why on its trigger too.
fn trigger_menu(s: &BarState) -> Item {
    let entries: Vec<Entry> = TRIGGERS
        .iter()
        .map(|&(t, icon)| Entry {
            icon: Some(icon),
            label: t.label().to_string(),
            command: BarCommand::StepTrigger(t),
            why: if t == StepTrigger::HotkeyOnly {
                None
            } else {
                s.clicks.err()
            },
        })
        .collect();
    let mut m = menu(entries, BarCommand::StepTrigger(s.steps.trigger), None);
    m.tip = Some(m.entries[m.current].why.unwrap_or("What takes a step"));
    Item::Menu(m)
}

/// How each step's picture marks the click.
fn marker_menu(s: &BarState) -> Item {
    Item::Menu(menu(
        MARKERS
            .iter()
            .map(|&(k, icon)| Entry {
                icon: Some(icon),
                label: k.label().to_string(),
                command: BarCommand::Marker(k),
                why: None,
            })
            .collect(),
        BarCommand::Marker(s.steps.marker),
        Some("Marker"),
    ))
}

fn plain(label: String, command: BarCommand) -> Entry {
    Entry {
        icon: None,
        label,
        command,
        why: None,
    }
}

/// Enter, fixed as Esc is: the editor's keys are the screenshot's.
fn enter() -> String {
    Shortcut::new(ShortcutModifiers::NONE, "ENTER").to_string()
}

/// The seg box's height: the segment plus its padding and hairline (§7.9).
fn seg_h() -> f32 {
    metrics::BUTTON + 2.0 * (metrics::SEG_PAD + metrics::HAIRLINE)
}

fn bar_h() -> f32 {
    seg_h() + 2.0 * metrics::S1
}

fn label_font() -> FontId {
    FontId::proportional(text::CONTROL)
}

fn text_w(painter: &Painter, s: &str, font: FontId) -> f32 {
    painter
        .layout_no_wrap(s.to_string(), font, Color32::WHITE)
        .size()
        .x
}

impl Bar {
    fn items(&self, s: &BarState) -> Vec<Item> {
        let close = IconButton {
            icon: Icon::X,
            tip: "Close",
            // Esc is fixed, never configured, but shown as every other key is.
            key: Some(Shortcut::new(ShortcutModifiers::NONE, "ESCAPE").to_string()),
            command: BarCommand::Close,
        };
        let settings = || {
            Item::Icon(IconButton {
                icon: Icon::Settings,
                tip: "Settings",
                key: None,
                command: BarCommand::Settings,
            })
        };
        let tools = || {
            Item::Seg(
                TOOLS
                    .iter()
                    .map(|&(t, icon, tip)| Seg {
                        icon,
                        tip,
                        on: s.tool == t,
                        enabled: true,
                        command: BarCommand::Tool(t),
                    })
                    .collect(),
            )
        };
        let undo = || {
            Item::Icon(IconButton {
                icon: Icon::Undo2,
                tip: "Undo",
                key: Some(s.keys.undo.to_string()),
                command: BarCommand::Undo,
            })
        };
        match s.stage {
            Stage::Choosing => {
                let modes = Item::Seg(
                    [
                        (CaptureMode::Screenshot, Icon::Camera, Ok("Screenshot")),
                        (CaptureMode::Video, Icon::Video, s.video.map(|()| "Video")),
                        (
                            CaptureMode::Steps,
                            Icon::Footprints,
                            s.steps_mode.map(|()| "Steps"),
                        ),
                    ]
                    .into_iter()
                    .map(|(m, icon, tip)| Seg {
                        icon,
                        tip: tip.unwrap_or_else(|why| why),
                        on: s.mode == m,
                        enabled: tip.is_ok(),
                        command: BarCommand::Mode(m),
                    })
                    .collect(),
                );
                if s.mode == CaptureMode::Steps {
                    // No selection to make: the choices and Start.
                    vec![
                        modes,
                        Item::Divider,
                        trigger_menu(s),
                        marker_menu(s),
                        Item::Divider,
                        settings(),
                        Item::Primary(Primary {
                            icon: Icon::Play,
                            label: "Start",
                            tip: "Start",
                            key: enter(),
                            command: BarCommand::StartSteps,
                        }),
                        Item::Icon(close),
                    ]
                } else {
                    vec![
                        modes,
                        Item::Divider,
                        target_menu(s),
                        Item::Divider,
                        settings(),
                        Item::Icon(close),
                    ]
                }
            }
            // A step's picture: no target, no file of its own. Done writes
            // the edits back to the step.
            Stage::Editing if s.purpose == Purpose::StepEdit => vec![
                tools(),
                Item::Swatch,
                Item::Divider,
                undo(),
                Item::Divider,
                Item::Primary(Primary {
                    icon: Icon::Check,
                    label: "Done",
                    tip: "Done (Ctrl+S too)",
                    key: enter(),
                    command: BarCommand::Copy,
                }),
                Item::Icon(close),
            ],
            // The target stays: a click outside the selection picks again.
            Stage::Editing => vec![
                target_menu(s),
                Item::Divider,
                tools(),
                Item::Swatch,
                Item::Divider,
                undo(),
                Item::Divider,
                Item::Icon(IconButton {
                    icon: Icon::Download,
                    tip: "Save",
                    key: Some(s.keys.save.to_string()),
                    command: BarCommand::Save,
                }),
                Item::Primary(Primary {
                    icon: Icon::Copy,
                    label: "Copy",
                    tip: "Copy",
                    key: s.keys.copy.to_string(),
                    command: BarCommand::Copy,
                }),
                Item::Icon(close),
            ],
            // The options row takes the tool row's place (option C).
            Stage::Ready => {
                let rate = menu(
                    s.rates
                        .iter()
                        .map(|&r| plain(fps_label(r), BarCommand::Fps(r)))
                        .collect(),
                    BarCommand::Fps(s.choice.fps),
                    Some("Frame rate"),
                );
                let quality = if s.gif {
                    menu(
                        GIF_COLOURS
                            .iter()
                            .map(|&n| plain(colours_label(n), BarCommand::GifColours(n)))
                            .collect(),
                        BarCommand::GifColours(s.choice.gif_colours),
                        Some("Colours"),
                    )
                } else {
                    menu(
                        std::iter::once(Bitrate::Auto)
                            .chain(FIXED_BITRATES.iter().map(|&k| Bitrate::Kbps(k)))
                            .map(|b| plain(bitrate_label(b), BarCommand::Bitrate(b)))
                            .collect(),
                        BarCommand::Bitrate(s.choice.bitrate),
                        Some("Bitrate"),
                    )
                };
                let toggle = |icon, tip, on, why: Result<(), &'static str>, command| {
                    Item::Toggle(Seg {
                        icon,
                        tip: why.map(|()| tip).unwrap_or_else(|why| why),
                        on,
                        enabled: why.is_ok(),
                        command,
                    })
                };
                vec![
                    target_menu(s),
                    Item::Divider,
                    Item::Menu(rate),
                    Item::Menu(quality),
                    Item::Divider,
                    toggle(
                        Icon::Volume2,
                        "Computer sound",
                        s.choice.system_audio,
                        s.system_audio,
                        BarCommand::ToggleSystemAudio,
                    ),
                    toggle(
                        Icon::Mic,
                        "Microphone",
                        s.choice.mic,
                        s.mic,
                        BarCommand::ToggleMic,
                    ),
                    Item::Divider,
                    Item::Primary(Primary {
                        icon: Icon::Circle,
                        label: "Record",
                        tip: "Record",
                        key: enter(),
                        command: BarCommand::Record,
                    }),
                    Item::Icon(close),
                ]
            }
        }
    }

    /// A dropdown is as wide as its longest entry, so the trigger keeps its
    /// width as the choice changes and its list can match it exactly (§7.7).
    fn menu_w(painter: &Painter, m: &Menu) -> f32 {
        let label = m
            .entries
            .iter()
            .map(|e| text_w(painter, &e.label, label_font()))
            .fold(0.0, f32::max);
        let icon = if m.entries.iter().any(|e| e.icon.is_some()) {
            metrics::ICON + metrics::ICON_GAP
        } else {
            0.0
        };
        2.0 * metrics::S2 + icon + metrics::ICON_GAP + label + metrics::CHEVRON
    }

    fn width(painter: &Painter, item: &Item) -> f32 {
        match item {
            Item::Seg(segs) => {
                let n = segs.len() as f32;
                n * metrics::SEG_ICON_W + (n + 1.0) * metrics::SEG_PAD + 2.0 * metrics::HAIRLINE
            }
            Item::Divider => metrics::HAIRLINE,
            Item::Menu(m) => Self::menu_w(painter, m),
            Item::Swatch => metrics::SWATCH_SMALL + 2.0 * metrics::S1,
            Item::Icon(_) | Item::Toggle(_) => metrics::BUTTON,
            Item::Primary(b) => {
                2.0 * metrics::S3
                    + metrics::ICON
                    + metrics::ICON_GAP
                    + text_w(painter, b.label, strong(text::CONTROL))
            }
        }
    }

    /// The bar's rect when centred at the top of `surface`.
    pub fn rect(&self, painter: &Painter, surface: Rect, s: &BarState) -> Rect {
        let items = self.items(s);
        let w = items.iter().map(|i| Self::width(painter, i)).sum::<f32>()
            + metrics::S2 * (items.len() as f32 - 1.0)
            + 2.0 * metrics::S2;
        Rect::from_min_size(
            pos2(surface.center().x - w / 2.0, surface.top() + metrics::S4),
            vec2(w, bar_h()),
        )
    }

    /// Paints the bar at the top of `surface` and answers its clicks.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        theme: Mode,
        surface: Rect,
        s: &BarState,
        now: f64,
    ) -> BarOutput {
        let ctx = ui.ctx().clone();
        let id = Id::new("windup-capture-bar");
        let painter = ctx
            .layer_painter(LayerId::new(Order::Middle, id))
            .with_clip_rect(surface);
        let mut out = BarOutput {
            busy: self.menu_open.is_some() || self.swatch_open,
            ..Default::default()
        };
        if self.menu_open.is_some_and(|(stage, _)| stage != s.stage) {
            self.menu_open = None;
        }
        if s.stage != Stage::Editing {
            self.swatch_open = false;
        }

        let bar = self.rect(&painter, surface, s);
        out.owns.push(bar);

        // §5 floating panel: chrome at 96 %, radius 10, shadow-float,
        // line-popover edge. (No backdrop blur: egui has none, and the frozen
        // screen under the bar is already dimmed.)
        painter.add(shadow::floating(theme).as_shape(bar, metrics::RADIUS_MODAL));
        painter.rect_filled(
            bar,
            metrics::RADIUS_MODAL,
            p.chrome.gamma_multiply(metrics::FLOAT_ALPHA),
        );
        painter.rect_stroke(
            bar,
            metrics::RADIUS_MODAL,
            Stroke::new(metrics::HAIRLINE, p.line_popover),
            StrokeKind::Inside,
        );

        let mut hovered_tip: Option<(Id, Rect, &str, Option<String>)> = None;
        let mut open_menu: Option<(Rect, &Menu)> = None;
        let mut swatch_rect = None;
        let mut x = bar.left() + metrics::S2;
        let cy = bar.center().y;
        let items = self.items(s);
        for (n, item) in items.iter().enumerate() {
            let w = Self::width(&painter, item);
            let h = match item {
                Item::Seg(_) => seg_h(),
                Item::Divider => bar_h() - 2.0 * (metrics::S1 + metrics::S1),
                Item::Swatch => metrics::SWATCH_SMALL,
                _ => metrics::BUTTON,
            };
            let rect = Rect::from_min_size(pos2(x, cy - h / 2.0), vec2(w, h));
            x += w + metrics::S2;
            let item_id = id.with(n);
            match item {
                Item::Seg(segs) => {
                    // §7.9: a line-bordered box, radius 5, 2 px padding and gaps.
                    painter.rect_stroke(
                        rect,
                        metrics::RADIUS,
                        Stroke::new(metrics::HAIRLINE, p.line),
                        StrokeKind::Inside,
                    );
                    let mut sx = rect.left() + metrics::HAIRLINE + metrics::SEG_PAD;
                    for (k, seg) in segs.iter().enumerate() {
                        let r = Rect::from_min_size(
                            pos2(sx, rect.top() + metrics::HAIRLINE + metrics::SEG_PAD),
                            vec2(metrics::SEG_ICON_W, metrics::BUTTON),
                        );
                        sx += metrics::SEG_ICON_W + metrics::SEG_PAD;
                        let clicked =
                            tool_button(ui, &painter, p, r, item_id.with(k), seg, &mut hovered_tip);
                        // One of a set: the chosen one stays chosen.
                        if clicked && !seg.on {
                            out.command = Some(seg.command);
                        }
                    }
                }
                Item::Toggle(seg) => {
                    // On or off; the session decides whether it may change.
                    if tool_button(ui, &painter, p, rect, item_id, seg, &mut hovered_tip) {
                        out.command = Some(seg.command);
                    }
                }
                Item::Divider => {
                    painter.line_segment(
                        [rect.center_top(), rect.center_bottom()],
                        Stroke::new(metrics::HAIRLINE, p.line),
                    );
                }
                Item::Menu(m) => {
                    let resp = ui.interact(rect, item_id, Sense::click());
                    if resp.clicked() {
                        self.menu_open = match self.menu_open {
                            Some((_, k)) if k == n => None,
                            _ => Some((s.stage, n)),
                        };
                    }
                    let open = self.menu_open == Some((s.stage, n));
                    // §7.7 dropdown standing alone: no fill, a line border,
                    // `line-dashed` while hovered or open.
                    let edge = if resp.hovered() || open {
                        p.line_dashed
                    } else {
                        p.line
                    };
                    painter.rect_stroke(
                        rect,
                        metrics::RADIUS,
                        Stroke::new(metrics::HAIRLINE, edge),
                        StrokeKind::Inside,
                    );
                    let entry = &m.entries[m.current];
                    let mut text_x = rect.left() + metrics::S2;
                    if let Some(icon) = entry.icon {
                        let icon_r = Rect::from_min_size(
                            pos2(text_x, rect.center().y - metrics::ICON / 2.0),
                            vec2(metrics::ICON, metrics::ICON),
                        );
                        icon.paint(&painter, icon_r, p.text_strong);
                        text_x = icon_r.right() + metrics::ICON_GAP;
                    }
                    painter.text(
                        pos2(text_x, rect.center().y),
                        Align2::LEFT_CENTER,
                        &entry.label,
                        label_font(),
                        p.text_strong,
                    );
                    let chev = Rect::from_min_size(
                        pos2(
                            rect.right() - metrics::S2 - metrics::CHEVRON,
                            rect.center().y - metrics::CHEVRON / 2.0,
                        ),
                        vec2(metrics::CHEVRON, metrics::CHEVRON),
                    );
                    // The chevron flips while the list is open (§7.7).
                    let chevron = if open {
                        Icon::ChevronUp
                    } else {
                        Icon::ChevronDown
                    };
                    chevron.paint(&painter, chev, p.text_dim);
                    if resp.hovered()
                        && !open
                        && let Some(tip) = m.tip
                    {
                        hovered_tip = Some((item_id, rect, tip, None));
                    }
                    if open {
                        open_menu = Some((rect, m));
                    }
                }
                Item::Swatch => {
                    swatch_rect = Some(rect);
                    let hit = rect.expand(metrics::S1);
                    let resp = ui.interact(hit, item_id, Sense::click());
                    if resp.clicked() {
                        self.swatch_open = !self.swatch_open;
                        if self.swatch_open {
                            let [r, g, b, _] = s.colour;
                            self.hsv = Some(match self.hsv {
                                Some(h) => h.with_rgb([r, g, b]),
                                None => Hsv::from_rgb([r, g, b]),
                            });
                        }
                    }
                    let shown = match (self.swatch_open, self.hsv) {
                        (true, Some(h)) => h.to_color32(),
                        _ => Color32::from_rgb(s.colour[0], s.colour[1], s.colour[2]),
                    };
                    painter.rect_filled(rect, metrics::RADIUS_TIGHT, shown);
                    // §7.20: a line-popover ring; the swatch in hand gets a
                    // 2 px text-strong ring (an accent one vanishes on an
                    // accent-coloured swatch).
                    let ring = if self.swatch_open {
                        Stroke::new(2.0 * metrics::HAIRLINE, p.text_strong)
                    } else {
                        Stroke::new(metrics::HAIRLINE, p.line_popover)
                    };
                    painter.rect_stroke(rect, metrics::RADIUS_TIGHT, ring, StrokeKind::Outside);
                    if resp.hovered() {
                        hovered_tip = Some((item_id, rect, "Colour", None));
                    }
                }
                Item::Icon(b) => {
                    let resp = ui.interact(rect, item_id, Sense::click());
                    // §7.6 icon button: no fill, hover control-hover, icon
                    // text-muted then text-strong.
                    let ink = if resp.hovered() {
                        painter.rect_filled(rect, metrics::RADIUS, p.control_hover);
                        p.text_strong
                    } else {
                        p.text_muted
                    };
                    b.icon.paint(
                        &painter,
                        Rect::from_center_size(rect.center(), vec2(metrics::ICON, metrics::ICON)),
                        ink,
                    );
                    if resp.hovered() {
                        hovered_tip = Some((item_id, rect, b.tip, b.key.clone()));
                    }
                    if resp.clicked() {
                        out.command = Some(b.command);
                    }
                }
                Item::Primary(b) => {
                    // §7.6 primary: accent fill, accent-ink 600 text. The one
                    // primary on the overlay.
                    let resp = ui.interact(rect, item_id, Sense::click());
                    // Hover moves the fill a step towards text-strong: lighter
                    // in the dark theme, darker in the light one, and the ink
                    // keeps its contrast either way. widgets/button.rs has no
                    // primary style to borrow.
                    let fill = if resp.hovered() {
                        p.accent
                            .lerp_to_gamma(p.text_strong, metrics::PRIMARY_HOVER_MIX)
                    } else {
                        p.accent
                    };
                    painter.rect_filled(rect, metrics::RADIUS, fill);
                    let icon_r = Rect::from_min_size(
                        pos2(
                            rect.left() + metrics::S3,
                            rect.center().y - metrics::ICON / 2.0,
                        ),
                        vec2(metrics::ICON, metrics::ICON),
                    );
                    b.icon.paint(&painter, icon_r, p.accent_ink);
                    painter.text(
                        pos2(icon_r.right() + metrics::ICON_GAP, rect.center().y),
                        Align2::LEFT_CENTER,
                        b.label,
                        strong(text::CONTROL),
                        p.accent_ink,
                    );
                    if resp.hovered() {
                        hovered_tip = Some((item_id, rect, b.tip, Some(b.key.clone())));
                    }
                    if resp.clicked() {
                        out.command = Some(b.command);
                    }
                }
            }
        }

        let pressed_at = ui.input(|i| {
            i.pointer
                .primary_pressed()
                .then(|| i.pointer.interact_pos())
                .flatten()
        });

        if let Some((trigger, m)) = open_menu {
            let (list, choice, why) = menu_list(ui, p, theme, trigger, m, id);
            out.owns.push(list);
            if why.is_some() {
                hovered_tip = why.map(|(tip_id, r, why)| (tip_id, r, why, None));
            }
            if let Some(k) = choice {
                self.menu_open = None;
                if k != m.current {
                    out.command = Some(m.entries[k].command);
                }
            } else if let Some(at) = pressed_at
                && !list.contains(at)
                && !trigger.contains(at)
            {
                self.menu_open = None;
            }
        }

        if self.swatch_open
            && let Some(swatch) = swatch_rect
        {
            let (popover, colour) = self.colour_popover(ui, p, theme, swatch, bar, surface, id);
            out.owns.push(popover);
            if colour.is_some() {
                out.command = colour.map(BarCommand::Colour);
            }
            if let Some(at) = pressed_at
                && !popover.contains(at)
                && !swatch.expand(metrics::S1).contains(at)
            {
                self.swatch_open = false;
            }
        }

        // The tooltip, after the §7.17 delay.
        match hovered_tip {
            Some((tip_id, rect, label, key)) => {
                let since = match self.tip {
                    Some((held, t)) if held == tip_id => t,
                    _ => {
                        self.tip = Some((tip_id, now));
                        now
                    }
                };
                let delay = metrics::TOOLTIP_DELAY_MS as f64 / 1000.0;
                if now - since >= delay {
                    tooltip(&ctx, p, theme, rect, surface, label, key.as_deref());
                } else {
                    ctx.request_repaint_after(Duration::from_secs_f64(delay - (now - since)));
                }
            }
            None => self.tip = None,
        }

        out
    }

    /// The swatch's popover with Umber's colour wheel. The colour is sent
    /// once the button is let go, so one turn of the wheel is one change.
    #[allow(clippy::too_many_arguments)]
    fn colour_popover(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        theme: Mode,
        swatch: Rect,
        bar: Rect,
        surface: Rect,
        id: Id,
    ) -> (Rect, Option<[u8; 4]>) {
        let side = metrics::WHEEL_POPOVER + 2.0 * metrics::PAD_PANEL;
        let left = (swatch.center().x - side / 2.0)
            .clamp(surface.left(), (surface.right() - side).max(surface.left()));
        let rect = Rect::from_min_size(
            pos2(left, bar.bottom() + metrics::POPOVER_GAP),
            vec2(side, side),
        );
        let mut hsv = self.hsv.unwrap_or_else(|| Hsv::from_rgb([255, 255, 255]));
        let mut changed = false;
        egui::Area::new(id.with("colour"))
            .order(Order::Foreground)
            .fixed_pos(rect.min)
            .constrain(false)
            .show(ui.ctx(), |ui| {
                let (r, _) = ui.allocate_exact_size(rect.size(), Sense::click());
                popover_box(ui.painter(), p, theme, r, metrics::RADIUS_CARD);
                ui.scope_builder(
                    UiBuilder::new().max_rect(r.shrink(metrics::PAD_PANEL)),
                    |ui| {
                        changed = colour::wheel(ui, p, &mut hsv, metrics::WHEEL_POPOVER);
                    },
                );
            });
        self.hsv = Some(hsv);
        if changed {
            self.wheel_pending = true;
        }
        let down = ui.input(|i| i.pointer.primary_down());
        let send = (self.wheel_pending && !down).then(|| {
            self.wheel_pending = false;
            let [r, g, b] = hsv.to_rgb();
            [r, g, b, 255]
        });
        (rect, send)
    }
}

/// A dropdown's list: §7.7, opening 4 px below the trigger, exactly its
/// width, the current item in the selected-row look. Returns the list's rect,
/// the entry clicked, and a disabled entry under the pointer with its reason.
#[allow(clippy::type_complexity)]
fn menu_list(
    ui: &mut Ui,
    p: &Palette,
    theme: Mode,
    trigger: Rect,
    m: &Menu,
    id: Id,
) -> (Rect, Option<usize>, Option<(Id, Rect, &'static str)>) {
    let painter = ui
        .ctx()
        .layer_painter(LayerId::new(Order::Foreground, id.with("menu")));
    let item_h = metrics::ICON + 2.0 * metrics::MENU_ITEM_PAD_Y;
    let n = m.entries.len() as f32;
    let list = Rect::from_min_size(
        pos2(trigger.left(), trigger.bottom() + metrics::POPOVER_GAP),
        vec2(
            trigger.width(),
            n * item_h + (n - 1.0) * metrics::HAIRLINE + 2.0 * metrics::MENU_PAD,
        ),
    );
    popover_box(&painter, p, theme, list, metrics::RADIUS_CARD);
    let mut choice = None;
    let mut why = None;
    for (k, entry) in m.entries.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(
                list.left() + metrics::MENU_PAD,
                list.top() + metrics::MENU_PAD + k as f32 * (item_h + metrics::HAIRLINE),
            ),
            vec2(list.width() - 2.0 * metrics::MENU_PAD, item_h),
        );
        let entry_id = id.with(("menu", k));
        let resp = ui.interact(r, entry_id, Sense::click());
        let on = k == m.current;
        let enabled = entry.why.is_none();
        if on {
            painter.rect_filled(r, metrics::RADIUS_SEG, p.control);
        } else if resp.hovered() && enabled {
            painter.rect_filled(r, metrics::RADIUS_SEG, p.control_hover);
        }
        let ink = if !enabled {
            p.text_muted.gamma_multiply(metrics::DISABLED_ALPHA)
        } else if on || resp.hovered() {
            p.text_strong
        } else {
            p.text
        };
        if resp.hovered()
            && let Some(reason) = entry.why
        {
            why = Some((entry_id, r, reason));
        }
        let mut text_x = r.left() + metrics::MENU_ITEM_PAD_X - metrics::S1;
        if let Some(icon) = entry.icon {
            let icon_r = Rect::from_min_size(
                pos2(text_x, r.center().y - metrics::ICON / 2.0),
                vec2(metrics::ICON, metrics::ICON),
            );
            icon.paint(&painter, icon_r, ink);
            text_x = icon_r.right() + metrics::ICON_GAP;
        }
        painter.text(
            pos2(text_x, r.center().y),
            Align2::LEFT_CENTER,
            &entry.label,
            label_font(),
            ink,
        );
        if resp.clicked() && enabled {
            choice = Some(k);
        }
    }
    (list, choice, why)
}

/// A tool-look icon button (§7.6 tool, §2.4): the one that is on is
/// control-active with its icon in accent, never an accent fill; a disabled
/// one is faded and its tooltip says why. True when clicked while enabled.
fn tool_button<'a>(
    ui: &mut Ui,
    painter: &Painter,
    p: &Palette,
    r: Rect,
    id: Id,
    seg: &'a Seg,
    hovered_tip: &mut Option<(Id, Rect, &'a str, Option<String>)>,
) -> bool {
    let resp = ui.interact(r, id, Sense::click());
    let hover = resp.hovered();
    let (fill, ink) = if !seg.enabled {
        (None, p.text_muted.gamma_multiply(metrics::DISABLED_ALPHA))
    } else if seg.on {
        (Some(p.control_active), p.accent)
    } else if hover {
        (Some(p.control_hover), p.text_strong)
    } else {
        (None, p.text_muted)
    };
    if let Some(fill) = fill {
        painter.rect_filled(r, metrics::RADIUS_SEG, fill);
    }
    seg.icon.paint(
        painter,
        Rect::from_center_size(r.center(), vec2(metrics::ICON, metrics::ICON)),
        ink,
    );
    if hover {
        *hovered_tip = Some((id, r, seg.tip, None));
    }
    resp.clicked() && seg.enabled
}

/// A menu or popover's box (§5, §7.1): popover fill, line-popover edge,
/// shadow-menu.
fn popover_box(painter: &Painter, p: &Palette, theme: Mode, rect: Rect, radius: f32) {
    painter.add(shadow::menu(theme).as_shape(rect, radius));
    painter.rect_filled(rect, radius, p.popover);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(metrics::HAIRLINE, p.line_popover),
        StrokeKind::Inside,
    );
}

/// §7.17 tooltip: popover fill, line edge, radius 6, 4 x 8 padding, 10.5 px
/// text, the shortcut in a keycap. Below the control, kept on the surface.
pub fn tooltip(
    ctx: &egui::Context,
    p: &Palette,
    theme: Mode,
    anchor: Rect,
    surface: Rect,
    label: &str,
    key: Option<&str>,
) {
    let painter = ctx
        .layer_painter(LayerId::new(Order::Tooltip, Id::new("windup-capture-tip")))
        .with_clip_rect(surface);
    let font = FontId::proportional(text::TINY);
    let galley = painter.layout_no_wrap(label.to_string(), font, p.text);
    let cap = key.map(|k| keycap_size(&painter, p, k));
    let inner_w = galley.size().x + cap.map_or(0.0, |c| metrics::S2 + c.x);
    let inner_h = galley.size().y.max(cap.map_or(0.0, |c| c.y));
    let size = vec2(
        inner_w + 2.0 * metrics::TOOLTIP_PAD_X,
        inner_h + 2.0 * metrics::TOOLTIP_PAD_Y,
    );
    let mut min = pos2(
        anchor.center().x - size.x / 2.0,
        anchor.bottom() + metrics::POPOVER_GAP,
    );
    min.x = min.x.clamp(
        surface.left(),
        (surface.right() - size.x).max(surface.left()),
    );
    let rect = Rect::from_min_size(min, size);
    painter.add(shadow::menu(theme).as_shape(rect, metrics::RADIUS_TOOL));
    painter.rect_filled(rect, metrics::RADIUS_TOOL, p.popover);
    painter.rect_stroke(
        rect,
        metrics::RADIUS_TOOL,
        Stroke::new(metrics::HAIRLINE, p.line),
        StrokeKind::Inside,
    );
    let text_pos = Pos2::new(
        rect.left() + metrics::TOOLTIP_PAD_X,
        rect.center().y - galley.size().y / 2.0,
    );
    let text_right = text_pos.x + galley.size().x;
    painter.galley(text_pos, galley, p.text);
    if let (Some(k), Some(c)) = (key, cap) {
        paint_keycap(
            &painter,
            p,
            pos2(text_right + metrics::S2, rect.center().y - c.y / 2.0),
            k,
        );
    }
}
