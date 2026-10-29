//! Whether a recording can work here, and if not, the reason the launcher
//! bar gives (style guide §7.6: a disabled control says why). Pure: the
//! desktop gathers the facts (the probe, the frame sources, the sound
//! devices) and these decide.

use super::format::{Codec, Container};
use super::pcm::{NO_SOUND_DEVICE, SOUND_NEEDS_PIPEWIRE, SOUND_STILL_CHECKING};
use super::probe::{Probe, pick_encoder};
use crate::capture::backend::Moving;

pub const NO_FFMPEG: &str = "Video needs FFmpeg, which is missing from this installation.";
pub const PROBE_RUNNING: &str = "Windup is still checking which encoders work.";
pub const NO_ENCODER: &str = "No video encoder works on this computer.";
pub const NO_SCREEN_SOURCE: &str = "This desktop does not let applications record the screen.";
pub const PROBE_GAVE_UP: &str =
    "Windup has not finished checking encoders. Reopen settings once it has.";
pub const SOURCES_CHECKING: &str = "Windup is still checking how this desktop can be recorded.";

/// The probe as the process that runs it publishes it, and as the settings
/// window is handed it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ProbeState {
    /// No answer yet, not even a cached one.
    #[default]
    Running,
    Done(Probe),
    NoFfmpeg,
    /// The settings window waited for another process's probe and stopped
    /// waiting: no answer will come to this process.
    GaveUp,
}

impl ProbeState {
    pub fn probed(&self) -> Probed<'_> {
        match self {
            ProbeState::Running | ProbeState::GaveUp => Probed::Running,
            ProbeState::Done(p) => Probed::Done(p),
            ProbeState::NoFfmpeg => Probed::NoFfmpeg,
        }
    }
}

/// What the encoder probe has found so far.
#[derive(Clone, Copy, Debug)]
pub enum Probed<'a> {
    NoFfmpeg,
    /// No answer yet, not even a cached one.
    Running,
    Done(&'a Probe),
}

/// Video's reason, the first that applies: no ffmpeg, the probe still
/// running, no working encoder for `codec` nor any H.264 one, no way to take
/// moving pictures (`sources`, the `video_order`; `None` while it is still
/// being found). A GIF is encoded by ffmpeg's own `gif`, which needs no
/// probe, so only ffmpeg and the frame source matter for it.
pub fn video(
    probed: Probed,
    container: Container,
    codec: Codec,
    sources: Option<&[Moving]>,
) -> Result<(), &'static str> {
    let gif = container == Container::Gif;
    match probed {
        Probed::NoFfmpeg => return Err(NO_FFMPEG),
        Probed::Running if !gif => return Err(PROBE_RUNNING),
        Probed::Done(p)
            if !gif
                && pick_encoder(codec, None, p).is_none()
                && pick_encoder(Codec::H264, None, p).is_none() =>
        {
            return Err(NO_ENCODER);
        }
        _ => {}
    }
    match sources {
        None => Err(SOURCES_CHECKING),
        Some(order) if order.iter().all(|m| *m == Moving::None) => Err(NO_SCREEN_SOURCE),
        Some(_) => Ok(()),
    }
}

/// The sound devices there are to record from, by the names a recording
/// opens them with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioDevices {
    /// Where computer sound can be recorded from.
    pub outputs: Vec<String>,
    /// Microphones.
    pub inputs: Vec<String>,
    /// False when there is no sound system to ask (Linux without PipeWire).
    pub available: bool,
}

impl AudioDevices {
    pub fn counts(&self) -> SoundDevices {
        SoundDevices {
            system: self.available,
            outputs: self.outputs.len(),
            inputs: self.inputs.len(),
        }
    }
}

/// The sound devices as last listed, by count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoundDevices {
    /// False without a sound system to ask (Linux without PipeWire).
    pub system: bool,
    pub outputs: usize,
    pub inputs: usize,
}

/// The reasons for (computer sound, microphone). `None` is a list not yet
/// made.
pub fn sound(
    devices: Option<SoundDevices>,
) -> (Result<(), &'static str>, Result<(), &'static str>) {
    let Some(d) = devices else {
        return (Err(SOUND_STILL_CHECKING), Err(SOUND_STILL_CHECKING));
    };
    if !d.system {
        return (Err(SOUND_NEEDS_PIPEWIRE), Err(SOUND_NEEDS_PIPEWIRE));
    }
    let some = |n: usize| if n > 0 { Ok(()) } else { Err(NO_SOUND_DEVICE) };
    (some(d.outputs), some(d.inputs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(working: &[&str]) -> Probe {
        Probe {
            working: working.iter().map(|s| s.to_string()).collect(),
            ..Probe::default()
        }
    }

    const WINDOWS: Option<&[Moving]> = Some(&[Moving::Ddagrab, Moving::Gdigrab]);
    const NOTHING: Option<&[Moving]> = Some(&[Moving::None]);

    #[test]
    fn video_reasons_come_in_order() {
        let mp4 = |p, order| video(p, Container::Mp4, Codec::H264, order);
        assert_eq!(mp4(Probed::NoFfmpeg, NOTHING), Err(NO_FFMPEG));
        assert_eq!(mp4(Probed::Running, NOTHING), Err(PROBE_RUNNING));
        let none = probe(&[]);
        assert_eq!(mp4(Probed::Done(&none), NOTHING), Err(NO_ENCODER));
        let x264 = probe(&["libx264"]);
        assert_eq!(mp4(Probed::Done(&x264), NOTHING), Err(NO_SCREEN_SOURCE));
        assert_eq!(mp4(Probed::Done(&x264), WINDOWS), Ok(()));
        assert_eq!(mp4(Probed::Done(&x264), None), Err(SOURCES_CHECKING));
    }

    #[test]
    fn h264_stands_in_for_a_codec_that_does_not_work() {
        let x264 = probe(&["libx264"]);
        assert_eq!(
            video(Probed::Done(&x264), Container::Mp4, Codec::Av1, WINDOWS),
            Ok(())
        );
    }

    #[test]
    fn a_gif_needs_ffmpeg_and_a_source_but_no_probe() {
        let gif = |p, order| video(p, Container::Gif, Codec::H264, order);
        assert_eq!(gif(Probed::Running, WINDOWS), Ok(()));
        let none = probe(&[]);
        assert_eq!(gif(Probed::Done(&none), WINDOWS), Ok(()));
        assert_eq!(gif(Probed::NoFfmpeg, WINDOWS), Err(NO_FFMPEG));
        assert_eq!(gif(Probed::Running, NOTHING), Err(NO_SCREEN_SOURCE));
    }

    #[test]
    fn sound_reasons_are_per_toggle() {
        let d = |system, outputs, inputs| {
            Some(SoundDevices {
                system,
                outputs,
                inputs,
            })
        };
        assert_eq!(
            sound(None),
            (Err(SOUND_STILL_CHECKING), Err(SOUND_STILL_CHECKING))
        );
        assert_eq!(
            sound(d(false, 0, 0)),
            (Err(SOUND_NEEDS_PIPEWIRE), Err(SOUND_NEEDS_PIPEWIRE))
        );
        assert_eq!(sound(d(true, 1, 0)), (Ok(()), Err(NO_SOUND_DEVICE)));
        assert_eq!(sound(d(true, 0, 2)), (Err(NO_SOUND_DEVICE), Ok(())));
        assert_eq!(sound(d(true, 1, 1)), (Ok(()), Ok(())));
    }
}
