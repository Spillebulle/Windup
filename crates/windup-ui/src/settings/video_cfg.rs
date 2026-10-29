//! Video tool settings. A placeholder until recording exists.

use crate::theme::{Palette, text};
use egui::{RichText, Ui};
use windup_core::Config;

pub fn render(ui: &mut Ui, _config: &mut Config, p: &Palette) {
    ui.label(
        RichText::new("Video settings arrive with recording.")
            .size(text::SMALL)
            .color(p.text_dim),
    );
}
