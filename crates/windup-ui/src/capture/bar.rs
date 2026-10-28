//! The capture bar: the launcher while choosing, the tool bar while editing.
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
use windup_core::capture::session::{CaptureMode, Stage};
use windup_core::config::{EditorKeys, TargetKind};

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
}

/// What the bar shows, read from the session each frame.
pub struct BarState<'a> {
    pub stage: Stage,
    pub mode: CaptureMode,
    pub target: TargetKind,
    pub tool: Tool,
    pub colour: [u8; 4],
    pub keys: &'a EditorKeys,
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
    menu_open: bool,
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
    Target,
    Swatch,
    Icon(IconButton),
    Copy,
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
    (Tool::Blur, Icon::Blur, "Blur"),
    (Tool::Mosaic, Icon::Grid3x3, "Mosaic"),
    (Tool::Pipette, Icon::Pipette, "Pick a colour"),
];

/// What a disabled mode says (§7.6: disabled controls say why).
pub const LATER: &str = "Arrives in a later version.";

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
            key: Some("Esc".into()),
            command: BarCommand::Close,
        };
        match s.stage {
            Stage::Choosing => vec![
                Item::Seg(
                    [
                        (CaptureMode::Screenshot, Icon::Camera, "Screenshot", true),
                        (CaptureMode::Video, Icon::Video, "Video", false),
                        (CaptureMode::Steps, Icon::Footprints, "Steps", false),
                    ]
                    .into_iter()
                    .map(|(m, icon, tip, enabled)| Seg {
                        icon,
                        tip: if enabled { tip } else { LATER },
                        on: s.mode == m,
                        enabled,
                        command: BarCommand::Mode(m),
                    })
                    .collect(),
                ),
                Item::Divider,
                Item::Target,
                Item::Divider,
                Item::Icon(IconButton {
                    icon: Icon::Settings,
                    tip: "Settings",
                    key: None,
                    command: BarCommand::Settings,
                }),
                Item::Icon(close),
            ],
            Stage::Editing => vec![
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
                ),
                Item::Swatch,
                Item::Divider,
                Item::Icon(IconButton {
                    icon: Icon::Undo2,
                    tip: "Undo",
                    key: Some(s.keys.undo.to_string()),
                    command: BarCommand::Undo,
                }),
                Item::Divider,
                Item::Icon(IconButton {
                    icon: Icon::Download,
                    tip: "Save",
                    key: Some(s.keys.save.to_string()),
                    command: BarCommand::Save,
                }),
                Item::Copy,
                Item::Icon(close),
            ],
        }
    }

    fn target_w(painter: &Painter) -> f32 {
        // Wide enough for the longest label, so the trigger does not change
        // width as the target changes and its list can match it exactly (§7.7).
        let label = TARGETS
            .iter()
            .map(|t| text_w(painter, t.2, label_font()))
            .fold(0.0, f32::max);
        2.0 * metrics::S2 + metrics::ICON + 2.0 * metrics::ICON_GAP + label + metrics::CHEVRON
    }

    fn width(painter: &Painter, item: &Item) -> f32 {
        match item {
            Item::Seg(segs) => {
                let n = segs.len() as f32;
                n * metrics::SEG_ICON_W + (n + 1.0) * metrics::SEG_PAD + 2.0 * metrics::HAIRLINE
            }
            Item::Divider => metrics::HAIRLINE,
            Item::Target => Self::target_w(painter),
            Item::Swatch => metrics::SWATCH_SMALL + 2.0 * metrics::S1,
            Item::Icon(_) => metrics::BUTTON,
            Item::Copy => {
                2.0 * metrics::S3
                    + metrics::ICON
                    + metrics::ICON_GAP
                    + text_w(painter, "Copy", strong(text::CONTROL))
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
            busy: self.menu_open || self.swatch_open,
            ..Default::default()
        };
        if s.stage != Stage::Choosing {
            self.menu_open = false;
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
        let mut target_rect = None;
        let mut swatch_rect = None;
        let mut x = bar.left() + metrics::S2;
        let cy = bar.center().y;
        for (n, item) in self.items(s).into_iter().enumerate() {
            let w = Self::width(&painter, &item);
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
                        let seg_id = item_id.with(k);
                        let resp = ui.interact(r, seg_id, Sense::click());
                        let hover = resp.hovered();
                        // §2.4 / §7.6 tool: the selected one is control-active
                        // with its icon in accent; never an accent fill.
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
                            &painter,
                            Rect::from_center_size(r.center(), vec2(metrics::ICON, metrics::ICON)),
                            ink,
                        );
                        if hover {
                            hovered_tip = Some((seg_id, r, seg.tip, None));
                        }
                        if resp.clicked() && seg.enabled && !seg.on {
                            out.command = Some(seg.command);
                        }
                    }
                }
                Item::Divider => {
                    painter.line_segment(
                        [rect.center_top(), rect.center_bottom()],
                        Stroke::new(metrics::HAIRLINE, p.line),
                    );
                }
                Item::Target => {
                    target_rect = Some(rect);
                    let resp = ui.interact(rect, item_id, Sense::click());
                    if resp.clicked() {
                        self.menu_open = !self.menu_open;
                    }
                    // §7.7 dropdown standing alone: no fill, a line border,
                    // `line-dashed` while hovered or open.
                    let edge = if resp.hovered() || self.menu_open {
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
                    let (_, icon, label) = target_entry(s.target);
                    let icon_r = Rect::from_min_size(
                        pos2(
                            rect.left() + metrics::S2,
                            rect.center().y - metrics::ICON / 2.0,
                        ),
                        vec2(metrics::ICON, metrics::ICON),
                    );
                    icon.paint(&painter, icon_r, p.text_strong);
                    painter.text(
                        pos2(icon_r.right() + metrics::ICON_GAP, rect.center().y),
                        Align2::LEFT_CENTER,
                        label,
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
                    let chevron = if self.menu_open {
                        Icon::ChevronUp
                    } else {
                        Icon::ChevronDown
                    };
                    chevron.paint(&painter, chev, p.text_dim);
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
                Item::Copy => {
                    // §7.6 primary: accent fill, accent-ink 600 text. The one
                    // primary on the overlay.
                    let resp = ui.interact(rect, item_id, Sense::click());
                    painter.rect_filled(rect, metrics::RADIUS, p.accent);
                    let icon_r = Rect::from_min_size(
                        pos2(
                            rect.left() + metrics::S3,
                            rect.center().y - metrics::ICON / 2.0,
                        ),
                        vec2(metrics::ICON, metrics::ICON),
                    );
                    Icon::Copy.paint(&painter, icon_r, p.accent_ink);
                    painter.text(
                        pos2(icon_r.right() + metrics::ICON_GAP, rect.center().y),
                        Align2::LEFT_CENTER,
                        "Copy",
                        strong(text::CONTROL),
                        p.accent_ink,
                    );
                    if resp.hovered() {
                        hovered_tip = Some((item_id, rect, "Copy", Some(s.keys.copy.to_string())));
                    }
                    if resp.clicked() {
                        out.command = Some(BarCommand::Copy);
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

        if self.menu_open
            && let Some(trigger) = target_rect
        {
            let (menu, choice) = self.target_menu(ui, p, theme, trigger, s.target, id);
            out.owns.push(menu);
            if let Some(kind) = choice {
                self.menu_open = false;
                if kind != s.target {
                    out.command = Some(BarCommand::Target(kind));
                }
            } else if let Some(at) = pressed_at
                && !menu.contains(at)
                && !trigger.contains(at)
            {
                self.menu_open = false;
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

    /// The target list: §7.7, opening 4 px below the trigger, exactly its
    /// width, the current item in the selected-row look.
    fn target_menu(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        theme: Mode,
        trigger: Rect,
        current: TargetKind,
        id: Id,
    ) -> (Rect, Option<TargetKind>) {
        let painter = ui
            .ctx()
            .layer_painter(LayerId::new(Order::Foreground, id.with("menu")));
        let item_h = metrics::ICON + 2.0 * metrics::MENU_ITEM_PAD_Y;
        let n = TARGETS.len() as f32;
        let menu = Rect::from_min_size(
            pos2(trigger.left(), trigger.bottom() + metrics::POPOVER_GAP),
            vec2(
                trigger.width(),
                n * item_h + (n - 1.0) * metrics::HAIRLINE + 2.0 * metrics::MENU_PAD,
            ),
        );
        popover_box(&painter, p, theme, menu, metrics::RADIUS_CARD);
        let mut choice = None;
        for (k, &(kind, icon, label)) in TARGETS.iter().enumerate() {
            let r = Rect::from_min_size(
                pos2(
                    menu.left() + metrics::MENU_PAD,
                    menu.top() + metrics::MENU_PAD + k as f32 * (item_h + metrics::HAIRLINE),
                ),
                vec2(menu.width() - 2.0 * metrics::MENU_PAD, item_h),
            );
            let resp = ui.interact(r, id.with(("target", k)), Sense::click());
            let on = kind == current;
            if on {
                painter.rect_filled(r, metrics::RADIUS_SEG, p.control);
            } else if resp.hovered() {
                painter.rect_filled(r, metrics::RADIUS_SEG, p.control_hover);
            }
            let ink = if on || resp.hovered() {
                p.text_strong
            } else {
                p.text
            };
            let icon_r = Rect::from_min_size(
                pos2(
                    r.left() + metrics::MENU_ITEM_PAD_X - metrics::S1,
                    r.center().y - metrics::ICON / 2.0,
                ),
                vec2(metrics::ICON, metrics::ICON),
            );
            icon.paint(&painter, icon_r, ink);
            painter.text(
                pos2(icon_r.right() + metrics::ICON_GAP, r.center().y),
                Align2::LEFT_CENTER,
                label,
                label_font(),
                ink,
            );
            if resp.clicked() {
                choice = Some(kind);
            }
        }
        (menu, choice)
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

fn target_entry(kind: TargetKind) -> (TargetKind, Icon, &'static str) {
    TARGETS
        .iter()
        .copied()
        .find(|t| t.0 == kind)
        .unwrap_or(TARGETS[0])
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
