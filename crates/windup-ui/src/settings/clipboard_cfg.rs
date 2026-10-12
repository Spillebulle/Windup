//! Clipboard History tool settings.

use crate::theme::{Palette, metrics};
use crate::widgets::{eyebrow, row, shortcut_recorder, slider_usize, toggle};
use egui::Ui;
use windup_core::Config;

pub fn render(ui: &mut Ui, config: &mut Config, p: &Palette) {
    eyebrow(ui, *p, "TOOL STATUS");

    // Enable Clipboard History
    row(
        ui,
        *p,
        "Enable Clipboard History",
        Some("Monitor clipboard events and maintain history list."),
        |ui| {
            let mut enabled = config.clipboard.enabled;
            if toggle(ui, *p, &mut enabled) {
                config.clipboard.enabled = enabled;
                config.save();
            }
        },
    );

    ui.add_space(metrics::S4);

    eyebrow(ui, *p, "BEHAVIOR & SHORTCUT");

    // Global Shortcut
    row(
        ui,
        *p,
        "Popup shortcut",
        Some("Shortcut to trigger the floating history popup at your cursor."),
        |ui| {
            let default_sc = windup_core::shortcut::Shortcut::super_v();
            if shortcut_recorder(
                ui,
                p,
                "cfg_clipboard_shortcut",
                &mut config.clipboard.shortcut,
                Some(&default_sc),
            ) {
                config.save();
            }
        },
    );

    ui.add_space(metrics::S2);

    // Auto paste toggle
    row(
        ui,
        *p,
        "Paste immediately on select",
        Some("Instantly paste into active target window. If disabled, populates clipboard only."),
        |ui| {
            let mut auto_paste = config.clipboard.auto_paste;
            if toggle(ui, *p, &mut auto_paste) {
                config.clipboard.auto_paste = auto_paste;
                config.save();
            }
        },
    );

    ui.add_space(metrics::S2);

    // Save image previews toggle
    row(
        ui,
        *p,
        "Capture images",
        Some("Store copied images and render thumbnail previews."),
        |ui| {
            let mut save_img = config.clipboard.save_images;
            if toggle(ui, *p, &mut save_img) {
                config.clipboard.save_images = save_img;
                config.save();
            }
        },
    );

    ui.add_space(metrics::S2);

    // Max history items
    row(
        ui,
        *p,
        "Maximum history items",
        Some("Oldest items are automatically removed when capacity is reached."),
        |ui| {
            let mut max = config.clipboard.max_items;
            let response = slider_usize(ui, *p, &mut max, 10, 1000, 10);
            if response.changed() {
                config.clipboard.max_items = max;
                config.save();
            }
        },
    );
}
