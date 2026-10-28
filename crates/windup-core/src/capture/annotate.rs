//! Annotations as objects: shapes placed on the screenshot that stay editable
//! (moved, resized, recoloured, deleted) until the picture is rendered.
//!
//! Everything is in desktop units, pinned to the screen rather than to the
//! selection. History is a stack of whole-list snapshots: annotations are few
//! and small, and a snapshot cannot get out of step with the list it restores.

use super::geometry::DRect;
use super::selection::Handle;

/// Smallest width or height a resize may leave a shape with.
const MIN_SIZE: f32 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Pen,
    Rect,
    Ellipse,
    Blur,
    Mosaic,
    Pipette,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Pen(Vec<(f32, f32)>),
    Rect(DRect),
    Ellipse(DRect),
    Blur(DRect),
    Mosaic(DRect),
}

impl Shape {
    pub fn tool(&self) -> Tool {
        match self {
            Shape::Pen(_) => Tool::Pen,
            Shape::Rect(_) => Tool::Rect,
            Shape::Ellipse(_) => Tool::Ellipse,
            Shape::Blur(_) => Tool::Blur,
            Shape::Mosaic(_) => Tool::Mosaic,
        }
    }

    /// The box around the shape (a stroke's points, not its thickness).
    pub fn bounds(&self) -> DRect {
        match self {
            Shape::Pen(points) => {
                let Some(&first) = points.first() else {
                    return DRect {
                        x: 0.0,
                        y: 0.0,
                        w: 0.0,
                        h: 0.0,
                    };
                };
                let (mut lo, mut hi) = (first, first);
                for &(x, y) in points {
                    lo = (lo.0.min(x), lo.1.min(y));
                    hi = (hi.0.max(x), hi.1.max(y));
                }
                DRect::from_corners(lo, hi)
            }
            Shape::Rect(r) | Shape::Ellipse(r) | Shape::Blur(r) | Shape::Mosaic(r) => *r,
        }
    }

    /// Fits the shape into `to`; a stroke's points are mapped proportionally.
    fn fit(&mut self, from: DRect, to: DRect) {
        match self {
            Shape::Pen(points) => {
                // A flat stroke has no extent to scale on that axis: offset only.
                let sx = (from.w > f32::EPSILON).then(|| to.w / from.w);
                let sy = (from.h > f32::EPSILON).then(|| to.h / from.h);
                for p in points {
                    p.0 = to.x + sx.map_or(p.0 - from.x, |s| (p.0 - from.x) * s);
                    p.1 = to.y + sy.map_or(p.1 - from.y, |s| (p.1 - from.y) * s);
                }
            }
            Shape::Rect(r) | Shape::Ellipse(r) | Shape::Blur(r) | Shape::Mosaic(r) => *r = to,
        }
    }

    /// Distance from `at` to the part of the shape a click should grab, or
    /// `None` for shapes that are grabbed anywhere inside.
    fn outline_distance(&self, at: (f32, f32)) -> Option<f32> {
        match self {
            Shape::Pen(points) => Some(match points.as_slice() {
                [] => f32::INFINITY,
                [p] => dist(at, *p),
                _ => points
                    .windows(2)
                    .map(|w| segment_distance(at, w[0], w[1]))
                    .fold(f32::INFINITY, f32::min),
            }),
            Shape::Rect(r) => Some(rect_outline_distance(at, r.normalised())),
            Shape::Ellipse(r) => Some(ellipse_outline_distance(at, r.normalised())),
            Shape::Blur(_) | Shape::Mosaic(_) => None,
        }
    }
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn segment_distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    if len2 <= f32::EPSILON {
        return dist(p, a);
    }
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0);
    dist(p, (a.0 + t * dx, a.1 + t * dy))
}

fn rect_outline_distance(p: (f32, f32), r: DRect) -> f32 {
    let dx = (r.x - p.0).max(p.0 - r.right()).max(0.0);
    let dy = (r.y - p.1).max(p.1 - r.bottom()).max(0.0);
    if dx > 0.0 || dy > 0.0 {
        return dx.hypot(dy);
    }
    // Inside: the nearest edge.
    (p.0 - r.x)
        .min(r.right() - p.0)
        .min(p.1 - r.y)
        .min(r.bottom() - p.1)
}

/// First-order distance to the curve: |f| / |grad f| for the implicit
/// ellipse f = (x/a)^2 + (y/b)^2 - 1. Exact enough near the curve, which is
/// the only place a click tolerance cares about.
fn ellipse_outline_distance(p: (f32, f32), r: DRect) -> f32 {
    let (a, b) = ((r.w / 2.0).max(f32::EPSILON), (r.h / 2.0).max(f32::EPSILON));
    let (x, y) = (p.0 - (r.x + a), p.1 - (r.y + b));
    let f = (x / a).powi(2) + (y / b).powi(2) - 1.0;
    let grad = 2.0 * ((x / (a * a)).powi(2) + (y / (b * b)).powi(2)).sqrt();
    if grad <= f32::EPSILON {
        return a.min(b); // the centre
    }
    f.abs() / grad
}

#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub id: u64,
    pub shape: Shape,
    pub colour: [u8; 4],
    /// Stroke width, blur radius or mosaic block, in desktop units.
    pub size: f32,
}

/// A press selects when the Select tool hits anything, or when a drawing tool
/// hits an annotation that tool made.
pub fn press_selects(tool: Tool, hit: Option<&Annotation>) -> bool {
    hit.is_some_and(|a| tool == Tool::Select || a.shape.tool() == tool)
}

struct Gesture {
    id: u64,
    handle: Handle,
    grab: (f32, f32),
    orig: Annotation,
    /// The list before the gesture, pushed as one step if it changed anything.
    before: Vec<Annotation>,
}

#[derive(Default)]
pub struct Doc {
    pub items: Vec<Annotation>,
    pub selected: Option<u64>,
    next_id: u64,
    undo: Vec<Vec<Annotation>>,
    redo: Vec<Vec<Annotation>>,
    gesture: Option<Gesture>,
}

impl Doc {
    /// Records the current list as an undo step and drops any redo.
    fn checkpoint(&mut self) {
        self.undo.push(self.items.clone());
        self.redo.clear();
    }

    /// Appends on top, assigning a fresh id (the one in `a` is ignored).
    pub fn add(&mut self, mut a: Annotation) -> u64 {
        self.checkpoint();
        self.next_id += 1;
        a.id = self.next_id;
        self.items.push(a);
        self.next_id
    }

    pub fn get(&self, id: u64) -> Option<&Annotation> {
        self.items.iter().find(|a| a.id == id)
    }

    /// The topmost annotation within reach of `at`.
    pub fn hit(&self, at: (f32, f32), tolerance: f32) -> Option<u64> {
        self.items
            .iter()
            .rev()
            .find(|a| match a.shape.outline_distance(at) {
                Some(d) => d <= a.size / 2.0 + tolerance,
                None => {
                    let b = a.shape.bounds().normalised();
                    at.0 >= b.x && at.0 <= b.right() && at.1 >= b.y && at.1 <= b.bottom()
                }
            })
            .map(|a| a.id)
    }

    pub fn begin_transform(&mut self, id: u64, handle: Handle, at: (f32, f32)) {
        self.gesture = self.get(id).map(|a| Gesture {
            id,
            handle,
            grab: at,
            orig: a.clone(),
            before: self.items.clone(),
        });
    }

    pub fn transform_to(&mut self, at: (f32, f32), keep_aspect: bool) {
        let Some(g) = &self.gesture else { return };
        let from = g.orig.shape.bounds();
        let d = (at.0 - g.grab.0, at.1 - g.grab.1);
        let to = transformed_bounds(from, g.handle, d, keep_aspect);
        let mut shape = g.orig.shape.clone();
        shape.fit(from, to);
        let id = g.id;
        if let Some(a) = self.items.iter_mut().find(|a| a.id == id) {
            a.shape = shape;
        }
    }

    pub fn end_transform(&mut self) {
        let Some(g) = self.gesture.take() else { return };
        if g.before != self.items {
            self.undo.push(g.before);
            self.redo.clear();
        }
    }

    pub fn set_colour(&mut self, id: u64, colour: [u8; 4]) {
        if self.get(id).is_some_and(|a| a.colour != colour) {
            self.checkpoint();
            if let Some(a) = self.items.iter_mut().find(|a| a.id == id) {
                a.colour = colour;
            }
        }
    }

    pub fn set_size(&mut self, id: u64, size: f32) {
        if self.get(id).is_some_and(|a| a.size != size) {
            self.checkpoint();
            if let Some(a) = self.items.iter_mut().find(|a| a.id == id) {
                a.size = size;
            }
        }
    }

    pub fn delete(&mut self, id: u64) {
        if self.get(id).is_none() {
            return;
        }
        self.checkpoint();
        self.items.retain(|a| a.id != id);
        if self.selected == Some(id) {
            self.selected = None;
        }
    }

    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        self.redo.push(std::mem::replace(&mut self.items, prev));
        self.drop_stale_selection();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(&mut self.items, next));
        self.drop_stale_selection();
        true
    }

    fn drop_stale_selection(&mut self) {
        if self.selected.is_some_and(|id| self.get(id).is_none()) {
            self.selected = None;
        }
    }
}

/// `from` after dragging `handle` by `d`. Resizing anchors the opposite edge
/// or corner; dragging past it flips, and each side stays at least `MIN_SIZE`.
fn transformed_bounds(from: DRect, handle: Handle, d: (f32, f32), keep_aspect: bool) -> DRect {
    let from = from.normalised();
    if handle == Handle::Move {
        return DRect {
            x: from.x + d.0,
            y: from.y + d.1,
            ..from
        };
    }
    let west = matches!(handle, Handle::W | Handle::NW | Handle::SW);
    let east = matches!(handle, Handle::E | Handle::NE | Handle::SE);
    let north = matches!(handle, Handle::N | Handle::NW | Handle::NE);
    let south = matches!(handle, Handle::S | Handle::SW | Handle::SE);
    let (mut l, mut t, mut r, mut b) = (from.x, from.y, from.right(), from.bottom());
    if west {
        l += d.0;
    }
    if east {
        r += d.0;
    }
    if north {
        t += d.1;
    }
    if south {
        b += d.1;
    }
    if keep_aspect && (west || east) && (north || south) && from.w > 0.0 && from.h > 0.0 {
        // The corner that stayed put, and the signed offset of the one dragged.
        let (ax, ay) = (if west { r } else { l }, if north { b } else { t });
        let (dx, dy) = (
            if west { l - ax } else { r - ax },
            if north { t - ay } else { b - ay },
        );
        let scale = (dx.abs() / from.w).max(dy.abs() / from.h);
        let sign = |v: f32, orig_negative: bool| match v {
            v if v < 0.0 => -1.0,
            v if v > 0.0 => 1.0,
            _ if orig_negative => -1.0,
            _ => 1.0,
        };
        let nx = ax + sign(dx, west) * from.w * scale;
        let ny = ay + sign(dy, north) * from.h * scale;
        (l, r) = (ax.min(nx), ax.max(nx));
        (t, b) = (ay.min(ny), ay.max(ny));
    }
    let mut out = DRect::from_corners((l, t), (r, b));
    out.w = out.w.max(MIN_SIZE);
    out.h = out.h.max(MIN_SIZE);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_ann(x: f32, y: f32, w: f32, h: f32) -> Annotation {
        Annotation {
            id: 0,
            shape: Shape::Rect(DRect { x, y, w, h }),
            colour: [255, 0, 0, 255],
            size: 4.0,
        }
    }

    fn pen_ann(points: Vec<(f32, f32)>) -> Annotation {
        Annotation {
            id: 0,
            shape: Shape::Pen(points),
            colour: [0, 0, 0, 255],
            size: 4.0,
        }
    }

    fn mosaic_ann(x: f32, y: f32, w: f32, h: f32) -> Annotation {
        Annotation {
            id: 0,
            shape: Shape::Mosaic(DRect { x, y, w, h }),
            colour: [0, 0, 0, 255],
            size: 8.0,
        }
    }

    #[test]
    fn the_topmost_annotation_is_hit() {
        let mut doc = Doc::default();
        let below = doc.add(mosaic_ann(0.0, 0.0, 100.0, 100.0));
        let above = doc.add(mosaic_ann(50.0, 50.0, 100.0, 100.0));
        assert_eq!(doc.hit((60.0, 60.0), 3.0), Some(above));
        assert_eq!(doc.hit((10.0, 10.0), 3.0), Some(below));
        assert_eq!(doc.hit((300.0, 300.0), 3.0), None);
    }

    #[test]
    fn a_pen_stroke_is_hit_near_its_line_only() {
        let mut doc = Doc::default();
        let id = doc.add(pen_ann(vec![(0.0, 0.0), (100.0, 0.0)]));
        // size 4 -> reach 2 + tolerance 3
        assert_eq!(doc.hit((50.0, 4.0), 3.0), Some(id));
        assert_eq!(doc.hit((50.0, 6.0), 3.0), None);
        assert_eq!(doc.hit((103.0, 0.0), 3.0), Some(id));
        assert_eq!(doc.hit((106.0, 0.0), 3.0), None);
    }

    #[test]
    fn rect_and_ellipse_hit_near_their_outline_only() {
        let mut doc = Doc::default();
        let r = doc.add(rect_ann(0.0, 0.0, 100.0, 100.0));
        assert_eq!(doc.hit((50.0, 1.0), 3.0), Some(r));
        assert_eq!(doc.hit((50.0, 50.0), 3.0), None);
        doc.delete(r);
        let e = doc.add(Annotation {
            id: 0,
            shape: Shape::Ellipse(DRect {
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 60.0,
            }),
            colour: [0, 0, 0, 255],
            size: 4.0,
        });
        assert_eq!(doc.hit((100.0, 30.0), 3.0), Some(e));
        assert_eq!(doc.hit((50.0, 0.0), 3.0), Some(e));
        assert_eq!(doc.hit((50.0, 30.0), 3.0), None);
        assert_eq!(doc.hit((2.0, 2.0), 3.0), None); // bounding-box corner, off the curve
    }

    #[test]
    fn moving_then_undo_restores_the_position_in_one_step() {
        let mut doc = Doc::default();
        let id = doc.add(rect_ann(10.0, 20.0, 40.0, 30.0));
        doc.begin_transform(id, Handle::Move, (15.0, 25.0));
        doc.transform_to((20.0, 28.0), false);
        doc.transform_to((25.0, 30.0), false);
        doc.end_transform();
        let b = doc.items[0].shape.bounds();
        assert_eq!((b.x, b.y, b.w, b.h), (20.0, 25.0, 40.0, 30.0));
        assert!(doc.undo());
        let b = doc.items[0].shape.bounds();
        assert_eq!((b.x, b.y), (10.0, 20.0));
        assert!(doc.redo());
        assert_eq!(doc.items[0].shape.bounds().x, 20.0);
        assert!(doc.undo());
        assert!(doc.undo()); // the add
        assert!(doc.items.is_empty());
        assert!(!doc.undo());
    }

    #[test]
    fn a_transform_that_changes_nothing_records_no_step() {
        let mut doc = Doc::default();
        let id = doc.add(rect_ann(0.0, 0.0, 10.0, 10.0));
        doc.begin_transform(id, Handle::Move, (5.0, 5.0));
        doc.transform_to((9.0, 9.0), false);
        doc.transform_to((5.0, 5.0), false);
        doc.end_transform();
        assert!(doc.undo()); // the add
        assert!(!doc.undo());
    }

    #[test]
    fn resizing_a_pen_stroke_scales_its_points() {
        let mut doc = Doc::default();
        let id = doc.add(pen_ann(vec![(0.0, 0.0), (10.0, 10.0), (20.0, 0.0)]));
        doc.begin_transform(id, Handle::SE, (20.0, 10.0));
        doc.transform_to((40.0, 30.0), false);
        doc.end_transform();
        let Shape::Pen(p) = &doc.items[0].shape else {
            panic!()
        };
        assert_eq!(p, &vec![(0.0, 0.0), (20.0, 30.0), (40.0, 0.0)]);
    }

    #[test]
    fn a_straight_line_stroke_resizes_without_dividing_by_zero() {
        let mut doc = Doc::default();
        let id = doc.add(pen_ann(vec![(0.0, 5.0), (10.0, 5.0)]));
        doc.begin_transform(id, Handle::E, (10.0, 5.0));
        doc.transform_to((30.0, 5.0), false);
        doc.end_transform();
        let Shape::Pen(p) = &doc.items[0].shape else {
            panic!()
        };
        assert_eq!(p, &vec![(0.0, 5.0), (30.0, 5.0)]);
        assert!(p.iter().all(|q| q.0.is_finite() && q.1.is_finite()));
    }

    #[test]
    fn shift_keeps_proportions_when_resizing() {
        let mut doc = Doc::default();
        let id = doc.add(rect_ann(0.0, 0.0, 100.0, 50.0));
        doc.begin_transform(id, Handle::SE, (100.0, 50.0));
        doc.transform_to((200.0, 60.0), true);
        let b = doc.items[0].shape.bounds();
        assert_eq!((b.x, b.y, b.w, b.h), (0.0, 0.0, 200.0, 100.0));
        doc.transform_to((200.0, 60.0), false);
        let b = doc.items[0].shape.bounds();
        assert_eq!((b.w, b.h), (200.0, 60.0));
        doc.end_transform();
    }

    #[test]
    fn dragging_past_the_opposite_edge_flips_and_keeps_a_minimum_size() {
        let mut doc = Doc::default();
        let id = doc.add(rect_ann(10.0, 10.0, 20.0, 20.0));
        doc.begin_transform(id, Handle::E, (30.0, 20.0));
        doc.transform_to((0.0, 20.0), false);
        let b = doc.items[0].shape.bounds();
        assert_eq!((b.x, b.w), (0.0, 10.0));
        doc.transform_to((10.0, 20.0), false);
        assert!(doc.items[0].shape.bounds().w >= 1.0);
        doc.end_transform();
    }

    #[test]
    fn a_new_step_clears_redo() {
        let mut doc = Doc::default();
        let a = doc.add(rect_ann(0.0, 0.0, 10.0, 10.0));
        doc.set_colour(a, [1, 2, 3, 255]);
        assert!(doc.undo());
        assert_eq!(doc.items[0].colour, [255, 0, 0, 255]);
        doc.set_size(a, 9.0);
        assert!(!doc.redo());
        assert_eq!(doc.items[0].size, 9.0);
    }

    #[test]
    fn a_rect_tool_press_on_a_rect_selects_it_but_on_an_ellipse_does_not() {
        let r = rect_ann(0.0, 0.0, 10.0, 10.0);
        let e = Annotation {
            shape: Shape::Ellipse(DRect {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            }),
            ..r.clone()
        };
        assert!(press_selects(Tool::Rect, Some(&r)));
        assert!(!press_selects(Tool::Rect, Some(&e)));
        assert!(press_selects(Tool::Select, Some(&e)));
        assert!(!press_selects(Tool::Select, None));
        assert!(!press_selects(Tool::Rect, None));
    }

    #[test]
    fn delete_then_undo_brings_it_back_with_the_same_id() {
        let mut doc = Doc::default();
        let a = doc.add(rect_ann(0.0, 0.0, 10.0, 10.0));
        let b = doc.add(rect_ann(5.0, 5.0, 10.0, 10.0));
        let c = doc.add(rect_ann(9.0, 9.0, 10.0, 10.0));
        doc.selected = Some(b);
        doc.delete(b);
        assert_eq!(doc.selected, None);
        assert_eq!(doc.items.iter().map(|i| i.id).collect::<Vec<_>>(), [a, c]);
        assert!(doc.undo());
        assert_eq!(
            doc.items.iter().map(|i| i.id).collect::<Vec<_>>(),
            [a, b, c]
        );
        let d = doc.add(rect_ann(0.0, 0.0, 1.0, 1.0));
        assert!(d > c, "ids are never reused");
    }
}
