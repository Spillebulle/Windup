//! The capture overlay as a state machine, from the moment the screen is
//! frozen until the result is copied, saved or cancelled.
//!
//! Pure: no egui, no clock, no I/O. Every position is in desktop units and
//! every time is passed in, so the view only converts pointer positions,
//! forwards events and paints what is here.
//!
//! Sizes the user sees are in screen points, so the editor feels the same on
//! every desktop and monitor: the handle tolerance, `MIN_DRAG`, the nudge
//! step and the tool sizes. Each is turned into desktop units by the scale of
//! the monitor under the pointer (`units_per_point`), or under the start of
//! an annotation being drawn (or the one a selected annotation is on, when
//! the wheel resizes it). An annotation keeps its width in desktop units
//! from then on, so moving it never changes its size.
//!
//! Video shares the choosing: once a selection is made it goes to `Ready`
//! instead of `Editing`, where the selection stays adjustable, there are no
//! annotation tools, and Record ends the session with the rectangle and the
//! recording choices. Each mode keeps its own target.
//!
//! Steps needs no selection: its row has the trigger and marker choices,
//! and Start ends the session with them. The same editor also opens on one
//! step's picture ([`Session::edit_image`]), where Copy and Save both give
//! the whole picture back with the edits burned in.

use image::RgbaImage;

use super::annotate::{Annotation, Doc, Shape, Tool, press_selects};
use super::geometry::{DRect, DesktopShot, MonitorShot};
use super::pick::{Tween, WindowRect, edge_rect, window_at};
use super::raster::render;
use super::selection::{Handle, MIN_DRAG, Selection};
use crate::config::{ScreenshotConfig, StepsConfig, TargetKind, ToolSizes, VideoConfig};
use crate::steps::options::{MarkerKind, StepTrigger};
use crate::video::pcm::GIF_HAS_NO_SOUND;
use crate::video::{Bitrate, Container, frame_rates};

/// How near the pointer must be to a handle or an outline, in screen points.
pub const HANDLE_TOLERANCE: f32 = 6.0;
/// How long the size figure by the pointer stays, in seconds.
pub const SIZE_FLASH_SECS: f64 = 0.8;
/// One wheel step changes a size by this much, in points.
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
    /// Video's selection is made: the recording options and Record.
    Ready,
}

#[derive(Debug)]
pub enum Outcome {
    Copy(RgbaImage),
    Save(RgbaImage),
    CopyAndSave(RgbaImage),
    /// Record `rect` (desktop units) with these choices.
    Record {
        rect: DRect,
        choice: VideoChoice,
    },
    /// Start a Steps session with these choices.
    Steps {
        choice: StepsChoice,
    },
    /// A step's picture as edited, the whole of it.
    StepDone(RgbaImage),
    Cancel,
}

/// What the session is for: a capture from the frozen desktop, or editing
/// one step's picture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Purpose {
    #[default]
    Capture,
    StepEdit,
}

/// What a Steps session is asked to be, as picked on the bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StepsChoice {
    pub trigger: StepTrigger,
    pub marker: MarkerKind,
}

/// What a recording is asked to be, as picked on the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoChoice {
    pub fps: u32,
    pub bitrate: Bitrate,
    pub gif_colours: u16,
    pub system_audio: bool,
    pub mic: bool,
}

/// What works on this machine right now, and if not, why (rule 7: a control
/// that cannot work is drawn disabled and says why). Plain data the desktop
/// gathers before the overlay opens; see `crate::video::avail`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeAvailability {
    pub video: Result<(), &'static str>,
    pub system_audio: Result<(), &'static str>,
    pub mic: Result<(), &'static str>,
    /// Whether Steps can take pictures here.
    pub steps: Result<(), &'static str>,
    /// Whether clicks can be seen, for the triggers that use them.
    pub clicks: Result<(), &'static str>,
}

impl ModeAvailability {
    /// Everything works.
    pub const ALL: ModeAvailability = ModeAvailability {
        video: Ok(()),
        system_audio: Ok(()),
        mic: Ok(()),
        steps: Ok(()),
        clicks: Ok(()),
    };
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
    Record,
    SetFps(u32),
    SetBitrate(Bitrate),
    SetGifColours(u16),
    ToggleSystemAudio,
    ToggleMic,
    StartSteps,
    SetStepTrigger(StepTrigger),
    SetMarker(MarkerKind),
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
    /// Video's choices as picked: preferences, kept even where they cannot
    /// apply right now. [`Session::video_choice`] is what would be recorded.
    pub video: VideoChoice,
    /// The configured container; a GIF has colours and no sound.
    pub container: Container,
    pub availability: ModeAvailability,
    /// Steps' choices as picked.
    pub steps: StepsChoice,
    pub purpose: Purpose,
    /// The mode `target` belongs to: Screenshot or Video. Steps has none, so
    /// choosing it leaves the target as it was.
    target_owner: CaptureMode,
    /// The target of the other of Screenshot and Video, for when it comes back.
    other_target: TargetKind,
    hover_target: Option<DRect>,
    also_save_on_copy: bool,
    grab: Grab,
    /// The selection edge last grabbed, which the arrow keys then move.
    edge: Option<Handle>,
    /// A press outside the selection, once one is made: a drag there draws
    /// a new area, a click picks the window or screen under it.
    reselecting: bool,
    draw_start: (f32, f32),
}

impl Session {
    pub fn new(
        shot: DesktopShot,
        windows: Vec<WindowRect>,
        cfg: &ScreenshotConfig,
        video: &VideoConfig,
        availability: ModeAvailability,
    ) -> Session {
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
            video: VideoChoice {
                fps: video.fps,
                bitrate: video.bitrate,
                gif_colours: video.gif_colours,
                system_audio: video.system_audio,
                mic: video.mic,
            },
            container: video.container,
            availability,
            steps: StepsChoice::default(),
            purpose: Purpose::Capture,
            target_owner: CaptureMode::Screenshot,
            other_target: video.last_target,
            hover_target: None,
            also_save_on_copy: cfg.also_save_on_copy,
            grab: Grab::Nothing,
            edge: None,
            reselecting: false,
            draw_start: (0.0, 0.0),
        }
    }

    /// The editor on one picture, `image`, shown as one monitor at `layout`:
    /// the whole of it selected, so Copy and Save give it back with the
    /// edits burned in ([`Outcome::StepDone`]).
    pub fn edit_image(image: RgbaImage, layout: DRect, cfg: &ScreenshotConfig) -> Session {
        let scale = if layout.w > 0.0 {
            image.width() as f32 / layout.w
        } else {
            1.0
        };
        let shot = DesktopShot {
            monitors: vec![MonitorShot {
                layout,
                image,
                name: String::new(),
                scale,
                refresh_hz: None,
            }],
        };
        let cfg = ScreenshotConfig {
            last_target: TargetKind::Area,
            ..cfg.clone()
        };
        let mut s = Session::new(
            shot,
            Vec::new(),
            &cfg,
            &VideoConfig::default(),
            ModeAvailability::ALL,
        );
        s.purpose = Purpose::StepEdit;
        s.selection.set_rect(layout);
        s.stage = Stage::Editing;
        s
    }

    /// Steps' choices from its settings.
    pub fn set_steps(&mut self, cfg: &StepsConfig) {
        self.steps = StepsChoice {
            trigger: cfg.trigger,
            marker: cfg.marker.kind,
        };
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

    /// While choosing, the window or screen a click would pick glides to the
    /// pointer; once a selection is made, the same outside it, where a
    /// click picks again.
    pub fn pointer_moved(&mut self, p: [f32; 2], now: f64) {
        self.pointer = Some(p);
        let at = (p[0], p[1]);
        let target = match self.stage {
            Stage::Choosing if self.mode == CaptureMode::Steps => None,
            Stage::Choosing => self.target_at(at),
            Stage::Editing | Stage::Ready
                if self.grab == Grab::Nothing && self.outside_selection(at) =>
            {
                self.target_at(at)
            }
            Stage::Editing | Stage::Ready => None,
        };
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
                if self.target == TargetKind::Area && self.mode != CaptureMode::Steps {
                    self.selection.press(at, self.handle_tolerance(at));
                    self.grab = Grab::Selection;
                }
            }
            Stage::Editing => self.press_editing(at),
            Stage::Ready => self.press_ready(at),
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
                let min_drag = MIN_DRAG * self.units_per_point((p[0], p[1]));
                let click = self.selection.release(min_drag);
                if self.stage == Stage::Choosing && self.has_selection(min_drag) {
                    self.enter_selected();
                }
                if std::mem::take(&mut self.reselecting) {
                    if !click {
                        // A drawn area, whatever the dropdown said.
                        self.set_target(TargetKind::Area);
                    } else if let Some(rect) = self.target_at((p[0], p[1])) {
                        self.selection.set_rect(rect);
                        self.hover = None;
                        self.hover_target = None;
                    }
                }
            }
            Grab::Transform => self.doc.end_transform(),
            Grab::Draw => self.commit_draft(),
            Grab::Nothing => {
                if self.stage == Stage::Choosing
                    && self.mode != CaptureMode::Steps
                    && self.target != TargetKind::Area
                    && let Some(rect) = self.target_at((p[0], p[1]))
                {
                    self.selection.set_rect(rect);
                    self.enter_selected();
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
            && let Some(a) = self.doc.get(id)
        {
            // The annotation's size is in desktop units; the step, the
            // limits and the figure shown are in points, by the scale of
            // the monitor the annotation is on, wherever the pointer is.
            let (current, b) = (a.size, a.shape.bounds());
            let k = self.units_per_point((b.x + b.w / 2.0, b.y + b.h / 2.0));
            let size = (current / k + delta).clamp(MIN_TOOL_SIZE, MAX_TOOL_SIZE);
            self.settle();
            self.doc.set_size(id, size * k);
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
        matches!(self.stage, Stage::Editing | Stage::Ready)
            && self.grab == Grab::Selection
            && self.edge.is_some()
    }

    /// Whether a new area is being dragged out over a selection already made,
    /// which the loupe helps aim as it does the first.
    pub fn redrawing_area(&self) -> bool {
        self.grab == Grab::Selection && self.reselecting
    }

    // --- video -------------------------------------------------------------

    /// The target `mode` has: the one shown for the current mode, the one
    /// set aside for the other.
    pub fn target_of(&self, mode: CaptureMode) -> TargetKind {
        if mode == self.target_owner || mode == CaptureMode::Steps {
            self.target
        } else {
            self.other_target
        }
    }

    /// Whether computer sound can be recorded, and why not.
    pub fn system_audio(&self) -> Result<(), &'static str> {
        self.sound(self.availability.system_audio)
    }

    /// Whether the microphone can be recorded, and why not.
    pub fn mic(&self) -> Result<(), &'static str> {
        self.sound(self.availability.mic)
    }

    fn sound(&self, available: Result<(), &'static str>) -> Result<(), &'static str> {
        if self.container == Container::Gif {
            Err(GIF_HAS_NO_SOUND)
        } else {
            available
        }
    }

    /// The frame rates to offer: the usual ones, plus the refresh rate of the
    /// fastest monitor the selection touches when that is faster. Asked
    /// afresh each time, so it follows the selection as it moves.
    pub fn frame_rates(&self) -> Vec<u32> {
        let max = self.selection.rect.map(|r| r.normalised()).and_then(|sel| {
            self.shot
                .monitors
                .iter()
                .filter(|m| m.layout.intersect(&sel).is_some())
                .filter_map(|m| m.refresh_hz)
                .max()
        });
        frame_rates(max)
    }

    /// What Record would ask for: the choices, with a frame rate the
    /// selection's monitors offer (the fastest one not above the choice) and
    /// no sound that cannot be recorded.
    pub fn video_choice(&self) -> VideoChoice {
        let rates = self.frame_rates();
        let fps = if rates.contains(&self.video.fps) {
            self.video.fps
        } else {
            rates
                .iter()
                .copied()
                .filter(|r| *r <= self.video.fps)
                .max()
                .unwrap_or(rates[0])
        };
        VideoChoice {
            fps,
            system_audio: self.video.system_audio && self.system_audio().is_ok(),
            mic: self.video.mic && self.mic().is_ok(),
            ..self.video
        }
    }

    // --- actions -----------------------------------------------------------

    pub fn act(&mut self, a: Action) -> Option<Outcome> {
        let editing = self.stage == Stage::Editing;
        let ready = self.stage == Stage::Ready;
        let choosing = self.stage == Stage::Choosing;
        let step_edit = self.purpose == Purpose::StepEdit;
        match a {
            Action::Cancel => return Some(Outcome::Cancel),
            Action::Copy | Action::Save if !editing => {}
            Action::Copy | Action::Save if step_edit => {
                let whole = self.shot.bounds();
                return render(&self.shot, whole, &self.doc).map(Outcome::StepDone);
            }
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
                if self.target == TargetKind::Area
                    && self.grab == Grab::Nothing
                    && self.mode != CaptureMode::Steps
                    && !step_edit
                {
                    self.selection.select_all(self.shot.bounds());
                    self.enter_selected();
                }
            }
            // A step's picture is selected whole and stays so.
            Action::Nudge { dx, dy } if (editing || ready) && !step_edit => {
                // A step is in points.
                let k = self.units_per_point(self.here());
                self.selection.nudge(dx * k, dy * k, self.edge);
                // Kept on the desktop, as a drag keeps it: a rect wholly
                // off it would render nothing, and Copy and Save would do
                // nothing without a word.
                let bounds = self.shot.bounds();
                let resizing = self.edge.is_some_and(|h| h != Handle::Move);
                if let Some(r) = &mut self.selection.rect {
                    if resizing {
                        // A nudged edge stops at the border; the opposite
                        // edge stays where it is.
                        let l = r.x.max(bounds.x).min(bounds.right() - 1.0);
                        let t = r.y.max(bounds.y).min(bounds.bottom() - 1.0);
                        let right = r.right().min(bounds.right()).max(l + 1.0);
                        let bottom = r.bottom().min(bounds.bottom()).max(t + 1.0);
                        *r = DRect {
                            x: l,
                            y: t,
                            w: right - l,
                            h: bottom - t,
                        };
                    } else {
                        r.x = r.x.min(bounds.right() - r.w).max(bounds.x);
                        r.y = r.y.min(bounds.bottom() - r.h).max(bounds.y);
                    }
                }
            }
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
            // Once a selection is made it stays; the target says what a
            // click outside it picks next.
            Action::SetTarget(kind) if self.grab == Grab::Nothing && !step_edit => {
                self.set_target(kind)
            }
            // Video and Steps when they can work here.
            Action::SetMode(mode) if choosing && mode != self.mode && !step_edit => {
                let allowed = match mode {
                    CaptureMode::Screenshot => true,
                    CaptureMode::Video => self.availability.video.is_ok(),
                    CaptureMode::Steps => self.availability.steps.is_ok(),
                };
                if allowed && self.grab == Grab::Nothing {
                    // Steps has no target: the one shown stays for whichever
                    // of the others comes back.
                    if mode != CaptureMode::Steps && mode != self.target_owner {
                        let shown = std::mem::replace(&mut self.other_target, self.target);
                        self.target_owner = mode;
                        self.set_target(shown);
                    }
                    self.mode = mode;
                    self.hover = None;
                    self.hover_target = None;
                }
            }
            Action::SetColour(colour) => self.set_colour(colour),
            Action::Record if ready => {
                return self
                    .selection
                    .rect
                    .map(|r| r.normalised())
                    .filter(|r| r.w > 0.0 && r.h > 0.0)
                    .map(|rect| Outcome::Record {
                        rect,
                        choice: self.video_choice(),
                    });
            }
            Action::SetFps(fps) => self.video.fps = fps,
            Action::SetBitrate(bitrate) => self.video.bitrate = bitrate,
            Action::SetGifColours(n) => self.video.gif_colours = n,
            Action::ToggleSystemAudio => {
                if self.system_audio().is_ok() {
                    self.video.system_audio = !self.video.system_audio;
                }
            }
            Action::ToggleMic => {
                if self.mic().is_ok() {
                    self.video.mic = !self.video.mic;
                }
            }
            Action::StartSteps if choosing && self.mode == CaptureMode::Steps => {
                return Some(Outcome::Steps { choice: self.steps });
            }
            Action::SetStepTrigger(trigger) => {
                let uses_clicks = trigger != StepTrigger::HotkeyOnly;
                if !uses_clicks || self.availability.clicks.is_ok() {
                    self.steps.trigger = trigger;
                }
            }
            Action::SetMarker(marker) => self.steps.marker = marker,
            Action::Nudge { .. }
            | Action::Delete
            | Action::SetTarget(_)
            | Action::SetMode(_)
            | Action::Record
            | Action::StartSteps => {}
        }
        None
    }

    /// Desktop units per screen point at `at`: the scale of the monitor
    /// there, or of the nearest one when `at` is between monitors.
    pub fn units_per_point(&self, at: (f32, f32)) -> f32 {
        let gap = |m: &&MonitorShot| {
            let dx = (m.layout.x - at.0).max(at.0 - m.layout.right()).max(0.0);
            let dy = (m.layout.y - at.1).max(at.1 - m.layout.bottom()).max(0.0);
            dx * dx + dy * dy
        };
        self.shot
            .monitors
            .iter()
            .min_by(|a, b| gap(a).total_cmp(&gap(b)))
            .map(|m| m.scale)
            .filter(|k| k.is_finite() && *k > 0.0)
            .unwrap_or(1.0)
    }

    /// `HANDLE_TOLERANCE` in desktop units at `at`.
    pub fn handle_tolerance(&self, at: (f32, f32)) -> f32 {
        HANDLE_TOLERANCE * self.units_per_point(at)
    }

    // --- internals ---------------------------------------------------------

    /// Where a key's step is measured: the pointer, else the selection.
    fn here(&self) -> (f32, f32) {
        match (self.pointer, self.selection.rect) {
            (Some(p), _) => (p[0], p[1]),
            (None, Some(r)) => (r.x, r.y),
            (None, None) => (0.0, 0.0),
        }
    }

    fn result(&self, copy: bool, save: bool) -> Option<Outcome> {
        match self.finish(copy, save) {
            Outcome::Cancel => None,
            done => Some(done),
        }
    }

    fn has_selection(&self, min_drag: f32) -> bool {
        self.selection
            .rect
            .is_some_and(|r| r.w >= min_drag && r.h >= min_drag)
    }

    /// A selection is made: the editor for a screenshot, Ready for video.
    fn enter_selected(&mut self) {
        self.stage = match self.mode {
            CaptureMode::Video => Stage::Ready,
            CaptureMode::Screenshot | CaptureMode::Steps => Stage::Editing,
        };
        self.hover = None;
        self.hover_target = None;
    }

    fn set_target(&mut self, kind: TargetKind) {
        self.target = kind;
        self.selection.kind = kind;
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

    /// Ready has no tools: an edge or corner resizes the selection, inside
    /// moves it, outside starts another.
    fn press_ready(&mut self, at: (f32, f32)) {
        let tolerance = self.handle_tolerance(at);
        match self.selection.handle_at(at, tolerance) {
            Some(h) => {
                self.edge = (h != Handle::Move).then_some(h);
                self.selection.press(at, tolerance);
                self.grab = Grab::Selection;
            }
            None => self.reselect(at, tolerance),
        }
    }

    /// A press outside the selection: a drag draws a new area in its place,
    /// a click picks the window or screen there (`release`).
    fn reselect(&mut self, at: (f32, f32), tolerance: f32) {
        self.doc.selected = None;
        self.edge = None;
        self.hover = None;
        self.hover_target = None;
        self.selection.press(at, tolerance);
        self.grab = Grab::Selection;
        self.reselecting = true;
    }

    /// Outside the selection and its edges.
    fn outside_selection(&self, at: (f32, f32)) -> bool {
        self.selection
            .handle_at(at, self.handle_tolerance(at))
            .is_none()
    }

    fn press_editing(&mut self, at: (f32, f32)) {
        if self.tool == Tool::Pipette {
            if let Some(colour) = self.sample(at) {
                self.set_colour(colour);
                self.tool = self.prev_tool;
            }
            return;
        }
        let tolerance = self.handle_tolerance(at);
        // The transform box of the selected annotation: its edges and corners.
        if let Some(id) = self.doc.selected
            && let Some(a) = self.doc.get(id)
        {
            let mut probe = Selection::default();
            probe.set_rect(a.shape.bounds().normalised());
            if let Some(h) = probe.handle_at(at, tolerance)
                && h != Handle::Move
            {
                self.doc.begin_transform(id, h, at);
                self.grab = Grab::Transform;
                return;
            }
        }
        // The tool that made an annotation, or Select, picks it up.
        if let Some(id) = self.doc.hit(at, tolerance / 2.0)
            && press_selects(self.tool, self.doc.get(id))
        {
            self.doc.selected = Some(id);
            self.doc.begin_transform(id, Handle::Move, at);
            self.grab = Grab::Transform;
            return;
        }
        // A step's picture is selected whole: no edges to drag, nothing
        // outside it to pick, and Select moves nothing.
        if self.purpose == Purpose::StepEdit {
            self.doc.selected = None;
            if self.tool != Tool::Select {
                self.start_draft(at);
            }
            return;
        }
        // The selection's own edges win over drawing, whatever the tool.
        let handle = self.selection.handle_at(at, tolerance);
        if let Some(h) = handle
            && h != Handle::Move
        {
            self.edge = Some(h);
            self.selection.press(at, tolerance);
            self.grab = Grab::Selection;
            return;
        }
        // Outside it, whatever the tool, the area is chosen again.
        if handle.is_none() {
            self.reselect(at, tolerance);
            return;
        }
        self.doc.selected = None;
        if self.tool == Tool::Select {
            // Inside: Select moves it.
            self.edge = None;
            self.selection.press(at, tolerance);
            self.grab = Grab::Selection;
            return;
        }
        self.start_draft(at);
    }

    fn start_draft(&mut self, at: (f32, f32)) {
        let tool = self.tool;
        // Points now, desktop units for good.
        let Some(size) = self.tool_size(tool).map(|pt| pt * self.units_per_point(at)) else {
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
        let min_drag = MIN_DRAG * self.units_per_point(self.draw_start);
        let worth_keeping = match &d.shape {
            Shape::Pen(points) => points.len() >= 2,
            shape => {
                let b = shape.bounds();
                b.w >= min_drag && b.h >= min_drag
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
    use crate::steps::avail::NO_STEPS_SOURCE;
    use crate::video::pcm::{GIF_HAS_NO_SOUND, SOUND_NEEDS_PIPEWIRE};
    use crate::video::{Bitrate, Container};
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
                    scale: 1.0,
                    refresh_hz: None,
                },
                MonitorShot {
                    layout: rect(100.0, 0.0, 100.0, 100.0),
                    image: RgbaImage::from_pixel(200, 200, Rgba([10, 200, 10, 255])),
                    name: "b".into(),
                    scale: 1.0,
                    refresh_hz: None,
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
        Session::new(
            shot(),
            windows(),
            &cfg,
            &VideoConfig::default(),
            ModeAvailability::ALL,
        )
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
        let mut s = Session::new(
            shot(),
            vec![],
            &cfg,
            &VideoConfig::default(),
            ModeAvailability::ALL,
        );
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
    fn a_drag_outside_the_selection_draws_a_new_area_with_any_tool() {
        for tool in [Tool::Select, Tool::Pen, Tool::Rect] {
            let mut s = editing();
            draw(&mut s, Tool::Ellipse, [20.0, 20.0], [60.0, 50.0], NONE);
            let before = s.doc.items.clone();
            s.act(Action::SetTool(tool));
            drag_area(&mut s, [120.0, 20.0], [180.0, 80.0]);
            assert_eq!(s.stage, Stage::Editing, "{tool:?}");
            assert_eq!(
                s.selection.rect,
                Some(rect(120.0, 20.0, 60.0, 60.0)),
                "{tool:?}"
            );
            assert_eq!(s.doc.items, before, "{tool:?}: nothing drawn, nothing lost");
        }
    }

    #[test]
    fn a_click_outside_the_selection_keeps_an_area_and_picks_a_window() {
        let mut s = editing();
        s.press([150.0, 50.0], NONE);
        s.release([150.0, 50.0], NONE);
        assert_eq!(s.selection.rect, Some(rect(10.0, 10.0, 80.0, 80.0)));

        s.act(Action::SetTarget(TargetKind::FullScreen));
        s.pointer_moved([50.0, 50.0], 0.0);
        assert!(s.hover.is_none(), "inside the selection nothing is offered");
        s.pointer_moved([150.0, 50.0], 0.0);
        assert!(s.hover.is_some(), "outside it, what a click would pick");
        s.press([150.0, 50.0], NONE);
        s.release([150.0, 50.0], NONE);
        assert_eq!(s.stage, Stage::Editing);
        assert_eq!(s.selection.rect, Some(rect(100.0, 0.0, 100.0, 100.0)));
        assert_eq!(s.target, TargetKind::FullScreen);
    }

    #[test]
    fn a_drawn_area_makes_the_target_area() {
        let mut s = session(TargetKind::Window);
        s.press([20.0, 20.0], NONE);
        s.release([20.0, 20.0], NONE);
        assert_eq!(s.stage, Stage::Editing);
        drag_area(&mut s, [120.0, 20.0], [180.0, 80.0]);
        assert_eq!(s.selection.rect, Some(rect(120.0, 20.0, 60.0, 60.0)));
        assert_eq!(s.target, TargetKind::Area);
    }

    #[test]
    fn ready_draws_a_new_area_outside_the_selection() {
        let mut s = ready();
        drag_area(&mut s, [120.0, 20.0], [180.0, 80.0]);
        assert_eq!(s.stage, Stage::Ready);
        assert_eq!(s.selection.rect, Some(rect(120.0, 20.0, 60.0, 60.0)));
    }

    #[test]
    fn redrawing_an_area_is_aimed_with_the_loupe() {
        let mut s = editing();
        s.press([120.0, 20.0], NONE);
        assert!(s.redrawing_area());
        s.drag([180.0, 80.0], NONE);
        assert!(s.redrawing_area());
        s.release([180.0, 80.0], NONE);
        assert!(!s.redrawing_area());
        s.press([150.0, 50.0], NONE);
        assert!(!s.redrawing_area(), "moving the new one is not");
    }

    /// `editing()` with monitor 0 at 150 %: 1.5 desktop units per point.
    fn editing_at_150() -> Session {
        let mut shot = shot();
        shot.monitors[0].scale = 1.5;
        let mut s = Session::new(
            shot,
            vec![],
            &ScreenshotConfig::default(),
            &VideoConfig::default(),
            ModeAvailability::ALL,
        );
        drag_area(&mut s, [10.0, 10.0], [90.0, 90.0]);
        assert_eq!(s.stage, Stage::Editing);
        s
    }

    #[test]
    fn the_handle_tolerance_is_in_points() {
        // 8 units right of the east edge at x = 90: outside 6 units, inside
        // 6 points (9 units) at 150 %.
        let mut s = editing_at_150();
        s.press([98.0, 50.0], NONE);
        assert!(s.adjusting_edge(), "grabbed the edge 6 points away");
        s.release([98.0, 50.0], NONE);

        let mut s = editing();
        s.press([98.0, 50.0], NONE);
        assert!(!s.adjusting_edge(), "8 units is past 6 points at 100 %");
    }

    #[test]
    fn a_pen_width_is_in_points_and_kept_in_units() {
        let mut s = editing_at_150();
        s.sizes.pen = 4.0;
        draw(&mut s, Tool::Pen, [20.0, 20.0], [60.0, 50.0], NONE);
        assert_eq!(s.doc.items[0].size, 6.0, "4 points at 150 %");
        assert_eq!(s.sizes.pen, 4.0, "the tool size stays in points");
        // Moved onto the 100 % monitor, it keeps its size.
        s.act(Action::SetTool(Tool::Select));
        s.press([40.0, 35.0], NONE);
        s.drag([140.0, 35.0], NONE);
        s.release([140.0, 35.0], NONE);
        assert_ne!(s.doc.items[0].shape, Shape::Pen(vec![(20.0, 20.0)]));
        assert_eq!(s.doc.items[0].size, 6.0);
    }

    #[test]
    fn a_nudge_is_one_point() {
        let mut s = editing_at_150();
        s.pointer_moved([50.0, 50.0], 0.0);
        s.act(Action::Nudge { dx: 1.0, dy: 0.0 });
        assert_eq!(s.selection.rect, Some(rect(11.5, 10.0, 80.0, 80.0)));
        let mut s = editing();
        s.pointer_moved([50.0, 50.0], 0.0);
        s.act(Action::Nudge { dx: 1.0, dy: 0.0 });
        assert_eq!(s.selection.rect, Some(rect(11.0, 10.0, 80.0, 80.0)));
    }

    #[test]
    fn the_shortest_drag_is_in_points() {
        // 4 units is past 3 points at 100 %, short of them (4.5) at 150 %.
        let mut s = session(TargetKind::Area);
        drag_area(&mut s, [10.0, 10.0], [14.0, 14.0]);
        assert_eq!(s.stage, Stage::Editing);
        let mut shot = shot();
        shot.monitors[0].scale = 1.5;
        let mut s = Session::new(
            shot,
            vec![],
            &ScreenshotConfig::default(),
            &VideoConfig::default(),
            ModeAvailability::ALL,
        );
        drag_area(&mut s, [10.0, 10.0], [14.0, 14.0]);
        assert_eq!(s.stage, Stage::Choosing);
    }

    #[test]
    fn scrolling_an_annotation_steps_one_point() {
        let mut s = editing_at_150();
        s.sizes.rect = 4.0;
        draw(&mut s, Tool::Rect, [20.0, 20.0], [60.0, 50.0], NONE);
        assert_eq!(s.doc.items[0].size, 6.0);
        s.press([40.0, 20.0], NONE);
        s.release([40.0, 20.0], NONE);
        s.scroll(1.0, 0.0);
        assert_eq!(s.doc.items[0].size, 7.5, "5 points");
        assert_eq!(s.size_flash.map(|f| f.0), Some(5.0), "shown in points");
    }

    #[test]
    fn scrolling_an_annotation_uses_its_own_monitor_scale() {
        let mut s = editing_at_150();
        s.sizes.rect = 4.0;
        draw(&mut s, Tool::Rect, [20.0, 20.0], [60.0, 50.0], NONE);
        s.press([40.0, 20.0], NONE);
        s.release([40.0, 20.0], NONE);
        // The pointer leaves for the scale-1 monitor before the wheel turns.
        s.pointer_moved([150.0, 50.0], 0.0);
        s.scroll(1.0, 0.0);
        assert_eq!(s.doc.items[0].size, 7.5, "5 points at 1.5");
    }

    #[test]
    fn a_nudged_edge_stops_at_the_desktop_border() {
        // The desktop is (0, 0)-(200, 100).
        let whole = rect(0.0, 0.0, 200.0, 100.0);
        for (edge, dx, dy) in [
            (Handle::W, -1.0, 0.0),
            (Handle::E, 1.0, 0.0),
            (Handle::N, 0.0, -1.0),
            (Handle::S, 0.0, 1.0),
            (Handle::NW, -1.0, -1.0),
            (Handle::SE, 10.0, 10.0),
        ] {
            let mut s = editing();
            s.selection.set_rect(whole);
            s.edge = Some(edge);
            for _ in 0..3 {
                s.act(Action::Nudge { dx, dy });
            }
            assert_eq!(s.selection.rect, Some(whole), "{edge:?}");
        }
    }

    #[test]
    fn a_nudged_edge_past_the_border_keeps_the_opposite_edge() {
        let mut s = editing(); // (10, 10)-(90, 90)
        s.edge = Some(Handle::W);
        s.act(Action::Nudge { dx: -50.0, dy: 0.0 });
        assert_eq!(s.selection.rect, Some(rect(0.0, 10.0, 90.0, 80.0)));
    }

    #[test]
    fn a_nudge_keeps_the_selection_on_the_desktop() {
        let mut s = editing();
        s.act(Action::Nudge {
            dx: 10_000.0,
            dy: -10_000.0,
        });
        // The desktop is (0, 0)-(200, 100); the selection is 80 x 80.
        assert_eq!(s.selection.rect, Some(rect(120.0, 0.0, 80.0, 80.0)));
        assert!(matches!(s.act(Action::Copy), Some(Outcome::Copy(_))));
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

    // --- video ---------------------------------------------------------------

    const NO_FFMPEG: &str = "Video needs FFmpeg, which is missing from this installation.";

    fn video_session(target: TargetKind, video: VideoConfig, avail: ModeAvailability) -> Session {
        let cfg = ScreenshotConfig {
            last_target: TargetKind::Area,
            ..ScreenshotConfig::default()
        };
        let video = VideoConfig {
            last_target: target,
            ..video
        };
        let mut s = Session::new(shot(), windows(), &cfg, &video, avail);
        s.act(Action::SetMode(CaptureMode::Video));
        s
    }

    fn ready() -> Session {
        let mut s = video_session(
            TargetKind::Area,
            VideoConfig::default(),
            ModeAvailability::ALL,
        );
        drag_area(&mut s, [10.0, 10.0], [90.0, 90.0]);
        s
    }

    #[test]
    fn video_mode_goes_to_ready_not_editing() {
        let s = ready();
        assert_eq!(s.mode, CaptureMode::Video);
        assert_eq!(s.stage, Stage::Ready);
        // No annotation tools: a drag inside moves the selection, draws nothing.
        let mut s = s;
        s.act(Action::SetTool(Tool::Pen));
        drag_area(&mut s, [50.0, 50.0], [60.0, 55.0]);
        assert!(s.doc.items.is_empty());
        assert_eq!(s.selection.rect, Some(rect(20.0, 15.0, 80.0, 80.0)));
    }

    #[test]
    fn enter_in_ready_records_the_selection_with_the_choices() {
        let video = VideoConfig {
            fps: 24,
            bitrate: Bitrate::Kbps(12000),
            system_audio: true,
            ..VideoConfig::default()
        };
        let mut s = video_session(TargetKind::Area, video, ModeAvailability::ALL);
        drag_area(&mut s, [10.0, 10.0], [90.0, 90.0]);
        s.act(Action::ToggleMic);
        s.act(Action::SetFps(60));
        // Copy and Save do nothing in Ready.
        assert!(s.act(Action::Copy).is_none());
        assert!(s.act(Action::Save).is_none());
        match s.act(Action::Record) {
            Some(Outcome::Record { rect: r, choice }) => {
                assert_eq!(r, rect(10.0, 10.0, 80.0, 80.0));
                assert_eq!(
                    choice,
                    VideoChoice {
                        fps: 60,
                        bitrate: Bitrate::Kbps(12000),
                        gif_colours: 256,
                        system_audio: true,
                        mic: true,
                    }
                );
            }
            other => panic!("expected a recording, got {other:?}"),
        }
    }

    #[test]
    fn record_does_nothing_while_choosing_or_in_screenshot_mode() {
        let mut s = video_session(
            TargetKind::Area,
            VideoConfig::default(),
            ModeAvailability::ALL,
        );
        assert!(s.act(Action::Record).is_none());
        let mut s = editing();
        assert!(s.act(Action::Record).is_none());
    }

    #[test]
    fn video_mode_is_refused_with_its_reason_when_unavailable() {
        let avail = ModeAvailability {
            video: Err(NO_FFMPEG),
            ..ModeAvailability::ALL
        };
        let s = video_session(TargetKind::Area, VideoConfig::default(), avail);
        assert_eq!(s.mode, CaptureMode::Screenshot);
        assert_eq!(s.availability.video, Err(NO_FFMPEG));
    }

    #[test]
    fn each_mode_keeps_its_own_target() {
        let video = VideoConfig {
            last_target: TargetKind::FullScreen,
            ..VideoConfig::default()
        };
        let cfg = ScreenshotConfig {
            last_target: TargetKind::Window,
            ..ScreenshotConfig::default()
        };
        let mut s = Session::new(shot(), windows(), &cfg, &video, ModeAvailability::ALL);
        assert_eq!(s.target, TargetKind::Window);
        s.act(Action::SetMode(CaptureMode::Video));
        assert_eq!(s.target, TargetKind::FullScreen);
        s.act(Action::SetTarget(TargetKind::Area));
        s.act(Action::SetMode(CaptureMode::Screenshot));
        assert_eq!(s.target, TargetKind::Window);
        assert_eq!(s.target_of(CaptureMode::Video), TargetKind::Area);
        assert_eq!(s.target_of(CaptureMode::Screenshot), TargetKind::Window);
    }

    #[test]
    fn a_window_target_in_video_records_its_rectangle_at_start() {
        let mut s = video_session(
            TargetKind::Window,
            VideoConfig::default(),
            ModeAvailability::ALL,
        );
        s.pointer_moved([20.0, 20.0], 0.0);
        s.press([20.0, 20.0], NONE);
        s.release([20.0, 20.0], NONE);
        assert_eq!(s.stage, Stage::Ready);
        // The window list changing later moves nothing.
        s.windows.clear();
        match s.act(Action::Record) {
            Some(Outcome::Record { rect: r, .. }) => assert_eq!(r, rect(10.0, 10.0, 40.0, 40.0)),
            other => panic!("expected a recording, got {other:?}"),
        }
    }

    #[test]
    fn audio_toggles_do_nothing_when_audio_is_unavailable() {
        let avail = ModeAvailability {
            system_audio: Err(SOUND_NEEDS_PIPEWIRE),
            mic: Err(SOUND_NEEDS_PIPEWIRE),
            ..ModeAvailability::ALL
        };
        // Wanted in settings, but there is no sound to record.
        let video = VideoConfig {
            mic: true,
            ..VideoConfig::default()
        };
        let mut s = video_session(TargetKind::Area, video, avail);
        drag_area(&mut s, [10.0, 10.0], [90.0, 90.0]);
        s.act(Action::ToggleSystemAudio);
        s.act(Action::ToggleMic);
        assert_eq!(s.system_audio(), Err(SOUND_NEEDS_PIPEWIRE));
        assert_eq!(s.mic(), Err(SOUND_NEEDS_PIPEWIRE));
        assert!(!s.video.system_audio, "the toggle did nothing");
        assert!(s.video.mic, "the preference is kept for later");
        match s.act(Action::Record) {
            Some(Outcome::Record { choice, .. }) => {
                assert!(!choice.system_audio && !choice.mic, "{choice:?}");
            }
            other => panic!("expected a recording, got {other:?}"),
        }
    }

    #[test]
    fn gif_disables_sound_with_its_reason() {
        let video = VideoConfig {
            container: Container::Gif,
            system_audio: true,
            ..VideoConfig::default()
        };
        let mut s = video_session(TargetKind::Area, video, ModeAvailability::ALL);
        drag_area(&mut s, [10.0, 10.0], [90.0, 90.0]);
        assert_eq!(s.system_audio(), Err(GIF_HAS_NO_SOUND));
        assert_eq!(s.mic(), Err(GIF_HAS_NO_SOUND));
        s.act(Action::ToggleMic);
        s.act(Action::SetGifColours(64));
        match s.act(Action::Record) {
            Some(Outcome::Record { choice, .. }) => {
                assert!(!choice.system_audio && !choice.mic);
                assert_eq!(choice.gif_colours, 64);
            }
            other => panic!("expected a recording, got {other:?}"),
        }
    }

    #[test]
    fn frame_rate_list_follows_the_selected_monitor() {
        let mut shot = shot();
        shot.monitors[0].refresh_hz = Some(60);
        shot.monitors[1].refresh_hz = Some(144);
        let video = VideoConfig {
            fps: 144,
            ..VideoConfig::default()
        };
        let mut s = Session::new(
            shot,
            vec![],
            &ScreenshotConfig::default(),
            &video,
            ModeAvailability::ALL,
        );
        s.act(Action::SetMode(CaptureMode::Video));
        assert_eq!(s.frame_rates(), vec![15, 24, 30, 60], "nothing selected");
        // On the 60 Hz monitor alone: 144 is not offered, and not recorded.
        drag_area(&mut s, [10.0, 10.0], [90.0, 90.0]);
        assert_eq!(s.frame_rates(), vec![15, 24, 30, 60]);
        assert_eq!(s.video_choice().fps, 60);
        // Across the seam it touches the 144 Hz one too.
        s.act(Action::Nudge { dx: 50.0, dy: 0.0 });
        assert_eq!(s.frame_rates(), vec![15, 24, 30, 60, 144]);
        assert_eq!(s.video_choice().fps, 144);
    }

    // --- steps --------------------------------------------------------------

    const NO_CLICKS: &str = "Windup cannot read the mouse.";

    fn steps_session(avail: ModeAvailability, cfg: &StepsConfig) -> Session {
        let mut s = Session::new(
            shot(),
            windows(),
            &ScreenshotConfig::default(),
            &VideoConfig::default(),
            avail,
        );
        s.set_steps(cfg);
        s.act(Action::SetMode(CaptureMode::Steps));
        s
    }

    #[test]
    fn steps_mode_needs_no_selection_and_start_ends_it() {
        let cfg = StepsConfig {
            trigger: StepTrigger::HotkeyOnly,
            ..StepsConfig::default()
        };
        let mut s = steps_session(ModeAvailability::ALL, &cfg);
        assert_eq!(s.mode, CaptureMode::Steps);
        drag_area(&mut s, [10.0, 10.0], [200.0, 200.0]);
        assert_eq!(s.selection.rect, None);
        assert_eq!(s.stage, Stage::Choosing);
        // A click picks nothing either, whatever the target.
        s.act(Action::SetTarget(TargetKind::FullScreen));
        s.pointer_moved([50.0, 50.0], 0.0);
        assert!(s.hover.is_none());
        s.press([50.0, 50.0], NONE);
        s.release([50.0, 50.0], NONE);
        assert_eq!(s.selection.rect, None);
        match s.act(Action::StartSteps) {
            Some(Outcome::Steps { choice }) => assert_eq!(
                choice,
                StepsChoice {
                    trigger: StepTrigger::HotkeyOnly,
                    marker: MarkerKind::Badge,
                }
            ),
            other => panic!("expected steps, got {other:?}"),
        }
    }

    #[test]
    fn steps_mode_is_refused_with_its_reason_when_unavailable() {
        let avail = ModeAvailability {
            steps: Err(NO_STEPS_SOURCE),
            ..ModeAvailability::ALL
        };
        let mut s = steps_session(avail, &StepsConfig::default());
        assert_eq!(s.mode, CaptureMode::Screenshot);
        assert_eq!(s.availability.steps, Err(NO_STEPS_SOURCE));
        assert!(s.act(Action::StartSteps).is_none());
    }

    #[test]
    fn a_clicks_trigger_is_ignored_without_click_capture() {
        let avail = ModeAvailability {
            clicks: Err(NO_CLICKS),
            ..ModeAvailability::ALL
        };
        let cfg = StepsConfig {
            trigger: StepTrigger::HotkeyOnly,
            ..StepsConfig::default()
        };
        let mut s = steps_session(avail, &cfg);
        s.act(Action::SetStepTrigger(StepTrigger::ClicksOnly));
        s.act(Action::SetStepTrigger(StepTrigger::ClicksAndHotkey));
        assert_eq!(s.steps.trigger, StepTrigger::HotkeyOnly);
        s.act(Action::SetMarker(MarkerKind::Arrow));
        assert_eq!(s.steps.marker, MarkerKind::Arrow);
        // With clicks, it changes.
        let mut s = steps_session(ModeAvailability::ALL, &cfg);
        s.act(Action::SetStepTrigger(StepTrigger::ClicksOnly));
        assert_eq!(s.steps.trigger, StepTrigger::ClicksOnly);
    }

    #[test]
    fn a_step_edit_selects_the_whole_picture_and_ends_in_step_done() {
        let img = RgbaImage::from_pixel(1920, 1080, Rgba([30, 60, 90, 255]));
        let layout = rect(0.0, 0.0, 960.0, 540.0);
        let mut s = Session::edit_image(img, layout, &ScreenshotConfig::default());
        assert_eq!(s.purpose, Purpose::StepEdit);
        assert_eq!(s.stage, Stage::Editing);
        assert_eq!(s.selection.rect, Some(layout));
        assert_eq!(s.shot.monitors.len(), 1);
        assert_eq!(s.shot.monitors[0].scale, 2.0);
        draw(&mut s, Tool::Rect, [100.0, 100.0], [300.0, 200.0], NONE);
        assert_eq!(s.doc.items.len(), 1);
        match s.act(Action::Copy) {
            Some(Outcome::StepDone(img)) => {
                assert_eq!(img.dimensions(), (1920, 1080));
                // The rectangle is burned in.
                assert_ne!(img.get_pixel(200, 200).0, [30, 60, 90, 255]);
            }
            other => panic!("expected a step, got {other:?}"),
        }
        match s.act(Action::Save) {
            Some(Outcome::StepDone(img)) => assert_eq!(img.dimensions(), (1920, 1080)),
            other => panic!("expected a step, got {other:?}"),
        }
        assert!(matches!(s.act(Action::Cancel), Some(Outcome::Cancel)));
    }

    #[test]
    fn start_steps_does_nothing_in_other_modes() {
        let mut s = session(TargetKind::Area);
        assert!(s.act(Action::StartSteps).is_none());
        let mut s = editing();
        assert!(s.act(Action::StartSteps).is_none());
        let mut s = ready();
        assert!(s.act(Action::StartSteps).is_none());
    }

    #[test]
    fn steps_keeps_the_other_modes_targets() {
        let video = VideoConfig {
            last_target: TargetKind::FullScreen,
            ..VideoConfig::default()
        };
        let cfg = ScreenshotConfig {
            last_target: TargetKind::Window,
            ..ScreenshotConfig::default()
        };
        let mut s = Session::new(shot(), windows(), &cfg, &video, ModeAvailability::ALL);
        s.act(Action::SetMode(CaptureMode::Steps));
        assert_eq!(s.target_of(CaptureMode::Screenshot), TargetKind::Window);
        assert_eq!(s.target_of(CaptureMode::Video), TargetKind::FullScreen);
        s.act(Action::SetMode(CaptureMode::Video));
        assert_eq!(s.target, TargetKind::FullScreen);
        s.act(Action::SetMode(CaptureMode::Steps));
        s.act(Action::SetMode(CaptureMode::Screenshot));
        assert_eq!(s.target, TargetKind::Window);
        assert_eq!(s.target_of(CaptureMode::Video), TargetKind::FullScreen);
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
