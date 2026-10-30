//! Steps tool settings: the keys, what takes a step, clicks on this desktop,
//! the marker and the export. What the desktop allows comes as plain data in
//! [`StepsFacts`]; the one thing the pane asks the desktop to do, the
//! one-time click setup, goes back as [`SettingsAction::JoinInputGroup`].

use super::capture_cfg::shortcut_row;
use super::{EscCaution, HotkeyHint, SettingsAction};
use crate::colour::{self, Hsv};
use crate::steps::paint::paint_prims;
use crate::theme::{Mode, Palette, metrics, shadow, text};
use crate::widgets::{
    button, dropdown, dropdown_gated, eyebrow, row, segmented, shortcut_badge, shortcut_recorder,
    toggle,
};
use egui::{Key, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use windup_core::Config;
use windup_core::capture::output;
use windup_core::shortcut::Shortcut;
use windup_core::steps::clicks::{CLICKS_NO_POINTER, ClickRoute, clicks_reason};
use windup_core::steps::marker::marker;
use windup_core::steps::options::{ExportFormat, MarkerKind, MarkerSize, MarkerStyle, StepTrigger};
use windup_core::tool::ToolId;

/// What the desktop found, for the rows that depend on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StepsFacts {
    /// How clicks reach Windup here.
    pub route: ClickRoute,
    /// Running from the AppImage, which cannot ship a device rule.
    pub appimage: bool,
    /// Whether the pictures can carry a marker, and if not, why.
    pub markers: Result<(), &'static str>,
    /// Whether the steps bar is shown while a session runs.
    pub bar_shown: bool,
    /// What to say about Esc, as the Video pane does.
    pub esc: EscCaution,
    /// Whether Steps can take pictures here at all.
    pub available: Result<(), &'static str>,
}

/// Until the desktop has looked: everything works, as on Windows.
impl Default for StepsFacts {
    fn default() -> Self {
        Self {
            route: ClickRoute::Native,
            appimage: false,
            markers: Ok(()),
            bar_shown: true,
            esc: EscCaution::None,
            available: Ok(()),
        }
    }
}

/// Where the one-time click setup stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ClickSetup {
    #[default]
    Idle,
    /// pkexec is asking for the password.
    Waiting,
    /// It did not work; the reason.
    Failed(String),
}

const ROUTE_UDEV: &str = "Clicks come from the mouse through Windup's device rule.";
const ROUTE_INPUT_GROUP: &str = "Clicks come from the mouse; your account is in the input group.";
const ROUTE_AFTER_LOGOUT: &str = "Log out and back in to finish setting up clicks.";
/// The explanation before **Capture clicks too**, as the AppImage shows it.
const ROUTE_NOT_SET_UP: &str = "Wayland does not let applications see clicks in other windows. Windup can read the mouse directly if your account is in the input group. That group can also read the keyboard. You will be asked for your password, and it takes effect after you log out and back in.";
/// The same where a package is installed, which can set up a rule instead.
const ROUTE_NOT_SET_UP_PACKAGE: &str = "Wayland does not let applications see clicks in other windows. Windup can read the mouse directly if your account is in the input group. That group can also read the keyboard. You will be asked for your password, and it takes effect after you log out and back in. Reinstalling Windup's package sets up a rule for the mouse alone.";

/// Where the desktop hides the bar.
pub const STEPS_NO_BAR: &str = "On this desktop the bar is not shown; press the Steps key to stop.";

/// What the pane says about clicks on this desktop; nothing where the OS
/// hands them over.
pub fn route_line(route: ClickRoute, appimage: bool) -> Option<&'static str> {
    match route {
        ClickRoute::Native => None,
        ClickRoute::Udev => Some(ROUTE_UDEV),
        ClickRoute::InputGroup => Some(ROUTE_INPUT_GROUP),
        ClickRoute::AfterLogout => Some(ROUTE_AFTER_LOGOUT),
        ClickRoute::NotSetUp if appimage => Some(ROUTE_NOT_SET_UP),
        ClickRoute::NotSetUp => Some(ROUTE_NOT_SET_UP_PACKAGE),
        ClickRoute::NoPointer => Some(CLICKS_NO_POINTER),
    }
}

/// What can take a step, each with why it cannot be chosen: the choices
/// that use clicks are disabled where clicks cannot be seen.
pub fn trigger_choices(route: ClickRoute) -> Vec<(StepTrigger, Option<&'static str>)> {
    let clicks = clicks_reason(route).err();
    StepTrigger::ALL
        .into_iter()
        .map(|t| {
            (
                t,
                if t == StepTrigger::HotkeyOnly {
                    None
                } else {
                    clicks
                },
            )
        })
        .collect()
}

/// Sets the marker and saves, as the other rows do, when it changed.
/// `save` is `Config::save` outside the tests.
pub fn set_marker(config: &mut Config, marker: MarkerStyle, save: impl FnOnce(&Config)) -> bool {
    if config.steps.marker == marker {
        return false;
    }
    config.steps.marker = marker;
    save(config);
    true
}

/// The kind segment's short labels; the review's dropdown says more.
fn kind_label(k: MarkerKind) -> &'static str {
    match k {
        MarkerKind::Ring => "Ring",
        MarkerKind::Badge => "Badge",
        MarkerKind::Arrow => "Arrow",
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render(
    ui: &mut Ui,
    config: &mut Config,
    p: &Palette,
    mode: Mode,
    hint: HotkeyHint,
    facts: &StepsFacts,
    setup: &ClickSetup,
) -> Option<SettingsAction> {
    let mut action = None;

    eyebrow(ui, *p, "TOOL STATUS");

    row(
        ui,
        *p,
        "Enable Steps",
        Some("Record a step-by-step guide: a picture at each click or key press."),
        |ui| {
            let mut enabled = config.steps.enabled;
            if toggle(ui, *p, &mut enabled) {
                config.steps.enabled = enabled;
                config.save();
            }
        },
    );

    ui.add_space(metrics::S4);

    eyebrow(ui, *p, "SHORTCUTS");

    let mut shortcut = config.steps.shortcut.clone();
    if shortcut_row(
        ui,
        config,
        p,
        ToolId::Steps,
        "Shortcut",
        "Starts recording steps; pressed again, it stops and opens the review.",
        &mut shortcut,
    ) {
        config.steps.shortcut = shortcut;
        config.save();
    }

    let default_step = Shortcut::super_shift_x();
    row(
        ui,
        *p,
        "Take a step",
        Some("Adds a step where the pointer is. Only taken while steps are recorded."),
        |ui| {
            let mut step = config.steps.step_shortcut.clone();
            if shortcut_recorder(
                ui,
                p,
                ("cfg_shortcut", "steps_step"),
                &mut step,
                Some(&default_step),
            ) {
                config.steps.step_shortcut = step;
                config.save();
            }
        },
    );

    let default_pause = Shortcut::super_shift_p();
    row(
        ui,
        *p,
        "Pause",
        Some("Shared with Video. Pauses and resumes taking steps."),
        |ui| {
            let mut pause = config.video.pause_shortcut.clone();
            if shortcut_recorder(
                ui,
                p,
                ("cfg_shortcut", "steps_pause"),
                &mut pause,
                Some(&default_pause),
            ) {
                config.video.pause_shortcut = pause;
                config.save();
            }
        },
    );

    row(
        ui,
        *p,
        "Cancel",
        Some("Esc always cancels and throws the steps away."),
        |ui| {
            shortcut_badge(ui, p, "Esc");
        },
    );
    hotkey_note(ui, p, hint);

    ui.add_space(metrics::S4);

    eyebrow(ui, *p, "TAKING STEPS");

    let choices = trigger_choices(facts.route);
    let current = choices.iter().position(|(t, _)| *t == config.steps.trigger);
    let why = current.and_then(|i| choices[i].1);
    row(
        ui,
        *p,
        "What takes a step",
        Some(why.unwrap_or("Clicks on Windup's own bar never count.")),
        |ui| {
            let labels: Vec<String> = choices.iter().map(|(t, _)| t.label().to_owned()).collect();
            let gates: Vec<Option<&str>> = choices.iter().map(|(_, w)| *w).collect();
            if let Some(i) = dropdown_gated(
                ui,
                p,
                mode,
                "steps_trigger",
                &labels,
                &gates,
                current,
                config.steps.trigger.label(),
            ) {
                config.steps.trigger = choices[i].0;
                config.save();
            }
        },
    );
    if why.is_some() {
        caution_line(
            ui,
            p,
            "Until clicks can be seen, the Take a step key takes the steps.",
        );
    }

    if let Some(line) = route_line(facts.route, facts.appimage) {
        ui.add_space(metrics::S4);
        eyebrow(ui, *p, "CLICKS ON THIS DESKTOP");
        ui.label(RichText::new(line).size(text::SMALL).color(p.text_dim));
        if facts.route == ClickRoute::NotSetUp {
            ui.add_space(metrics::S2);
            let waiting = *setup == ClickSetup::Waiting;
            ui.add_enabled_ui(!waiting, |ui| {
                if button(ui, p, "Capture clicks too", true).clicked() {
                    action = Some(SettingsAction::JoinInputGroup);
                }
            });
            if waiting {
                ui.label(
                    RichText::new("Waiting for your password.")
                        .size(text::SMALL)
                        .color(p.text_dim),
                );
            }
        }
        if let ClickSetup::Failed(why) = setup {
            caution_line(ui, p, why);
        }
    }

    ui.add_space(metrics::S4);

    eyebrow(ui, *p, "MARKER");

    let mut style = config.steps.marker;
    row(
        ui,
        *p,
        "Marker",
        Some("Where the pointer was, drawn on each step's picture."),
        |ui| {
            segmented(ui, *p, &mut style.kind, &MarkerKind::ALL, kind_label);
        },
    );
    ui.add_space(metrics::S2);
    row(ui, *p, "Colour", None, |ui| {
        if let Some(c) = colour_swatch(ui, p, mode, style.colour) {
            style.colour = c;
        }
    });
    ui.add_space(metrics::S2);
    row(ui, *p, "Size", None, |ui| {
        let labels: Vec<String> = MarkerSize::ALL
            .iter()
            .map(|s| s.label().to_owned())
            .collect();
        let current = MarkerSize::ALL.iter().position(|s| *s == style.size);
        if let Some(i) = dropdown(
            ui,
            p,
            mode,
            "steps_marker_size",
            &labels,
            current,
            style.size.label(),
        ) {
            style.size = MarkerSize::ALL[i];
        }
    });
    set_marker(config, style, Config::save);
    ui.add_space(metrics::S2);
    row(ui, *p, "Preview", None, |ui| {
        marker_preview(ui, p, &config.steps.marker);
    });

    let cautions = [
        facts.markers.err(),
        facts.esc.line(),
        (!facts.bar_shown).then_some(STEPS_NO_BAR),
        facts.available.err(),
    ];
    if cautions.iter().any(Option::is_some) {
        ui.add_space(metrics::S2);
    }
    for line in cautions.into_iter().flatten() {
        caution_line(ui, p, line);
    }

    ui.add_space(metrics::S4);

    eyebrow(ui, *p, "EXPORT");

    row(
        ui,
        *p,
        "Export as",
        Some("A web page holds every picture in one file; Markdown and Images make a folder."),
        |ui| {
            let labels: Vec<String> = ExportFormat::ALL
                .iter()
                .map(|f| f.label().to_owned())
                .collect();
            let current = ExportFormat::ALL
                .iter()
                .position(|f| *f == config.steps.export);
            if let Some(i) = dropdown(
                ui,
                p,
                mode,
                "steps_export",
                &labels,
                current,
                config.steps.export.label(),
            ) {
                config.steps.export = ExportFormat::ALL[i];
                config.save();
            }
        },
    );

    ui.add_space(metrics::S2);

    let folder = config
        .steps
        .export_dir
        .clone()
        .or_else(output::default_steps_dir)
        .map(|d| d.display().to_string())
        .unwrap_or_else(|| "Documents".to_owned());
    row(ui, *p, "Export folder", Some(&folder), |ui| {
        if config.steps.export_dir.is_some() && button(ui, p, "Reset", false).clicked() {
            config.steps.export_dir = None;
            config.save();
        }
        if button(ui, p, "Choose folder...", false).clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            config.steps.export_dir = Some(dir);
            config.save();
        }
    });

    ui.add_space(metrics::S2);

    row(
        ui,
        *p,
        "File name",
        Some("{date} and {time} are replaced when the steps are exported."),
        |ui| {
            let mut pattern = config.steps.name_pattern.clone();
            let response = ui.add(
                egui::TextEdit::singleline(&mut pattern)
                    .font(egui::FontId::proportional(text::CONTROL))
                    .desired_width(metrics::DROPDOWN_W),
            );
            if response.changed() {
                config.steps.name_pattern = pattern;
                config.save();
            }
        },
    );

    action
}

/// A line in the caution colour (§7.8), under the rows it is about.
fn caution_line(ui: &mut Ui, p: &Palette, line: &str) {
    ui.label(RichText::new(line).size(text::SMALL).color(p.caution));
}

/// The line under the shortcut rows about binding the keys in the desktop.
fn hotkey_note(ui: &mut Ui, p: &Palette, hint: HotkeyHint) {
    let note = match hint {
        HotkeyHint::None => return,
        HotkeyHint::Command => {
            "Bind windup --steps in your desktop's keyboard settings; windup --step takes a step."
        }
        HotkeyHint::Cosmic => {
            "On COSMIC, change it in COSMIC Settings, Keyboard, Custom shortcuts too."
        }
    };
    ui.label(RichText::new(note).size(text::SMALL).color(p.text_dim));
}

/// The marker's colour: a swatch that opens the colour wheel (the capture
/// bar's picker). The colour is returned once the button is let go, so one
/// turn of the wheel is one save.
fn colour_swatch(ui: &mut Ui, p: &Palette, mode: Mode, colour: [u8; 4]) -> Option<[u8; 4]> {
    let id = ui.make_persistent_id("steps_marker_colour");
    let (hsv_id, pending_id) = (id.with("hsv"), id.with("pending"));
    let mut open = ui.data(|d| d.get_temp::<bool>(id).unwrap_or(false));
    let (rect, response) =
        ui.allocate_exact_size(vec2(metrics::BUTTON, metrics::BUTTON), Sense::click());
    let [r, g, b, _] = colour;
    if response.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_temp(hsv_id, Hsv::from_rgb([r, g, b])));
    }
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(
            rect,
            metrics::RADIUS_TIGHT,
            egui::Color32::from_rgb(r, g, b),
        );
        let ring = if open {
            p.accent
        } else if response.hovered() {
            p.text_dim
        } else {
            p.line
        };
        painter.rect_stroke(
            rect,
            metrics::RADIUS_TIGHT,
            Stroke::new(metrics::HAIRLINE, ring),
            egui::StrokeKind::Inside,
        );
    }

    let mut send = None;
    if open {
        let mut hsv = ui
            .data(|d| d.get_temp::<Hsv>(hsv_id))
            .unwrap_or_else(|| Hsv::from_rgb([r, g, b]));
        let mut pending = ui.data(|d| d.get_temp::<bool>(pending_id).unwrap_or(false));
        let side = metrics::WHEEL_POPOVER + 2.0 * metrics::PAD_PANEL;
        // Opens below the swatch, its right edge on the swatch's.
        let at = pos2(rect.right() - side, rect.bottom() + metrics::POPOVER_GAP);
        let area = egui::Area::new(id.with("popover"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(p.popover)
                    .corner_radius(metrics::RADIUS_CARD)
                    .stroke(Stroke::new(metrics::HAIRLINE, p.line_popover))
                    .shadow(shadow::menu(mode))
                    .inner_margin(metrics::PAD_PANEL)
                    .show(ui, |ui| {
                        if colour::wheel(ui, p, &mut hsv, metrics::WHEEL_POPOVER) {
                            pending = true;
                        }
                    });
            });
        if pending && !ui.input(|i| i.pointer.primary_down()) {
            pending = false;
            let [r, g, b] = hsv.to_rgb();
            send = Some([r, g, b, 255]);
        }
        let clicked_elsewhere = ui.input(|i| i.pointer.any_click())
            && !response.contains_pointer()
            && !area.response.contains_pointer();
        if clicked_elsewhere || ui.input(|i| i.key_pressed(Key::Escape)) {
            open = false;
        }
        ui.data_mut(|d| {
            d.insert_temp(hsv_id, hsv);
            d.insert_temp(pending_id, pending);
        });
    }
    ui.data_mut(|d| d.insert_temp(id, open));
    send
}

/// The marker on a sample tile, drawn as the review and the exports draw it:
/// the pointer on a button-like shape, the marker over it.
fn marker_preview(ui: &mut Ui, p: &Palette, style: &MarkerStyle) {
    let size = vec2(metrics::MARKER_TILE_W, metrics::MARKER_TILE_H);
    let (tile, _) = ui.allocate_exact_size(size, Sense::hover());
    if !ui.is_rect_visible(tile) {
        return;
    }
    let painter = ui.painter_at(tile);
    painter.rect_filled(tile, metrics::RADIUS, p.field);
    painter.rect_stroke(
        tile,
        metrics::RADIUS,
        Stroke::new(metrics::HAIRLINE, p.line),
        egui::StrokeKind::Inside,
    );
    // Low and to the left, so the badge and the arrow have room.
    let at = [0.35 * size.x, 0.68 * size.y];
    let target = Rect::from_center_size(
        pos2(tile.min.x + at[0], tile.min.y + at[1]),
        vec2(metrics::MARKER_TARGET_W, metrics::BUTTON),
    );
    painter.rect_filled(target, metrics::RADIUS, p.control);
    let prims = marker(style, at, 1, 1.0, (size.x as u32, size.y as u32));
    paint_prims(
        &painter,
        &prims,
        |q| pos2(tile.min.x + q[0], tile.min.y + q[1]),
        1.0,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use windup_core::steps::clicks::{CLICKS_NOT_SET_UP, clicks_reason};
    use windup_core::steps::options::{MarkerKind, MarkerSize};

    #[test]
    fn the_route_line_per_click_route() {
        assert_eq!(route_line(ClickRoute::Native, false), None);
        assert_eq!(route_line(ClickRoute::Native, true), None);
        assert_eq!(
            route_line(ClickRoute::AfterLogout, false),
            Some("Log out and back in to finish setting up clicks.")
        );
        assert_eq!(
            route_line(ClickRoute::Udev, false),
            Some("Clicks come from the mouse through Windup's device rule.")
        );
        assert_eq!(
            route_line(ClickRoute::InputGroup, true),
            Some("Clicks come from the mouse; your account is in the input group.")
        );
        assert_eq!(
            route_line(ClickRoute::NoPointer, false),
            Some("No mouse or touchpad was found.")
        );
        let appimage = route_line(ClickRoute::NotSetUp, true).unwrap();
        assert!(appimage.starts_with("Wayland does not let applications see clicks"));
        assert!(appimage.contains("can also read the keyboard"));
        assert!(!appimage.contains("Reinstalling"), "{appimage}");
        let package = route_line(ClickRoute::NotSetUp, false).unwrap();
        assert!(package.starts_with(appimage), "{package}");
        assert!(
            package.ends_with("Reinstalling Windup's package sets up a rule for the mouse alone."),
            "{package}"
        );
    }

    #[test]
    fn clicks_choices_are_disabled_without_clicks() {
        let choices = trigger_choices(ClickRoute::NotSetUp);
        let why = |t: StepTrigger| choices.iter().find(|(c, _)| *c == t).map(|(_, w)| *w);
        assert_eq!(why(StepTrigger::ClicksOnly), Some(Some(CLICKS_NOT_SET_UP)));
        assert_eq!(
            why(StepTrigger::ClicksAndHotkey),
            Some(Some(CLICKS_NOT_SET_UP))
        );
        assert_eq!(why(StepTrigger::HotkeyOnly), Some(None));
        assert_eq!(clicks_reason(ClickRoute::NotSetUp), Err(CLICKS_NOT_SET_UP));
        // Where clicks work, everything can be chosen, in the usual order.
        let all = trigger_choices(ClickRoute::Udev);
        assert_eq!(all, StepTrigger::ALL.map(|t| (t, None)).to_vec(), "{all:?}");
    }

    #[test]
    fn changing_the_marker_saves_it() {
        let mut config = Config::default();
        let ring = MarkerStyle {
            kind: MarkerKind::Ring,
            colour: [10, 20, 30, 255],
            size: MarkerSize::Large,
        };
        let mut saved = Vec::new();
        assert!(set_marker(&mut config, ring, |c| saved.push(c.steps.marker)));
        assert_eq!(config.steps.marker, ring);
        assert_eq!(saved, [ring]);
        // The same marker again changes nothing and writes nothing.
        assert!(!set_marker(&mut config, ring, |c| saved.push(c.steps.marker)));
        assert_eq!(saved.len(), 1);
    }
}
