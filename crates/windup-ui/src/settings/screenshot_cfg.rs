//! Screenshot tool settings: the instant screenshot, where files go, and the
//! editor's keys.

use super::HotkeyHint;
use super::capture_cfg::{hotkey_note, shortcut_row};
use crate::theme::{Palette, metrics, text};
use crate::widgets::{button, eyebrow, row, segmented, shortcut_badge, shortcut_recorder, toggle};
use egui::Ui;
use windup_core::Config;
use windup_core::capture::output;
use windup_core::config::{EditorKeys, ImageFormat};
use windup_core::shortcut::Shortcut;
use windup_core::tool::ToolId;

pub fn render(ui: &mut Ui, config: &mut Config, p: &Palette, hint: HotkeyHint) {
    eyebrow(ui, *p, "TOOL STATUS");

    row(
        ui,
        *p,
        "Enable Screenshot",
        Some("Take a screenshot straight away and open it in the editor."),
        |ui| {
            let mut enabled = config.screenshot.enabled;
            if toggle(ui, *p, &mut enabled) {
                config.screenshot.enabled = enabled;
                config.save();
            }
        },
    );

    ui.add_space(metrics::S4);

    eyebrow(ui, *p, "SHORTCUT");

    let mut shortcut = config.screenshot.shortcut.clone();
    if shortcut_row(
        ui,
        config,
        p,
        ToolId::Screenshot,
        "Screenshot shortcut",
        "Global shortcut that takes a screenshot straight away.",
        &mut shortcut,
    ) {
        config.screenshot.shortcut = shortcut;
        config.save();
    }
    hotkey_note(ui, p, hint);

    ui.add_space(metrics::S4);

    eyebrow(ui, *p, "SAVING");

    let folder = config
        .screenshot
        .save_dir
        .clone()
        .or_else(output::default_dir)
        .map(|d| d.display().to_string())
        .unwrap_or_else(|| "Pictures".to_owned());
    row(ui, *p, "Save folder", Some(&folder), |ui| {
        if config.screenshot.save_dir.is_some() && button(ui, p, "Reset", false).clicked() {
            config.screenshot.save_dir = None;
            config.save();
        }
        if button(ui, p, "Choose folder...", false).clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            config.screenshot.save_dir = Some(dir);
            config.save();
        }
    });

    ui.add_space(metrics::S2);

    row(
        ui,
        *p,
        "File name",
        Some("{date} and {time} are replaced when the file is saved."),
        |ui| {
            let mut pattern = config.screenshot.name_pattern.clone();
            let response = ui.add(
                egui::TextEdit::singleline(&mut pattern)
                    .font(egui::FontId::proportional(text::CONTROL))
                    .desired_width(220.0),
            );
            if response.changed() {
                config.screenshot.name_pattern = pattern;
                config.save();
            }
        },
    );

    ui.add_space(metrics::S2);

    row(
        ui,
        *p,
        "Format",
        Some("JPEG is smaller; PNG is exact."),
        |ui| {
            let mut format = config.screenshot.format;
            if segmented(
                ui,
                *p,
                &mut format,
                &[ImageFormat::Png, ImageFormat::Jpeg],
                |f| match f {
                    ImageFormat::Png => "PNG",
                    ImageFormat::Jpeg => "JPEG",
                },
            ) {
                config.screenshot.format = format;
                config.save();
            }
        },
    );

    ui.add_space(metrics::S2);

    row(
        ui,
        *p,
        "Also save when copying",
        Some("Copying a screenshot writes the file to the save folder as well."),
        |ui| {
            let mut on = config.screenshot.also_save_on_copy;
            if toggle(ui, *p, &mut on) {
                config.screenshot.also_save_on_copy = on;
                config.save();
            }
        },
    );

    ui.add_space(metrics::S4);

    eyebrow(ui, *p, "EDITOR KEYS");

    let defaults = EditorKeys::default();
    let mut keys = config.screenshot.keys.clone();
    let mut changed = false;
    let mut key_row =
        |ui: &mut Ui, id: &'static str, label: &str, value: &mut Shortcut, default: &Shortcut| {
            row(ui, *p, label, None, |ui| {
                changed |= shortcut_recorder(ui, p, ("cfg_editor_key", id), value, Some(default));
            });
            ui.add_space(metrics::S1);
        };
    key_row(ui, "copy", "Copy", &mut keys.copy, &defaults.copy);
    key_row(ui, "save", "Save", &mut keys.save, &defaults.save);
    key_row(ui, "undo", "Undo", &mut keys.undo, &defaults.undo);
    key_row(ui, "redo", "Redo", &mut keys.redo, &defaults.redo);
    key_row(ui, "loupe", "Magnifier", &mut keys.loupe, &defaults.loupe);
    key_row(
        ui,
        "select_all",
        "Select all",
        &mut keys.select_all,
        &defaults.select_all,
    );
    if changed {
        config.screenshot.keys = keys;
        config.save();
    }

    row(ui, *p, "Cancel", Some("Always cancels."), |ui| {
        shortcut_badge(ui, p, "Esc");
    });
}
