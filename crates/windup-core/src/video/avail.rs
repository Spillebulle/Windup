//! Whether a recording can work here, and if not, the reason the launcher
//! bar gives (style guide §7.6: a disabled control says why). Pure: the
//! desktop gathers the facts (the probe, the frame sources, the sound
//! devices) and these decide.

use super::format::{Codec, Container};
use super::pcm::{NO_SOUND_DEVICE, SOUND_NEEDS_PIPEWIRE, SOUND_STILL_LOOKING};
use super::probe::{Probe, pick_encoder};
use crate::capture::backend::Moving;

pub const NO_FFMPEG: &str = "Video needs FFmpeg, which is missing from this installation.";
pub const PROBE_RUNNING: &str = "Windup is still checking which encoders work.";
pub const NO_ENCODER: &str = "No video encoder works on this computer.";
pub const NO_SCREEN_SOURCE: &str = "This desktop does not let applications record the screen.";

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
/// moving pictures. A GIF is encoded by ffmpeg's own `gif`, which needs no
/// probe, so only ffmpeg and the frame source matter for it.
pub fn video(
    probed: Probed,
    container: Container,
    codec: Codec,
    order: &[Moving],
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
    if order.iter().all(|m| *m == Moving::None) {
        return Err(NO_SCREEN_SOURCE);
    }
    Ok(())
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
        return (Err(SOUND_STILL_LOOKING), Err(SOUND_STILL_LOOKING));
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

    const WINDOWS: &[Moving] = &[Moving::Ddagrab, Moving::Gdigrab];

    #[test]
    fn video_reasons_come_in_order() {
        let mp4 = |p, order| video(p, Container::Mp4, Codec::H264, order);
        assert_eq!(mp4(Probed::NoFfmpeg, &[Moving::None]), Err(NO_FFMPEG));
        assert_eq!(mp4(Probed::Running, &[Moving::None]), Err(PROBE_RUNNING));
        let none = probe(&[]);
        assert_eq!(mp4(Probed::Done(&none), &[Moving::None]), Err(NO_ENCODER));
        let x264 = probe(&["libx264"]);
        assert_eq!(
            mp4(Probed::Done(&x264), &[Moving::None]),
            Err(NO_SCREEN_SOURCE)
        );
        assert_eq!(mp4(Probed::Done(&x264), WINDOWS), Ok(()));
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
        assert_eq!(gif(Probed::Running, &[Moving::None]), Err(NO_SCREEN_SOURCE));
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
            (Err(SOUND_STILL_LOOKING), Err(SOUND_STILL_LOOKING))
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
