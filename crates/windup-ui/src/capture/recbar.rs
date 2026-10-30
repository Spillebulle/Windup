//! The recording bar (spec "Recording"): what shows while a recording runs.
//! A pulsing dot, the elapsed time, the frame rate and bitrate, Pause and
//! Stop. It is drawn into a window or surface the host keeps out of the
//! recording; this view only paints and reports clicks and keys.
//!
//! A Steps session uses the same bar with its step count in place of the
//! dot, the timer and the chips: nothing records between steps, so nothing
//! pulses.
//!
//! Space and Enter work only while the bar's own window has focus; Esc is
//! global and handled by the host, never here.

use super::bar::{fps_label, tooltip};
use crate::icons::Icon;
use crate::theme::{Mode, Palette, metrics, text};
use egui::{
    Color32, Context, FontId, Id, Key, LayerId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2,
    pos2, vec2,
};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub struct RecBarState {
    /// Seconds recorded so far; the caller stops advancing it while paused.
    pub elapsed: f64,
    pub paused: bool,
    pub fps: u32,
    /// The second figure: a bitrate, or the colour count for a GIF.
    pub bitrate_label: String,
    /// The OS reduced-motion setting: the dot then stays still.
    pub reduced_motion: bool,
    /// A Steps session's step count. The bar then shows it, in place of the
    /// dot, the timer and the chips; pause and stop stay.
    pub steps: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecBarAction {
    Pause,
    Resume,
    Stop,
}

/// `m:ss`, or `h:mm:ss` from one hour.
pub fn timer_text(elapsed: f64) -> String {
    let s = elapsed.max(0.0) as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The dot's opacity at time `t`. Spec section 13 allows exactly one moving
/// thing outside the interface's own feedback: this recording indicator,
/// which is how people know they are on air. It stays still when motion is
/// reduced or the recording is paused.
pub fn dot_alpha(t: f64, still: bool) -> f32 {
    if still {
        return 1.0;
    }
    let phase = std::f64::consts::TAU * t / metrics::REC_PULSE_SECS;
    (0.55 + 0.45 * phase.cos()) as f32
}

/// `1 step`, `3 steps`.
pub fn count_text(n: u32) -> String {
    if n == 1 {
        "1 step".to_string()
    } else {
        format!("{n} steps")
    }
}

/// The widest the count gets at this many digits, so the bar keeps its
/// width from one step to the next until a digit is added.
fn count_slot(ctx: &Context, n: u32) -> f32 {
    let digits = "0".repeat(n.to_string().len());
    width_of(ctx, &format!("{digits} steps"), timer_font())
}

fn timer_font() -> FontId {
    FontId::monospace(text::CONTROL)
}

fn chip_font() -> FontId {
    FontId::monospace(text::TINY)
}

fn width_of(ctx: &Context, s: &str, font: FontId) -> f32 {
    ctx.layer_painter(LayerId::background())
        .layout_no_wrap(s.to_string(), font, Color32::WHITE)
        .size()
        .x
}

fn chip_w(ctx: &Context, s: &str) -> f32 {
    width_of(ctx, s, chip_font()) + 2.0 * metrics::CHIP_PAD_X
}

/// The widest the timer gets in this hour bracket (the font is mono, so a
/// width of digits is a width of any digits).
fn timer_slot(ctx: &Context, elapsed: f64) -> f32 {
    let widest = if elapsed >= 3600.0 {
        "0:00:00"
    } else {
        "00:00"
    };
    width_of(ctx, widest, timer_font())
}

fn button_size() -> Vec2 {
    vec2(metrics::SEG_ICON_W, metrics::BUTTON)
}

/// The bar's size, so a host can size its window or surface before the
/// first frame (inside or after one egui frame, since text can be measured
/// only then). Stable while the timer stays in its hour bracket.
pub fn bar_size(ctx: &Context, state: &RecBarState) -> Vec2 {
    let gap = metrics::REC_GAP;
    let lead = match state.steps {
        Some(n) => metrics::ICON + gap + count_slot(ctx, n) + gap,
        None => {
            metrics::REC_DOT
                + gap
                + timer_slot(ctx, state.elapsed)
                + gap
                + chip_w(ctx, &fps_label(state.fps))
                + gap
                + chip_w(ctx, &state.bitrate_label)
                + gap
        }
    };
    let w = metrics::S2
        + lead
        + metrics::HAIRLINE
        + gap
        + 2.0 * button_size().x
        + metrics::SEG_PAD
        + metrics::S2;
    vec2(w, metrics::REC_BAR_H)
}

/// The window or surface a host keeps for the bar: the bar at its top, and
/// below it room for the buttons' tooltips, which paint under the bar and
/// are clipped to the surface. Hosts place and size this, not the bar alone,
/// so the room too stays off the recorded area.
pub fn host_size(ctx: &Context, state: &RecBarState) -> Vec2 {
    let bar = bar_size(ctx, state);
    let tip = ctx
        .layer_painter(LayerId::background())
        .layout_no_wrap(
            "Ag".into(),
            FontId::proportional(text::TINY),
            Color32::WHITE,
        )
        .size()
        .y
        + 2.0 * metrics::TOOLTIP_PAD_Y;
    vec2(bar.x, bar.y + metrics::POPOVER_GAP + tip + metrics::S2)
}

#[derive(Default)]
pub struct RecBarView {
    tip: Option<(Id, f64)>,
}

impl RecBarView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a button is hovered, so its tooltip is due or showing under
    /// the bar: a host that limits where its window takes the pointer
    /// widens that to the tooltip room meanwhile.
    pub fn tip_pending(&self) -> bool {
        self.tip.is_some()
    }

    /// Paints the bar at the top-left of `ui`'s space and returns what the
    /// user asked for this frame.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        mode: Mode,
        state: &RecBarState,
    ) -> Option<RecBarAction> {
        let ctx = ui.ctx().clone();
        let now = ui.input(|i| i.time);
        let size = bar_size(&ctx, state);
        let bar = Rect::from_min_size(ui.max_rect().min, size);
        let painter = ui.painter().clone();
        let gap = metrics::REC_GAP;

        painter.rect_filled(bar, metrics::RADIUS_CARD, p.popover);
        painter.rect_stroke(
            bar,
            metrics::RADIUS_CARD,
            Stroke::new(metrics::HAIRLINE, p.line_popover),
            StrokeKind::Inside,
        );

        let cy = bar.center().y;
        let mut x = bar.left() + metrics::S2;

        if let Some(n) = state.steps {
            Icon::Footprints.paint(
                &painter,
                Rect::from_min_size(
                    pos2(x, cy - metrics::ICON / 2.0),
                    vec2(metrics::ICON, metrics::ICON),
                ),
                p.accent,
            );
            x += metrics::ICON + gap;
            let count = painter.layout_no_wrap(count_text(n), timer_font(), p.text_strong);
            painter.galley(pos2(x, cy - count.size().y / 2.0), count, p.text_strong);
            x += count_slot(&ctx, n) + gap;
        } else {
            let still = state.reduced_motion || state.paused;
            painter.circle_filled(
                pos2(x + metrics::REC_DOT / 2.0, cy),
                metrics::REC_DOT / 2.0,
                p.critical.gamma_multiply(dot_alpha(now, still)),
            );
            x += metrics::REC_DOT + gap;

            let timer =
                painter.layout_no_wrap(timer_text(state.elapsed), timer_font(), p.text_strong);
            painter.galley(pos2(x, cy - timer.size().y / 2.0), timer, p.text_strong);
            x += timer_slot(&ctx, state.elapsed) + gap;

            for label in [fps_label(state.fps), state.bitrate_label.clone()] {
                let w = chip_w(&ctx, &label);
                let h = text::TINY + 2.0 * metrics::CHIP_PAD_Y;
                let r = Rect::from_min_size(pos2(x, cy - h / 2.0), vec2(w, h));
                painter.rect_filled(r, metrics::RADIUS, p.control);
                painter.text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    chip_font(),
                    p.text_muted,
                );
                x += w + gap;
            }
        }

        painter.line_segment(
            [
                pos2(x, bar.top() + metrics::S2),
                pos2(x, bar.bottom() - metrics::S2),
            ],
            Stroke::new(metrics::HAIRLINE, p.line_soft),
        );
        x += metrics::HAIRLINE + gap;

        let mut action = None;
        let mut hovered_tip: Option<(Id, Rect, &str)> = None;
        let (pause_icon, pause_tip, pause_action) = if state.paused {
            (Icon::Play, "Resume (Space)", RecBarAction::Resume)
        } else {
            (Icon::Pause, "Pause (Space)", RecBarAction::Pause)
        };
        let buttons = [
            (Id::new("recbar-pause"), pause_icon, pause_tip, pause_action),
            (
                Id::new("recbar-stop"),
                Icon::Square,
                "Stop (Enter)",
                RecBarAction::Stop,
            ),
        ];
        for (id, icon, tip, act) in buttons {
            let r = Rect::from_min_size(Pos2::new(x, cy - button_size().y / 2.0), button_size());
            let resp = ui.interact(r, id, Sense::click());
            let hover = resp.hovered();
            if hover {
                painter.rect_filled(r, metrics::RADIUS_SEG, p.control_hover);
                hovered_tip = Some((id, r, tip));
            }
            icon.paint(
                &painter,
                Rect::from_center_size(r.center(), vec2(metrics::ICON, metrics::ICON)),
                if hover { p.text_strong } else { p.text_muted },
            );
            if resp.clicked() {
                action = Some(act);
            }
            x += r.width() + metrics::SEG_PAD;
        }

        // Keys count only while this window is the focused one.
        if action.is_none() && ui.input(|i| i.focused) {
            if ui.input(|i| i.key_pressed(Key::Space)) {
                action = Some(pause_action);
            } else if ui.input(|i| i.key_pressed(Key::Enter)) {
                action = Some(RecBarAction::Stop);
            }
        }

        // The tooltip, after the section 7.17 delay.
        let screen = ctx.screen_rect();
        match hovered_tip {
            Some((id, rect, label)) => {
                let since = match self.tip {
                    Some((held, t)) if held == id => t,
                    _ => {
                        self.tip = Some((id, now));
                        now
                    }
                };
                let delay = metrics::TOOLTIP_DELAY_MS as f64 / 1000.0;
                if now - since >= delay {
                    tooltip(&ctx, p, mode, rect, screen, label, None);
                } else {
                    ctx.request_repaint_after(Duration::from_secs_f64(delay - (now - since)));
                }
            }
            None => self.tip = None,
        }

        // The pulse needs frames; the timer needs one a few times a second.
        // A step count changes only when the host says so.
        if !state.paused && state.steps.is_none() {
            let ms = if state.reduced_motion { 250 } else { 33 };
            ctx.request_repaint_after(Duration::from_millis(ms));
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, RawInput};

    fn state() -> RecBarState {
        RecBarState {
            elapsed: 12.0,
            paused: false,
            fps: 30,
            bitrate_label: "8 Mb/s".into(),
            reduced_motion: false,
            steps: None,
        }
    }

    #[test]
    fn the_timer_reads_minutes_then_hours() {
        assert_eq!(timer_text(0.0), "0:00");
        assert_eq!(timer_text(59.9), "0:59");
        assert_eq!(timer_text(754.0), "12:34");
        assert_eq!(timer_text(3599.0), "59:59");
        assert_eq!(timer_text(3600.0), "1:00:00");
    }

    #[test]
    fn reduced_motion_keeps_the_dot_still() {
        assert_eq!(dot_alpha(0.0, true), dot_alpha(0.8, true));
        assert_ne!(dot_alpha(0.0, false), dot_alpha(0.8, false));
        assert!((dot_alpha(0.0, false) - 1.0).abs() < 1e-6);
        assert!((dot_alpha(0.8, false) - 0.1).abs() < 1e-6);
    }

    /// The bar in a window of its own size, frame by frame.
    struct Rig {
        ctx: Context,
        view: RecBarView,
        t: f64,
    }

    impl Rig {
        fn new() -> Self {
            let ctx = Context::default();
            crate::theme::install_fonts(&ctx);
            // Fonts exist only once a frame has run.
            let _ = ctx.run(RawInput::default(), |_| {});
            Rig {
                ctx,
                view: RecBarView::new(),
                t: 0.0,
            }
        }

        fn frame(
            &mut self,
            s: &RecBarState,
            focused: bool,
            events: Vec<Event>,
        ) -> Option<RecBarAction> {
            self.t += 0.016;
            let size = bar_size(&self.ctx, s);
            let raw = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                time: Some(self.t),
                focused,
                events,
                ..Default::default()
            };
            let p = Palette::dark(28.0);
            let mut out = None;
            let view = &mut self.view;
            let _ = self.ctx.run(raw, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| out = view.show(ui, &p, Mode::Dark, s));
            });
            out
        }

        /// Move to `at`, press, release; the first action reported.
        fn click(&mut self, s: &RecBarState, at: Pos2) -> Option<RecBarAction> {
            let b = |pressed| Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            let mut got = None;
            for ev in [Event::PointerMoved(at), b(true), b(false)] {
                got = got.or(self.frame(s, true, vec![ev]));
            }
            got
        }
    }

    fn key(k: Key) -> Vec<Event> {
        vec![Event::Key {
            key: k,
            physical_key: Some(k),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]
    }

    #[test]
    fn the_bar_reports_pause_and_stop_clicks() {
        let mut rig = Rig::new();
        let mut s = state();
        let size = bar_size(&rig.ctx, &s);
        // Both buttons sit at the right: stop last, pause before it.
        let stop = pos2(
            size.x - metrics::S2 - metrics::SEG_ICON_W / 2.0,
            size.y / 2.0,
        );
        let pause = stop - vec2(metrics::SEG_ICON_W + metrics::SEG_PAD, 0.0);
        assert_eq!(rig.click(&s, pause), Some(RecBarAction::Pause));
        assert_eq!(rig.click(&s, stop), Some(RecBarAction::Stop));
        s.paused = true;
        assert_eq!(rig.click(&s, pause), Some(RecBarAction::Resume));
    }

    #[test]
    fn space_and_enter_do_nothing_without_focus() {
        let mut rig = Rig::new();
        let s = state();
        assert_eq!(rig.frame(&s, false, key(Key::Space)), None);
        assert_eq!(rig.frame(&s, false, key(Key::Enter)), None);
        assert_eq!(
            rig.frame(&s, true, key(Key::Space)),
            Some(RecBarAction::Pause)
        );
        assert_eq!(
            rig.frame(&s, true, key(Key::Enter)),
            Some(RecBarAction::Stop)
        );
        let paused = RecBarState { paused: true, ..s };
        assert_eq!(
            rig.frame(&paused, true, key(Key::Space)),
            Some(RecBarAction::Resume)
        );
    }

    #[test]
    fn the_size_holds_while_the_timer_runs_within_an_hour() {
        let rig = Rig::new();
        let at = |e| {
            bar_size(
                &rig.ctx,
                &RecBarState {
                    elapsed: e,
                    ..state()
                },
            )
        };
        assert_eq!(at(0.0), at(3599.0));
        assert!(at(3600.0).x > at(0.0).x);
    }

    #[test]
    fn hovering_a_button_makes_a_tip_pending_and_leaving_ends_it() {
        let mut rig = Rig::new();
        let s = state();
        let size = bar_size(&rig.ctx, &s);
        let stop = pos2(
            size.x - metrics::S2 - metrics::SEG_ICON_W / 2.0,
            size.y / 2.0,
        );
        // Hover is known from the frame after the pointer arrives.
        rig.frame(&s, false, vec![Event::PointerMoved(stop)]);
        rig.frame(&s, false, vec![]);
        assert!(rig.view.tip_pending());
        rig.frame(&s, false, vec![Event::PointerMoved(pos2(1.0, 1.0))]);
        assert!(!rig.view.tip_pending());
    }

    #[test]
    fn the_count_says_step_or_steps() {
        assert_eq!(count_text(1), "1 step");
        assert_eq!(count_text(3), "3 steps");
        assert_eq!(count_text(0), "0 steps");
    }

    #[test]
    fn a_steps_bar_is_narrower_than_a_video_bar_and_keeps_its_buttons() {
        let mut rig = Rig::new();
        let video = state();
        let mut s = RecBarState {
            steps: Some(3),
            ..state()
        };
        let size = bar_size(&rig.ctx, &s);
        assert!(size.x < bar_size(&rig.ctx, &video).x, "{size:?}");
        assert_eq!(size.y, metrics::REC_BAR_H);
        assert!(host_size(&rig.ctx, &s).x == size.x);
        let stop = pos2(
            size.x - metrics::S2 - metrics::SEG_ICON_W / 2.0,
            size.y / 2.0,
        );
        let pause = stop - vec2(metrics::SEG_ICON_W + metrics::SEG_PAD, 0.0);
        assert_eq!(rig.click(&s, pause), Some(RecBarAction::Pause));
        assert_eq!(rig.click(&s, stop), Some(RecBarAction::Stop));
        s.paused = true;
        assert_eq!(rig.click(&s, pause), Some(RecBarAction::Resume));
    }

    #[test]
    fn the_host_leaves_room_for_a_tooltip_under_the_bar() {
        let rig = Rig::new();
        let s = state();
        let bar = bar_size(&rig.ctx, &s);
        let host = host_size(&rig.ctx, &s);
        assert_eq!(host.x, bar.x);
        assert!(
            host.y >= bar.y + metrics::POPOVER_GAP + text::TINY + 2.0 * metrics::TOOLTIP_PAD_Y,
            "{host:?} for {bar:?}"
        );
    }
}
