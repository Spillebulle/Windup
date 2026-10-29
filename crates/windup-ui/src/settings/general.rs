//! General settings: theme mode, accent hue color picker, custom themes, background behavior.

use crate::theme::{Mode, Palette, metrics, oklch, rgb_to_oklch_hue, strong, text};
use crate::themelib;
use crate::widgets::{button, dropdown, eyebrow, row, segmented, slider, toggle};
use egui::{Color32, Key, Sense, Stroke, Ui, pos2, vec2};
use windup_core::{Config, ThemeMode};

pub fn render(ui: &mut Ui, config: &mut Config, p: &Palette, mode: Mode) {
    eyebrow(ui, *p, "APPEARANCE");

    // 1. Theme mode segmented picker (§7.9)
    row(
        ui,
        *p,
        "Theme mode",
        Some("Dark (Graphite), Light (Paper), or follow system preference."),
        |ui| {
            let mut theme_mode = config.theme_mode;
            if segmented(ui, *p, &mut theme_mode, &ThemeMode::ALL, |tm| match tm {
                ThemeMode::Dark => "Dark",
                ThemeMode::Light => "Light",
                ThemeMode::System => "System",
            }) {
                config.theme_mode = theme_mode;
                config.save();
            }
        },
    );

    ui.add_space(metrics::S2);

    // 2. Accent hue with interactive Color Picker popover (§2.3 & §7.10)
    row(
        ui,
        *p,
        "Accent color & hue",
        Some("The primary brand tone (0°–360°). Click swatch to open interactive palette."),
        |ui| {
            render_accent_picker(ui, config, p, mode);
        },
    );

    ui.add_space(metrics::S4);

    // 3. Custom Themes (.umbertheme format §3.2)
    eyebrow(ui, *p, "CUSTOM THEMES");
    render_custom_themes(ui, config, p, mode);

    ui.add_space(metrics::S4);

    eyebrow(ui, *p, "SYSTEM & STARTUP");

    // 4. Launch at login
    row(
        ui,
        *p,
        "Launch at login",
        Some("Start Windup in the background when you sign in."),
        |ui| {
            let mut launch = config.launch_at_login;
            if toggle(ui, *p, &mut launch) {
                config.launch_at_login = launch;
                config.save();
            }
        },
    );

    ui.add_space(metrics::S2);

    // 5. Run in background
    row(
        ui,
        *p,
        "Run in background",
        Some("Keep global shortcuts listening when the window is closed."),
        |ui| {
            let mut close_tray = config.close_to_tray;
            if toggle(ui, *p, &mut close_tray) {
                config.close_to_tray = close_tray;
                config.save();
            }
        },
    );
}

fn render_accent_picker(ui: &mut Ui, config: &mut Config, p: &Palette, mode: Mode) {
    let popup_id = ui.make_persistent_id("accent_picker_popover");
    let mut is_open = ui.data_mut(|d| d.get_temp::<bool>(popup_id).unwrap_or(false));

    let preview_color = match mode {
        Mode::Dark => oklch(0.674, 0.101, config.accent_hue),
        Mode::Light => oklch(0.55, 0.10, config.accent_hue),
    };
    let hex_str = format!(
        "#{:02X}{:02X}{:02X}",
        preview_color.r(),
        preview_color.g(),
        preview_color.b()
    );

    ui.horizontal(|ui| {
        // Swatch trigger button
        let (swatch_rect, response) = ui.allocate_exact_size(vec2(28.0, 24.0), Sense::click());
        ui.painter()
            .rect_filled(swatch_rect, metrics::RADIUS_TIGHT, preview_color);
        ui.painter().rect_stroke(
            swatch_rect,
            metrics::RADIUS_TIGHT,
            Stroke::new(
                metrics::HAIRLINE,
                if is_open { p.text_strong } else { p.line },
            ),
            egui::StrokeKind::Inside,
        );

        if response.clicked() {
            is_open = !is_open;
            ui.data_mut(|d| d.insert_temp(popup_id, is_open));
        }

        ui.add_space(metrics::S1);
        ui.label(
            egui::RichText::new(format!("{:.0}°  {}", config.accent_hue, hex_str))
                .font(strong(text::CONTROL))
                .color(p.text),
        );

        // Quick slider alongside
        ui.add_space(metrics::S2);
        let mut hue = config.accent_hue;
        let s_resp = slider(ui, *p, &mut hue, 0.0, 360.0, 1.0);
        if s_resp.changed() {
            config.accent_hue = hue;
            config.save();
        }

        if is_open {
            let area_pos = pos2(
                swatch_rect
                    .left()
                    .min(ui.ctx().screen_rect().right() - 280.0),
                swatch_rect.bottom() + 6.0,
            );

            egui::Area::new(popup_id.with("popover_area"))
                .order(egui::Order::Foreground)
                .fixed_pos(area_pos)
                .show(ui.ctx(), |ui| {
                    egui::Frame::NONE
                        .fill(p.popover)
                        .corner_radius(metrics::RADIUS_CARD)
                        .stroke(Stroke::new(metrics::HAIRLINE, p.line_popover))
                        .shadow(egui::Shadow {
                            offset: [0, 6],
                            blur: 20,
                            spread: 0,
                            color: Color32::from_black_alpha(70),
                        })
                        .inner_margin(metrics::PAD_PANEL)
                        .show(ui, |ui| {
                            ui.set_max_width(260.0);

                            ui.label(
                                egui::RichText::new("CURATED PALETTE")
                                    .font(strong(text::EYEBROW))
                                    .color(p.placeholder),
                            );
                            ui.add_space(metrics::S2);

                            // Curated swatches: [Terracotta, Bronze, Amber, Sage, Emerald, Cyan, Cobalt, Violet, Rose]
                            const PALETTES: [(&str, f32); 9] = [
                                ("Terracotta", 28.0),
                                ("Bronze", 48.0),
                                ("Amber", 75.0),
                                ("Sage", 130.0),
                                ("Emerald", 155.0),
                                ("Cyan", 205.0),
                                ("Cobalt", 250.0),
                                ("Violet", 295.0),
                                ("Rose", 345.0),
                            ];

                            ui.horizontal_wrapped(|ui| {
                                for (name, curated_h) in PALETTES {
                                    let col = match mode {
                                        Mode::Dark => oklch(0.674, 0.101, curated_h),
                                        Mode::Light => oklch(0.55, 0.10, curated_h),
                                    };
                                    let (c_rect, c_resp) =
                                        ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                                    let is_selected = (config.accent_hue - curated_h).abs() < 4.0;
                                    ui.painter().rect_filled(c_rect, metrics::RADIUS_TIGHT, col);
                                    if is_selected {
                                        ui.painter().rect_stroke(
                                            c_rect,
                                            metrics::RADIUS_TIGHT,
                                            Stroke::new(2.0_f32, p.text_strong),
                                            egui::StrokeKind::Inside,
                                        );
                                    } else if c_resp.hovered() {
                                        ui.painter().rect_stroke(
                                            c_rect,
                                            metrics::RADIUS_TIGHT,
                                            Stroke::new(1.5_f32, p.knob),
                                            egui::StrokeKind::Inside,
                                        );
                                    }

                                    if c_resp.on_hover_text(name).clicked() {
                                        config.accent_hue = curated_h;
                                        config.save();
                                    }
                                }
                            });

                            ui.add_space(metrics::S3);

                            ui.label(
                                egui::RichText::new("HEX COLOR ENTRY")
                                    .font(strong(text::EYEBROW))
                                    .color(p.placeholder),
                            );
                            ui.add_space(metrics::S1);

                            let hex_input_id = ui.make_persistent_id("accent_hex_input");
                            let mut hex_val = ui.data_mut(|d| {
                                d.get_temp::<String>(hex_input_id)
                                    .unwrap_or_else(|| hex_str.clone())
                            });

                            ui.horizontal(|ui| {
                                let te = egui::TextEdit::singleline(&mut hex_val)
                                    .desired_width(120.0)
                                    .font(egui::FontId::monospace(text::CONTROL));
                                if ui.add(te).changed() {
                                    let trimmed = hex_val.trim().trim_start_matches('#');
                                    if trimmed.len() == 6
                                        && let Ok(val) = u32::from_str_radix(trimmed, 16)
                                    {
                                        let r = ((val >> 16) & 0xFF) as u8;
                                        let g = ((val >> 8) & 0xFF) as u8;
                                        let b = (val & 0xFF) as u8;
                                        let computed_hue = rgb_to_oklch_hue(r, g, b);
                                        config.accent_hue = computed_hue;
                                        config.save();
                                    }
                                }
                                ui.data_mut(|d| d.insert_temp(hex_input_id, hex_val));

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

                            if ui.input(|i| i.key_pressed(Key::Escape)) {
                                is_open = false;
                            }
                        });
                });

            ui.data_mut(|d| d.insert_temp(popup_id, is_open));
        }
    });
}

fn render_custom_themes(ui: &mut Ui, config: &mut Config, p: &Palette, mode: Mode) {
    let themes_dir = Config::themes_dir().unwrap_or_default();
    let (custom_themes, problems) = themelib::load_all_from_dir(&themes_dir);

    row(
        ui,
        *p,
        "Active theme",
        Some("Select default palette or an imported .umbertheme file."),
        |ui| {
            let active_name = if let Some(ref custom_id) = config.custom_theme {
                custom_themes
                    .iter()
                    .find(|t| &t.id == custom_id)
                    .map(|t| t.name.as_str())
                    .unwrap_or("Custom (Missing)")
            } else {
                "Default (Built-in)"
            };

            let mut labels = vec!["Default (Built-in)".to_owned()];
            labels.extend(
                custom_themes
                    .iter()
                    .map(|t| format!("{} ({})", t.name, t.base)),
            );
            let current = match &config.custom_theme {
                None => Some(0),
                Some(id) => custom_themes
                    .iter()
                    .position(|t| &t.id == id)
                    .map(|i| i + 1),
            };
            if let Some(i) = dropdown(
                ui,
                p,
                mode,
                "theme_dropdown_popover",
                &labels,
                current,
                active_name,
            ) {
                config.custom_theme = i.checked_sub(1).map(|i| custom_themes[i].id.clone());
                config.save();
            }
        },
    );

    ui.add_space(metrics::S2);

    row(
        ui,
        *p,
        "Manage themes",
        Some("Import external .umbertheme files or inspect themes directory."),
        |ui| {
            ui.horizontal(|ui| {
                if button(ui, p, "+ Import .umbertheme...", true).clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("Umber theme", &[themelib::EXTENSION])
                        .pick_file()
                {
                    match themelib::import_theme_file(&path, &themes_dir) {
                        Ok(imported) => {
                            config.custom_theme = Some(imported.id);
                            config.save();
                        }
                        Err(e) => {
                            log::error!("Failed to import theme: {e}");
                        }
                    }
                }

                ui.add_space(metrics::S2);

                if button(ui, p, "Open themes folder", false).clicked() {
                    let _ = std::fs::create_dir_all(&themes_dir);
                    #[cfg(target_os = "windows")]
                    let _ = std::process::Command::new("explorer")
                        .arg(&themes_dir)
                        .spawn();
                }
            });
        },
    );

    if !problems.is_empty() {
        ui.add_space(metrics::S1);
        for prob in &problems {
            ui.label(
                egui::RichText::new(format!("Notice: {prob}"))
                    .font(egui::FontId::proportional(text::TINY))
                    .color(p.caution),
            );
        }
    }
}
