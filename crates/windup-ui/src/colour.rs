//! The colour wheel, ported from Umber's picker (`umber-app/src/colorpicker.rs`,
//! same owner, same licence). Only its Wheel mode: a hue ring around a
//! saturation/value triangle whose apex follows the hue. Umber's square centre,
//! angle and swap controls, sliders and harmony are left out, and so is every
//! control below the wheel; this draws the wheel and nothing else.
//!
//! HSV is the picker's state, not the colour. Deriving hue from RGB each frame
//! would lose it whenever saturation or value hits zero: drag the value to
//! black and the hue would snap back to red on the way out.
//!
//! A wheel carries two controls in one hit area, and they are kept apart by two
//! rules that have to hold together. [`Hub`] is the centre's geometry, handed to
//! the hit test and to the painting by one function, so a press cannot land on a
//! shape that is not drawn there. And [`settle`] runs **once, on the press
//! frame**, with [`frame`] holding its answer for the rest of the gesture; the
//! shape moving underneath afterwards therefore cannot change what the gesture
//! is. Each was a bug on its own, and the second is the subtler: a press on the
//! ring sets a hue, the hue swings the apex round to meet that very press, and a
//! wheel that asked again would read it as a press on the triangle: hue frozen,
//! marker slammed to the apex.
//!
//! What ends one gesture and begins the next is an **event**, the primary button
//! going down, and never a position. egui's `press_origin` is the pointer's
//! rather than any widget's, so it moves under a second button and is cleared by
//! a second release; asking it whether a new gesture began made a right-click
//! during a ring drag throw the marker at the pointer, and made a new press at a
//! pixel an abandoned gesture had used inherit that gesture's aim.

use crate::theme::{Palette, metrics};
use egui::{
    Color32, Mesh, Pos2, Response, Sense, Shape, Stroke, Ui, ViewportId, epaint::Vertex, pos2, vec2,
};

/// Degrees brought into `0.0..360.0`, the range a hue is defined over.
///
/// Two cases a bare `rem_euclid` gets wrong, and both end up in `sin_cos` and
/// then in vertex positions, where one NaN is a mesh egui discards whole:
///
/// * **Not a number.** `NaN.rem_euclid(360.0)` is `NaN`, and a hue is a plain
///   public field that anything may write.
/// * **A tiny negative.** `(-1e-7).rem_euclid(360.0)` rounds up to exactly
///   `360.0` in f32, the one value outside the range this promises.
pub fn wrap_hue(degrees: f32) -> f32 {
    if !degrees.is_finite() {
        return 0.0;
    }
    let wrapped = degrees.rem_euclid(360.0);
    if wrapped >= 360.0 { 0.0 } else { wrapped }
}

/// Hue/saturation/value, the space colour pickers are built on.
///
/// Defined over **sRGB** components rather than linear ones: a picker is a
/// perceptual instrument, and running HSV over linear values makes the
/// saturation/value triangle look badly bunched towards the dark end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hsv {
    /// Degrees, `0.0..360.0`.
    pub h: f32,
    /// `0.0..=1.0`.
    pub s: f32,
    /// `0.0..=1.0`.
    pub v: f32,
}

impl Hsv {
    pub fn new(h: f32, s: f32, v: f32) -> Self {
        Self {
            h: wrap_hue(h),
            s: s.clamp(0.0, 1.0),
            v: v.clamp(0.0, 1.0),
        }
    }

    /// sRGB bytes.
    pub fn to_rgb(self) -> [u8; 3] {
        let h = wrap_hue(self.h) / 60.0;
        let c = self.v * self.s;
        let x = c * (1.0 - (h % 2.0 - 1.0).abs());
        let m = self.v - c;
        let (r, g, b) = match h as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        [r + m, g + m, b + m].map(|c| (c * 255.0 + 0.5) as u8)
    }

    /// Decompose sRGB bytes. Hue is undefined for greys and comes back as 0;
    /// a picker holding a colour should use [`Self::with_rgb`] instead, or
    /// dragging value to zero would silently reset the hue to red.
    pub fn from_rgb(rgb: [u8; 3]) -> Self {
        let [r, g, b] = rgb.map(|c| c as f32 / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let h = if d <= f32::EPSILON {
            0.0
        } else if max == r {
            60.0 * (((g - b) / d) % 6.0)
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        Self {
            h: wrap_hue(h),
            s: if max <= f32::EPSILON { 0.0 } else { d / max },
            v: max,
        }
    }

    /// This state moved to a new colour, keeping its hue where the colour has
    /// none of its own (any grey, black and white included).
    pub fn with_rgb(self, rgb: [u8; 3]) -> Self {
        let new = Self::from_rgb(rgb);
        if new.s <= f32::EPSILON || new.v <= f32::EPSILON {
            Self { h: self.h, ..new }
        } else {
            new
        }
    }

    pub fn to_color32(self) -> Color32 {
        let [r, g, b] = self.to_rgb();
        Color32::from_rgb(r, g, b)
    }
}

/// The white ring every marker is drawn with. Not a palette colour: it has to
/// read against every hue the wheel can show.
const MARKER_COLOUR: Color32 = Color32::WHITE;

fn marker_stroke() -> Stroke {
    Stroke::new(metrics::WHEEL_MARKER_STROKE, MARKER_COLOUR)
}

const RING_SEGMENTS: usize = 96;

/// How wide a shape's edge is faded over, in points.
///
/// One physical pixel, which is what egui's own tessellator uses.
///
/// Both shapes are hand-built [`Mesh`]es rather than a [`Shape`] egui
/// tessellates, because neither is one flat colour and a tessellated shape has
/// exactly one. The cost is antialiasing: the tessellator extrudes a shape's
/// outline into a one-pixel skirt that fades to nothing, and a mesh handed to it
/// whole never goes through that step. So the meshes here carry their own skirt;
/// without it the ring's two circular edges and the triangle's three diagonals
/// come out as visible stair-steps.
///
/// This is the width of the *band*, not a distance to move a vertex by: the ring
/// pushes its vertices `f` along the radius, because a circle's normal is its
/// radius, and the triangle needs [`SKIRT_MITRE`] times as much.
fn feather(ui: &Ui) -> f32 {
    1.0 / ui.ctx().pixels_per_point().max(0.5)
}

/// The same colour with nothing left of it: the outer edge of a skirt.
///
/// Mesh vertex colours are premultiplied, so this is all zeroes and the
/// interpolation from the opaque vertex to it is a plain linear fade.
fn faded(colour: Color32) -> Color32 {
    Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), 0)
}

fn hue_colour(h: f32) -> Color32 {
    Hsv::new(h, 1.0, 1.0).to_color32()
}

/// How far out a press has to be to belong to the hue ring rather than to the
/// centre.
///
/// The hub's outermost 8% steers hue too, so the ring's inner edge is forgiving
/// to grab, but only where the centre's own shape is not already under the
/// pointer, which is why [`settle`] asks [`Hub::contains`] first. At the sizes
/// the picker is actually drawn at, a triangle inscribed at `inner - 3` puts its
/// corners *outside* this radius, so a ring tested first would take every press
/// on the apex: the corner that is the hue, and the one place on the shape
/// somebody aims at deliberately. (Below an `inner` of about 37 the corners fall
/// inside it instead. Nothing depends on which way round it is, since the hub is
/// asked first either way.)
const RING_GRIP: f32 = 0.92;

/// The triangle drawn in the middle of the wheel, as both the hit test and the
/// painting read it: its three corners, full hue, white and black.
///
/// One value rather than a shape and a hit test that agree by discipline: a
/// control may never be drawn where the hit test disagrees with it. The triangle
/// is turned by the hue itself, so a test that was right at one heading would be
/// this bug at another. A bounding box is right at *no* heading: round the
/// triangle it covers the three wide gaps between the shape and the ring, which
/// is what a press on the hue ring used to be read as. See [`frame`] for the
/// other half of it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Hub {
    hue_pt: Pos2,
    white_pt: Pos2,
    black_pt: Pos2,
}

impl Hub {
    /// Whether a press at `pos` landed on the shape that was drawn.
    ///
    /// Asked of the **press** alone. A drag that began inside goes on being the
    /// centre's however far out it wanders, which is what [`clamp_barycentric`]
    /// is for, so putting this test on each frame of a drag would freeze the
    /// marker at the edge instead of sliding it along.
    fn contains(self, pos: Pos2) -> bool {
        // A degenerate triangle answers NaN, which is neither inside nor out;
        // false is the honest reading and keeps the NaN out of a hue.
        let (x, y, z) = barycentric(pos, self.hue_pt, self.white_pt, self.black_pt);
        x.is_finite() && x >= 0.0 && y >= 0.0 && z >= 0.0
    }
}

/// The centre's geometry, from the ring it is inscribed in.
///
/// One function, so the hit test and the painting cannot disagree about where
/// the shape is.
fn hub_of(centre: Pos2, inner: f32, base: f32) -> Hub {
    let (hue_pt, white_pt, black_pt) = triangle_corners(centre, (inner - 3.0).max(1.0), base);
    Hub {
        hue_pt,
        white_pt,
        black_pt,
    }
}

/// Where the triangle's apex points, in radians from egui's zero angle (right,
/// y down): at the hue's own place on the ring.
///
/// Following the hue keeps the apex next to the marker that sets it, so the two
/// controls read as one instrument. Its own function because the hit test and the
/// painting both need it.
fn wheel_base(hue: f32) -> f32 {
    wrap_hue(hue).to_radians()
}

/// What egui reported about this frame's gesture on a wheel.
///
/// A plain reading rather than a [`Response`], so that [`frame`] below is a pure
/// function of it, the only way the frames a gesture is made of can be driven
/// without a window.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Reported {
    /// Where the pointer is, if this widget owns a gesture at all. `Some` from
    /// the frame of the press, *before* egui has decided the gesture is a drag,
    /// through to the frame of the release.
    at: Option<Pos2>,
    /// Where the button went down, while it is still down. egui clears it on
    /// the release.
    ///
    /// Read **only** to give a press frame its origin. It is the *pointer's* and
    /// not this widget's (any button moves it and any release clears it), so it
    /// can say where a gesture began and must never be asked whether one began.
    /// That is [`Self::pressed`]'s job.
    press_origin: Option<Pos2>,
    /// Did the **primary** button go down this frame?
    ///
    /// This is the only thing that starts a wheel gesture, so it is the only
    /// honest reading of "a new one began". `any_pressed` would be true for a
    /// right button pressed in the middle of a left drag. It is safe under touch
    /// as well: `egui-winit` synthesises pointer buttons for the *first* contact
    /// only, so a second finger cannot raise it.
    pressed: bool,
}

/// Which control a gesture reached, with no position in it.
///
/// Positionless because it is settled **once**, at the press, and then held:
/// where the pointer is afterwards is this frame's business and which control
/// it is working is not.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Aim {
    /// Neither control: the gaps between the triangle and the ring, and
    /// everything beyond the ring's outer edge. Nothing is drawn in either, so
    /// nothing in either may move a colour.
    #[default]
    Nothing,
    /// The hue ring.
    Ring,
    /// The saturation and value triangle.
    Centre,
}

impl Aim {
    /// This aim, working at a position.
    fn at(self, pos: Pos2) -> WheelAim {
        match self {
            Self::Nothing => WheelAim::Idle,
            Self::Ring => WheelAim::Ring(pos),
            Self::Centre => WheelAim::Centre(pos),
        }
    }
}

/// What a wheel does with this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Frame {
    /// No gesture. Anything recorded belongs to one that has ended.
    Idle,
    /// A gesture already settled, now at this position.
    Held(Aim, Pos2),
    /// The first frame of a gesture: settle it from this origin, record the
    /// answer, and work it at this position.
    Press(Pos2, Pos2),
}

/// Which of those three this frame is, given what egui reported and what the
/// widget recorded on an earlier frame of the same gesture.
///
/// The wheel's two controls overlap, so which of them a gesture belongs to is
/// decided **once, at the press**, and held for the whole gesture. That is what
/// lets a hue drag carry on across the middle, what stops a drag out of the
/// centre being handed to the ring when it is let go over one, and, the reason
/// holding it is not merely tidy, what stops the *shape itself* changing the
/// answer. The apex follows the hue, so a press on the ring sets a hue that
/// swings the apex round to meet the very point it was pressed at: re-asking
/// [`Hub::contains`] on the next frame then reads that press as a press on the
/// triangle, the hue freezes, and the saturation and value marker slams to the
/// apex. That is the bug, in the 2 px band [`RING_GRIP`] exists to serve, and
/// holding the aim is the whole of the cure.
///
/// Two more frames egui makes awkward. A press is not yet a *drag* (egui waits
/// for the pointer to travel) and a release is no longer one, so a reading that
/// asked for a drag fell through to the pointer's own position on both, and
/// handed a press on the ring to the triangle. Here the press frame settles the
/// aim and the release frame is `Held`, because egui clears `press_origin` on
/// the release and `held` is what is left.
///
/// A press and a release inside one frame is `Press` with the pointer's own
/// position for the origin, which it is: there has been no frame in which to
/// move.
///
/// **A new gesture is an event, never a position.** Comparing egui's
/// `press_origin` against the origin the record was settled at is wrong in both
/// directions. It reads a *second* button pressed mid-drag as a new gesture, and
/// it reads a genuinely new press at the *same pixel* as the old gesture, so a
/// record abandoned by the wheel not being drawn for a while was inherited, aim
/// and all. [`Reported::pressed`] separates the two readings: a record is kept
/// unless the primary button actually went down.
fn frame(reported: Reported, held: Option<Aim>) -> Frame {
    let Some(at) = reported.at else {
        return Frame::Idle;
    };
    match held {
        // Still the gesture this widget settled: nothing has started a new one.
        Some(aim) if !reported.pressed => Frame::Held(aim, at),
        _ => Frame::Press(reported.press_origin.unwrap_or(at), at),
    }
}

/// Which of a wheel's two controls this frame's gesture is working, and where.
#[derive(Clone, Copy, Debug, PartialEq)]
enum WheelAim {
    /// Nothing to do.
    Idle,
    /// The hue ring, at this position.
    Ring(Pos2),
    /// The saturation and value triangle, at this position.
    Centre(Pos2),
}

impl WheelAim {
    /// Where the centre should follow the pointer, if it should at all.
    fn centre(self) -> Option<Pos2> {
        match self {
            Self::Centre(pos) => Some(pos),
            Self::Idle | Self::Ring(_) => None,
        }
    }
}

/// Settle a press against the two controls.
///
/// The one place the hub is consulted, which is what makes the answer a property
/// of the press rather than of whatever the shape happens to be doing a frame
/// later; see [`frame`].
///
/// The hub is asked **first** and the ring second: [`RING_GRIP`] lets the ring be
/// grabbed from a little inside its own edge, and the triangle's corners reach
/// into that band, so a ring tested first would take every press on the apex.
///
/// And the ring has an outer edge as well as an inner one. The wheel's hit area
/// is the square around it, so its four corners reach `outer × √2` with nothing
/// drawn out there; a press in one used to set a hue. The bound is `outer`
/// exactly, where [`hue_ring`] fades a skirt one feather *past* it, so the one
/// faded pixel at the rim takes no press. That is the conservative direction.
fn settle(centre: Pos2, inner: f32, outer: f32, hub: Hub, from: Pos2) -> Aim {
    if hub.contains(from) {
        return Aim::Centre;
    }
    let out = (from - centre).length();
    if out > inner * RING_GRIP && out <= outer {
        Aim::Ring
    } else {
        Aim::Nothing
    }
}

/// [`frame`] and [`settle`], against what egui and the previous frame have to
/// say: one frame of a wheel gesture, what to do now and what to remember.
///
/// **The whole of the dispatch, in one place.** [`wheel_aim`] and the tests used
/// to hold a copy each, and the copy was exactly the thing this file's bug lives
/// in: re-settling the aim per frame could be put back into the *shipped* path
/// with the whole suite green, because the guard only ever drove the harness's
/// twin.
///
/// The aim to keep comes back rather than being stored here, because the two
/// callers store it differently (egui's memory in the picker, a field in the
/// tests).
fn resolve(
    reported: Reported,
    held: Option<Aim>,
    centre: Pos2,
    inner: f32,
    outer: f32,
    hub: Hub,
) -> (Option<Aim>, WheelAim) {
    match frame(reported, held) {
        Frame::Idle => (None, WheelAim::Idle),
        Frame::Held(aim, at) => (Some(aim), aim.at(at)),
        Frame::Press(from, at) => {
            let aim = settle(centre, inner, outer, hub, from);
            (Some(aim), aim.at(at))
        }
    }
}

/// Where a wheel's settled aim is kept in egui's memory.
///
/// Memory is the context's, and a context has every viewport in it, so the
/// widget id alone can be the same in two windows (two panels laid out alike
/// derive the same ids). The viewport is part of the key so that a gesture in one
/// window cannot be held, or cleared, by a wheel in the other.
fn aim_key(id: egui::Id, viewport: ViewportId) -> egui::Id {
    id.with(viewport)
}

/// [`resolve`], reading egui and keeping its answer in this widget's memory.
///
/// The settled aim is recorded on its press frame and cleared on the first frame
/// this widget owns no gesture. A record can therefore outlive its gesture only
/// where no such frame ran (the wheel not being drawn mid-drag), and a stale one
/// is harmless because [`frame`] settles afresh whenever the primary button goes
/// down.
fn wheel_aim(
    ui: &Ui,
    id: egui::Id,
    response: &Response,
    centre: Pos2,
    inner: f32,
    outer: f32,
    hub: Hub,
) -> WheelAim {
    let key = aim_key(id, ui.ctx().viewport_id());
    let reported = ui.ctx().input(|i| Reported {
        at: response.interact_pointer_pos(),
        press_origin: i.pointer.press_origin(),
        pressed: i.pointer.primary_pressed(),
    });
    let held = ui.ctx().data_mut(|d| d.get_temp::<Aim>(key));
    let (keep, aimed) = resolve(reported, held, centre, inner, outer, hub);
    ui.ctx().data_mut(|d| match keep {
        // Written when it changes, which is once per gesture: the aim cannot
        // move within one, and `insert_temp` boxes what it is given.
        Some(aim) if held != Some(aim) => {
            d.insert_temp(key, aim);
        }
        None => {
            d.remove_temp::<Aim>(key);
        }
        Some(_) => {}
    });
    aimed
}

/// Draw the wheel, `size` points on a side. Returns true when the colour
/// changed.
///
/// `_p` is unused: the wheel is all hue and white markers, nothing the palette
/// colours. It stays in the signature so a caller does not change if a ring
/// outline ever has to read against the theme.
pub fn wheel(ui: &mut Ui, _p: &Palette, hsv: &mut Hsv, size: f32) -> bool {
    let mut changed = false;

    let size = size.max(metrics::WHEEL_MIN);
    let id = ui.auto_id_with("wheel");
    let (area, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let centre = area.center();
    let (inner, outer) = ring_radii(size);

    // One interaction for the whole wheel, and where a gesture was *pressed*
    // decides which of the two controls inside it the gesture belongs to.
    //
    // Two overlapping `ui.interact` rects cannot do it: egui hands a press to the
    // topmost widget under it, and the centre's rect is the square around a shape
    // inscribed in the ring, so its corners cover the ring at the diagonals and a
    // hue drag begun there went to the saturation and value field instead.
    // Settling it at the press rather than per frame is also what lets a drag
    // begun on the ring carry on across the middle.
    let response = ui.interact(area, id, Sense::click_and_drag());

    // The centre as it stands *now*, which is the shape a press this frame
    // landed on. The hue the ring is about to be given belongs to a triangle
    // nobody has seen yet, and judging a press against that one would be judging
    // it by where the shape is going. It is read only on a press frame (see
    // `frame`), so the shape swinging round afterwards cannot take the gesture
    // off the ring.
    let aimed = wheel_aim(
        ui,
        id,
        &response,
        centre,
        inner,
        outer,
        hub_of(centre, inner, wheel_base(hsv.h)),
    );

    // --- hue ring ---
    if let WheelAim::Ring(pos) = aimed {
        hsv.h = ring_hue(centre, pos);
        changed = true;
    }

    hue_ring(ui, centre, inner, outer);

    // Hue marker, on the middle of the ring wherever the ring ended up.
    ui.painter().circle_stroke(
        ring_point(centre, inner, outer, hsv.h),
        metrics::WHEEL_HUE_MARKER,
        marker_stroke(),
    );

    // --- saturation / value triangle ---
    //
    // Rebuilt from the hue as it now is, so the triangle turns on the frame the
    // ring is dragged rather than a frame behind the marker that turned it.
    changed |= sv_triangle(
        ui,
        hub_of(centre, inner, wheel_base(hsv.h)),
        aimed.centre(),
        hsv,
    );

    changed
}

/// The hue ring's two radii for a wheel of the given side.
///
/// The ring is a fixed thickness, so a small enough wheel would have its inner
/// edge outside its outer one; below about 54 points the hub is kept instead and
/// the band narrows.
fn ring_radii(size: f32) -> (f32, f32) {
    let outer = size * 0.5;
    ((outer - metrics::WHEEL_RING).max(outer * 0.25), outer)
}

/// The middle of the ring at a given hue: where a marker for that hue goes.
fn ring_point(centre: Pos2, inner: f32, outer: f32, hue: f32) -> Pos2 {
    let a = hue.to_radians();
    centre + vec2(a.cos(), a.sin()) * (inner + outer) * 0.5
}

/// The hue a point on the ring stands for: [`ring_point`]'s inverse, and the only
/// thing in this file that writes a hue from a gesture.
///
/// Through [`wrap_hue`] rather than a bare `rem_euclid`. `atan2` answers in
/// `-π..=π`, and `(-1e-7f32).to_degrees().rem_euclid(360.0)` is exactly `360.0`:
/// a hue one step anticlockwise of red, held in the one value the type says it
/// never holds, which `to_rgb` reads as the sixth sextant and paints magenta.
fn ring_hue(centre: Pos2, at: Pos2) -> f32 {
    let d = at - centre;
    wrap_hue(d.y.atan2(d.x).to_degrees())
}

/// The hue ring itself, drawn between two radii about `centre`. See [`feather`]
/// for why the mesh carries its own skirt at all.
fn hue_ring(ui: &Ui, centre: Pos2, inner: f32, outer: f32) {
    let f = feather(ui);
    // Enough segments that the facets are shorter than the feather is wide,
    // otherwise a smooth edge is drawn along a visibly polygonal outline. Scaled
    // to the wheel rather than fixed.
    let segments = ((outer * 0.9) as usize).clamp(RING_SEGMENTS, 320);

    let mut mesh = Mesh::default();
    // Four radii across the ring: a transparent skirt, the ring itself, and
    // another skirt. See `feather`.
    let radii = [
        ((inner - f).max(0.0), false),
        (inner, true),
        (outer, true),
        (outer + f, false),
    ];
    for i in 0..segments {
        let a0 = (i as f32 / segments as f32) * std::f32::consts::TAU;
        let a1 = ((i + 1) as f32 / segments as f32) * std::f32::consts::TAU;

        let base = mesh.vertices.len() as u32;
        for angle in [a0, a1] {
            let dir = vec2(angle.cos(), angle.sin());
            let colour = hue_colour(angle.to_degrees());
            for (radius, solid) in radii {
                mesh.vertices.push(Vertex {
                    pos: centre + dir * radius,
                    uv: egui::epaint::WHITE_UV,
                    color: if solid { colour } else { faded(colour) },
                });
            }
        }
        // Three bands between the four radii, each a quad spanning the segment.
        for band in 0..radii.len() as u32 - 1 {
            let l = base + band;
            let r = base + radii.len() as u32 + band;
            mesh.indices
                .extend_from_slice(&[l, l + 1, r, l + 1, r + 1, r]);
        }
    }
    ui.painter().add(Shape::mesh(mesh));
}

/// What saturation and value a point in the triangle stands for, and `None`
/// where the three corners are not a triangle at all.
///
/// Clamped rather than refused, for the reason [`clamp_barycentric`] gives: a
/// drag past an edge slides along it instead of freezing the picker. Whether a
/// point is *in* the triangle is [`Hub::contains`]'s question and is asked of the
/// press alone; these are two different questions and the clamp is not an answer
/// to the first.
fn triangle_at(hue_pt: Pos2, white_pt: Pos2, black_pt: Pos2, pos: Pos2) -> Option<(f32, f32)> {
    // Barycentric coordinates give saturation and value directly.
    let (a, b, c) = barycentric(pos, hue_pt, white_pt, black_pt);
    if !a.is_finite() {
        return None;
    }
    let (a, b, _) = clamp_barycentric(a, b, c);
    let v = (a + b).clamp(0.0, 1.0);
    let s = if v > 1e-4 {
        (a / v).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Some((s, v))
}

/// Saturation/value triangle, drawn at the three corners [`hub_of`] placed.
///
/// The apex is the full hue, with white and black at the other two corners. It
/// points where the hue is, which keeps the apex next to the marker that sets it,
/// at the price that the whole field swings under the pointer while the ring is
/// being dragged.
///
/// The corners are passed in rather than built here, because the hit test needs
/// exactly these three points; see [`Hub`]. `drag` is where the pointer is, if
/// this frame's gesture belongs to the centre. The wheel decides that, so there
/// is no `interact` of its own to overlap the ring's.
fn sv_triangle(ui: &mut Ui, hub: Hub, drag: Option<Pos2>, hsv: &mut Hsv) -> bool {
    let Hub {
        hue_pt,
        white_pt,
        black_pt,
    } = hub;
    let mut changed = false;

    if let Some(pos) = drag
        && let Some((s, v)) = triangle_at(hue_pt, white_pt, black_pt, pos)
    {
        hsv.s = s;
        hsv.v = v;
        changed = true;
    }

    let f = feather(ui);
    let corners = [
        (hue_pt, hue_colour(hsv.h)),
        (white_pt, Color32::WHITE),
        (black_pt, Color32::BLACK),
    ];
    // The centroid gives each corner its outward direction. For an equilateral
    // triangle that direction is also the corner's bisector, which is what
    // [`skirt_corner`] needs.
    let centroid = pos2(
        corners.iter().map(|(pt, _)| pt.x).sum::<f32>() / 3.0,
        corners.iter().map(|(pt, _)| pt.y).sum::<f32>() / 3.0,
    );

    let painter = ui.painter();
    let mut mesh = Mesh::default();
    // 0..3 is the triangle; 3..6 is the transparent skirt outside it. See
    // `feather` for why it is here rather than left to egui.
    for (pt, colour) in corners {
        mesh.vertices.push(Vertex {
            pos: pt,
            uv: egui::epaint::WHITE_UV,
            color: colour,
        });
    }
    for (pt, colour) in corners {
        mesh.vertices.push(Vertex {
            pos: skirt_corner(pt, centroid, f),
            uv: egui::epaint::WHITE_UV,
            color: faded(colour),
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2]);
    for i in 0..3u32 {
        let j = (i + 1) % 3;
        mesh.indices
            .extend_from_slice(&[i, j, j + 3, i, j + 3, i + 3]);
    }
    painter.add(Shape::mesh(mesh));

    // Marker: rebuild the point from the current saturation and value.
    let a = hsv.s * hsv.v;
    let b = hsv.v - a;
    let c = 1.0 - hsv.v;
    let marker = pos2(
        hue_pt.x * a + white_pt.x * b + black_pt.x * c,
        hue_pt.y * a + white_pt.y * b + black_pt.y * c,
    );
    painter.circle_stroke(marker, metrics::WHEEL_SV_MARKER, marker_stroke());

    changed
}

/// The triangle's three corners, in the order the rest of this file reads them:
/// full hue, white, black.
///
/// Everything else about the triangle (the hit test, the mesh, the marker) is
/// derived from these three points, so testing it without a `Ui` is testing the
/// feature. [`Hub`] is what carries them from here to both.
fn triangle_corners(centre: Pos2, radius: f32, base: f32) -> (Pos2, Pos2, Pos2) {
    let corner = |k: f32| {
        let a = base + k * std::f32::consts::TAU / 3.0;
        centre + vec2(a.cos(), a.sin()) * radius
    };
    (corner(0.0), corner(1.0), corner(2.0))
}

/// How much further out a corner of the triangle goes than its edges do.
///
/// The ring's skirt is easy, and that is exactly why the triangle's looked like
/// it had one when it did not. A circle's outward *normal* is its radial
/// direction, so a vertex pushed a feather out from the centre carries its edge a
/// feather out with it. A triangle's corners are not its edges. Pushing a corner
/// out along the line from the centroid scales the whole shape about that point,
/// and an equilateral triangle's inradius is *half* its circumradius, so a corner
/// moved by `f` moves each of the three edges by only `f/2`. The diagonals were
/// feathered over half a pixel while the ring got a whole one, which is why they
/// went on stair-stepping.
///
/// What an edge needs is to be offset `f` along its own normal, and the corner
/// then goes where two such offset edges meet: `f / sin(θ/2)` along the bisector,
/// for an interior angle θ. [`triangle_corners`] places its three points a third
/// of a turn apart on a circle, so this triangle is *always* equilateral: θ is
/// always 60°, `sin(30°)` is exactly a half, and the factor is exactly 2.
const SKIRT_MITRE: f32 = 2.0;

/// One corner of the skirt: the triangle's corner pushed out along its bisector
/// far enough that all three edges move out by exactly `f`.
///
/// A triangle squashed to nothing has no outward direction, and normalising it
/// would put NaN into the mesh, which egui discards whole, so the picker would
/// simply stop drawing.
fn skirt_corner(pt: Pos2, centroid: Pos2, f: f32) -> Pos2 {
    let away = pt - centroid;
    if away.length() > 1e-3 {
        pt + away.normalized() * f * SKIRT_MITRE
    } else {
        pt
    }
}

fn barycentric(p: Pos2, a: Pos2, b: Pos2, c: Pos2) -> (f32, f32, f32) {
    let v0 = b - a;
    let v1 = c - a;
    let v2 = p - a;
    let den = v0.x * v1.y - v1.x * v0.y;
    if den.abs() < 1e-6 {
        return (f32::NAN, f32::NAN, f32::NAN);
    }
    let v = (v2.x * v1.y - v1.x * v2.y) / den;
    let w = (v0.x * v2.y - v2.x * v0.y) / den;
    (1.0 - v - w, v, w)
}

/// Push a point inside the triangle rather than rejecting it, so dragging past
/// an edge slides along it instead of freezing the picker.
fn clamp_barycentric(a: f32, b: f32, c: f32) -> (f32, f32, f32) {
    let (a, b, c) = (a.max(0.0), b.max(0.0), c.max(0.0));
    let sum = a + b + c;
    if sum <= 1e-6 {
        (1.0, 0.0, 0.0)
    } else {
        (a / sum, b / sum, c / sum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CENTRE: Pos2 = pos2(100.0, 100.0);
    const RADIUS: f32 = 50.0;

    /// The ring the widest wheel the picker draws actually has, so every margin
    /// below is checked at a size the picker can be at.
    const SIZE: f32 = 176.0;
    const OUTER: f32 = SIZE * 0.5;
    const INNER: f32 = OUTER - metrics::WHEEL_RING;

    /// The triangle as the wheel builds it, resolved through the one function
    /// that decides.
    fn corners(hue: f32) -> (Pos2, Pos2, Pos2) {
        triangle_corners(CENTRE, RADIUS, wheel_base(hue))
    }

    fn hub_at(hue: f32) -> Hub {
        hub_of(CENTRE, INNER, wheel_base(hue))
    }

    /// A point on the hue ring, at a given fraction across the band.
    fn on_the_ring(degrees: f32, across: f32) -> Pos2 {
        let a = degrees.to_radians();
        CENTRE + vec2(a.cos(), a.sin()) * (INNER + (OUTER - INNER) * across)
    }

    fn hub_corners(hub: Hub) -> [Pos2; 3] {
        [hub.hue_pt, hub.white_pt, hub.black_pt]
    }

    /// [`wheel`]'s own loop with the `Ui` and the painting taken out: the hub
    /// rebuilt from the hue every frame exactly as the real one rebuilds it,
    /// the frame settled, then whichever of the picker's own readers the aim
    /// names.
    ///
    /// Rebuilding the hub per frame is the point. A triangle following the hue
    /// *moves while the ring is being dragged*, and a test that held one still
    /// could not see the gesture being taken off the ring by the shape swinging
    /// round to meet the press. `hub_of`, `wheel_base`, `frame`, `settle`,
    /// `ring_hue` and `triangle_at` are the functions the running picker calls.
    struct Wheel {
        held: Option<Aim>,
        hsv: Hsv,
    }

    impl Wheel {
        fn new(hsv: Hsv) -> Self {
            Self { held: None, hsv }
        }

        fn hub(&self) -> Hub {
            hub_at(self.hsv.h)
        }

        fn step(&mut self, reported: Reported) -> WheelAim {
            // `resolve` and not a copy of it: the dispatch that holds the aim is
            // the thing under test.
            let (keep, aimed) = resolve(reported, self.held, CENTRE, INNER, OUTER, self.hub());
            self.held = keep;
            match aimed {
                WheelAim::Idle => {}
                WheelAim::Ring(pos) => self.hsv.h = ring_hue(CENTRE, pos),
                // Rebuilt after the hue, as `wheel` does.
                WheelAim::Centre(pos) => {
                    let hub = self.hub();
                    if let Some((s, v)) = triangle_at(hub.hue_pt, hub.white_pt, hub.black_pt, pos) {
                        self.hsv.s = s;
                        self.hsv.v = v;
                    }
                }
            }
            aimed
        }

        fn press(&mut self, at: Pos2) -> WheelAim {
            self.step(Reported {
                at: Some(at),
                press_origin: Some(at),
                pressed: true,
            })
        }

        fn drag(&mut self, from: Pos2, at: Pos2) -> WheelAim {
            self.step(Reported {
                at: Some(at),
                press_origin: Some(from),
                pressed: false,
            })
        }

        /// The release, on which egui has already cleared its own origin.
        fn release(&mut self, at: Pos2) -> WheelAim {
            self.step(Reported {
                at: Some(at),
                press_origin: None,
                pressed: false,
            })
        }

        /// A *second* button pressed mid-drag. egui keeps this widget's
        /// interaction, moves `press_origin` to wherever the pointer is, and
        /// does not raise `primary_pressed`.
        fn second_button_down(&mut self, at: Pos2) -> WheelAim {
            self.step(Reported {
                at: Some(at),
                press_origin: Some(at),
                pressed: false,
            })
        }

        /// And its release, which clears egui's origin while the primary button
        /// is still down.
        fn second_button_up(&mut self, at: Pos2) -> WheelAim {
            self.step(Reported {
                at: Some(at),
                press_origin: None,
                pressed: false,
            })
        }

        /// A frame on which this widget owns no gesture, which clears the record.
        fn idle(&mut self) -> WheelAim {
            self.step(Reported::default())
        }
    }

    fn apart(a: Pos2, b: Pos2) -> f32 {
        (a - b).length()
    }

    // --- Hsv ---

    #[test]
    fn hsv_round_trips_through_rgb() {
        for rgb in [
            [255, 0, 0],
            [12, 200, 90],
            [0, 0, 0],
            [255, 255, 255],
            [37, 99, 180],
        ] {
            let back = Hsv::from_rgb(rgb).to_rgb();
            for (a, b) in rgb.iter().zip(back) {
                assert!((*a as i32 - b as i32).abs() <= 1, "{rgb:?} -> {back:?}");
            }
        }
        assert_eq!(Hsv::new(0.0, 1.0, 1.0).to_rgb(), [255, 0, 0]);
        assert_eq!(Hsv::new(120.0, 1.0, 1.0).to_rgb(), [0, 255, 0]);
        assert_eq!(Hsv::new(240.0, 1.0, 1.0).to_rgb(), [0, 0, 255]);
    }

    /// Dragging value to black or saturation to nothing must not send the hue
    /// back to red on the way out: the hue is the picker's state, not something
    /// to be read off the colour each frame.
    #[test]
    fn grey_keeps_its_hue() {
        let mut wheel = Wheel::new(Hsv::new(200.0, 0.6, 0.8));
        // Straight onto the black corner, then the white one.
        let hub = hub_at(200.0);
        for corner in [hub.black_pt, hub.white_pt] {
            assert!(matches!(wheel.press(corner), WheelAim::Centre(_)));
            wheel.release(corner);
            assert_eq!(wheel.hsv.h, 200.0, "the centre moved the hue");
        }
        assert_eq!(wheel.hsv.s, 0.0);

        // And through the byte round trip: a grey has no hue of its own, so the
        // state's hue is what survives.
        let kept = Hsv::new(200.0, 0.0, 0.5).with_rgb([128, 128, 128]);
        assert_eq!(kept.h, 200.0);
        assert_eq!(kept.s, 0.0);
        let black = Hsv::new(200.0, 0.7, 0.4).with_rgb([0, 0, 0]);
        assert_eq!(black.h, 200.0);
        // A colour that has a hue takes it.
        assert!((Hsv::new(200.0, 0.0, 0.5).with_rgb([255, 0, 0]).h).abs() < 1e-3);
        // Without a state to keep, a grey reads as hue 0.
        assert_eq!(Hsv::from_rgb([90, 90, 90]).h, 0.0);
    }

    #[test]
    fn a_hue_is_always_inside_its_range() {
        assert_eq!(wrap_hue(360.0), 0.0);
        assert_eq!(wrap_hue(-90.0), 270.0);
        assert_eq!(wrap_hue(f32::NAN), 0.0);
        assert_eq!(wrap_hue(f32::INFINITY), 0.0);
        // `(-1e-7).rem_euclid(360.0)` rounds up to exactly 360.
        assert!((0.0..360.0).contains(&wrap_hue(-1e-7)));
        assert_eq!(Hsv::new(f32::NAN, 2.0, -1.0), Hsv::new(0.0, 1.0, 0.0));
    }

    // --- geometry ---

    #[test]
    fn the_triangle_follows_the_hue() {
        let (red, ..) = corners(0.0);
        let (green, ..) = corners(120.0);
        // A third of a turn round the ring is a third of a turn of the apex.
        assert!(apart(red, green) > RADIUS, "{red:?} vs {green:?}");
        // And a full turn brings it back to where it started.
        let (round_again, ..) = corners(360.0);
        assert!(apart(red, round_again) < 1e-3);
        // The apex is on the ring's side of the hue: at 270 it points up.
        let (up, ..) = corners(270.0);
        assert!(up.y < CENTRE.y - RADIUS + 1e-3 && (up.x - CENTRE.x).abs() < 1e-3);
    }

    /// The ring's own inverse, and the trap it exists for: a point a hair
    /// anticlockwise of red is `-1e-7` radians, whose degrees `rem_euclid`
    /// rounds up to exactly 360, which `to_rgb` would read as magenta.
    #[test]
    fn the_ring_reads_back_the_hue_its_marker_was_drawn_at() {
        for hue in [0.0_f32, 1.0, 90.0, 179.9, 270.0, 359.9] {
            let at = ring_point(CENTRE, 40.0, 60.0, hue);
            let back = ring_hue(CENTRE, at);
            let apart = (back - hue).abs().min(360.0 - (back - hue).abs());
            assert!(apart < 1e-2, "{hue} -> {back}");
        }
        let just_under = ring_hue(CENTRE, CENTRE + vec2(1.0, -1e-7));
        assert!((0.0..360.0).contains(&just_under), "{just_under}");
        assert_eq!(
            Hsv::new(just_under, 1.0, 1.0).to_rgb(),
            [255, 0, 0],
            "a hair off red must not be magenta"
        );
        // And the centre itself, where there is no direction at all.
        assert!(ring_hue(CENTRE, CENTRE).is_finite());
    }

    // --- gestures ---

    /// Turning the hue ring must not move the saturation and value marker,
    /// towards whatever direction the ring had been grabbed from. A whole turn,
    /// a degree at a time, from a press on the ring: the press frame, every
    /// frame of the drag and the release are one gesture. Saturation and value
    /// come out bit for bit as they went in.
    ///
    /// The triangle's apex chases the hue the ring is setting, so a wheel that
    /// re-settled its aim per frame reads the press as a press on the triangle
    /// the moment the apex reaches it. Pressed at the ring's *inner* edge,
    /// which is the press the apex can swing round to.
    #[test]
    fn turning_the_hue_through_a_whole_circle_leaves_the_saturation_and_value_alone() {
        for pressed_at in [0.0_f32, 45.0, 137.0, 250.0] {
            let start = Hsv::new(0.0, 0.375, 0.625);
            let mut wheel = Wheel::new(start);
            let press = on_the_ring(pressed_at, 0.0);
            let mut hues = Vec::new();

            let check = |aimed: WheelAim, wheel: &Wheel, what: &str| {
                assert!(
                    matches!(aimed, WheelAim::Ring(_)),
                    "pressed at {pressed_at}°: {what} left the ring as {aimed:?}"
                );
                assert_eq!(
                    (wheel.hsv.s, wheel.hsv.v),
                    (start.s, start.v),
                    "pressed at {pressed_at}°: {what} moved the marker"
                );
            };

            let aimed = wheel.press(press);
            check(aimed, &wheel, "the press");
            for step in 0..=360 {
                let aimed = wheel.drag(press, on_the_ring(step as f32, 0.5));
                check(aimed, &wheel, "a drag frame");
                assert!(
                    (0.0..360.0).contains(&wheel.hsv.h),
                    "{} is not a hue",
                    wheel.hsv.h
                );
                hues.push(wheel.hsv.h.round() as i32);
            }
            // A drag begun on the ring carries on across the middle, and is
            // still the ring's when it is let go there.
            let aimed = wheel.drag(press, CENTRE);
            check(aimed, &wheel, "crossing the middle");
            let aimed = wheel.release(CENTRE);
            check(aimed, &wheel, "the release");

            // The hue went where it was dragged, rather than a control that
            // holds still and passes the assertions above for free.
            hues.sort_unstable();
            hues.dedup();
            assert!(hues.len() > 300, "the sweep reached {} hues", hues.len());
        }
    }

    /// A press in the band between [`RING_GRIP`]'s forgiving edge and the ring
    /// itself is the ring's. That press sets a hue, and the hue points the apex
    /// *at the press*: the pressed point is then inside the triangle. A wheel
    /// that asked again would hand the rest of the gesture to the saturation and
    /// value field, the hue would freeze, and the marker would slam to the apex.
    #[test]
    fn a_hue_the_press_itself_set_cannot_turn_the_shape_onto_that_press() {
        let start = Hsv::new(0.0, 0.4, 0.6);
        for across in [0.005_f32, 0.015, 0.03] {
            let at = |degrees: f32| {
                let a = degrees.to_radians();
                CENTRE + vec2(a.cos(), a.sin()) * (INNER * RING_GRIP + across * INNER)
            };
            // 340° crosses the wrap on the way round.
            for degrees in [7.0_f32, 100.0, 250.0, 340.0] {
                let press = at(degrees);
                assert!(
                    (press - CENTRE).length() > INNER * RING_GRIP,
                    "not the grip band"
                );
                let mut wheel = Wheel::new(start);
                assert!(matches!(wheel.press(press), WheelAim::Ring(_)));
                // The apex is now at `degrees`, so the press is inside the
                // triangle, which is exactly the trap.
                assert!(
                    wheel.hub().contains(press),
                    "at {degrees}° the apex did not reach the press, so this proves nothing"
                );
                for step in 1..=8 {
                    let aimed = wheel.drag(press, at(degrees + step as f32 * 5.0));
                    assert!(
                        matches!(aimed, WheelAim::Ring(_)),
                        "{degrees}°, frame {step}: the triangle took the drag"
                    );
                }
                assert_eq!((wheel.hsv.s, wheel.hsv.v), (start.s, start.v));
                let apart = (wheel.hsv.h - degrees).abs();
                let moved = apart.min(360.0 - apart);
                assert!(moved > 30.0, "the hue froze at {}", wheel.hsv.h);
            }
        }
    }

    /// The press has to land on the shape that is *drawn*, not on the square
    /// around the ring it is inscribed in.
    #[test]
    fn only_the_shape_that_is_drawn_takes_a_press() {
        for hue in [0.0_f32, 37.0, 90.0, 214.0] {
            let hub = hub_at(hue);
            assert!(hub.contains(CENTRE), "the triangle at {hue}° is empty");
            // Nowhere on the ring is inside the centre. Stopping a hair short of
            // the outer edge: `length()` of a point placed by `cos`/`sin` at
            // exactly that radius rounds either way.
            for across in [0.0_f32, 0.5, 0.98] {
                for step in 0..72 {
                    let at = on_the_ring(step as f32 * 5.0, across);
                    assert!(
                        !hub.contains(at),
                        "{hue}° reaches the ring at {}°, {across} across",
                        step * 5
                    );
                    assert_eq!(
                        settle(CENTRE, INNER, OUTER, hub, at),
                        Aim::Ring,
                        "{hue}°: a press on the ring at {}° went elsewhere",
                        step * 5
                    );
                }
            }
            // Past the ring's *outer* edge nothing is drawn either, and the
            // wheel's hit area is the square around it.
            for step in 0..72 {
                let out = on_the_ring(step as f32 * 5.0, 1.0);
                let past = CENTRE + (out - CENTRE) * 1.02;
                assert_eq!(
                    settle(CENTRE, INNER, OUTER, hub, past),
                    Aim::Nothing,
                    "{hue}°: outside the ring at {}° set a hue",
                    step * 5
                );
            }
            // The shape's own corners are its own, which is the whole reason the
            // hub is asked before the ring: they reach past `RING_GRIP`.
            for corner in hub_corners(hub) {
                let just_inside = CENTRE + (corner - CENTRE) * 0.98;
                assert!(
                    (just_inside - CENTRE).length() > INNER * RING_GRIP,
                    "{hue}°: this corner does not reach the grip, so the ordering is untested"
                );
                assert_eq!(
                    settle(CENTRE, INNER, OUTER, hub, just_inside),
                    Aim::Centre,
                    "{hue}°: the ring took a press on a corner"
                );
            }
        }
    }

    /// A drag that began inside the centre keeps it however far out it wanders,
    /// and goes on sliding along the edge. Containment is the *press*'s
    /// question; asking it every frame would freeze the marker where the
    /// pointer left the shape.
    #[test]
    fn a_drag_out_of_the_centre_still_belongs_to_the_centre() {
        let mut wheel = Wheel::new(Hsv::new(123.0, 0.5, 0.5));
        assert!(matches!(wheel.press(CENTRE), WheelAim::Centre(_)));
        let mut seen = Vec::new();
        for step in 0..36 {
            let a = (step as f32 * 10.0).to_radians();
            let at = CENTRE + vec2(a.cos(), a.sin()) * INNER * 2.0;
            let aimed = wheel.drag(CENTRE, at);
            assert!(
                matches!(aimed, WheelAim::Centre(_)),
                "lost the drag at {}° as {aimed:?}",
                step * 10
            );
            assert_eq!(wheel.hsv.h, 123.0, "the hue is not the centre's to move");
            seen.push((wheel.hsv.s, wheel.hsv.v));
        }
        // Let go out on the ring: still the centre's.
        let aimed = wheel.release(on_the_ring(210.0, 0.5));
        assert!(matches!(aimed, WheelAim::Centre(_)), "{aimed:?}");
        assert_eq!(wheel.hsv.h, 123.0, "the release moved the hue");
        // It slid rather than froze.
        seen.dedup();
        assert!(seen.len() > 4, "froze: {seen:?}");
    }

    /// A press in a gap between the shape and the ring moves nothing at all.
    /// Nothing is drawn there, and a press that quietly snapped the marker to
    /// the nearest edge is the control that lies.
    #[test]
    fn a_press_between_the_shape_and_the_ring_moves_no_colour() {
        // A hue of 270 points the apex straight up, so straight down from the
        // centre is the middle of the opposite edge: the widest of the gaps.
        let before = Hsv::new(270.0, 0.4, 0.6);
        let hub = hub_at(before.h);
        let from = CENTRE + vec2(0.0, INNER * 0.7);
        assert!(!hub.contains(from), "that is not a gap");
        assert!(
            (from - CENTRE).length() < INNER * RING_GRIP,
            "nor is it ring"
        );
        assert_eq!(settle(CENTRE, INNER, OUTER, hub, from), Aim::Nothing);

        let mut wheel = Wheel::new(before);
        assert_eq!(wheel.press(from), WheelAim::Idle);
        // And a drag begun there stays nothing, wherever it goes.
        assert_eq!(wheel.drag(from, CENTRE), WheelAim::Idle);
        assert_eq!(wheel.drag(from, on_the_ring(90.0, 0.5)), WheelAim::Idle);
        assert_eq!(wheel.release(CENTRE), WheelAim::Idle);
        assert_eq!(
            (wheel.hsv.h, wheel.hsv.s, wheel.hsv.v),
            (before.h, before.s, before.v)
        );
    }

    /// egui reports a *drag* only once the pointer has travelled far enough to
    /// be one, which a press is not yet and a release is no longer. Reading the
    /// pointer's own position on those two frames is what handed a press on the
    /// hue ring to the saturation and value field.
    #[test]
    fn a_gesture_is_settled_at_its_press_and_not_at_its_release() {
        let press = pos2(10.0, 10.0);
        let now = pos2(80.0, 90.0);
        let reported = |at, press_origin, pressed| Reported {
            at: Some(at),
            press_origin,
            pressed,
        };

        // The press: nothing recorded yet, so it is settled here.
        assert_eq!(
            frame(reported(press, Some(press), true), None),
            Frame::Press(press, press)
        );
        // Held down since, whether or not egui calls it a drag yet: the aim
        // stands and only the position moves.
        assert_eq!(
            frame(reported(now, Some(press), false), Some(Aim::Ring)),
            Frame::Held(Aim::Ring, now)
        );
        // Released: egui has cleared its own origin, and what was recorded is
        // all that is left.
        assert_eq!(
            frame(reported(now, None, false), Some(Aim::Ring)),
            Frame::Held(Aim::Ring, now)
        );
        // Pressed and released inside one frame: the pointer's own position is
        // the origin.
        assert_eq!(
            frame(reported(now, None, true), None),
            Frame::Press(now, now)
        );
        // The primary button going down is a new gesture whatever the position,
        // including the exact pixel an abandoned gesture was pressed at.
        assert_eq!(
            frame(reported(press, Some(press), true), Some(Aim::Centre)),
            Frame::Press(press, press)
        );
        // No gesture on this widget. The record goes.
        assert_eq!(
            frame(
                Reported {
                    at: None,
                    press_origin: Some(press),
                    pressed: false,
                },
                Some(Aim::Ring)
            ),
            Frame::Idle
        );
        assert_eq!(frame(Reported::default(), None), Frame::Idle);
    }

    /// A second button pressed part-way through a drag must change nothing.
    /// egui keeps this widget's interaction, so `at` goes on being reported,
    /// while `press_origin` jumps to wherever the pointer now is and is then
    /// cleared by that button's release. Reading either as "a new gesture
    /// began" threw saturation and value at the pointer, twice.
    #[test]
    fn a_second_button_pressed_mid_drag_changes_nothing() {
        let start = Hsv::new(200.0, 0.42, 0.58);
        let mut wheel = Wheel::new(start);
        let press = on_the_ring(40.0, 0.5);
        assert!(matches!(wheel.press(press), WheelAim::Ring(_)));
        assert!(matches!(
            wheel.drag(press, on_the_ring(80.0, 0.5)),
            WheelAim::Ring(_)
        ));

        let over_the_centre = CENTRE;
        for (what, aimed) in [
            (
                "the second press",
                wheel.second_button_down(over_the_centre),
            ),
            ("the frame after", wheel.drag(press, over_the_centre)),
            (
                "the second release",
                wheel.second_button_up(over_the_centre),
            ),
        ] {
            assert!(
                matches!(aimed, WheelAim::Ring(_)),
                "{what} took the drag as {aimed:?}"
            );
        }
        assert_eq!(
            (wheel.hsv.s, wheel.hsv.v),
            (start.s, start.v),
            "the marker moved"
        );
        // The hue is still the ring's to move, and moved only because the
        // pointer did: to the centre, where `ring_hue` reads zero.
        assert_eq!(wheel.hsv.h, ring_hue(CENTRE, over_the_centre));
    }

    /// A record left behind by a gesture that ended with no frame to clear it
    /// must not be inherited, not even by a press at the very pixel it was
    /// settled at.
    #[test]
    fn a_new_press_on_the_same_pixel_settles_afresh() {
        let spot = on_the_ring(90.0, 0.5);
        let mut wheel = Wheel::new(Hsv::new(10.0, 0.5, 0.5));
        assert!(matches!(wheel.press(spot), WheelAim::Ring(_)));
        wheel.held = Some(Aim::Centre);

        assert!(
            matches!(wheel.press(spot), WheelAim::Ring(_)),
            "the stale aim was inherited"
        );
        // And an idle frame is what clears a record in the ordinary case.
        assert_eq!(wheel.idle(), WheelAim::Idle);
        assert_eq!(wheel.held, None);
    }

    /// The release frame of a drag out of the centre must not be handed to the
    /// ring: without the recorded aim the release falls back to the pointer,
    /// which by then is out on the ring.
    #[test]
    fn a_drag_released_over_the_ring_does_not_move_the_hue() {
        let mut wheel = Wheel::new(Hsv::new(30.0, 0.5, 0.5));
        assert!(matches!(wheel.press(CENTRE), WheelAim::Centre(_)));
        let aimed = wheel.release(on_the_ring(210.0, 0.5));
        assert!(
            matches!(aimed, WheelAim::Centre(_)),
            "the release was read as a press on the ring: {aimed:?}"
        );
        assert_eq!(wheel.hsv.h, 30.0);
    }

    /// Two wheels in two viewports share one egui memory, so a gesture's record
    /// is keyed by the viewport as well as the widget.
    #[test]
    fn two_viewports_keep_separate_gestures() {
        let id = egui::Id::new("wheel");
        let other = egui::ViewportId::from_hash_of("other");
        assert_ne!(aim_key(id, egui::ViewportId::ROOT), aim_key(id, other));
        assert_ne!(
            aim_key(id, egui::ViewportId::ROOT),
            aim_key(egui::Id::new("second"), egui::ViewportId::ROOT)
        );
        assert_eq!(
            aim_key(id, egui::ViewportId::ROOT),
            aim_key(id, egui::ViewportId::ROOT)
        );
    }

    /// The picker driven the way a hand drives it: real pointer events through a
    /// real [`egui::Context`], calling [`wheel`] itself.
    ///
    /// Every other test here drives [`resolve`], which is the same dispatch but
    /// not the same *frame timing*. Whether `interact_pointer_pos` is `Some` on
    /// the press frame, whether egui clears `press_origin` on the release, and
    /// whether `primary_pressed` is raised on exactly one frame are exercised
    /// here, along with the memory the settled aim is kept in.
    ///
    /// The property is geometry-free: **one gesture may move the hue, or the
    /// saturation and value, or neither, never both.**
    #[test]
    fn a_real_gesture_moves_one_control_and_not_both() {
        use egui::{Event, Modifiers, PointerButton, RawInput, Rect};

        let ctx = egui::Context::default();
        let palette = Palette::dark(28.0);
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 300.0));

        let run = |hsv: &mut Hsv, events: Vec<Event>| {
            let input = RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        wheel(ui, &palette, hsv, SIZE);
                    });
            });
        };

        let button = |pos: Pos2, pressed: bool| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
        };

        // A grid over and a little beyond the wheel: some land on the ring,
        // some in the centre, some in the gaps and some outside. Which is
        // which is deliberately not worked out here.
        let mut probed = 0;
        let mut hue_gestures = 0;
        let mut sv_gestures = 0;
        for row in 0..12 {
            for column in 0..12 {
                let spot = pos2(
                    (column as f32 + 0.5) * (SIZE + 20.0) / 12.0,
                    (row as f32 + 0.5) * (SIZE + 20.0) / 12.0,
                );
                let mut hsv = Hsv::new(210.0, 0.4, 0.6);
                // A frame with the pointer merely present, so the widget exists
                // for egui's hit test before anything is pressed.
                run(&mut hsv, vec![Event::PointerMoved(spot)]);
                let before = hsv;

                let mut hue_moved = false;
                let mut sv_moved = false;
                let mut check = |hsv: Hsv, what: &str| {
                    hue_moved |= hsv.h != before.h;
                    sv_moved |= (hsv.s, hsv.v) != (before.s, before.v);
                    assert!(
                        !(hue_moved && sv_moved),
                        "one gesture from {spot:?} moved the hue *and* the marker, \
                         by {what}: {before:?} -> {hsv:?}"
                    );
                    (hue_moved, sv_moved)
                };

                run(&mut hsv, vec![button(spot, true)]);
                check(hsv, "the press");
                // Far enough to be a drag by egui's own reckoning, and across
                // the middle, which is where a re-settled aim changes hands.
                for step in 1..=6 {
                    let to = spot + vec2(step as f32 * 7.0, step as f32 * 5.0);
                    run(&mut hsv, vec![Event::PointerMoved(to)]);
                    check(hsv, "a drag frame");
                }
                let last = spot + vec2(42.0, 30.0);
                run(&mut hsv, vec![button(last, false)]);
                check(hsv, "the release");
                run(&mut hsv, Vec::new());
                let (hue, sv) = check(hsv, "the frame after");

                probed += 1;
                hue_gestures += usize::from(hue);
                sv_gestures += usize::from(sv);
            }
        }
        assert_eq!(probed, 144);
        // The sweep reaches both controls, so the property above is about a
        // picker that answers the pointer rather than one that ignores it.
        assert!(hue_gestures > 0, "no gesture reached the hue ring");
        assert!(sv_gestures > 0, "no gesture reached the centre");
    }

    /// The triangle's skirt is one feather wide along every edge. Pushing a
    /// corner out by `f` moves each edge by only `f / 2` for an equilateral
    /// triangle, which left the diagonals visibly stepped; the corner needs
    /// `f / sin(30°)`.
    #[test]
    fn the_triangles_skirt_is_one_feather_wide_along_every_edge() {
        let f = 1.0;
        for degrees in [0.0_f32, 17.0, 45.0, 120.0, 271.0, 359.0] {
            let (a, b, c) = triangle_corners(CENTRE, RADIUS, degrees.to_radians());
            let inner = [a, b, c];
            let centroid = pos2(
                inner.iter().map(|p| p.x).sum::<f32>() / 3.0,
                inner.iter().map(|p| p.y).sum::<f32>() / 3.0,
            );
            let outer = inner.map(|pt| skirt_corner(pt, centroid, f));

            for (i, j) in [(0, 1), (1, 2), (2, 0)] {
                let dir = inner[j] - inner[i];
                // Both ends, so the two are parallel as well as far enough
                // apart: a skirt that fanned would be wide at one end only.
                for end in [outer[i], outer[j]] {
                    let d = end - inner[i];
                    let across = (dir.x * d.y - dir.y * d.x).abs() / dir.length();
                    assert!(
                        (across - f).abs() < 1e-3,
                        "edge {i}-{j} at {degrees}° is feathered over {across}, not {f}"
                    );
                }
                // And outside, not folded back over the gradient.
                let out = outer[i] - centroid;
                let inn = inner[i] - centroid;
                assert!(out.length() > inn.length(), "corner {i} folded inwards");
            }
        }
    }

    /// A triangle squashed to nothing has no outward direction, and one NaN
    /// vertex is a mesh egui throws away.
    #[test]
    fn a_degenerate_triangle_gets_no_skirt_rather_than_a_nan() {
        assert_eq!(skirt_corner(CENTRE, CENTRE, 1.0), CENTRE);
    }

    /// The barycentric hit test divides by the area, and a degenerate triangle
    /// returns NaN and freezes the picker.
    #[test]
    fn the_corners_are_never_collinear() {
        for hue in [0.0, 1.0, 30.0, 90.0, 180.0, 275.0, 359.9] {
            let (h, w, b) = corners(hue);
            let (x, y, z) = barycentric(CENTRE, h, w, b);
            assert!(x.is_finite(), "degenerate at hue={hue}");
            // The centre is inside, so every weight is positive.
            assert!(x > 0.0 && y > 0.0 && z > 0.0, "{x} {y} {z}");
        }
    }
}
