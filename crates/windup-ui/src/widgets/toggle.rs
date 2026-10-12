//! Painted toggle switch. §7.12 of STYLE-GUIDE.md.
//! 34x18 px pill, 14 px circular knob, neutral inactive rail, accent active rail.

use crate::theme::{Palette, metrics};
use egui::{Sense, Stroke, Ui, pos2, vec2};

pub fn toggle(ui: &mut Ui, p: Palette, on: &mut bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(34.0, 18.0), Sense::click());
    let changed = response.clicked();
    if changed {
        *on = !*on;
    }
    ui.painter()
        .rect_filled(rect, 9.0, if *on { p.accent } else { p.control });
    if !*on {
        ui.painter().rect_stroke(
            rect,
            9.0,
            Stroke::new(metrics::HAIRLINE, p.line),
            egui::StrokeKind::Inside,
        );
    }
    let x = if *on {
        rect.right() - 9.0
    } else {
        rect.left() + 9.0
    };
    ui.painter().circle_filled(
        pos2(x, rect.center().y),
        6.0,
        if *on { p.accent_ink } else { p.knob },
    );
    changed
}
