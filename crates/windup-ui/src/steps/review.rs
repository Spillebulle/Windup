//! The Steps review: shown when a session stops. A header with the count,
//! the marker and format choices, Export and Discard, then one row per step
//! with its number, thumbnail and caption, to edit or remove. Closing with
//! steps left asks first, since they have not been exported.
//!
//! [`ReviewModel`] holds the decisions and is tested without a window;
//! [`ReviewView`] paints it and reports what was asked for. The host owns
//! the files: it saves, draws thumbnails, opens the editor and exports as
//! the returned [`Effect`] says.

use crate::capture::bar::tooltip;
use crate::capture::recbar::count_text;
use crate::icons::Icon;
use crate::theme::{Mode, Palette, metrics, shadow, strong, text};
use crate::widgets::{button, dropdown};
use egui::{
    Align, Align2, FontId, Id, Key, Layout, Order, Pos2, Rect, RichText, Sense, Stroke, StrokeKind,
    TextEdit, TextureHandle, Ui, UiBuilder, pos2, vec2,
};
use std::collections::HashMap;
use std::time::Duration;
use windup_core::steps::export::NOTHING_TO_EXPORT;
use windup_core::steps::model::StepsDoc;
use windup_core::steps::options::{ExportFormat, MarkerKind, MarkerStyle};

/// Whether the review is asking before it throws the steps away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closing {
    Open,
    Confirming,
}

/// What the person asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum ReviewAction {
    /// Open step `i` in the capture editor.
    Edit(usize),
    Remove(usize),
    Caption(usize, String),
    Marker(MarkerKind),
    Format(ExportFormat),
    Export,
    /// The window's close, or the header's Discard: asks first while steps
    /// remain.
    Close,
    /// The confirm card's Keep, or Esc on it.
    Keep,
    /// The confirm card's Discard.
    Discard,
}

/// What the host does after an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    /// Write `steps.json`.
    Save,
    /// Draw the thumbnails again (the numbers or the marker changed), and
    /// write `steps.json`.
    Rethumb,
    OpenEditor(usize),
    /// Export in this format; the host clears `busy` when it is done.
    Export(ExportFormat),
    /// Delete the session and close.
    Discard,
    /// Close; there was nothing to keep.
    Closed,
}

const EXPORT_RUNNING: &str = "The steps are being exported.";

#[derive(Clone, Debug, PartialEq)]
pub struct ReviewModel {
    pub doc: StepsDoc,
    pub style: MarkerStyle,
    pub format: ExportFormat,
    pub closing: Closing,
    /// Why the last export failed, shown under the header.
    pub error: Option<String>,
    /// An export is running.
    pub busy: bool,
}

impl ReviewModel {
    pub fn new(doc: StepsDoc, style: MarkerStyle, format: ExportFormat) -> Self {
        Self {
            doc,
            style,
            format,
            closing: Closing::Open,
            error: None,
            busy: false,
        }
    }

    pub fn apply(&mut self, a: ReviewAction) -> Effect {
        let n = self.doc.steps.len();
        match a {
            ReviewAction::Edit(i) if i < n => Effect::OpenEditor(i),
            ReviewAction::Remove(i) if self.doc.remove(i).is_some() => Effect::Rethumb,
            ReviewAction::Caption(i, text) if i < n => {
                self.doc.set_caption(i, &text);
                Effect::Save
            }
            ReviewAction::Marker(kind) => {
                self.style.kind = kind;
                Effect::Rethumb
            }
            ReviewAction::Format(format) => {
                self.format = format;
                Effect::None
            }
            ReviewAction::Export if self.export_reason().is_ok() => {
                self.busy = true;
                self.error = None;
                Effect::Export(self.format)
            }
            ReviewAction::Close if n == 0 => Effect::Closed,
            ReviewAction::Close => {
                self.closing = Closing::Confirming;
                Effect::None
            }
            ReviewAction::Keep => {
                self.closing = Closing::Open;
                Effect::None
            }
            ReviewAction::Discard => Effect::Discard,
            _ => Effect::None,
        }
    }

    /// Why Export cannot run now, for its tooltip.
    pub fn export_reason(&self) -> Result<(), &'static str> {
        if self.doc.steps.is_empty() {
            Err(NOTHING_TO_EXPORT)
        } else if self.busy {
            Err(EXPORT_RUNNING)
        } else {
            Ok(())
        }
    }
}

const CONFIRM: &str = "Discard these steps? They have not been exported.";

/// A control the pointer is over, for its tooltip.
type Tip = Option<(Id, Rect, &'static str)>;

#[derive(Default)]
pub struct ReviewView {
    /// Captions as typed, by step file. The model's caption wins whenever
    /// its field does not have focus.
    drafts: HashMap<String, String>,
    tip: Option<(Id, f64)>,
}

impl ReviewView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Paints the review over `ui`'s space. `thumbs[i]` is step `i`'s
    /// picture with its marker, once the host has made it.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        mode: Mode,
        m: &ReviewModel,
        thumbs: &[Option<TextureHandle>],
    ) -> Vec<ReviewAction> {
        let ctx = ui.ctx().clone();
        let full = ui.max_rect();
        let confirming = m.closing == Closing::Confirming;
        let mut out = Vec::new();
        let mut hovered: Tip = None;
        ui.painter().rect_filled(full, 0.0, p.window);
        self.drafts
            .retain(|file, _| m.doc.steps.iter().any(|s| &s.file == file));

        let inner = full.shrink(metrics::S4);
        ui.add_enabled_ui(!confirming, |ui| {
            let header = Rect::from_min_size(inner.min, vec2(inner.width(), metrics::BUTTON));
            header_row(ui, p, mode, m, header, &mut out, &mut hovered);
            let mut top = header.bottom() + metrics::S3;
            if let Some(error) = &m.error {
                let galley = ui.painter().layout(
                    error.clone(),
                    FontId::proportional(text::BODY),
                    p.critical,
                    inner.width(),
                );
                let h = galley.size().y;
                ui.painter()
                    .galley(pos2(inner.left(), top), galley, p.critical);
                top += h + metrics::S3;
            }
            ui.painter().hline(
                full.x_range(),
                top,
                Stroke::new(metrics::HAIRLINE, p.line_soft),
            );
            let list = Rect::from_min_max(pos2(inner.left(), top + metrics::HAIRLINE), inner.max);
            ui.scope_builder(UiBuilder::new().max_rect(list), |ui| {
                if m.doc.steps.is_empty() {
                    empty(ui, p, list);
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt("steps-review-list")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for i in 0..m.doc.steps.len() {
                            let thumb = thumbs.get(i).and_then(Option::as_ref);
                            self.row(ui, p, m, i, thumb, &mut out, &mut hovered);
                        }
                    });
            });
        });

        if confirming {
            confirm(&ctx, p, mode, full, &mut out);
        }

        // The tooltip, after the §7.17 delay.
        let now = ui.input(|i| i.time);
        match hovered {
            Some((id, rect, label)) => {
                let since = match self.tip {
                    Some((held, t)) if held == id => t,
                    _ => {
                        self.tip = Some((id, now));
                        now
                    }
                };
                let delay = metrics::TOOLTIP_DELAY_MS as f64 / 1000.0;
                if now - since >= delay {
                    tooltip(&ctx, p, mode, rect, ctx.screen_rect(), label, None);
                } else {
                    ctx.request_repaint_after(Duration::from_secs_f64(delay - (now - since)));
                }
            }
            None => self.tip = None,
        }
        out
    }

    /// One step: its number, thumbnail and caption, then Edit and Remove.
    #[allow(clippy::too_many_arguments)]
    fn row(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        m: &ReviewModel,
        i: usize,
        thumb: Option<&TextureHandle>,
        out: &mut Vec<ReviewAction>,
        hovered: &mut Tip,
    ) {
        let step = &m.doc.steps[i];
        let thumb_w = metrics::REVIEW_THUMB_W;
        let thumb_h = thumb.map_or(thumb_w * metrics::REVIEW_PLACEHOLDER_ASPECT, |t| {
            let s = t.size_vec2();
            thumb_w * s.y / s.x.max(1.0)
        });
        let (row, _) = ui.allocate_exact_size(
            vec2(ui.available_width(), thumb_h + 2.0 * metrics::S3),
            Sense::hover(),
        );
        if !ui.is_rect_visible(row) {
            return;
        }
        let painter = ui.painter().clone();
        if i > 0 {
            painter.hline(
                row.x_range(),
                row.top(),
                Stroke::new(metrics::HAIRLINE, p.line_soft),
            );
        }
        let top = row.top() + metrics::S3;

        // §7.13 neutral badge, mono like a count, as wide as two digits.
        let font = FontId::monospace(text::TINY);
        let digits = painter
            .layout_no_wrap("00".into(), font.clone(), p.text_muted)
            .size();
        let chip = Rect::from_min_size(
            pos2(row.left(), top),
            vec2(
                digits.x + 2.0 * metrics::CHIP_PAD_X,
                digits.y + 2.0 * metrics::CHIP_PAD_Y,
            ),
        );
        painter.rect_filled(chip, metrics::RADIUS_TIGHT, p.control);
        painter.text(
            chip.center(),
            Align2::CENTER_CENTER,
            (i + 1).to_string(),
            font,
            p.text_muted,
        );

        let pic = Rect::from_min_size(
            pos2(chip.right() + metrics::S3, top),
            vec2(thumb_w, thumb_h),
        );
        match thumb {
            Some(t) => {
                painter.image(
                    t.id(),
                    pic,
                    Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            None => {
                painter.rect_filled(pic, metrics::RADIUS_TIGHT, p.control);
            }
        }
        painter.rect_stroke(
            pic,
            metrics::RADIUS_TIGHT,
            Stroke::new(metrics::HAIRLINE, p.line),
            StrokeKind::Inside,
        );

        // Remove at the far right, Edit before it (§7.6: 8 px apart).
        let side = vec2(metrics::BUTTON, metrics::BUTTON);
        let remove = Rect::from_min_size(pos2(row.right() - side.x, top), side);
        let edit = remove.translate(vec2(-(side.x + metrics::S2), 0.0));
        let edit_id = Id::new(("steps-edit", &step.file));
        if icon_button(ui, p, edit_id, edit, Icon::Pen, "Edit", hovered) {
            out.push(ReviewAction::Edit(i));
        }
        let remove_id = Id::new(("steps-remove", &step.file));
        if icon_button(ui, p, remove_id, remove, Icon::Trash, "Remove", hovered) {
            out.push(ReviewAction::Remove(i));
        }

        // §7.11 field, grown to the row; inside it a frameless multiline
        // edit in the field's tokens.
        let field = Rect::from_min_max(
            pos2(pic.right() + metrics::S3, top),
            pos2(edit.left() - metrics::S3, row.bottom() - metrics::S3),
        );
        let id = Id::new(("steps-caption", &step.file));
        let focused = ui.ctx().memory(|mem| mem.has_focus(id));
        painter.rect_filled(field, metrics::RADIUS_TOOL, p.field);
        let edge = if focused {
            Stroke::new(2.0 * metrics::HAIRLINE, p.accent_ring)
        } else {
            Stroke::new(metrics::HAIRLINE, p.line)
        };
        painter.rect_stroke(field, metrics::RADIUS_TOOL, edge, StrokeKind::Inside);
        let draft = self
            .drafts
            .entry(step.file.clone())
            .or_insert_with(|| step.caption.clone());
        if !focused && *draft != step.caption {
            draft.clone_from(&step.caption);
        }
        let text_rect = field.shrink2(vec2(metrics::S3, metrics::S2));
        let changed = ui
            .scope_builder(UiBuilder::new().max_rect(text_rect), |ui| {
                ui.set_clip_rect(text_rect.intersect(ui.clip_rect()));
                ui.add(
                    TextEdit::multiline(draft)
                        .id(id)
                        .frame(false)
                        .font(FontId::proportional(text::BODY))
                        .text_color(p.text)
                        .hint_text(
                            RichText::new("Add a caption")
                                .font(FontId::proportional(text::BODY))
                                .color(p.placeholder),
                        )
                        .desired_width(text_rect.width())
                        .min_size(text_rect.size()),
                )
                .changed()
            })
            .inner;
        if changed {
            out.push(ReviewAction::Caption(i, draft.clone()));
        }
    }
}

/// "3 steps" at the left; Marker, Format, Export and Discard at the right,
/// Discard last.
fn header_row(
    ui: &mut Ui,
    p: &Palette,
    mode: Mode,
    m: &ReviewModel,
    header: Rect,
    out: &mut Vec<ReviewAction>,
    hovered: &mut Tip,
) {
    ui.painter().text(
        header.left_center(),
        Align2::LEFT_CENTER,
        count_text(m.doc.steps.len() as u32),
        strong(text::PAGE),
        p.text_strong,
    );
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(header)
            .layout(Layout::right_to_left(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = metrics::S2;
            let side = vec2(metrics::BUTTON, metrics::BUTTON);
            let (discard, _) = ui.allocate_exact_size(side, Sense::hover());
            let tip = "Discard these steps";
            if icon_button(
                ui,
                p,
                Id::new("steps-discard"),
                discard,
                Icon::Trash,
                tip,
                hovered,
            ) {
                out.push(ReviewAction::Close);
            }
            if primary(ui, p, Icon::Download, "Export", m.export_reason(), hovered) {
                out.push(ReviewAction::Export);
            }
            let formats: Vec<String> = ExportFormat::ALL
                .iter()
                .map(|f| f.label().to_string())
                .collect();
            let at = ExportFormat::ALL.iter().position(|f| *f == m.format);
            let shown = m.format.label();
            if let Some(i) = dropdown(ui, p, mode, "steps-format", &formats, at, shown) {
                out.push(ReviewAction::Format(ExportFormat::ALL[i]));
            }
            let kinds: Vec<String> = MarkerKind::ALL
                .iter()
                .map(|k| k.label().to_string())
                .collect();
            let at = MarkerKind::ALL.iter().position(|k| *k == m.style.kind);
            let shown = m.style.kind.label();
            if let Some(i) = dropdown(ui, p, mode, "steps-marker", &kinds, at, shown) {
                out.push(ReviewAction::Marker(MarkerKind::ALL[i]));
            }
        },
    );
}

/// §7.6 icon button: no fill until hovered, the icon muted then strong,
/// always tooltipped. True when clicked.
fn icon_button(
    ui: &mut Ui,
    p: &Palette,
    id: Id,
    rect: Rect,
    icon: Icon,
    tip: &'static str,
    hovered: &mut Tip,
) -> bool {
    let resp = ui.interact(rect, id, Sense::click());
    let hover = resp.hovered();
    let painter = ui.painter();
    if hover {
        painter.rect_filled(rect, metrics::RADIUS, p.control_hover);
        *hovered = Some((id, rect, tip));
    }
    icon.paint(
        painter,
        Rect::from_center_size(rect.center(), vec2(metrics::ICON, metrics::ICON)),
        if hover { p.text_strong } else { p.text_muted },
    );
    resp.clicked()
}

/// §7.6 primary: accent fill, accent-ink 600 text and a leading icon. When
/// it cannot run it is faded and its tooltip says why.
fn primary(
    ui: &mut Ui,
    p: &Palette,
    icon: Icon,
    label: &str,
    ready: Result<(), &'static str>,
    hovered: &mut Tip,
) -> bool {
    let font = strong(text::CONTROL);
    let text_w = ui
        .painter()
        .layout_no_wrap(label.to_string(), font.clone(), p.accent_ink)
        .size()
        .x;
    let w = metrics::S3 + metrics::ICON + metrics::ICON_GAP + text_w + metrics::S3;
    let sense = if ready.is_ok() {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, metrics::BUTTON), sense);
    let fade = if ready.is_ok() {
        1.0
    } else {
        metrics::DISABLED_ALPHA
    };
    let fill = if resp.hovered() && ready.is_ok() {
        p.accent
            .lerp_to_gamma(p.text_strong, metrics::PRIMARY_HOVER_MIX)
    } else {
        p.accent
    };
    let painter = ui.painter();
    painter.rect_filled(rect, metrics::RADIUS, fill.gamma_multiply(fade));
    let ink = p.accent_ink.gamma_multiply(fade);
    let icon_r = Rect::from_min_size(
        pos2(
            rect.left() + metrics::S3,
            rect.center().y - metrics::ICON / 2.0,
        ),
        vec2(metrics::ICON, metrics::ICON),
    );
    icon.paint(painter, icon_r, ink);
    painter.text(
        pos2(icon_r.right() + metrics::ICON_GAP, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        ink,
    );
    if let Err(why) = ready
        && resp.hovered()
    {
        *hovered = Some((resp.id, rect, why));
    }
    ready.is_ok() && resp.clicked()
}

/// §7.19 empty state: the icon in `line-dashed` and one sentence.
fn empty(ui: &mut Ui, p: &Palette, list: Rect) {
    let painter = ui.painter();
    let c = list.center();
    let icon = Rect::from_center_size(
        pos2(c.x, c.y - metrics::S5 / 2.0),
        vec2(metrics::S5, metrics::S5),
    );
    Icon::Footprints.paint(painter, icon, p.line_dashed);
    painter.text(
        pos2(c.x, icon.bottom() + metrics::S3),
        Align2::CENTER_TOP,
        "There are no steps left.",
        FontId::proportional(text::BODY),
        p.text_dim,
    );
}

/// The card that asks before throwing the steps away, over the dimmed
/// review. Esc keeps them.
fn confirm(ctx: &egui::Context, p: &Palette, mode: Mode, full: Rect, out: &mut Vec<ReviewAction>) {
    ctx.layer_painter(egui::LayerId::new(
        Order::Middle,
        Id::new("steps-review-dim"),
    ))
    .rect_filled(full, 0.0, p.backdrop.gamma_multiply(metrics::DIM_ALPHA));
    let wrap = metrics::NOTICE_W - 2.0 * metrics::S4;
    let galley = ctx.fonts(|f| {
        f.layout(
            CONFIRM.to_owned(),
            FontId::proportional(text::BODY),
            p.text,
            wrap,
        )
    });
    let size = vec2(
        metrics::NOTICE_W,
        metrics::S4 + galley.size().y + metrics::S3 + metrics::BUTTON + metrics::S4,
    );
    let card = Rect::from_center_size(full.center(), size);
    egui::Area::new(Id::new("steps-review-confirm"))
        .order(Order::Foreground)
        .fixed_pos(card.min)
        .show(ctx, |ui| {
            ui.allocate_exact_size(size, Sense::hover());
            let painter = ui.painter();
            painter.add(shadow::modal(mode).as_shape(card, metrics::RADIUS_MODAL));
            painter.rect_filled(card, metrics::RADIUS_MODAL, p.chrome);
            painter.rect_stroke(
                card,
                metrics::RADIUS_MODAL,
                Stroke::new(metrics::HAIRLINE, p.line_popover),
                StrokeKind::Inside,
            );
            let inner = card.shrink(metrics::S4);
            painter.galley(inner.min, galley, p.text);
            let buttons = Rect::from_min_max(
                pos2(inner.left(), inner.bottom() - metrics::BUTTON),
                inner.max,
            );
            ui.scope_builder(
                UiBuilder::new()
                    .max_rect(buttons)
                    .layout(Layout::right_to_left(Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = metrics::S2;
                    if danger(ui, p, "Discard") {
                        out.push(ReviewAction::Discard);
                    }
                    if button(ui, p, "Keep", false).clicked() {
                        out.push(ReviewAction::Keep);
                    }
                },
            );
        });
    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        out.push(ReviewAction::Keep);
    }
}

/// §7.6 danger: `critical` text and a `critical` edge at 40 %, beside the
/// sentence that says what is lost.
fn danger(ui: &mut Ui, p: &Palette, label: &str) -> bool {
    let font = strong(text::CONTROL);
    let text_w = ui
        .painter()
        .layout_no_wrap(label.to_string(), font.clone(), p.critical)
        .size()
        .x;
    let (rect, resp) = ui.allocate_exact_size(
        vec2(text_w + 2.0 * metrics::S3, metrics::BUTTON),
        Sense::click(),
    );
    let painter = ui.painter();
    if resp.hovered() {
        painter.rect_filled(rect, metrics::RADIUS, p.control_hover);
    }
    painter.rect_stroke(
        rect,
        metrics::RADIUS,
        Stroke::new(
            metrics::HAIRLINE,
            p.critical.gamma_multiply(metrics::DANGER_EDGE_ALPHA),
        ),
        StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        font,
        p.critical,
    );
    resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windup_core::steps::model::{Button, Step, StepCause, StepsDoc};

    fn doc(n: usize) -> StepsDoc {
        let mut doc = StepsDoc {
            version: 1,
            started: "2026-10-30T14:25:01".into(),
            next: 1,
            steps: Vec::new(),
        };
        for _ in 0..n {
            let file = doc.next_file();
            doc.push(Step {
                file,
                cause: StepCause::Click(Button::Left),
                pointer: Some([10.0, 20.0]),
                scale: 1.0,
                monitor: "DISPLAY1".into(),
                caption: String::new(),
                at_ms: 0,
            });
        }
        doc
    }

    fn model(n: usize) -> ReviewModel {
        ReviewModel::new(doc(n), MarkerStyle::default(), ExportFormat::Html)
    }

    #[test]
    fn removing_renumbers_and_rethumbs() {
        let mut m = model(3);
        assert_eq!(m.apply(ReviewAction::Remove(1)), Effect::Rethumb);
        assert_eq!(m.doc.steps.len(), 2);
        let files: Vec<_> = m.doc.steps.iter().map(|s| s.file.as_str()).collect();
        assert_eq!(files, ["01.png", "03.png"]);
        assert_eq!(m.apply(ReviewAction::Remove(7)), Effect::None);
    }

    #[test]
    fn closing_with_steps_asks_first() {
        let mut m = model(2);
        assert_eq!(m.apply(ReviewAction::Close), Effect::None);
        assert_eq!(m.closing, Closing::Confirming);
        assert_eq!(m.apply(ReviewAction::Keep), Effect::None);
        assert_eq!(m.closing, Closing::Open);
        m.apply(ReviewAction::Close);
        assert_eq!(m.apply(ReviewAction::Discard), Effect::Discard);
    }

    #[test]
    fn closing_with_no_steps_just_closes() {
        let mut m = model(0);
        assert_eq!(m.apply(ReviewAction::Close), Effect::Closed);
        assert_eq!(m.closing, Closing::Open);
    }

    #[test]
    fn export_is_refused_with_its_reason_when_empty() {
        let mut m = model(0);
        assert_eq!(m.export_reason(), Err("There are no steps to export."));
        assert_eq!(m.apply(ReviewAction::Export), Effect::None);
        assert!(!m.busy);
    }

    #[test]
    fn export_runs_once_at_a_time_in_the_chosen_format() {
        let mut m = model(2);
        assert_eq!(m.export_reason(), Ok(()));
        assert_eq!(
            m.apply(ReviewAction::Format(ExportFormat::Pdf)),
            Effect::None
        );
        m.error = Some("The folder is in use.".into());
        assert_eq!(
            m.apply(ReviewAction::Export),
            Effect::Export(ExportFormat::Pdf)
        );
        assert!(m.busy);
        assert_eq!(m.error, None);
        assert!(m.export_reason().is_err());
        assert_eq!(m.apply(ReviewAction::Export), Effect::None);
    }

    #[test]
    fn a_caption_is_saved() {
        let mut m = model(2);
        assert_eq!(
            m.apply(ReviewAction::Caption(1, "Open the menu".into())),
            Effect::Save
        );
        assert_eq!(m.doc.steps[1].caption, "Open the menu");
        assert_eq!(
            m.apply(ReviewAction::Caption(5, "Nowhere".into())),
            Effect::None
        );
    }

    #[test]
    fn a_marker_change_rethumbs_and_edit_opens_the_editor() {
        let mut m = model(2);
        assert_eq!(
            m.apply(ReviewAction::Marker(MarkerKind::Ring)),
            Effect::Rethumb
        );
        assert_eq!(m.style.kind, MarkerKind::Ring);
        assert_eq!(m.apply(ReviewAction::Edit(1)), Effect::OpenEditor(1));
        assert_eq!(m.apply(ReviewAction::Edit(2)), Effect::None);
    }

    /// The review in a window of its own size, frame by frame.
    struct Rig {
        ctx: egui::Context,
        view: ReviewView,
        t: f64,
    }

    impl Rig {
        fn new() -> Self {
            let ctx = egui::Context::default();
            crate::theme::install_fonts(&ctx);
            let _ = ctx.run(egui::RawInput::default(), |_| {});
            Rig {
                ctx,
                view: ReviewView::new(),
                t: 0.0,
            }
        }

        fn frame(&mut self, m: &ReviewModel, events: Vec<egui::Event>) -> Vec<ReviewAction> {
            self.t += 0.016;
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Pos2::ZERO,
                    vec2(metrics::REVIEW_W, metrics::REVIEW_H),
                )),
                time: Some(self.t),
                focused: true,
                events,
                ..Default::default()
            };
            let p = Palette::dark(28.0);
            let mut out = Vec::new();
            let view = &mut self.view;
            let _ = self.ctx.run(raw, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        out = view.show(ui, &p, Mode::Dark, m, &[None, None]);
                    });
            });
            out
        }

        fn click(&mut self, m: &ReviewModel, at: Pos2) -> Vec<ReviewAction> {
            let b = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            let mut got = Vec::new();
            for ev in [egui::Event::PointerMoved(at), b(true), b(false)] {
                got.extend(self.frame(m, vec![ev]));
            }
            got
        }
    }

    #[test]
    fn the_header_discard_asks_and_esc_on_the_card_keeps() {
        let mut rig = Rig::new();
        let mut m = model(2);
        let discard = pos2(
            metrics::REVIEW_W - metrics::S4 - metrics::BUTTON / 2.0,
            metrics::S4 + metrics::BUTTON / 2.0,
        );
        assert_eq!(rig.click(&m, discard), [ReviewAction::Close]);
        m.apply(ReviewAction::Close);
        // Behind the card the header no longer answers.
        assert!(rig.click(&m, discard).is_empty());
        let esc = egui::Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        assert_eq!(rig.frame(&m, vec![esc]), [ReviewAction::Keep]);
    }

    #[test]
    fn a_row_reports_edit_and_remove() {
        let mut rig = Rig::new();
        let m = model(2);
        rig.frame(&m, vec![]);
        // The first row's buttons, right-aligned at its top.
        let list_top = metrics::S4 + metrics::BUTTON + metrics::S3 + metrics::HAIRLINE;
        let y = list_top + metrics::S3 + metrics::BUTTON / 2.0;
        let remove = pos2(metrics::REVIEW_W - metrics::S4 - metrics::BUTTON / 2.0, y);
        let edit = remove - vec2(metrics::BUTTON + metrics::S2, 0.0);
        assert_eq!(rig.click(&m, edit), [ReviewAction::Edit(0)]);
        assert_eq!(rig.click(&m, remove), [ReviewAction::Remove(0)]);
    }
}
