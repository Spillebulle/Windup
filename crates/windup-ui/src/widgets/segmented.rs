//! Segmented control widget. §7.9 of STYLE-GUIDE.md.
//! Rounded container with individual clickable segments; active segment has neutral fill and strong text.

use crate::theme::{Palette, metrics, text};
use egui::{Rect, Sense, Stroke, Ui, pos2, vec2};

pub fn segmented<T: Copy + PartialEq>(
    ui: &mut Ui,
    p: Palette,
    value: &mut T,
    options: &[T],
    label: impl Fn(T) -> &'static str,
) -> bool {
    let mut changed = false;
    let seg_w = 96.0;
    let total_w = seg_w * options.len() as f32;
    let (rect, _) = ui.allocate_exact_size(vec2(total_w, metrics::BUTTON), Sense::hover());

    ui.painter().rect_filled(rect, metrics::RADIUS, p.control);
    ui.painter().rect_stroke(
        rect,
        metrics::RADIUS,
        Stroke::new(metrics::HAIRLINE, p.line),
        egui::StrokeKind::Inside,
    );

    for (i, option) in options.iter().enumerate() {
        let seg = Rect::from_min_size(
            pos2(rect.left() + seg_w * i as f32, rect.top()),
            vec2(seg_w, rect.height()),
        );
        let response = ui.interact(seg, ui.id().with(("seg", i)), Sense::click());
        let selected = *value == *option;

        if selected {
            // §7.9: A neutral fill and strong text, never an accent background.
            ui.painter()
                .rect_filled(seg.shrink(2.0), metrics::RADIUS_TIGHT, p.control_active);
        } else if response.hovered() {
            ui.painter()
                .rect_filled(seg.shrink(2.0), metrics::RADIUS_TIGHT, p.control_hover);
        }

        ui.painter().text(
            seg.center(),
            egui::Align2::CENTER_CENTER,
            label(*option),
            egui::FontId::proportional(text::CONTROL),
            if selected {
                p.text_strong
            } else {
                p.text_muted
            },
        );

        if response.clicked() && !selected {
            *value = *option;
            changed = true;
        }
    }

    changed
}
