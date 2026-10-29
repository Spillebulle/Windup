//! The ffmpeg command lines, as argv lists. Pure: nothing here runs ffmpeg,
//! so the exact flags are testable without it.

use std::path::{Path, PathBuf};

use super::format::{AudioCodec, Container, audio_for};

/// Where the picture comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// Desktop Duplication on Windows, with the frames kept on the GPU when
    /// the encoder can take them.
    Ddagrab {
        output: u32,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    },
    Gdigrab {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    },
    X11grab {
        display: String,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    },
    /// BGRA frames at the recording's frame rate, written to a pipe by us.
    RawPipe {
        path: String,
        w: u32,
        h: u32,
    },
}

/// Sound written to a pipe by us, as f32le.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioIn {
    pub path: String,
    pub rate: u32,
    pub channels: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentJob {
    pub source: Source,
    pub fps: u32,
    pub encoder: &'static str,
    /// `None` for a GIF, and when the encoder should pick its own.
    pub bitrate_kbps: Option<u32>,
    pub audio: Vec<AudioIn>,
    pub out: PathBuf,
}

/// Every segment is a Matroska file of the final codec, so the join can copy.
pub fn segment_ext(_container: Container) -> &'static str {
    "mkv"
}

const VAAPI_DEVICE: &str = "/dev/dri/renderD128";

fn push(args: &mut Vec<String>, items: &[&str]) {
    args.extend(items.iter().map(|s| s.to_string()));
}

fn path_arg(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

pub fn segment_args(job: &SegmentJob, container: Container) -> Vec<String> {
    let gif = container == Container::Gif;
    let encoder = job.encoder;
    let vaapi = !gif && encoder.ends_with("_vaapi");
    let software = gif || encoder.starts_with("lib");
    let audio: &[AudioIn] = if gif { &[] } else { &job.audio };
    let fps = job.fps;

    let mut a = Vec::new();
    push(&mut a, &["-hide_banner", "-loglevel", "error", "-y"]);
    if vaapi && !matches!(job.source, Source::Ddagrab { .. }) {
        push(&mut a, &["-vaapi_device", VAAPI_DEVICE]);
    }

    // Ddagrab is a filter, so it has no `-i` and sits in the graph.
    let mut graph: Vec<String> = Vec::new();
    let mut video_inputs = 0;
    match &job.source {
        Source::Gdigrab { x, y, w, h } => {
            push(&mut a, &["-thread_queue_size", "512", "-f", "gdigrab"]);
            a.extend(["-framerate".into(), fps.to_string()]);
            a.extend(["-offset_x".into(), x.to_string()]);
            a.extend(["-offset_y".into(), y.to_string()]);
            a.extend(["-video_size".into(), format!("{w}x{h}")]);
            push(&mut a, &["-draw_mouse", "1", "-i", "desktop"]);
            video_inputs = 1;
        }
        Source::X11grab {
            display,
            x,
            y,
            w,
            h,
        } => {
            push(&mut a, &["-thread_queue_size", "512", "-f", "x11grab"]);
            a.extend(["-framerate".into(), fps.to_string()]);
            a.extend(["-video_size".into(), format!("{w}x{h}")]);
            push(&mut a, &["-draw_mouse", "1"]);
            a.extend(["-i".into(), format!("{display}+{x},{y}")]);
            video_inputs = 1;
        }
        Source::RawPipe { path, w, h } => {
            push(&mut a, &["-thread_queue_size", "512"]);
            push(&mut a, &["-f", "rawvideo", "-pix_fmt", "bgra"]);
            a.extend(["-s".into(), format!("{w}x{h}")]);
            a.extend(["-r".into(), fps.to_string()]);
            a.extend(["-i".into(), path.clone()]);
            video_inputs = 1;
        }
        Source::Ddagrab { output, x, y, w, h } => {
            let tail = if !gif && (encoder.ends_with("_nvenc") || encoder.ends_with("_amf")) {
                ""
            } else if !gif && encoder.ends_with("_qsv") {
                ",hwmap=derive_device=qsv,format=qsv"
            } else {
                ",hwdownload,format=bgra"
            };
            graph.push(format!(
                "ddagrab=output_idx={output}:framerate={fps}:offset_x={x}:offset_y={y}:video_size={w}x{h}:draw_mouse=1{tail}[v]"
            ));
        }
    }

    // Sound inputs come after the picture, so their indices start there.
    for inp in audio {
        push(&mut a, &["-thread_queue_size", "512"]);
        push(
            &mut a,
            &["-use_wallclock_as_timestamps", "1", "-f", "f32le"],
        );
        a.extend(["-ar".into(), inp.rate.to_string()]);
        a.extend(["-ac".into(), inp.channels.to_string()]);
        a.extend(["-i".into(), inp.path.clone()]);
    }
    let mixed = audio.len() == 2;
    if mixed {
        graph.push(format!(
            "[{video_inputs}:a][{}:a]amix=inputs=2:normalize=0[a]",
            video_inputs + 1
        ));
    }
    if !graph.is_empty() {
        a.extend(["-filter_complex".into(), graph.join(";")]);
    }
    if vaapi && video_inputs == 1 {
        push(&mut a, &["-vf", "format=nv12,hwupload"]);
    }

    // Maps are only needed when the graph labels its streams or the sound
    // has to be picked from inputs of its own.
    if video_inputs == 0 {
        push(&mut a, &["-map", "[v]"]);
    } else if !audio.is_empty() {
        push(&mut a, &["-map", "0:v"]);
    }
    if mixed {
        push(&mut a, &["-map", "[a]"]);
    } else if audio.len() == 1 {
        a.extend(["-map".into(), format!("{video_inputs}:a")]);
    }

    if gif {
        push(&mut a, &["-c:v", "ffv1"]);
    } else {
        push(&mut a, &["-c:v", encoder]);
        match encoder {
            "libx264" => push(&mut a, &["-preset", "veryfast"]),
            "libx265" => push(&mut a, &["-preset", "fast"]),
            "libsvtav1" => push(&mut a, &["-preset", "8"]),
            "libvpx-vp9" => push(
                &mut a,
                &["-deadline", "realtime", "-cpu-used", "8", "-row-mt", "1"],
            ),
            _ => {}
        }
        if let Some(n) = job.bitrate_kbps {
            a.extend([
                "-b:v".into(),
                format!("{n}k"),
                "-maxrate".into(),
                format!("{n}k"),
                "-bufsize".into(),
                format!("{}k", n * 2),
            ]);
        }
    }
    let sw_h26x = encoder == "libx264" || encoder == "libx265";
    if !gif && software && (sw_h26x || video_inputs == 1) {
        push(&mut a, &["-pix_fmt", "yuv420p"]);
    }

    if !audio.is_empty()
        && let Some(codec) = audio_for(container)
    {
        match codec {
            AudioCodec::Aac => push(&mut a, &["-c:a", "aac", "-b:a", "160k"]),
            AudioCodec::Opus => push(&mut a, &["-c:a", "libopus", "-b:a", "128k"]),
        }
    }

    push(&mut a, &["-f", "matroska"]);
    a.push(path_arg(&job.out));
    a
}

/// Joins the segments listed in a concat file into the final file.
pub fn join_args(
    list_file: &Path,
    out: &Path,
    container: Container,
    gif_colours: u16,
    fps: u32,
) -> Vec<String> {
    let mut a = Vec::new();
    push(&mut a, &["-hide_banner", "-loglevel", "error", "-y"]);
    push(&mut a, &["-f", "concat", "-safe", "0", "-i"]);
    a.push(path_arg(list_file));
    match container {
        Container::Gif => a.extend([
            "-filter_complex".into(),
            format!(
                "[0:v]fps={fps},split[a][b];[a]palettegen=max_colors={gif_colours}[p];[b][p]paletteuse"
            ),
        ]),
        Container::Mp4 | Container::Mov => {
            push(&mut a, &["-c", "copy", "-movflags", "+faststart"]);
        }
        Container::WebM | Container::Mkv => push(&mut a, &["-c", "copy"]),
    }
    a.push(path_arg(out));
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(source: Source, encoder: &'static str) -> SegmentJob {
        SegmentJob {
            source,
            fps: 30,
            encoder,
            bitrate_kbps: None,
            audio: vec![],
            out: PathBuf::from("seg0.mkv"),
        }
    }

    fn dda() -> Source {
        Source::Ddagrab {
            output: 0,
            x: 10,
            y: 20,
            w: 640,
            h: 358,
        }
    }

    fn raw() -> Source {
        Source::RawPipe {
            path: "pipe1".into(),
            w: 640,
            h: 358,
        }
    }

    /// Whether the words appear in this order, next to each other.
    fn has(args: &[String], seq: &str) -> bool {
        let want: Vec<&str> = seq.split(' ').collect();
        args.windows(want.len())
            .any(|w| w.iter().zip(&want).all(|(a, b)| a == b))
    }

    fn audio(n: usize) -> Vec<AudioIn> {
        (0..n)
            .map(|i| AudioIn {
                path: format!("a{i}"),
                rate: 48000,
                channels: 2,
            })
            .collect()
    }

    fn graph(args: &[String]) -> &str {
        let i = args.iter().position(|a| a == "-filter_complex").unwrap();
        &args[i + 1]
    }

    #[test]
    fn ddagrab_nvenc_keeps_frames_on_the_gpu() {
        let a = segment_args(&job(dda(), "h264_nvenc"), Container::Mp4);
        assert!(graph(&a).starts_with(
            "ddagrab=output_idx=0:framerate=30:offset_x=10:offset_y=20:video_size=640x358"
        ));
        assert!(!graph(&a).contains("hwdownload"));
        assert!(has(&a, "-map [v]"));
        assert!(!a.iter().any(|x| x == "-i"));
    }

    #[test]
    fn ddagrab_software_downloads_the_frames() {
        let a = segment_args(&job(dda(), "libx264"), Container::Mp4);
        assert!(graph(&a).contains("hwdownload,format=bgra"));
        assert!(has(&a, "-pix_fmt yuv420p"));
        assert!(has(&a, "-preset veryfast"));
    }

    #[test]
    fn ddagrab_qsv_maps_to_qsv() {
        let a = segment_args(&job(dda(), "h264_qsv"), Container::Mp4);
        assert!(graph(&a).contains(",hwmap=derive_device=qsv,format=qsv"));
    }

    #[test]
    fn every_argv_starts_quiet_and_keeps_stdin() {
        let a = segment_args(&job(raw(), "libx264"), Container::Mp4);
        assert!(has(&a[..4], "-hide_banner -loglevel error -y"));
        assert!(!a.iter().any(|x| x == "-nostdin"));
    }

    #[test]
    fn raw_pipe_input() {
        let a = segment_args(&job(raw(), "libx264"), Container::Mp4);
        assert!(has(
            &a,
            "-f rawvideo -pix_fmt bgra -s 640x358 -r 30 -i pipe1"
        ));
        assert!(has(&a, "-thread_queue_size 512 -f rawvideo"));
        assert!(has(&a, "-f matroska seg0.mkv"));
    }

    #[test]
    fn gdigrab_input() {
        let s = Source::Gdigrab {
            x: -5,
            y: 7,
            w: 640,
            h: 358,
        };
        let a = segment_args(&job(s, "libx264"), Container::Mp4);
        assert!(has(
            &a,
            "-f gdigrab -framerate 30 -offset_x -5 -offset_y 7 -video_size 640x358 -draw_mouse 1 -i desktop"
        ));
        assert!(has(&a, "-pix_fmt yuv420p"));
    }

    #[test]
    fn x11grab_input() {
        let s = Source::X11grab {
            display: ":0".into(),
            x: 1,
            y: 2,
            w: 640,
            h: 358,
        };
        let a = segment_args(&job(s, "libx264"), Container::Mp4);
        assert!(has(
            &a,
            "-f x11grab -framerate 30 -video_size 640x358 -draw_mouse 1 -i :0+1,2"
        ));
    }

    #[test]
    fn vaapi_uploads_raw_frames() {
        let a = segment_args(&job(raw(), "h264_vaapi"), Container::Mp4);
        assert!(has(&a, "-vaapi_device /dev/dri/renderD128"));
        assert!(has(&a, "-vf format=nv12,hwupload"));
        assert!(!has(&a, "-pix_fmt yuv420p"));
    }

    #[test]
    fn two_audio_inputs_are_mixed() {
        let mut j = job(raw(), "libx264");
        j.audio = audio(2);
        let a = segment_args(&j, Container::Mp4);
        assert!(graph(&a).contains("amix=inputs=2:normalize=0"));
        assert!(has(&a, "-map 0:v -map [a]"));
        assert!(has(
            &a,
            "-thread_queue_size 512 -use_wallclock_as_timestamps 1 -f f32le -ar 48000 -ac 2 -i a0"
        ));
        assert!(has(&a, "-c:a aac -b:a 160k"));
    }

    #[test]
    fn one_audio_input_is_mapped_directly_and_ddagrab_shares_one_graph() {
        let mut j = job(raw(), "libx264");
        j.audio = audio(1);
        let a = segment_args(&j, Container::WebM);
        assert!(!a.iter().any(|x| x == "-filter_complex"));
        assert!(has(&a, "-map 0:v -map 1:a"));
        assert!(has(&a, "-c:a libopus -b:a 128k"));

        let mut j = job(dda(), "h264_nvenc");
        j.audio = audio(2);
        let a = segment_args(&j, Container::Mp4);
        assert_eq!(a.iter().filter(|x| *x == "-filter_complex").count(), 1);
        assert!(graph(&a).contains("[v];[0:a][1:a]amix"));
    }

    #[test]
    fn fixed_bitrate() {
        let mut j = job(raw(), "libx264");
        j.bitrate_kbps = Some(8000);
        let a = segment_args(&j, Container::Mp4);
        assert!(has(&a, "-b:v 8000k -maxrate 8000k -bufsize 16000k"));
    }

    #[test]
    fn gif_segments_are_lossless_and_silent() {
        let mut j = job(raw(), "libx264");
        j.audio = audio(1);
        let a = segment_args(&j, Container::Gif);
        assert!(has(&a, "-c:v ffv1"));
        assert!(!a.iter().any(|x| x == "-c:a" || x == "libx264"));
        assert!(!a.iter().any(|x| x == "-map"));
        assert_eq!(segment_ext(Container::Mp4), "mkv");
        assert_eq!(segment_ext(Container::Gif), "mkv");
    }

    #[test]
    fn join_mp4_copies_with_faststart() {
        let a = join_args(
            Path::new("list.txt"),
            Path::new("out.mp4"),
            Container::Mp4,
            256,
            30,
        );
        assert!(has(&a, "-f concat -safe 0 -i list.txt"));
        assert!(has(&a, "-c copy -movflags +faststart"));
        assert_eq!(a.last().unwrap(), "out.mp4");
        let m = join_args(Path::new("l"), Path::new("o.mkv"), Container::Mkv, 256, 30);
        assert!(has(&m, "-c copy") && !has(&m, "-movflags +faststart"));
    }

    #[test]
    fn join_gif_builds_a_palette() {
        let a = join_args(
            Path::new("list.txt"),
            Path::new("out.gif"),
            Container::Gif,
            128,
            15,
        );
        let g = graph(&a);
        assert!(g.contains("palettegen=max_colors=128") && g.contains("paletteuse"));
        assert!(g.contains("fps=15"));
        assert!(!has(&a, "-c copy"));
    }
}
