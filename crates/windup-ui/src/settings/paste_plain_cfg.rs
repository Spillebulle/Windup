//! Paste Plain Text tool settings.

use crate::theme::{Palette, metrics};
use crate::widgets::{eyebrow, row, shortcut_recorder, toggle};
use egui::Ui;
use windup_core::Config;

pub fn render(ui: &mut Ui, config: &mut Config, p: &Palette) {
    eyebrow(ui, *p, "TOOL STATUS");

    row(
        ui,
        *p,
        "Enable Paste Plain Text",
        Some("Listen for shortcut to strip rich formatting and paste unstyled text."),
        |ui| {
            let mut enabled = config.paste_plain.enabled;
            if toggle(ui, *p, &mut enabled) {
                config.paste_plain.enabled = enabled;
                config.save();
            }
        },
    );

    ui.add_space(metrics::S4);

    eyebrow(ui, *p, "BEHAVIOR & SHORTCUT");

    row(
        ui,
        *p,
        "Paste plain shortcut",
        Some("Global shortcut that triggers immediate plain text paste."),
        |ui| {
            let default_sc = windup_core::shortcut::Shortcut::super_shift_v();
            if shortcut_recorder(
                ui,
                p,
                "cfg_paste_plain_shortcut",
                &mut config.paste_plain.shortcut,
                Some(&default_sc),
            ) {
                config.save();
            }
        },
    );
}
