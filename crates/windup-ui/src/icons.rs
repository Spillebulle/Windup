//! Lucide monochrome vector icons renderer. §11 of STYLE-GUIDE.md.
//! Authored against Lucide's 24x24 box and flattened via crate::lucide.

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
}

impl Icon {
    pub const ALL: [Self; 19] = [
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
