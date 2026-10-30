# Capture, phase 3 (Steps) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Record a series of steps (one still of the pointer's monitor per click or per step key), review them with captions, edit any one in the capture editor, and export them as one self-contained HTML file, a PDF, Markdown with an images folder, or numbered images. It must work the same way on Windows, X11 and every Wayland host.

**Architecture:**
- **Core (pure and tested).** Every decision that can be pure lives in `windup-core::steps`:
  - the session document (`steps.json`)
  - the click filter, the trigger choice and the click-setup route
  - where the steps bar goes and when it must be hidden for a shot
  - the markers, drawn with tiny-skia from stored pointer positions
  - the four exports, including a small PDF writer of our own
- **Markers are drawn at review and export time, never at capture.** Removing a step renumbers the badges, and changing the marker style changes every step. Edits made in the editor are burned into the step's PNG.
- **Desktop.** A desktop `StepsLive` runs a session:
  - A camera thread takes the shots. Each platform has one `Camera`, chosen by the pure `steps_order`.
  - A writer thread encodes the PNGs and keeps `steps.json` up to date.
  - The click watchers (`WH_MOUSE_LL`, XInput2 raw events, evdev) feed the same trigger channel the step key uses.
- **Keys.** Esc, pause, stop and step reuse video's "live keys" mechanisms (the Windows hook, X11 grabs, COSMIC temporary entries, the second GlobalShortcuts portal session). They are generalised from "recording" to a `LiveKind` of Video or Steps.
- **The review.** On Windows and X11 it is an in-process eframe viewport. On Wayland it is a child process, `windup --steps-review <dir>`, which asks the host over the socket to open a step in the editor.

**Tech Stack:**
- Existing: Rust 2024; egui/eframe 0.32 and egui-wgpu; windows-sys 0.61; x11rb 0.13; smithay-client-toolkit 0.20; wayland-protocols 0.32; ashpd 0.11; pipewire 0.8 (feature `pipewire`); tiny-skia 0.11; image 0.25 (png, jpeg); jiff.
- New:
  - `evdev` 0.13 (Apache-2.0 OR MIT), Linux only, for reading clicks on Wayland. It is pure Rust, so it builds on Ubuntu 22.04 and cross-clippies from Windows. If it ever fails to, read `input_event` through `rustix` directly.
  - The x11rb feature `xinput`.
  - No PDF or base64 crate: both are written in core.

**Spec:** `docs/superpowers/specs/2026-10-28-capture-design.md`, sections "Steps", "Hotkeys", "Escape", "Capture back ends", "Settings", "Phases" and "Constraints". Read them first; this plan does not repeat their reasons. Where this plan and the spec differ, this plan wins (see "Decisions" in each constraint).

**Builds on phase 1 and 2 (shipped as v0.3.1).** It does not touch these, which a parallel branch is changing and which will be merged before Task 1:
- `hotkeys::clashes`
- `video/probe.rs`
- the ScreenCast recorder clock
- `tools/fetch-ffmpeg.sh`

## Global Constraints

Owner decisions:
- **What takes a step.** The **step hotkey** works on every platform and can be rebound. **Mouse clicks** count too, wherever Windup can see them. A setting picks one of:
  - Clicks and hotkey (the default where clicks work)
  - Hotkey only
  - Clicks only
- **Clicks, per platform:**
  - Windows: `WH_MOUSE_LL`.
  - X11: XInput2 raw button events.
  - Wayland: evdev, through either of two routes:
    - **(A)** An optional, one-time setting, "Capture clicks too", explained in the pane. It runs `pkexec usermod -aG input $USER` and says plainly that this group can also read the keyboard, and that it takes effect after logging out and in.
    - **(B)** The deb, rpm and Arch packages ship a udev rule that gives the logged-in user (`TAG+="uaccess"`) the mouse and touchpad event nodes only.
  - The AppImage can only use A. The settings detect whether the nodes are readable and say which route is active.
- **Markers are configurable:** a ring, a numbered badge or an arrow, with a colour and a size.
- **Stop opens a review list.** Each step is numbered, can be removed, can be opened in the editor, and has an optional caption the user types.
- **Export** to one self-contained HTML file with the images embedded (the default), PDF, Markdown with an images folder, or the raw numbered images.
- **Esc always cancels and cannot be rebound.** During Steps it discards everything and leaves no files.
- **The steps bar** shows a step count instead of a timer, with pause and stop. It is **never in a step's picture.**
- **The same experience on every desktop.** A one-time portal grant on GNOME and KDE is acceptable. Every keybind can be changed.
- **UI rules:** follow `../Design-Principles/STYLE-GUIDE.md`. Painted, tokens only, Lucide only, British spelling, Sentence case, no em dashes in UI copy.

Controller decisions:
- **Each step is a still of the pointer's monitor**, taken at the click press or the step key, plus the pointer position in that image's pixels when known.
  - Markers are drawn at review and export time from that stored position.
  - Edits made in the editor are burned into the step's PNG.
- **Clicks on Windup's own bar are not steps.**
- **Session storage** is `<cache dir>/steps/<YYYY-MM-DD HHMMSS>/`, holding `NN.png` files and a `steps.json` (a pure serde model in core).
  - Cancel deletes the folder.
  - Export writes elsewhere, and the folder is removed after a successful export.
  - File numbers are never reused within a session.
- **Shot and pointer per platform:**

  | Platform | Shot | Pointer |
  |---|---|---|
  | Windows | GDI still of one monitor | `GetCursorPos`, or the hook's point for a click |
  | X11 | `GetImage` of one monitor | `QueryPointer` |
  | COSMIC and wlroots with `ext-image-copy-capture` | Copy of the output under the pointer | A cursor session per output |
  | Hyprland | Screencopy | IPC `cursorpos` |
  | Sway, where no position is available | The focused output, with the cursor painted in | None, so no marker; the Steps pane says so |
  | KDE, installed package | KWin ScreenShot2 | A KWin script |
  | KDE AppImage, and GNOME | The ScreenCast portal stream, kept open for the session with the restore token shared with video | The stream's cursor metadata |

- **Launcher:**
  - Choosing Steps on the bar needs no selection. The bar shows the trigger and marker choices and a **Start** button, and Enter starts.
  - The Steps tool hotkey starts at once, without the launcher.
  - Stop is the Steps hotkey again, or the bar's stop button.
- **Default keys:**
  - Steps is **Super+Shift+G**.
  - Take a step is **Super+Shift+X**.
  - Pause shares video's pause key (Super+Shift+P).
- **Review hosting:**
  - Windows and X11: an in-process eframe viewport.
  - Wayland: a child process, `windup --steps-review <dir>`, which asks the host over IPC to open a step in the editor.
  - The editor on a step is the capture session opened on that one image as if it were the frozen monitor, with the whole image selected:
    - Enter or Ctrl+S writes the edits back to the step's file.
    - Esc leaves the file unchanged.
    - The review reloads the thumbnail afterwards.
- **PDF:** a minimal writer in core, with no new dependency.
  - One step per page.
  - JPEG images with `DCTDecode`.
  - Captions in Helvetica with WinAnsi encoding and a width table for wrapping. Characters outside WinAnsi become `?`.
- **HTML:** base64 PNG inline, styles inline, no scripts.
- **Markdown:** `steps.md` plus `images/NN.png`.
- **Keys during Steps** use the same temporary-binding mechanisms as video recording. Evdev is mouse only and never watches keys.
- **Release:** v0.4.0. The last task writes the docs and `## Unreleased`; it does not bump the version.

Builder decisions in this plan:
- **Keeping the bar out of a shot, on non-Windows:**
  - The bar goes on a monitor **other than the pointer's** when there is one: the top centre of the first other monitor (`steps_bar_slot`).
  - Whenever a shot's monitor holds the bar (one monitor, or the pointer moved onto the bar's monitor), the bar is **concealed for the instant of the shot** (`must_conceal`):
    - X11: the camera thread moves the bar window off-screen on its own X connection, syncs, shoots and moves it back. This never waits on the UI thread.
    - Layer host: the Wayland thread unmaps the bar's layer surface and acknowledges after the compositor's next frame callback.
  - Windows uses `WDA_EXCLUDEFROMCAPTURE` and never conceals, unless exclusion failed.
  - GNOME and Weston show no bar, as for video.
  - Why both: concealing alone works everywhere but flickers on every step. On a second monitor nothing flickers, which is the common desktop setup. The conceal path is what keeps the rule on one-monitor laptops.
- **The steps bar** is the recording bar with the count in place of the dot, the timer and the chips:
  - A static `footprints` icon in `accent`, then `3 steps` in mono, then pause and stop.
  - No pulsing dot: nothing records between steps.
- **Which clicks count:**
  - Left, right and middle **presses**. The wheel, wheel tilt and side buttons never count.
  - A second press within 0.4 s, and within 4 points when both positions are known, is one step (a double-click).
  - On evdev a touchpad tap counts: one finger, at most 0.18 s, at most 3 mm of travel.
  - Clicks and the step key are ignored while paused.
- **Clicks only without click capture** falls back to the step key, and the pane says why.
- **Failed shots.** A failed shot is skipped and logged. Three failures in a row end the session, which opens the review with the steps taken and the reason.
- **One review at a time.** Starting Steps while a review is open gives the notice "Finish the steps you are reviewing first."
- **Closing the review** without exporting asks "Discard these steps? They have not been exported." with **Keep** and **Discard**.
- **Leftover session folders** (a crash) are removed at startup, like video segments.
- **Export destination:**
  - `<Documents>/Windup/`, named `Steps YYYY-MM-DD HHMMSS.html` or `.pdf`.
  - Markdown and Images make a folder of that name.
  - The pattern is in settings.
- **PDF layout:**
  - A4 (595 × 842 pt) with 48 pt margins.
  - A heading `Step N` in Helvetica at 14 pt.
  - The image fitted to 499 pt wide and at most 560 pt high.
  - The caption in Helvetica at 11 pt with 14 pt leading, under the image.
  - A caption that does not fit continues on a text-only page.
- **Default marker:** Badge, at Medium size, in the screenshot editor's default colour.
  - Sizes are Small 24 pt, Medium 32 pt and Large 44 pt, scaled by the step's monitor scale.
  - Every shape has a contrasting halo.
  - Badge digits are a stroke font in core, so the review and the exports look identical.
- **GNOME and KDE without KWin** get no screenshot-portal fallback for Steps. It flashes and plays a shutter sound on every call.
- **The camera is opened, and one shot taken and thrown away, at session start**, so a missing permission fails before the user starts clicking.
- **Commands:**
  - `--record-pause`, `--record-stop` and `--record-cancel` act on whichever live session runs.
  - `--steps-stop` stops only a Steps session.
- **The udev rule** is `70-windup-pointer.rules`, which must sort before `73-seat-late`. It leaves out any node that is also a keyboard.
- **Hidden flags,** not in `USAGE`:
  - `--steps-export <dir> <html|pdf|md|images> <out>`
  - `--step-shot-test <out.png>`
  - `--steps-review <dir>`, internal

Rules from phases 1 and 2 still hold:
- Every Win32 block is `cfg(windows)` with a no-op twin. Linux code lives under `windup-desktop/src/linux/`. macOS still compiles.
- CI stays warning-free on Windows x64, Linux and ARM64.
- `cosmic-protocols` stays out.

**The checks** each task runs before committing:
1. `cargo fmt --check`
2. `cargo test --workspace`
3. `RUSTFLAGS=-D warnings cargo clippy --workspace --all-targets`
4. The Linux cross-clippy from CLAUDE.md: `PKG_CONFIG=<the .cmd that echoes -lxkbcommon> PKG_CONFIG_ALLOW_CROSS=1 cargo clippy --workspace --all-targets --target x86_64-unknown-linux-gnu --no-default-features`

Linux tests and `pipewire` code run on CI or WSL.

## Review Focus

1. **Esc during Steps leaves nothing.** Esc while a shot is still being encoded and written must leave no session folder and no PNG; a write that finishes after the cancel must not recreate the folder.
   - Test: `cancel_while_a_shot_is_being_written_leaves_no_folder` in Task 9.
   - On X11 it is checked end to end (Task 14).
2. **Removing a step renumbers; files are never reused.** Remove step 2 of 3: the export says Steps 1 and 2, the badge on the old step 3 reads "2", and the next shot is `04.png`, even after the review reloads `steps.json`.
   - Tests: `files_are_never_reused_even_after_a_reload` (Task 2) and `export_numbers_follow_the_list_not_the_files` (Task 5).
3. **Clicks on Windup's own bar are not steps,** a double-click is one step, and the wheel is not a click.
   - Tests: `a_click_on_the_bar_is_not_a_step`, `a_double_click_is_one_step` and `the_wheel_and_side_buttons_are_not_clicks` (Task 3).
4. **The PDF is valid.** Every xref offset points at its `N 0 obj`, there is one page per step plus continuation pages, `€` becomes WinAnsi 0x80, `漢` becomes `?`, and parentheses and backslashes are escaped.
   - Tests: Task 6.
5. **The bar is never in a step's picture off Windows:**
   - With two monitors the bar sits on the one the pointer is not on, and no shot of the other conceals it.
   - With one monitor every shot conceals it, and it is hidden before the shot and shown after.
   - Tests: `must_conceal_*` (Task 3) and `a_concealed_bar_is_hidden_for_the_shot_and_shown_after` (Task 9).

---

## File Structure

`windup-core/src/steps/` (new; pure and tested):
- `mod.rs`: re-exports.
- `options.rs`: `StepTrigger`, `MarkerKind`, `MarkerSize`, `MarkerStyle`, `ExportFormat`.
- `model.rs`: `StepsDoc`, `Step`, `StepCause`, `Button`, session folder names, load and save.
- `clicks.rs`: `ClickFilter`, `TapDetector`, button mapping, `takes`, `ClickRoute`, `click_route`, `listed_in_group`, `pointer_kind`.
- `place.rs`: `steps_bar_slot`, `must_conceal`.
- `pointer.rs`: `cursor_point`, `to_image`.
- `avail.rs`: whether Steps and markers work here, and why not.
- `marker.rs`, `glyphs.rs`, `render.rs`: marker geometry, stroke digits, burning and thumbnails.
- `export/`:
  - `mod.rs`: `Item`, `export`, `export_name`
  - `base64.rs`, `html.rs`, `markdown.rs`
  - `pdf.rs`, `winansi.rs`

Changes in `windup-core`:
- `config.rs`: `StepsConfig`.
- `tool.rs`: `ToolId::Steps`.
- `hotkeys.rs`: `Action::{Steps, Step}`, `LiveKind`, `session_bindings`.
- `shortcut.rs`: the new default keys.
- `ipc.rs`: the commands, the launches and `Request`.
- `capture/backend.rs`: `StepRoute`, `steps_order`.
- `capture/session.rs`: the Steps launcher and `edit_image`.
- `capture/output.rs`: `default_steps_dir`.
- `video/screencast.rs`: `cursor_in_stream`.

`windup-ui`:
- `capture/bar.rs`: the Steps options row, and the step-edit tool bar.
- `capture/recbar.rs`: the step count.
- `capture/mod.rs`: Enter on Steps.
- `icons.rs`: Lucide `hash`, `mouse-pointer-click`, `mouse`, `file-code` and `images`.
- `steps/` (new): `review.rs` (`ReviewModel`, `ReviewView`) and `paint.rs` (marker prims to egui).
- `settings/steps_cfg.rs` (new): the Steps pane.

`windup-desktop`:
- `steps/` (new):
  - `live.rs`: `StepsLive`
  - `camera.rs`: `Camera`, `Conceal`, `open_camera`
  - `clicks.rs`: `ClickWatch`, Windows and X11
  - `windows.rs`: GDI camera
  - `x11.rs`: X11 camera and conceal
  - `review.rs`: `ReviewHost`, which the viewport and the child share
  - `review_app.rs`: the Wayland child
- `review_host.rs` (new): the review viewport on Windows and X11.
- Linux, under `linux/steps/` (new):
  - `screencopy.rs`, with `CursorTracker`
  - `kwin.rs`
  - `screencast.rs`
  - `evdev.rs`
  - `setup.rs`: `pkexec`
- Modified:
  - `hotkey.rs`, `main.rs`, `overlay.rs`, `recording.rs`
  - `capture/{windows.rs,x11.rs,ipcrects.rs,kwin_rects.rs,finish.rs}`
  - `linux/{mod.rs,capture.rs,ipc.rs,kde.rs,portal.rs}`
  - `linux/video/{screencast.rs,pw_stream.rs}`
  - `linux/wayland/{screencopy.rs,recbar.rs,mod.rs}`

Packaging and tests:
- `packaging/linux/70-windup-pointer.rules` (new).
- Changes to `packaging/linux/{package.sh,windup.install,check-enrolment.sh}`.
- New cases in `tools/linux-e2e.sh`: `sway_steps`, `x11_steps`, and a `--step-shot-test` case on each.

---

### Task 1: Steps tool, config, keys and commands

**Files:**
- Create: `crates/windup-core/src/steps/{mod.rs,options.rs}`
- Modify:
  - In `crates/windup-core/src/`: `lib.rs`, `config.rs`, `tool.rs`, `shortcut.rs`, `hotkeys.rs`, `ipc.rs`, and the `cosmic_shortcuts.rs` tests.
  - `crates/windup-ui/src/settings/mod.rs`: a `SettingsPane::Steps` placeholder arm, so the match is exhaustive. Task 16 fills it in.
  - `crates/windup-desktop/src/hotkey.rs`: `HotkeyEvent::{Steps{target}, Step}` and `from_action`.
  - `crates/windup-desktop/src/main.rs`: arms that log `steps: not yet` (Task 14 replaces them).
  - `crates/windup-desktop/src/linux/mod.rs`: nothing yet; its `_ => {}` arm covers the new commands.

**Interfaces:**
- Produces, in `windup_core::steps::options`, each with serde and `Default`:
  - `enum StepTrigger { ClicksAndHotkey, HotkeyOnly, ClicksOnly }`, with `ALL` and `label()`. The labels are "Clicks and hotkey", "Hotkey only" and "Clicks only".
  - `enum MarkerKind { Ring, Badge, Arrow }` (default `Badge`), with `ALL` and `label()`: "Ring", "Numbered badge", "Arrow".
  - `enum MarkerSize { Small, Medium, Large }` (default `Medium`), with `points() -> f32` giving 24, 32 or 44, and `label()`.
  - `struct MarkerStyle { kind: MarkerKind, colour: [u8; 4], size: MarkerSize }`. The default colour is `config::DEFAULT_COLOUR`, made `pub(crate)`.
  - `enum ExportFormat { Html, Pdf, Markdown, Images }` (default `Html`):
    - `ALL`
    - `label()`: "Web page (HTML)", "PDF", "Markdown", "Images"
    - `arg() -> &'static str`: `html`, `pdf`, `md` or `images`
    - `from_arg(&str) -> Option<Self>`
- Produces, in config: `struct StepsConfig` (serde default), and `Config.steps: StepsConfig` with `#[serde(default)]`. Its fields:
  - `enabled: bool`, true
  - `shortcut: Shortcut`, `Shortcut::super_shift_g()`
  - `step_shortcut: Shortcut`, `Shortcut::super_shift_x()`
  - `trigger: StepTrigger`
  - `marker: MarkerStyle`
  - `export: ExportFormat`
  - `export_dir: Option<PathBuf>`
  - `name_pattern: String`, `"Steps {date} {time}"`
- Produces, in `tool.rs`: `ToolId::Steps`:
  - `as_str` gives `"steps"` and `display_name` gives `"Steps"`.
  - `description` is "Records the steps you take, a picture for each click or key press, and exports them as a document."
  - Its default shortcut is Super+Shift+G.
  - It is added to `ALL`, which becomes `[ToolId; 6]`, and to the `Config` accessors.
- Produces, in `hotkeys.rs`:
  - `Action::{Steps, Step}`. `Action::tool()` maps both to `ToolId::Steps`.
  - `enum LiveKind { Video, Steps }`.
  - `fn steps_bindings(config: &Config) -> Vec<(Shortcut, Action)>` gives, in order:
    1. `(steps.step_shortcut, Step)`
    2. `(video.pause_shortcut, RecordPause)`
    3. `(steps.shortcut, RecordStop)`
    4. `(Esc, RecordCancel)`
  - `fn session_bindings(config: &Config, kind: LiveKind) -> Vec<(Shortcut, Action)>`. Video is `recording_bindings`.
  - `fn live_bindings_for(config: &Config, live: Option<LiveKind>) -> Vec<(Shortcut, Action)>`. `live_bindings(config, recording)` stays and calls it with `recording.then_some(LiveKind::Video)`.
  - `fn session_keys_untaken(config: &Config, kind: LiveKind) -> Vec<(Shortcut, Action)>`. `recording_keys_untaken` calls it with Video.
- Produces, in `ipc.rs`:
  - `Command::{Steps, Step, StepsStop}`, on the wire as `steps`, `step` and `steps-stop`, with the flags `--steps`, `--step` and `--steps-stop`. `ALL` becomes `[Command; 13]`.
  - `for_action(Steps) = Steps` and `for_action(Step) = Step`.
  - `USAGE` gains three lines.

- [ ] **Step 1: Write the failing tests:**
```rust
// tool.rs: the existing test, with the length changed to 6, plus:
#[test] fn the_step_and_pause_keys_collide_with_no_tool_default() {
    let c = Config::default();
    for s in [&c.steps.step_shortcut, &c.video.pause_shortcut] {
        assert!(ToolId::ALL.iter().all(|t| t.default_shortcut().as_ref() != Some(s)), "{s}");
    }
    assert_ne!(c.steps.step_shortcut, c.video.pause_shortcut);
}
// hotkeys.rs
#[test] fn steps_keys_are_step_pause_stop_and_esc() {
    let b = session_bindings(&Config::default(), LiveKind::Steps);
    let s = Shortcut::new;
    assert_eq!(b, vec![
        (s(ShortcutModifiers::SUPER_SHIFT, "X"), Action::Step),
        (s(ShortcutModifiers::SUPER_SHIFT, "P"), Action::RecordPause),
        (s(ShortcutModifiers::SUPER_SHIFT, "G"), Action::RecordStop),
        (s(ShortcutModifiers::NONE, "ESCAPE"), Action::RecordCancel)]);
}
#[test] fn during_steps_the_steps_key_is_stop_once_and_the_rest_stay() {
    let c = Config::default();
    let live = live_bindings_for(&c, Some(LiveKind::Steps));
    assert_eq!(live.iter().filter(|(s, _)| *s == c.steps.shortcut).count(), 1);
    assert!(live.contains(&(c.steps.shortcut.clone(), Action::RecordStop)));
    assert!(live.contains(&(c.video.shortcut.clone(), Action::Video)));
    assert_eq!(live_bindings_for(&c, Some(LiveKind::Video)), live_bindings(&c, true));
}
#[test] fn the_step_key_a_tool_holds_is_left_to_it() { /* steps.step_shortcut = ctrl_shift_s → session_keys_untaken(Steps) has no Step */ }
// Rename defaults_bind_all_five to defaults_bind_all_six: len 6, clashes empty.
// Extend portal_triggers: t(ToolId::Steps) == "LOGO+SHIFT+g", and portal_trigger(&c.steps.step_shortcut) == "LOGO+SHIFT+x".
// config.rs
#[test] fn a_config_from_0_3_without_steps_keeps_everything_else() { /* to_value(default), remove "steps", set clipboard.max_items 37 → parses, 37 kept, steps == StepsConfig::default() */ }
#[test] fn steps_defaults_and_tool_accessors_cover_steps() { /* tool_shortcut(Steps) == super_shift_g; set_tool_enabled(Steps,false) flips steps.enabled */ }
// ipc.rs: extend every_command_round_trips (13); and
#[test] fn steps_flags_parse() { assert_eq!(parse_args(["--steps"]), Launch::Send(Command::Steps)); assert_eq!(parse_args(["--step"]), Launch::Send(Command::Step)); assert_eq!(parse_args(["--steps-stop"]), Launch::Send(Command::StepsStop)); assert_eq!(Command::for_action(Action::Step), Command::Step); }
// cosmic_shortcuts.rs
#[test] fn steps_is_offered_to_cosmic() { /* wanted(&default) has args "--steps", modifiers ["Super","Shift"], key "g", description "Steps"; no "--step" (session-only) */ }
```
- [ ] **Step 2:** Run `cargo test -p windup-core`. Expected: fails to compile.
- [ ] **Step 3: Implement.**
  - Follow CLAUDE.md "Adding a tool" for every touch point except the pane body and the hotkey behaviour.
  - Add `Shortcut::super_shift_g()` and `super_shift_x()`.
  - Do not edit `clashes`.
- [ ] **Step 4:** Run the checks. Expected: PASS.
- [ ] **Step 5:** Commit: "Steps: tool, config, keys and commands".

### Task 2: The session document and its folder

**Files:**
- Create: `crates/windup-core/src/steps/model.rs`
- Modify: `crates/windup-core/src/capture/output.rs` (`default_steps_dir`)

**Interfaces:**
- Produces:
  - `enum Button { Left, Right, Middle }` and `enum StepCause { Hotkey, Click(Button) }`, both with serde.
  - `struct Step { file: String, cause: StepCause, pointer: Option<[f32; 2]>, scale: f32, monitor: String, caption: String, at_ms: u64 }`:
    - `pointer` is in the image's pixels.
    - `scale` is image pixels per point, used to size the marker.
  - `struct StepsDoc { version: u32, started: String, next: u32, steps: Vec<Step> }`. `version` is 1. `next` is the next file number, so a number is never reused.
  - Methods on `StepsDoc`:
    - `new(started: &jiff::Zoned) -> StepsDoc`
    - `next_file(&mut self) -> String`, which gives `01.png`, … `99.png`, `100.png` and advances `next`
    - `push(&mut self, step: Step)`
    - `remove(&mut self, i: usize) -> Option<Step>`
    - `set_caption(&mut self, i: usize, text: &str)`
    - `load(dir: &Path) -> Result<StepsDoc, String>`
    - `save(&self, dir: &Path) -> std::io::Result<()>`, which writes `steps.json.tmp` and then renames it
  - `const DOC_FILE: &str = "steps.json"`.
  - `fn session_dir_name(when: &jiff::Zoned) -> String`, e.g. `"2026-10-30 142501"`.
  - `fn new_session_dir(root: &Path, when: &jiff::Zoned) -> std::io::Result<PathBuf>`. It creates the folder, adding ` (2)` and so on when the name exists.
  - `fn steps_root() -> Option<PathBuf>`, which is `Config::cache_dir()/steps`.
  - `output::default_steps_dir() -> Option<PathBuf>`, which is `<Documents>/Windup`.

- [ ] **Step 1: Write the failing tests:**
  - `files_are_never_reused_even_after_a_reload`. Push 3 steps, giving files `01.png`, `02.png` and `03.png`. Then `remove(1)`, then `save` and `load`. The remaining files are `["01.png", "03.png"]` and `next_file() == "04.png"`.
  - `the_hundredth_file_is_100_png`. With `next = 100`, `next_file() == "100.png"`.
  - `a_doc_round_trips_through_json`, including `StepCause::Click(Button::Right)` and `pointer: None`.
  - `session_dir_name_is_date_and_time`. `2026-10-30T14:25:01` gives `"2026-10-30 142501"`.
  - `a_second_folder_in_the_same_second_gets_a_number`. It ends with `" (2)"`.
  - `load_refuses_a_file_outside_the_folder`. A `file` of `"../x.png"`, `"/etc/passwd"` or `"a/b.png"` gives `Err`. The review child is handed a folder and must never read outside it.
  - `load_reports_a_broken_json`.
- [ ] **Step 2:** Run `cargo test -p windup-core steps::model`. Expected: FAIL.
- [ ] **Step 3: Implement.** A `file` must match `^[0-9]{2,}\.png$`.
- [ ] **Step 4:** Run the checks. Expected: PASS.
- [ ] **Step 5:** Commit: "Steps: the session document and its folder".

### Task 3: Pure decisions: clicks, triggers, the bar, the back end

**Files:**
- Create: `crates/windup-core/src/steps/{clicks.rs,place.rs,pointer.rs,avail.rs}`
- Modify: `crates/windup-core/src/capture/backend.rs`

**Interfaces:**
- Produces, in `clicks.rs`:
  - `struct Press { button: Button, at: Option<[f32; 2]>, t: f64 }`, where `at` is in desktop units when known.
  - `const DOUBLE_CLICK_SECS: f64 = 0.4` and `const DOUBLE_CLICK_POINTS: f32 = 4.0`.
  - `struct ClickFilter`, with `fn accept(&mut self, p: Press, units_per_point: f32, paused: bool, own: bool) -> bool`.
  - `fn is_own_click(at: Option<[f32; 2]>, bar: Option<DRect>, pointer_on_bar: bool) -> bool`.
  - `fn windows_button(msg: u32) -> Option<Button>`: `0x201` gives Left, `0x204` Right, `0x207` Middle; anything else gives `None`.
  - `fn x11_button(detail: u32) -> Option<Button>`: 1, 2 and 3; 4 to 9 give `None`.
  - `fn evdev_button(code: u16) -> Option<Button>`: `0x110`, `0x111` and `0x112`; `0x113` and `0x114` give `None`.
  - `struct TapDetector`, with `fn touch(&mut self, down: bool, fingers: u8, pos_mm: [f32; 2], t: f64) -> bool`. It says true once for a tap.
  - `struct Takes { clicks: bool, hotkey: bool }` and `fn takes(trigger: StepTrigger, clicks: Result<(), &'static str>) -> Takes`.
  - `enum ClickRoute { Native, Udev, InputGroup, AfterLogout, NotSetUp, NoPointer }`.
  - `struct ClickFacts { native: bool, pointer_nodes: usize, readable: usize, in_group_now: bool, listed_in_group: bool }`.
  - `fn click_route(f: &ClickFacts) -> ClickRoute`.
  - `fn clicks_reason(r: ClickRoute) -> Result<(), &'static str>`.
  - `fn listed_in_group(etc_group: &str, user: &str, group: &str) -> bool`.
  - `enum PointerKind { Mouse, Touchpad }` and `fn pointer_kind(keys: &[u16], rel: &[u16], abs: &[u16]) -> Option<PointerKind>`. Anything with `KEY_A` (30) is `None`.
- Produces, in `place.rs`:
  - `fn steps_bar_slot(host: BarHost, pointer_monitor: DRect, monitors: &[DRect], bar: (f32, f32), top: f32) -> Option<DRect>`:
    - `ExcludedWindow` and `Outside` give the top centre of the first monitor that is not `pointer_monitor`, else of `pointer_monitor`.
    - `Hidden` gives `None`.
  - `fn must_conceal(host: BarHost, excluded: bool, bar: Option<DRect>, shot: DRect) -> bool`.
- Produces, in `pointer.rs`:
  - `fn cursor_point(position: (i32, i32), hotspot: (i32, i32)) -> [f32; 2]`.
  - `fn to_image(global: (f32, f32), layout: DRect, image: (u32, u32)) -> Option<[f32; 2]>`. It gives `None` when the point is off that monitor.
- Produces, in `backend.rs`:
  - `enum WlPointer { CursorSession, Hyprland, SwayFocus, Unknown }`.
  - `enum StepRoute { Gdi, X11, Screencopy { ext: bool, pointer: WlPointer }, KWin, ScreenCast, None }`.
  - `fn steps_order(f: &Facts) -> Vec<StepRoute>`, which is never empty.
- Produces, in `avail.rs`:
  - `fn steps_available(order: &[StepRoute]) -> Result<(), &'static str>`. With no route it gives `NO_STEPS_SOURCE`, "This desktop does not let applications read the screen."
  - `fn markers_available(route: StepRoute) -> Result<(), &'static str>`:
    - Sway focus gives `NO_POINTER_ON_SWAY`: "Sway does not tell applications where the pointer is, so steps show the pointer itself and no marker."
    - `Unknown` gives `NO_POINTER_HERE`.

- [ ] **Step 1: Write the failing tests:**
```rust
#[test] fn a_click_on_the_bar_is_not_a_step() {
    let bar = Some(DRect { x: 100.0, y: 10.0, w: 200.0, h: 40.0 });
    assert!(is_own_click(Some([105.0, 15.0]), bar, false));
    assert!(!is_own_click(Some([50.0, 15.0]), bar, false));
    assert!(is_own_click(None, None, true)); // Wayland: the bar surface has the pointer
    assert!(!is_own_click(None, bar, false));
}
#[test] fn a_double_click_is_one_step() {
    let mut f = ClickFilter::default();
    let p = |t| Press { button: Button::Left, at: Some([10.0, 10.0]), t };
    assert!(f.accept(p(0.0), 1.0, false, false));
    assert!(!f.accept(p(0.3), 1.0, false, false));
    assert!(f.accept(p(0.9), 1.0, false, false));
    // Far apart is two steps even when quick.
    assert!(f.accept(Press { at: Some([200.0, 10.0]), ..p(1.0) }, 1.0, false, false));
    // Paused, or on the bar: never.
    assert!(!f.accept(p(5.0), 1.0, true, false));
    assert!(!f.accept(p(6.0), 1.0, false, true));
}
#[test] fn the_wheel_and_side_buttons_are_not_clicks() {
    assert_eq!(windows_button(0x20A), None); assert_eq!(windows_button(0x20B), None);
    assert_eq!(x11_button(4), None); assert_eq!(x11_button(7), None); assert_eq!(x11_button(3), Some(Button::Right));
    assert_eq!(evdev_button(0x113), None); assert_eq!(evdev_button(0x112), Some(Button::Middle));
}
#[test] fn a_short_still_one_finger_touch_is_a_tap() { /* down t=0, up t=0.12 travel 2mm → true; up at 0.3 → false; travel 5mm → false; fingers 2 → false */ }
#[test] fn clicks_only_without_click_capture_falls_back_to_the_key() {
    assert_eq!(takes(StepTrigger::ClicksOnly, Err("x")), Takes { clicks: false, hotkey: true });
    assert_eq!(takes(StepTrigger::ClicksOnly, Ok(())), Takes { clicks: true, hotkey: false });
    assert_eq!(takes(StepTrigger::ClicksAndHotkey, Err("x")), Takes { clicks: false, hotkey: true });
    assert_eq!(takes(StepTrigger::HotkeyOnly, Ok(())), Takes { clicks: false, hotkey: true });
}
#[test] fn the_click_route_says_which_way_clicks_come() {
    // native → Native; readable + in group now → InputGroup; readable, not in group → Udev;
    // unreadable, listed in /etc/group but not in this session → AfterLogout; unreadable → NotSetUp; 0 nodes → NoPointer
}
#[test] fn group_membership_is_read_from_etc_group() { assert!(listed_in_group("input:x:104:anna,bo\n", "bo", "input")); assert!(!listed_in_group("input:x:104:bob\n", "bo", "input")); }
#[test] fn keyboards_are_never_pointers() { assert_eq!(pointer_kind(&[0x110, 30], &[0, 1], &[]), None); assert_eq!(pointer_kind(&[0x110], &[0, 1], &[]), Some(PointerKind::Mouse)); assert_eq!(pointer_kind(&[0x14a, 0x145], &[], &[0, 1]), Some(PointerKind::Touchpad)); }
// place.rs
#[test] fn the_steps_bar_goes_to_a_monitor_the_pointer_is_not_on() { /* A(0,0,1920,1080) B(1920,0,1920,1080), pointer on A → slot.x within B, y == B.y + top */ }
#[test] fn must_conceal_on_one_monitor_for_every_shot() { /* one monitor: slot on A; must_conceal(Outside, false, slot, A) == true */ }
#[test] fn must_conceal_not_when_the_shot_is_of_the_other_monitor() { /* slot on B; shot A → false; shot B → true */ }
#[test] fn must_conceal_never_on_windows_with_exclusion_or_with_no_bar() { /* (ExcludedWindow, true, Some(on A), A) → false; (ExcludedWindow, false, ...) → true; (Hidden, false, None, A) → false */ }
// pointer.rs
#[test] fn the_cursor_point_is_the_image_corner_plus_the_hotspot() { assert_eq!(cursor_point((100, 50), (4, 2)), [104.0, 52.0]); }
#[test] fn a_global_point_lands_in_the_monitors_pixels() { /* layout (1920,0,1280,720), image 2560x1440, global (2560,360) → Some([1280.0, 720.0]); (100,100) → None */ }
// backend.rs, one per row
#[test] fn steps_order_per_desktop() {
    // windows → [Gdi]; x11 → [X11]
    // COSMIC (EXT) → [Screencopy{ext:true, pointer:CursorSession}]
    // Sway with EXT + wlr + swaysock → [Screencopy{true, CursorSession}, Screencopy{false, SwayFocus}]
    // Sway wlr only → [Screencopy{false, SwayFocus}]
    // Hyprland (wlr, hyprland) → [Screencopy{false, Hyprland}]
    // KDE (portal_screencast) → [KWin, ScreenCast]; GNOME (portal_screencast) → [ScreenCast]
    // Weston, no portal → [None]
}
```
- [ ] **Step 2:** Run `cargo test -p windup-core steps capture::backend`. Expected: FAIL.
- [ ] **Step 3: Implement.** The double-click distance is `DOUBLE_CLICK_POINTS × units_per_point` when both positions are known; with either unknown, time alone decides.
- [ ] **Step 4:** Run the checks. Expected: PASS.
- [ ] **Step 5:** Commit: "Steps: clicks, triggers, the bar's place, the back end".

### Task 4: Markers

**Files:**
- Create: `crates/windup-core/src/steps/{marker.rs,glyphs.rs,render.rs}`

**Interfaces:**
- Produces:
  - `enum Prim`, where every coordinate is in image pixels:
    - `Disc { c: [f32; 2], r: f32, colour: [u8; 4] }`
    - `Ring { c: [f32; 2], r: f32, width: f32, colour: [u8; 4] }`
    - `Polygon { points: Vec<[f32; 2]>, colour: [u8; 4] }`
    - `Stroke { points: Vec<[f32; 2]>, width: f32, colour: [u8; 4] }`
  - `fn marker(style: &MarkerStyle, at: [f32; 2], number: usize, scale: f32, image: (u32, u32)) -> Vec<Prim>`. The halo comes first, then the shape.
    - **Ring:** radius `size/2`, width `size/8`, centred on `at`.
    - **Badge:** a dot of radius `size/8` at `at`, plus a numbered disc of diameter `size`, centred `0.75 × size` up and to the right of `at`. It flips left or down so the whole disc stays inside the image.
    - **Arrow:** a polygon with its tip at `at`, pointing up and to the left, `size` long. It flips when it would leave the image.
  - `fn halo(colour: [u8; 4]) -> [u8; 4]`: black at 70 % on light colours, white at 85 % on dark ones (WCAG relative luminance above 0.5 counts as light).
  - `fn digits(n: usize, centre: [f32; 2], height: f32) -> Vec<Vec<[f32; 2]>>`: stroke polylines for each digit from a 0..1 grid, spaced at `0.6 × height`.
  - `fn burn(img: &mut RgbaImage, prims: &[Prim])`: tiny-skia, premultiplied, like `capture::raster`.
  - `fn render_step(img: &RgbaImage, step: &Step, number: usize, style: &MarkerStyle) -> RgbaImage`.
  - `fn thumbnail(img: &RgbaImage, step: &Step, number: usize, style: &MarkerStyle, max_w: u32) -> RgbaImage`. It scales the picture first, then burns the marker at the thumbnail's scale.

- [ ] **Step 1: Write the failing tests:**
  - `a_ring_is_centred_on_the_pointer`. Medium at scale 1, at (100, 100) on 400×300: a `Ring` with `c == [100.0, 100.0]` and `r == 16.0`.
  - `marker_size_follows_the_monitor_scale`. At scale 2, `r == 32.0`.
  - `the_badge_stays_inside_the_picture`. At (1910, 5) on 1920×1080, Medium: the badge disc's centre minus r and plus r lies within 0..1920 and 0..1080, and its centre x < 1910 and y > 5.
  - `the_arrow_tip_is_at_the_pointer`. The polygon's first point is `at`. At (1915, 1075) every point is ≤ 1920 and ≤ 1080.
  - `the_halo_contrasts`. `halo([255,255,255,255])[0] < 64` and `halo([120,20,20,255])[0] > 200`.
  - `digits_fit_the_badge`. Every point of `digits(128, [50,50], h)` with `h` the badge's text height is within the disc's radius of (50, 50).
  - `render_step_without_a_pointer_leaves_the_picture_alone`. The pixels are identical.
  - `render_step_burns_the_marker_colour`. A Ring in `[255,0,0,255]` on black 200×200 at (100, 100): pixel (116, 100) has r > 200.
  - `the_badge_number_is_the_one_given_not_the_file`. A step with `file: "03.png"` rendered with `number: 2` gives the same pixels as one with `file: "02.png"`.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the checks. Expected: PASS.
- [ ] **Step 5:** Commit: "Steps: markers drawn from the stored pointer".

### Task 5: Exports: HTML, Markdown, images

**Files:**
- Create: `crates/windup-core/src/steps/export/{mod.rs,base64.rs,html.rs,markdown.rs}`

**Interfaces:**
- Consumes: `StepsDoc::load` (Task 2) and `render_step` (Task 4).
- Produces:
  - `struct Item { number: usize, caption: String, png: Vec<u8> }`.
  - `fn base64(bytes: &[u8]) -> String`, standard alphabet with padding.
  - `fn escape_html(s: &str) -> String`. `& < > " '` become entities, and a newline becomes `<br>`.
  - `fn escape_markdown(s: &str) -> String`. A backslash goes before each of ``\ ` * _ [ ] < > #``.
  - `fn html(title: &str, items: &[Item]) -> String`. One `<figure>` per item: `<h2>Step N</h2>`, then `<img alt="Step N" src="data:image/png;base64,…">`, then the caption in `<figcaption>` when not empty. A `<style>` in the head, and no `<script>`.
  - `fn markdown(title: &str, items: &[Item]) -> String`: `# title`, then per item `## Step N`, `![Step N](images/NN.png)` and the caption.
  - `fn export(dir: &Path, format: ExportFormat, style: &MarkerStyle, dest: &Path) -> Result<PathBuf, String>`:
    - It loads `dir/steps.json`, renders each step with its number from its place in the list, and writes `dest`. `dest` is a file for Html and Pdf, and a folder (created) for Markdown and Images.
    - Images are renamed `01.png` onwards in list order.
    - A doc with no steps gives `Err(NOTHING_TO_EXPORT)`, "There are no steps to export."
    - Pdf gives `Err("…")` until Task 6.
  - `fn export_name(pattern: &str, when: jiff::Zoned, format: ExportFormat) -> String`, through `output::file_name_ext`. Markdown and Images give a folder name with no extension.

- [ ] **Step 1: Write the failing tests**, on a temp dir holding a 3-step doc with 64×48 PNGs:
  - `base64_vectors`. `""` gives `""`, `"f"` gives `"Zg=="`, `"fo"` gives `"Zm8="`, `"foo"` gives `"Zm9v"`, and `"foobar"` gives `"Zm9vYmFy"`.
  - `html_has_one_img_per_step_and_no_script`. With 3 steps, `<img` appears 3 times, the output contains `data:image/png;base64,iVBORw0KGgo`, and it has no `<script`.
  - `html_escapes_captions`. `<b>&` becomes `&lt;b&gt;&amp;`, and `a\nb` becomes `a<br>b`.
  - `export_numbers_follow_the_list_not_the_files`. Remove step 2 and export Html: it contains `Step 1` and `Step 2` and no `Step 3`. Exporting Images gives exactly `01.png` and `02.png`, and `02.png` decodes to the pixels of the old `03.png` with badge number 2.
  - `markdown_links_its_images_and_escapes_captions`. `steps.md` contains `![Step 2](images/02.png)`, `*not bold*` becomes `\*not bold\*`, and `images/01.png` exists.
  - `an_empty_doc_exports_nothing`.
  - `export_name_per_format`. `Html` gives `"Steps 2026-10-30 142501.html"`; `Images` gives `"Steps 2026-10-30 142501"`.
- [ ] **Step 2:** Run `cargo test -p windup-core steps::export`. Expected: FAIL.
- [ ] **Step 3:** Implement. Images are encoded with `image`'s PNG encoder at `CompressionType::Default`.
- [ ] **Step 4:** Run the checks. Expected: PASS.
- [ ] **Step 5:** Commit: "Steps: HTML, Markdown and image exports".

### Task 6: A minimal PDF writer

**Files:**
- Create: `crates/windup-core/src/steps/export/{pdf.rs,winansi.rs}`
- Modify: `crates/windup-core/src/steps/export/mod.rs`, wiring up `ExportFormat::Pdf`

**Interfaces:**
- Produces:
  - `fn winansi(s: &str) -> Vec<u8>`:
    - ASCII and Latin-1 (0xA0 to 0xFF) map to themselves.
    - The cp1252 specials map to 0x80 to 0x9F: `€ ‚ ƒ „ … † ‡ ˆ ‰ Š ‹ Œ Ž ‘ ’ “ ” • – — ˜ ™ š › œ ž Ÿ`.
    - A tab becomes a space.
    - Anything else becomes `?`.
  - `fn text_width(bytes: &[u8], size: f32) -> f32`, from Helvetica's AFM widths for WinAnsi 32 to 255 (a 224-entry table).
  - `fn wrap(text: &str, size: f32, width: f32) -> Vec<Vec<u8>>`. It breaks at spaces and newlines, and breaks a word longer than a line by characters.
  - `struct PdfImage { jpeg: Vec<u8>, w: u32, h: u32 }` and `fn jpeg_for_pdf(img: &RgbaImage) -> Result<PdfImage, String>`. Alpha goes onto white, at quality 90.
  - `fn pdf(title: &str, steps: &[(usize, String, PdfImage)]) -> Vec<u8>`:
    - PDF 1.4: catalog, pages, one Helvetica Type1 font with `/Encoding /WinAnsiEncoding`, and per page a page, a content stream and an image XObject with `/Filter /DCTDecode /ColorSpace /DeviceRGB /BitsPerComponent 8`.
    - `/Title` in an Info dictionary.
    - An xref table with 10-digit offsets, a trailer, `startxref` and `%%EOF`.
    - Strings are escaped: `(`, `)` and `\` get a backslash.

- [ ] **Step 1: Write the failing tests:**
```rust
#[test] fn pdf_starts_and_ends_right() { let b = pdf("T", &three()); assert!(b.starts_with(b"%PDF-1.4\n")); assert!(b.ends_with(b"%%EOF\n")); }
#[test] fn xref_offsets_point_at_their_objects() {
    let b = pdf("T", &three());
    // parse the table after "xref\n0 N\n"; entry n (n ≥ 1) at offset o: &b[o..] starts with format!("{n} 0 obj")
    // and startxref's number is the offset of "xref"
}
#[test] fn one_page_per_step() { let s = String::from_utf8_lossy(&pdf("T", &three())).into_owned(); assert!(s.contains("/Count 3")); assert_eq!(s.matches("/Filter /DCTDecode").count(), 3); assert!(s.contains("/Width 64 /Height 48")); }
#[test] fn winansi_maps_and_substitutes() { assert_eq!(winansi("café €5 — 漢"), b"caf\xe9 \x805 \x97 ?".to_vec()); }
#[test] fn helvetica_widths() { assert!((text_width(b"Hello", 10.0) - 22.78).abs() < 0.01); }
#[test] fn wrapped_lines_fit() { for l in wrap(&"word ".repeat(80), 11.0, 499.0) { assert!(text_width(&l, 11.0) <= 499.0); } assert_eq!(wrap("a\nb", 11.0, 499.0).len(), 2); }
#[test] fn parentheses_and_backslashes_are_escaped() { let s = String::from_utf8_lossy(&pdf("T", &[(1, r"a(b)\c".into(), img())])).into_owned(); assert!(s.contains(r"(a\(b\)\\c) Tj")); }
#[test] fn a_long_caption_continues_on_the_next_page() { let s = …pdf("T", &[(1, "word ".repeat(2000), img())]); assert!(s.contains("/Count 2") || s.contains("/Count 3")); /* and xref still valid */ }
#[test] fn export_writes_a_pdf_file() { /* export(dir, Pdf, …) → file starts with %PDF */ }
```
- [ ] **Step 2:** Run `cargo test -p windup-core steps::export::pdf`. Expected: FAIL.
- [ ] **Step 3:** Implement.
  - Take the width table from Adobe's Helvetica.afm (the standard 14 metrics are freely distributable). A comment says so.
  - Keep the content streams uncompressed. The images are JPEG already.
- [ ] **Step 4:** Run the checks, and open one exported PDF from a test temp dir in a viewer by hand. Expected: PASS, and the PDF opens.
- [ ] **Step 5:** Commit: "Steps: PDF export".

### Task 7: Steps on the launcher bar, and the editor on one picture

**Files:**
- Modify:
  - `crates/windup-core/src/capture/session.rs`
  - `crates/windup-ui/src/capture/{bar.rs,mod.rs,view_tests.rs}`
  - `crates/windup-ui/src/icons.rs`: Lucide `hash`, `mouse-pointer-click`, `mouse`, `file-code` and `images`, transcribed verbatim, added to `ALL` and checked on the icon sheet (§11)
  - `crates/windup-desktop/src/capture/finish.rs`: `wants` and `deliver` arms, plus `remember_steps`
  - `crates/windup-desktop/src/video/availability.rs`: fill in the new fields
  - `crates/windup-desktop/src/{overlay.rs,linux/capture.rs}`: map `Outcome::Steps` and `Outcome::StepDone` to a log line until Tasks 14 and 15

**Interfaces:**
- Produces, in core `session.rs`:
  - `struct StepsChoice { trigger: StepTrigger, marker: MarkerKind }`.
  - `Session.steps: StepsChoice` and `fn set_steps(&mut self, cfg: &StepsConfig)`.
  - `ModeAvailability` gains `steps: Result<(), &'static str>` and `clicks: Result<(), &'static str>`, and `ALL` gives `Ok` for both.
  - `Action::{StartSteps, SetStepTrigger(StepTrigger), SetMarker(MarkerKind)}`.
  - `Outcome::{Steps { choice: StepsChoice }, StepDone(RgbaImage)}`.
  - `enum Purpose { Capture, StepEdit }` and `Session.purpose`.
  - `fn Session::edit_image(image: RgbaImage, layout: DRect, cfg: &ScreenshotConfig) -> Session`:
    - one monitor with `layout`, `scale = image.width / layout.w`
    - `stage = Editing`, the selection equal to `layout`, `purpose = StepEdit`
  - Behaviour:
    - `SetMode(Steps)` is allowed while choosing when `availability.steps` is `Ok`.
    - In Steps mode, a press or drag makes no selection.
    - `StartSteps` in Steps mode gives `Outcome::Steps`.
    - `SetStepTrigger` to a clicks trigger is ignored while `clicks` is `Err`.
    - With `StepEdit`, both `Copy` and `Save` give `StepDone(render of the whole layout)`.
- Produces, in the ui bar:
  - `BarCommand::{StartSteps, StepTrigger(StepTrigger), Marker(MarkerKind)}`.
  - `BarState` gains `steps: StepsChoice`, `steps_mode: Result<(), &'static str>` (replacing `LATER` for Steps), `clicks: Result<(), &'static str>` and `purpose: Purpose`.
  - In Choosing with mode Steps, the items are:
    - the mode segment
    - a divider
    - a trigger dropdown, with icons `mouse-pointer-click`, `keyboard` and `mouse`
    - a marker dropdown, with icons `circle`, `hash` and `mouse-pointer-2`
    - a divider
    - Settings
    - the primary **Start** (`play`, key Enter)
    - Close
  - In Editing with `StepEdit`:
    - the tool segment, the swatch and Undo
    - the primary **Done** (`check`, key Enter, tooltip "Done (Ctrl+S too)")
    - Close
    - No target menu and no Save button.
  - Copy: "Start", "Done". A disabled trigger entry gives the `clicks` reason as its tooltip.
  - In `CaptureView`: Enter in Steps mode gives `Action::StartSteps`.

- [ ] **Step 1: Write the failing tests:**
  - core:
    - `steps_mode_needs_no_selection_and_start_ends_it`: `SetMode(Steps)`, then drag (10,10)→(200,200) leaves `selection.rect == None`, and `act(StartSteps)` gives `Outcome::Steps { choice }` with the config's trigger.
    - `steps_mode_is_refused_with_its_reason_when_unavailable`.
    - `a_clicks_trigger_is_ignored_without_click_capture`.
    - `a_step_edit_selects_the_whole_picture_and_ends_in_step_done`: an image of 1920×1080 at layout `(0,0,960,540)`. The stage is `Editing` and the selection equals the layout. Draw a rect, then `act(Copy)` gives `StepDone(img)` with `img.dimensions() == (1920, 1080)`; `act(Save)` gives the same, and `act(Cancel)` gives `Cancel`.
    - `start_steps_does_nothing_in_other_modes`.
  - ui `view_tests.rs`:
    - `the_steps_row_shows_start_not_a_target`: the Steps items contain a Primary labelled "Start" and no target menu.
    - `enter_in_steps_mode_starts`.
    - `the_step_edit_bar_has_done_and_no_save`.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement.
  - `remember_steps(cfg: &mut StepsConfig, session: &Session)` saves the trigger and marker kind picked on the bar, as `remember_video` does.
  - Remove `LATER` once nothing uses it.
- [ ] **Step 4:** Run the checks and `cargo run -p windup-ui --example capture-view`, which draws headless. Expected: PASS.
- [ ] **Step 5:** Commit: "Steps: the launcher's Steps row and the editor on one picture".

### Task 8: The step count bar and the review list

**Files:**
- Create: `crates/windup-ui/src/steps/{mod.rs,review.rs,paint.rs}`
- Modify:
  - `crates/windup-ui/src/capture/recbar.rs`
  - `crates/windup-ui/src/theme.rs`: tokens `REVIEW_THUMB_W = 240.0`, `REVIEW_W = 760.0`, `REVIEW_H = 820.0`
  - `crates/windup-ui/src/lib.rs`

**Interfaces:**
- Consumes: `StepsDoc` (Task 2), `marker` and `Prim` (Task 4), `ExportFormat` and `NOTHING_TO_EXPORT` (Task 5).
- Produces, in the recording bar:
  - `RecBarState.steps: Option<u32>`. When it is `Some(n)`, the bar draws the `footprints` icon in `accent` and `count_text(n)` in the mono timer font in place of the dot, the timer and the chips; pause and stop stay.
  - `fn count_text(n: u32) -> String`.
  - `bar_size` and `host_size` follow.
- Produces, in `steps/paint.rs`: `fn paint_prims(painter: &egui::Painter, prims: &[Prim], to_screen: impl Fn([f32; 2]) -> egui::Pos2, k: f32)`.
- Produces, in `steps/review.rs`:
  - `enum Closing { Open, Confirming }`.
  - `struct ReviewModel { doc: StepsDoc, style: MarkerStyle, format: ExportFormat, closing: Closing, error: Option<String>, busy: bool }`.
  - `enum ReviewAction { Edit(usize), Remove(usize), Caption(usize, String), Marker(MarkerKind), Format(ExportFormat), Export, Close, Keep, Discard }`.
  - `enum Effect { None, Save, Rethumb, OpenEditor(usize), Export(ExportFormat), Discard, Closed }`.
  - `fn ReviewModel::apply(&mut self, a: ReviewAction) -> Effect`:
    - Remove gives `Rethumb` and `Save`; this is one effect, `Rethumb` implies a save.
    - Caption gives `Save`, and Marker gives `Rethumb`.
    - Close gives `Confirming` while steps remain, and `Closed` when none do.
    - Keep returns to `Open`, and Discard gives `Discard`.
    - Export while `busy` or empty gives `None`.
  - `fn export_reason(&self) -> Result<(), &'static str>`.
  - `struct ReviewView`, with `fn show(&mut self, ui: &mut Ui, p: &Palette, mode: Mode, m: &ReviewModel, thumbs: &[Option<egui::TextureHandle>]) -> Vec<ReviewAction>`.
- Layout (painted):
  - A header with "N steps", the Marker dropdown, the Format dropdown, the primary **Export** (`download`) and a **Discard** icon button (`trash`).
  - Then a scrolling list of rows. Each row has a number chip, the thumbnail, the caption field ("Add a caption", a frameless multiline `TextEdit` styled with tokens), then **Edit** (`pen`) and **Remove** (`trash`), each tooltipped.
  - With `Confirming`, a painted card says "Discard these steps? They have not been exported." with **Keep** and **Discard**.
  - The error line uses `critical`.

- [ ] **Step 1: Write the failing tests:**
  - `count_text(1) == "1 step"`, `count_text(3) == "3 steps"` and `count_text(0) == "0 steps"`.
  - `a_steps_bar_is_narrower_than_a_video_bar_and_keeps_its_buttons` (headless context).
  - `removing_renumbers_and_rethumbs`. With 3 steps, `apply(Remove(1))` gives `Rethumb`, and `doc.steps.len() == 2`.
  - `closing_with_steps_asks_first`. `Close` gives `Confirming`, `Keep` gives `Open`, and `Close` then `Discard` gives `Effect::Discard`.
  - `closing_with_no_steps_just_closes`.
  - `export_is_refused_with_its_reason_when_empty`. `export_reason() == Err("There are no steps to export.")`, and `apply(Export) == Effect::None`.
  - `a_caption_is_saved`.
- [ ] **Step 2:** Run `cargo test -p windup-ui`. Expected: FAIL.
- [ ] **Step 3:** Implement with the existing painted widgets (`dropdown`, `button`, `badge`).
- [ ] **Step 4:** Run the checks. Expected: PASS.
- [ ] **Step 5:** Commit: "Steps: the step count bar and the review list".

### Task 9: The steps runtime and `--steps-export`

**Files:**
- Create: `crates/windup-desktop/src/steps/{mod.rs,live.rs,camera.rs}`
- Modify:
  - `crates/windup-core/src/ipc.rs`: `Launch::StepsExport`
  - `crates/windup-desktop/src/main.rs`: the flag, and `remove_stale_sessions` next to `remove_stale_segments`

**Interfaces:**
- Produces, in `camera.rs`:
  - `struct Shot { image: RgbaImage, pointer: Option<[f32; 2]>, monitor: DRect, monitor_name: String, scale: f32 }`.
  - `trait Camera: Send { fn shoot(&mut self, at: Option<[f32; 2]>) -> Result<Shot, CaptureError>; }`. `at` is the press point, when the watcher knew it.
  - `trait Conceal: Send + Sync { fn hide(&self, shot_monitor: DRect) -> bool; fn show(&self); }`. `hide` says whether it hid anything. `struct NoConceal` implements it.
  - `fn open_camera(route_order: &[StepRoute]) -> Result<(Box<dyn Camera>, StepRoute), String>`. It tries each route and takes one shot, thrown away. It is filled in by Tasks 10, 12 and 13.
- Produces, in `live.rs`:
  - `enum Trigger { Hotkey, Click(Press) }`.
  - `struct StepsLive`:
    - `fn start(cfg: &StepsConfig, takes: Takes, camera: Box<dyn Camera>, conceal: Arc<dyn Conceal>, bar: Arc<Mutex<BarFacts>>) -> Result<StepsLive, String>`. It creates the session folder and starts the threads.
    - `fn sender(&self) -> Sender<Trigger>`
    - `fn trigger(&self, t: Trigger)`
    - `fn toggle_pause(&self) -> bool`, which says whether it is now paused
    - `fn count(&self) -> u32`
    - `fn stop(&mut self)`
    - `fn cancel(&mut self)`
    - `fn poll(&mut self) -> Vec<StepsEvent>`
  - `struct BarFacts { rect: Option<DRect>, pointer_on_bar: bool, host: BarHost, excluded: bool }`. The host updates it.
  - `enum StepsEvent { Step(u32), Over(StepsEnd) }`.
  - `enum StepsEnd { Review(PathBuf), Discarded, Empty, Failed { why: String, review: Option<PathBuf> } }`.
  - `fn remove_stale_sessions()`.
- Produces, in `ipc.rs`: `Launch::StepsExport(StepsExport)`, from `--steps-export <dir> <html|pdf|md|images> <out>`. `windup --steps-export` exits 0 when `export` returns `Ok`.
- Log lines (the e2e reads them): `steps: started`, `steps: step N written`, `steps: bar hidden for the shot`, `steps: cancelled`, `steps: over (…)`.

- [ ] **Step 1: Write the failing tests** (unit tests in `live.rs`, with a `FakeCamera` that returns 32×24 pictures and can block on a barrier, and a `FakeConceal` that records the order of calls):
  - `two_steps_write_two_pngs_and_the_json`. `steps.json` has 2 steps with files `01.png` and `02.png`, and stop gives `Over(Review(dir))`.
  - `cancel_while_a_shot_is_being_written_leaves_no_folder`. The writer is held on a barrier and `cancel()` is called, then the barrier is released. `poll` gives `Over(Discarded)`, the folder does not exist, and 200 ms later it still does not.
  - `a_concealed_bar_is_hidden_for_the_shot_and_shown_after`. With `BarFacts { host: Outside, rect on the shot's monitor }`, the calls are `[hide, shoot, show]`. With the rect on another monitor, the calls are `[shoot]`.
  - `pause_ignores_triggers`. `toggle_pause()` then 2 triggers gives `count() == 0`.
  - `clicks_follow_takes`. With `Takes { clicks: false, hotkey: true }`, a `Trigger::Click` takes no step.
  - `a_click_on_the_bar_takes_no_step` (through `BarFacts`).
  - `three_failed_shots_end_with_what_was_taken`. Two good shots then three failures give `Failed { review: Some(dir) }`, and the doc has 2 steps.
  - `stop_with_no_steps_removes_the_folder`. It gives `Over(Empty)` and no folder.
  - `steps_export_flag_parses`, in core `ipc`: `parse_args(["--steps-export","/d","html","/o.html"])` gives `StepsExport { dir: "/d", format: Html, out: "/o.html" }`, and a format of `"doc"` gives `Unknown`.
- [ ] **Step 2:** Run `cargo test -p windup-desktop steps`. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - **The camera thread** receives `Trigger`s and runs `ClickFilter` with the `BarFacts`. For each accepted trigger it:
    1. calls `conceal.hide` when `must_conceal` says so
    2. shoots
    3. calls `show`
    4. hands the `Shot` to the writer
  - **The writer thread** encodes PNG at `CompressionType::Fast`, writes `NN.png` and saves the doc.
  - **Cancel** sets a flag the writer checks before each write. It joins both threads, then removes the folder.
  - **The session folder** comes from `new_session_dir(steps_root())`.
  - **`remove_stale_sessions`** removes every folder under `steps_root()` at startup, on the one Windup per display.
- [ ] **Step 4:** Run the checks. Expected: PASS.
- [ ] **Step 5:** Commit: "Steps: the session runtime and --steps-export".

### Task 10: Windows and X11: the picture, the pointer and the clicks

**Files:**
- Create: `crates/windup-desktop/src/steps/{windows.rs,x11.rs,clicks.rs}`
- Modify:
  - `crates/windup-desktop/src/capture/{windows.rs,x11.rs}`
  - `crates/windup-desktop/src/steps/camera.rs`
  - `crates/windup-desktop/Cargo.toml`: x11rb features `["randr", "xinput"]`
  - `crates/windup-core/src/ipc.rs`: `Launch::StepShotTest(PathBuf)`
  - `crates/windup-desktop/src/main.rs`

**Interfaces:**
- Produces:
  - `capture::windows::grab_monitor_at(x: f32, y: f32) -> Result<MonitorShot, CaptureError>` and `capture::x11::grab_monitor_at(x: f32, y: f32) -> Result<MonitorShot, CaptureError>`. Each is the existing per-monitor loop filtered to the monitor holding the point, else the nearest one.
  - `capture::x11::monitor_list() -> Vec<DRect>`.
  - `GdiCamera`, a `Camera`: the point is `at`, else `GetCursorPos` (physical pixels); `pointer = to_image(point, layout, size)`.
  - `X11Camera`, a `Camera`: `QueryPointer`, and the root position is the image position minus the monitor's origin.
  - `X11Conceal`, a `Conceal`. On its own x11rb connection it finds the bar with `x11::window_named("Windup recording")`, `ConfigureWindow`s it to x = −32000 and syncs with `GetInputFocus`. `show` restores the saved x and y.
  - `ClickWatch::start(tx: Sender<Trigger>) -> Option<ClickWatch>`. Dropping it stops it.
    - **Windows:** `WH_MOUSE_LL` on a thread of its own with a `GetMessageW` loop, installed only for the session. It **always** calls `CallNextHookEx` and sends `Press { button: windows_button(msg)?, at: Some(pt), t }`. Drop posts `WM_QUIT`.
    - **X11:** XInput2 `XISelectEvents` on the root for `RawButtonPress` with `XIAllMasterDevices`, on its own connection; `x11_button(detail)`, with `at` from `QueryPointer`. Drop closes the connection.
  - `windup --step-shot-test <out.png>` writes one step shot by `open_camera` and prints `pointer X,Y` or `pointer unknown`. It exits 0 when the PNG was written.

- [ ] **Step 1: Write the failing tests:**
  - `ipc`: `step_shot_test_is_a_hidden_flag`. It parses, and `USAGE` does not mention it.
  - `clicks.rs` (cfg windows): `the_mouse_hook_never_swallows`. The pure `hook_reply(msg) -> HookReply` gives `Pass` for every message, and `press_from(0x204, (5, 7), t)` gives `Some(Press { Right, at: Some([5.0, 7.0]) })`.
  - `capture/x11.rs`: `the_monitor_at_a_point_or_the_nearest`, as a pure `monitor_index_at(&[DRect], x, y) -> usize`. A point in the gap between two monitors picks the nearer one.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement. Every Win32 block is `cfg(windows)` with a twin, and every X11 block is `cfg(target_os = "linux")`.
- [ ] **Step 4:** Run the checks.
  - Windows by hand: `cargo run -- --step-shot-test %TEMP%\s.png` writes the pointer's monitor and prints a position inside it.
  - Add to `tools/linux-e2e.sh` in `x11_session`: `xdotool mousemove 400 300; "$bin" --step-shot-test /tmp/s.png`. The PNG is the size of the Xvfb screen and the output is `pointer 400,300`.
  - Expected: PASS on CI.
- [ ] **Step 5:** Commit: "Steps: shots, pointer and clicks on Windows and X11".

### Task 11: Clicks on Wayland: evdev, the udev rule and the input group

**Files:**
- Create:
  - `crates/windup-desktop/src/linux/steps/{mod.rs,evdev.rs,setup.rs}`
  - `packaging/linux/70-windup-pointer.rules`
- Modify:
  - `crates/windup-desktop/Cargo.toml`: `evdev = "0.13"` under the Linux target
  - `packaging/linux/package.sh`, `packaging/linux/windup.install`, `packaging/linux/check-enrolment.sh`
  - `docs/packaging.md`

**Interfaces:**
- Consumes: `evdev_button`, `TapDetector`, `pointer_kind`, `click_route` and `listed_in_group` (Task 3).
- Produces:
  - `fn click_facts() -> ClickFacts`:
    - It scans `/dev/input/event*` and counts the pointer devices through `pointer_kind` on the device's capabilities.
    - It counts the readable ones with `rustix::fs::access(R_OK)`.
    - `in_group_now` comes from `getgroups` against `input`'s gid.
    - `listed_in_group` reads `/etc/group`.
  - `EvdevWatch::start(tx: Sender<Trigger>) -> Result<EvdevWatch, ClickRoute>`. One thread polls every readable pointer node and sends `Press { at: None }`. A touchpad tap is a Left press, with millimetres from `ABS_X`/`ABS_Y` resolution. New devices are picked up on `IN_CREATE` in `/dev/input` (rustix inotify). Drop stops the thread.
  - `fn join_input_group() -> Result<(), String>`. It runs `pkexec usermod -aG input <user>`, where `<user>` comes from `getuid` via `/etc/passwd`. "Cancelled" (pkexec exit 126 or 127) is `Err("Cancelled.")`.
- The udev rule:
```
# Windup reads clicks (never keys) while recording steps. Mice and touchpads
# only; a device that is also a keyboard is left out.
SUBSYSTEM=="input", KERNEL=="event*", ENV{ID_INPUT_KEYBOARD}!="1", ENV{ID_INPUT_MOUSE}=="1", TAG+="uaccess"
SUBSYSTEM=="input", KERNEL=="event*", ENV{ID_INPUT_KEYBOARD}!="1", ENV{ID_INPUT_TOUCHPAD}=="1", TAG+="uaccess"
```
  - It is installed to `/usr/lib/udev/rules.d/70-windup-pointer.rules` in the deb, rpm and Arch packages, and not in the AppImage.
  - Installing and upgrading run `command -v udevadm >/dev/null && { udevadm control --reload-rules; udevadm trigger --subsystem-match=input --action=change; } || true`. This goes in the deb `postinst` and `postrm`, the rpm `%post` and `%postun`, and a new `_windup_udev` function in `windup.install` called from `post_install` and `post_upgrade`. The enrolment function stays verbatim.
  - `check-enrolment.sh` checks that the rules file exists after installing, in each of deb, rpm and Arch.

- [ ] **Step 1: Write the failing tests:**
  - `linux/steps/evdev.rs`: `facts_from(nodes: &[(bool, Option<PointerKind>)], groups…)` is pure and gives the counts. A node that is a keyboard is not counted.
  - `setup.rs`: `usermod_args("anna") == ["usermod", "-aG", "input", "anna"]`. A user name with a `:` or a `/` is refused.
  - `packaging/check.sh` gains a check that the rules file mentions no `ID_INPUT_KEY=` and has `ID_INPUT_KEYBOARD}!="1"` on every line that has `uaccess`.
- [ ] **Step 2:** Run the tests on CI or WSL. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the checks (the cross-clippy covers `evdev`) and `sh packaging/check.sh`. The CI packages job runs `check-enrolment.sh`. Expected: PASS.
- [ ] **Step 5:** Commit: "Steps: clicks on Wayland through evdev, with a udev rule".

### Task 12: Wayland shots: screencopy, cursor sessions, Sway and Hyprland

**Files:**
- Create: `crates/windup-desktop/src/linux/steps/screencopy.rs`
- Modify:
  - `crates/windup-desktop/src/linux/wayland/screencopy.rs`: `grab_output`
  - `crates/windup-desktop/src/capture/ipcrects.rs`: the parsers and the live calls
  - `crates/windup-desktop/src/steps/camera.rs`

**Interfaces:**
- Produces:
  - `pub fn grab_output(method: Method, name: &str, paint_cursor: bool) -> Result<MonitorShot, CaptureError>`. It shares `grab_ext` and `grab_wlr` with `grab_by`: ext gets `Options::PaintCursors` and wlr gets `overlay_cursor = 1` when `paint_cursor` is set.
  - `CursorTracker::start() -> Option<CursorTracker>`. It runs on its own connection and thread, binds a `wl_seat` and its `wl_pointer`, and creates one `create_pointer_cursor_session(source, pointer)` per output, following `enter`, `leave`, `position` and `hotspot`.
    - `fn where_now(&self) -> Option<(String, [f32; 2])>` gives the output name and `cursor_point(position, hotspot)` in buffer pixels.
    - It is `None` when no session reports an `enter` within 300 ms of the start.
  - `ScreencopyCamera::new(ext: bool, pointer: WlPointer) -> ScreencopyCamera`, a `Camera`. It picks the output as follows:
    - `CursorSession`: the tracker's output and position. With no tracker it falls back to `SwayFocus` when `SWAYSOCK` is set, else to `Unknown`.
    - `Hyprland`: `hyprland_cursor` mapped through `to_image`, with the output from the layout.
    - `SwayFocus`: the focused output, `pointer: None`, with the cursor painted.
    - `Unknown`: output 0, `pointer: None`, with the cursor painted.
  - In `ipcrects.rs`:
    - `pub(crate) fn sway_focused_output(json: &str) -> Option<(String, DRect)>` and `pub(crate) fn sway_focused_live() -> Option<(String, DRect)>`.
    - `pub(crate) fn hyprland_cursor(json: &str) -> Option<(f32, f32)>` and `pub(crate) fn hyprland_cursor_live() -> Option<(f32, f32)>`, through `j/cursorpos`.

- [ ] **Step 1: Write the failing tests:**
  - `sway_focused_output_is_the_one_marked_focused`, on a recorded `GET_OUTPUTS` reply with two outputs. It gives `("DP-2", DRect{1920,0,2560,1440})`, and an inactive output is skipped.
  - `hyprland_cursorpos_parses`. `{"x": 2100, "y": 300}` gives `(2100.0, 300.0)`, and junk gives `None`.
  - `paint_the_cursor_only_when_its_place_is_unknown`, as a pure `paint_cursor(pointer: WlPointer, tracker_found: bool) -> bool`.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the checks. Add to `sway_capture` in `tools/linux-e2e.sh`: `"$bin" --step-shot-test /tmp/s.png`. The PNG is the size of `HEADLESS-1`, and the output is `pointer unknown` or `pointer X,Y`. A headless Sway has no pointer device, so either is accepted. Expected: PASS on CI. The owner's checklist gains: on COSMIC, `--step-shot-test` gives a position.
- [ ] **Step 5:** Commit: "Steps: Wayland shots with screencopy and the pointer's place".

### Task 13: KDE and GNOME shots

**Files:**
- Create: `crates/windup-desktop/src/linux/steps/{kwin.rs,screencast.rs}`
- Modify:
  - `crates/windup-desktop/src/capture/kwin_rects.rs`: the cursor script and its parser
  - `crates/windup-desktop/src/linux/kde.rs`: `run_script`, factored out of `try_window_rects`; `cursor()`; and `capture_screen` made `pub(crate)`
  - `crates/windup-desktop/src/linux/video/{screencast.rs,pw_stream.rs}`
  - `crates/windup-core/src/video/screencast.rs`: `cursor_in_stream`, `STILL_EVERY`

**Interfaces:**
- Produces, for KWin:
  - `kwin_rects::cursor_script(service, path, iface, method) -> String`. It calls back with `JSON.stringify({x: workspace.cursorPos.x, y: workspace.cursorPos.y, screen: workspace.activeScreen.name})`.
  - `parse_cursor(json) -> Option<(f32, f32, String)>`.
  - `kde::cursor() -> Result<(f32, f32, String), String>`.
  - `KWinCamera`, a `Camera`: `cursor()`, then `capture_screen(conn, &screen)`, then `to_image`. A refusal of `CaptureScreen` (the AppImage) fails `open_camera`'s probe, so `ScreenCast` is tried next.
- Produces, for the portal:
  - The portal `session(…)` takes `cursor: CursorMode`. Video passes `Embedded` and is unchanged.
  - `pw_stream::spawn(fd, streams, want: Want, latest)` with `enum Want { Compose(DRect), Stills(Arc<Mutex<Vec<StillSlot>>>) }`:
    - `Stills` asks for `SPA_PARAM_Meta` of type `SPA_META_Cursor`, sized for a 64×64 bitmap.
    - Each stream keeps `StillSlot { frame: Option<(RgbaImage, Instant)>, cursor: Option<[f32; 2]> }`. A full frame is copied at most once per `STILL_EVERY = 100 ms`, and always the first one after `want_now` is set.
    - The cursor meta is read on every buffer, through `spa_buffer_find_meta_data`, which is the one `unsafe` block and is commented.
  - `open_screencast_stills(token: &Mutex<Option<String>>) -> Result<ScreenCastStills, CaptureError>`. It asks for `CursorMode::Metadata` when the portal's `available_cursor_modes` has it, else `Embedded` with no positions. It uses the same restore token file as video.
  - `PortalCamera`, a `Camera`, which holds the session open for the whole Steps session. The step's monitor is the stream whose cursor meta is valid; with none valid, it is the first stream, with `pointer: None`.
  - `cursor_in_stream(id: u32, position: (i32, i32), hotspot: (i32, i32), size: (u32, u32)) -> Option<[f32; 2]>`. It gives `None` when `id == 0` or the point is outside the frame.
- Token: `StepsLive` hands back a `TokenSlot`, which the host saves as video does (`Config::save_screencast_token`).

- [ ] **Step 1: Write the failing tests:**
  - `kwin_cursor_parses`. `{"x":10.5,"y":20,"screen":"DP-1"}` gives `Some((10.5, 20.0, "DP-1"))`, and a missing `screen` gives `None`.
  - `the_cursor_script_calls_back_with_position_and_screen`. It contains `workspace.cursorPos` and `workspace.activeScreen.name`.
  - `cursor_meta_maps_into_the_frame`. `(7, (100,50), (4,2), (1920,1080))` gives `Some([104.0, 52.0])`. `id 0` gives `None`, and `(1919,50)` with hotspot `(4,0)` gives `None`.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement. PipeWire code stays behind `feature = "pipewire"`, with a twin that returns `Unsupported`.
- [ ] **Step 4:** Run the checks. `pipewire` builds on CI only. The owner's checklist gains:
  - KDE, installed package: `--step-shot-test` with no dialog and a position.
  - KDE AppImage and GNOME: one dialog, then a position, with no dialog on the second run.
  - Expected: PASS.
- [ ] **Step 5:** Commit: "Steps: KDE and GNOME shots with the pointer's place".

### Task 14: Hosting on Windows and X11: start, keys, bar, review, editor

**Files:**
- Create:
  - `crates/windup-desktop/src/review_host.rs`
  - `crates/windup-desktop/src/steps/review.rs`: `ReviewHost`, shared with Task 15
- Modify:
  - `crates/windup-desktop/src/{hotkey.rs,main.rs,overlay.rs,recording.rs}`
  - `crates/windup-desktop/src/capture/finish.rs`
  - `tools/linux-e2e.sh`: `x11_steps`

**Interfaces:**
- Consumes: Tasks 7 to 10.
- Produces, in `hotkey.rs`:
  - `HotkeyListener::set_live(&self, live: Option<LiveKind>, config: &Config)` replaces `set_recording`.
  - The Windows hook has a `LIVE: AtomicU8` (0, 1 = Video, 2 = Steps) and one table per kind. While a kind is live, its table matches first and swallows, Esc included.
  - X11 rebinds with `live_bindings_for`.
  - `HotkeyEvent::Step` comes from `Action::Step`.
- Produces, in `recording.rs`:
  - `Rec.steps: Option<StepsLive>`, alongside `Rec.live` (video).
  - `fn start_steps(&mut self, ctx, choice: Option<StepsChoice>)`. `None` means the hotkey, with the saved choice. It calls `open_camera(steps_order(facts))` on a worker, starts the `ClickWatch` when `takes(..).clicks`, and puts up the bar.
  - The bar draws `RecBarState { steps: Some(count), .. }`:
    - Windows: placed by `top_centre` and excluded, with `BarFacts.excluded = exclude_from_capture(..)`.
    - X11: placed by `steps_bar_slot`, with `X11Conceal`.
  - Each frame `BarFacts.rect` is updated from `Bar.slot`.
  - `fn steps_hotkey(&mut self, ctx)` toggles start and stop. The Video hotkey and the capture hotkeys are ignored while Steps runs, and the Steps hotkey while video runs.
  - A Pause from the bar or the key calls `toggle_pause`. RecordStop and Esc map to `stop` and `cancel` for whichever session is live.
- Produces, in `review_host.rs`: `ReviewViewport`, a `show_viewport_immediate` window titled "Windup steps", `REVIEW_W × REVIEW_H` points.
  - `ReviewHost { dir, model: ReviewModel, thumbs, view }`, with `fn ui(&mut self, ctx) -> Option<Effect>`.
  - Thumbnails are made by `thumbnail(…, REVIEW_THUMB_W × ppp)` on a worker and uploaded as textures.
  - `Effect::Save` writes `steps.json`, debounced by 500 ms, and on close.
  - `Effect::Export(f)` asks for the destination with `rfd::FileDialog::save_file()` for HTML and PDF, or `pick_folder()` for Markdown and Images. The default name is `export_name(..)` and the default folder `steps.export_dir` or `default_steps_dir`. The export runs on a worker. On `Ok(path)`:
    1. the notice says "Exported to <name>", with "Open folder"
    2. the review closes
    3. the session folder is removed
  - On `Err`, the error line shows it and the folder is kept.
  - `Effect::Discard` removes the folder and closes.
  - `Effect::OpenEditor(i)` calls `overlay.edit_step(path, monitor)`.
- Produces, in `overlay.rs`: `fn edit_step(&mut self, ctx, path: PathBuf)`.
  - It opens `Session::edit_image` on the monitor named in the step, else on the pointer's.
  - `Outcome::StepDone(img)` writes the PNG over `path` (through a temp file, then a rename) and tells the review to `reload(i)`. `Cancel` writes nothing.
  - The gate is held, so no capture runs at the same time.
- Notices:
  - `StepsEnd::Empty` gives "No steps were taken."
  - `Failed` gives "Steps stopped: <why>". When some were taken, the review opens with them.
  - Starting while a review is open gives "Finish the steps you are reviewing first."

- [ ] **Step 1: Write the failing tests:**
  - `hotkey.rs`: `the_step_key_is_matched_only_during_steps`. With `match_key` and live `None`, Super+Shift+X is not matched. With Steps live it gives `Action::Step`, swallowed. With Video live, Super+Shift+X passes and Esc gives `RecordCancel`.
  - `hotkey.rs`: `during_steps_esc_cancels_and_the_steps_key_stops`.
  - `recording.rs`: `hold_close` holds a close while Steps runs. `hold_close(true, false, steps_busy)` is true.
  - `review_host.rs`: `an_export_that_failed_keeps_the_folder`. A pure `after_export(Result<PathBuf, String>) -> (remove_folder: bool, close: bool, error: Option<String>)`.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement.
  - `overlay.rs` has a private `steps(frame)`, which is about overlay frame steps. Import the module as `crate::steps as stepsrec` there to avoid the clash.
- [ ] **Step 4:** Run the checks. Add `x11_steps` to `tools/linux-e2e.sh`, run after `x11_record_cancel`:
  1. `xdotool key super+shift+g` gives `steps: started`.
  2. `xdotool mousemove 300 400 click 1`, then `sleep 0.6`, then `click 3`, then `key super+shift+x` give `steps: step 3 written`.
  3. `key super+shift+g` gives `steps: review opened`.
  4. `steps.json` has 3 steps, each with a non-null `pointer` (python3 json).
  5. `"$bin" --steps-export "$dir" html /tmp/x.html` gives 3 `<img`.
  6. Close the review: `xdotool search --name '^Windup steps$' windowactivate key alt+F4`, then click Discard via `key Return`. The folder is gone.
  7. A second session: start, `click 1`, then `key Escape` gives `steps: cancelled`, and `$home/.cache/windup/steps` is empty.

  Run it by hand on Windows: start, three clicks (one of them on the bar's pause button, which is not a step), stop, edit a step, export HTML, open it.
  Expected: PASS on CI and by hand.
- [ ] **Step 5:** Commit: "Steps: record, review and export on Windows and X11".

### Task 15: Hosting on Wayland: the background loop, the bar, the keys, the review child

**Files:**
- Create: `crates/windup-desktop/src/steps/review_app.rs`
- Modify:
  - `crates/windup-core/src/ipc.rs`: `Request`, and `Launch::StepsReview`
  - `crates/windup-desktop/src/linux/{mod.rs,capture.rs,ipc.rs,portal.rs,cosmic.rs}`
  - `crates/windup-desktop/src/linux/wayland/{mod.rs,recbar.rs}`
  - `crates/windup-desktop/src/main.rs`
  - `tools/linux-e2e.sh`: `sway_steps`

**Interfaces:**
- Produces, in core `ipc`:
  - `enum Request { Do(Command), EditStep(PathBuf) }`, with `fn parse(line: &str) -> Option<Request>` and `fn encode(&self) -> String`. The wire form is `edit-step <path>`: the rest of the line is the path, and a path holding `\n` is refused at encode.
  - `Launch::StepsReview(PathBuf)`, from `--steps-review <dir>`. It is hidden.
- Produces, in `linux::ipc`: `serve` hands `Request`s to its callback, and `send_request(&Request)`.
- Produces, in `linux/capture.rs`, on `Background`:
  - `steps: Option<StepsLive>`.
  - `steps_command()` (`--steps`) toggles, and `step_command()` (`--step`) calls `trigger(Hotkey)`. `--steps-stop` is `stop`, and `RecordPause`, `RecordStop` and `RecordCancel` go to whichever session is live.
  - `start_steps_from(choice)` comes from the overlay's `Outcome::Steps`.
  - `sync_recording_keys` becomes `sync_live_keys` and binds `session_bindings(config, kind)`:
    - COSMIC `add_temporary` gains `--step`.
    - The portal session's ids include `step`, with the description "Take a step".
  - On `StepsEnd::Review(dir)` it spawns `windup --steps-review <dir>` as a `Child`, kept like `settings`, and logs `steps: review opened`.
  - `Request::EditStep(path)` opens the overlay on that picture (`edit_image`, on the output named in the step, else the first output). On `StepDone` it writes the file back.
- Produces, in `linux/wayland`:
  - `RecBarSetup` gains `steps: Option<Arc<AtomicU32>>` and `pointer_on_bar: Arc<AtomicBool>`, which is set on pointer enter and leave on the bar's surface.
  - `Service::conceal_recbar(hide: bool) -> Receiver<()>`. Hiding attaches a null buffer and commits; showing redraws. It acknowledges after the next `wl_surface.frame` callback, or after 100 ms.
  - `LayerConceal`, a `Conceal`, wraps it with a 250 ms wait.
  - On the window host (GNOME, Weston) there is no bar, as for video.
- Produces, in `review_app.rs`: `fn run(dir: PathBuf) -> eframe::Result<()>`. A small eframe app around `ReviewHost`, whose `Effect::OpenEditor(i)` becomes `send_request(&Request::EditStep(path))`. It polls the edited file's mtime every 500 ms and rethumbs it. Export and discard work as on X11.

- [ ] **Step 1: Write the failing tests:**
  - core `ipc`:
    - `edit_step_requests_round_trip`. `Request::parse("edit-step /tmp/a b/01.png\n")` gives `Some(EditStep("/tmp/a b/01.png"))`, `Request::parse("edit-step ")` gives `None`, `Request::parse("popup")` gives `Some(Do(Popup))`, and `encode` then `parse` gives the same request back.
    - `steps_review_is_hidden`.
  - `cosmic_shortcuts`: `steps_temporary_keys_include_step`. `add_temporary(base, &[(super_shift_x, "--step"), …])`, then `remove_temporary`, leaves the file byte-identical.
  - `linux/capture.rs`: `live_kind_follows_the_session`, as a pure `fn live_kind(video_busy: bool, steps_busy: bool) -> Option<LiveKind>`.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the checks. Add `sway_steps` to `wayland_session`, after `sway_record_live`:
```sh
sway_steps() {
    local root=$home/.cache/windup/steps log=$art/wayland-windup.log dir out=/tmp/windup-e2e-steps-$$.html
    "$bin" --steps
    wait_for 100 grep -q "steps: started" "$log" || { bad "wayland: --steps did not start"; grep steps "$log"; return; }
    ok "wayland: --steps started at once"
    "$bin" --step; wait_for 50 grep -q "steps: step 1 written" "$log"
    "$bin" --step
    if wait_for 50 grep -q "steps: step 2 written" "$log"; then ok "wayland: --step twice took two steps"; else bad "wayland: --step did not take two steps"; fi
    "$bin" --steps-stop
    if wait_for 100 grep -q "steps: review opened" "$log"; then ok "wayland: --steps-stop opened the review"; else bad "wayland: no review"; return; fi
    dir=$(ls -d "$root"/*/ | head -1)
    if python3 -c 'import json,sys; assert len(json.load(open(sys.argv[1]))["steps"]) == 2' "$dir/steps.json"; then
        ok "wayland: steps.json lists two steps"; else bad "wayland: steps.json: $(cat "$dir/steps.json")"; fi
    if timeout 30 "$bin" --steps-export "$dir" html "$out" && [ "$(grep -o '<img' "$out" | wc -l)" -eq 2 ] && ! grep -q '<script' "$out"; then
        ok "wayland: the HTML export has two pictures and no script"; else bad "wayland: the HTML export is wrong"; fi
    pkill -f -- '--steps-review' ; rm -rf "$root" "$out"
}
```
  Expected: PASS on CI. The owner's checklist gains:
  - COSMIC, KDE and GNOME: Esc during Steps discards, `--step` from the desktop's key takes a step, and Edit opens the editor and the thumbnail updates.
  - COSMIC with clicks set up: clicks take steps, and a click on the bar does not.
- [ ] **Step 5:** Commit: "Steps: record, review and export on Wayland".

### Task 16: Settings pane, docs and changelog

**Files:**
- Create: `crates/windup-ui/src/settings/steps_cfg.rs`
- Modify:
  - `crates/windup-ui/src/settings/mod.rs`: `SettingsPane::Steps`, with the icon `footprints` and the blurb "Step-by-step guides, their markers and exports."
  - `crates/windup-desktop/src/main.rs`: `StepsFacts` for the settings window, and the `pkexec` action
  - `README.md`, `docs/linux.md`, `docs/packaging.md`
  - `CHANGELOG.md`: `## Unreleased`
  - `CLAUDE.md`: "What this is" (six tools), and a "Steps" paragraph under "Capture"
  - `docs/superpowers/specs/2026-10-28-capture-design.md`: the Steps default key, and Esc, which evdev no longer watches

**Interfaces:**
- Consumes: plain data from the desktop: `StepsFacts { route: ClickRoute, appimage: bool, markers: Result<(), &'static str>, bar_shown: bool, esc: EscCaution, available: Result<(), &'static str> }`.
- Produces: `SettingsView.steps: StepsFacts`, and the action `SettingsAction::JoinInputGroup`. The desktop runs `join_input_group` on a worker; the settings child does this on Wayland.
- Pane rows, in order:
  1. **Shortcut** (Steps), **Take a step**, **Pause** (bound to `video.pause_shortcut`, noted "Shared with Video"), and **Cancel**, a fixed Esc keycap: "Esc always cancels and throws the steps away."
  2. **What takes a step**: the three choices. The clicks entries are disabled with `clicks_reason`.
  3. **Clicks on this desktop**, shown only when the route is not `Native`:
     - Udev: "Clicks come from the mouse through Windup's device rule."
     - InputGroup: "Clicks come from the mouse; your account is in the input group."
     - AfterLogout: "Log out and back in to finish setting up clicks."
     - NotSetUp: the explanation, then the button **Capture clicks too**. The explanation reads: "Wayland does not let applications see clicks in other windows. Windup can read the mouse directly if your account is in the input group. That group can also read the keyboard. You will be asked for your password, and it takes effect after you log out and back in."
       - With a package installed, it adds "Reinstalling Windup's package sets up a rule for the mouse alone."
       - The AppImage shows the group only.
     - NoPointer: "No mouse or touchpad was found."
  4. **Marker**: a kind segment (ring, badge, arrow), a colour swatch (the existing picker) and a size dropdown, with a painted preview of the marker on a sample tile.
  5. The caution lines that apply: the marker reason; the EscCaution line (reused); "On this desktop the bar is not shown; press the Steps key to stop."; and `available`.
  6. **Export as** (the format), **Export folder** and **File name**.

- [ ] **Step 1: Write the failing tests:**
  - `the_route_line_per_click_route`, as a pure `route_line(ClickRoute, appimage) -> Option<&str>`. `Native` gives `None`, `AfterLogout` gives the logout text, and `(NotSetUp, true)` has no "Reinstalling".
  - `clicks_choices_are_disabled_without_clicks`. With the route `NotSetUp`, `trigger_choices(..)` marks "Clicks only" disabled with its reason.
  - `changing_the_marker_saves_it`. A change sets `config.steps.marker` and calls `save`, as the other panes do.
- [ ] **Step 2:** Run `cargo test -p windup-ui settings`. Expected: FAIL.
- [ ] **Step 3:** Implement with the existing painted row widgets. Docs:
  - README: a Steps feature line.
  - `docs/linux.md`: clicks on Wayland, the udev rule, the input group, Sway's missing marker, and GNOME's sharing indicator.
  - CHANGELOG: `## Unreleased` with Steps.
- [ ] **Step 4:** Run the checks, `sh packaging/check.sh` and `bash tools/linux-e2e.sh target/debug/windup all`, on CI. Expected: PASS.
- [ ] **Step 5:** Commit: "Steps: settings pane and docs".

