//! A small card that says one thing, with an OK button: why a capture could
//! not start, or why its picture could not be saved. The host draws it into
//! its own borderless viewport at the cursor, sized by [`size`], the way it
//! draws the clipboard popup, and in the popup's card style.

use crate::theme::{Palette, metrics, text};
use crate::widgets::button;
use egui::{
    Align, FontId, Galley, Key, Layout, Rect, Stroke, StrokeKind, Ui, UiBuilder, Vec2, vec2,
};
use std::sync::Arc;

fn galley(ctx: &egui::Context, message: &str, p: &Palette) -> Arc<Galley> {
    let wrap = metrics::NOTICE_W - 2.0 * metrics::S4;
    ctx.fonts(|f| {
        f.layout(
            message.to_owned(),
            FontId::proportional(text::BODY),
            p.text,
            wrap,
        )
    })
}

/// The card's size in points for `message`.
pub fn size(ctx: &egui::Context, message: &str, p: &Palette) -> Vec2 {
    let text_h = galley(ctx, message, p).size().y;
    vec2(
        metrics::NOTICE_W,
        metrics::S4 + text_h + metrics::S3 + metrics::BUTTON + metrics::S4,
    )
}

/// Draws the card over `ui.max_rect()`. True when it is dismissed: OK, Enter
/// or Esc.
pub fn show(ui: &mut Ui, p: &Palette, message: &str) -> bool {
    let card = ui.max_rect();
    let painter = ui.painter();
    painter.rect_filled(card, metrics::RADIUS_CARD, p.dock);
    painter.rect_stroke(
        card,
        metrics::RADIUS_CARD,
        Stroke::new(metrics::HAIRLINE, p.line_popover),
        StrokeKind::Inside,
    );
    let inner = card.shrink(metrics::S4);
    painter.galley(inner.min, galley(ui.ctx(), message, p), p.text);

    let row = Rect::from_min_max(inner.max - vec2(inner.width(), metrics::BUTTON), inner.max);
    let ok = ui
        .scope_builder(
            UiBuilder::new()
                .max_rect(row)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| button(ui, p, "OK", true).clicked(),
        )
        .inner;
    ok || ui.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Escape))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, Pos2, RawInput};

    fn frame(ctx: &egui::Context, events: Vec<Event>, message: &str) -> bool {
        let p = Palette::dark(28.0);
        let size = size(ctx, message, &p);
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            events,
            ..Default::default()
        };
        let mut done = false;
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| done = show(ui, &p, message));
        });
        done
    }

    fn key(key: Key) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }

    #[test]
    fn a_longer_message_makes_a_taller_card_of_the_same_width() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let p = Palette::dark(28.0);
        // Fonts are ready after the first pass.
        let _ = ctx.run(RawInput::default(), |_| {});
        let short = size(&ctx, "No.", &p);
        let long = size(&ctx, &"The screen could not be captured. ".repeat(6), &p);
        assert_eq!(short.x, metrics::NOTICE_W);
        assert_eq!(long.x, metrics::NOTICE_W);
        assert!(long.y > short.y, "{long:?} vs {short:?}");
    }

    #[test]
    fn enter_and_escape_dismiss_and_nothing_else_does() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run(RawInput::default(), |_| {});
        let m = "The screen could not be captured.";
        assert!(!frame(&ctx, vec![], m));
        assert!(!frame(&ctx, vec![key(Key::A)], m));
        assert!(frame(&ctx, vec![key(Key::Enter)], m));
        assert!(frame(&ctx, vec![key(Key::Escape)], m));
    }
}
