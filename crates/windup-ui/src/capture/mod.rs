//! The capture overlay: the frozen screen drawn per monitor, with the bar,
//! the target hover, the selection and its marching ants, the annotations,
//! the loupe and the size figures.
//!
//! All state lives in [`Session`] (windup-core), in desktop units. This view
//! only turns pointer positions on one monitor's surface into desktop units
//! ([`Mapping`]), keys into [`Action`]s ([`action_for`]), and paints. One
//! `CaptureView` serves every monitor: the host calls
//! [`CaptureView::monitor_ui`] once per monitor surface per frame, each with
//! its own `Ui`.

pub mod ants;
pub mod bar;
pub mod loupe;
pub mod paint;
pub mod recbar;
#[cfg(test)]
mod view_tests;

use crate::theme::{Mode, Palette, metrics};
use bar::{Bar, BarCommand, BarOutput, BarState};
use egui::{
    Color32, ColorImage, CursorIcon, Event, Painter, Pos2, Rect, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Ui, Vec2, ViewportId, pos2, vec2,
};
use paint::{around, chip, chip_monitor, handles, result_size, size_chip};
use std::collections::HashMap;
use std::time::Duration;
use windup_core::capture::annotate::Tool;
use windup_core::capture::selection::Handle;
use windup_core::capture::session::{Action, CaptureMode, Mods, Outcome, Purpose, Session, Stage};
use windup_core::capture::{DRect, TWEEN_SECS};
use windup_core::config::{EditorKeys, TargetKind};
use windup_core::shortcut::{Shortcut, ShortcutModifiers};
use windup_core::video::Container;

/// Wheel input given in points (a touchpad, or a platform that reports
/// pixels) is this many points to one size step: egui's own figure for one
/// line of a notched wheel.
const POINTS_PER_WHEEL_STEP: f32 = 50.0;

/// One monitor's surface in points against its layout in desktop units.
///
/// Linear on each axis and not clamped, so a position past the surface's
/// edge maps to the desktop beyond it: that is how a drag held by one surface
/// carries on into the next monitor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mapping {
    pub surface: Rect,
    pub layout: DRect,
}

impl Mapping {
    /// Points per desktop unit on each axis.
    pub fn points_per_unit(&self) -> Vec2 {
        vec2(
            self.surface.width() / self.layout.w,
            self.surface.height() / self.layout.h,
        )
    }

    pub fn to_desktop(&self, p: Pos2) -> [f32; 2] {
        let s = self.points_per_unit();
        [
            self.layout.x + (p.x - self.surface.min.x) / s.x,
            self.layout.y + (p.y - self.surface.min.y) / s.y,
        ]
    }

    pub fn to_surface(&self, d: (f32, f32)) -> Pos2 {
        let s = self.points_per_unit();
        pos2(
            self.surface.min.x + (d.0 - self.layout.x) * s.x,
            self.surface.min.y + (d.1 - self.layout.y) * s.y,
        )
    }

    pub fn rect_to_surface(&self, r: DRect) -> Rect {
        let r = r.normalised();
        Rect::from_min_max(
            self.to_surface((r.x, r.y)),
            self.to_surface((r.right(), r.bottom())),
        )
    }
}

/// A key as pressed: the canonical name `Shortcut::key` uses (egui's name,
/// upper-cased, as the shortcut recorder stores it) and the modifiers held.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyPress {
    pub key: String,
    pub mods: ShortcutModifiers,
}

impl KeyPress {
    pub fn from_egui(key: egui::Key, m: egui::Modifiers) -> Self {
        Self {
            key: key.name().to_uppercase(),
            mods: ShortcutModifiers {
                super_key: false,
                // `command` is Ctrl off the Mac and Cmd on it.
                ctrl: m.ctrl || m.command,
                alt: m.alt,
                shift: m.shift,
            },
        }
    }
}

fn matches(s: &Shortcut, k: &KeyPress) -> bool {
    !s.key.is_empty() && s.key == k.key && s.modifiers == k.mods
}

/// What a key does in the overlay. Pure, so the mapping is tested without a
/// window.
///
/// Esc is first and cannot be rebound (spec "Escape"): whatever else is bound
/// to it, it cancels. Every other editor key comes from `keys` (Copy is bare
/// Enter by default); the fixed aliases are Ctrl+Y and Ctrl+Shift+Z for
/// redo, which the spec names beside the configured redo key, so both work
/// whichever one the settings hold. Modifiers match exactly, so the
/// loupe's bare Z and undo's Ctrl+Z do not overlap.
///
/// Video's Ready stage has nothing to copy, save or undo: plain Enter
/// records (fixed, as Esc is, since the editor's keys are the screenshot's),
/// and the arrows, the loupe and select-all work as in the editor. Plain
/// Enter likewise starts Steps from its row, and finishes the editor on a
/// step's picture, whatever the copy key is.
pub fn action_for(
    k: &KeyPress,
    keys: &EditorKeys,
    stage: Stage,
    mode: CaptureMode,
    purpose: Purpose,
) -> Option<Action> {
    const NONE: ShortcutModifiers = ShortcutModifiers::NONE;
    const SHIFT: ShortcutModifiers = ShortcutModifiers {
        super_key: false,
        ctrl: false,
        alt: false,
        shift: true,
    };
    if k.key == "ESCAPE" {
        return Some(Action::Cancel);
    }
    let ready = stage == Stage::Ready;
    if k.mods == NONE && k.key == "ENTER" {
        match (stage, mode, purpose) {
            (Stage::Ready, _, _) => return Some(Action::Record),
            (Stage::Choosing, CaptureMode::Steps, _) => return Some(Action::StartSteps),
            (Stage::Editing, _, Purpose::StepEdit) => return Some(Action::Copy),
            _ => {}
        }
    }
    for (s, a) in [
        (&keys.copy, Action::Copy),
        (&keys.save, Action::Save),
        (&keys.undo, Action::Undo),
        (&keys.redo, Action::Redo),
        (&keys.loupe, Action::ToggleLoupe),
        (&keys.select_all, Action::SelectAll),
    ] {
        if matches(s, k) {
            let works = !ready || matches!(a, Action::ToggleLoupe | Action::SelectAll);
            return works.then_some(a);
        }
    }
    if !ready
        && ((k.mods == ShortcutModifiers::CTRL && k.key == "Y")
            || (k.mods == ShortcutModifiers::CTRL_SHIFT && k.key == "Z"))
    {
        return Some(Action::Redo);
    }
    if k.mods == NONE || k.mods == SHIFT {
        let step = if k.mods.shift { 10.0 } else { 1.0 };
        let (dx, dy) = match k.key.as_str() {
            "LEFT" => (-step, 0.0),
            "RIGHT" => (step, 0.0),
            "UP" => (0.0, -step),
            "DOWN" => (0.0, step),
            _ => (0.0, 0.0),
        };
        if dx != 0.0 || dy != 0.0 {
            return Some(Action::Nudge { dx, dy });
        }
    }
    if !ready && k.mods == NONE && k.key == "DELETE" {
        return Some(Action::Delete);
    }
    None
}

/// What the pointer did on one surface this frame.
struct PointerIn {
    latest: Option<Pos2>,
    origin: Option<Pos2>,
    pressed: bool,
    released: bool,
    down: bool,
    mods: Mods,
    wheel: f32,
}

/// The overlay's view state across frames and monitors.
///
/// **One view per capture.** The frozen pictures are uploaded once and kept
/// by monitor index, so a view reused for the next capture would show the
/// previous one's screens. Make a new `CaptureView` with each `Session`.
#[derive(Default)]
pub struct CaptureView {
    /// The frozen image of each monitor, uploaded once.
    textures: HashMap<usize, TextureHandle>,
    previews: paint::Previews,
    loupe_tex: Option<TextureHandle>,
    /// The monitor and pixel the loupe texture was sampled at.
    loupe_at: Option<(usize, i32, i32)>,
    bar: Bar,
    /// The monitor whose surface took the press: it alone follows the drag,
    /// past its own edge if need be, until the release.
    held: Option<usize>,
    /// The viewport and pass whose keys were already handled, so a host that
    /// draws several monitors in one viewport (the example) acts once.
    keys_seen: Option<(ViewportId, u64)>,
    /// Wheel steps not yet whole.
    scroll: f32,
    open_settings: bool,
    done: bool,
    /// No glide for the window highlight. The host sets it from the OS's
    /// reduced-motion preference; Windup has no such flag yet.
    pub reduced_motion: bool,
}

impl CaptureView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the settings button was clicked since the last call. The host
    /// opens the settings window (and decides whether the capture goes on).
    pub fn take_open_settings(&mut self) -> bool {
        std::mem::take(&mut self.open_settings)
    }

    /// Draws monitor `monitor` of `session.shot` filling `ui.max_rect()` and
    /// handles its input. Returns the outcome once, on the frame the capture
    /// ends, and `None` on every call after.
    pub fn monitor_ui(
        &mut self,
        ui: &mut Ui,
        session: &mut Session,
        monitor: usize,
        p: &Palette,
        keys: &EditorKeys,
    ) -> Option<Outcome> {
        if self.done {
            return None;
        }
        let layout = session.shot.monitors.get(monitor)?.layout;
        let map = Mapping {
            surface: ui.max_rect(),
            layout,
        };
        if !map.surface.is_positive() || layout.w <= 0.0 || layout.h <= 0.0 {
            return None;
        }
        let now = ui.input(|i| i.time);
        let theme = if ui.visuals().dark_mode {
            Mode::Dark
        } else {
            Mode::Light
        };
        self.upload(ui.ctx(), session, monitor);

        let mut out = self.keys(ui, session, keys);

        // The bar first: it paints on its own layer above the overlay, and
        // where it is decides who gets the pointer.
        let bar_out = if out.is_none() && self.bar_monitor(session) == monitor {
            let state = bar_state(session, keys);
            let o = self.bar.show(ui, p, theme, map.surface, &state, now);
            if let Some(c) = o.command {
                out = self.command(session, c);
            }
            o
        } else {
            BarOutput::default()
        };

        let mut on_overlay = false;
        if out.is_none() {
            (out, on_overlay) = self.pointer(ui, session, monitor, &map, &bar_out, now);
        }

        self.paint(ui, session, monitor, &map, p, theme, now, on_overlay);

        if out.is_some() {
            self.done = true;
        }
        out
    }

    /// The monitor the bar is on: the one under the pointer, else the first.
    fn bar_monitor(&self, session: &Session) -> usize {
        session
            .pointer
            .and_then(|p| session.shot.monitor_at(p[0], p[1]))
            .unwrap_or(0)
    }

    fn upload(&mut self, ctx: &egui::Context, session: &Session, monitor: usize) {
        if self.textures.contains_key(&monitor) {
            return;
        }
        let img = &session.shot.monitors[monitor].image;
        let size = [img.width() as usize, img.height() as usize];
        let tex = ctx.load_texture(
            format!("windup-capture-monitor-{monitor}"),
            ColorImage::from_rgba_unmultiplied(size, img.as_raw()),
            TextureOptions::LINEAR,
        );
        self.textures.insert(monitor, tex);
    }

    fn command(&mut self, session: &mut Session, c: BarCommand) -> Option<Outcome> {
        let action = match c {
            BarCommand::Mode(m) => Action::SetMode(m),
            BarCommand::Target(t) => Action::SetTarget(t),
            BarCommand::Tool(t) => Action::SetTool(t),
            BarCommand::Colour(c) => Action::SetColour(c),
            BarCommand::Undo => Action::Undo,
            BarCommand::Save => Action::Save,
            BarCommand::Copy => Action::Copy,
            BarCommand::Close => Action::Cancel,
            BarCommand::Record => Action::Record,
            BarCommand::Fps(fps) => Action::SetFps(fps),
            BarCommand::Bitrate(b) => Action::SetBitrate(b),
            BarCommand::GifColours(n) => Action::SetGifColours(n),
            BarCommand::ToggleSystemAudio => Action::ToggleSystemAudio,
            BarCommand::ToggleMic => Action::ToggleMic,
            BarCommand::StartSteps => Action::StartSteps,
            BarCommand::StepTrigger(t) => Action::SetStepTrigger(t),
            BarCommand::Marker(m) => Action::SetMarker(m),
            BarCommand::Settings => {
                self.open_settings = true;
                return None;
            }
        };
        session.act(action)
    }

    // --- input -------------------------------------------------------------

    fn keys(&mut self, ui: &Ui, session: &mut Session, keys: &EditorKeys) -> Option<Outcome> {
        let ctx = ui.ctx();
        let pass = (ctx.viewport_id(), ctx.cumulative_pass_nr());
        if self.keys_seen == Some(pass) {
            return None;
        }
        self.keys_seen = Some(pass);
        let presses: Vec<(KeyPress, bool)> = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    Event::Key {
                        key,
                        pressed: true,
                        repeat,
                        modifiers,
                        ..
                    } => Some((KeyPress::from_egui(*key, *modifiers), *repeat)),
                    _ => None,
                })
                .collect()
        });
        for (press, repeat) in presses {
            let Some(action) =
                action_for(&press, keys, session.stage, session.mode, session.purpose)
            else {
                continue;
            };
            // A held key repeats; a toggle must not flicker with it.
            if repeat && action == Action::ToggleLoupe {
                continue;
            }
            if let Some(o) = session.act(action) {
                return Some(o);
            }
        }
        None
    }

    /// Hands this surface's pointer to the session. Returns the outcome (none
    /// today; `release` may end a session later) and whether the pointer is
    /// on this monitor's overlay rather than on the bar.
    fn pointer(
        &mut self,
        ui: &Ui,
        session: &mut Session,
        monitor: usize,
        map: &Mapping,
        bar: &BarOutput,
        now: f64,
    ) -> (Option<Outcome>, bool) {
        let i = ui.input(|i| PointerIn {
            latest: i.pointer.latest_pos(),
            origin: i.pointer.press_origin(),
            pressed: i.pointer.primary_pressed(),
            released: i.pointer.primary_released(),
            down: i.pointer.primary_down(),
            mods: Mods {
                shift: i.modifiers.shift,
                ctrl: i.modifiers.command,
            },
            wheel: i
                .events
                .iter()
                .map(|e| match e {
                    Event::MouseWheel { unit, delta, .. } => match unit {
                        egui::MouseWheelUnit::Point => delta.y / POINTS_PER_WHEEL_STEP,
                        egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => delta.y,
                    },
                    _ => 0.0,
                })
                .sum(),
        });

        let over_bar = i
            .latest
            .is_some_and(|at| bar.owns.iter().any(|r| r.contains(at)));
        let here = !over_bar && i.latest.is_some_and(|at| map.surface.contains(at));
        let mut out = None;

        if here
            && self.held.is_none()
            && let Some(latest) = i.latest
        {
            let d = map.to_desktop(latest);
            if session.pointer != Some(d) {
                session.pointer_moved(d, now);
            }
            if i.pressed && !bar.busy {
                let at = i.origin.unwrap_or(latest);
                session.press(map.to_desktop(at), i.mods);
                self.held = Some(monitor);
            }
            if i.wheel != 0.0 {
                self.scroll += i.wheel;
                let steps = self.scroll.trunc();
                if steps != 0.0 {
                    self.scroll -= steps;
                    session.scroll(steps, now);
                }
            }
            ui.ctx().set_cursor_icon(cursor(session, d));
        }

        if self.held == Some(monitor) {
            let at = i.latest.map(|at| map.to_desktop(at));
            if let Some(d) = at
                && i.down
                && session.pointer != Some(d)
            {
                session.drag(d, i.mods);
            }
            if let Some(d) = at {
                // Keep the gesture's cursor for the whole drag.
                ui.ctx().set_cursor_icon(cursor(session, d));
            }
            if i.released || !i.down {
                // A release this surface never saw (the pointer left the
                // window with the button down) ends where it was last seen.
                let d = at.or(session.pointer).unwrap_or([0.0, 0.0]);
                out = session.release(d, i.mods);
                self.held = None;
            }
        }
        (out, here)
    }

    // --- painting ----------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn paint(
        &mut self,
        ui: &Ui,
        session: &Session,
        monitor: usize,
        map: &Mapping,
        p: &Palette,
        theme: Mode,
        now: f64,
        on_overlay: bool,
    ) {
        let ctx = ui.ctx().clone();
        let painter = ui.painter_at(map.surface);
        let surface = map.surface;
        let full_uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));

        // The frozen screen.
        if let Some(tex) = self.textures.get(&monitor) {
            painter.image(tex.id(), surface, full_uv, Color32::WHITE);
        }

        // Annotations, under the dim: a part outside the selection shows
        // dimmed, which is what it is (cropped from the result, not lost).
        self.previews.prune(session);
        for a in &session.doc.items {
            self.previews
                .paint_annotation(&ctx, &painter, session, map, a, a.id, now);
        }
        if let Some(d) = &session.draft {
            self.previews
                .paint_annotation(&ctx, &painter, session, map, d, paint::DRAFT_ID, now);
        }

        // The hole: the selection, or the highlighted target while choosing.
        let hole = match session.stage {
            Stage::Editing | Stage::Ready => session.selection.rect,
            Stage::Choosing if session.target == TargetKind::Area => session.selection.rect,
            Stage::Choosing => session.hover.map(|t| t.at(now, self.reduced_motion)),
        }
        .map(|r| r.normalised())
        .filter(|r| r.w > 0.0 && r.h > 0.0);

        // §7.17's dim, `backdrop` at 60 %, over everything outside the hole.
        let dim = p.backdrop.gamma_multiply(metrics::DIM_ALPHA);
        match hole.map(|r| map.rect_to_surface(r)) {
            Some(h) => {
                for r in around(surface, h) {
                    painter.rect_filled(r, 0.0, dim);
                }
            }
            None => {
                painter.rect_filled(surface, 0.0, dim);
            }
        }

        if let Some(h) = hole {
            let r = map.rect_to_surface(h);
            let ring = [
                r.left_top(),
                r.right_top(),
                r.right_bottom(),
                r.left_bottom(),
            ];
            ants::ants(&painter, &ring, true, p, now, ctx.pixels_per_point());
            if gliding(session, now, self.reduced_motion) {
                // The hover glide lasts TWEEN_SECS and is smooth only at the
                // display's rate; ANT_FRAME_MS would make it three hops.
                ctx.request_repaint();
            } else {
                // The ants alone: a frame every ANT_FRAME_MS, never every frame.
                ctx.request_repaint_after(Duration::from_millis(metrics::ANT_FRAME_MS));
            }

            // A step's picture is selected whole: no handles to drag.
            if session.stage != Stage::Choosing && session.purpose != Purpose::StepEdit {
                handles(&painter, p, r);
            }
            if chip_monitor(session, h) == Some(monitor) {
                size_chip(&painter, p, theme, r, surface, &result_size(session, h));
            }
        }

        // Once a selection is made, the window or screen a click outside it
        // would pick instead: its ants, without the hole.
        if session.stage != Stage::Choosing
            && let Some(t) = session.hover
        {
            let r = map.rect_to_surface(t.at(now, self.reduced_motion).normalised());
            let ring = [
                r.left_top(),
                r.right_top(),
                r.right_bottom(),
                r.left_bottom(),
            ];
            ants::ants(&painter, &ring, true, p, now, ctx.pixels_per_point());
            if gliding(session, now, self.reduced_motion) {
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(Duration::from_millis(metrics::ANT_FRAME_MS));
            }
        }

        // The selected annotation's transform box.
        if session.stage == Stage::Editing
            && let Some(id) = session.doc.selected
            && let Some(a) = session.doc.get(id)
        {
            let r = map.rect_to_surface(a.shape.bounds().normalised());
            let w = ants::ant_width(ctx.pixels_per_point());
            painter.rect_stroke(
                r,
                0.0,
                Stroke::new(3.0 * w, p.accent_underlay()),
                StrokeKind::Middle,
            );
            painter.rect_stroke(r, 0.0, Stroke::new(w, p.accent), StrokeKind::Middle);
            handles(&painter, p, r);
        }

        let pointer_here = session
            .pointer
            .filter(|d| session.shot.monitor_at(d[0], d[1]) == Some(monitor));

        // The size figure by the pointer after a wheel step.
        if let Some((value, until)) = session.size_flash
            && now < until
            && let Some(d) = pointer_here
        {
            let at = map.to_surface((d[0], d[1])) + Vec2::splat(metrics::S3);
            chip(&painter, p, theme, at, &format!("{}", value.round() as i32));
            ctx.request_repaint_after(Duration::from_secs_f64(until - now));
        }

        // The loupe, where it helps aim and the pointer is on this overlay.
        if loupe_wanted(session)
            && on_overlay
            && let Some(d) = pointer_here
        {
            self.paint_loupe(&ctx, &painter, session, monitor, map, p, theme, d);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_loupe(
        &mut self,
        ctx: &egui::Context,
        painter: &Painter,
        session: &Session,
        monitor: usize,
        map: &Mapping,
        p: &Palette,
        theme: Mode,
        d: [f32; 2],
    ) {
        let m = &session.shot.monitors[monitor];
        let ppu = m.px_per_unit();
        let px = (
            ((d[0] - m.layout.x) * ppu).floor() as i32,
            ((d[1] - m.layout.y) * ppu).floor() as i32,
        );
        if self.loupe_at != Some((monitor, px.0, px.1)) || self.loupe_tex.is_none() {
            let Some(patch) = loupe::Patch::sample(&m.image, px, loupe::CELLS) else {
                return;
            };
            let img = patch.to_image();
            match &mut self.loupe_tex {
                Some(tex) => tex.set(img, TextureOptions::NEAREST),
                None => {
                    self.loupe_tex = Some(ctx.load_texture(
                        "windup-capture-loupe",
                        img,
                        TextureOptions::NEAREST,
                    ));
                }
            }
            self.loupe_at = Some((monitor, px.0, px.1));
        }
        let Some(tex) = &self.loupe_tex else { return };
        let pointer = map.to_surface((d[0], d[1]));
        let view = loupe::View {
            min: map.surface.min.to_vec2(),
            max: map.surface.max.to_vec2(),
        };
        let Some(centre) = loupe::place(pointer.to_vec2(), view, loupe::size() / 2.0) else {
            return;
        };
        let position = format!("{}, {}", d[0].floor() as i32, d[1].floor() as i32);
        loupe::paint(painter, p, theme, centre.to_pos2(), tex.id(), &position);
    }
}

/// Whether the loupe shows: when it is on, while choosing (any target: the
/// press or click is what it aims, though Steps has nothing to aim), and
/// while editing or ready only for the pipette, while an edge or corner of
/// the selection is being dragged, or while a new area is dragged out over it.
pub fn loupe_wanted(session: &Session) -> bool {
    session.loupe
        && match session.stage {
            Stage::Choosing => session.mode != CaptureMode::Steps,
            Stage::Editing => {
                session.tool == Tool::Pipette
                    || session.adjusting_edge()
                    || session.redrawing_area()
            }
            Stage::Ready => session.adjusting_edge() || session.redrawing_area(),
        }
}

/// What the bar shows this frame, read from the session.
fn bar_state<'a>(session: &Session, keys: &'a EditorKeys) -> BarState<'a> {
    BarState {
        stage: session.stage,
        mode: session.mode,
        target: session.target,
        tool: session.tool,
        colour: session.colour,
        keys,
        video: session.availability.video,
        choice: session.video_choice(),
        rates: session.frame_rates(),
        gif: session.container == Container::Gif,
        system_audio: session.system_audio(),
        mic: session.mic(),
        steps: session.steps,
        steps_mode: session.availability.steps,
        clicks: session.availability.clicks,
        purpose: session.purpose,
    }
}

/// Whether the window or full-screen highlight is still gliding to its
/// target. `Tween` keeps its timing private; once it has arrived, where it is
/// now and where it will be a whole tween later are the same rect.
pub fn gliding(session: &Session, now: f64, reduced_motion: bool) -> bool {
    !reduced_motion
        && session
            .hover
            .is_some_and(|t| t.at(now, false) != t.at(now + TWEEN_SECS, false))
}

/// The pointer's shape over the overlay at desktop position `d`.
fn cursor(session: &Session, d: [f32; 2]) -> CursorIcon {
    // Outside a selection already made, as while choosing: a drag there
    // draws a new area, a click picks the window or screen.
    let outside = if session.target == TargetKind::Area {
        CursorIcon::Crosshair
    } else {
        CursorIcon::PointingHand
    };
    match session.stage {
        // Steps picks nothing here.
        Stage::Choosing if session.mode == CaptureMode::Steps => CursorIcon::Default,
        Stage::Choosing => outside,
        Stage::Editing if session.tool == Tool::Pipette => CursorIcon::Crosshair,
        // A step's picture: Select picks up annotations, the rest draw.
        Stage::Editing if session.purpose == Purpose::StepEdit => {
            if session.tool == Tool::Select {
                CursorIcon::Default
            } else {
                CursorIcon::Crosshair
            }
        }
        // Ready has no tools: it acts as the select tool does.
        Stage::Ready => match session
            .selection
            .handle_at((d[0], d[1]), session.handle_tolerance((d[0], d[1])))
        {
            Some(Handle::N | Handle::S) => CursorIcon::ResizeVertical,
            Some(Handle::E | Handle::W) => CursorIcon::ResizeHorizontal,
            Some(Handle::NE | Handle::SW) => CursorIcon::ResizeNeSw,
            Some(Handle::NW | Handle::SE) => CursorIcon::ResizeNwSe,
            Some(Handle::Move) => CursorIcon::Move,
            None => outside,
        },
        Stage::Editing => match session
            .selection
            .handle_at((d[0], d[1]), session.handle_tolerance((d[0], d[1])))
        {
            Some(Handle::N | Handle::S) => CursorIcon::ResizeVertical,
            Some(Handle::E | Handle::W) => CursorIcon::ResizeHorizontal,
            Some(Handle::NE | Handle::SW) => CursorIcon::ResizeNeSw,
            Some(Handle::NW | Handle::SE) => CursorIcon::ResizeNwSe,
            Some(Handle::Move) if session.tool == Tool::Select => CursorIcon::Move,
            None => outside,
            _ if session.tool == Tool::Select => CursorIcon::Default,
            _ => CursorIcon::Crosshair,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A monitor left of the primary, at 2x: 1280 x 800 desktop units on a
    /// surface of 640 x 400 points, placed away from the origin.
    fn hidpi_left() -> Mapping {
        Mapping {
            surface: Rect::from_min_size(pos2(100.0, 50.0), vec2(640.0, 400.0)),
            layout: DRect {
                x: -1280.0,
                y: 0.0,
                w: 1280.0,
                h: 800.0,
            },
        }
    }

    #[test]
    fn surface_corners_land_on_the_layout_corners() {
        let m = hidpi_left();
        assert_eq!(m.to_desktop(pos2(100.0, 50.0)), [-1280.0, 0.0]);
        assert_eq!(m.to_desktop(pos2(740.0, 450.0)), [0.0, 800.0]);
        assert_eq!(m.to_desktop(pos2(420.0, 250.0)), [-640.0, 400.0]);
        assert_eq!(m.points_per_unit(), vec2(0.5, 0.5));
    }

    #[test]
    fn desktop_to_surface_is_the_inverse() {
        let m = hidpi_left();
        assert_eq!(m.to_surface((-1280.0, 0.0)), pos2(100.0, 50.0));
        assert_eq!(m.to_surface((-640.0, 400.0)), pos2(420.0, 250.0));
        for p in [pos2(101.5, 77.25), pos2(739.0, 51.0), pos2(300.0, 449.5)] {
            let d = m.to_desktop(p);
            let back = m.to_surface((d[0], d[1]));
            assert!((back - p).length() < 1e-3, "{p:?} -> {d:?} -> {back:?}");
        }
    }

    #[test]
    fn a_drag_past_the_seam_carries_on_into_the_next_monitor() {
        // Ten points right of this surface is twenty units into the monitor
        // at x = 0, which is how a drag held by one surface reaches the next.
        let m = hidpi_left();
        assert_eq!(m.to_desktop(pos2(750.0, 50.0)), [20.0, 0.0]);
        assert_eq!(m.to_surface((20.0, 0.0)), pos2(750.0, 50.0));
    }

    fn press(mods: ShortcutModifiers, key: &str) -> KeyPress {
        KeyPress {
            key: key.to_uppercase(),
            mods,
        }
    }

    fn editing(k: &KeyPress, keys: &EditorKeys) -> Option<Action> {
        action_for(
            k,
            keys,
            Stage::Editing,
            CaptureMode::Screenshot,
            Purpose::Capture,
        )
    }

    fn ready(k: &KeyPress, keys: &EditorKeys) -> Option<Action> {
        action_for(k, keys, Stage::Ready, CaptureMode::Video, Purpose::Capture)
    }

    const NONE: ShortcutModifiers = ShortcutModifiers::NONE;
    const CTRL: ShortcutModifiers = ShortcutModifiers::CTRL;
    const CTRL_SHIFT: ShortcutModifiers = ShortcutModifiers::CTRL_SHIFT;
    const SHIFT: ShortcutModifiers = ShortcutModifiers {
        super_key: false,
        ctrl: false,
        alt: false,
        shift: true,
    };

    #[test]
    fn the_default_keys_map_to_their_actions() {
        let k = EditorKeys::default();
        let a = |m, key| editing(&press(m, key), &k);
        assert_eq!(a(NONE, "Escape"), Some(Action::Cancel));
        assert_eq!(a(NONE, "Enter"), Some(Action::Copy));
        assert_eq!(a(CTRL, "S"), Some(Action::Save));
        assert_eq!(a(CTRL, "Z"), Some(Action::Undo));
        assert_eq!(a(CTRL_SHIFT, "Z"), Some(Action::Redo));
        assert_eq!(a(CTRL, "Y"), Some(Action::Redo));
        assert_eq!(a(NONE, "Z"), Some(Action::ToggleLoupe));
        assert_eq!(a(CTRL, "A"), Some(Action::SelectAll));
        assert_eq!(a(NONE, "Delete"), Some(Action::Delete));
    }

    #[test]
    fn both_redo_keys_work_whatever_redo_is_bound_to() {
        let k = EditorKeys {
            redo: Shortcut::new(ShortcutModifiers::CTRL, "R"),
            ..EditorKeys::default()
        };
        let a = |m, key| editing(&press(m, key), &k);
        assert_eq!(a(CTRL, "R"), Some(Action::Redo));
        assert_eq!(a(CTRL, "Y"), Some(Action::Redo));
        assert_eq!(a(CTRL_SHIFT, "Z"), Some(Action::Redo));
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let k = EditorKeys::default();
        let a = |m, key| editing(&press(m, key), &k);
        assert_eq!(a(NONE, "S"), None);
        assert_eq!(a(CTRL_SHIFT, "S"), None);
        assert_eq!(a(SHIFT, "Z"), None);
        assert_eq!(a(ShortcutModifiers { alt: true, ..CTRL }, "Z"), None);
        assert_eq!(a(NONE, "Q"), None);
    }

    #[test]
    fn arrows_nudge_by_one_and_by_ten_with_shift() {
        let k = EditorKeys::default();
        let a = |m, key| editing(&press(m, key), &k);
        assert_eq!(a(NONE, "Left"), Some(Action::Nudge { dx: -1.0, dy: 0.0 }));
        assert_eq!(a(NONE, "Up"), Some(Action::Nudge { dx: 0.0, dy: -1.0 }));
        assert_eq!(a(SHIFT, "Right"), Some(Action::Nudge { dx: 10.0, dy: 0.0 }));
        assert_eq!(a(SHIFT, "Down"), Some(Action::Nudge { dx: 0.0, dy: 10.0 }));
        assert_eq!(a(CTRL, "Down"), None);
    }

    #[test]
    fn escape_cancels_even_when_another_key_is_bound_to_it() {
        let k = EditorKeys {
            copy: Shortcut::new(NONE, "ESCAPE"),
            loupe: Shortcut::new(NONE, "ESCAPE"),
            ..Default::default()
        };
        assert_eq!(editing(&press(NONE, "Escape"), &k), Some(Action::Cancel));
    }

    #[test]
    fn rebound_keys_are_honoured_and_the_old_ones_do_nothing() {
        let k = EditorKeys {
            copy: Shortcut::new(CTRL, "C"),
            save: Shortcut::new(CTRL_SHIFT, "S"),
            loupe: Shortcut::new(NONE, "L"),
            ..Default::default()
        };
        let a = |m, key| editing(&press(m, key), &k);
        assert_eq!(a(CTRL, "C"), Some(Action::Copy));
        // Enter is only the default copy key, not a second one.
        assert_eq!(a(NONE, "Enter"), None);
        assert_eq!(a(CTRL_SHIFT, "S"), Some(Action::Save));
        assert_eq!(a(CTRL, "S"), None);
        assert_eq!(a(NONE, "L"), Some(Action::ToggleLoupe));
        assert_eq!(a(NONE, "Z"), None);
    }

    #[test]
    fn in_ready_enter_records_and_the_editor_keys_rest() {
        let k = EditorKeys::default();
        let a = |m, key| ready(&press(m, key), &k);
        assert_eq!(a(NONE, "Enter"), Some(Action::Record));
        assert_eq!(a(NONE, "Escape"), Some(Action::Cancel));
        assert_eq!(a(CTRL, "S"), None);
        assert_eq!(a(CTRL, "Z"), None);
        assert_eq!(a(CTRL, "Y"), None);
        assert_eq!(a(NONE, "Delete"), None);
        assert_eq!(a(NONE, "Z"), Some(Action::ToggleLoupe));
        assert_eq!(a(NONE, "Left"), Some(Action::Nudge { dx: -1.0, dy: 0.0 }));
        // Enter is Record even when copy is bound elsewhere.
        let k = EditorKeys {
            copy: Shortcut::new(CTRL, "C"),
            ..EditorKeys::default()
        };
        assert_eq!(ready(&press(NONE, "Enter"), &k), Some(Action::Record));
        assert_eq!(ready(&press(CTRL, "C"), &k), None);
    }

    #[test]
    fn enter_starts_steps_and_finishes_a_step_edit() {
        let k = EditorKeys {
            copy: Shortcut::new(CTRL, "C"),
            ..EditorKeys::default()
        };
        let enter = press(NONE, "Enter");
        let a = |stage, mode, purpose| action_for(&enter, &k, stage, mode, purpose);
        assert_eq!(
            a(Stage::Choosing, CaptureMode::Steps, Purpose::Capture),
            Some(Action::StartSteps)
        );
        assert_eq!(
            a(Stage::Choosing, CaptureMode::Screenshot, Purpose::Capture),
            None
        );
        assert_eq!(
            a(Stage::Editing, CaptureMode::Screenshot, Purpose::StepEdit),
            Some(Action::Copy)
        );
        assert_eq!(
            a(Stage::Editing, CaptureMode::Screenshot, Purpose::Capture),
            None
        );
    }

    #[test]
    fn egui_key_names_match_the_stored_shortcut_names() {
        // The recorder stores `key.name().to_uppercase()`; so must a press.
        let k = KeyPress::from_egui(egui::Key::Enter, egui::Modifiers::NONE);
        assert_eq!(k.key, EditorKeys::default().copy.key);
        let k = KeyPress::from_egui(egui::Key::S, egui::Modifiers::COMMAND);
        assert_eq!(editing(&k, &EditorKeys::default()), Some(Action::Save));
        let k = KeyPress::from_egui(egui::Key::ArrowLeft, egui::Modifiers::NONE);
        assert_eq!(
            editing(&k, &EditorKeys::default()),
            Some(Action::Nudge { dx: -1.0, dy: 0.0 })
        );
        for (key, name) in [
            (egui::Key::Escape, "ESCAPE"),
            (egui::Key::Delete, "DELETE"),
            (egui::Key::ArrowDown, "DOWN"),
        ] {
            assert_eq!(KeyPress::from_egui(key, egui::Modifiers::NONE).key, name);
        }
    }
}
