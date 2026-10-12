//! Painted button widget. §7.4 of STYLE-GUIDE.md.
//! 26 px height, metrics::RADIUS (5.0 px) corners, p.control surface with p.line border.
//! Emphasis: p.accent border and p.text_strong.

use crate::theme::{Palette, metrics, strong, text};
use egui::{Align2, Response, Sense, Stroke, Ui, vec2};

pub fn button(ui: &mut Ui, p: &Palette, label: &str, emphasis: bool) -> Response {
    let font = strong(text::CONTROL);
    let text_w = ui
        .painter()
        .layout_no_wrap(label.to_string(), font.clone(), p.text)
        .size()
        .x;
    let width = text_w + 24.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, metrics::BUTTON), Sense::click());

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = if response.is_pointer_button_down_on() {
            p.control_active
        } else if response.hovered() {
            p.control_hover
        } else {
            p.control
        };

        let stroke_col = if emphasis {
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
            Stroke::new(metrics::HAIRLINE, stroke_col),
            egui::StrokeKind::Inside,
        );

        let text_col = if emphasis {
            p.text_strong
        } else if response.hovered() {
            p.text_strong
        } else {
            p.text
        };

        painter.text(rect.center(), Align2::CENTER_CENTER, label, font, text_col);
    }

    response
}
