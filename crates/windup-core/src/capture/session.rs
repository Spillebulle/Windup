//! The capture overlay as a state machine, from the moment the screen is
//! frozen until the result is copied, saved or cancelled.
//!
//! Pure: no egui, no clock, no I/O. Every position is in desktop units and
//! every time is passed in, so the view only converts pointer positions,
//! forwards events and paints what is here.

use image::RgbaImage;

use super::annotate::{Annotation, Doc, Shape, Tool, press_selects};
use super::geometry::{DRect, DesktopShot};
use super::pick::{Tween, WindowRect, edge_rect, window_at};
use super::raster::render;
use super::selection::{Handle, MIN_DRAG, Selection};
use crate::config::{ScreenshotConfig, TargetKind, ToolSizes};

/// How near the pointer must be to a handle or an outline, in desktop units.
pub const HANDLE_TOLERANCE: f32 = 6.0;
/// How long the size figure by the pointer stays, in seconds.
pub const SIZE_FLASH_SECS: f64 = 0.8;
/// One wheel step changes a size by this much.
pub const SIZE_STEP: f32 = 1.0;
pub const MIN_TOOL_SIZE: f32 = 1.0;
pub const MAX_TOOL_SIZE: f32 = 64.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureMode {
    Screenshot,
    Video,
    Steps,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Choosing,
    Editing,
}

#[derive(Debug)]
pub enum Outcome {
    Copy(RgbaImage),
    Save(RgbaImage),
    CopyAndSave(RgbaImage),
    Cancel,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Cancel,
    Copy,
    Save,
    Undo,
    Redo,
    ToggleLoupe,
    SelectAll,
    Nudge { dx: f32, dy: f32 },
    Delete,
    SetTool(Tool),
    SetTarget(TargetKind),
    SetMode(CaptureMode),
    SetColour([u8; 4]),
}

/// What the held mouse button is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grab {
    Nothing,
    Selection,
    Transform,
    Draw,
}

pub struct Session {
    pub shot: DesktopShot,
    pub windows: Vec<WindowRect>,
    pub mode: CaptureMode,
    pub target: TargetKind,
    pub stage: Stage,
    pub selection: Selection,
    pub doc: Doc,
    pub tool: Tool,
    /// The tool the pipette returns to.
    pub prev_tool: Tool,
    pub colour: [u8; 4],
    pub sizes: ToolSizes,
    pub loupe: bool,
    /// The highlight in Window and Full screen while choosing.
    pub hover: Option<Tween>,
    /// The pointer in desktop units, for the loupe and the size figure.
    pub pointer: Option<[f32; 2]>,
    /// The annotation being drawn right now, not yet in `doc`.
    pub draft: Option<Annotation>,
    /// A size to show by the pointer and the time it stops showing.
    pub size_flash: Option<(f32, f64)>,
    hover_target: Option<DRect>,
    also_save_on_copy: bool,
    grab: Grab,
    /// The selection edge last grabbed, which the arrow keys then move.
    edge: Option<Handle>,
    draw_start: (f32, f32),
}

impl Session {
    pub fn new(shot: DesktopShot, windows: Vec<WindowRect>, cfg: &ScreenshotConfig) -> Session {
        let mut selection = Selection::default();
        selection.kind = cfg.last_target;
        Session {
            shot,
            windows,
            mode: CaptureMode::Screenshot,
            target: cfg.last_target,
            stage: Stage::Choosing,
            selection,
            doc: Doc::default(),
            tool: Tool::Select,
            prev_tool: Tool::Select,
            colour: cfg.colour,
            sizes: cfg.sizes.clone(),
            loupe: cfg.loupe_on,
            hover: None,
            pointer: None,
            draft: None,
            size_flash: None,
            hover_target: None,
            also_save_on_copy: cfg.also_save_on_copy,
            grab: Grab::Nothing,
            edge: None,
            draw_start: (0.0, 0.0),
        }
    }

    /// The picture of the selection with the annotations on it. `Cancel` when
    /// neither is wanted or there is nothing to render.
    pub fn finish(&self, copy: bool, save: bool) -> Outcome {
        if !copy && !save {
            return Outcome::Cancel;
        }
        let Some(img) = self
            .selection
            .rect
            .and_then(|r| render(&self.shot, r, &self.doc))
        else {
            return Outcome::Cancel;
        };
        match (copy, save) {
            (true, true) => Outcome::CopyAndSave(img),
            (true, false) => Outcome::Copy(img),
            _ => Outcome::Save(img),
        }
    }

    // --- pointer -----------------------------------------------------------

    pub fn pointer_moved(&mut self, p: [f32; 2], now: f64) {
        self.pointer = Some(p);
        if self.stage != Stage::Choosing {
            return;
        }
        let target = self.target_at((p[0], p[1]));
        if target == self.hover_target {
            return;
        }
        self.hover_target = target;
        self.hover = match (target, self.hover.take()) {
            (Some(to), Some(mut tween)) => {
                tween.retarget(to, now);
                Some(tween)
            }
            (Some(to), None) => Some(Tween::new(to, now)),
            (None, _) => None,
        };
    }

    pub fn press(&mut self, p: [f32; 2], _mods: Mods) {
        self.pointer = Some(p);
        let at = (p[0], p[1]);
        match self.stage {
            Stage::Choosing => {
                if self.target == TargetKind::Area {
                    self.selection.press(at, HANDLE_TOLERANCE);
                    self.grab = Grab::Selection;
                }
            }
            Stage::Editing => self.press_editing(at),
        }
    }

    pub fn drag(&mut self, p: [f32; 2], mods: Mods) {
        self.pointer = Some(p);
        let at = (p[0], p[1]);
        match self.grab {
            Grab::Nothing => {}
            Grab::Selection => self.selection.drag_to(at, self.shot.bounds()),
            Grab::Transform => self.doc.transform_to(at, mods.shift),
            Grab::Draw => self.extend_draft(at, mods.shift),
        }
    }

    /// Always `None` today; kept so a release can end the session later
    /// without the view changing.
    pub fn release(&mut self, p: [f32; 2], _mods: Mods) -> Option<Outcome> {
        self.pointer = Some(p);
        let grab = std::mem::replace(&mut self.grab, Grab::Nothing);
        match grab {
            Grab::Selection => {
                self.selection.release();
                if self.stage == Stage::Choosing && self.has_selection() {
                    self.enter_editing();
                }
            }
            Grab::Transform => self.doc.end_transform(),
            Grab::Draw => self.commit_draft(),
            Grab::Nothing => {
                if self.stage == Stage::Choosing
                    && self.target != TargetKind::Area
                    && let Some(rect) = self.target_at((p[0], p[1]))
                {
                    self.selection.set_rect(rect);
                    self.enter_editing();
                }
            }
        }
        None
    }

    /// Wheel steps (positive grows). Resizes the selected annotation as one
    /// undoable step, else the active tool.
    pub fn scroll(&mut self, steps: f32, now: f64) {
        if self.stage != Stage::Editing || steps == 0.0 {
            return;
        }
        let delta = steps * SIZE_STEP;
        let size = if let Some(id) = self.doc.selected
            && let Some(current) = self.doc.get(id).map(|a| a.size)
        {
            let size = (current + delta).clamp(MIN_TOOL_SIZE, MAX_TOOL_SIZE);
            self.settle();
            self.doc.set_size(id, size);
            size
        } else if let Some(current) = self.tool_size(self.tool) {
            let size = (current + delta).clamp(MIN_TOOL_SIZE, MAX_TOOL_SIZE);
            self.set_tool_size(self.tool, size);
            size
        } else {
            return;
        };
        self.size_flash = Some((size, now + SIZE_FLASH_SECS));
    }

    /// Whether an edge or corner of the selection is being dragged while
    /// editing: the one editing gesture the loupe helps aim, besides the
    /// pipette. Moving the whole selection is not one.
    pub fn adjusting_edge(&self) -> bool {
        self.stage == Stage::Editing && self.grab == Grab::Selection && self.edge.is_some()
    }

    // --- actions -----------------------------------------------------------

    pub fn act(&mut self, a: Action) -> Option<Outcome> {
        let editing = self.stage == Stage::Editing;
        match a {
            Action::Cancel => return Some(Outcome::Cancel),
            Action::Copy | Action::Save if !editing => {}
            Action::Copy => return self.result(true, self.also_save_on_copy),
            Action::Save => return self.result(false, true),
            Action::Undo => {
                self.settle();
                self.doc.undo();
            }
            Action::Redo => {
                self.settle();
                self.doc.redo();
            }
            Action::ToggleLoupe => self.loupe = !self.loupe,
            Action::SelectAll => {
                if self.target == TargetKind::Area && self.grab == Grab::Nothing {
                    self.selection.select_all(self.shot.bounds());
                    self.enter_editing();
                }
            }
            Action::Nudge { dx, dy } if editing => self.selection.nudge(dx, dy, self.edge),
            Action::Delete if editing => {
                if let Some(id) = self.doc.selected {
                    self.settle();
                    self.doc.delete(id);
                }
            }
            Action::SetTool(tool) => {
                if tool == Tool::Pipette && self.tool != Tool::Pipette {
                    self.prev_tool = self.tool;
                }
                self.tool = tool;
            }
            Action::SetTarget(kind) if !editing => {
                self.target = kind;
                self.selection.kind = kind;
                self.hover = None;
                self.hover_target = None;
            }
            // Video and Steps arrive in a later version.
            Action::SetMode(mode) => {
                if mode == CaptureMode::Screenshot {
                    self.mode = mode;
                }
            }
            Action::SetColour(colour) => self.set_colour(colour),
            Action::Nudge { .. } | Action::Delete | Action::SetTarget(_) => {}
        }
        None
    }

    // --- internals ---------------------------------------------------------

    fn result(&self, copy: bool, save: bool) -> Option<Outcome> {
        match self.finish(copy, save) {
            Outcome::Cancel => None,
            done => Some(done),
        }
    }

    fn has_selection(&self) -> bool {
        self.selection
            .rect
            .is_some_and(|r| r.w >= MIN_DRAG && r.h >= MIN_DRAG)
    }

    fn enter_editing(&mut self) {
        self.stage = Stage::Editing;
        self.hover = None;
        self.hover_target = None;
    }

    /// End any open gesture or draft before the document changes under it.
    fn settle(&mut self) {
        self.doc.end_transform();
        self.draft = None;
        if matches!(self.grab, Grab::Transform | Grab::Draw) {
            self.grab = Grab::Nothing;
        }
    }

    fn set_colour(&mut self, colour: [u8; 4]) {
        self.settle();
        self.colour = colour;
        if let Some(id) = self.doc.selected {
            self.doc.set_colour(id, colour);
        }
    }

    /// What a click would pick at `at`, for the current target.
    fn target_at(&self, at: (f32, f32)) -> Option<DRect> {
        match self.target {
            TargetKind::Area => None,
            TargetKind::FullScreen => self.monitor_layout(at),
            TargetKind::Window if self.windows.is_empty() => {
                edge_rect(&self.shot, at).or_else(|| self.monitor_layout(at))
            }
            TargetKind::Window => window_at(&self.windows, at),
        }
    }

    fn monitor_layout(&self, at: (f32, f32)) -> Option<DRect> {
        let i = self.shot.monitor_at(at.0, at.1)?;
        Some(self.shot.monitors[i].layout)
    }

    fn tool_size(&self, tool: Tool) -> Option<f32> {
        match tool {
            Tool::Pen => Some(self.sizes.pen),
            Tool::Rect => Some(self.sizes.rect),
            Tool::Ellipse => Some(self.sizes.ellipse),
            Tool::Blur => Some(self.sizes.blur),
            Tool::Mosaic => Some(self.sizes.mosaic),
            Tool::Select | Tool::Pipette => None,
        }
    }

    fn set_tool_size(&mut self, tool: Tool, size: f32) {
        match tool {
            Tool::Pen => self.sizes.pen = size,
            Tool::Rect => self.sizes.rect = size,
            Tool::Ellipse => self.sizes.ellipse = size,
            Tool::Blur => self.sizes.blur = size,
            Tool::Mosaic => self.sizes.mosaic = size,
            Tool::Select | Tool::Pipette => {}
        }
    }

    /// The frozen pixel under `at`, sampled at its monitor's own scale.
    fn sample(&self, at: (f32, f32)) -> Option<[u8; 4]> {
        let m = &self.shot.monitors[self.shot.monitor_at(at.0, at.1)?];
        let ppu = m.px_per_unit();
        let x = (((at.0 - m.layout.x) * ppu) as u32).min(m.image.width().saturating_sub(1));
        let y = (((at.1 - m.layout.y) * ppu) as u32).min(m.image.height().saturating_sub(1));
        let [r, g, b, _] = m.image.get_pixel(x, y).0;
        Some([r, g, b, 255])
    }

    fn press_editing(&mut self, at: (f32, f32)) {
        if self.tool == Tool::Pipette {
            if let Some(colour) = self.sample(at) {
                self.set_colour(colour);
                self.tool = self.prev_tool;
            }
            return;
        }
        // The transform box of the selected annotation: its edges and corners.
        if let Some(id) = self.doc.selected
            && let Some(a) = self.doc.get(id)
        {
            let mut probe = Selection::default();
            probe.set_rect(a.shape.bounds().normalised());
            if let Some(h) = probe.handle_at(at, HANDLE_TOLERANCE)
                && h != Handle::Move
            {
                self.doc.begin_transform(id, h, at);
                self.grab = Grab::Transform;
                return;
            }
        }
        // The tool that made an annotation, or Select, picks it up.
        if let Some(id) = self.doc.hit(at, HANDLE_TOLERANCE / 2.0)
            && press_selects(self.tool, self.doc.get(id))
        {
            self.doc.selected = Some(id);
            self.doc.begin_transform(id, Handle::Move, at);
            self.grab = Grab::Transform;
            return;
        }
        // The selection's own edges win over drawing, whatever the tool.
        let handle = self.selection.handle_at(at, HANDLE_TOLERANCE);
        if let Some(h) = handle
            && h != Handle::Move
        {
            self.edge = Some(h);
            self.selection.press(at, HANDLE_TOLERANCE);
            self.grab = Grab::Selection;
            return;
        }
        if self.tool == Tool::Select {
            self.doc.selected = None;
            if handle.is_some() {
                self.edge = None;
                self.selection.press(at, HANDLE_TOLERANCE);
                self.grab = Grab::Selection;
            }
            return;
        }
        self.doc.selected = None;
        self.start_draft(at);
    }

    fn start_draft(&mut self, at: (f32, f32)) {
        let tool = self.tool;
        let Some(size) = self.tool_size(tool) else {
            return;
        };
        let empty = DRect {
            x: at.0,
            y: at.1,
            w: 0.0,
            h: 0.0,
        };
        let shape = match tool {
            Tool::Pen => Shape::Pen(vec![at]),
            Tool::Rect => Shape::Rect(empty),
            Tool::Ellipse => Shape::Ellipse(empty),
            Tool::Blur => Shape::Blur(empty),
            Tool::Mosaic => Shape::Mosaic(empty),
            Tool::Select | Tool::Pipette => return,
        };
        self.draw_start = at;
        self.draft = Some(Annotation {
            id: 0,
            shape,
            colour: self.colour,
            size,
        });
        self.grab = Grab::Draw;
    }

    fn extend_draft(&mut self, at: (f32, f32), shift: bool) {
        let start = self.draw_start;
        let Some(d) = &mut self.draft else { return };
        let square = shift && matches!(d.shape, Shape::Rect(_) | Shape::Ellipse(_));
        match &mut d.shape {
            Shape::Pen(points) => {
                if points.last() != Some(&at) {
                    points.push(at);
                }
            }
            Shape::Rect(r) | Shape::Ellipse(r) | Shape::Blur(r) | Shape::Mosaic(r) => {
                *r = DRect::from_corners(start, if square { squared(start, at) } else { at });
            }
        }
    }

    fn commit_draft(&mut self) {
        let Some(d) = self.draft.take() else { return };
        let worth_keeping = match &d.shape {
            Shape::Pen(points) => points.len() >= 2,
            shape => {
                let b = shape.bounds();
                b.w >= MIN_DRAG && b.h >= MIN_DRAG
            }
        };
        if worth_keeping {
            self.doc.add(d);
        }
    }
}

/// The corner of the square (or circle's box) from `start` towards `at`.
fn squared(start: (f32, f32), at: (f32, f32)) -> (f32, f32) {
    let (dx, dy) = (at.0 - start.0, at.1 - start.1);
    let side = dx.abs().max(dy.abs());
    let sign = |v: f32| if v < 0.0 { -1.0 } else { 1.0 };
    (start.0 + sign(dx) * side, start.1 + sign(dy) * side)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::geometry::MonitorShot;
    use image::Rgba;

    const NONE: Mods = Mods {
        shift: false,
        ctrl: false,
    };

    fn rect(x: f32, y: f32, w: f32, h: f32) -> DRect {
        DRect { x, y, w, h }
    }

    /// Monitor 0 at the origin, 100x100 red; monitor 1 beside it, 100x100
    /// units at 2x, green.
    fn shot() -> DesktopShot {
        DesktopShot {
            monitors: vec![
                MonitorShot {
                    layout: rect(0.0, 0.0, 100.0, 100.0),
                    image: RgbaImage::from_pixel(100, 100, Rgba([200, 10, 10, 255])),
                    name: "a".into(),
                },
                MonitorShot {
                    layout: rect(100.0, 0.0, 100.0, 100.0),
                    image: RgbaImage::from_pixel(200, 200, Rgba([10, 200, 10, 255])),
                    name: "b".into(),
                },
            ],
        }
    }

    fn windows() -> Vec<WindowRect> {
        vec![
            WindowRect {
                rect: rect(10.0, 10.0, 40.0, 40.0),
                title: "top".into(),
            },
            WindowRect {
                rect: rect(0.0, 0.0, 100.0, 100.0),
                title: "below".into(),
            },
        ]
    }

    fn session(target: TargetKind) -> Session {
        let cfg = ScreenshotConfig {
            last_target: target,
            ..ScreenshotConfig::default()
        };
        Session::new(shot(), windows(), &cfg)
    }

    /// An Area session already in the editor with (10,10)-(90,90) selected.
    fn editing() -> Session {
        let mut s = session(TargetKind::Area);
        drag_area(&mut s, [10.0, 10.0], [90.0, 90.0]);
        assert_eq!(s.stage, Stage::Editing);
        s
    }

    fn drag_area(s: &mut Session, from: [f32; 2], to: [f32; 2]) {
        s.press(from, NONE);
        s.drag(to, NONE);
        s.release(to, NONE);
    }

    fn draw(s: &mut Session, tool: Tool, from: [f32; 2], to: [f32; 2], mods: Mods) {
        s.act(Action::SetTool(tool));
        s.press(from, mods);
        s.drag(to, mods);
        s.release(to, mods);
    }

    #[test]
    fn esc_cancels_in_every_stage() {
        let mut s = session(TargetKind::Area);
        assert!(matches!(s.act(Action::Cancel), Some(Outcome::Cancel)));
        let mut s = editing();
        assert!(matches!(s.act(Action::Cancel), Some(Outcome::Cancel)));
    }

    #[test]
    fn copy_with_also_save_is_copy_and_save() {
        let cfg = ScreenshotConfig {
            also_save_on_copy: true,
            ..ScreenshotConfig::default()
        };
        let mut s = Session::new(shot(), vec![], &cfg);
        drag_area(&mut s, [10.0, 10.0], [90.0, 90.0]);
        assert!(matches!(s.act(Action::Copy), Some(Outcome::CopyAndSave(_))));
        let mut plain = editing();
        assert!(matches!(plain.act(Action::Copy), Some(Outcome::Copy(_))));
        assert!(matches!(plain.act(Action::Save), Some(Outcome::Save(_))));
    }

    #[test]
    fn a_window_click_uses_the_hovered_rect() {
        let mut s = session(TargetKind::Window);
        s.pointer_moved([20.0, 20.0], 0.0);
        s.press([20.0, 20.0], NONE);
        s.release([20.0, 20.0], NONE);
        assert_eq!(s.stage, Stage::Editing);
        assert_eq!(s.selection.rect, Some(rect(10.0, 10.0, 40.0, 40.0)));
    }

    #[test]
    fn a_full_screen_click_selects_that_monitors_layout() {
        let mut s = session(TargetKind::FullScreen);
        s.pointer_moved([150.0, 50.0], 0.0);
        s.press([150.0, 50.0], NONE);
        s.release([150.0, 50.0], NONE);
        assert_eq!(s.stage, Stage::Editing);
        assert_eq!(s.selection.rect, Some(rect(100.0, 0.0, 100.0, 100.0)));
    }

    #[test]
    fn annotations_stay_put_when_the_selection_changes() {
        let mut s = editing();
        draw(&mut s, Tool::Rect, [20.0, 20.0], [60.0, 50.0], NONE);
        let before = s.doc.items.clone();
        assert_eq!(before.len(), 1);
        s.act(Action::SetTool(Tool::Select));
        s.act(Action::Nudge { dx: 5.0, dy: 7.0 });
        // Drag the selection's east edge, then move it from inside.
        s.press([95.0, 70.0], NONE);
        s.drag([70.0, 70.0], NONE);
        s.release([70.0, 70.0], NONE);
        s.press([30.0, 80.0], NONE);
        s.drag([40.0, 85.0], NONE);
        s.release([40.0, 85.0], NONE);
        assert_ne!(s.selection.rect, Some(rect(10.0, 10.0, 80.0, 80.0)));
        assert_eq!(s.doc.items, before);
    }

    #[test]
    fn an_area_drag_goes_to_editing_and_a_click_does_not() {
        let mut s = session(TargetKind::Area);
        s.press([10.0, 10.0], NONE);
        s.release([10.0, 10.0], NONE);
        assert_eq!(s.stage, Stage::Choosing);
        drag_area(&mut s, [10.0, 20.0], [60.0, 70.0]);
        assert_eq!(s.stage, Stage::Editing);
        assert_eq!(s.selection.rect, Some(rect(10.0, 20.0, 50.0, 50.0)));
    }

    #[test]
    fn adjusting_edge_is_only_an_edge_drag_while_editing() {
        let mut s = session(TargetKind::Area);
        s.press([10.0, 10.0], NONE);
        s.drag([90.0, 90.0], NONE);
        assert!(
            !s.adjusting_edge(),
            "the first drag is choosing, not editing"
        );
        s.release([90.0, 90.0], NONE);
        assert_eq!(s.stage, Stage::Editing);

        // The right edge of (10, 10)-(90, 90).
        s.press([90.0, 50.0], NONE);
        assert!(s.adjusting_edge());
        s.drag([95.0, 50.0], NONE);
        assert!(s.adjusting_edge());
        s.release([95.0, 50.0], NONE);
        assert!(!s.adjusting_edge(), "over once released");

        // Moving the whole selection with the select tool is not an edge.
        s.press([50.0, 50.0], NONE);
        assert!(!s.adjusting_edge());
        s.release([50.0, 50.0], NONE);
    }

    #[test]
    fn the_pipette_takes_the_frozen_pixel_and_returns_to_the_previous_tool() {
        let mut s = editing();
        s.act(Action::SetTool(Tool::Pen));
        s.act(Action::SetTool(Tool::Pipette));
        s.press([150.0, 50.0], NONE);
        s.release([150.0, 50.0], NONE);
        assert_eq!(s.colour, [10, 200, 10, 255]);
        assert_eq!(s.tool, Tool::Pen);
        assert!(s.doc.items.is_empty());
    }

    #[test]
    fn scrolling_a_selected_annotation_is_one_undoable_step() {
        let mut s = editing();
        draw(&mut s, Tool::Rect, [20.0, 20.0], [60.0, 50.0], NONE);
        // The rect tool picks up its own outline.
        s.press([40.0, 20.0], NONE);
        s.release([40.0, 20.0], NONE);
        assert_eq!(s.doc.selected, Some(s.doc.items[0].id));
        let before = s.doc.items[0].size;
        s.scroll(2.0, 1.0);
        let grown = before + 2.0 * SIZE_STEP;
        assert_eq!(s.doc.items[0].size, grown);
        assert_eq!(s.size_flash, Some((grown, 1.0 + SIZE_FLASH_SECS)));
        s.act(Action::Undo);
        assert_eq!(s.doc.items[0].size, before);
    }

    #[test]
    fn scrolling_without_a_selection_changes_the_tool_size() {
        let mut s = editing();
        s.act(Action::SetTool(Tool::Pen));
        let before = s.sizes.pen;
        s.scroll(-1.0, 0.0);
        assert_eq!(s.sizes.pen, before - SIZE_STEP);
    }

    #[test]
    fn undo_during_an_open_gesture_leaves_nothing_stale() {
        let mut s = editing();
        draw(&mut s, Tool::Rect, [20.0, 20.0], [60.0, 50.0], NONE);
        let original = s.doc.items.clone();
        s.press([40.0, 20.0], NONE); // picks the rect up, gesture open
        s.drag([45.0, 30.0], NONE);
        assert_ne!(s.doc.items, original);
        s.act(Action::Undo);
        assert_eq!(s.doc.items, original); // the move is undone, not the rect
        s.drag([80.0, 80.0], NONE); // a stale gesture would move it again
        s.release([80.0, 80.0], NONE);
        assert_eq!(s.doc.items, original);
    }

    #[test]
    fn copy_and_save_do_nothing_while_choosing() {
        let mut s = session(TargetKind::Area);
        assert!(s.act(Action::Copy).is_none());
        assert!(s.act(Action::Save).is_none());
        assert_eq!(s.stage, Stage::Choosing);
    }

    #[test]
    fn the_hover_tween_retargets_only_when_the_target_changes() {
        let mut s = session(TargetKind::Window);
        s.pointer_moved([20.0, 20.0], 0.0);
        assert!(s.hover.is_some());
        s.pointer_moved([70.0, 70.0], 1.0); // the window below
        // Still inside it: no retarget, so the glide finishes at 1.16.
        s.pointer_moved([80.0, 80.0], 1.1);
        let end = s.hover.unwrap().at(1.16, false);
        assert_eq!(end, rect(0.0, 0.0, 100.0, 100.0));
        // Nothing under the pointer: no highlight.
        s.pointer_moved([150.0, 50.0], 2.0);
        assert!(s.hover.is_none());
    }

    #[test]
    fn shift_makes_rectangles_square() {
        let mut s = editing();
        let shift = Mods {
            shift: true,
            ctrl: false,
        };
        draw(&mut s, Tool::Rect, [20.0, 20.0], [60.0, 30.0], shift);
        assert_eq!(
            s.doc.items[0].shape,
            Shape::Rect(rect(20.0, 20.0, 40.0, 40.0))
        );
    }

    #[test]
    fn select_all_works_only_in_area() {
        let mut s = session(TargetKind::Window);
        s.act(Action::SelectAll);
        assert_eq!(s.stage, Stage::Choosing);
        let mut s = session(TargetKind::Area);
        s.act(Action::SelectAll);
        assert_eq!(s.selection.rect, Some(rect(0.0, 0.0, 200.0, 100.0)));
    }

    #[test]
    fn recolouring_and_deleting_are_undoable() {
        let mut s = editing();
        draw(&mut s, Tool::Rect, [20.0, 20.0], [60.0, 50.0], NONE);
        s.press([40.0, 20.0], NONE);
        s.release([40.0, 20.0], NONE);
        s.act(Action::SetColour([1, 2, 3, 255]));
        assert_eq!(s.doc.items[0].colour, [1, 2, 3, 255]);
        s.act(Action::Delete);
        assert!(s.doc.items.is_empty());
        s.act(Action::Undo);
        s.act(Action::Undo);
        assert_ne!(s.doc.items[0].colour, [1, 2, 3, 255]);
    }
}
