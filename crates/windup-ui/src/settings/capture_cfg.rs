//! Capture launcher settings: the capture bar's shortcut and its loupe.

use super::HotkeyHint;
use crate::theme::{Palette, metrics, text};
use crate::widgets::{caution_keycap, eyebrow, row, shortcut_recorder, toggle};
use egui::{RichText, Ui};
use windup_core::Config;
use windup_core::hotkeys;
use windup_core::shortcut::Shortcut;
use windup_core::tool::ToolId;

pub fn render(ui: &mut Ui, config: &mut Config, p: &Palette, hint: HotkeyHint) {
    eyebrow(ui, *p, "TOOL STATUS");

    row(
        ui,
        *p,
        "Enable Capture launcher",
        Some("Open the capture bar to take a screenshot of an area, a window or the screen."),
        |ui| {
            let mut enabled = config.capture.enabled;
            if toggle(ui, *p, &mut enabled) {
                config.capture.enabled = enabled;
                config.save();
            }
        },
    );

    ui.add_space(metrics::S4);

    eyebrow(ui, *p, "BEHAVIOR & SHORTCUT");

    let mut shortcut = config.capture.shortcut.clone();
    if shortcut_row(
        ui,
        config,
        p,
        ToolId::CaptureLauncher,
        "Capture shortcut",
        "Global shortcut that opens the capture bar.",
        &mut shortcut,
    ) {
        config.capture.shortcut = shortcut;
        config.save();
    }
    hotkey_note(ui, p, hint);

    ui.add_space(metrics::S2);

    row(
        ui,
        *p,
        "Magnifier starts on",
        Some("Show the magnifier while choosing an area."),
        |ui| {
            let mut on = config.screenshot.loupe_on;
            if toggle(ui, *p, &mut on) {
                config.screenshot.loupe_on = on;
                config.save();
            }
        },
    );
}

/// A tool's shortcut row: the recorder, and a caution keycap on its left when
/// another enabled tool already has the same keys (STYLE-GUIDE §7.13). The
/// first tool in `ToolId::ALL` keeps the keys; this one does not respond.
/// Returns whether the shortcut was edited.
pub(super) fn shortcut_row(
    ui: &mut Ui,
    config: &Config,
    p: &Palette,
    tool: ToolId,
    label: &str,
    under: &str,
    shortcut: &mut Shortcut,
) -> bool {
    let clash = if hotkeys::clashes(config).contains(&tool) {
        ToolId::ALL
            .into_iter()
            .find(|other| {
                *other != tool
                    && config.is_tool_enabled(*other)
                    && config.tool_shortcut(*other) == config.tool_shortcut(tool)
            })
            .map(|other| other.display_name())
    } else {
        None
    };
    let default = tool.default_shortcut();
    let mut changed = false;
    row(ui, *p, label, Some(under), |ui| {
        changed = shortcut_recorder(ui, p, ("cfg_shortcut", tool), shortcut, default.as_ref());
        if let Some(name) = clash {
            caution_keycap(ui, p, &format!("Also used by {name}"))
                .on_hover_text("Only the first tool responds until one of them changes.");
        }
    });
    changed
}

/// The line under the shortcut rows about binding the keys in the desktop.
pub(super) fn hotkey_note(ui: &mut Ui, p: &Palette, hint: HotkeyHint) {
    let note = match hint {
        HotkeyHint::None => return,
        HotkeyHint::Command => {
            "Bind windup --capture, windup --screenshot and windup --record in your desktop's keyboard settings."
        }
        HotkeyHint::Cosmic => {
            "On COSMIC, change it in COSMIC Settings, Keyboard, Custom shortcuts too."
        }
    };
    ui.label(RichText::new(note).size(text::SMALL).color(p.text_dim));
}
