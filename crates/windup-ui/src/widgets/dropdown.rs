//! Dropdown for a settings row. §7.7 of STYLE-GUIDE.md.
//! The trigger shows the current entry; its list opens 4 px below, at least
//! as wide as the trigger, and closes on a choice, Esc or a click elsewhere.

use crate::icons::Icon;
use crate::theme::{Mode, Palette, metrics, shadow, strong, text};
use egui::{Align2, Key, Rect, Sense, Stroke, Ui, pos2, vec2};
use std::hash::Hash;

/// Returns the index of a newly chosen entry. `shown` is the trigger's text,
/// which may be a value the list no longer offers. In a disabled `Ui` the
/// trigger is drawn faded and never opens.
pub fn dropdown(
    ui: &mut Ui,
    p: &Palette,
    mode: Mode,
    id_salt: impl Hash,
    entries: &[String],
    current: Option<usize>,
    shown: &str,
) -> Option<usize> {
    dropdown_gated(ui, p, mode, id_salt, entries, &[], current, shown)
}

/// [`dropdown`] where some entries cannot be chosen: `why[i]`, when set, says
/// why entry `i` is disabled (§7.6). It is drawn faded, says why on hover and
/// is never picked. Entries past the end of `why` are enabled.
#[allow(clippy::too_many_arguments)]
pub fn dropdown_gated(
    ui: &mut Ui,
    p: &Palette,
    mode: Mode,
    id_salt: impl Hash,
    entries: &[String],
    why: &[Option<&str>],
    current: Option<usize>,
    shown: &str,
) -> Option<usize> {
    let id = ui.make_persistent_id(id_salt);
    let enabled = ui.is_enabled();
    let mut open = enabled && ui.data(|d| d.get_temp::<bool>(id).unwrap_or(false));

    let (rect, response) =
        ui.allocate_exact_size(vec2(metrics::DROPDOWN_W, metrics::BUTTON), Sense::click());
    if response.clicked() {
        open = !open;
    }

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = if response.is_pointer_button_down_on() {
            p.control_active
        } else if response.hovered() || open {
            p.control_hover
        } else {
            p.control
        };
        let stroke = if open {
            p.accent
        } else if response.hovered() {
            p.text_dim
        } else {
            p.line
        };
        painter.rect_filled(rect, metrics::RADIUS, fill);
        painter.rect_stroke(
            rect,
            metrics::RADIUS,
            Stroke::new(metrics::HAIRLINE, stroke),
            egui::StrokeKind::Inside,
        );
        let chevron = Rect::from_center_size(
            pos2(
                rect.max.x - metrics::PAD_STRIP - metrics::CHEVRON / 2.0,
                rect.center().y,
            ),
            vec2(metrics::CHEVRON, metrics::CHEVRON),
        );
        // A long device name is cut at the chevron rather than run past it.
        let label_clip = Rect::from_min_max(
            rect.min,
            pos2(chevron.min.x - metrics::ICON_GAP, rect.max.y),
        );
        painter.with_clip_rect(label_clip).text(
            pos2(rect.min.x + metrics::PAD_STRIP, rect.center().y),
            Align2::LEFT_CENTER,
            shown,
            strong(text::CONTROL),
            p.text_strong,
        );
        let icon = if open {
            Icon::ChevronUp
        } else {
            Icon::ChevronDown
        };
        icon.paint(painter, chevron, if open { p.accent } else { p.text_dim });
    }

    let mut picked = None;
    if open {
        let font = egui::FontId::proportional(text::CONTROL);
        let widest = entries
            .iter()
            .map(|e| {
                ui.painter()
                    .layout_no_wrap(e.clone(), strong(text::CONTROL), p.text)
                    .size()
                    .x
            })
            .fold(0.0_f32, f32::max);
        let item_w = (widest + metrics::MENU_ITEM_PAD_X * 2.0)
            .max(metrics::DROPDOWN_W - metrics::MENU_PAD * 2.0);
        let list = egui::Area::new(id.with("list"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(rect.left(), rect.bottom() + metrics::POPOVER_GAP))
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(p.popover)
                    .corner_radius(metrics::RADIUS_CARD)
                    .stroke(Stroke::new(metrics::HAIRLINE, p.line_popover))
                    .shadow(shadow::menu(mode))
                    .inner_margin(metrics::MENU_PAD)
                    .show(ui, |ui| {
                        // A long list of sound devices scrolls rather than
                        // running off the window.
                        egui::ScrollArea::vertical()
                            .max_height(metrics::DROPDOWN_LIST_MAX)
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                for (i, entry) in entries.iter().enumerate() {
                                    let gate = why.get(i).copied().flatten();
                                    let sense = if gate.is_some() {
                                        Sense::hover()
                                    } else {
                                        Sense::click()
                                    };
                                    let (item, mut item_response) = ui
                                        .allocate_exact_size(vec2(item_w, metrics::BUTTON), sense);
                                    if let Some(reason) = gate {
                                        item_response = item_response.on_hover_text(reason);
                                    }
                                    let selected = current == Some(i);
                                    let painter = ui.painter();
                                    if item_response.hovered() && gate.is_none() {
                                        painter.rect_filled(
                                            item,
                                            metrics::RADIUS_TIGHT,
                                            p.control_hover,
                                        );
                                    }
                                    if selected {
                                        painter.rect_filled(
                                            Rect::from_min_size(
                                                item.left_top(),
                                                vec2(metrics::NAV_MARK_W, item.height()),
                                            ),
                                            0.0,
                                            p.accent,
                                        );
                                    }
                                    painter.text(
                                        pos2(
                                            item.min.x + metrics::MENU_ITEM_PAD_X,
                                            item.center().y,
                                        ),
                                        Align2::LEFT_CENTER,
                                        entry,
                                        if selected {
                                            strong(text::CONTROL)
                                        } else {
                                            font.clone()
                                        },
                                        if gate.is_some() {
                                            p.text_muted.gamma_multiply(metrics::DISABLED_ALPHA)
                                        } else if selected {
                                            p.text_strong
                                        } else {
                                            p.text
                                        },
                                    );
                                    if gate.is_none() && item_response.clicked() {
                                        picked = Some(i);
                                    }
                                }
                            });
                    });
            });
        let clicked_elsewhere = ui.input(|i| i.pointer.any_click())
            && !response.contains_pointer()
            && !list.response.contains_pointer();
        if picked.is_some() || clicked_elsewhere || ui.input(|i| i.key_pressed(Key::Escape)) {
            open = false;
        }
    }
    ui.data_mut(|d| d.insert_temp(id, open));
    picked.filter(|i| Some(*i) != current)
}
