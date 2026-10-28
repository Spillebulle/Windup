use crate::theme::{Palette, metrics, strong, text};
use egui::{Align2, Color32, Key, Painter, Pos2, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};
use std::hash::Hash;
use windup_core::shortcut::Shortcut;

/// Static keycap badge displaying a keyboard shortcut string.
pub fn shortcut_badge(ui: &mut Ui, p: &Palette, text: &str) {
    let size = keycap_size(ui.painter(), p, text);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        paint_keycap(ui.painter(), p, rect.min, text);
    }
}

const KEYCAP_PAD_X: f32 = metrics::S2;
const KEYCAP_PAD_Y: f32 = 3.0;

/// The size [`paint_keycap`] will take for `text`.
pub fn keycap_size(painter: &Painter, p: &Palette, text: &str) -> Vec2 {
    let font_id = egui::FontId::monospace(text::TINY);
    let size = painter
        .layout_no_wrap(text.to_string(), font_id, p.text)
        .size();
    vec2(size.x + KEYCAP_PAD_X * 2.0, size.y + KEYCAP_PAD_Y * 2.0)
}

/// Paints a keycap with its top-left at `min`, for places that draw rather
/// than allocate (a tooltip on the capture overlay). Returns its rect.
pub fn paint_keycap(painter: &Painter, p: &Palette, min: Pos2, text: &str) -> Rect {
    let font_id = egui::FontId::monospace(text::TINY);
    let galley = painter.layout_no_wrap(text.to_string(), font_id, p.text);
    let size = galley.size();
    let rect = Rect::from_min_size(
        min,
        vec2(size.x + KEYCAP_PAD_X * 2.0, size.y + KEYCAP_PAD_Y * 2.0),
    );
    painter.rect_filled(rect, metrics::RADIUS_TIGHT, p.control);
    painter.rect_stroke(
        rect,
        metrics::RADIUS_TIGHT,
        Stroke::new(metrics::HAIRLINE, p.line),
        egui::StrokeKind::Inside,
    );
    painter.galley(
        pos2(rect.min.x + KEYCAP_PAD_X, rect.min.y + KEYCAP_PAD_Y),
        galley,
        p.text,
    );
    rect
}

/// Interactive shortcut recorder badge.
/// Clicking it opens a focused popover to modify modifiers and press keys.
pub fn shortcut_recorder(
    ui: &mut Ui,
    p: &Palette,
    id_source: impl Hash,
    shortcut: &mut Shortcut,
    default_shortcut: Option<&Shortcut>,
) -> bool {
    let mut changed = false;
    let popup_id = ui.make_persistent_id(id_source);
    let mut is_open = ui.data_mut(|d| d.get_temp::<bool>(popup_id).unwrap_or(false));

    let text_str = shortcut.to_string();
    let font_id = egui::FontId::monospace(text::TINY);
    let galley = ui.painter().layout_no_wrap(text_str, font_id, p.text);
    let text_size = galley.size();

    let pad_x = metrics::S2;
    let pad_y = 3.0;
    let badge_size = vec2(text_size.x + pad_x * 2.0 + 8.0, text_size.y + pad_y * 2.0);

    let (rect, response) = ui.allocate_exact_size(badge_size, Sense::click());
    if response.clicked() {
        is_open = !is_open;
        ui.data_mut(|d| d.insert_temp(popup_id, is_open));
    }

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = if is_open {
            p.control_active
        } else if response.hovered() {
            p.control_hover
        } else {
            p.control
        };

        let stroke_col = if is_open { p.accent } else { p.line };
        painter.rect_filled(rect, metrics::RADIUS_TIGHT, fill);
        painter.rect_stroke(
            rect,
            metrics::RADIUS_TIGHT,
            Stroke::new(metrics::HAIRLINE, stroke_col),
            egui::StrokeKind::Inside,
        );

        let text_pos = pos2(rect.min.x + pad_x, rect.min.y + pad_y);
        painter.galley(text_pos, galley, if is_open { p.accent } else { p.text });
    }

    response
        .on_hover_text("Click to edit shortcut")
        .on_hover_cursor(egui::CursorIcon::PointingHand);

    // Popover for editing shortcut
    if is_open {
        let area_pos = pos2(
            rect.left().min(ui.ctx().screen_rect().right() - 260.0),
            rect.bottom() + 4.0,
        );

        egui::Area::new(popup_id.with("area"))
            .order(egui::Order::Foreground)
            .fixed_pos(area_pos)
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(p.dock)
                    .corner_radius(metrics::RADIUS_CARD)
                    .stroke(Stroke::new(metrics::HAIRLINE, p.line))
                    .shadow(egui::Shadow {
                        offset: [0, 4],
                        blur: 16,
                        spread: 0,
                        color: Color32::from_black_alpha(60),
                    })
                    .inner_margin(metrics::PAD_PANEL)
                    .show(ui, |ui| {
                        ui.set_max_width(240.0);

                        ui.label(
                            egui::RichText::new("EDIT SHORTCUT")
                                .font(strong(text::EYEBROW))
                                .color(p.placeholder),
                        );
                        ui.add_space(metrics::S2);

                        // Modifier chips
                        ui.horizontal_wrapped(|ui| {
                            let mut sup = shortcut.modifiers.super_key;
                            #[cfg(target_os = "windows")]
                            let sup_label = "Win";
                            #[cfg(target_os = "macos")]
                            let sup_label = "Cmd";
                            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
                            let sup_label = "Super";

                            if mod_chip(ui, p, sup_label, &mut sup) {
                                shortcut.modifiers.super_key = sup;
                                changed = true;
                            }

                            let mut ctrl = shortcut.modifiers.ctrl;
                            if mod_chip(ui, p, "Ctrl", &mut ctrl) {
                                shortcut.modifiers.ctrl = ctrl;
                                changed = true;
                            }

                            let mut alt = shortcut.modifiers.alt;
                            if mod_chip(ui, p, "Alt", &mut alt) {
                                shortcut.modifiers.alt = alt;
                                changed = true;
                            }

                            let mut shift = shortcut.modifiers.shift;
                            if mod_chip(ui, p, "Shift", &mut shift) {
                                shortcut.modifiers.shift = shift;
                                changed = true;
                            }
                        });

                        ui.add_space(metrics::S3);

                        // Key prompt
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Key:")
                                    .font(egui::FontId::proportional(text::CONTROL))
                                    .color(p.text_dim),
                            );
                            ui.label(
                                egui::RichText::new(&shortcut.key)
                                    .font(strong(text::CONTROL))
                                    .color(p.accent),
                            );
                            ui.label(
                                egui::RichText::new("(press any key)")
                                    .font(egui::FontId::proportional(text::TINY))
                                    .color(p.text_muted),
                            );
                        });

                        ui.add_space(metrics::S3);

                        // Listen for keyboard input
                        ui.input(|i| {
                            if i.key_pressed(Key::Escape) {
                                is_open = false;
                            }

                            for event in &i.events {
                                if let egui::Event::Key {
                                    key,
                                    pressed: true,
                                    modifiers,
                                    ..
                                } = event
                                {
                                    if *key == Key::Escape {
                                        is_open = false;
                                        break;
                                    }
                                    if *key == Key::Enter {
                                        is_open = false;
                                        break;
                                    }

                                    // Egui's key names match the canonical ones for
                                    // everything `windows_vk` knows. Egui has no Key
                                    // for Print Screen, so that is only ever a default.
                                    let key_str = key.name().to_uppercase();
                                    if windup_core::hotkeys::windows_vk(&key_str).is_none() {
                                        continue;
                                    }

                                    shortcut.key = key_str;
                                    if modifiers.ctrl {
                                        shortcut.modifiers.ctrl = true;
                                    }
                                    if modifiers.alt {
                                        shortcut.modifiers.alt = true;
                                    }
                                    if modifiers.shift {
                                        shortcut.modifiers.shift = true;
                                    }
                                    changed = true;
                                    break;
                                }
                            }
                        });

                        // Revert and Done buttons
                        ui.horizontal(|ui| {
                            if let Some(def) = default_shortcut {
                                let is_default = shortcut == def;
                                if ui
                                    .add_enabled(
                                        !is_default,
                                        egui::Button::new(
                                            egui::RichText::new("Revert")
                                                .font(egui::FontId::proportional(text::CONTROL))
                                                .color(if !is_default {
                                                    p.text
                                                } else {
                                                    p.placeholder
                                                }),
                                        ),
                                    )
                                    .on_hover_text(format!("Reset to default: {}", def))
                                    .clicked()
                                {
                                    *shortcut = def.clone();
                                    changed = true;
                                }
                            }

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .button(
                                            egui::RichText::new("Done")
                                                .font(strong(text::CONTROL))
                                                .color(p.text_strong),
                                        )
                                        .clicked()
                                    {
                                        is_open = false;
                                    }
                                },
                            );
                        });
                    });
            });

        ui.data_mut(|d| d.insert_temp(popup_id, is_open));
    }

    changed
}

fn mod_chip(ui: &mut Ui, p: &Palette, label: &str, on: &mut bool) -> bool {
    let chip_w = 46.0;
    let (rect, response) =
        ui.allocate_exact_size(vec2(chip_w, metrics::BUTTON * 0.8), Sense::click());
    let clicked = response.clicked();
    if clicked {
        *on = !*on;
    }

    let painter = ui.painter();
    let fill = if *on {
        p.accent
    } else if response.hovered() {
        p.control_hover
    } else {
        p.control
    };
    painter.rect_filled(rect, metrics::RADIUS_TIGHT, fill);
    if !*on {
        painter.rect_stroke(
            rect,
            metrics::RADIUS_TIGHT,
            Stroke::new(metrics::HAIRLINE, p.line),
            egui::StrokeKind::Inside,
        );
    }

    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        if *on {
            strong(text::TINY)
        } else {
            egui::FontId::proportional(text::TINY)
        },
        if *on { p.accent_ink } else { p.text },
    );

    clicked
}
