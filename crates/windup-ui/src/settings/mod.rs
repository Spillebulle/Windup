//! The settings window adhering to §9 of STYLE-GUIDE.md.
//! Left navigation rail (240px), pane switching, live-saving, "Restore all" footer.

pub mod clipboard_cfg;
pub mod general;
pub mod overview;
pub mod paste_plain_cfg;

use crate::icons::Icon;
use crate::theme::{Mode, Palette, metrics, strong, text};
use egui::{Rect, Stroke, Ui, pos2, vec2};
use windup_core::Config;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SettingsPane {
    #[default]
    Overview,
    General,
    ClipboardHistory,
    PastePlainText,
}

impl SettingsPane {
    pub const ALL: [SettingsPane; 4] = [
        SettingsPane::Overview,
        SettingsPane::General,
        SettingsPane::ClipboardHistory,
        SettingsPane::PastePlainText,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingsPane::Overview => "Tools overview",
            SettingsPane::General => "General & appearance",
            SettingsPane::ClipboardHistory => "Clipboard history",
            SettingsPane::PastePlainText => "Paste plain text",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            SettingsPane::Overview => Icon::Layers,
            SettingsPane::General => Icon::Sliders,
            SettingsPane::ClipboardHistory => Icon::Clipboard,
            SettingsPane::PastePlainText => Icon::FileText,
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            SettingsPane::Overview => "All available tools, their bindings, and quick toggles.",
            SettingsPane::General => "Theme, accent hue, and background behavior.",
            SettingsPane::ClipboardHistory => {
                "History storage, instant paste, and popup shortcuts."
            }
            SettingsPane::PastePlainText => "Instant plain text paste shortcut and strip settings.",
        }
    }
}

pub struct SettingsView {
    pub active_pane: SettingsPane,
}

impl Default for SettingsView {
    fn default() -> Self {
        Self {
            active_pane: SettingsPane::Overview,
        }
    }
}

impl SettingsView {
    pub fn ui(&mut self, ui: &mut Ui, config: &mut Config, p: &Palette, mode: Mode) {
        let total_rect = ui.available_rect_before_wrap();

        // 1. Sidebar Rail on the left (240px)
        let rail_rect =
            Rect::from_min_size(total_rect.min, vec2(metrics::SIDEBAR, total_rect.height()));

        // 2. Content Pane on the right
        let content_rect = Rect::from_min_size(
            pos2(total_rect.min.x + metrics::SIDEBAR, total_rect.min.y),
            vec2(total_rect.width() - metrics::SIDEBAR, total_rect.height()),
        );

        // Draw rail background & right hairline separator
        let painter = ui.painter();
        painter.rect_filled(rail_rect, 0.0, p.dock);
        painter.line_segment(
            [rail_rect.right_top(), rail_rect.right_bottom()],
            Stroke::new(metrics::HAIRLINE, p.line),
        );

        // --- Render Sidebar UI ---
        let mut rail_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rail_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        self.render_rail(&mut rail_ui, config, p);

        // --- Render Active Content Pane UI ---
        let mut content_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(content_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        self.render_content(&mut content_ui, config, p, mode);
    }

    fn render_rail(&mut self, ui: &mut Ui, config: &mut Config, p: &Palette) {
        ui.add_space(metrics::S3);

        // App Wordmark / Mark in header
        ui.horizontal(|ui| {
            ui.add_space(metrics::PAD_STRIP);
            let mark_rect =
                Rect::from_min_size(ui.cursor().min, vec2(metrics::MARK, metrics::MARK));
            ui.allocate_rect(mark_rect, egui::Sense::hover());
            ui.painter()
                .rect_filled(mark_rect, metrics::RADIUS_TIGHT, p.accent);
            Icon::Wrench.paint(ui.painter(), mark_rect.shrink(2.5), p.accent_ink);
            ui.add_space(metrics::S2);
            ui.label(
                egui::RichText::new("Windup")
                    .font(strong(text::HEADING))
                    .color(p.text_strong),
            );
        });

        ui.add_space(metrics::S4);

        // Rail items
        for pane in SettingsPane::ALL {
            let is_selected = self.active_pane == pane;
            let (rect, response) = ui.allocate_exact_size(
                vec2(metrics::SIDEBAR, metrics::NAV_ROW),
                egui::Sense::click(),
            );

            if response.clicked() {
                self.active_pane = pane;
            }

            if ui.is_rect_visible(rect) {
                let painter = ui.painter();

                // Selected fill and accent mark per §1 & §2.4 of STYLE-GUIDE.md:
                // Raised neutral fill + strong text + small 3px accent bar on left edge.
                if is_selected {
                    painter.rect_filled(rect, 0.0, p.control);
                    let bar_rect = Rect::from_min_size(
                        rect.left_top(),
                        vec2(metrics::NAV_MARK_W, rect.height()),
                    );
                    painter.rect_filled(bar_rect, 0.0, p.accent);
                } else if response.hovered() {
                    painter.rect_filled(rect, 0.0, p.control_hover);
                }

                // Icon
                let icon_rect = Rect::from_center_size(
                    pos2(rect.min.x + metrics::PAD_STRIP + 8.0, rect.center().y),
                    vec2(metrics::ICON, metrics::ICON),
                );
                let icon_color = if is_selected { p.accent } else { p.text_muted };
                pane.icon().paint(painter, icon_rect, icon_color);

                // Label
                let text_color = if is_selected { p.text_strong } else { p.text };
                let label_pos = pos2(
                    rect.min.x + metrics::PAD_STRIP + 24.0,
                    rect.center().y - 6.0,
                );
                painter.text(
                    label_pos,
                    egui::Align2::LEFT_TOP,
                    pane.label(),
                    if is_selected {
                        strong(text::CONTROL)
                    } else {
                        egui::FontId::proportional(text::CONTROL)
                    },
                    text_color,
                );
            }
        }

        // Footer in rail: Restore all command per §9
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(metrics::S3);
            ui.horizontal(|ui| {
                ui.add_space(metrics::PAD_STRIP);
                if ui
                    .button(
                        egui::RichText::new("Restore all")
                            .font(egui::FontId::proportional(text::SMALL))
                            .color(p.text_strong),
                    )
                    .clicked()
                {
                    config.restore_defaults();
                }
            });
            ui.add_space(metrics::S2);
            let sep_rect =
                Rect::from_min_size(ui.cursor().min, vec2(metrics::SIDEBAR, metrics::HAIRLINE));
            ui.painter().rect_filled(sep_rect, 0.0, p.line_soft);
        });
    }

    fn render_content(&mut self, ui: &mut Ui, config: &mut Config, p: &Palette, mode: Mode) {
        // Page header (§9: 15px title + blurb)
        ui.horizontal(|ui| {
            ui.add_space(metrics::PAD_PANEL * 2.0);
            ui.vertical(|ui| {
                ui.add_space(metrics::S4);
                ui.label(
                    egui::RichText::new(self.active_pane.label())
                        .font(strong(text::PAGE))
                        .color(p.text_strong),
                );
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(self.active_pane.blurb())
                        .font(egui::FontId::proportional(text::BODY))
                        .color(p.text_muted),
                );
                ui.add_space(metrics::S3);
            });
        });

        // Separator below header
        let sep_rect = Rect::from_min_size(
            pos2(ui.cursor().min.x, ui.cursor().min.y),
            vec2(ui.available_width(), metrics::HAIRLINE),
        );
        ui.painter().rect_filled(sep_rect, 0.0, p.line_soft);
        ui.add_space(metrics::S4);

        // Content body container taking full available width
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(
                        (metrics::PAD_PANEL * 2.0) as i8,
                        metrics::PAD_PANEL as i8,
                    ))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        match self.active_pane {
                            SettingsPane::Overview => {
                                overview::render(ui, config, p);
                            }
                            SettingsPane::General => {
                                general::render(ui, config, p, mode);
                            }
                            SettingsPane::ClipboardHistory => {
                                clipboard_cfg::render(ui, config, p);
                            }
                            SettingsPane::PastePlainText => {
                                paste_plain_cfg::render(ui, config, p);
                            }
                        }
                    });
            });
    }
}
