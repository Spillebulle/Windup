# Capture, phase 2 (video) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Record an area, a window's rectangle or a whole screen to MP4, WebM, MOV, MKV or GIF with a recording bar that is never in the recording, the same on Windows, X11 and every Wayland host.

**Architecture:** Windup drives a **bundled `ffmpeg` executable**; it does not link FFmpeg. Every decision that can be pure lives in `windup-core::video`: which codecs and containers go together, which encoders the probe found, the automatic bitrate, the ffmpeg command lines, the recording state machine, frame pacing and per-frame cropping. Where ffmpeg can read the screen itself it does: `ddagrab` on Windows, which is zero-copy to the hardware encoders, and `x11grab` on X11. On Wayland Windup reads the frames itself and pipes raw BGRA into ffmpeg. wlroots and COSMIC use the screencopy protocols the stills already use. KDE and GNOME use the ScreenCast portal over PipeWire. Pause closes a segment and resume starts the next, so every source, and audio, pauses the same way. Stop joins the segments into the chosen container.

**Tech Stack:**
- Existing: Rust 2024; egui/eframe 0.32 and egui-wgpu; windows-sys 0.61; x11rb; smithay-client-toolkit 0.20; wayland-protocols 0.32; ashpd 0.11.
- New, desktop crate only:
  - `cpal` (Apache-2.0), for WASAPI loopback and the microphone on Windows.
  - `pipewire` 0.8 (MIT), for ScreenCast streams and audio on Linux. It links libpipewire-0.3; CI needs `libpipewire-0.3-dev` and `libclang-dev`.
- FFmpeg comes from BtbN's GPL builds (win64, winarm64, linux64), pinned by tag and SHA-256.

**Spec:** `docs/superpowers/specs/2026-10-28-capture-design.md`, the sections "Video", "Hotkeys", "Escape", "Capture back ends", "Settings", "Phases" and "Known risks". Read them first; this plan does not repeat their reasons.

## Global Constraints

- **Same experience everywhere.** Only the capture underneath differs. No system picker in normal use. GNOME and KDE get at most a one-time ScreenCast grant, which asks for **every monitor** and keeps the restore token.
- **The bar is never in a recording.** Where the recording bar goes on each platform:
  - Windows: where the launcher was, with `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`.
  - Linux layer host and X11: just outside the recorded rectangle, above it and else below it. Failing that, on another monitor. Failing that, not shown.
  - GNOME and Weston (the window host): never shown.
- **Recording bar contents:**
  - A red dot in `critical`, pulsing slowly. The exception to style guide §13 is commented in the code. Under reduced motion the dot does not pulse.
  - A mono timer, plus pause and stop.
  - Space pauses and Enter stops, only while the bar has focus.
- **Before recording starts**, the bar offers a frame-rate dropdown, a bitrate dropdown (a colour-count dropdown for GIF) and the two audio toggles next to the start button.
  - Once recording runs, these become read-only chips (`30 fps`, `8 Mb/s`).
  - Whatever is picked on the bar becomes the new default.
- **Frame rates** are 15, 24, 30 and 60, plus the monitor's refresh rate if it is higher. The default is **30**.
- **Bitrate** defaults to **Auto**: 8 Mb/s for 1920×1080 at 30 fps with H.264, ×1.5 at 60 fps, ×0.6 for H.265, AV1 and VP9.
  - The fixed choices are 2, 4, 8, 12, 20 and 40 Mb/s.
  - GIF colour counts are 256, 128, 64 and 32.
- **Containers and codecs:**

  | Container | Video codecs | Audio |
  |---|---|---|
  | MP4 (default) | H.264 (default), H.265, AV1 | AAC |
  | WebM | VP9, AV1 | Opus |
  | MOV | H.264, H.265 | AAC |
  | MKV | H.264, H.265, AV1, VP9 | Opus |
  | GIF | none | none |

  - Hardware encoders come first: NVENC, AMF, QSV, and VA-API on Linux. Software encoders come after: libx264, libx265, libsvtav1 and libvpx-vp9.
  - Only encoders that the **probe** encoded one real frame with are offered.
- **Audio:** computer sound and the microphone are **both off by default**. The devices are chosen in settings.
- **Files** are saved as `<Videos>/Windup/Recording YYYY-MM-DD HHMMSS.<ext>`. After saving, a toast says where the file went and has an "Open folder" action.
- **Window** records the rectangle the window had when recording started.
- **Esc during a recording cancels and discards it.** Esc is taken globally while the recording runs and cannot be rebound.
  - Where a desktop cannot bind it (Sway with no GlobalShortcuts portal, GNOME without the portal), the Video settings pane says so.
- **Hotkeys:**
  - Video defaults to Win+Shift+R (Super+Shift+R on Linux).
  - **Stop is the Video hotkey pressed again.**
  - **Pause is Win+Shift+P** (Super+Shift+P). It is configurable and bound only while a recording runs.
- **A control that cannot work is not offered** (rule 7):
  - The Video mode is drawn disabled with a tooltip saying why: no ffmpeg, no working encoder, no frame source on this desktop, or the probe still running.
  - Audio toggles are disabled the same way.
- The rules from phase 1 still hold:
  - Painted UI, tokens only, Lucide only.
  - British spelling, Sentence case, no em dashes in UI copy.
  - Every Win32 block `cfg(windows)` with a no-op twin. Linux code under `windup-desktop/src/linux/`. macOS still compiles.
  - CI warning-free on Windows x64, Linux and ARM64 with `-D warnings`.
  - `cosmic-protocols` stays out.
- FFmpeg is GPL; the packages ship its licence and a note saying which build and source it came from.

## Review Focus

1. **Odd dimensions.** An area 641×359 px must record; H.264, H.265 and VA-API reject odd sizes. The pure size function rounds down to even, and the recording is at most 1 px smaller on each side, never empty. Test in Task 1.
2. **Pause, resume, pause, stop.** A three-segment recording must play as one file with no gap or repeat. The timer must count recorded time only, and Esc after a pause must leave no file and no temp folder. Tests in Tasks 4 and 5.
3. **ffmpeg dies mid-recording** (a hardware encoder fails at frame 200, or the disk is full). The bar closes, the segments recorded so far are still joined and saved, and a notice says what happened. Windup must not hang with a bar counting forever. Test in Task 5.
4. **A config from 0.2.x** without a `video` key must load every other setting intact. A saved encoder that the probe no longer finds (a GPU removed) must fall back to the automatic encoder, not fail at record time. Tests in Tasks 1 and 2.
5. **Areas crossing monitors with different scales** on Wayland must record at the highest scale, with gaps black, the same as stills. A monitor unplugged mid-recording stops the recording cleanly and saves what was recorded. Test in Task 8.

---

## File Structure

`windup-core/src/video/` (new, pure, tested)
- `mod.rs`: re-exports.
- `format.rs`: `Container`, `Codec`, `Encoder`, validity, `auto_bitrate`, `frame_rates`, `even_size`, extensions.
- `probe.rs`: probe candidates, parsing the probe's results, the cache key, `pick_encoder`.
- `command.rs`: ffmpeg argv for a segment, for joining, and for the GIF pass.
- `recorder.rs`: the recording state machine (segments, timer, pause, stop, cancel).
- `pacing.rs`: a fixed-rate pacer for piped frames (repeat or drop).
- `compose.rs`: per-frame crop and stitch from monitor frames into the recorded rectangle.
- `place.rs`: where the recording bar goes, if anywhere.

Changes in `windup-core`:
- `config.rs` gains `VideoConfig`.
- `tool.rs` gains `ToolId::Video`.
- `hotkeys.rs` gains `Action::Video` and the recording-only actions.
- `ipc.rs` gains the record commands.
- `capture/backend.rs` gains `video_order`.
- `capture/session.rs` gains `Stage::Ready` and video outcomes.

`windup-ui`:
- `capture/bar.rs`: the video options row on the launcher.
- `capture/recbar.rs` (new): the recording bar.
- `settings/video_cfg.rs` (new): the Video settings pane.

`windup-desktop/src/video/` (new):
- `ffmpeg.rs`: locating ffmpeg, the probe, and running processes.
- `run.rs`: the recorder runtime (segments, temp folder, join).
- `windows.rs`: ddagrab and gdigrab settings, and audio through cpal.
- `pipe.rs`: frame and audio pipes, using FIFOs on Linux and named pipes on Windows.
- `linux/stream.rs`: screencopy streaming.
- `linux/screencast.rs`: the ScreenCast portal and PipeWire.
- `linux/audio.rs`: PipeWire audio.
- `linux/wayland/recbar.rs`: the recording bar as a layer surface.

Packaging:
- `packaging/ffmpeg.lock`, `tools/fetch-ffmpeg.sh`.
- Edits to the Windows and Linux `package.sh`, `windup.wxs`, `release.yml`, `ci.yml` and `docs/packaging.md`.

---

### Task 1: Formats, config, tool and hotkey entries

**Files:**
- Create: `crates/windup-core/src/video/{mod.rs,format.rs}`
- Modify:
  - `crates/windup-core/src/lib.rs`
  - `config.rs`, `tool.rs`, `hotkeys.rs`, `shortcut.rs`, `capture/output.rs`
  - `crates/windup-ui/src/settings/mod.rs`: a placeholder pane arm so the match is exhaustive. Task 15 fills it in.

**Interfaces:**
- Produces:
  - `enum Container { Mp4, WebM, Mov, Mkv, Gif }` with `ext() -> &'static str` and `ALL`.
  - `enum Codec { H264, H265, Av1, Vp9 }`.
  - `fn codecs_for(Container) -> &'static [Codec]`.
  - `fn audio_for(Container) -> Option<AudioCodec>`, where `enum AudioCodec { Aac, Opus }`.
  - `enum Bitrate { Auto, Kbps(u32) }`.
  - `fn auto_bitrate(w: u32, h: u32, fps: u32, codec: Codec) -> u32`, in kb/s.
  - `fn frame_rates(max_refresh_hz: Option<u32>) -> Vec<u32>`.
  - `fn even_size(w: u32, h: u32) -> Option<(u32, u32)>`.
  - `const FIXED_BITRATES: [u32; 6] = [2000, 4000, 8000, 12000, 20000, 40000]` and `const GIF_COLOURS: [u16; 4] = [256, 128, 64, 32]`.
  - `struct VideoConfig` with these fields:
    - `enabled: bool`
    - `shortcut: Shortcut`, default Super+Shift+R
    - `pause_shortcut: Shortcut`, default Super+Shift+P
    - `save_dir: Option<PathBuf>`
    - `name_pattern: String`, default `"Recording {date} {time}"`
    - `container: Container`, default Mp4
    - `codec: Codec`, default H264
    - `encoder: Option<String>`, where None means automatic
    - `fps: u32`, default 30
    - `bitrate: Bitrate`, default Auto
    - `gif_colours: u16`, default 256
    - `system_audio: bool` and `mic: bool`, both false
    - `system_device: Option<String>` and `mic_device: Option<String>`
    - `last_target: TargetKind`
    - `screencast_token: Option<String>`
  - `Config.video: VideoConfig` with `#[serde(default)]`.
  - `ToolId::Video`: `as_str` gives `"video"`, `display_name` gives `"Video"`.
  - `hotkeys::Action::{Video, RecordPause, RecordStop, RecordCancel}`. `bindings(config)` gives only `Video`; `recording_bindings(config) -> Vec<(Shortcut, Action)>` gives Pause (from `pause_shortcut`), Stop (from the Video shortcut) and Cancel (Esc).
  - `output::file_name_ext(pattern, when, ext: &str) -> String`. `file_name` calls it.
  - `output::default_video_dir() -> Option<PathBuf>` gives `<Videos>/Windup`.

- [ ] **Step 1: Write the failing tests** in `video/format.rs`, `config.rs` and `hotkeys.rs`:

```rust
#[test] fn mp4_takes_h264_h265_av1_and_aac() { assert_eq!(codecs_for(Container::Mp4), &[Codec::H264, Codec::H265, Codec::Av1]); assert_eq!(audio_for(Container::Mp4), Some(AudioCodec::Aac)); }
#[test] fn webm_takes_vp9_av1_and_opus() { assert_eq!(codecs_for(Container::WebM), &[Codec::Vp9, Codec::Av1]); assert_eq!(audio_for(Container::WebM), Some(AudioCodec::Opus)); }
#[test] fn gif_takes_no_codec_and_no_audio() { assert!(codecs_for(Container::Gif).is_empty()); assert_eq!(audio_for(Container::Gif), None); }
#[test] fn auto_bitrate_follows_pixels_rate_and_codec() {
    assert_eq!(auto_bitrate(1920, 1080, 30, Codec::H264), 8000);
    assert_eq!(auto_bitrate(1920, 1080, 60, Codec::H264), 12000);
    assert_eq!(auto_bitrate(1920, 1080, 30, Codec::H265), 4800);
    assert_eq!(auto_bitrate(3840, 2160, 30, Codec::H264), 32000);
    assert_eq!(auto_bitrate(1920, 1080, 15, Codec::H264), 4000);
    assert_eq!(auto_bitrate(2, 2, 15, Codec::Av1), 1000); // floor 1 Mb/s
}
#[test] fn frame_rates_add_a_faster_monitor() { assert_eq!(frame_rates(Some(144)), vec![15, 24, 30, 60, 144]); assert_eq!(frame_rates(Some(60)), vec![15, 24, 30, 60]); assert_eq!(frame_rates(None), vec![15, 24, 30, 60]); }
#[test] fn odd_sizes_round_down_to_even_and_tiny_ones_are_none() { assert_eq!(even_size(641, 359), Some((640, 358))); assert_eq!(even_size(1, 500), None); assert_eq!(even_size(2, 2), Some((2, 2))); }
// config.rs
#[test] fn a_config_from_0_2_without_video_keeps_everything_else() { /* parse a 0.2.2 config.json literal with a custom clipboard limit; assert the limit survives and video == VideoConfig::default() */ }
// hotkeys.rs
#[test] fn recording_keys_are_pause_stop_and_esc() { let b = recording_bindings(&Config::default()); assert!(b.contains(&(Shortcut::new(SUPER | SHIFT, "P"), Action::RecordPause))); assert!(b.contains(&(Shortcut::new(SUPER | SHIFT, "R"), Action::RecordStop))); assert!(b.contains(&(Shortcut::new(NONE, "ESCAPE"), Action::RecordCancel))); }
```

Also extend the existing `defaults_match_their_actions`, portal and COSMIC default tests so that Video gives `"SUPER+SHIFT+r"` and `"Super+Shift+R"`.

- [ ] **Step 2:** Run `cargo test -p windup-core`. Expected: the new tests fail to compile.
- [ ] **Step 3: Implement.**
  - `auto_bitrate` is `8000 × pixels / (1920×1080)`, multiplied by a rate factor: `fps/30` below 30 fps, `1 + 0.5 × (fps − 30)/30` above it.
  - Multiply by `0.6` for every codec except H.264.
  - Round to the nearest 100 and clamp to 1000..=100000.
  - Follow CLAUDE.md "Adding a tool" for every touch point except the settings pane body.
- [ ] **Step 4:** Run `cargo test --workspace` and clippy, plus the Linux cross-clippy. Expected: PASS.
- [ ] **Step 5:** Commit: "Video: formats, config, tool and hotkeys".

### Task 2: Finding ffmpeg and probing encoders

**Files:**
- Create: `crates/windup-core/src/video/probe.rs`, `crates/windup-desktop/src/video/{mod.rs,ffmpeg.rs}`
- Modify: `crates/windup-desktop/src/main.rs` (the hidden `--encoders-test` flag)

**Interfaces:**
- Produces, in core:
  - `fn candidates(codec: Codec, linux: bool) -> &'static [Encoder]`, where `struct Encoder { name: &'static str, hardware: bool }`. The order is NVENC, AMF, QSV, VA-API (Linux only), then software.
  - `struct Probe { ffmpeg_version: String, working: Vec<String>, audio: Vec<AudioCodec> }`, with serde.
  - `fn cache_key(path: &Path, len: u64, mtime_secs: u64) -> String`.
  - `fn pick_encoder(codec: Codec, wanted: Option<&str>, probe: &Probe) -> Option<&'static str>`. It returns `wanted` if the probe found it working, else the first working candidate, else `None`.
  - `fn probe_args(enc: &Encoder) -> Vec<String>`: a one-frame encode of `color=black:s=256x256:d=0.04` to `-f null -`. VA-API adds `-vaapi_device /dev/dri/renderD128 -vf format=nv12,hwupload`.
- Produces, in desktop:
  - `fn locate() -> Option<PathBuf>`, which tries these in order:
    1. `WINDUP_FFMPEG`
    2. next to the exe (`ffmpeg.exe`, `ffmpeg`)
    3. `<exe>/../lib/windup/ffmpeg`
    4. `PATH`, only in debug builds
  - `fn probe_cached() -> Option<Probe>`. It runs the probes in parallel (4 at a time, 5 s timeout each) and caches them in `<config_dir>/encoders.json`, keyed by `cache_key`.
  - `static PROBE: OnceLock<ProbeState>`, where `enum ProbeState { Running, Done(Probe), NoFfmpeg }`. A background thread fills it at startup.
  - `windup --encoders-test` prints one line per candidate, `name ok|fails`, and exits 0 if any H.264 encoder works.

- [ ] **Step 1: Write the failing tests** in `probe.rs`:
  - `pick_encoder(H264, Some("h264_nvenc"), &probe_with(&["libx264"])) == Some("libx264")`. A saved encoder that is gone falls back.
  - `pick_encoder(H264, None, &probe_with(&["h264_qsv", "libx264"])) == Some("h264_qsv")`. Hardware comes first.
  - `pick_encoder(Av1, None, &probe_with(&["libx264"])) == None`.
  - `candidates(H264, false)` does not contain `h264_vaapi`, and `candidates(H264, true)` does.
  - `cache_key` changes when `len` or `mtime` changes.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - The probe passes if ffmpeg exits 0. Its stderr goes to `log::debug`.
  - Audio is probed the same way with `aac` and `libopus` on `anullsrc`.
- [ ] **Step 4:** Run `cargo test --workspace`. If an ffmpeg is installed, also run `WINDUP_FFMPEG=<path> cargo run -- --encoders-test`. Expected: PASS, and libx264 is listed as `ok`.
- [ ] **Step 5:** Commit: "Video: locate ffmpeg and probe its encoders".

### Task 3: Bundling FFmpeg in every package

**Files:**
- Create:
  - `packaging/ffmpeg.lock`: lines of `<platform> <url> <sha256>` for `win64`, `winarm64` and `linux64`, plus a `tag` line.
  - `tools/fetch-ffmpeg.sh <platform> <dest-dir>`: downloads, checks the SHA-256, extracts `ffmpeg[.exe]` and `LICENSE.txt` into `dest-dir`, and writes `FFMPEG-SOURCE.txt` with the tag and the BtbN and FFmpeg source URLs.
- Modify:
  - `.github/workflows/release.yml`: fetch into the bin dir before Package.
  - `.github/workflows/ci.yml`: the e2e job fetches linux64, and the Windows test job fetches win64 and runs `--encoders-test`.
  - `packaging/windows/package.sh` and `packaging/windows/windup.wxs`: new components `Ffmpeg` and `FfmpegLicence` with new GUIDs, pinned by a test in `windup_core::install` next to the existing ones.
  - `packaging/linux/package.sh`: `/usr/lib/windup/ffmpeg` and `/usr/share/licenses/windup/ffmpeg/` in the deb, rpm and Arch packages, and `usr/lib/windup/ffmpeg` in the AppImage.
  - `docs/packaging.md`.

- [ ] **Step 1: Pick the tag.** Choose the newest BtbN `autobuild-*` release whose assets include `ffmpeg-n8.0*-{win64,winarm64,linux64}-gpl-8.0` (n7.1 if 8.0 is missing; ddagrab needs ≥ 6.1). Write the URLs and SHA-256 sums into `ffmpeg.lock`.
- [ ] **Step 2:** Run `bash tools/fetch-ffmpeg.sh win64 target/ff` locally. Expected: `target/ff/ffmpeg.exe` runs `-version`. A wrong SHA in a copy of the lock must exit non-zero with "checksum mismatch".
- [ ] **Step 3:** Wire up the workflows and package scripts.
  - Keep platform logic in the scripts, not in YAML (docs/packaging.md).
  - The WiX XML comments must not contain `--`.
- [ ] **Step 4:** Run `cargo test -p windup-core install` and `sh packaging/check.sh`. Push the branch to see CI only when the owner approves; otherwise run `package.sh` locally for Windows. Expected: the MSI contains `ffmpeg.exe`.
- [ ] **Step 5:** Commit: "Packaging: bundle a pinned FFmpeg".

### Task 4: Command lines and the recording state machine

**Files:**
- Create: `crates/windup-core/src/video/{command.rs,recorder.rs}`

**Interfaces:**
- Consumes: Task 1's formats and Task 2's `pick_encoder`.
- Produces:
  - `enum Source`:
    - `Ddagrab { output: u32, x: i32, y: i32, w: u32, h: u32 }`
    - `Gdigrab { x: i32, y: i32, w: u32, h: u32 }`
    - `X11grab { display: String, x: i32, y: i32, w: u32, h: u32 }`
    - `RawPipe { path: String, w: u32, h: u32 }`, which is BGRA at the recording's fps
  - `struct AudioIn { path: String, rate: u32, channels: u16 }`. The input is f32le from a pipe.
  - `struct SegmentJob { source: Source, fps: u32, encoder: &'static str, bitrate_kbps: Option<u32>, audio: Vec<AudioIn>, out: PathBuf }`. `bitrate_kbps` is `None` for GIF.
  - `fn segment_args(job: &SegmentJob, container: Container) -> Vec<String>`.
  - `fn join_args(list_file: &Path, out: &Path, container: Container, gif_colours: u16, fps: u32) -> Vec<String>`.
  - `fn segment_ext(container: Container) -> &'static str`.
  - `struct Recorder`, which is pure:
    - `fn start(now: f64)`, `fn pause(now)`, `fn resume(now)`, `fn stop(now) -> Finish`, `fn cancel() -> Finish`, `fn segment_failed(now)`
    - `fn elapsed(now) -> f64`
    - `fn state() -> RecState`, where `enum RecState { Recording, Paused, Done }`
    - `fn segments() -> &[u32]`
    - `fn next_segment() -> u32`
  - `enum Finish { Join(Vec<u32>), Discard }`.

- [ ] **Step 1: Write the failing tests:**
  - `segment_args` for `Ddagrab` contains `-filter_complex` with `ddagrab=output_idx=0:framerate=30:offset_x=…:video_size=640x358`.
    - With `h264_nvenc` it has no `hwdownload`.
    - With `libx264` it contains `hwdownload,format=bgra`.
  - For `RawPipe` it contains `-f rawvideo -pix_fmt bgra -s 640x358 -r 30 -i <path>`.
  - Two audio inputs give `amix=inputs=2:normalize=0`.
  - A fixed bitrate gives `-b:v 8000k -maxrate 8000k -bufsize 16000k`.
  - `segment_ext(Mp4) == "mkv"` and `segment_ext(Gif) == "mkv"`. GIF segments use `-c:v ffv1`.
  - `join_args(…, Mp4, …)` contains `-f concat -safe 0` and `-c copy -movflags +faststart`. With Gif it contains `palettegen=max_colors=128` and `paletteuse`, and no `-c copy`.
  - `recorder_counts_only_recorded_time`: start(0), pause(2), resume(5), stop(8) gives `elapsed == 5` and `Finish::Join(vec![0, 1])`.
  - `cancel_after_pause_discards`.
  - `a_failed_segment_ends_the_recording_and_keeps_what_was_recorded`: `segment_failed(3)` gives state `Done`, and the segments recorded so far are kept for the join.
  - `pause_while_paused_does_nothing`.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - Every segment is an MKV of the final codec, so the join can be `-c copy`.
  - Video inputs get `-thread_queue_size 512`.
  - Pipe inputs get `-use_wallclock_as_timestamps 1` on audio only.
  - Output gets `-pix_fmt yuv420p` for software H.264 and H.265.
- [ ] **Step 4:** Run `cargo test -p windup-core video`. Expected: PASS.
- [ ] **Step 5:** Commit: "Video: ffmpeg command lines and the recorder".

### Task 5: The recorder runtime, and `--record-test`

**Files:**
- Create: `crates/windup-desktop/src/video/{run.rs,pipe.rs}`
- Modify: `crates/windup-desktop/src/main.rs`

**Interfaces:**
- Consumes: Task 4's `Recorder`, `segment_args` and `join_args`, and Task 2's `locate`.
- Produces:
  - `trait FrameSource: Send { fn size(&self) -> (u32, u32); fn next_frame(&mut self, buf: &mut Vec<u8>) -> Result<bool, String>; }`. `Ok(false)` means "no new frame yet".
  - `trait SourceFactory: Send { fn source(&self) -> Source; fn open_pipe(&self) -> Option<Box<dyn FrameSource>>; }`.
  - `struct Recording`:
    - `fn start(job: RecordJob) -> Result<Recording, String>`
    - `fn pause()`, `fn resume()`, `fn stop() -> JoinHandle<Result<PathBuf, String>>`, `fn cancel()`
    - `fn elapsed() -> f64`
    - `fn failed() -> Option<String>`
  - `struct RecordJob { factory: Box<dyn SourceFactory>, audio: Vec<Box<dyn AudioSource>>, cfg: VideoConfig, size: (u32, u32), out_dir: PathBuf }`.
  - `trait AudioSource: Send { fn rate(&self) -> u32; fn channels(&self) -> u16; fn read(&mut self, buf: &mut Vec<f32>) -> Result<(), String>; }`.
  - Hidden flag: `windup --record-test <out> --seconds N [--area x,y,w,h] [--container mp4]`. It records with the source Task 7 picks for this desktop and exits 0 when the file exists and is not empty.

- [ ] **Step 1: Write the failing tests** (unit, with a fake `FrameSource` making 64×64 colour frames and the real ffmpeg found by `locate()`; the tests skip with a log line when there is none):
  - `a_piped_recording_with_a_pause_is_one_file`: record, pause, resume and stop. The output exists, and `ffmpeg -i out` shows `64x64`.
  - `cancel_leaves_no_file_and_no_temp_folder`.
  - `ffmpeg_dying_keeps_what_was_recorded`: use an encoder name that does not exist for the second segment. `failed()` is `Some`, and stop still returns a file of segment 0.
- [ ] **Step 2:** Run `cargo test -p windup-desktop video`. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - Segments go in `<cache_dir>/recording-<pid>-<n>/`, which is removed after the join or the cancel.
  - Self-capturing sources are stopped by writing `q` to ffmpeg's stdin. Pipe sources are stopped by closing the pipe, then waiting for ffmpeg to exit, with a 5 s kill timeout.
  - A pacer thread (Task 6) writes frames for pipe sources.
  - Audio threads write f32le to their pipes.
  - Pipes: `mkfifo` on Linux; on Windows, `CreateNamedPipeW` `\\.\pipe\windup-<pid>-<n>-{video,audio0,audio1}` with a no-op twin elsewhere.
- [ ] **Step 4:** Run the tests. Expected: PASS, with CI's linux64 ffmpeg.
- [ ] **Step 5:** Commit: "Video: recorder runtime and --record-test".

### Task 6: Pacing piped frames

**Files:**
- Create: `crates/windup-core/src/video/pacing.rs`
- Modify: `crates/windup-desktop/src/video/run.rs`

**Interfaces:**
- Produces: `struct Pacer { fps: u32, start: f64, sent: u64 }`, with `fn due(&mut self, now: f64) -> u64`. It returns how many frames should be written now (0, 1, or more to catch up, capped at `fps / 2`); the last frame is repeated when no new one came.

- [ ] **Step 1: Write the failing tests:**
  - At 30 fps, calling `due` every 1/30 s gives 1 each time.
  - A 200 ms stall gives 6, then back to 1.
  - A 10 s stall gives at most 15, and it does not try to catch up for ever.
  - `due` before `start` gives 0.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement it, and use it in the pacer thread from Task 5.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit: "Video: frame pacing".

### Task 7: Choosing the frame source; Windows and X11

**Files:**
- Modify: `crates/windup-core/src/capture/backend.rs`
- Create: `crates/windup-desktop/src/video/windows.rs`, `crates/windup-desktop/src/video/x11.rs`
- Modify: refresh rates for monitors in `capture/windows.rs` and `capture/x11.rs`. They are added to `MonitorShot` as `refresh_hz: Option<u32>`, with a serde default where it is serialised.

**Interfaces:**
- Produces:
  - `enum Moving { Ddagrab, Gdigrab, X11grab, ExtImageCopy, WlrScreencopy, ScreenCast, None }`.
  - `fn video_order(f: &Facts) -> Vec<Moving>`. The orders are:
    - Windows: `[Ddagrab, Gdigrab]`
    - X11: `[X11grab]`
    - COSMIC: `[ExtImageCopy, WlrScreencopy, ScreenCast]`
    - wlroots: `[WlrScreencopy, ExtImageCopy, ScreenCast]`
    - KDE and GNOME: `[ScreenCast]`
    - Weston without the portal: `[None]`
  - `fn record_bar_host(f: &Facts) -> BarHost`, where `enum BarHost { ExcludedWindow, Outside, Hidden }`. GNOME and the window host give `Hidden`.
  - Windows `SourceFactory`:
    - A rectangle inside one monitor gives `Ddagrab` with that output's index.
    - A rectangle across monitors gives `Gdigrab` over the virtual desktop, in physical pixels.
    - If ddagrab's ffmpeg exits within 1 s of starting, `Recording::start` retries once with `Gdigrab`, logged.
  - X11 `SourceFactory`: `X11grab` with `DISPLAY` and the rectangle.

- [ ] **Step 1: Write the failing tests** in `backend.rs`. One per row of the order list above, and one per `BarHost` case.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement. Get refresh rates from `EnumDisplaySettingsW` on Windows and the RandR mode on X11.
- [ ] **Step 4:** Run `cargo test --workspace`. Then run `cargo run -- --record-test %TEMP%\t.mp4 --seconds 2` on Windows. Expected: the file plays. This writes to the owner's temp folder only and does not open an overlay.
- [ ] **Step 5:** Commit: "Video: frame sources on Windows and X11".

### Task 8: Wayland streaming (COSMIC, wlroots) and per-frame stitching

**Files:**
- Create:
  - `crates/windup-core/src/video/compose.rs`
  - `crates/windup-desktop/src/linux/video/{mod.rs,stream.rs}` (a new `linux/video/` folder)
- Modify: `crates/windup-desktop/src/linux/wayland/screencopy.rs`. Share the buffer and format code; do not copy it.

**Interfaces:**
- Consumes: `capture::geometry::{DRect, MonitorShot}` and `capture::pixels::{to_rgba, untransform}`.
- Produces:
  - `struct Composer`:
    - `fn new(rect: DRect, monitors: &[(DRect, f32, (u32, u32))]) -> Composer`. Each monitor is given as its layout, its scale and its pixel size.
    - `fn size() -> (u32, u32)`, which is even.
    - `fn blit(&self, monitor: usize, frame: &[u8], stride: usize, out: &mut [u8])`, BGRA, nearest neighbour.
    - The mapping is precomputed once per recording, and gaps are black.
  - `struct ScreencopyStream`, which implements `FrameSource`. It holds one Wayland connection per recording. With ext-image-copy-capture it keeps one session per crossed output and asks for frames continuously. With wlr it calls `capture_output_region` per frame. An output that goes away gives `Err("A monitor was disconnected.")`.

- [ ] **Step 1: Write the failing tests** in `compose.rs`:
  - A rectangle on one scale-1 monitor copies pixels 1:1.
  - A rectangle across a scale-2 and a scale-1 monitor gives an output at scale 2, where each scale-1 pixel becomes 2×2.
  - A rectangle over a gap between monitors leaves those pixels black and opaque.
  - A monitor at negative coordinates lands at the right offset.
  - `size()` of a rectangle 641×359 at scale 1 is 640×358.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement the stream.
- [ ] **Step 4:** Run the tests, then add an e2e case to `tools/linux-e2e.sh` on headless Sway: `windup --record-test /tmp/r.mp4 --seconds 2 --area 0,0,320,240`. `ffprobe` (which ships with the BtbN archive; `fetch-ffmpeg.sh` keeps it for CI only) must show 320x240 and a duration of 1.5 to 2.5 s. Run it on CI. Expected: PASS.
- [ ] **Step 5:** Commit: "Video: Wayland screencopy streams".

### Task 9: ScreenCast portal and PipeWire (KDE, GNOME)

**Files:**
- Create: `crates/windup-desktop/src/linux/video/screencast.rs`
- Modify:
  - `crates/windup-desktop/Cargo.toml`: `pipewire = "0.8"` (Linux only).
  - `ci.yml` and `release.yml`: the Linux deps `libpipewire-0.3-dev libclang-dev`.
  - `docs/linux.md`.

**Interfaces:**
- Produces: `fn open_screencast(token: Option<String>) -> Result<(ScreenCastStream, Option<String>), CaptureError>`.
  - It asks for `SourceType::Monitor` with `multiple = true`, `persist_mode = ExplicitlyRevoked` and `cursor_mode = Embedded`.
  - It returns the new restore token, which the caller saves to `VideoConfig.screencast_token` through `change_config`.
  - `ScreenCastStream` implements `FrameSource`. It offers only SHM formats (BGRx, BGRA, RGBx, RGBA, with no DMA-BUF modifiers), so frames arrive in memory, and it feeds the Task 8 `Composer` with the portal's stream positions as the layout.
  - A refused dialog gives `CaptureError::Denied`.

- [ ] **Step 1: Write the failing tests.** Make the pure part a function: `fn layout_from_streams(streams: &[(u32, Option<(i32, i32)>, Option<(i32, i32)>)]) -> Vec<DRect>`. With every position missing (GNOME before 46), it lays the streams out left to right. Test that case, and one with real positions.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement. The PipeWire main loop runs on its own thread, and frames cross to `next_frame` through a one-slot mailbox that holds the newest frame.
- [ ] **Step 4:** Run `cargo test` and the Linux cross-clippy. No CI desktop has the portal; note it in the memory file's owner checklist for KDE and GNOME. Expected: PASS.
- [ ] **Step 5:** Commit: "Video: ScreenCast portal and PipeWire".

### Task 10: Audio

**Files:**
- Create: `crates/windup-desktop/src/video/audio.rs`, `crates/windup-desktop/src/linux/video/audio.rs`
- Modify: `crates/windup-desktop/Cargo.toml` (`cpal`, Windows only)

**Interfaces:**
- Produces:
  - `fn audio_devices() -> AudioDevices`, where `struct AudioDevices { outputs: Vec<String>, inputs: Vec<String>, available: bool }`.
  - `fn open_system(device: Option<&str>) -> Result<Box<dyn AudioSource>, String>` and `fn open_mic(device: Option<&str>) -> Result<Box<dyn AudioSource>, String>`.
  - Windows: cpal WASAPI. `open_system` builds an input stream on the output device, which is loopback.
  - Linux: a PipeWire stream. For `open_system` it is a capture of the default sink's monitor (`stream.capture.sink = true`); for `open_mic` it is the default source.
  - `available` is false when PipeWire is not running. The audio toggles are then disabled with the tooltip "Sound needs PipeWire."

- [ ] **Step 1: Write the failing test.** Put the pure f32 conversion in core, `video::pcm::to_f32le(samples: &[i16]) -> Vec<u8>`, for devices that deliver i16, and test it at −32768, 0 and 32767.
- [ ] **Step 2:** Run the test. Expected: FAIL.
- [ ] **Step 3:** Implement. Silence is still written while nothing plays, so the audio stays aligned with the video.
- [ ] **Step 4:** Run `cargo test` and the clippy targets. On Windows, record 3 s with computer sound on (`--record-test … --system-audio`) while a sound plays. The file must have an audio stream. Expected: PASS.
- [ ] **Step 5:** Commit: "Video: computer sound and microphone".

### Task 11: Video in the session and the launcher bar

**Files:**
- Modify:
  - `crates/windup-core/src/capture/session.rs`
  - `crates/windup-ui/src/capture/{bar.rs,mod.rs,view_tests.rs}`
  - `crates/windup-ui/src/icons.rs`: Lucide `circle` for record, `pause`, `square` for stop, `volume-2` and `mic`, transcribed verbatim.

**Interfaces:**
- Consumes: Task 1's `VideoConfig` and `frame_rates`, and Task 2's `ProbeState`.
- Produces:
  - `Stage::Ready`: Video's equivalent of Editing. The selection stays adjustable and there are no annotation tools.
  - `struct VideoChoice { fps: u32, bitrate: Bitrate, gif_colours: u16, system_audio: bool, mic: bool }` in `Session.video`.
  - `Action::{Record, SetFps(u32), SetBitrate(Bitrate), SetGifColours(u16), ToggleSystemAudio, ToggleMic}`.
  - `Outcome::Record { rect: DRect, choice: VideoChoice }`.
  - `Session::new` takes `availability: ModeAvailability { video: Result<(), &'static str>, audio: Result<(), &'static str> }`. The texts are exactly these:
    - "Video needs FFmpeg, which is missing from this installation."
    - "No video encoder works on this computer."
    - "This desktop does not let applications record the screen."
    - "Windup is still checking which encoders work."
    - "Sound needs PipeWire."
- Copy:
  - The record button says "Record".
  - Enter in Ready starts recording.
  - The fps dropdown labels read `30 fps`, the bitrate ones `Auto` or `8 Mb/s`, and the GIF ones `256 colours`.

- [ ] **Step 1: Write the failing tests** in `session.rs`:
  - `video_mode_goes_to_ready_not_editing`.
  - `enter_in_ready_records_the_selection_with_the_choices`.
  - `video_mode_is_refused_with_its_reason_when_unavailable`: `SetMode(Video)` leaves Screenshot when `availability.video` is Err.
  - `a_window_target_in_video_records_its_rectangle_at_start`.
  - `audio_toggles_do_nothing_when_audio_is_unavailable`.

  And in `view_tests.rs`: `the_disabled_video_button_shows_its_reason` (the tooltip text is the reason).
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement.
  - The bar layout follows option C. In Ready, the options row replaces the tool row: the fps dropdown, the bitrate (or colours) dropdown, the speaker toggle, the mic toggle, then an accent "Record" button.
  - Remove the `LATER` text for Video only; Steps keeps it.
  - The desktop side saves the remembered choices to `VideoConfig` on `Outcome::Record`, next to `finish::remember`.
- [ ] **Step 4:** Run `cargo test --workspace`, and `cargo run -p windup-ui --example capture-view` to check the bar drawing headless (the example writes nothing to the screen). Expected: PASS.
- [ ] **Step 5:** Commit: "Video: Ready stage and options on the launcher bar".

### Task 12: The recording bar

**Files:**
- Create:
  - `crates/windup-core/src/video/place.rs`
  - `crates/windup-ui/src/capture/recbar.rs`
- Modify: `crates/windup-ui/src/theme.rs`: tokens `REC_BAR_H`, `REC_DOT`, `REC_PULSE_SECS = 1.6`, `REC_GAP = 8.0`.

**Interfaces:**
- Produces:
  - `fn place_bar(rect: DRect, monitors: &[DRect], bar: (f32, f32), gap: f32) -> Option<DRect>`. It tries the following in order, and every candidate must lie wholly on one monitor and not intersect `rect`:
    1. Centred above `rect` on the same monitor.
    2. Centred below it.
    3. Top centre of another monitor.
    4. `None`.
  - `struct RecBarView`, with `fn show(ui: &mut Ui, p: &Palette, state: &RecBarState) -> Option<RecBarAction>`.
    - `RecBarState { elapsed: f64, paused: bool, fps: u32, bitrate_label: String, reduced_motion: bool }`.
    - `enum RecBarAction { Pause, Resume, Stop }`.
    - Space and Enter are handled only when the bar's viewport has focus.
- Timer format: `m:ss`, or `h:mm:ss` from one hour, in the mono font token.

- [ ] **Step 1: Write the failing tests** in `place.rs`:
  - An area in the middle of a monitor gives the slot above it.
  - An area touching the top gives the slot below.
  - Full screen of the left monitor with a right monitor gives the right monitor's top centre.
  - Full screen of the only monitor gives `None`.
  - An area across two monitors gives a slot on one monitor only.

  In `recbar.rs`: `timer_text(59.9) == "0:59"` and `timer_text(3600.0) == "1:00:00"`. And `reduced_motion_keeps_the_dot_still`: the dot's alpha is the same at t=0 and at t=0.8.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement. The pulse is `0.55 + 0.45 × cos(2π t / REC_PULSE_SECS)` on alpha. A comment cites the spec's §13 exception.
- [ ] **Step 4:** Run `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5:** Commit: "Video: the recording bar and where it goes".

### Task 13: Hosting the recording, end to end

**Files:**
- Modify:
  - `crates/windup-desktop/src/main.rs` and `overlay.rs` (Windows and X11)
  - `crates/windup-desktop/src/linux/{capture.rs,mod.rs}`
  - `crates/windup-desktop/src/capture/gate.rs`
- Create: `crates/windup-desktop/src/linux/wayland/recbar.rs`

**Interfaces:**
- Consumes: `Outcome::Record` (Task 11), `Recording` (Task 5), `video_order` and `record_bar_host` (Task 7), the stream factories (Tasks 7 to 9), audio (Task 10), and `place_bar` and `RecBarView` (Task 12).
- Produces:
  - `Gate::Recording`, a fourth state. A capture hotkey is ignored while it is set.
  - On `Outcome::Record` the overlay closes first, and recording starts after one presented frame, so the overlay is never in the first frame.
  - The bar is hosted as follows:
    - Windows: a borderless topmost immediate viewport where the launcher was, with `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)` set before it is shown. On a failure (Windows before 2004) it falls back to `place_bar`.
    - X11: a viewport at `place_bar`.
    - Layer host: a layer surface on the `Overlay` layer at `place_bar`'s output and margins, with keyboard interactivity `OnDemand`.
    - Window host: no bar.
  - On stop, the join runs on a worker thread. Then the notice shows "Saved to <file name>" with an "Open folder" button. It opens the folder with `explorer /select,` on Windows and `xdg-open <dir>` on Linux.
  - On a failure, the notice shows the error. If segments were joined, it shows "Saved what was recorded before the error to <file name>." instead.

- [ ] **Step 1: Write the failing tests** in `gate.rs`: `a_capture_hotkey_is_ignored_while_recording`, and `ending_a_recording_reopens_the_gate`.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - The notice gains an optional action button on all three hosts. Extend `NoticeSetup` and the Windows/X11 notice; do not add a second notice type.
- [ ] **Step 4:** Run `cargo test --workspace` and all three clippy targets. Extend the e2e on Sway: drive `windup --record` through IPC, wait 2 s, run `windup --record-stop`, then expect a new file in `$XDG_VIDEOS_DIR/Windup` and a notice. The script presses no keys on the owner's screens. Expected: PASS on CI.
- [ ] **Step 5:** Commit: "Video: record from the launcher, with the recording bar".

### Task 14: Recording keys (pause, stop, Esc) on every platform

**Files:**
- Modify:
  - `crates/windup-desktop/src/hotkey.rs` (the Windows hook)
  - `crates/windup-desktop/src/linux/{x11.rs,ipc.rs,portal.rs,cosmic_shortcuts.rs}`
  - `crates/windup-core/src/ipc.rs`
  - `crates/windup-desktop/src/main.rs` (the flags)

**Interfaces:**
- Consumes: `hotkeys::recording_bindings` (Task 1).
- Produces:
  - `ipc::Command::{Record, RecordPause, RecordStop, RecordCancel}` with the flags `--record`, `--record-pause`, `--record-stop` and `--record-cancel`. `Command::Record` while recording means stop, matching the hotkey.
  - Windows: the hook takes a `recording: Arc<AtomicBool>`. While it is set, the recording bindings are matched and swallowed, Esc included.
  - X11: passive grabs for the recording bindings from start to end.
  - COSMIC: `cosmic_shortcuts::add_temporary(&[(Shortcut, &str command)])` and `remove_temporary()`. The entries are tagged so they can be found again, and they are removed at startup if a crash left them behind.
  - Portal (KDE, GNOME, Hyprland): a second GlobalShortcuts session, opened at record start with ids `record-pause`, `record-stop` and `record-cancel`, and closed at the end.
  - Sway, and GNOME without GlobalShortcuts: no global Esc. The Video settings pane shows the caution line "On this desktop Esc only cancels from the recording bar; bind `windup --record-cancel` in your desktop's settings."

- [ ] **Step 1: Write the failing tests:**
  - core `ipc`: the new commands round-trip through their wire format.
  - `cosmic_shortcuts`: `add_temporary` then `remove_temporary` leaves the file byte-identical, and a file with stale tagged entries is cleaned by `remove_temporary`.
  - `hotkey.rs`: `esc_is_matched_only_while_recording`, using the pure matcher the hook already calls.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test --workspace` and the clippy targets. Add an e2e case on X11 (Xvfb): `xdotool key super+shift+r` starts a recording and `Escape` leaves no file. Expected: PASS on CI.
- [ ] **Step 5:** Commit: "Video: pause, stop and Esc while recording".

### Task 15: Settings pane, docs and changelog

**Files:**
- Create: `crates/windup-ui/src/settings/video_cfg.rs`
- Modify:
  - `crates/windup-ui/src/settings/mod.rs`
  - `README.md`, `docs/linux.md`, `CHANGELOG.md` (`## Unreleased`), `CLAUDE.md` (the "What this is" and "Capture" sections)

**Interfaces:**
- Consumes: the `ProbeState` and `AudioDevices` snapshots, passed into the settings view as plain data; the UI never calls into desktop.
- Pane rows, in this order:
  1. Shortcut, and Pause shortcut.
  2. Save folder and File name.
  3. Container, then Codec, which offers only `codecs_for(container)` that have a working encoder.
  4. Encoder: "Automatic" plus the working ones for that codec.
  5. Frame rate and Bitrate, or Colours for GIF.
  6. Computer sound with its device, and Microphone with its device.
  7. The caution line from Task 14 where it applies.

  - Changing the container to one that does not take the current codec moves the codec to the first valid one.
  - Rows that cannot work are disabled, and each says why.

- [ ] **Step 1: Write the failing tests:** `changing_container_keeps_the_codec_valid` (MP4 with H.265, then WebM, gives VP9 or AV1), and `codec_list_shows_only_working_ones`, using a probe snapshot with only libx264, which gives H.264 alone.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement the pane with the existing painted row widgets.
- [ ] **Step 4:** Run `cargo test --workspace`, `cargo fmt --check`, all three clippy targets, and `sh packaging/check.sh`. Expected: PASS.
- [ ] **Step 5:** Commit: "Video: settings pane and docs".
