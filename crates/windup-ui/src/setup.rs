//! The first-run installer's window: the mark and a heading, a bar, one line
//! saying which step it is on, and at most one primary button.
//!
//! Drawing only. `windup-desktop`'s `setup.rs` owns the worker that runs
//! msiexec and feeds this a [`Step`]; what each step means is decided in
//! `windup_core::install`.

use crate::Icon;
use crate::theme::{Palette, metrics, strong, text};
use crate::widgets::button;
use egui::{Align, Layout, Rect, RichText, Sense, Ui, vec2};
use std::path::Path;
use windup_core::install::Step;

/// The window, in points. Small: it says one thing.
pub const WINDOW: [f32; 2] = [440.0, 220.0];

/// The mark's side beside the heading.
const MARK: f32 = 32.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupAction {
    Install,
    Close,
}

pub fn show(
    ui: &mut Ui,
    p: &Palette,
    step: &Step,
    version: &str,
    log: Option<&Path>,
) -> Option<SetupAction> {
    let mut action = None;

    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(MARK, MARK), Sense::hover());
        ui.painter()
            .rect_filled(rect, metrics::RADIUS_CARD, p.accent);
        Icon::Wrench.paint(ui.painter(), rect.shrink(MARK * 0.2), p.accent_ink);
        ui.add_space(metrics::S3 - ui.spacing().item_spacing.x);
        let heading = match version {
            "" => "Install Windup".to_string(),
            v => format!("Install Windup {v}"),
        };
        ui.label(
            RichText::new(heading)
                .font(strong(text::PAGE))
                .color(p.text_strong),
        );
    });
    ui.add_space(metrics::S5);

    bar(ui, p, step.progress());
    ui.add_space(metrics::S2);
    ui.label(RichText::new(step.label()).size(text::BODY).color(p.text));

    if let (Step::Failed(_), Some(log)) = (step, log) {
        ui.add_space(metrics::S1);
        ui.label(
            RichText::new(format!("The installer's log is at {}", log.display()))
                .size(text::TINY)
                .color(p.text_dim),
        );
    }

    ui.with_layout(Layout::bottom_up(Align::Max), |ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| match step {
            Step::Ready => {
                if button(ui, p, "Install", true).clicked() {
                    action = Some(SetupAction::Install);
                }
                if button(ui, p, "Cancel", false).clicked() {
                    action = Some(SetupAction::Close);
                }
            }
            Step::Installed | Step::Failed(_) if button(ui, p, "Close", true).clicked() => {
                action = Some(SetupAction::Close);
            }
            // Working. No Cancel: msiexec cannot be called back once Windows
            // has it, and a button that did nothing would be a lie.
            _ => {}
        });
    });

    action
}

/// A rail that fills to `fraction`, and stays an empty track when there is
/// nothing honest to show.
fn bar(ui: &mut Ui, p: &Palette, fraction: Option<f32>) {
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), metrics::RAIL), Sense::hover());
    ui.painter().rect_filled(rect, metrics::RAIL / 2.0, p.rail);
    if let Some(f) = fraction {
        let filled = Rect::from_min_size(
            rect.left_top(),
            vec2(rect.width() * f.clamp(0.0, 1.0), rect.height()),
        );
        ui.painter()
            .rect_filled(filled, metrics::RAIL / 2.0, p.accent);
    }
}
