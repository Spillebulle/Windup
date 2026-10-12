//! Layout helpers: settings row and section eyebrow. §9 of STYLE-GUIDE.md.

use crate::theme::{Palette, metrics, strong, text};
use egui::{Align, Layout, RichText, Ui, vec2};

/// Section eyebrow label in small uppercase muted text.
pub fn eyebrow(ui: &mut Ui, p: Palette, label: &str) {
    ui.add_space(metrics::S4);
    ui.label(
        RichText::new(label)
            .font(strong(text::EYEBROW))
            .color(p.placeholder),
    );
    ui.add_space(metrics::S1);
}

/// One canonical settings row: label, optional description, and right-docked control.
/// Takes full available width and aligns controls across all rows.
pub fn row(
    ui: &mut Ui,
    p: Palette,
    label: &str,
    under: Option<&str>,
    control: impl FnOnce(&mut Ui),
) {
    ui.add_space(metrics::S1);
    let total_w = ui.available_width();
    ui.allocate_ui_with_layout(
        vec2(total_w, metrics::ROW),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(label).size(text::CONTROL).color(p.text));
                if let Some(under) = under {
                    ui.label(RichText::new(under).size(text::SMALL).color(p.text_dim));
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(metrics::PAD_PANEL);
                control(ui);
            });
        },
    );
    ui.add_space(metrics::S1);
}
