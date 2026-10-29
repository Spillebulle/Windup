//! Which encoders a given ffmpeg can really use. Having a name in `ffmpeg
//! -encoders` proves little (a build lists NVENC on a machine with no Nvidia
//! card), so each candidate is asked to encode one frame and the ones that
//! succeed are the choices. The running of ffmpeg is the desktop's job; this
//! is the list, the arguments and the choice.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::format::{AudioCodec, Codec};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Encoder {
    pub name: &'static str,
    pub hardware: bool,
}

const fn hw(name: &'static str) -> Encoder {
    Encoder {
        name,
        hardware: true,
    }
}

const fn sw(name: &'static str) -> Encoder {
    Encoder {
        name,
        hardware: false,
    }
}

const H264: &[Encoder] = &[
    hw("h264_nvenc"),
    hw("h264_amf"),
    hw("h264_qsv"),
    hw("h264_vaapi"),
    sw("libx264"),
];
const H265: &[Encoder] = &[
    hw("hevc_nvenc"),
    hw("hevc_amf"),
    hw("hevc_qsv"),
    hw("hevc_vaapi"),
    sw("libx265"),
];
const AV1: &[Encoder] = &[
    hw("av1_nvenc"),
    hw("av1_amf"),
    hw("av1_qsv"),
    hw("av1_vaapi"),
    sw("libsvtav1"),
];
const VP9: &[Encoder] = &[hw("vp9_qsv"), hw("vp9_vaapi"), sw("libvpx-vp9")];

const H264_NO_VAAPI: &[Encoder] = &[
    hw("h264_nvenc"),
    hw("h264_amf"),
    hw("h264_qsv"),
    sw("libx264"),
];
const H265_NO_VAAPI: &[Encoder] = &[
    hw("hevc_nvenc"),
    hw("hevc_amf"),
    hw("hevc_qsv"),
    sw("libx265"),
];
const AV1_NO_VAAPI: &[Encoder] = &[
    hw("av1_nvenc"),
    hw("av1_amf"),
    hw("av1_qsv"),
    sw("libsvtav1"),
];
const VP9_NO_VAAPI: &[Encoder] = &[hw("vp9_qsv"), sw("libvpx-vp9")];

/// Encoders worth trying for a codec, best first: NVENC, AMF, QSV, VA-API
/// (Linux only), then software. AMF is listed everywhere and left to the
/// probe, which is the only thing that really knows.
pub fn candidates(codec: Codec, linux: bool) -> &'static [Encoder] {
    match (codec, linux) {
        (Codec::H264, true) => H264,
        (Codec::H264, false) => H264_NO_VAAPI,
        (Codec::H265, true) => H265,
        (Codec::H265, false) => H265_NO_VAAPI,
        (Codec::Av1, true) => AV1,
        (Codec::Av1, false) => AV1_NO_VAAPI,
        (Codec::Vp9, true) => VP9,
        (Codec::Vp9, false) => VP9_NO_VAAPI,
    }
}

/// The audio encoder ffmpeg names for a codec.
pub fn audio_encoder(codec: AudioCodec) -> &'static str {
    match codec {
        AudioCodec::Aac => "aac",
        AudioCodec::Opus => "libopus",
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probe {
    pub ffmpeg_version: String,
    /// Names of the video encoders that encoded a frame.
    pub working: Vec<String>,
    /// Audio codecs whose encoder passed.
    pub audio: Vec<AudioCodec>,
}

/// Identifies one ffmpeg file, so a replaced binary is probed again and an
/// untouched one is not.
pub fn cache_key(path: &Path, len: u64, mtime_secs: u64) -> String {
    format!("{}|{len}|{mtime_secs}", path.display())
}

/// The saved choice if it works, else the best encoder that does.
pub fn pick_encoder(codec: Codec, wanted: Option<&str>, probe: &Probe) -> Option<&'static str> {
    let works = |name: &str| probe.working.iter().any(|w| w == name);
    // Both lists are the same up to VA-API, so the longer one finds `wanted`.
    let list = candidates(codec, true);
    if let Some(w) = wanted
        && let Some(e) = list.iter().find(|e| e.name == w)
        && works(e.name)
    {
        return Some(e.name);
    }
    list.iter().map(|e| e.name).find(|n| works(n))
}

/// One frame of black through the encoder into nowhere.
pub fn probe_args(enc: &Encoder) -> Vec<String> {
    let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostdin"]
        .map(String::from)
        .into();
    let vaapi = enc.name.ends_with("_vaapi");
    if vaapi {
        args.extend(["-vaapi_device", "/dev/dri/renderD128"].map(String::from));
    }
    args.extend(["-f", "lavfi", "-i", "color=black:s=256x256:d=0.04"].map(String::from));
    if vaapi {
        args.extend(["-vf", "format=nv12,hwupload"].map(String::from));
    }
    args.extend(["-frames:v", "1", "-c:v", enc.name, "-f", "null", "-"].map(String::from));
    args
}

/// A fraction of a second of silence through the audio encoder.
pub fn audio_probe_args(codec: AudioCodec) -> Vec<String> {
    [
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-f",
        "lavfi",
        "-i",
        "anullsrc=r=48000:cl=stereo",
        "-t",
        "0.1",
        "-c:a",
        audio_encoder(codec),
        "-f",
        "null",
        "-",
    ]
    .map(String::from)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_with(names: &[&str]) -> Probe {
        Probe {
            ffmpeg_version: "7.1".into(),
            working: names.iter().map(|s| s.to_string()).collect(),
            audio: vec![],
        }
    }

    #[test]
    fn a_saved_encoder_that_is_gone_falls_back() {
        let p = probe_with(&["libx264"]);
        assert_eq!(
            pick_encoder(Codec::H264, Some("h264_nvenc"), &p),
            Some("libx264")
        );
    }

    #[test]
    fn a_saved_encoder_that_works_is_kept() {
        let p = probe_with(&["h264_qsv", "libx264"]);
        assert_eq!(
            pick_encoder(Codec::H264, Some("libx264"), &p),
            Some("libx264")
        );
    }

    #[test]
    fn hardware_comes_first() {
        let p = probe_with(&["h264_qsv", "libx264"]);
        assert_eq!(pick_encoder(Codec::H264, None, &p), Some("h264_qsv"));
    }

    #[test]
    fn no_encoder_for_the_codec_is_none() {
        let p = probe_with(&["libx264"]);
        assert_eq!(pick_encoder(Codec::Av1, None, &p), None);
    }

    #[test]
    fn vaapi_is_linux_only() {
        let has = |linux| {
            candidates(Codec::H264, linux)
                .iter()
                .any(|e| e.name == "h264_vaapi")
        };
        assert!(has(true));
        assert!(!has(false));
    }

    #[test]
    fn software_is_last_and_not_hardware() {
        for codec in [Codec::H264, Codec::H265, Codec::Av1, Codec::Vp9] {
            let last = candidates(codec, true).last().unwrap();
            assert!(!last.hardware);
        }
    }

    #[test]
    fn cache_key_changes_with_size_and_time() {
        let p = Path::new("ffmpeg");
        let base = cache_key(p, 10, 20);
        assert_ne!(base, cache_key(p, 11, 20));
        assert_ne!(base, cache_key(p, 10, 21));
        assert_eq!(base, cache_key(p, 10, 20));
    }

    #[test]
    fn vaapi_probe_uploads_the_frame() {
        let args = probe_args(&hw("h264_vaapi")).join(" ");
        assert!(args.contains("-vaapi_device /dev/dri/renderD128"));
        assert!(args.contains("-vf format=nv12,hwupload"));
        assert!(!probe_args(&sw("libx264")).join(" ").contains("vaapi"));
    }
}
