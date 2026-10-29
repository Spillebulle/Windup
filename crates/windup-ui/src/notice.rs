//! A small card that says one thing, with an OK button: why a capture could
//! not start, why its picture could not be saved, or where a recording went.
//! It may carry one more button for what to do next ("Open folder"). The
//! host draws it into its own borderless viewport at the cursor, sized by
//! [`size`], the way it draws the clipboard popup, and in the popup's card
//! style.

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

/// How the card was put away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeReply {
    /// OK, Enter or Esc.
    Dismissed,
    /// The `action` button; the card goes too.
    Action,
}

/// Draws the card over `ui.max_rect()`, with an `action` button beside OK
/// when there is one. Says how it was put away, if it was.
pub fn show(ui: &mut Ui, p: &Palette, message: &str, action: Option<&str>) -> Option<NoticeReply> {
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
    let (ok, act) = ui
        .scope_builder(
            UiBuilder::new()
                .max_rect(row)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = metrics::S2;
                let ok = button(ui, p, "OK", true).clicked();
                let act = action.is_some_and(|label| button(ui, p, label, false).clicked());
                (ok, act)
            },
        )
        .inner;
    if act {
        Some(NoticeReply::Action)
    } else if ok || ui.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Escape)) {
        Some(NoticeReply::Dismissed)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, Pos2, RawInput};

    fn frame_with(
        ctx: &egui::Context,
        events: Vec<Event>,
        message: &str,
        action: Option<&str>,
    ) -> Option<NoticeReply> {
        let p = Palette::dark(28.0);
        let size = size(ctx, message, &p);
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            events,
            ..Default::default()
        };
        let mut done = None;
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| done = show(ui, &p, message, action));
        });
        done
    }

    fn frame(ctx: &egui::Context, events: Vec<Event>, message: &str) -> bool {
        frame_with(ctx, events, message, None) == Some(NoticeReply::Dismissed)
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

    /// A click at `at`: move, press, release, one frame each.
    fn click(ctx: &egui::Context, at: Pos2, m: &str, action: Option<&str>) -> Option<NoticeReply> {
        let b = |pressed| Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        let mut got = None;
        for ev in [Event::PointerMoved(at), b(true), b(false)] {
            got = got.or(frame_with(ctx, vec![ev], m, action));
        }
        got
    }

    #[test]
    fn the_action_button_sits_left_of_ok_and_reports_itself() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run(RawInput::default(), |_| {});
        let p = Palette::dark(28.0);
        let m = "Saved to Recording 2026-10-29 101500.mp4";
        let card = size(&ctx, m, &p);
        let y = card.y - metrics::S4 - metrics::BUTTON / 2.0;
        // OK is the rightmost button; just inside the card's padding.
        let ok = Pos2::new(card.x - metrics::S4 - 4.0, y);
        assert_eq!(
            click(&ctx, ok, m, Some("Open folder")),
            Some(NoticeReply::Dismissed)
        );
        // Well left of OK, before the action's right edge: the action.
        let action = Pos2::new(card.x - metrics::S4 - 80.0, y);
        assert_eq!(
            click(&ctx, action, m, Some("Open folder")),
            Some(NoticeReply::Action)
        );
        // Without one, the same spot is empty.
        assert_eq!(click(&ctx, action, m, None), None);
        // Enter still means OK.
        assert_eq!(
            frame_with(&ctx, vec![key(Key::Enter)], m, Some("Open folder")),
            Some(NoticeReply::Dismissed)
        );
    }
}
