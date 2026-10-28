//! The view driven through a real egui context with synthetic input: the
//! pointer and key plumbing the pure tests in `mod.rs` cannot reach.

use super::*;

use egui::{Modifiers, PointerButton, RawInput};
use image::RgbaImage;
use windup_core::capture::session::Outcome;
use windup_core::capture::{DesktopShot, MonitorShot};
use windup_core::config::ScreenshotConfig;

/// Two monitors side by side in one 400 x 200 point window, one point to
/// one desktop unit: monitor 0 at x = -200 at 1x, monitor 1 at x = 0 at
/// 2x. The same arrangement the example has, small.
struct Rig {
    ctx: egui::Context,
    session: Session,
    view: CaptureView,
    keys: EditorKeys,
    t: f64,
}

impl Rig {
    fn new() -> Self {
        let monitor = |x: f32, ppu: u32| MonitorShot {
            layout: DRect {
                x,
                y: 0.0,
                w: 200.0,
                h: 200.0,
            },
            image: RgbaImage::from_pixel(200 * ppu, 200 * ppu, image::Rgba([90, 90, 90, 255])),
            name: String::new(),
            scale: 1.0,
        };
        let shot = DesktopShot {
            monitors: vec![monitor(-200.0, 1), monitor(0.0, 2)],
        };
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        Rig {
            ctx,
            session: Session::new(shot, Vec::new(), &ScreenshotConfig::default()),
            view: CaptureView::new(),
            keys: EditorKeys::default(),
            t: 0.0,
        }
    }

    /// One frame with these events; the outcome if any surface ended it.
    fn frame(&mut self, events: Vec<Event>) -> Option<Outcome> {
        self.t += 0.016;
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 200.0))),
            time: Some(self.t),
            events,
            ..Default::default()
        };
        let p = Palette::dark(28.0);
        let mut out = None;
        let _ = self.ctx.run(raw, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    for i in 0..2 {
                        let surface =
                            Rect::from_min_size(pos2(200.0 * i as f32, 0.0), vec2(200.0, 200.0));
                        let o = ui
                            .scope_builder(egui::UiBuilder::new().max_rect(surface), |ui| {
                                self.view
                                    .monitor_ui(ui, &mut self.session, i, &p, &self.keys)
                            })
                            .inner;
                        if out.is_none() {
                            out = o;
                        }
                    }
                });
        });
        out
    }

    /// A primary-button drag from `a` to `b`, in window points, over
    /// several frames.
    fn drag(&mut self, a: Pos2, b: Pos2) {
        let button = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.frame(vec![Event::PointerMoved(a)]);
        self.frame(vec![button(a, true)]);
        for k in 1..=4 {
            let at = a + (b - a) * (k as f32 / 4.0);
            self.frame(vec![Event::PointerMoved(at)]);
        }
        self.frame(vec![button(b, false)]);
    }

    fn key(&mut self, key: egui::Key, modifiers: Modifiers) -> Option<Outcome> {
        self.frame(vec![Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }])
    }
}

// Below the bar, which sits at the top of the monitor under the pointer.
const Y0: f32 = 80.0;

#[test]
fn an_area_drag_on_the_2x_monitor_selects_in_desktop_units_and_enter_copies_once() {
    let mut rig = Rig::new();
    rig.drag(pos2(260.0, Y0), pos2(340.0, Y0 + 80.0));
    assert_eq!(rig.session.stage, Stage::Editing);
    let r = rig.session.selection.rect.expect("a selection");
    assert_eq!((r.x, r.y, r.w, r.h), (60.0, Y0, 80.0, 80.0));

    match rig.key(egui::Key::Enter, Modifiers::NONE) {
        // 80 units at 2 pixels a unit.
        Some(Outcome::Copy(img)) => assert_eq!(img.dimensions(), (160, 160)),
        other => panic!("expected a copy, got {other:?}"),
    }
    assert!(rig.key(egui::Key::Enter, Modifiers::NONE).is_none(), "once");
    assert!(rig.frame(vec![]).is_none(), "and never again");
}

#[test]
fn a_drag_across_the_seam_is_one_selection() {
    let mut rig = Rig::new();
    rig.drag(pos2(150.0, Y0), pos2(250.0, Y0 + 50.0));
    let r = rig.session.selection.rect.expect("a selection");
    assert_eq!((r.x, r.y, r.w, r.h), (-50.0, Y0, 100.0, 50.0));
    match rig.key(egui::Key::S, Modifiers::COMMAND) {
        // At the higher of the two scales.
        Some(Outcome::Save(img)) => assert_eq!(img.dimensions(), (200, 100)),
        other => panic!("expected a save, got {other:?}"),
    }
}

#[test]
fn a_key_acts_once_however_many_surfaces_share_the_viewport() {
    let mut rig = Rig::new();
    rig.drag(pos2(260.0, Y0), pos2(340.0, Y0 + 80.0));
    let before = rig.session.selection.rect.expect("a selection").x;
    rig.key(egui::Key::ArrowRight, Modifiers::NONE);
    let after = rig.session.selection.rect.expect("a selection").x;
    assert_eq!(after - before, 1.0);
}

#[test]
fn escape_cancels_from_the_launcher() {
    let mut rig = Rig::new();
    rig.frame(vec![Event::PointerMoved(pos2(100.0, Y0))]);
    assert!(matches!(
        rig.key(egui::Key::Escape, Modifiers::NONE),
        Some(Outcome::Cancel)
    ));
}

#[test]
fn the_wheel_sizes_the_active_tool() {
    let mut rig = Rig::new();
    rig.drag(pos2(260.0, Y0), pos2(340.0, Y0 + 80.0));
    rig.session.act(Action::SetTool(Tool::Pen));
    let before = rig.session.sizes.pen;
    rig.frame(vec![Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: vec2(0.0, 1.0),
        modifiers: Modifiers::NONE,
    }]);
    assert_eq!(rig.session.sizes.pen, before + 1.0);
    assert!(rig.session.size_flash.is_some());
}

#[test]
fn a_press_on_the_bar_does_not_start_a_selection() {
    let mut rig = Rig::new();
    // The bar is centred at the top of the monitor under the pointer.
    let on_bar = pos2(300.0, metrics::S4 + 10.0);
    rig.drag(on_bar, pos2(340.0, Y0 + 80.0));
    assert_eq!(rig.session.stage, Stage::Choosing);
    assert!(rig.session.selection.rect.is_none());
}

#[test]
fn the_loupe_shows_while_choosing_and_editing_only_to_aim() {
    let mut rig = Rig::new();
    rig.session.loupe = true;
    assert!(loupe_wanted(&rig.session), "choosing");
    rig.drag(pos2(260.0, Y0), pos2(340.0, Y0 + 80.0));
    assert!(!loupe_wanted(&rig.session), "editing with the select tool");
    rig.session.act(Action::SetTool(Tool::Pen));
    assert!(!loupe_wanted(&rig.session), "drawing");
    rig.session.act(Action::SetTool(Tool::Pipette));
    assert!(loupe_wanted(&rig.session), "the pipette");
    rig.session.act(Action::SetTool(Tool::Select));
    // The selection's right edge, at desktop x = 140.
    rig.session.press([140.0, Y0 + 40.0], Mods::default());
    assert!(loupe_wanted(&rig.session), "dragging an edge");
    rig.session.release([140.0, Y0 + 40.0], Mods::default());
    assert!(!loupe_wanted(&rig.session));
    rig.session.loupe = false;
    rig.session.act(Action::SetTool(Tool::Pipette));
    assert!(!loupe_wanted(&rig.session), "off is off");
}

#[test]
fn the_highlight_glides_for_one_tween_and_not_with_reduced_motion() {
    let mut rig = Rig::new();
    rig.session.act(Action::SetTarget(TargetKind::FullScreen));
    rig.session.pointer_moved([-100.0, 100.0], 0.0);
    rig.session.pointer_moved([100.0, 100.0], 1.0);
    assert!(gliding(&rig.session, 1.05, false), "on its way");
    assert!(!gliding(&rig.session, 1.05, true), "reduced motion");
    assert!(
        !gliding(&rig.session, 1.0 + TWEEN_SECS + 0.01, false),
        "arrived"
    );
}
