//! The ffmpeg command lines, as argv lists. Pure: nothing here runs ffmpeg,
//! so the exact flags are testable without it.

use std::path::{Path, PathBuf};

use super::format::{AudioCodec, Codec, Container, audio_for};

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
    // Stamped by the wall clock as it arrives, so silence padded in late
    // never looks behind the picture (ffmpeg would stop reading the picture
    // to wait for it), then moved to start at zero: ffmpeg moves an input
    // there itself only when it learns its start, which a pipe with nothing
    // in it yet does not tell, and a track stamped from 1970 made the MP4
    // decades long, which GStreamer players refuse.
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
    let zeroed = |i: usize| format!("[{}:a]asetpts=PTS-STARTPTS", video_inputs + i);
    match audio.len() {
        0 => {}
        1 => graph.push(format!("{}[a]", zeroed(0))),
        _ => graph.push(format!(
            "{}[a0];{}[a1];[a0][a1]amix=inputs=2:normalize=0[a]",
            zeroed(0),
            zeroed(1)
        )),
    }
    if !graph.is_empty() {
        a.extend(["-filter_complex".into(), graph.join(";")]);
    }
    if vaapi && video_inputs == 1 {
        push(&mut a, &["-vf", "format=nv12,hwupload"]);
    }

    // Maps are only needed when the graph labels its streams; sound always
    // goes through it.
    if video_inputs == 0 {
        push(&mut a, &["-map", "[v]"]);
    } else if !audio.is_empty() {
        push(&mut a, &["-map", "0:v"]);
    }
    if !audio.is_empty() {
        push(&mut a, &["-map", "[a]"]);
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

/// The fastest a GIF plays: its frame delays are whole hundredths of a
/// second, and browsers slow anything under 2/100 s down to 1/10 s.
pub const GIF_MAX_FPS: u32 = 50;

/// The frame rate a GIF is written at for a recording at `fps`.
pub fn gif_fps(fps: u32) -> u32 {
    fps.clamp(1, GIF_MAX_FPS)
}

/// Where a GIF join keeps its palette: beside the list, in the segment
/// folder, so it goes with the segments.
pub fn palette_path(list_file: &Path) -> PathBuf {
    list_file.with_file_name("palette.png")
}

/// The ffmpeg runs that join the segments listed in a concat file into the
/// final file, in order; every one has to succeed.
///
/// A GIF takes two: the palette first, then the frames through it. One
/// graph doing both (`split` into `palettegen` and `paletteuse`) holds
/// every frame in memory until the palette is done at the end.
pub fn join_passes(
    list_file: &Path,
    out: &Path,
    container: Container,
    codec: Codec,
    gif_colours: u16,
    fps: u32,
) -> Vec<Vec<String>> {
    let concat = |a: &mut Vec<String>| {
        push(a, &["-hide_banner", "-loglevel", "error", "-y"]);
        push(a, &["-f", "concat", "-safe", "0", "-i"]);
        a.push(path_arg(list_file));
    };
    let mut a = Vec::new();
    concat(&mut a);
    match container {
        Container::Gif => {
            let fps = gif_fps(fps);
            let palette = palette_path(list_file);
            let mut first = a;
            first.extend([
                "-vf".into(),
                format!("fps={fps},palettegen=max_colors={gif_colours}"),
            ]);
            push(&mut first, &["-frames:v", "1", "-update", "1"]);
            first.push(path_arg(&palette));

            let mut second = Vec::new();
            concat(&mut second);
            second.extend(["-i".into(), path_arg(&palette)]);
            second.extend([
                "-filter_complex".into(),
                format!("[0:v]fps={fps}[x];[x][1:v]paletteuse"),
            ]);
            second.push(path_arg(out));
            return vec![first, second];
        }
        Container::Mp4 | Container::Mov => {
            push(&mut a, &["-c", "copy", "-movflags", "+faststart"]);
            // ffmpeg tags H.265 `hev1` by default, which Apple's players
            // refuse; `hvc1` plays everywhere.
            if codec == Codec::H265 {
                push(&mut a, &["-tag:v", "hvc1"]);
            }
        }
        Container::WebM | Container::Mkv => push(&mut a, &["-c", "copy"]),
    }
    a.push(path_arg(out));
    vec![a]
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
        assert_eq!(
            graph(&a),
            "[1:a]asetpts=PTS-STARTPTS[a0];[2:a]asetpts=PTS-STARTPTS[a1];\
             [a0][a1]amix=inputs=2:normalize=0[a]"
        );
        assert!(has(&a, "-map 0:v -map [a]"));
        assert!(has(
            &a,
            "-thread_queue_size 512 -use_wallclock_as_timestamps 1 -f f32le -ar 48000 -ac 2 -i a0"
        ));
        assert!(has(&a, "-c:a aac -b:a 160k"));
    }

    #[test]
    fn one_audio_input_starts_at_zero_and_ddagrab_shares_one_graph() {
        let mut j = job(raw(), "libx264");
        j.audio = audio(1);
        let a = segment_args(&j, Container::WebM);
        assert_eq!(graph(&a), "[1:a]asetpts=PTS-STARTPTS[a]");
        assert!(has(&a, "-map 0:v -map [a]"));
        assert!(has(&a, "-c:a libopus -b:a 128k"));

        let mut j = job(dda(), "h264_nvenc");
        j.audio = audio(2);
        let a = segment_args(&j, Container::Mp4);
        assert_eq!(a.iter().filter(|x| *x == "-filter_complex").count(), 1);
        assert!(graph(&a).contains("[v];[0:a]asetpts=PTS-STARTPTS[a0];[1:a]"));
        assert!(has(&a, "-map [v] -map [a]"));
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

    fn passes(container: Container, codec: Codec, colours: u16, fps: u32) -> Vec<Vec<String>> {
        let dir = Path::new("work");
        join_passes(
            &dir.join("list.txt"),
            Path::new(&format!("out.{}", container.ext())),
            container,
            codec,
            colours,
            fps,
        )
    }

    #[test]
    fn join_mp4_copies_with_faststart() {
        let p = passes(Container::Mp4, Codec::H264, 256, 30);
        assert_eq!(p.len(), 1, "one pass");
        let a = &p[0];
        let list = Path::new("work").join("list.txt");
        assert!(has(a, &format!("-f concat -safe 0 -i {}", list.display())));
        assert!(has(a, "-c copy -movflags +faststart"));
        assert!(!a.iter().any(|x| x == "-tag:v"));
        assert_eq!(a.last().unwrap(), "out.mp4");
        let m = &passes(Container::Mkv, Codec::H264, 256, 30)[0];
        assert!(has(m, "-c copy") && !has(m, "-movflags +faststart"));
    }

    #[test]
    fn h265_in_mp4_and_mov_is_tagged_for_apple_players() {
        for container in [Container::Mp4, Container::Mov] {
            let a = &passes(container, Codec::H265, 256, 30)[0];
            assert!(has(a, "-c copy"), "{a:?}");
            assert!(has(a, "-tag:v hvc1"), "{a:?}");
        }
        // Matroska has no such tag, and H.264 needs none.
        assert!(!has(
            &passes(Container::Mkv, Codec::H265, 256, 30)[0],
            "-tag:v hvc1"
        ));
        assert!(!has(
            &passes(Container::Mov, Codec::H264, 256, 30)[0],
            "-tag:v hvc1"
        ));
    }

    #[test]
    fn join_gif_makes_the_palette_first_then_uses_it() {
        let p = passes(Container::Gif, Codec::H264, 128, 15);
        assert_eq!(p.len(), 2, "two passes, so no frame is held in memory");
        let palette = Path::new("work").join("palette.png");
        let palette = palette.to_string_lossy();
        let (one, two) = (&p[0], &p[1]);
        assert!(has(one, "-vf fps=15,palettegen=max_colors=128"), "{one:?}");
        assert_eq!(one.last().unwrap(), &*palette);
        assert!(!one.iter().any(|x| x.contains("paletteuse")));
        let list = Path::new("work").join("list.txt");
        assert!(has(two, &format!("-i {} -i {palette}", list.display())));
        assert_eq!(graph(two), "[0:v]fps=15[x];[x][1:v]paletteuse");
        assert!(!two.iter().any(|x| x.contains("palettegen")));
        assert_eq!(two.last().unwrap(), "out.gif");
        assert!(!has(two, "-c copy"));
    }

    #[test]
    fn a_gif_runs_at_fifty_frames_a_second_at_most() {
        // GIF delays are in hundredths: 60 fps would be 1/100 s frames,
        // which browsers slow down.
        assert_eq!(gif_fps(15), 15);
        assert_eq!(gif_fps(50), 50);
        assert_eq!(gif_fps(60), 50);
        assert_eq!(gif_fps(144), 50);
        let p = passes(Container::Gif, Codec::H264, 256, 60);
        assert!(p[0].iter().any(|x| x.starts_with("fps=50,")));
        assert!(graph(&p[1]).starts_with("[0:v]fps=50["));
    }
}
