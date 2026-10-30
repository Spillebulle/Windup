//! Video tool settings: the shortcuts, where recordings go, the format and
//! the sound. What works here (the encoder probe, the sound devices) comes
//! from the desktop as plain data in [`VideoFacts`].

use super::capture_cfg::{hotkey_note, shortcut_row};
use super::{EscCaution, HotkeyHint};
use crate::capture::bar::{bitrate_label, colours_label, fps_label};
use crate::theme::{Mode, Palette, metrics, text};
use crate::widgets::{
    button, caution_keycap, dropdown, eyebrow, row, shortcut_badge, shortcut_recorder, toggle,
};
use egui::{RichText, Ui};
use windup_core::Config;
use windup_core::capture::output;
use windup_core::config::VideoConfig;
use windup_core::hotkeys::{self, Action};
use windup_core::shortcut::Shortcut;
use windup_core::tool::ToolId;
use windup_core::video::avail::{
    self, AudioDevices, NO_FFMPEG, PROBE_GAVE_UP, PROBE_RUNNING, ProbeState,
};
use windup_core::video::{
    Bitrate, Codec, Container, FIXED_BITRATES, GIF_COLOURS, candidates, codecs_for, frame_rates,
};

/// What the desktop found, for the rows that depend on it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VideoFacts {
    pub probe: ProbeState,
    /// `None` until the devices have been listed.
    pub sound: Option<AudioDevices>,
}

const GIF_NO_CODEC: &str = "A GIF has no codec; it keeps its own colours.";
const GIF_NO_SOUND: &str = "A GIF has no sound.";
const NO_CODEC: &str = "No encoder works for this container; choose another.";
const NO_ENCODER_FOR_CODEC: &str = "No encoder works for this codec; choose another.";

/// The codecs to offer for `container`: the ones it takes that have an
/// encoder that works. Empty until the probe has answered.
pub fn codec_choices(container: Container, probe: &ProbeState) -> Vec<Codec> {
    codecs_for(container)
        .iter()
        .copied()
        .filter(|c| !encoder_choices(*c, probe).is_empty())
        .collect()
}

/// The encoders that work for `codec`, best first.
pub fn encoder_choices(codec: Codec, probe: &ProbeState) -> Vec<&'static str> {
    let ProbeState::Done(p) = probe else {
        return Vec::new();
    };
    // The Linux list holds every name; the probe only passes ones that ran.
    candidates(codec, true)
        .iter()
        .map(|e| e.name)
        .filter(|n| p.working.iter().any(|w| w == n))
        .collect()
}

/// Sets the container, moving the codec to the first valid one when the new
/// container does not take it. A GIF keeps the codec it has, for when the
/// container changes back.
pub fn set_container(video: &mut VideoConfig, container: Container, probe: &ProbeState) {
    video.container = container;
    let takes = codecs_for(container);
    if takes.is_empty() {
        return;
    }
    let working = codec_choices(container, probe);
    let valid = if working.is_empty() {
        takes.contains(&video.codec)
    } else {
        working.contains(&video.codec)
    };
    if !valid {
        let first = working.first().copied().unwrap_or(takes[0]);
        set_codec(video, first, probe);
    }
}

/// Sets the codec, and goes back to an automatic encoder when the chosen one
/// does not do this codec.
pub fn set_codec(video: &mut VideoConfig, codec: Codec, probe: &ProbeState) {
    video.codec = codec;
    if let Some(name) = &video.encoder
        && !encoder_choices(codec, probe).contains(&name.as_str())
    {
        video.encoder = None;
    }
}

fn container_label(c: Container) -> &'static str {
    match c {
        Container::Mp4 => "MP4",
        Container::WebM => "WebM",
        Container::Mov => "MOV",
        Container::Mkv => "MKV",
        Container::Gif => "GIF",
    }
}

fn codec_label(c: Codec) -> &'static str {
    match c {
        Codec::H264 => "H.264",
        Codec::H265 => "H.265",
        Codec::Av1 => "AV1",
        Codec::Vp9 => "VP9",
    }
}

/// `NVENC (Nvidia)`, `Software (libx264)`.
fn encoder_label(name: &str) -> String {
    let hardware = [
        ("_nvenc", "NVENC (Nvidia)"),
        ("_amf", "AMF (AMD)"),
        ("_qsv", "Quick Sync (Intel)"),
        ("_vaapi", "VA-API"),
    ];
    hardware
        .iter()
        .find(|(suffix, _)| name.ends_with(suffix))
        .map(|(_, label)| (*label).to_owned())
        .unwrap_or_else(|| format!("Software ({name})"))
}

/// Why the codec and encoder rows cannot be used, if they cannot.
fn format_reason(video: &VideoConfig, probe: &ProbeState) -> Option<&'static str> {
    if video.container == Container::Gif {
        return Some(GIF_NO_CODEC);
    }
    match probe {
        ProbeState::NoFfmpeg => Some(NO_FFMPEG),
        ProbeState::Running => Some(PROBE_RUNNING),
        ProbeState::GaveUp => Some(PROBE_GAVE_UP),
        ProbeState::Done(_) => codec_choices(video.container, probe)
            .is_empty()
            .then_some(NO_CODEC),
    }
}

/// A row whose control is disabled when `why` is set, which then takes the
/// description's place (§7.6: a disabled control says why).
fn gated_row(
    ui: &mut Ui,
    p: &Palette,
    label: &str,
    under: &str,
    why: Option<&str>,
    control: impl FnOnce(&mut Ui),
) {
    row(ui, *p, label, Some(why.unwrap_or(under)), |ui| {
        ui.add_enabled_ui(why.is_none(), control);
    });
}

#[allow(clippy::too_many_arguments)]
pub fn render(
    ui: &mut Ui,
    config: &mut Config,
    p: &Palette,
    mode: Mode,
    hint: HotkeyHint,
    facts: &VideoFacts,
    esc: EscCaution,
) {
    eyebrow(ui, *p, "TOOL STATUS");

    row(
        ui,
        *p,
        "Enable Video",
        Some("Record the screen, a window or an area to a video file or a GIF."),
        |ui| {
            let mut enabled = config.video.enabled;
            if toggle(ui, *p, &mut enabled) {
                config.video.enabled = enabled;
                config.save();
            }
        },
    );

    ui.add_space(metrics::S4);

    eyebrow(ui, *p, "SHORTCUTS");

    let mut shortcut = config.video.shortcut.clone();
    if shortcut_row(
        ui,
        config,
        p,
        ToolId::Video,
        "Video shortcut",
        "Opens the capture bar for a recording; pressed again, it stops one.",
        &mut shortcut,
    ) {
        config.video.shortcut = shortcut;
        config.save();
    }

    // The stop key is the Video key, whose row above already warns.
    let pause_clash = hotkeys::recording_clashes(config)
        .into_iter()
        .find(|(action, _)| *action == Action::RecordPause)
        .map(|(_, tool)| tool.display_name());
    let default_pause = Shortcut::super_shift_p();
    row(
        ui,
        *p,
        "Pause shortcut",
        Some("Pauses and resumes a recording. Only taken while one runs."),
        |ui| {
            let mut pause = config.video.pause_shortcut.clone();
            if shortcut_recorder(
                ui,
                p,
                ("cfg_shortcut", "video_pause"),
                &mut pause,
                Some(&default_pause),
            ) {
                config.video.pause_shortcut = pause;
                config.save();
            }
            if let Some(name) = pause_clash {
                caution_keycap(ui, p, &format!("Also used by {name}"))
                    .on_hover_text("While a recording runs, these keys pause it instead.");
            }
        },
    );
    hotkey_note(ui, p, hint);

    ui.add_space(metrics::S4);

    eyebrow(ui, *p, "SAVING");

    let folder = config
        .video
        .save_dir
        .clone()
        .or_else(output::default_video_dir)
        .map(|d| d.display().to_string())
        .unwrap_or_else(|| "Videos".to_owned());
    row(ui, *p, "Save folder", Some(&folder), |ui| {
        if config.video.save_dir.is_some() && button(ui, p, "Reset", false).clicked() {
            config.video.save_dir = None;
            config.save();
        }
        if button(ui, p, "Choose folder...", false).clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            config.video.save_dir = Some(dir);
            config.save();
        }
    });

    ui.add_space(metrics::S2);

    row(
        ui,
        *p,
        "File name",
        Some("{date} and {time} are replaced when the file is saved."),
        |ui| {
            let mut pattern = config.video.name_pattern.clone();
            let response = ui.add(
                egui::TextEdit::singleline(&mut pattern)
                    .font(egui::FontId::proportional(text::CONTROL))
                    .desired_width(metrics::DROPDOWN_W),
            );
            if response.changed() {
                config.video.name_pattern = pattern;
                config.save();
            }
        },
    );

    ui.add_space(metrics::S4);

    eyebrow(ui, *p, "FORMAT");

    row(
        ui,
        *p,
        "Container",
        Some("MP4 plays almost anywhere; a GIF has no sound."),
        |ui| {
            let labels: Vec<String> = Container::ALL
                .iter()
                .map(|c| container_label(*c).to_owned())
                .collect();
            let current = Container::ALL
                .iter()
                .position(|c| *c == config.video.container);
            if let Some(i) = dropdown(
                ui,
                p,
                mode,
                "video_container",
                &labels,
                current,
                container_label(config.video.container),
            ) {
                set_container(&mut config.video, Container::ALL[i], &facts.probe);
                config.save();
            }
        },
    );

    ui.add_space(metrics::S2);

    let why = format_reason(&config.video, &facts.probe);
    let codecs = codec_choices(config.video.container, &facts.probe);
    gated_row(
        ui,
        p,
        "Codec",
        "Only codecs this computer can encode are offered.",
        why,
        |ui| {
            let labels: Vec<String> = codecs.iter().map(|c| codec_label(*c).to_owned()).collect();
            let current = codecs.iter().position(|c| *c == config.video.codec);
            let shown = if config.video.container == Container::Gif {
                "None"
            } else {
                codec_label(config.video.codec)
            };
            if let Some(i) = dropdown(ui, p, mode, "video_codec", &labels, current, shown) {
                set_codec(&mut config.video, codecs[i], &facts.probe);
                config.save();
            }
        },
    );

    ui.add_space(metrics::S2);

    let encoders = encoder_choices(config.video.codec, &facts.probe);
    let encoder_why = why.or_else(|| encoders.is_empty().then_some(NO_ENCODER_FOR_CODEC));
    gated_row(
        ui,
        p,
        "Encoder",
        "Automatic uses the graphics card's encoder when it works.",
        encoder_why,
        |ui| {
            let mut labels = vec!["Automatic".to_owned()];
            labels.extend(encoders.iter().map(|e| encoder_label(e)));
            let current = match &config.video.encoder {
                None => Some(0),
                Some(name) => encoders.iter().position(|e| e == name).map(|i| i + 1),
            };
            let shown = match current {
                Some(i) => labels[i].clone(),
                None => config
                    .video
                    .encoder
                    .as_deref()
                    .map(encoder_label)
                    .unwrap_or_default(),
            };
            if let Some(i) = dropdown(ui, p, mode, "video_encoder", &labels, current, &shown) {
                config.video.encoder = i.checked_sub(1).map(|i| encoders[i].to_owned());
                config.save();
            }
        },
    );

    ui.add_space(metrics::S4);

    eyebrow(ui, *p, "QUALITY");

    row(
        ui,
        *p,
        "Frame rate",
        Some("The capture bar also offers a faster monitor's own rate."),
        |ui| {
            let mut rates = frame_rates(None);
            if !rates.contains(&config.video.fps) {
                rates.push(config.video.fps);
                rates.sort_unstable();
            }
            let labels: Vec<String> = rates.iter().map(|r| fps_label(*r)).collect();
            let current = rates.iter().position(|r| *r == config.video.fps);
            let shown = fps_label(config.video.fps);
            if let Some(i) = dropdown(ui, p, mode, "video_fps", &labels, current, &shown) {
                config.video.fps = rates[i];
                config.save();
            }
        },
    );

    ui.add_space(metrics::S2);

    if config.video.container == Container::Gif {
        row(
            ui,
            *p,
            "Colours",
            Some("Fewer colours make a smaller GIF."),
            |ui| {
                let labels: Vec<String> = GIF_COLOURS.iter().map(|n| colours_label(*n)).collect();
                let current = GIF_COLOURS
                    .iter()
                    .position(|n| *n == config.video.gif_colours);
                let shown = colours_label(config.video.gif_colours);
                if let Some(i) = dropdown(ui, p, mode, "video_colours", &labels, current, &shown) {
                    config.video.gif_colours = GIF_COLOURS[i];
                    config.save();
                }
            },
        );
    } else {
        row(
            ui,
            *p,
            "Bitrate",
            Some("Auto follows the size, the frame rate and the codec."),
            |ui| {
                let choices: Vec<Bitrate> = std::iter::once(Bitrate::Auto)
                    .chain(FIXED_BITRATES.iter().map(|k| Bitrate::Kbps(*k)))
                    .collect();
                let labels: Vec<String> = choices.iter().map(|b| bitrate_label(*b)).collect();
                let current = choices.iter().position(|b| *b == config.video.bitrate);
                let shown = bitrate_label(config.video.bitrate);
                if let Some(i) = dropdown(ui, p, mode, "video_bitrate", &labels, current, &shown) {
                    config.video.bitrate = choices[i];
                    config.save();
                }
            },
        );
    }

    ui.add_space(metrics::S4);

    eyebrow(ui, *p, "SOUND");

    let (system_why, mic_why) = if config.video.container == Container::Gif {
        (Some(GIF_NO_SOUND), Some(GIF_NO_SOUND))
    } else {
        let (system, mic) = avail::sound(facts.sound.as_ref().map(AudioDevices::counts));
        (system.err(), mic.err())
    };
    let none = Vec::new();
    let (outputs, inputs) = facts
        .sound
        .as_ref()
        .map_or((&none, &none), |d| (&d.outputs, &d.inputs));

    let VideoConfig {
        system_audio,
        system_device,
        mic,
        mic_device,
        ..
    } = &mut config.video;
    let mut changed = sound_row(
        ui,
        p,
        mode,
        ("Computer sound", "What the computer plays.", system_why),
        "video_system_device",
        outputs,
        system_audio,
        system_device,
    );
    ui.add_space(metrics::S2);
    changed |= sound_row(
        ui,
        p,
        mode,
        ("Microphone", "Your voice, over the recording.", mic_why),
        "video_mic_device",
        inputs,
        mic,
        mic_device,
    );
    if changed {
        config.save();
    }

    ui.add_space(metrics::S4);

    eyebrow(ui, *p, "CANCEL");

    row(
        ui,
        *p,
        "Cancel",
        Some("Always cancels a recording and discards it."),
        |ui| {
            shortcut_badge(ui, p, "Esc");
        },
    );
    if let Some(line) = esc.line() {
        ui.label(RichText::new(line).size(text::SMALL).color(p.caution));
    }
}

/// A sound toggle with its device to its left. `(label, under, why)`; the
/// device is `None` for the system's default. Returns whether either changed.
#[allow(clippy::too_many_arguments)]
fn sound_row(
    ui: &mut Ui,
    p: &Palette,
    mode: Mode,
    (label, under, why): (&str, &str, Option<&str>),
    id: &'static str,
    devices: &[String],
    on: &mut bool,
    device: &mut Option<String>,
) -> bool {
    let mut changed = false;
    gated_row(ui, p, label, under, why, |ui| {
        changed |= toggle(ui, *p, on);
        ui.add_space(metrics::S2);
        let mut labels = vec!["Default device".to_owned()];
        labels.extend(devices.iter().cloned());
        let current = match device {
            None => Some(0),
            Some(name) => devices.iter().position(|d| d == name).map(|i| i + 1),
        };
        let shown = device.clone().unwrap_or_else(|| labels[0].clone());
        if let Some(i) = dropdown(ui, p, mode, id, &labels, current, &shown) {
            *device = i.checked_sub(1).map(|i| devices[i].clone());
            changed = true;
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use windup_core::video::Probe;

    fn probe(working: &[&str]) -> ProbeState {
        ProbeState::Done(Probe {
            working: working.iter().map(|s| s.to_string()).collect(),
            ..Probe::default()
        })
    }

    #[test]
    fn changing_container_keeps_the_codec_valid() {
        let probe = probe(&["libx264", "libx265", "libsvtav1", "libvpx-vp9"]);
        let mut video = VideoConfig {
            container: Container::Mp4,
            codec: Codec::H265,
            ..VideoConfig::default()
        };
        set_container(&mut video, Container::WebM, &probe);
        assert_eq!(video.container, Container::WebM);
        assert!(
            matches!(video.codec, Codec::Vp9 | Codec::Av1),
            "{:?}",
            video.codec
        );
    }

    #[test]
    fn codec_list_shows_only_working_ones() {
        let probe = probe(&["libx264"]);
        assert_eq!(codec_choices(Container::Mp4, &probe), [Codec::H264]);
    }

    #[test]
    fn a_container_that_takes_the_codec_keeps_it_and_a_gif_changes_nothing() {
        let probe = probe(&["libx264", "libx265"]);
        let mut video = VideoConfig {
            codec: Codec::H265,
            encoder: Some("libx265".into()),
            ..VideoConfig::default()
        };
        set_container(&mut video, Container::Mov, &probe);
        assert_eq!(
            (video.codec, video.encoder.as_deref()),
            (Codec::H265, Some("libx265"))
        );
        set_container(&mut video, Container::Gif, &probe);
        assert_eq!(
            (video.codec, video.encoder.as_deref()),
            (Codec::H265, Some("libx265"))
        );
    }

    #[test]
    fn a_new_codec_drops_an_encoder_that_cannot_do_it() {
        let probe = probe(&["libx264", "libvpx-vp9"]);
        let mut video = VideoConfig {
            encoder: Some("libx264".into()),
            ..VideoConfig::default()
        };
        set_container(&mut video, Container::WebM, &probe);
        assert_eq!(video.codec, Codec::Vp9);
        assert_eq!(video.encoder, None);
    }

    #[test]
    fn before_the_probe_answers_nothing_is_offered_and_rows_say_why() {
        assert!(codec_choices(Container::Mp4, &ProbeState::Running).is_empty());
        let video = VideoConfig::default();
        assert_eq!(
            format_reason(&video, &ProbeState::Running),
            Some(PROBE_RUNNING)
        );
        assert_eq!(
            format_reason(&video, &ProbeState::GaveUp),
            Some(PROBE_GAVE_UP)
        );
        assert_eq!(
            format_reason(&video, &ProbeState::NoFfmpeg),
            Some(NO_FFMPEG)
        );
        assert_eq!(format_reason(&video, &probe(&["libx264"])), None);
        assert_eq!(format_reason(&video, &probe(&[])), Some(NO_CODEC));
    }

    #[test]
    fn encoders_read_as_their_hardware() {
        assert_eq!(encoder_label("h264_nvenc"), "NVENC (Nvidia)");
        assert_eq!(encoder_label("libx264"), "Software (libx264)");
    }
}
