//! Painting the overlay's content: the annotations (with cached previews for
//! blur and mosaic) and the marks over the user's pixels (handles, the size
//! chip). Split from `mod.rs`, which keeps the view and its input.

use super::Mapping;
use crate::theme::{Mode, Palette, metrics, shadow, text};
use egui::{
    Color32, ColorImage, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Vec2, pos2, vec2,
};
use std::collections::HashMap;
use std::time::Duration;
use windup_core::capture::DRect;
use windup_core::capture::annotate::{Annotation, Shape};
use windup_core::capture::raster::region_preview;
use windup_core::capture::session::Session;

/// How often the preview of a blur or mosaic being drawn is recomputed while
/// its rectangle changes, in seconds. Between recomputations the last one is
/// stretched to fit; a large region blurred on every drag frame would stall.
const DRAFT_PREVIEW_SECS: f64 = 0.1;

/// The id the annotation being drawn has in the preview cache. Real ids
/// count up from one.
pub const DRAFT_ID: u64 = u64::MAX;

/// The blurred or mosaicked pixels of one region, as a texture.
struct Preview {
    /// The shape's bounds and size it was made for, as bits so the
    /// comparison is exact and cheap.
    key: [u32; 5],
    tex: TextureHandle,
    made: f64,
}

/// The region previews, by annotation id.
#[derive(Default)]
pub struct Previews {
    held: HashMap<u64, Preview>,
}

impl Previews {
    pub fn prune(&mut self, session: &Session) {
        let draft = session.draft.is_some();
        self.held
            .retain(|id, _| (*id == DRAFT_ID && draft) || session.doc.get(*id).is_some());
    }

    #[allow(clippy::too_many_arguments)]
    pub fn paint_annotation(
        &mut self,
        ctx: &egui::Context,
        painter: &Painter,
        session: &Session,
        map: &Mapping,
        a: &Annotation,
        id: u64,
        now: f64,
    ) {
        let [r, g, b, al] = a.colour;
        let colour = Color32::from_rgba_unmultiplied(r, g, b, al);
        let width = a.size * map.points_per_unit().x;
        let stroke = Stroke::new(width, colour);
        match &a.shape {
            Shape::Pen(points) => {
                let pts: Vec<Pos2> = points.iter().map(|&q| map.to_surface(q)).collect();
                // Round caps, as the render has them.
                if let (Some(first), Some(last)) = (pts.first(), pts.last()) {
                    painter.circle_filled(*first, width / 2.0, colour);
                    painter.circle_filled(*last, width / 2.0, colour);
                }
                if pts.len() >= 2 {
                    painter.add(egui::Shape::line(pts, stroke));
                }
            }
            Shape::Rect(rect) => {
                painter.rect_stroke(map.rect_to_surface(*rect), 0.0, stroke, StrokeKind::Middle);
            }
            Shape::Ellipse(rect) => {
                let s = map.rect_to_surface(*rect);
                painter.add(egui::Shape::ellipse_stroke(
                    s.center(),
                    s.size() / 2.0,
                    stroke,
                ));
            }
            Shape::Blur(rect) | Shape::Mosaic(rect) => {
                self.paint_region(ctx, painter, session, map, a, *rect, id, now);
            }
        }
    }

    /// A blur or mosaic: `region_preview`'s pixels as a texture, cached per
    /// annotation and remade only when its rectangle or size changes.
    #[allow(clippy::too_many_arguments)]
    fn paint_region(
        &mut self,
        ctx: &egui::Context,
        painter: &Painter,
        session: &Session,
        map: &Mapping,
        a: &Annotation,
        rect: DRect,
        id: u64,
        now: f64,
    ) {
        let Some(shown) = rect.normalised().intersect(&session.shot.bounds()) else {
            return;
        };
        let key = [
            rect.x.to_bits(),
            rect.y.to_bits(),
            rect.w.to_bits(),
            rect.h.to_bits(),
            a.size.to_bits(),
        ];
        let held = self.held.get(&id);
        let stale = held.is_none_or(|pv| pv.key != key);
        let throttled = id == DRAFT_ID && held.is_some_and(|pv| now - pv.made < DRAFT_PREVIEW_SECS);
        if stale && throttled {
            ctx.request_repaint_after(Duration::from_secs_f64(DRAFT_PREVIEW_SECS));
        } else if stale {
            match region_preview(&session.shot, a) {
                Some(img) => {
                    let size = [img.width() as usize, img.height() as usize];
                    let image = ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                    match self.held.get_mut(&id) {
                        Some(pv) => {
                            pv.tex.set(image, TextureOptions::LINEAR);
                            pv.key = key;
                            pv.made = now;
                        }
                        None => {
                            let tex = ctx.load_texture(
                                format!("windup-capture-region-{id}"),
                                image,
                                TextureOptions::LINEAR,
                            );
                            self.held.insert(
                                id,
                                Preview {
                                    key,
                                    tex,
                                    made: now,
                                },
                            );
                        }
                    }
                }
                None => {
                    self.held.remove(&id);
                }
            }
        }
        if let Some(pv) = self.held.get(&id) {
            painter.image(
                pv.tex.id(),
                map.rect_to_surface(shown),
                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }
}

/// The parts of `outer` outside `hole`: up to four rects.
pub fn around(outer: Rect, hole: Rect) -> Vec<Rect> {
    let h = hole.intersect(outer);
    if !h.is_positive() {
        return vec![outer];
    }
    [
        Rect::from_min_max(outer.min, pos2(outer.max.x, h.min.y)),
        Rect::from_min_max(pos2(outer.min.x, h.max.y), outer.max),
        Rect::from_min_max(pos2(outer.min.x, h.min.y), pos2(h.min.x, h.max.y)),
        Rect::from_min_max(pos2(h.max.x, h.min.y), pos2(outer.max.x, h.max.y)),
    ]
    .into_iter()
    .filter(|r| r.is_positive())
    .collect()
}

/// The eight resize handles of `r`: accent squares edged with the underlay,
/// edit-mode marks (§2.4) over the user's pixels that read on light and dark
/// alike (§2.6).
pub fn handles(painter: &Painter, p: &Palette, r: Rect) {
    let (l, c, rt) = (r.left(), r.center().x, r.right());
    let (t, m, b) = (r.top(), r.center().y, r.bottom());
    for at in [
        pos2(l, t),
        pos2(c, t),
        pos2(rt, t),
        pos2(l, m),
        pos2(rt, m),
        pos2(l, b),
        pos2(c, b),
        pos2(rt, b),
    ] {
        let h = Rect::from_center_size(at, Vec2::splat(metrics::HANDLE));
        painter.rect_filled(h, 0.0, p.accent);
        painter.rect_stroke(
            h,
            0.0,
            Stroke::new(metrics::HAIRLINE, p.accent_underlay()),
            StrokeKind::Outside,
        );
    }
}

/// The monitor that draws the size chip: the one holding the selection's
/// top-left corner, else the one holding its middle.
pub fn chip_monitor(session: &Session, r: DRect) -> Option<usize> {
    let shot = &session.shot;
    shot.monitor_at(r.x, r.y)
        .or_else(|| shot.monitor_at(r.x + r.w / 2.0, r.y + r.h / 2.0))
}

/// The result's size in pixels, as the chip shows it: at the highest scale
/// among the monitors the rect touches (spec "Mixed scale").
pub fn result_size(session: &Session, r: DRect) -> String {
    let scale = session
        .shot
        .monitors
        .iter()
        .filter(|m| m.layout.intersect(&r).is_some())
        .map(|m| m.px_per_unit())
        .fold(0.0_f32, f32::max);
    let clipped = r.intersect(&session.shot.bounds()).unwrap_or(r);
    format!(
        "{} \u{d7} {}",
        (clipped.w * scale).round() as i64,
        (clipped.h * scale).round() as i64
    )
}

/// §7.13 chip: a read-only figure in mono, `popover` fill because it floats
/// over content, `line-popover` edge, `shadow-menu`. Top-left at `at`.
pub fn chip(painter: &Painter, p: &Palette, theme: Mode, at: Pos2, figure: &str) -> Rect {
    let galley = painter.layout_no_wrap(
        figure.to_string(),
        FontId::monospace(text::TINY),
        p.text_strong,
    );
    let rect = Rect::from_min_size(
        at,
        galley.size() + 2.0 * vec2(metrics::CHIP_PAD_X, metrics::CHIP_PAD_Y),
    );
    painter.add(shadow::menu(theme).as_shape(rect, metrics::RADIUS));
    painter.rect_filled(rect, metrics::RADIUS, p.popover);
    painter.rect_stroke(
        rect,
        metrics::RADIUS,
        Stroke::new(metrics::HAIRLINE, p.line_popover),
        StrokeKind::Inside,
    );
    painter.galley(
        rect.min + vec2(metrics::CHIP_PAD_X, metrics::CHIP_PAD_Y),
        galley,
        p.text_strong,
    );
    rect
}

/// The selection's size above its top-left corner, or inside it where the
/// surface has no room above.
pub fn size_chip(
    painter: &Painter,
    p: &Palette,
    theme: Mode,
    hole: Rect,
    surface: Rect,
    figure: &str,
) {
    let h = painter
        .layout_no_wrap(figure.to_string(), FontId::monospace(text::TINY), p.text)
        .size()
        .y
        + 2.0 * metrics::CHIP_PAD_Y;
    let above = hole.top() - metrics::S2 - h;
    let y = if above >= surface.top() {
        above
    } else {
        hole.top() + metrics::S2
    };
    let x = hole.left().max(surface.left() + metrics::S1);
    chip(painter, p, theme, pos2(x, y), figure);
}
