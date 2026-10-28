//! Design tokens transcribed from `Design-Principles/tokens.css` and `STYLE-GUIDE.md`.

use egui::{Color32, Stroke, vec2};

/// Metric sizes verbatim from Design-Principles.
pub mod metrics {
    pub const MENU_BAR: f32 = 34.0;
    pub const STATUS_BAR: f32 = 26.0;
    pub const SIDEBAR: f32 = 240.0;
    pub const PANEL: f32 = 264.0;
    pub const NAV_ROW: f32 = 30.0;
    pub const ROW_PLAIN: f32 = 20.0;
    pub const ROW: f32 = 26.0;
    pub const BUTTON: f32 = 26.0;
    pub const FIELD: f32 = 26.0;
    pub const PAD_STRIP: f32 = 12.0;
    pub const PAD_PANEL: f32 = 12.0;

    pub const RADIUS_TIGHT: f32 = 3.0;
    pub const RADIUS: f32 = 5.0;
    pub const RADIUS_TOOL: f32 = 6.0;
    pub const RADIUS_CARD: f32 = 8.0;
    pub const RADIUS_MODAL: f32 = 10.0;

    pub const MARK: f32 = 15.0;
    pub const ICON: f32 = 16.0;
    pub const HAIRLINE: f32 = 1.0;
    pub const RAIL: f32 = 3.0;
    pub const OPTIONS_STRIP: f32 = 36.0;
    pub const NAV_MARK_W: f32 = 3.0;

    pub const DASH: f32 = 5.0;
    pub const DASH_GAP: f32 = 4.0;

    pub const S1: f32 = 4.0;
    pub const S2: f32 = 8.0;
    pub const S3: f32 = 12.0;
    pub const S4: f32 = 16.0;
    pub const S5: f32 = 24.0;
    pub const S6: f32 = 32.0;

    // Capture overlay. The ants are Umber's selection outline, same numbers.
    /// Length of one marching-ants dash and the gap after it, in points.
    pub const ANT_DASH: f32 = 4.0;
    /// How fast the dashes travel, in points per second: one period a second.
    pub const ANT_SPEED: f32 = 2.0 * ANT_DASH;
    /// How often a frame is asked for while ants are on screen.
    pub const ANT_FRAME_MS: u64 = 60;
    /// Opacity of the dim laid over everything outside the selection.
    pub const DIM_ALPHA: f32 = 0.6;
    /// Duration of the highlight rectangle's tween between windows.
    pub const TWEEN_MS: f32 = 160.0;
    /// Side of the loupe, in points.
    pub const LOUPE: f32 = 112.0;
    /// Magnification of the loupe.
    pub const LOUPE_ZOOM: f32 = 8.0;
    /// Height of the position caption under the loupe's grid.
    pub const LOUPE_CAPTION: f32 = 18.0;
    /// The gap kept between the pointer and the loupe (Umber's `CLEARANCE`).
    pub const LOUPE_CLEARANCE: f32 = 22.0;
    /// Opacity of the black hairlines between the loupe's texels (the
    /// mockup's `#0003`). Black, not a token: a mark over user pixels (§2.6).
    pub const LOUPE_GRID_ALPHA: u8 = 0x33;

    // The capture bar and the overlay's marks (owner's mockups
    // `launcher-c2.html` and `editor-placement.html`).
    /// A floating panel's fill opacity: `chrome` at 96 % (§5).
    pub const FLOAT_ALPHA: f32 = 0.96;
    /// How far a hovered primary button's accent moves towards `text_strong`.
    pub const PRIMARY_HOVER_MIX: f32 = 0.12;
    /// Opacity of a disabled control (§7.6).
    pub const DISABLED_ALPHA: f32 = 0.45;
    /// Padding and gap inside a segmented control (§7.9).
    pub const SEG_PAD: f32 = 2.0;
    /// A segment's corner radius (§7.9).
    pub const RADIUS_SEG: f32 = 4.0;
    /// Width of an icon-only segment in the capture bar (mockup `.si`).
    pub const SEG_ICON_W: f32 = 28.0;
    /// The chevron of a dropdown trigger (§7.7, mockup `.dd svg.chev`).
    pub const CHEVRON: f32 = 12.0;
    /// Gap between an icon and its label inside a control (mockup `gap: 6px`).
    pub const ICON_GAP: f32 = 6.0;
    /// The colour swatch in the tool bar (mockup `.sw`).
    pub const SWATCH_SMALL: f32 = 18.0;
    /// A menu's inner padding (§7.1).
    pub const MENU_PAD: f32 = 5.0;
    /// A menu item's padding, vertical then horizontal (§7.1).
    pub const MENU_ITEM_PAD_Y: f32 = 6.0;
    pub const MENU_ITEM_PAD_X: f32 = 10.0;
    /// How far a menu or popover opens from its trigger (§7.7, §7.17).
    pub const POPOVER_GAP: f32 = 4.0;
    /// A tooltip's padding, vertical then horizontal (§7.17).
    pub const TOOLTIP_PAD_Y: f32 = 4.0;
    pub const TOOLTIP_PAD_X: f32 = 8.0;
    /// How long the pointer rests on a control before its tooltip (§7.17).
    pub const TOOLTIP_DELAY_MS: u64 = 400;
    /// A read-only figure chip's padding (mockup `.chip`, §7.13).
    pub const CHIP_PAD_X: f32 = 7.0;
    pub const CHIP_PAD_Y: f32 = 2.0;
    /// Side of a resize handle on the selection or an annotation.
    pub const HANDLE: f32 = 7.0;
    /// Side of the colour wheel in the swatch's popover.
    pub const WHEEL_POPOVER: f32 = 168.0;
    /// Width of the notice card (a capture error with an OK button).
    pub const NOTICE_W: f32 = 320.0;

    // Colour wheel, from Umber's picker.
    /// Width of the hue ring. A wheel too small for it keeps a quarter of its
    /// radius as hub and narrows the band instead.
    pub const WHEEL_RING: f32 = 20.0;
    /// Smallest wheel that is still a wheel: below this a ring's inner radius
    /// would go negative and its mesh turn inside out.
    pub const WHEEL_MIN: f32 = 48.0;
    /// Radius of the hue marker on the ring.
    pub const WHEEL_HUE_MARKER: f32 = 6.0;
    /// Radius of the saturation and value marker in the triangle.
    pub const WHEEL_SV_MARKER: f32 = 5.0;
    /// Stroke width of both markers.
    pub const WHEEL_MARKER_STROKE: f32 = 2.0;
}

pub const STRONG: &str = "archivo-semibold";

pub fn strong(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(STRONG.into()))
}

pub mod text {
    pub const PAGE: f32 = 15.0;
    pub const EYEBROW: f32 = 10.0;
    pub const HEADING: f32 = 13.0;
    pub const BODY: f32 = 12.0;
    pub const CONTROL: f32 = 11.5;
    pub const SMALL: f32 = 11.0;
    pub const TINY: f32 = 10.5;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub backdrop: Color32,
    pub window: Color32,
    pub dock: Color32,
    pub chrome: Color32,
    pub popover: Color32,

    pub line_soft: Color32,
    pub line: Color32,
    pub line_popover: Color32,
    pub line_dashed: Color32,

    pub control: Color32,
    pub control_hover: Color32,
    pub rail: Color32,
    pub knob: Color32,
    pub control_active: Color32,
    pub field: Color32,

    pub text_strong: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub text_dim: Color32,
    pub placeholder: Color32,

    pub accent: Color32,
    pub accent_dim: Color32,
    pub accent_ring: Color32,
    pub accent_tint: Color32,
    pub accent_ink: Color32,

    pub caution: Color32,
    pub caution_bg: Color32,
    pub caution_line: Color32,
    pub good: Color32,
    pub critical: Color32,
    pub series: [Color32; 6],
}

impl Palette {
    /// The solid line under the marching ants.
    ///
    /// Derived from the accent, not stored, so it is not a theme token: the far
    /// end of the lightness axis from whatever the accent is. `backdrop` and
    /// `accent` are each dark in one theme and light in the other, so a pair
    /// built from them can land within a hair of each other and the underlay
    /// vanishes on mid-grey artwork. Black under a light accent and white under
    /// a dark one reads on any screenshot, whichever half of the pair is
    /// momentarily over a dark pixel. (Umber's `accent_underlay`, which takes
    /// the same end by bisecting a contrast ratio; at its strongest rank that
    /// lands on the extreme itself, so this goes there directly.)
    pub fn accent_underlay(&self) -> Color32 {
        // WCAG relative luminance; the end with more contrast headroom wins.
        // Compared as ratios rather than distances: the +0.05 is worth far more
        // at the dark end, so mid grey is still better served by black.
        let l = luminance(self.accent);
        let lighter = 1.05 / (l + 0.05);
        let darker = (l + 0.05) / 0.05;
        if lighter >= darker {
            Color32::WHITE
        } else {
            Color32::BLACK
        }
    }

    pub fn of(mode: Mode, accent_h: f32) -> Self {
        match mode {
            Mode::Dark => Self::dark(accent_h),
            Mode::Light => Self::light(accent_h),
        }
    }

    pub fn dark(accent_h: f32) -> Self {
        Self {
            backdrop: oklch(0.164, 0.005, 264.0),
            window: oklch(0.182, 0.004, 264.0),
            dock: oklch(0.195, 0.004, 264.0),
            chrome: oklch(0.209, 0.004, 264.0),
            popover: oklch(0.227, 0.006, 271.0),

            line_soft: oklch(0.243, 0.006, 258.0),
            line: oklch(0.276, 0.006, 258.0),
            line_popover: oklch(0.301, 0.008, 264.0),
            line_dashed: oklch(0.359, 0.010, 261.0),

            control: oklch(0.244, 0.006, 271.0),
            control_hover: oklch(0.276, 0.006, 258.0),
            rail: oklch(0.276, 0.006, 258.0),
            knob: oklch(0.928, 0.003, 265.0),
            control_active: oklch(0.29, 0.012, accent_h),
            field: oklch(0.195, 0.004, 264.0),

            text_strong: oklch(0.928, 0.003, 265.0),
            text: oklch(0.841, 0.005, 258.0),
            text_muted: oklch(0.695, 0.008, 261.0),
            text_dim: oklch(0.622, 0.008, 261.0),
            placeholder: oklch(0.481, 0.009, 261.0),

            accent: oklch(0.674, 0.101, accent_h),
            accent_dim: oklch(0.447, 0.061, accent_h),
            accent_ring: oklch(0.674, 0.101, accent_h).gamma_multiply(0.35),
            accent_tint: oklch(0.674, 0.101, accent_h).gamma_multiply(0.07),
            accent_ink: oklch(0.182, 0.004, 264.0),

            caution: oklch(0.693, 0.096, 38.0),
            caution_bg: oklch(0.245, 0.023, 42.0),
            caution_line: oklch(0.424, 0.068, 35.0),
            good: oklch(0.70, 0.10, 145.0),
            critical: oklch(0.66, 0.13, 22.0),
            series: [
                oklch(0.674, 0.101, accent_h),
                oklch(0.674, 0.101, accent_h + 30.0),
                oklch(0.674, 0.101, accent_h + 60.0),
                oklch(0.674, 0.101, accent_h + 90.0),
                oklch(0.674, 0.101, accent_h + 120.0),
                oklch(0.674, 0.101, accent_h + 150.0),
            ],
        }
    }

    pub fn light(accent_h: f32) -> Self {
        Self {
            backdrop: oklch(0.908, 0.010, 82.0),
            window: oklch(0.944, 0.007, 81.0),
            dock: oklch(0.953, 0.007, 81.0),
            chrome: oklch(0.971, 0.006, 85.0),
            popover: Color32::WHITE,

            line_soft: oklch(0.915, 0.008, 82.0),
            line: oklch(0.890, 0.010, 82.0),
            line_popover: oklch(0.890, 0.010, 82.0),
            line_dashed: oklch(0.80, 0.012, 82.0),

            control: oklch(0.929, 0.009, 85.0),
            control_hover: oklch(0.896, 0.010, 82.0),
            rail: oklch(0.890, 0.010, 82.0),
            knob: Color32::WHITE,
            control_active: oklch(0.909, 0.020, accent_h),
            field: Color32::WHITE,

            text_strong: oklch(0.342, 0.004, 68.0),
            text: oklch(0.342, 0.004, 68.0),
            text_muted: oklch(0.526, 0.007, 75.0),
            text_dim: oklch(0.634, 0.008, 81.0),
            placeholder: oklch(0.72, 0.008, 81.0),

            accent: oklch(0.55, 0.10, accent_h),
            accent_dim: oklch(0.79, 0.036, accent_h),
            accent_ring: oklch(0.55, 0.10, accent_h).gamma_multiply(0.35),
            accent_tint: oklch(0.55, 0.10, accent_h).gamma_multiply(0.07),
            accent_ink: Color32::WHITE,

            caution: oklch(0.518, 0.114, 39.0),
            caution_bg: oklch(0.943, 0.018, 49.0),
            caution_line: oklch(0.832, 0.042, 51.0),
            good: oklch(0.50, 0.11, 145.0),
            critical: oklch(0.50, 0.14, 22.0),
            series: [
                oklch(0.55, 0.10, accent_h),
                oklch(0.55, 0.10, accent_h + 30.0),
                oklch(0.55, 0.10, accent_h + 60.0),
                oklch(0.55, 0.10, accent_h + 90.0),
                oklch(0.55, 0.10, accent_h + 120.0),
                oklch(0.55, 0.10, accent_h + 150.0),
            ],
        }
    }
}

pub mod shadow {
    use super::Mode;
    use egui::Color32;
    use egui::epaint::Shadow;

    pub fn menu(mode: Mode) -> Shadow {
        build(mode, 8, 32, 153)
    }

    pub fn floating(mode: Mode) -> Shadow {
        build(mode, 16, 48, 178)
    }

    pub fn modal(mode: Mode) -> Shadow {
        build(mode, 24, 64, 178)
    }

    fn build(mode: Mode, offset: i8, blur: u8, alpha: u8) -> Shadow {
        let color = match mode {
            Mode::Dark => Color32::from_black_alpha(alpha),
            Mode::Light => Color32::from_rgba_unmultiplied(60, 50, 40, alpha / 4),
        };
        Shadow {
            offset: [0, offset],
            blur,
            spread: 0,
            color,
        }
    }
}

/// WCAG relative luminance of an sRGB colour.
fn luminance(c: Color32) -> f64 {
    let channel = |b: u8| {
        let c = f64::from(b) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
}

pub fn oklch(l: f32, c: f32, h_deg: f32) -> Color32 {
    let h = h_deg.to_radians();
    let (a, b) = (c * h.cos(), c * h.sin());

    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;

    let (lc, mc, sc) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);

    let r = 4.076_741_7 * lc - 3.307_711_6 * mc + 0.230_969_94 * sc;
    let g = -1.268_438 * lc + 2.609_757_4 * mc - 0.341_319_38 * sc;
    let bl = -0.004_196_086_3 * lc - 0.703_418_6 * mc + 1.707_614_7 * sc;

    Color32::from_rgb(to_srgb(r), to_srgb(g), to_srgb(bl))
}

fn to_srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let encoded = if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}

#[allow(clippy::excessive_precision)]
pub fn rgb_to_oklch_hue(r: u8, g: u8, b: u8) -> f32 {
    let to_linear = |v: u8| -> f32 {
        let f = v as f32 / 255.0;
        if f <= 0.04045 {
            f / 12.92
        } else {
            ((f + 0.055) / 1.055).powf(2.4)
        }
    };
    let lr = to_linear(r);
    let lg = to_linear(g);
    let lb = to_linear(b);

    let l = 0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb;
    let m = 0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb;
    let s = 0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb;

    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();

    let a = 1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_;
    let b_val = 0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_;

    let mut h = b_val.atan2(a).to_degrees();
    if h < 0.0 {
        h += 360.0;
    }
    h
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    for (name, bytes) in [
        (
            "archivo",
            include_bytes!("../../../assets/Archivo-Regular.ttf").as_slice(),
        ),
        (
            STRONG,
            include_bytes!("../../../assets/Archivo-SemiBold.ttf").as_slice(),
        ),
    ] {
        fonts.font_data.insert(
            name.into(),
            std::sync::Arc::new(egui::FontData::from_static(bytes)),
        );
    }

    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "archivo".into());

    fonts.families.insert(
        egui::FontFamily::Name(STRONG.into()),
        vec![STRONG.to_string()],
    );
    ctx.set_fonts(fonts);
}

pub fn apply_theme(ctx: &egui::Context, p: Palette, mode: Mode) {
    let mut visuals = match mode {
        Mode::Dark => egui::Visuals::dark(),
        Mode::Light => egui::Visuals::light(),
    };

    visuals.panel_fill = p.window;
    visuals.window_fill = p.popover;
    visuals.extreme_bg_color = p.field;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(metrics::HAIRLINE, p.line);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(metrics::HAIRLINE, p.text);
    visuals.window_stroke = Stroke::new(metrics::HAIRLINE, p.line_popover);
    visuals.window_shadow = shadow::floating(mode);
    visuals.popup_shadow = shadow::menu(mode);
    visuals.selection.bg_fill = p.accent_ring;
    visuals.selection.stroke = Stroke::new(metrics::HAIRLINE, p.text_strong);

    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = vec2(metrics::S2, metrics::S1);
    style.spacing.scroll.bar_width = 6.0;
    ctx.set_style(style);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(c: Color32, expected: (u8, u8, u8), tolerance: u8, name: &str) {
        let (r, g, b) = (c.r(), c.g(), c.b());
        let dr = r.abs_diff(expected.0);
        let dg = g.abs_diff(expected.1);
        let db = b.abs_diff(expected.2);
        assert!(
            dr <= tolerance && dg <= tolerance && db <= tolerance,
            "{name}: got ({r}, {g}, {b}), expected {expected:?} (+/- {tolerance})"
        );
    }

    #[test]
    fn the_dark_ladder_matches_design_principles() {
        let p = Palette::dark(28.0);
        near(p.backdrop, (0x0D, 0x0E, 0x10), 2, "--backdrop");
        near(p.window, (0x11, 0x12, 0x14), 2, "--window");
        near(p.dock, (0x14, 0x15, 0x17), 2, "--dock");
        near(p.chrome, (0x17, 0x18, 0x1A), 2, "--chrome");
        near(p.popover, (0x1B, 0x1C, 0x1F), 2, "--popover");

        near(p.line_soft, (0x1E, 0x20, 0x23), 2, "--line-soft");
        near(p.line, (0x26, 0x28, 0x2B), 2, "--line");
        near(p.line_popover, (0x2C, 0x2E, 0x32), 2, "--line-popover");

        near(p.control, (0x1F, 0x20, 0x23), 2, "--control");
        near(p.text_strong, (0xE6, 0xE7, 0xE9), 2, "--text-strong");
        near(p.text, (0xC9, 0xCB, 0xCE), 2, "--text");
        near(p.text_muted, (0x9A, 0x9D, 0xA2), 2, "--text-muted");
        near(p.text_dim, (0x84, 0x87, 0x8C), 2, "--text-dim");
    }

    #[test]
    fn the_light_ladder_matches_design_principles() {
        let p = Palette::light(28.0);
        near(p.backdrop, (0xE4, 0xE0, 0xD9), 2, "--backdrop");
        near(p.window, (0xEF, 0xEB, 0xE6), 2, "--window");
        near(p.dock, (0xF2, 0xEE, 0xE9), 2, "--dock");
        near(p.chrome, (0xF7, 0xF3, 0xEF), 2, "--chrome");
        near(p.popover, (0xFF, 0xFF, 0xFF), 0, "--popover");

        near(p.text_strong, (0x3A, 0x38, 0x36), 2, "--text-strong");
        near(p.text, (0x3A, 0x38, 0x36), 2, "--text");
        near(p.text_muted, (0x6D, 0x6A, 0x66), 2, "--text-muted");
    }

    #[test]
    fn metric_tokens_match_the_spec() {
        assert_eq!(metrics::MENU_BAR, 34.0);
        assert_eq!(metrics::STATUS_BAR, 26.0);
        assert_eq!(metrics::SIDEBAR, 240.0);
        assert_eq!(metrics::NAV_ROW, 30.0);
        assert_eq!(metrics::ROW, 26.0);
        assert_eq!(metrics::BUTTON, 26.0);
        assert_eq!(metrics::RADIUS, 5.0);
        assert_eq!(metrics::RADIUS_CARD, 8.0);
        assert_eq!(metrics::RADIUS_MODAL, 10.0);
        assert_eq!(metrics::MARK, 15.0);
        assert_eq!(metrics::HAIRLINE, 1.0);
        assert_eq!(metrics::NAV_MARK_W, 3.0);
    }
}
