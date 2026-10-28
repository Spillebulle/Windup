//! Lucide monochrome vector icons renderer. §11 of STYLE-GUIDE.md.
//! Authored against Lucide's 24x24 box and flattened via crate::lucide.
//! The capture icons (Camera onward) are copied from lucide-static 1.34.0, the
//! release Umber's set is at.

use crate::lucide::{Node, Outline};
use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2};
use std::sync::LazyLock;

const BOX: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Sliders,
    Clipboard,
    FileText,
    Settings,
    Sparkles,
    Check,
    Copy,
    X,
    Pen,
    ChevronRight,
    Image,
    RotateCcw,
    Layers,
    Monitor,
    Keyboard,
    Search,
    Trash,
    Contrast,
    Wrench,
    Camera,
    Video,
    Footprints,
    SquareDashed,
    AppWindow,
    MousePointer2,
    Brush,
    Square,
    Circle,
    Grid3x3,
    Pipette,
    Undo2,
    Redo2,
    Download,
    ChevronDown,
    Blur,
    ChevronUp,
}

impl Icon {
    pub const ALL: [Self; 36] = [
        Self::Sliders,
        Self::Clipboard,
        Self::FileText,
        Self::Settings,
        Self::Sparkles,
        Self::Check,
        Self::Copy,
        Self::X,
        Self::Pen,
        Self::ChevronRight,
        Self::Image,
        Self::RotateCcw,
        Self::Layers,
        Self::Monitor,
        Self::Keyboard,
        Self::Search,
        Self::Trash,
        Self::Contrast,
        Self::Wrench,
        Self::Camera,
        Self::Video,
        Self::Footprints,
        Self::SquareDashed,
        Self::AppWindow,
        Self::MousePointer2,
        Self::Brush,
        Self::Square,
        Self::Circle,
        Self::Grid3x3,
        Self::Pipette,
        Self::Undo2,
        Self::Redo2,
        Self::Download,
        Self::ChevronDown,
        Self::Blur,
        Self::ChevronUp,
    ];

    pub fn nodes(self) -> &'static [Node] {
        match self {
            // Lucide sliders-horizontal
            Self::Sliders => &[
                Node::Path("M10 5H3"),
                Node::Path("M12 19H3"),
                Node::Path("M14 3v4"),
                Node::Path("M16 17v4"),
                Node::Path("M21 12h-9"),
                Node::Path("M21 19h-5"),
                Node::Path("M21 5h-7"),
                Node::Path("M8 10v4"),
                Node::Path("M8 12H3"),
            ],
            // Lucide clipboard
            Self::Clipboard => &[
                Node::Rect {
                    x: 8.0,
                    y: 2.0,
                    w: 8.0,
                    h: 4.0,
                    r: 1.0,
                },
                Node::Path(
                    "M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2",
                ),
            ],
            // Lucide file-text
            Self::FileText => &[
                Node::Path("M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"),
                Node::Path("M14 2v4a2 2 0 0 0 2 2h4"),
                Node::Path("M10 9H8"),
                Node::Path("M16 13H8"),
                Node::Path("M16 17H8"),
            ],
            // Lucide settings
            Self::Settings => &[
                Node::Path(
                    "M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915",
                ),
                Node::Circle {
                    cx: 12.0,
                    cy: 12.0,
                    r: 3.0,
                },
            ],
            // Lucide sparkles
            Self::Sparkles => &[
                Node::Path(
                    "M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .963 0L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.581a.5.5 0 0 1 0 .964L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.963 0z",
                ),
                Node::Path("M20 3v4"),
                Node::Path("M22 5h-4"),
                Node::Path("M4 17v2"),
                Node::Path("M5 18H3"),
            ],
            // Lucide check
            Self::Check => &[Node::Path("M20 6 9 17l-5-5")],
            // Lucide copy
            Self::Copy => &[
                Node::Rect {
                    x: 8.0,
                    y: 8.0,
                    w: 14.0,
                    h: 14.0,
                    r: 2.0,
                },
                Node::Path("M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"),
            ],
            // Lucide x
            Self::X => &[Node::Path("M18 6 6 18"), Node::Path("m6 6 12 12")],
            // Lucide pencil
            Self::Pen => &[
                Node::Path(
                    "M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z",
                ),
                Node::Path("m15 5 4 4"),
            ],
            // Lucide chevron-right
            Self::ChevronRight => &[Node::Path("m9 18 6-6-6-6")],
            // Lucide image
            Self::Image => &[
                Node::Rect {
                    x: 3.0,
                    y: 3.0,
                    w: 18.0,
                    h: 18.0,
                    r: 2.0,
                },
                Node::Circle {
                    cx: 9.0,
                    cy: 9.0,
                    r: 2.0,
                },
                Node::Path("m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"),
            ],
            // Lucide rotate-ccw
            Self::RotateCcw => &[
                Node::Path("M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"),
                Node::Path("M3 3v5h5"),
            ],
            // Lucide layout-grid
            Self::Layers => &[
                Node::Rect {
                    x: 3.0,
                    y: 3.0,
                    w: 7.0,
                    h: 7.0,
                    r: 1.0,
                },
                Node::Rect {
                    x: 14.0,
                    y: 3.0,
                    w: 7.0,
                    h: 7.0,
                    r: 1.0,
                },
                Node::Rect {
                    x: 14.0,
                    y: 14.0,
                    w: 7.0,
                    h: 7.0,
                    r: 1.0,
                },
                Node::Rect {
                    x: 3.0,
                    y: 14.0,
                    w: 7.0,
                    h: 7.0,
                    r: 1.0,
                },
            ],
            // Lucide monitor
            Self::Monitor => &[
                Node::Rect {
                    x: 2.0,
                    y: 3.0,
                    w: 20.0,
                    h: 14.0,
                    r: 2.0,
                },
                Node::Path("M8 21h8"),
                Node::Path("M12 17v4"),
            ],
            // Lucide keyboard
            Self::Keyboard => &[
                Node::Path("M10 8h.01"),
                Node::Path("M12 12h.01"),
                Node::Path("M14 8h.01"),
                Node::Path("M16 12h.01"),
                Node::Path("M18 8h.01"),
                Node::Path("M6 8h.01"),
                Node::Path("M7 16h10"),
                Node::Path("M8 12h.01"),
                Node::Rect {
                    x: 2.0,
                    y: 4.0,
                    w: 20.0,
                    h: 16.0,
                    r: 2.0,
                },
            ],
            // Lucide search
            Self::Search => &[
                Node::Path("m21 21-4.34-4.34"),
                Node::Circle {
                    cx: 11.0,
                    cy: 11.0,
                    r: 8.0,
                },
            ],
            // Lucide trash-2
            Self::Trash => &[
                Node::Path("M10 11v6"),
                Node::Path("M14 11v6"),
                Node::Path("M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"),
                Node::Path("M3 6h18"),
                Node::Path("M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"),
            ],
            // Lucide contrast
            Self::Contrast => &[
                Node::Circle {
                    cx: 12.0,
                    cy: 12.0,
                    r: 10.0,
                },
                Node::Path("M12 18a6 6 0 0 0 0-12v12z"),
            ],
            // Lucide wrench
            Self::Wrench => &[Node::Path(
                "M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z",
            )],
            // Lucide camera
            Self::Camera => &[
                Node::Path(
                    "M13.997 4a2 2 0 0 1 1.76 1.05l.486.9A2 2 0 0 0 18.003 7H20a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V9a2 2 0 0 1 2-2h1.997a2 2 0 0 0 1.759-1.048l.489-.904A2 2 0 0 1 10.004 4z",
                ),
                Node::Circle {
                    cx: 12.0,
                    cy: 13.0,
                    r: 3.0,
                },
            ],
            // Lucide video
            Self::Video => &[
                Node::Path(
                    "m16 13 5.223 3.482a.5.5 0 0 0 .777-.416V7.87a.5.5 0 0 0-.752-.432L16 10.5",
                ),
                Node::Rect {
                    x: 2.0,
                    y: 6.0,
                    w: 14.0,
                    h: 12.0,
                    r: 2.0,
                },
            ],
            // Lucide footprints
            Self::Footprints => &[
                Node::Path(
                    "M4 16v-2.38C4 11.5 2.97 10.5 3 8c.03-2.72 1.49-6 4.5-6C9.37 2 10 3.8 10 5.5c0 3.11-2 5.66-2 8.68V16a2 2 0 1 1-4 0Z",
                ),
                Node::Path(
                    "M20 20v-2.38c0-2.12 1.03-3.12 1-5.62-.03-2.72-1.49-6-4.5-6C14.63 6 14 7.8 14 9.5c0 3.11 2 5.66 2 8.68V20a2 2 0 1 0 4 0Z",
                ),
                Node::Path("M16 17h4"),
                Node::Path("M4 13h4"),
            ],
            // Lucide square-dashed
            Self::SquareDashed => &[
                Node::Path("M5 3a2 2 0 0 0-2 2"),
                Node::Path("M19 3a2 2 0 0 1 2 2"),
                Node::Path("M21 19a2 2 0 0 1-2 2"),
                Node::Path("M5 21a2 2 0 0 1-2-2"),
                Node::Path("M9 3h1"),
                Node::Path("M9 21h1"),
                Node::Path("M14 3h1"),
                Node::Path("M14 21h1"),
                Node::Path("M3 9v1"),
                Node::Path("M21 9v1"),
                Node::Path("M3 14v1"),
                Node::Path("M21 14v1"),
            ],
            // Lucide app-window
            Self::AppWindow => &[
                Node::Rect {
                    x: 2.0,
                    y: 4.0,
                    w: 20.0,
                    h: 16.0,
                    r: 2.0,
                },
                Node::Path("M10 4v4"),
                Node::Path("M2 8h20"),
                Node::Path("M6 4v4"),
            ],
            // Lucide mouse-pointer-2
            Self::MousePointer2 => &[Node::Path(
                "M4.037 4.688a.495.495 0 0 1 .651-.651l16 6.5a.5.5 0 0 1-.063.947l-6.124 1.58a2 2 0 0 0-1.438 1.435l-1.579 6.126a.5.5 0 0 1-.947.063z",
            )],
            // Lucide brush
            Self::Brush => &[
                Node::Path("m11 10 3 3"),
                Node::Path(
                    "M6.5 21A3.5 3.5 0 1 0 3 17.5a2.62 2.62 0 0 1-.708 1.792A1 1 0 0 0 3 21z",
                ),
                Node::Path("M9.969 17.031 21.378 5.624a1 1 0 0 0-3.002-3.002L6.967 14.031"),
            ],
            // Lucide square
            Self::Square => &[Node::Rect {
                x: 3.0,
                y: 3.0,
                w: 18.0,
                h: 18.0,
                r: 2.0,
            }],
            // Lucide circle
            Self::Circle => &[Node::Circle {
                cx: 12.0,
                cy: 12.0,
                r: 10.0,
            }],
            // Lucide grid-3x3
            Self::Grid3x3 => &[
                Node::Rect {
                    x: 3.0,
                    y: 3.0,
                    w: 18.0,
                    h: 18.0,
                    r: 2.0,
                },
                Node::Path("M3 9h18"),
                Node::Path("M3 15h18"),
                Node::Path("M9 3v18"),
                Node::Path("M15 3v18"),
            ],
            // Lucide pipette
            Self::Pipette => &[
                Node::Path(
                    "m12 9-8.414 8.414A2 2 0 0 0 3 18.828v1.344a2 2 0 0 1-.586 1.414A2 2 0 0 1 3.828 21h1.344a2 2 0 0 0 1.414-.586L15 12",
                ),
                Node::Path(
                    "m18 9 .4.4a1 1 0 1 1-3 3l-3.8-3.8a1 1 0 1 1 3-3l.4.4 3.4-3.4a1 1 0 1 1 3 3z",
                ),
                Node::Path("m2 22 .414-.414"),
            ],
            // Lucide undo-2
            Self::Undo2 => &[
                Node::Path("M9 14 4 9l5-5"),
                Node::Path("M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5a5.5 5.5 0 0 1-5.5 5.5H11"),
            ],
            // Lucide redo-2
            Self::Redo2 => &[
                Node::Path("m15 14 5-5-5-5"),
                Node::Path("M20 9H9.5A5.5 5.5 0 0 0 4 14.5A5.5 5.5 0 0 0 9.5 20H13"),
            ],
            // Lucide download
            Self::Download => &[
                Node::Path("M12 15V3"),
                Node::Path("M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"),
                Node::Path("m7 10 5 5 5-5"),
            ],
            // Lucide chevron-down
            Self::ChevronDown => &[Node::Path("m6 9 6 6 6-6")],
            // Lucide chevron-up
            Self::ChevronUp => &[Node::Path("m18 15-6-6-6 6")],
            // Lucide droplet (blur: a drop reads as soft focus, and no Lucide mark is named for it)
            Self::Blur => &[Node::Path(
                "M12 22a7 7 0 0 0 7-7c0-2-1-3.9-3-5.5s-3.5-4-4-6.5c-.5 2.5-2 4.9-4 6.5C6 11.1 5 13 5 15a7 7 0 0 0 7 7z",
            )],
        }
    }

    pub fn paint(&self, painter: &Painter, rect: Rect, color: Color32) {
        let size = rect.width().min(rect.height());
        if size <= 1.0 {
            return;
        }
        let scale = size / BOX;
        let origin = rect.center() - Vec2::splat(size * 0.5);
        let width = (1.5 * scale).max(1.0);
        let stroke = Stroke::new(width, color);
        let drawn = geometry(*self);
        let place = |p: &Pos2| pos2(origin.x + p.x * scale, origin.y + p.y * scale);

        for outline in &drawn.outlines {
            let points: Vec<_> = outline.points.iter().map(place).collect();
            if outline.is_dot() {
                painter.circle_filled(points[0], width, color);
            } else if outline.closed {
                painter.add(Shape::closed_line(points, stroke));
            } else {
                painter.add(Shape::line(points, stroke));
            }
        }
    }
}

struct Drawn {
    outlines: Vec<Outline>,
}

static GEOMETRY: LazyLock<Vec<Drawn>> = LazyLock::new(|| {
    Icon::ALL
        .iter()
        .map(|i| {
            let outlines = crate::lucide::flatten(i.nodes());
            Drawn { outlines }
        })
        .collect()
});

fn geometry(icon: Icon) -> &'static Drawn {
    let at = Icon::ALL
        .iter()
        .position(|i| *i == icon)
        .expect("every icon is in ALL");
    &GEOMETRY[at]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_flattens_to_something_inside_the_box() {
        for icon in Icon::ALL {
            let drawn = geometry(icon);
            assert!(!drawn.outlines.is_empty(), "{icon:?}");
            for p in drawn.outlines.iter().flat_map(|o| &o.points) {
                assert!(
                    (0.0..=BOX).contains(&p.x) && (0.0..=BOX).contains(&p.y),
                    "{icon:?} {p:?}"
                );
            }
        }
    }
}
