//! Containers, codecs and the numbers the settings and the encoder share.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Container {
    #[default]
    Mp4,
    WebM,
    Mov,
    Mkv,
    Gif,
}

impl Container {
    pub const ALL: [Container; 5] = [
        Container::Mp4,
        Container::WebM,
        Container::Mov,
        Container::Mkv,
        Container::Gif,
    ];

    /// The file extension, without the dot.
    pub fn ext(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::WebM => "webm",
            Container::Mov => "mov",
            Container::Mkv => "mkv",
            Container::Gif => "gif",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Codec {
    #[default]
    H264,
    H265,
    Av1,
    Vp9,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioCodec {
    Aac,
    Opus,
}

/// The video codecs a container takes, the first being the usual choice.
/// A GIF has no codec.
pub fn codecs_for(container: Container) -> &'static [Codec] {
    match container {
        Container::Mp4 => &[Codec::H264, Codec::H265, Codec::Av1],
        Container::WebM => &[Codec::Vp9, Codec::Av1],
        Container::Mov => &[Codec::H264, Codec::H265],
        Container::Mkv => &[Codec::H264, Codec::H265, Codec::Av1, Codec::Vp9],
        Container::Gif => &[],
    }
}

/// The audio codec a container carries, or `None` when it carries no sound.
pub fn audio_for(container: Container) -> Option<AudioCodec> {
    match container {
        Container::Mp4 | Container::Mov => Some(AudioCodec::Aac),
        Container::WebM | Container::Mkv => Some(AudioCodec::Opus),
        Container::Gif => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Bitrate {
    #[default]
    Auto,
    /// Kilobits per second.
    Kbps(u32),
}

/// The fixed choices the settings offer besides Auto, in kb/s.
pub const FIXED_BITRATES: [u32; 6] = [2000, 4000, 8000, 12000, 20000, 40000];

/// The palette sizes a GIF can use.
pub const GIF_COLOURS: [u16; 4] = [256, 128, 64, 32];

/// A bitrate in kb/s that follows the picture: 8 Mb/s for 1080p30 H.264,
/// scaled by pixels, half as much again at twice the rate, and 40% less for
/// the codecs that need it. Never below 1 Mb/s.
pub fn auto_bitrate(w: u32, h: u32, fps: u32, codec: Codec) -> u32 {
    let pixels = f64::from(w) * f64::from(h);
    let fps = f64::from(fps);
    let rate = if fps < 30.0 {
        fps / 30.0
    } else {
        1.0 + 0.5 * (fps - 30.0) / 30.0
    };
    let codec = if codec == Codec::H264 { 1.0 } else { 0.6 };
    let kbps = 8000.0 * pixels / (1920.0 * 1080.0) * rate * codec;
    ((kbps / 100.0).round() * 100.0).clamp(1000.0, 100_000.0) as u32
}

/// The frame rates to offer: the usual ones, plus a faster monitor's own.
pub fn frame_rates(max_refresh_hz: Option<u32>) -> Vec<u32> {
    let mut rates = vec![15, 24, 30, 60];
    if let Some(hz) = max_refresh_hz
        && hz > 60
    {
        rates.push(hz);
    }
    rates
}

/// Encoders want even sizes: rounds each side down. `None` when a side would
/// be under 2.
pub fn even_size(w: u32, h: u32) -> Option<(u32, u32)> {
    let (w, h) = (w & !1, h & !1);
    (w >= 2 && h >= 2).then_some((w, h))
}

/// What a recording is encoded with, worked out once for ffmpeg and the
/// recording bar's chip alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobRate {
    /// The configured codec if the container takes it, else the container's
    /// usual one (the configured one for a GIF, which has none).
    pub codec: Codec,
    /// The area's size, rounded down to even sides.
    pub size: (u32, u32),
    /// The bitrate in kb/s (Auto worked out for the size), `None` for a GIF.
    pub kbps: Option<u32>,
}

/// The codec and bitrate a recording of `size` runs with. `None` when the
/// area is too small to record.
pub fn job_rate(
    container: Container,
    codec: Codec,
    bitrate: Bitrate,
    fps: u32,
    size: (u32, u32),
) -> Option<JobRate> {
    let codecs = codecs_for(container);
    let codec = if codecs.contains(&codec) || codecs.is_empty() {
        codec
    } else {
        codecs[0]
    };
    let (w, h) = even_size(size.0, size.1)?;
    let kbps = match (container, bitrate) {
        (Container::Gif, _) => None,
        (_, Bitrate::Auto) => Some(auto_bitrate(w, h, fps, codec)),
        (_, Bitrate::Kbps(n)) => Some(n),
    };
    Some(JobRate {
        codec,
        size: (w, h),
        kbps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_job_rate_falls_back_to_the_containers_codec_and_works_out_auto() {
        // WebM does not take H.264: VP9, at 0.6 of H.264's rate.
        let r = job_rate(
            Container::WebM,
            Codec::H264,
            Bitrate::Auto,
            30,
            (1921, 1081),
        )
        .unwrap();
        assert_eq!(r.codec, Codec::Vp9);
        assert_eq!(r.size, (1920, 1080));
        assert_eq!(r.kbps, Some(auto_bitrate(1920, 1080, 30, Codec::Vp9)));
        let fixed = job_rate(
            Container::Mp4,
            Codec::H265,
            Bitrate::Kbps(4000),
            60,
            (640, 480),
        );
        assert_eq!(fixed.unwrap().kbps, Some(4000));
        assert_eq!(fixed.unwrap().codec, Codec::H265);
        let gif = job_rate(Container::Gif, Codec::H264, Bitrate::Auto, 15, (640, 480));
        assert_eq!(gif.unwrap().kbps, None);
        assert_eq!(
            job_rate(Container::Mp4, Codec::H264, Bitrate::Auto, 30, (1, 480)),
            None
        );
    }

    #[test]
    fn mp4_takes_h264_h265_av1_and_aac() {
        assert_eq!(
            codecs_for(Container::Mp4),
            &[Codec::H264, Codec::H265, Codec::Av1]
        );
        assert_eq!(audio_for(Container::Mp4), Some(AudioCodec::Aac));
    }

    #[test]
    fn webm_takes_vp9_av1_and_opus() {
        assert_eq!(codecs_for(Container::WebM), &[Codec::Vp9, Codec::Av1]);
        assert_eq!(audio_for(Container::WebM), Some(AudioCodec::Opus));
    }

    #[test]
    fn gif_takes_no_codec_and_no_audio() {
        assert!(codecs_for(Container::Gif).is_empty());
        assert_eq!(audio_for(Container::Gif), None);
    }

    #[test]
    fn every_container_has_an_extension() {
        let exts: Vec<_> = Container::ALL.iter().map(|c| c.ext()).collect();
        assert_eq!(exts, ["mp4", "webm", "mov", "mkv", "gif"]);
    }

    #[test]
    fn auto_bitrate_follows_pixels_rate_and_codec() {
        assert_eq!(auto_bitrate(1920, 1080, 30, Codec::H264), 8000);
        assert_eq!(auto_bitrate(1920, 1080, 60, Codec::H264), 12000);
        assert_eq!(auto_bitrate(1920, 1080, 30, Codec::H265), 4800);
        assert_eq!(auto_bitrate(3840, 2160, 30, Codec::H264), 32000);
        assert_eq!(auto_bitrate(1920, 1080, 15, Codec::H264), 4000);
        assert_eq!(auto_bitrate(2, 2, 15, Codec::Av1), 1000); // floor 1 Mb/s
    }

    #[test]
    fn frame_rates_add_a_faster_monitor() {
        assert_eq!(frame_rates(Some(144)), vec![15, 24, 30, 60, 144]);
        assert_eq!(frame_rates(Some(60)), vec![15, 24, 30, 60]);
        assert_eq!(frame_rates(None), vec![15, 24, 30, 60]);
    }

    #[test]
    fn odd_sizes_round_down_to_even_and_tiny_ones_are_none() {
        assert_eq!(even_size(641, 359), Some((640, 358)));
        assert_eq!(even_size(1, 500), None);
        assert_eq!(even_size(2, 2), Some((2, 2)));
    }
}
