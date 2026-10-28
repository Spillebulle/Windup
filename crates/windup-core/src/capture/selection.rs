//! The area the user selects: drag, resize, move and nudge, in desktop units.

use super::geometry::DRect;
use crate::config::TargetKind;

pub const MIN_DRAG: f32 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
    Move,
}

#[derive(Clone, Copy, Debug)]
enum Drag {
    New {
        anchor: (f32, f32),
        prev: Option<DRect>,
    },
    Resize {
        handle: Handle,
        orig: DRect,
    },
    Move {
        grab: (f32, f32),
        orig: DRect,
    },
}

#[derive(Clone, Debug)]
pub struct Selection {
    pub rect: Option<DRect>,
    pub kind: TargetKind,
    drag: Option<Drag>,
}

impl Default for Selection {
    fn default() -> Self {
        Selection {
            rect: None,
            kind: TargetKind::Area,
            drag: None,
        }
    }
}

impl Selection {
    /// Start resizing or moving when `at` is on the rect, else a new drag.
    pub fn press(&mut self, at: (f32, f32), handle_tolerance: f32) {
        self.drag = Some(match (self.rect, self.handle_at(at, handle_tolerance)) {
            (Some(orig), Some(Handle::Move)) => Drag::Move {
                grab: (at.0 - orig.x, at.1 - orig.y),
                orig,
            },
            (Some(orig), Some(handle)) => Drag::Resize { handle, orig },
            (prev, _) => Drag::New { anchor: at, prev },
        });
    }

    /// The pointer is clamped to `bounds`; a moved rect stays inside it.
    pub fn drag_to(&mut self, at: (f32, f32), bounds: DRect) {
        let Some(drag) = self.drag else { return };
        let at = (
            at.0.clamp(bounds.x, bounds.right()),
            at.1.clamp(bounds.y, bounds.bottom()),
        );
        self.rect = Some(match drag {
            Drag::New { anchor, .. } => {
                let anchor = (
                    anchor.0.clamp(bounds.x, bounds.right()),
                    anchor.1.clamp(bounds.y, bounds.bottom()),
                );
                DRect::from_corners(anchor, at)
            }
            Drag::Resize { handle, orig } => {
                let (mut l, mut t, mut r, mut b) = (orig.x, orig.y, orig.right(), orig.bottom());
                match handle {
                    Handle::N | Handle::NE | Handle::NW => t = at.1,
                    Handle::S | Handle::SE | Handle::SW => b = at.1,
                    _ => {}
                }
                match handle {
                    Handle::W | Handle::NW | Handle::SW => l = at.0,
                    Handle::E | Handle::NE | Handle::SE => r = at.0,
                    _ => {}
                }
                DRect::from_corners((l, t), (r, b))
            }
            Drag::Move { grab, orig } => DRect {
                x: (at.0 - grab.0).min(bounds.right() - orig.w).max(bounds.x),
                y: (at.1 - grab.1).min(bounds.bottom() - orig.h).max(bounds.y),
                ..orig
            },
        });
    }

    /// A new drag shorter than `MIN_DRAG` in both directions was a click:
    /// the previous rect comes back.
    pub fn release(&mut self) {
        if let Some(Drag::New { prev, .. }) = self.drag.take()
            && let Some(rect) = self.rect
            && rect.w < MIN_DRAG
            && rect.h < MIN_DRAG
        {
            self.rect = prev;
        }
    }

    /// Corners win over edges, edges over the inside (`Move`).
    pub fn handle_at(&self, at: (f32, f32), tolerance: f32) -> Option<Handle> {
        let rect = self.rect?;
        let near = |a: f32, b: f32| (a - b).abs() <= tolerance;
        let across_x = at.0 >= rect.x - tolerance && at.0 <= rect.right() + tolerance;
        let across_y = at.1 >= rect.y - tolerance && at.1 <= rect.bottom() + tolerance;
        let (w, e) = (near(at.0, rect.x), near(at.0, rect.right()));
        let (n, s) = (near(at.1, rect.y), near(at.1, rect.bottom()));
        match (n, s, w, e) {
            (true, _, true, _) => Some(Handle::NW),
            (true, _, _, true) => Some(Handle::NE),
            (_, true, true, _) => Some(Handle::SW),
            (_, true, _, true) => Some(Handle::SE),
            (true, _, _, _) if across_x => Some(Handle::N),
            (_, true, _, _) if across_x => Some(Handle::S),
            (_, _, true, _) if across_y => Some(Handle::W),
            (_, _, _, true) if across_y => Some(Handle::E),
            _ if rect.contains(at.0, at.1) => Some(Handle::Move),
            _ => None,
        }
    }

    /// Moves the rect, or just `edge` (a corner moves both of its edges).
    /// Width and height stay at least 1.
    pub fn nudge(&mut self, dx: f32, dy: f32, edge: Option<Handle>) {
        let Some(rect) = &mut self.rect else { return };
        let Some(h) = edge.filter(|h| *h != Handle::Move) else {
            rect.x += dx;
            rect.y += dy;
            return;
        };
        let top = matches!(h, Handle::N | Handle::NE | Handle::NW);
        let bottom = matches!(h, Handle::S | Handle::SE | Handle::SW);
        let left = matches!(h, Handle::W | Handle::NW | Handle::SW);
        let right = matches!(h, Handle::E | Handle::NE | Handle::SE);
        if right {
            rect.w += dx.max(1.0 - rect.w);
        }
        if left {
            let dx = dx.min(rect.w - 1.0);
            rect.x += dx;
            rect.w -= dx;
        }
        if bottom {
            rect.h += dy.max(1.0 - rect.h);
        }
        if top {
            let dy = dy.min(rect.h - 1.0);
            rect.y += dy;
            rect.h -= dy;
        }
    }

    pub fn select_all(&mut self, bounds: DRect) {
        self.rect = Some(bounds);
    }

    /// A Window or Full screen pick.
    pub fn set_rect(&mut self, rect: DRect) {
        self.rect = Some(rect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> DRect {
        DRect { x, y, w, h }
    }

    fn desktop() -> DRect {
        r(0.0, 0.0, 1000.0, 800.0)
    }

    fn drawn(sel: &mut Selection, from: (f32, f32), to: (f32, f32), bounds: DRect) {
        sel.press(from, 6.0);
        sel.drag_to(to, bounds);
        sel.release();
    }

    #[test]
    fn dragging_up_and_left_normalises() {
        let mut sel = Selection::default();
        drawn(&mut sel, (100.0, 100.0), (40.0, 30.0), desktop());
        assert_eq!(sel.rect, Some(r(40.0, 30.0, 60.0, 70.0)));
    }

    #[test]
    fn a_click_without_a_drag_selects_nothing() {
        let mut sel = Selection::default();
        sel.press((50.0, 50.0), 6.0);
        sel.release();
        assert_eq!(sel.rect, None);
        drawn(&mut sel, (50.0, 50.0), (51.0, 52.0), desktop());
        assert_eq!(sel.rect, None);
    }

    #[test]
    fn a_tiny_new_drag_keeps_the_previous_rect() {
        let mut sel = Selection::default();
        drawn(&mut sel, (100.0, 100.0), (200.0, 200.0), desktop());
        drawn(&mut sel, (500.0, 500.0), (501.0, 501.0), desktop());
        assert_eq!(sel.rect, Some(r(100.0, 100.0, 100.0, 100.0)));
    }

    #[test]
    fn a_drag_continues_across_a_negative_monitor() {
        let bounds = r(-1920.0, 0.0, 3840.0, 1080.0);
        let mut sel = Selection::default();
        drawn(&mut sel, (-100.0, 10.0), (100.0, 50.0), bounds);
        assert_eq!(sel.rect, Some(r(-100.0, 10.0, 200.0, 40.0)));
    }

    #[test]
    fn dragging_is_clamped_to_the_desktop() {
        let mut sel = Selection::default();
        drawn(&mut sel, (100.0, 100.0), (99999.0, 99999.0), desktop());
        assert_eq!(sel.rect, Some(r(100.0, 100.0, 900.0, 700.0)));
    }

    #[test]
    fn resizing_past_the_opposite_edge_flips() {
        let mut sel = Selection::default();
        drawn(&mut sel, (100.0, 100.0), (200.0, 200.0), desktop());
        assert_eq!(sel.handle_at((200.0, 200.0), 6.0), Some(Handle::SE));
        sel.press((200.0, 200.0), 6.0);
        sel.drag_to((50.0, 300.0), desktop());
        sel.release();
        assert_eq!(sel.rect, Some(r(50.0, 100.0, 50.0, 200.0)));
    }

    #[test]
    fn handles_prefer_corners_then_edges_then_move() {
        let mut sel = Selection::default();
        sel.set_rect(r(100.0, 100.0, 200.0, 100.0));
        assert_eq!(sel.handle_at((102.0, 98.0), 6.0), Some(Handle::NW));
        assert_eq!(sel.handle_at((300.0, 100.0), 6.0), Some(Handle::NE));
        assert_eq!(sel.handle_at((100.0, 200.0), 6.0), Some(Handle::SW));
        assert_eq!(sel.handle_at((200.0, 98.0), 6.0), Some(Handle::N));
        assert_eq!(sel.handle_at((200.0, 203.0), 6.0), Some(Handle::S));
        assert_eq!(sel.handle_at((98.0, 150.0), 6.0), Some(Handle::W));
        assert_eq!(sel.handle_at((301.0, 150.0), 6.0), Some(Handle::E));
        assert_eq!(sel.handle_at((200.0, 150.0), 6.0), Some(Handle::Move));
        assert_eq!(sel.handle_at((500.0, 500.0), 6.0), None);
    }

    #[test]
    fn moving_keeps_the_rect_inside_the_desktop() {
        let mut sel = Selection::default();
        sel.set_rect(r(100.0, 100.0, 200.0, 100.0));
        sel.press((200.0, 150.0), 6.0);
        sel.drag_to((260.0, 170.0), desktop());
        assert_eq!(sel.rect, Some(r(160.0, 120.0, 200.0, 100.0)));
        sel.drag_to((99999.0, 99999.0), desktop());
        assert_eq!(sel.rect, Some(r(800.0, 700.0, 200.0, 100.0)));
        sel.release();
    }

    #[test]
    fn pressing_outside_starts_a_new_drag() {
        let mut sel = Selection::default();
        sel.set_rect(r(100.0, 100.0, 50.0, 50.0));
        drawn(&mut sel, (400.0, 400.0), (500.0, 450.0), desktop());
        assert_eq!(sel.rect, Some(r(400.0, 400.0, 100.0, 50.0)));
    }

    #[test]
    fn nudge_moves_one_unit_or_one_edge() {
        let mut sel = Selection::default();
        sel.set_rect(r(10.0, 10.0, 20.0, 20.0));
        sel.nudge(1.0, 0.0, None);
        assert_eq!(sel.rect, Some(r(11.0, 10.0, 20.0, 20.0)));
        sel.nudge(1.0, 0.0, Some(Handle::E));
        assert_eq!(sel.rect, Some(r(11.0, 10.0, 21.0, 20.0)));
        sel.nudge(1.0, 0.0, Some(Handle::W));
        assert_eq!(sel.rect, Some(r(12.0, 10.0, 20.0, 20.0)));
        sel.nudge(0.0, -2.0, Some(Handle::N));
        assert_eq!(sel.rect, Some(r(12.0, 8.0, 20.0, 22.0)));
        sel.nudge(0.0, -1.0, Some(Handle::S));
        assert_eq!(sel.rect, Some(r(12.0, 8.0, 20.0, 21.0)));
    }

    #[test]
    fn nudge_never_collapses_the_rect() {
        let mut sel = Selection::default();
        sel.set_rect(r(10.0, 10.0, 5.0, 5.0));
        sel.nudge(-50.0, 0.0, Some(Handle::E));
        assert_eq!(sel.rect, Some(r(10.0, 10.0, 1.0, 5.0)));
        sel.nudge(50.0, 0.0, Some(Handle::W));
        assert_eq!(sel.rect, Some(r(10.0, 10.0, 1.0, 5.0)));
    }

    #[test]
    fn select_all_is_the_desktop() {
        let mut sel = Selection::default();
        let bounds = r(-1920.0, 0.0, 3840.0, 1080.0);
        sel.select_all(bounds);
        assert_eq!(sel.rect, Some(bounds));
    }
}
