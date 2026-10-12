//! Painted slider widget. §7.10 of STYLE-GUIDE.md.
//! 3 px rail with accent fill, round 6 px knob, snapped to step.

use crate::theme::{Palette, metrics, text};
use egui::{Rect, Sense, Stroke, Ui, pos2, vec2};

/// A painted slider, §7.10: a 3 px rail with an accent fill and a round knob.
pub fn slider(
    ui: &mut Ui,
    p: Palette,
    value: &mut f32,
    min: f32,
    max: f32,
    step: f32,
) -> egui::Response {
    let width = 160.0;
    let (rect, mut response) =
        ui.allocate_exact_size(vec2(width, metrics::BUTTON), Sense::click_and_drag());

    let rail = Rect::from_center_size(rect.center(), vec2(width, 3.0));
    ui.painter().rect_filled(rail, 1.5, p.rail);

    let mut t = ((*value - min) / (max - min)).clamp(0.0, 1.0);
    let moving = response.dragged() || response.clicked();
    if moving && let Some(at) = response.interact_pointer_pos() {
        t = ((at.x - rail.left()) / rail.width()).clamp(0.0, 1.0);
        let raw = min + t * (max - min);
        let new_val = ((raw / step).round() * step).clamp(min, max);
        if new_val != *value {
            *value = new_val;
            response.mark_changed();
        }
        t = ((*value - min) / (max - min)).clamp(0.0, 1.0);
    }

    let filled = Rect::from_min_size(rail.left_top(), vec2(rail.width() * t, rail.height()));
    ui.painter().rect_filled(filled, 1.5, p.accent);
    ui.painter().circle_filled(
        pos2(rail.left() + rail.width() * t, rail.center().y),
        6.0,
        p.knob,
    );
    response
}

/// A painted slider for integer values, displaying a painted editable figure box alongside (§7.6).
pub fn slider_usize(
    ui: &mut Ui,
    p: Palette,
    value: &mut usize,
    min: usize,
    max: usize,
    step: usize,
) -> egui::Response {
    let old_val = *value;
    let mut float_val = (*value as f32).clamp(min as f32, max as f32);
    let mut changed = false;

    let response = ui.horizontal(|ui| {
        let mut s_resp = slider(
            ui,
            p,
            &mut float_val,
            min as f32,
            max as f32,
            step.max(1) as f32,
        );
        if s_resp.dragged() || s_resp.clicked() {
            *value = float_val.round() as usize;
            if *value != old_val {
                changed = true;
                s_resp.mark_changed();
            }
        }

        ui.add_space(metrics::S2);

        // Painted numeric field box per Design-Principles (§7.6)
        let box_w = 60.0;
        let box_h = metrics::BUTTON;
        let id = ui.make_persistent_id("slider_usize_edit_buf");
        let mut text_buf = ui.data_mut(|d| {
            d.get_temp::<String>(id)
                .unwrap_or_else(|| value.to_string())
        });

        let (rect, resp) = ui.allocate_exact_size(vec2(box_w, box_h), Sense::click_and_drag());
        let focused_id = ui.make_persistent_id("slider_usize_focused");
        let mut is_focused = ui.data_mut(|d| d.get_temp::<bool>(focused_id).unwrap_or(false));

        if resp.clicked() {
            is_focused = true;
            text_buf = value.to_string();
            ui.data_mut(|d| d.insert_temp(focused_id, true));
        }

        // Drag vertically to nudge value
        if resp.dragged() {
            let dy = resp.drag_delta().y;
            if dy.abs() >= 1.0 {
                let delta = (-dy.signum()) as isize * (step.max(1) as isize);
                let new_v = ((*value as isize) + delta).clamp(min as isize, max as isize) as usize;
                if new_v != *value {
                    *value = new_v;
                    text_buf = value.to_string();
                    changed = true;
                }
            }
        }

        let painter = ui.painter();
        let border_col = if is_focused {
            p.accent
        } else if resp.hovered() {
            p.text_dim
        } else {
            p.line
        };

        painter.rect_filled(rect, metrics::RADIUS_TIGHT, p.field);
        painter.rect_stroke(
            rect,
            metrics::RADIUS_TIGHT,
            Stroke::new(metrics::HAIRLINE, border_col),
            egui::StrokeKind::Inside,
        );

        if is_focused {
            let mut edit_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect.shrink2(vec2(4.0, 2.0)))
                    .layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight)),
            );
            let te = egui::TextEdit::singleline(&mut text_buf)
                .font(egui::FontId::monospace(text::CONTROL))
                .text_color(p.text_strong)
                .frame(false)
                .margin(egui::Margin::ZERO);
            let te_resp = edit_ui.add(te);
            te_resp.request_focus();

            if te_resp.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                ui.data_mut(|d| d.insert_temp(focused_id, false));
                if let Ok(parsed) = text_buf.trim().parse::<usize>() {
                    let clamped = parsed.clamp(min, 100_000);
                    if clamped != *value {
                        *value = clamped;
                        changed = true;
                    }
                }
                text_buf = value.to_string();
            } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                ui.data_mut(|d| d.insert_temp(focused_id, false));
                text_buf = value.to_string();
            }
        } else {
            text_buf = value.to_string();
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &text_buf,
                egui::FontId::monospace(text::CONTROL),
                p.text,
            );
        }

        ui.data_mut(|d| d.insert_temp(id, text_buf));

        if changed {
            s_resp.mark_changed();
        }

        s_resp
    }).inner;

    let mut final_resp = response;
    if changed {
        final_resp.mark_changed();
    }
    final_resp
}
