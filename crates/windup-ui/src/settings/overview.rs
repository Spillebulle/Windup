//! Overview of all tools in the multi-tool hub with quick toggles and shortcuts.

use crate::icons::Icon;
use crate::theme::{Palette, metrics, strong, text};
use crate::widgets::{eyebrow, shortcut_recorder, toggle};
use egui::{Align, Layout, RichText, Stroke, Ui, vec2};
use windup_core::Config;
use windup_core::tool::ToolId;

pub fn render(ui: &mut Ui, config: &mut Config, p: &Palette) {
    eyebrow(ui, *p, "INSTALLED TOOLS");
    ui.add_space(metrics::S2);

    for tool_id in ToolId::ALL {
        let is_enabled = config.is_tool_enabled(tool_id);

        egui::Frame::NONE
            .fill(p.dock)
            .corner_radius(metrics::RADIUS_CARD)
            .stroke(Stroke::new(metrics::HAIRLINE, p.line))
            .inner_margin(egui::Margin::symmetric(
                metrics::PAD_PANEL as i8,
                metrics::S3 as i8,
            ))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                ui.horizontal(|ui| {
                    // Tool Icon on left
                    let icon = match tool_id {
                        ToolId::ClipboardHistory => Icon::Clipboard,
                        ToolId::PastePlainText => Icon::FileText,
                    };
                    let (icon_rect, _) =
                        ui.allocate_exact_size(vec2(22.0, 22.0), egui::Sense::hover());
                    icon.paint(
                        ui.painter(),
                        icon_rect,
                        if is_enabled { p.accent } else { p.text_dim },
                    );

                    ui.add_space(metrics::S3);

                    // Name & Description in middle column (constrained so it never collides with controls)
                    let text_w = (ui.available_width() - 200.0).max(120.0);
                    ui.allocate_ui_with_layout(
                        vec2(text_w, 36.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.label(
                                RichText::new(tool_id.display_name())
                                    .font(strong(text::HEADING))
                                    .color(if is_enabled {
                                        p.text_strong
                                    } else {
                                        p.text_muted
                                    }),
                            );
                            ui.label(
                                RichText::new(tool_id.description())
                                    .font(egui::FontId::proportional(text::SMALL))
                                    .color(p.text_dim),
                            );
                        },
                    );

                    // Right controls docked to the right edge
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let mut enabled_mut = is_enabled;
                        if toggle(ui, *p, &mut enabled_mut) {
                            config.set_tool_enabled(tool_id, enabled_mut);
                        }

                        ui.add_space(metrics::S3);

                        let default_shortcut = match tool_id {
                            ToolId::ClipboardHistory => windup_core::shortcut::Shortcut::super_v(),
                            ToolId::PastePlainText => windup_core::shortcut::Shortcut::super_shift_v(),
                        };

                        let mut shortcut = config.tool_shortcut(tool_id).clone();
                        if shortcut_recorder(
                            ui,
                            p,
                            ("overview_shortcut", tool_id),
                            &mut shortcut,
                            Some(&default_shortcut),
                        ) {
                            config.set_tool_shortcut(tool_id, shortcut);
                        }
                    });
                });
            });

        ui.add_space(metrics::S2);
    }
}
