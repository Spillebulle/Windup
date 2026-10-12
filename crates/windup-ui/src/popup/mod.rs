//! Floating popup over cursor for Clipboard History. §7.11 & §7.13 of STYLE-GUIDE.md.

use crate::icons::Icon;
use crate::theme::{Palette, metrics, strong, text};
use egui::{Color32, Key, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use windup_core::clipboard::{ClipboardContent, ClipboardEntry, ClipboardHistory};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusedButton {
    Item,
    Pen,
    PastePlain,
    Delete,
}

#[derive(Clone, Debug)]
pub enum PopupAction {
    Paste(ClipboardEntry),
    PastePlain(String),
    Delete(u64),
    Resize(f32, f32),
    ResizeLeft(f32, f32, f32),
    Dismiss,
}

#[derive(Default)]
pub struct PopupView {
    pub selected_index: usize,
    pub focused_button: Option<FocusedButton>,
    pub search_query: String,
    pub request_focus: bool,
    pub scroll_to_selected: bool,
    textures: HashMap<u64, egui::TextureHandle>,
}

impl PopupView {
    pub fn new() -> Self {
        Self {
            selected_index: 0,
            focused_button: Some(FocusedButton::Item),
            search_query: String::new(),
            request_focus: true,
            scroll_to_selected: false,
            textures: HashMap::new(),
        }
    }

    pub fn reset(&mut self) {
        self.selected_index = 0;
        self.focused_button = Some(FocusedButton::Item);
        self.search_query.clear();
        self.request_focus = true;
        self.scroll_to_selected = true;
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        history: &ClipboardHistory,
        p: &Palette,
    ) -> Option<PopupAction> {
        let mut action = None;

        // Filter entries based on search_query
        let query = self.search_query.trim().to_lowercase();
        let filtered: Vec<&ClipboardEntry> = history
            .entries()
            .iter()
            .filter(|e| {
                if query.is_empty() {
                    return true;
                }
                match &e.content {
                    ClipboardContent::Text { text, .. } => text.to_lowercase().contains(&query),
                    ClipboardContent::Image { .. } => {
                        "image".contains(&query) || "picture".contains(&query)
                    }
                }
            })
            .collect();

        let count = filtered.len();
        if count == 0 {
            self.selected_index = 0;
        } else if self.selected_index >= count {
            self.selected_index = count - 1;
        }

        // -------------------------------------------------------------
        // Keyboard Navigation
        // -------------------------------------------------------------
        ui.input(|i| {
            if i.key_pressed(Key::Escape) {
                action = Some(PopupAction::Dismiss);
            }

            if i.key_pressed(Key::ArrowDown) && count > 0 {
                self.selected_index = (self.selected_index + 1).min(count - 1);
                self.focused_button = Some(FocusedButton::Item);
                self.scroll_to_selected = true;
            }

            if i.key_pressed(Key::ArrowUp) && count > 0 {
                self.selected_index = self.selected_index.saturating_sub(1);
                self.focused_button = Some(FocusedButton::Item);
                self.scroll_to_selected = true;
            }

            // Direct Delete key, or Backspace when search query is empty
            let is_search_empty = self.search_query.is_empty();
            let should_delete = i.key_pressed(Key::Delete) || (is_search_empty && i.key_pressed(Key::Backspace));
            if should_delete && let Some(entry) = filtered.get(self.selected_index) {
                action = Some(PopupAction::Delete(entry.id));
            }

            if i.key_pressed(Key::Tab)
                && let Some(entry) = filtered.get(self.selected_index)
            {
                let has_pen = entry.is_image();
                let has_plain = entry.plain_text().is_some();
                self.focused_button = match self.focused_button {
                    Some(FocusedButton::Item) => {
                        if has_pen {
                            Some(FocusedButton::Pen)
                        } else if has_plain {
                            Some(FocusedButton::PastePlain)
                        } else {
                            Some(FocusedButton::Delete)
                        }
                    }
                    Some(FocusedButton::Pen) => {
                        if has_plain {
                            Some(FocusedButton::PastePlain)
                        } else {
                            Some(FocusedButton::Delete)
                        }
                    }
                    Some(FocusedButton::PastePlain) => Some(FocusedButton::Delete),
                    Some(FocusedButton::Delete) | None => Some(FocusedButton::Item),
                };
            }

            if i.key_pressed(Key::Enter)
                && let Some(entry) = filtered.get(self.selected_index)
            {
                match self.focused_button {
                    Some(FocusedButton::Delete) => {
                        action = Some(PopupAction::Delete(entry.id));
                    }
                    Some(FocusedButton::PastePlain) => {
                        if let Some(text) = entry.plain_text() {
                            action = Some(PopupAction::PastePlain(text.to_string()));
                        }
                    }
                    Some(FocusedButton::Pen) => {
                        // Pen edit placeholder: no action for now
                    }
                    _ => {
                        action = Some(PopupAction::Paste((*entry).clone()));
                    }
                }
            }
        });

        if action.is_some() {
            return action;
        }

        // -------------------------------------------------------------
        // UI Layout: Card Container with Hairline & Header
        // -------------------------------------------------------------
        let total_rect = ui.available_rect_before_wrap();
        ui.painter()
            .rect_filled(total_rect, metrics::RADIUS_CARD, p.dock);
        ui.painter().rect_stroke(
            total_rect,
            metrics::RADIUS_CARD,
            Stroke::new(metrics::HAIRLINE, p.line_popover),
            StrokeKind::Inside,
        );

        ui.vertical(|ui| {
            ui.add_space(metrics::S2);

            // Header: Search & Count
            ui.horizontal(|ui| {
                ui.add_space(metrics::S3);

                let (search_icon_rect, _) =
                    ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::hover());
                Icon::Search.paint(ui.painter(), search_icon_rect, p.placeholder);
                ui.add_space(metrics::S1);

                // Search field
                let edit = egui::TextEdit::singleline(&mut self.search_query)
                    .hint_text("Search clipboard...")
                    .desired_width(ui.available_width() - 70.0)
                    .margin(egui::Margin::symmetric(
                        metrics::S2 as i8,
                        metrics::S1 as i8,
                    ));
                let resp = ui.add(edit);
                if self.request_focus {
                    resp.request_focus();
                    self.request_focus = false;
                }

                // If typing in search, let ArrowDown or Tab surrender text field focus to navigate items
                if resp.has_focus()
                    && ui.input(|i| i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::Tab))
                {
                    ui.memory_mut(|m| m.surrender_focus(resp.id));
                }

                // Count chip (monospaced figure)
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(metrics::S3);
                    ui.label(
                        egui::RichText::new(format!("{count}"))
                            .font(egui::FontId::monospace(text::TINY))
                            .color(p.text_dim),
                    );
                });
            });

            ui.add_space(metrics::S2);

            // Hairline divider
            let sep_rect = Rect::from_min_size(
                pos2(ui.cursor().min.x, ui.cursor().min.y),
                vec2(ui.available_width(), metrics::HAIRLINE),
            );
            ui.painter().rect_filled(sep_rect, 0.0, p.line_soft);

            // ---------------------------------------------------------
            // Entries List (Virtual Scrolling with show_rows)
            // ---------------------------------------------------------
            if count == 0 {
                ui.add_space(metrics::S5);
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new("No items in clipboard history")
                            .font(egui::FontId::proportional(text::BODY))
                            .color(p.text_muted),
                    );
                });
                return;
            }

            const ROW_HEIGHT: f32 = 48.0;
            let scroll_id = ui.id().with("clipboard_scroll_y");
            let mut scroll_area = egui::ScrollArea::vertical().auto_shrink([false, false]);

            if self.scroll_to_selected {
                let target_top = self.selected_index as f32 * ROW_HEIGHT;
                let target_bottom = target_top + ROW_HEIGHT;
                let current_scroll: f32 =
                    ui.ctx().data(|d| d.get_temp(scroll_id)).unwrap_or(0.0);
                let view_h = (ui.available_height() - 20.0).max(100.0);
                if target_top < current_scroll {
                    scroll_area = scroll_area.vertical_scroll_offset(target_top);
                } else if target_bottom > current_scroll + view_h {
                    let new_offset = target_bottom - view_h;
                    scroll_area = scroll_area.vertical_scroll_offset(new_offset);
                }
                self.scroll_to_selected = false;
            }

            let output = scroll_area.show_rows(ui, ROW_HEIGHT, count, |ui, row_range| {
                for idx in row_range {
                    let entry = filtered[idx];
                    let is_selected = idx == self.selected_index;
                    let item_action = self.render_entry(ui, idx, entry, is_selected, p);
                    if let Some(act) = item_action {
                        action = Some(act);
                    }
                }
            });

            ui.ctx().data_mut(|d| d.insert_temp(scroll_id, output.state.offset.y));
        });

        // -------------------------------------------------------------
        // Bottom-right Resize Grip (§7.15)
        // -------------------------------------------------------------
        let grip_size = vec2(16.0, 16.0);
        let grip_rect = Rect::from_min_size(
            pos2(total_rect.max.x - grip_size.x, total_rect.max.y - grip_size.y),
            grip_size,
        );
        let grip_resp = ui
            .allocate_rect(grip_rect, Sense::drag())
            .on_hover_cursor(egui::CursorIcon::ResizeSouthEast);

        if grip_resp.dragged() {
            let delta = grip_resp.drag_delta();
            let current_size = ui.ctx().screen_rect().size();
            let new_w = (current_size.x + delta.x).clamp(280.0, 900.0);
            let new_h = (current_size.y + delta.y).clamp(260.0, 1000.0);
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(new_w, new_h)));
            action = Some(PopupAction::Resize(new_w, new_h));
        }

        if ui.is_rect_visible(grip_rect) {
            let p_col = if grip_resp.hovered() || grip_resp.dragged() {
                p.accent
            } else {
                p.placeholder
            };
            let p_line = |offset_x: f32, offset_y: f32| {
                pos2(grip_rect.max.x - offset_x, grip_rect.max.y - offset_y)
            };
            ui.painter().line_segment(
                [p_line(4.0, 12.0), p_line(12.0, 4.0)],
                Stroke::new(1.0_f32, p_col),
            );
            ui.painter().line_segment(
                [p_line(4.0, 8.0), p_line(8.0, 4.0)],
                Stroke::new(1.0_f32, p_col),
            );
            ui.painter().line_segment(
                [p_line(4.0, 4.0), p_line(4.0, 4.0)],
                Stroke::new(1.5_f32, p_col),
            );
        }

        // -------------------------------------------------------------
        // Bottom-left Resize Grip
        // -------------------------------------------------------------
        let grip_bl_rect = Rect::from_min_size(
            pos2(total_rect.min.x, total_rect.max.y - grip_size.y),
            grip_size,
        );
        let grip_bl_resp = ui
            .allocate_rect(grip_bl_rect, Sense::drag())
            .on_hover_cursor(egui::CursorIcon::ResizeSouthWest);

        if grip_bl_resp.dragged() {
            let delta = grip_bl_resp.drag_delta();
            let current_size = ui.ctx().screen_rect().size();
            let new_w = (current_size.x - delta.x).clamp(280.0, 900.0);
            let new_h = (current_size.y + delta.y).clamp(260.0, 1000.0);
            let actual_dx = current_size.x - new_w;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(new_w, new_h)));
            action = Some(PopupAction::ResizeLeft(new_w, new_h, actual_dx));
        }

        if ui.is_rect_visible(grip_bl_rect) {
            let p_col = if grip_bl_resp.hovered() || grip_bl_resp.dragged() {
                p.accent
            } else {
                p.placeholder
            };
            let p_line = |offset_x: f32, offset_y: f32| {
                pos2(grip_bl_rect.min.x + offset_x, grip_bl_rect.max.y - offset_y)
            };
            ui.painter().line_segment(
                [p_line(4.0, 12.0), p_line(12.0, 4.0)],
                Stroke::new(1.0_f32, p_col),
            );
            ui.painter().line_segment(
                [p_line(4.0, 8.0), p_line(8.0, 4.0)],
                Stroke::new(1.0_f32, p_col),
            );
            ui.painter().line_segment(
                [p_line(4.0, 4.0), p_line(4.0, 4.0)],
                Stroke::new(1.5_f32, p_col),
            );
        }

        action
    }

    fn render_entry(
        &mut self,
        ui: &mut Ui,
        idx: usize,
        entry: &ClipboardEntry,
        is_selected: bool,
        p: &Palette,
    ) -> Option<PopupAction> {
        let mut action = None;
        let row_h = 48.0;
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());

        if response.clicked() {
            self.selected_index = idx;
            self.focused_button = Some(FocusedButton::Item);
            action = Some(PopupAction::Paste((*entry).clone()));
        }

        if ui.is_rect_visible(rect) {
            let painter = ui.painter();

            // Row background per §1 & §2.4:
            // Selected = neutral fill (`p.control`) + 3px accent indicator bar on left.
            if is_selected {
                painter.rect_filled(rect, 0.0, p.control);
                let bar =
                    Rect::from_min_size(rect.left_top(), vec2(metrics::NAV_MARK_W, rect.height()));
                painter.rect_filled(bar, 0.0, p.accent);
            } else if response.hovered() {
                painter.rect_filled(rect, 0.0, p.control_hover);
            }

            // Bottom hairline separator
            painter.line_segment(
                [rect.left_bottom(), rect.right_bottom()],
                Stroke::new(metrics::HAIRLINE, p.line_soft),
            );

            // Calculate max width for text content to prevent overlapping action buttons
            let text_max_w = if entry.is_image() {
                (rect.width() - 150.0).max(60.0)
            } else {
                (rect.width() - 100.0).max(60.0)
            };

            // Content preview
            match &entry.content {
                ClipboardContent::Text { text: t, .. } => {
                    let text_color = if is_selected { p.text_strong } else { p.text };
                    let font = if is_selected {
                        strong(text::CONTROL)
                    } else {
                        egui::FontId::proportional(text::CONTROL)
                    };

                    let preview = entry.preview_label();
                    let galley = fit_text(ui, &preview, font, text_color, text_max_w);
                    painter.galley(
                        pos2(rect.min.x + metrics::PAD_STRIP, rect.min.y + metrics::S2),
                        galley,
                        text_color,
                    );

                    // Sub-info: char count & relative timestamp
                    let info_text = format!(
                        "{} chars  ·  {}",
                        t.chars().count(),
                        entry.relative_time_string()
                    );
                    let info_galley = fit_text(
                        ui,
                        &info_text,
                        egui::FontId::monospace(text::TINY),
                        p.text_dim,
                        text_max_w,
                    );
                    painter.galley(
                        pos2(rect.min.x + metrics::PAD_STRIP, rect.min.y + 28.0),
                        info_galley,
                        p.text_dim,
                    );
                }
                ClipboardContent::Image {
                    width,
                    height,
                    thumb_rgba,
                    thumb_w,
                    thumb_h,
                    ..
                } => {
                    // Texture handle for thumbnail
                    let texture_id = entry.id;
                    let texture = self.textures.entry(texture_id).or_insert_with(|| {
                        let color_img = egui::ColorImage::from_rgba_unmultiplied(
                            [*thumb_w as usize, *thumb_h as usize],
                            thumb_rgba,
                        );
                        ui.ctx().load_texture(
                            format!("clip-thumb-{}", texture_id),
                            color_img,
                            egui::TextureOptions::LINEAR,
                        )
                    });

                    // Draw thumbnail preview (36x36, centered vertically in 48px row)
                    let thumb_rect = Rect::from_min_size(
                        pos2(rect.min.x + metrics::PAD_STRIP, rect.min.y + 6.0),
                        vec2(36.0, 36.0),
                    );
                    painter.image(
                        texture.id(),
                        thumb_rect,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    painter.rect_stroke(
                        thumb_rect,
                        metrics::RADIUS_TIGHT,
                        Stroke::new(metrics::HAIRLINE, p.line),
                        StrokeKind::Inside,
                    );

                    // Label & info
                    let label = format!("Image ({} × {} px)", width, height);
                    let font = if is_selected {
                        strong(text::CONTROL)
                    } else {
                        egui::FontId::proportional(text::CONTROL)
                    };
                    let text_color = if is_selected { p.text_strong } else { p.text };
                    let galley = fit_text(ui, &label, font, text_color, text_max_w);
                    painter.galley(
                        pos2(rect.min.x + metrics::PAD_STRIP + 44.0, rect.min.y + 6.0),
                        galley,
                        text_color,
                    );

                    let info_galley = fit_text(
                        ui,
                        &entry.relative_time_string(),
                        egui::FontId::monospace(text::TINY),
                        p.text_dim,
                        text_max_w,
                    );
                    painter.galley(
                        pos2(rect.min.x + metrics::PAD_STRIP + 44.0, rect.min.y + 26.0),
                        info_galley,
                        p.text_dim,
                    );
                }
            }

            // Action buttons on the right side of the row: Trash, PastePlain, Pen
            let mut btns_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_size(
                        pos2(rect.max.x - 96.0, rect.min.y),
                        vec2(92.0, row_h),
                    ))
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );

            // 1. Delete button (Icon::Trash)
            let is_del_focused = is_selected && self.focused_button == Some(FocusedButton::Delete);
            let del_btn_rect = Rect::from_center_size(
                pos2(btns_ui.cursor().max.x - 12.0, rect.center().y),
                vec2(22.0, 22.0),
            );
            let del_resp = btns_ui
                .allocate_rect(del_btn_rect, Sense::click())
                .on_hover_text("Delete entry (Delete / Backspace)");

            if del_resp.clicked() {
                action = Some(PopupAction::Delete(entry.id));
            }

            if btns_ui.is_rect_visible(del_btn_rect) {
                let painter = btns_ui.painter();
                if is_del_focused || del_resp.hovered() {
                    painter.rect_filled(del_btn_rect, metrics::RADIUS_TIGHT, p.caution_bg);
                    painter.rect_stroke(
                        del_btn_rect,
                        metrics::RADIUS_TIGHT,
                        Stroke::new(metrics::HAIRLINE, p.caution_line),
                        StrokeKind::Inside,
                    );
                }
                let icon_color = if is_del_focused || del_resp.hovered() {
                    p.caution
                } else {
                    p.text_muted
                };
                Icon::Trash.paint(
                    painter,
                    Rect::from_center_size(del_btn_rect.center(), vec2(13.0, 13.0)),
                    icon_color,
                );
            }

            // 2. Paste Plain Text button (available on text items)
            if let Some(plain_text) = entry.plain_text() {
                let is_plain_focused =
                    is_selected && self.focused_button == Some(FocusedButton::PastePlain);
                let plain_btn_rect = Rect::from_center_size(
                    pos2(del_btn_rect.left() - 14.0, rect.center().y),
                    vec2(22.0, 22.0),
                );
                let plain_resp = btns_ui
                    .allocate_rect(plain_btn_rect, Sense::click())
                    .on_hover_text("Paste as plain text");

                if plain_resp.clicked() {
                    action = Some(PopupAction::PastePlain(plain_text.to_string()));
                }

                if btns_ui.is_rect_visible(plain_btn_rect) {
                    let painter = btns_ui.painter();
                    if is_plain_focused || plain_resp.hovered() {
                        painter.rect_filled(plain_btn_rect, metrics::RADIUS_TIGHT, p.control_hover);
                        painter.rect_stroke(
                            plain_btn_rect,
                            metrics::RADIUS_TIGHT,
                            Stroke::new(metrics::HAIRLINE, p.accent),
                            StrokeKind::Inside,
                        );
                    }
                    let icon_color = if is_plain_focused {
                        p.accent
                    } else {
                        p.text_muted
                    };
                    Icon::FileText.paint(
                        painter,
                        Rect::from_center_size(plain_btn_rect.center(), vec2(13.0, 13.0)),
                        icon_color,
                    );
                }
            }

            // 3. Little Pen button for images (placeholder for edit mode)
            if entry.is_image() {
                let is_pen_focused =
                    is_selected && self.focused_button == Some(FocusedButton::Pen);
                let pen_btn_rect = Rect::from_center_size(
                    pos2(del_btn_rect.left() - 14.0, rect.center().y),
                    vec2(22.0, 22.0),
                );
                let pen_resp = btns_ui
                    .allocate_rect(pen_btn_rect, Sense::click())
                    .on_hover_text("Edit image (coming soon)");

                if btns_ui.is_rect_visible(pen_btn_rect) {
                    let painter = btns_ui.painter();
                    if is_pen_focused || pen_resp.hovered() {
                        painter.rect_filled(pen_btn_rect, metrics::RADIUS_TIGHT, p.control_hover);
                        painter.rect_stroke(
                            pen_btn_rect,
                            metrics::RADIUS_TIGHT,
                            Stroke::new(metrics::HAIRLINE, p.accent),
                            StrokeKind::Inside,
                        );
                    }
                    let icon_color = if is_pen_focused {
                        p.accent
                    } else {
                        p.text_muted
                    };
                    Icon::Pen.paint(
                        painter,
                        Rect::from_center_size(pen_btn_rect.center(), vec2(13.0, 13.0)),
                        icon_color,
                    );
                }
            }
        }

        action
    }
}

fn fit_text(
    ui: &Ui,
    text: &str,
    font: egui::FontId,
    color: Color32,
    max_w: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap.max_width = max_w;
    job.wrap.break_anywhere = true;
    job.wrap.max_rows = 1;
    ui.fonts(|f| f.layout_job(job))
}
