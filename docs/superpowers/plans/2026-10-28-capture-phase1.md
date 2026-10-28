# Capture, phase 1 (hotkeys and screenshots) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Configurable hotkeys for every tool, and screenshots (launcher bar, Area / Window / Full screen, live editor) that look and behave the same on Windows, X11 and every Wayland host.

**Architecture:** Every decision that can be pure lives in `windup-core::capture` (desktop geometry, selection, annotations, rasterising, window picking, file naming) and is tested there. `windup-ui::capture` draws one monitor's slice of a shared session with `Painter`, knowing nothing about platforms. `windup-desktop` grabs the frozen screens (one back end per platform, chosen by a pure function), creates one overlay surface per monitor (eframe viewports on Windows/X11, our own Wayland surfaces elsewhere), feeds input to the session and acts on its result (copy, save, cancel).

**Tech Stack:** Rust 2024, egui/eframe 0.32, egui-wgpu, windows-sys 0.61, x11rb, smithay-client-toolkit 0.20 + wayland-protocols 0.32 (staging) + wayland-protocols-wlr, ashpd 0.11 (zbus already in the tree), new: `tiny-skia` (BSD-3) for rasterising annotations, `image` gains the `jpeg` feature.

**Spec:** `docs/superpowers/specs/2026-10-28-capture-design.md`. Read it first; this plan argues from it and does not repeat its reasons.

## Global Constraints

- Same experience on every platform; only the capture underneath differs. No system picker or dialog in normal use; at most a one-time portal grant on GNOME/KDE.
- Everything painted with `Painter`, colours and sizes from `theme.rs` tokens only, icons Lucide verbatim in `icons.rs` (CLAUDE.md "UI conventions"). British spelling, Sentence case, no em dashes in UI copy.
- Every Win32 block `#[cfg(target_os = "windows")]` with a no-op twin; Linux code under `windup-desktop/src/linux/`; all crates still compile on macOS.
- CI warning-free with `RUSTFLAGS=-D warnings` on Windows x64, Linux, Windows ARM64. Cross-check Linux from Windows with `cargo clippy --target x86_64-unknown-linux-gnu` (CLAUDE.md recipe).
- `cosmic-protocols` stays out (GPL-3.0-only).
- **Esc always cancels the overlay and editor, and is not rebindable.**
- Enter = copy and close; Ctrl+S = save and close; "Also save a file when copying" makes Enter do both. Undo Ctrl+Z, redo Ctrl+Shift+Z and Ctrl+Y.
- Defaults: launcher Win+Shift+S, screenshot Print Screen. Video and Steps icons are shown in the launcher but **disabled with a tooltip** ("Arrives in a later version.") until phases 2 and 3.
- Saved images: PNG default, JPEG option, `<Pictures>/Windup/Screenshot YYYY-MM-DD HHMMSS.png`.
- Area selections may cross monitors; the result is rendered at the highest scale among the crossed monitors; gaps transparent in PNG, black in JPEG.
- Window highlight animates ~160 ms with the house easing, none under reduced motion.
- Marching ants: dash 4 / gap 4 pt, 8 pt/s, repaint every 60 ms, width rounded to whole device pixels.

## Review Focus

1. **An existing `config.json` from 0.1.2** (no capture fields) must load with every old setting intact and capture defaults filled in — not fall back to all defaults. Test in Task 1.
2. **A user-configured shortcut that collides** with another tool's (two tools on Win+Shift+S) must fire one deterministic tool, and the settings row must show the clash. Test in Task 2.
3. **Monitors with negative coordinates** (a monitor left of or above the primary) must select, stitch and crop correctly. Tests in Tasks 5 and 6.
4. **A zero-size or one-pixel selection** (a click without a drag in Area mode) must not produce an empty image or panic; a click in Area mode with no drag does nothing. Test in Task 6.
5. **Capture failure** (portal denied, Weston with no capture protocol, KWin refusing) must close cleanly with a toast saying what to do, never a black overlay or a hang. Test of the pure fallback order in Task 13; behaviour checked in Task 18's e2e.

---

## File Structure

`windup-core/src/capture/` (new, pure, tested)
- `mod.rs` — re-exports.
- `geometry.rs` — `DRect`, `MonitorShot`, `DesktopShot`, stitching.
- `selection.rs` — `Target`, `Selection`, drag/resize/nudge state machine.
- `annotate.rs` — `Annotation`, `Doc` (objects + undo/redo), hit test, transforms.
- `raster.rs` — flatten annotations onto a stitched image (tiny-skia), blur, mosaic.
- `pick.rs` — window rectangle under a point; edge detection fallback; highlight tween.
- `output.rs` — file naming, folder resolution, PNG/JPEG encode.
- `backend.rs` — pure choice of capture back end and window-rect source.

`windup-core/src/hotkeys.rs` (new) — `Action`, bindings table from `Config`, key names, Windows VK map, clash detection.

`windup-ui/src/capture/` (new)
- `mod.rs` — `CaptureView`: per-monitor drawing and input for a `Session`.
- `bar.rs` — launcher bar and editor tool bar (option C).
- `ants.rs` — marching ants, ported from Umber.
- `loupe.rs` — zoom loupe.
`windup-ui/src/colour.rs` (new) — Umber's picker, Wheel mode, ported.
`windup-ui/src/settings/capture_cfg.rs`, `screenshot_cfg.rs` (new) — panes.

`windup-desktop/src/capture/` (new)
- `mod.rs` — `grab_all() -> Result<DesktopShot, CaptureError>` dispatching to a back end; `window_rects()`.
- `windows.rs` — GDI grab, `EnumWindows` + DWM frame bounds.
- `x11.rs` — x11rb `GetImage` per RandR monitor, `_NET_CLIENT_LIST_STACKING`.
- `ipcrects.rs` — Sway and Hyprland IPC window rects.
`windup-desktop/src/linux/wayland/screencopy.rs` — ext-image-copy-capture and wlr-screencopy.
`windup-desktop/src/linux/wayland/overlay.rs` — overlay surfaces (layer or fullscreen xdg toplevel), one per output.
`windup-desktop/src/linux/kde.rs` — KWin ScreenShot2 and the KWin geometry script.
`windup-desktop/src/linux/portal.rs` — gains the Screenshot portal.
`windup-desktop/src/overlay.rs` — Windows/X11 overlay: one eframe viewport per monitor.

---

### Task 1: Tool ids and config for the launcher and screenshots

**Files:**
- Modify: `crates/windup-core/src/tool.rs`, `crates/windup-core/src/config.rs`, `crates/windup-core/src/shortcut.rs`
- Test: in-file `#[cfg(test)]` modules

**Interfaces:**
- Produces:
  - `ToolId::CaptureLauncher`, `ToolId::Screenshot`; `ToolId::ALL` gains both (len 4). `as_str`: `"capture_launcher"`, `"screenshot"`. `display_name`: `"Capture"`, `"Screenshot"`.
  - `Shortcut::super_shift_s()`, `Shortcut::print_screen()` (`key: "PRINTSCREEN"`, no modifiers).
  - `pub struct CaptureLauncherConfig { enabled: bool, shortcut: Shortcut }`, default enabled, Win+Shift+S.
  - `pub enum ImageFormat { Png, Jpeg }`, `pub enum TargetKind { Area, Window, FullScreen }` (serde, `Default` = `Png` / `Area`).
  - `pub struct EditorKeys { copy: Shortcut, save: Shortcut, undo: Shortcut, redo: Shortcut, loupe: Shortcut, select_all: Shortcut }` defaults Enter, Ctrl+S, Ctrl+Z, Ctrl+Shift+Z, `Z`, Ctrl+A. Esc is deliberately not a field.
  - `pub struct ScreenshotConfig { enabled, shortcut, loupe_on: bool (false), save_dir: Option<PathBuf>, name_pattern: String ("Screenshot {date} {time}"), format: ImageFormat, also_save_on_copy: bool (false), last_target: TargetKind, colour: [u8; 4] (the `critical` red of the dark palette), sizes: ToolSizes, keys: EditorKeys }`.
  - `pub struct ToolSizes { pen: f32 (4), rect: f32 (3), ellipse: f32 (3), blur: f32 (12), mosaic: f32 (12) }`.
  - `Config.capture: CaptureLauncherConfig`, `Config.screenshot: ScreenshotConfig`, both `#[serde(default)]`; every field of the new structs `#[serde(default)]` too. `is_tool_enabled` / `set_tool_enabled` / `tool_shortcut` / `set_tool_shortcut` cover the new ids.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_012_config_keeps_its_settings_and_gains_capture_defaults() {
    // config.json exactly as 0.1.2 writes it (copy one from serde_json::to_string_pretty(&old default)
    // with accent_hue 200.0 and clipboard.max_items 42 changed)
    let c: Config = serde_json::from_str(OLD_CONFIG).unwrap();
    assert_eq!(c.accent_hue, 200.0);
    assert_eq!(c.clipboard.max_items, 42);
    assert_eq!(c.capture.shortcut, Shortcut::super_shift_s());
    assert_eq!(c.screenshot.shortcut, Shortcut::print_screen());
    assert!(!c.screenshot.loupe_on);
}

#[test]
fn every_tool_has_a_default_shortcut_and_none_collide() {
    let mut seen = std::collections::HashSet::new();
    for t in ToolId::ALL {
        assert!(seen.insert(t.default_shortcut().unwrap()), "{t:?}");
    }
}
```

- [ ] **Step 2: Run** `cargo test -p windup-core a_012_config` — FAIL (no field `capture`).
- [ ] **Step 3: Implement** the types above. `Shortcut` `Display` shows `PRINTSCREEN` as `Print Screen`.
- [ ] **Step 4: Run** `cargo test -p windup-core` — PASS.
- [ ] **Step 5: Commit** `git commit -m "Capture: tool ids and config"`

---

### Task 2: The bindings table

**Files:**
- Create: `crates/windup-core/src/hotkeys.rs` (add `pub mod hotkeys;` to `lib.rs`)

**Interfaces:**
- Consumes: Task 1 `ToolId`, `Config::tool_shortcut`, `Config::is_tool_enabled`.
- Produces:
  - `pub enum Action { ClipboardPopup, PastePlain, CaptureLauncher, Screenshot }`, `Action::for_tool(ToolId) -> Action`.
  - `pub fn bindings(config: &Config) -> Vec<(Shortcut, Action)>` — enabled tools only, in `ToolId::ALL` order; a shortcut already taken by an earlier tool is left out (first wins).
  - `pub fn clashes(config: &Config) -> Vec<ToolId>` — enabled tools whose shortcut an earlier enabled tool already has.
  - `pub fn windows_vk(key: &str) -> Option<u16>` — `"A"`–`"Z"` → 0x41–0x5A, `"0"`–`"9"`, `"F1"`–`"F24"` → 0x70–0x87, `"PRINTSCREEN"` → 0x2C, `"SPACE"`, `"ENTER"`, `"TAB"`, `"INSERT"`, `"DELETE"`, `"HOME"`, `"END"`, `"PAGEUP"`, `"PAGEDOWN"`.
  - `pub fn xkb_keysym_name(key: &str) -> Option<&'static str>` — same key set, e.g. `"PRINTSCREEN"` → `"Print"`, `"V"` → `"v"` (COSMIC and portals use these).

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn defaults_bind_all_four() { assert_eq!(bindings(&Config::default()).len(), 4); }
#[test] fn a_disabled_tool_is_not_bound() { /* screenshot.enabled = false → no (PrintScreen, Screenshot) */ }
#[test] fn the_first_tool_wins_a_shared_shortcut() {
    let mut c = Config::default();
    c.screenshot.shortcut = Shortcut::super_shift_s();
    let b = bindings(&c);
    assert_eq!(b.iter().filter(|(s, _)| *s == Shortcut::super_shift_s()).count(), 1);
    assert!(b.contains(&(Shortcut::super_shift_s(), Action::CaptureLauncher)));
    assert_eq!(clashes(&c), vec![ToolId::Screenshot]);
}
#[test] fn vk_codes() { assert_eq!(windows_vk("PRINTSCREEN"), Some(0x2C)); assert_eq!(windows_vk("V"), Some(0x56)); assert_eq!(windows_vk("F13"), Some(0x7C)); assert_eq!(windows_vk("nonsense"), None); }
```

- [ ] **Step 2: Run** `cargo test -p windup-core hotkeys` — FAIL.
- [ ] **Step 3: Implement.** Also teach `windup-ui/src/widgets/badge.rs` `shortcut_recorder` to record F1–F24, Space and Print Screen as these names (egui reports Print Screen as no `Key`; read it from the raw `Event::Key` with `physical_key` where egui gives one, else leave it to the default only).
- [ ] **Step 4: Run** `cargo test -p windup-core` — PASS.
- [ ] **Step 5: Commit** `"Hotkeys: one bindings table from the config"`

---

### Task 3: Hotkeys honour the config on Windows and X11

**Files:**
- Modify: `crates/windup-desktop/src/hotkey.rs`, `crates/windup-desktop/src/main.rs` (event match)

**Interfaces:**
- Consumes: Task 2 `bindings`, `windows_vk`, `Action`.
- Produces:
  - `HotkeyEvent::Capture { target: Target }`, `HotkeyEvent::Screenshot { target: Target }`.
  - `HotkeyListener::new(plan: &Plan, config: &Config)`; `HotkeyListener::rebind(&self, config: &Config)` — called by `WindupApp::update` whenever `Config` changes (compare `bindings()` with the last applied).

- [ ] **Step 1:** Windows: replace the `VK_V` branch with a lookup in a `static BINDINGS: RwLock<Vec<(u16 /*vk*/, ShortcutModifiers, Action)>>`. Modifier state still from `GetAsyncKeyState` (CTRL, ALT, SHIFT, L/RWIN); match requires **exact** modifiers. Keep the injected-key skip, `AllowSetForegroundWindow`, the `0xFF` dummy keyup only when Win is held, and `return 1`. A binding with no modifiers (Print Screen) swallows the key.
- [ ] **Step 2:** X11: unregister all and register from `bindings()` on `rebind`; map key names to `global_hotkey::hotkey::Code` with a `fn code_for(key: &str) -> Option<Code>` next to the thread.
- [ ] **Step 3: Verify manually** on Windows: `cargo run`, change Clipboard history to Ctrl+Alt+H in settings, press it — popup opens; Win+V no longer opens Windup's popup. Change back. Print Screen logs `Screenshot` at `RUST_LOG=debug`.
- [ ] **Step 4:** `cargo clippy --workspace --all-targets` and the Linux cross-clippy — clean.
- [ ] **Step 5: Commit** `"Hotkeys: Windows hook and X11 grabs read the config"`

---

### Task 4: Wayland commands for capture

**Files:**
- Modify: `crates/windup-core/src/ipc.rs`, `crates/windup-core/src/cosmic_shortcuts.rs`, `crates/windup-desktop/src/linux/{mod.rs,cosmic.rs,portal.rs}`

**Interfaces:**
- Produces: `Command::Capture` (`"capture"`, flag `--capture`), `Command::Screenshot` (`"screenshot"`, `--screenshot`); `USAGE` lists both. `cosmic_shortcuts::wanted(config: &Config) -> Vec<Wanted>` replaces the `WANTED` const (owned strings: `Wanted { modifiers: Vec<String>, key: String, description: String, args: String }`), built from `hotkeys::bindings` + `xkb_keysym_name`. `portal::start_shortcuts(plan, config)` registers one GlobalShortcuts entry per binding.

- [ ] **Step 1: Write the failing tests** in `ipc.rs` and `cosmic_shortcuts.rs`:

```rust
#[test] fn capture_flags_parse() { assert_eq!(parse_args(["--capture"]), Launch::Send(Command::Capture)); assert_eq!(parse_args(["--screenshot"]), Launch::Send(Command::Screenshot)); }
#[test] fn wanted_follows_the_config() {
    let w = wanted(&Config::default());
    assert!(w.iter().any(|w| w.args == "--capture" && w.key == "s" && w.modifiers == ["Super", "Shift"]));
    assert!(w.iter().any(|w| w.args == "--screenshot" && w.key == "Print" && w.modifiers.is_empty()));
}
```
  Existing `cosmic_shortcuts` tests keep passing with `wanted(&Config::default())` in place of `WANTED`.
- [ ] **Step 2: Run** `cargo test -p windup-core ipc cosmic` — FAIL.
- [ ] **Step 3: Implement;** in `linux/mod.rs` `Background::command`, route `Capture`/`Screenshot` to `start_capture(mode)` (Task 19). Until then they log and do nothing.
- [ ] **Step 4: Run** `cargo test -p windup-core` — PASS.
- [ ] **Step 5: Commit** `"Wayland: --capture and --screenshot, shortcuts from the config"`

---

### Task 5: Desktop geometry and stitching

**Files:**
- Create: `crates/windup-core/src/capture/{mod.rs,geometry.rs}`; add `pub mod capture;` and the `image` `jpeg` feature at workspace level.

**Interfaces:**
- Produces:
  - `#[derive(Clone, Copy, Debug, PartialEq)] pub struct DRect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }` in **desktop units** (Windows virtual-screen physical pixels; X11 root pixels; Wayland logical layout). Methods `contains`, `intersect -> Option<DRect>`, `union`, `normalised` (positive w/h from any two corners), `from_corners(a, b)`.
  - `pub struct MonitorShot { pub layout: DRect, pub image: image::RgbaImage, pub name: String }`; `fn px_per_unit(&self) -> f32` = `image.width() as f32 / layout.w`.
  - `pub struct DesktopShot { pub monitors: Vec<MonitorShot> }`; `fn bounds(&self) -> DRect`; `fn monitor_at(&self, x: f32, y: f32) -> Option<usize>`; `fn compose(&self, rect: DRect) -> Option<image::RgbaImage>` — `None` when `rect` is smaller than one output pixel either way; output scale = max `px_per_unit` of monitors the rect touches; other monitors scaled with `image::imageops::FilterType::Triangle`; uncovered pixels transparent `[0,0,0,0]`.

- [ ] **Step 1: Write the failing tests** (build monitors from solid-colour images):

```rust
#[test] fn compose_inside_one_monitor_is_a_crop() { /* 1920×1080 @ (0,0), rect (10,10,100,50) → 100×50, all pixels that colour */ }
#[test] fn compose_across_a_left_monitor_with_negative_x() {
    // A: layout (-1920,0,1920,1080) red; B: (0,0,1920,1080) blue; rect (-50,0,100,10) → 100×10, left half red, right half blue
}
#[test] fn mixed_scale_uses_the_highest() {
    // A: layout (0,0,1000,500) image 1000×500; B: layout (1000,0,1000,500) image 2000×1000
    // rect (900,0,200,100) → 400×200
}
#[test] fn gaps_are_transparent() { /* B at (1920,200,...) ; rect spanning y 0..300 at the seam → pixel above B transparent */ }
#[test] fn a_degenerate_rect_composes_to_none() { assert!(shot.compose(DRect { x: 5.0, y: 5.0, w: 0.3, h: 40.0 }).is_none()); }
```
- [ ] **Step 2: Run** `cargo test -p windup-core capture::geometry` — FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `"Capture: desktop geometry and stitching"`

---

### Task 6: Selection

**Files:**
- Create: `crates/windup-core/src/capture/selection.rs`

**Interfaces:**
- Consumes: Task 5 `DRect`, `DesktopShot`; Task 1 `TargetKind`.
- Produces:
  - `pub enum Handle { N, S, E, W, NE, NW, SE, SW, Move }`.
  - `pub struct Selection { pub rect: Option<DRect>, pub kind: TargetKind, drag: Option<Drag> }` with
    `press(&mut self, at: (f32, f32), handle_tolerance: f32)`, `drag_to(&mut self, at: (f32, f32), bounds: DRect)`, `release(&mut self)`,
    `handle_at(&self, at, tolerance) -> Option<Handle>`, `nudge(&mut self, dx: f32, dy: f32, edge: Option<Handle>)`, `select_all(&mut self, bounds: DRect)`, `set_rect(&mut self, rect: DRect)` (Window / Full screen picks).
  - `pub const MIN_DRAG: f32 = 3.0;` — a press-release that moved less than this in Area mode leaves `rect` unchanged.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn dragging_up_and_left_normalises() { /* press (100,100), drag (40,30) → rect (40,30,60,70) */ }
#[test] fn a_click_without_a_drag_selects_nothing() { /* press+release at same point → rect None */ }
#[test] fn a_drag_continues_across_a_negative_monitor() { /* bounds (-1920,0,3840,1080); press (-100,10) drag (100,50) → (-100,10,200,40) */ }
#[test] fn dragging_is_clamped_to_the_desktop() { /* drag to (99999, 99999) → rect ends at bounds' far corner */ }
#[test] fn resizing_past_the_opposite_edge_flips() { /* SE handle dragged left of the W edge → rect still positive w */ }
#[test] fn nudge_moves_one_unit_or_one_edge() { /* nudge(1,0,None) moves; nudge(1,0,Some(E)) grows w by 1 */ }
#[test] fn select_all_is_the_desktop() { /* select_all(bounds) → rect == bounds */ }
```
- [ ] **Step 2–4:** run (FAIL), implement, run (PASS) with `cargo test -p windup-core capture::selection`.
- [ ] **Step 5: Commit** `"Capture: selection"`

---

### Task 7: Annotations as objects, with undo

**Files:**
- Create: `crates/windup-core/src/capture/annotate.rs`

**Interfaces:**
- Consumes: Task 5 `DRect`, Task 6 `Handle`.
- Produces:
  - `pub enum Tool { Select, Pen, Rect, Ellipse, Blur, Mosaic, Pipette }`.
  - `pub enum Shape { Pen(Vec<(f32, f32)>), Rect(DRect), Ellipse(DRect), Blur(DRect), Mosaic(DRect) }`; `Shape::tool() -> Tool`; `Shape::bounds() -> DRect`.
  - `pub struct Annotation { pub id: u64, pub shape: Shape, pub colour: [u8; 4], pub size: f32 }` (size: stroke width, blur radius or mosaic block, in desktop units).
  - `pub struct Doc { pub items: Vec<Annotation>, pub selected: Option<u64>, .. }` with
    `add(&mut self, a: Annotation)`, `hit(&self, at: (f32, f32), tolerance: f32) -> Option<u64>` (topmost first; pen hits within `size/2 + tolerance` of a segment; rect/ellipse outlines hit near the outline; blur/mosaic hit anywhere inside),
    `begin_transform(&mut self, id: u64, handle: Handle, at)`, `transform_to(&mut self, at, keep_aspect: bool)`, `end_transform(&mut self)`,
    `set_colour(&mut self, id, colour)`, `set_size(&mut self, id, size)`, `delete(&mut self, id)`, `undo(&mut self) -> bool`, `redo(&mut self) -> bool`.
  - A transform of a pen stroke scales its points into the new bounds.
  - Undo granularity: one step per add, per whole transform gesture (begin→end), per colour/size change, per delete. Redo cleared by any new step.
  - `pub fn press_selects(tool: Tool, hit: Option<&Annotation>) -> bool` — true for `Select` on any hit, and for a drawing tool when the hit annotation's `shape.tool() == tool` (spec: clicking with the tool that made it selects it).

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn the_topmost_annotation_is_hit() {}
#[test] fn a_pen_stroke_is_hit_near_its_line_only() {}
#[test] fn moving_then_undo_restores_the_position_in_one_step() {}
#[test] fn resizing_a_pen_stroke_scales_its_points() {}
#[test] fn shift_keeps_proportions_when_resizing() {}
#[test] fn a_new_step_clears_redo() {}
#[test] fn a_rect_tool_press_on_a_rect_selects_it_but_on_an_ellipse_does_not() {}
#[test] fn delete_then_undo_brings_it_back_with_the_same_id() {}
```
  Each with concrete coordinates; e.g. move by (10,5) → `bounds()` shifted by exactly (10,5).
- [ ] **Step 2–4:** `cargo test -p windup-core capture::annotate` FAIL → implement → PASS.
- [ ] **Step 5: Commit** `"Capture: annotations as objects with undo"`

---

### Task 8: Rasterising the result

**Files:**
- Create: `crates/windup-core/src/capture/raster.rs`; add `tiny-skia = "0.11"` to windup-core.

**Interfaces:**
- Consumes: Tasks 5, 7.
- Produces:
  - `pub fn render(shot: &DesktopShot, crop: DRect, doc: &Doc) -> Option<image::RgbaImage>` — `compose(crop)`, then, in list order: blur/mosaic regions are computed from the **composed original pixels** (not from earlier annotations, spec) and pasted; pen/rect/ellipse stroked anti-aliased with tiny-skia, round caps/joins. Desktop units map to output pixels by the composed scale. Annotations partly outside `crop` are clipped.
  - `pub fn blur_region(img: &mut RgbaImage, area: (u32, u32, u32, u32), radius: u32)` — three box-blur passes.
  - `pub fn mosaic_region(img: &mut RgbaImage, area, block: u32)` — block average.
  - `pub fn region_preview(shot: &DesktopShot, a: &Annotation) -> Option<RgbaImage>` — the blurred/mosaicked pixels of one region at the region's scale, for the UI to show while editing.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn a_rect_outline_paints_its_edge_not_its_inside() {}
#[test] fn mosaic_makes_each_block_one_colour() { /* checkerboard input, block 8 → every 8×8 block uniform */ }
#[test] fn blur_reads_the_original_not_a_rect_drawn_before_it() { /* red rect under a blur region: blurred output contains no red */ }
#[test] fn an_annotation_outside_the_crop_does_not_change_it() {}
#[test] fn strokes_scale_with_a_2x_monitor() { /* size 4 on a 2x monitor → ~8 px wide line */ }
```
- [ ] **Step 2–4:** `cargo test -p windup-core capture::raster` FAIL → implement → PASS.
- [ ] **Step 5: Commit** `"Capture: render annotations onto the stitched image"`

---

### Task 9: Picking windows, and the highlight tween

**Files:**
- Create: `crates/windup-core/src/capture/pick.rs`

**Interfaces:**
- Produces:
  - `pub struct WindowRect { pub rect: DRect, pub title: String }` (z-order: index 0 is topmost).
  - `pub fn window_at(rects: &[WindowRect], at: (f32, f32)) -> Option<DRect>` — first containing rect.
  - `pub fn edge_rect(shot: &DesktopShot, at: (f32, f32)) -> Option<DRect>` — the heuristic fallback: the smallest rectangle around `at` bounded by long straight luminance edges on all four sides, within the monitor under `at`; `None` if no closed rectangle found (the UI then highlights the monitor). Method is the builder's (spec "Left to the builder").
  - `pub struct Tween { from: DRect, to: DRect, started: f64 }`; `pub const TWEEN_SECS: f64 = 0.16;` `fn retarget(&mut self, to: DRect, now: f64)` (starts from the *current* interpolated rect); `fn at(&self, now: f64, reduced_motion: bool) -> DRect` — `ease_out_quint`-like curve matching `cubic-bezier(.22,1,.36,1)`; reduced motion returns `to`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn the_topmost_window_wins() {}
#[test] fn edges_find_a_drawn_window() { /* grey image with a 1-px dark outlined 300×200 box; at its centre → that box ±1 */ }
#[test] fn edges_give_none_on_a_flat_image() {}
#[test] fn tween_ends_at_the_target_and_starts_where_it_was() { /* at(started)==from, at(started+0.16)==to, retarget mid-way starts from the midpoint value */ }
#[test] fn reduced_motion_jumps() {}
```
- [ ] **Step 2–4:** `cargo test -p windup-core capture::pick` FAIL → implement → PASS.
- [ ] **Step 5: Commit** `"Capture: window picking, edge fallback, highlight tween"`

---

### Task 10: Output: names, folders, encoding

**Files:**
- Create: `crates/windup-core/src/capture/output.rs`

**Interfaces:**
- Consumes: Task 1 `ScreenshotConfig`, `ImageFormat`.
- Produces:
  - `pub fn file_name(pattern: &str, when: jiff::Zoned, format: ImageFormat) -> String` — `{date}` → `YYYY-MM-DD`, `{time}` → `HHMMSS`; characters illegal on Windows (`<>:"/\|?*`) replaced with `-`; extension `.png` / `.jpg`.
  - `pub fn unique_path(dir: &Path, name: &str) -> PathBuf` — appends ` (2)`, ` (3)` if taken.
  - `pub fn default_dir() -> Option<PathBuf>` — `directories::UserDirs::picture_dir()/Windup`.
  - `pub fn encode(img: &RgbaImage, format: ImageFormat) -> Vec<u8>` — JPEG flattens transparency onto black, quality 90.

- [ ] **Step 1: Write the failing tests:** `file_name("Screenshot {date} {time}", 2026-10-28T17:47:12, Png) == "Screenshot 2026-10-28 174712.png"`; a pattern with `a:b` → `a-b`; `unique_path` with an existing file → ` (2)`; JPEG of a transparent pixel decodes to black.
- [ ] **Step 2–4:** run FAIL → implement → PASS.
- [ ] **Step 5: Commit** `"Capture: file names and encoding"`

---

### Task 11: Tokens, icons and the marching ants

**Files:**
- Modify: `crates/windup-ui/src/theme.rs`, `crates/windup-ui/src/icons.rs`
- Create: `crates/windup-ui/src/capture/{mod.rs,ants.rs}`

**Interfaces:**
- Produces:
  - `Palette::accent_underlay(&self) -> Color32` — derived, as Umber's `../Umber/crates/umber-app/src/theme.rs:1241` (not a stored theme token, so `themelib.rs` is unchanged).
  - `metrics::ANT_DASH = 4.0`, `metrics::ANT_SPEED = 8.0`, `metrics::ANT_FRAME_MS: u64 = 60`, `metrics::DIM_ALPHA = 0.6`, `metrics::TWEEN_MS = 160.0`, `metrics::LOUPE = 112.0`, `metrics::LOUPE_ZOOM = 8.0`.
  - `Icon` variants, path data **verbatim from lucide.dev, in its element order**: `Camera`, `Video`, `Footprints`, `SquareDashed`, `AppWindow`, `Monitor`, `MousePointer2`, `Brush`, `Square`, `Circle`, `Grid3x3`, `Pipette`, `Undo2`, `Redo2`, `Download`, `Copy`, `X` (reuse if present), `ChevronDown` (reuse if present), and a blur icon of the builder's choosing. Add all to `Icon::ALL`; render the icon sheet test (if the repo has one) and look at it for collisions (style guide §11).
  - `pub fn ants(painter: &Painter, ring: &[Pos2], closed: bool, p: &Palette, time: f64, ppp: f32)` — port of Umber's `selection_outline` drawing: underlay solid, accent dashes at phase `period - (time * ANT_SPEED) mod period` (computed in f64), width `ant_width(ppp)`. Caller requests a repaint after `ANT_FRAME_MS`.
- [ ] **Step 1: Write the failing test** `fn ant_width_is_whole_device_pixels()` — at ppp 1.0, 1.25, 1.5, 1.75, 2.0: `(ant_width(ppp) * ppp).fract() == 0.0` and ≥ 1 device pixel.
- [ ] **Step 2–4:** `cargo test -p windup-ui ant_width` FAIL → implement → PASS.
- [ ] **Step 5: Commit** `"UI: capture tokens, icons, marching ants"`

---

### Task 12: Colour picker from Umber

**Files:**
- Create: `crates/windup-ui/src/colour.rs`

**Interfaces:**
- Produces: `pub struct Hsv { h: f32, s: f32, v: f32 }` with `to_rgb`/`from_rgb` (hue kept when s or v is 0, as Umber's doc comment requires); `pub fn wheel(ui: &mut Ui, p: &Palette, hsv: &mut Hsv, size: f32) -> bool` — Umber's Wheel mode (`../Umber/crates/umber-app/src/colorpicker.rs`), with its `Hub` / `settle` / `frame` gesture rules intact. Port only what Wheel needs; keep the module doc's reasoning as comments. Same licence (GPL-3.0-or-later).
- [ ] **Step 1:** port Umber's tests for the wheel's hit geometry and gesture settling that apply to Wheel mode; add `fn grey_keeps_its_hue()`.
- [ ] **Step 2–4:** `cargo test -p windup-ui colour` FAIL → port → PASS.
- [ ] **Step 5: Commit** `"UI: colour wheel ported from Umber"`

---

### Task 13: Choosing the capture back end

**Files:**
- Create: `crates/windup-core/src/capture/backend.rs`

**Interfaces:**
- Produces:
  - `pub enum Still { Gdi, X11, ExtImageCopy, WlrScreencopy, KWin, Portal, None }`.
  - `pub enum Rects { Win32, X11, Sway, Hyprland, KWinScript, Edges }`.
  - `pub struct Facts { pub windows: bool, pub x11_only: bool, pub globals: Vec<String>, pub desktop: String /* XDG_CURRENT_DESKTOP */, pub swaysock: bool, pub hyprland: bool, pub portal_screenshot: bool }`.
  - `pub fn still_order(f: &Facts) -> Vec<Still>` — the order to *try*: Windows `[Gdi]`; X11 `[X11]`; Wayland: `ExtImageCopy` if `ext_image_copy_capture_manager_v1` + `ext_output_image_capture_source_manager_v1`, `WlrScreencopy` if `zwlr_screencopy_manager_v1`, `KWin` if desktop contains `KDE`, `Portal` if available; empty means `[None]`.
  - `pub fn rects(f: &Facts) -> Rects` — Windows `Win32`; X11 `X11`; `Sway` if `SWAYSOCK`; `Hyprland` if `HYPRLAND_INSTANCE_SIGNATURE`; `KWinScript` on KDE; else `Edges`.
- [ ] **Step 1: Write the failing tests:** COSMIC globals → `[ExtImageCopy, Portal]` when portal present; Sway → `[ExtImageCopy?, WlrScreencopy, …]` per globals; KDE without screencopy → `[KWin, Portal]`; GNOME → `[Portal]`; Weston (no portal, no globals) → `[None]`; rects for each.
- [ ] **Step 2–4:** run FAIL → implement → PASS.
- [ ] **Step 5: Commit** `"Capture: back-end choice"`

---

### Task 14: Grabbing on Windows and X11, and `--capture-test`

**Files:**
- Create: `crates/windup-desktop/src/capture/{mod.rs,windows.rs,x11.rs}`
- Modify: `crates/windup-core/src/ipc.rs` (`Launch::CaptureTest(PathBuf)`, flag `--capture-test <file.png>`, hidden from `USAGE`), `crates/windup-desktop/src/main.rs`, `crates/windup-desktop/Cargo.toml` (windows-sys features as needed, e.g. `Win32_Graphics_Gdi` already present, `Win32_UI_HiDpi`)

**Interfaces:**
- Consumes: Task 5 `DesktopShot`, Task 13 `still_order`, Task 9 `WindowRect`.
- Produces:
  - `pub enum CaptureError { Denied, Unsupported, Failed(String) }` with `fn message(&self) -> &'static str` user copy: Denied → "Windup was not allowed to read the screen. Allow it in the dialog, or in your desktop's privacy settings."; Unsupported → "This desktop does not let applications read the screen."; Failed → "The screen could not be captured.".
  - `pub fn grab_all() -> Result<DesktopShot, CaptureError>` — tries `still_order` in turn; first success wins.
  - `pub fn window_rects(shot: &DesktopShot) -> Vec<WindowRect>`.
  - Windows: per monitor (`EnumDisplayMonitors`, `GetMonitorInfoW`), `BitBlt` with `CAPTUREBLT` from the screen DC into a DIB section, BGRA→RGBA; layout in virtual-screen physical pixels (process is per-monitor DPI aware — check `build.rs`/manifest; if not, set it). Window rects: `EnumWindows` (z-order), visible, not cloaked (`DWMWA_CLOAKED`), not minimised, `DWMWA_EXTENDED_FRAME_BOUNDS`, skip Windup's own windows.
  - X11: RandR monitors; `GetImage` ZPixmap of root per monitor; rects from `_NET_CLIENT_LIST_STACKING` (reversed: topmost first), frame via `_NET_FRAME_EXTENTS`.
  - `windup --capture-test out.png` runs `grab_all`, writes `compose(bounds)` to the file, exits 0, or prints the error and exits 1.
- [ ] **Step 1: Verify on Windows:** `cargo run -- --capture-test %TEMP%\all.png` — a PNG of every monitor, correct layout, sharp on a 150 % monitor.
- [ ] **Step 2: Verify X11** in CI later (Task 18).
- [ ] **Step 3:** clippy (Windows + Linux cross) clean.
- [ ] **Step 4: Commit** `"Capture: grab on Windows and X11, --capture-test"`

---

### Task 15: Wayland screencopy (COSMIC, wlroots)

**Files:**
- Create: `crates/windup-desktop/src/linux/wayland/screencopy.rs`; register in `linux/wayland/mod.rs`

**Interfaces:**
- Produces: `pub fn grab(conn: &Connection, globals: &GlobalList) -> Result<DesktopShot, CaptureError>` on its **own** short-lived event queue (so it works from the background loop and in `--capture-test` without the service). Outputs and logical layout from `xdg_output` (sctk `OutputState` gives `logical_position`/`logical_size`). Per output: `ext_image_copy_capture` with an output source, shm buffer in the format the session advertises (accept `Argb8888`/`Xrgb8888`/`Abgr8888`/`Xbgr8888`, convert to RGBA), one frame; else `zwlr_screencopy_manager_v1.capture_output(0, output)`. Respect output transform. `overlay_cursor = 0`.
- [ ] **Step 1:** unit-test the pixel conversion and transform handling as pure functions (`fn to_rgba(format, stride, bytes) -> RgbaImage`, `fn untransform(img, wl_output::Transform) -> RgbaImage`).
- [ ] **Step 2:** `cargo test -p windup-desktop screencopy` (on Linux/WSL/CI) — PASS.
- [ ] **Step 3:** e2e on headless Sway in Task 18.
- [ ] **Step 4: Commit** `"Wayland: still capture via ext-image-copy-capture and wlr-screencopy"`

---

### Task 16: KDE and GNOME stills

**Files:**
- Create: `crates/windup-desktop/src/linux/kde.rs`
- Modify: `crates/windup-desktop/src/linux/portal.rs`, `packaging/linux/` desktop entry (`X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2`), `packaging/check.sh` if it validates keys

**Interfaces:**
- Produces:
  - `kde::grab() -> Result<DesktopShot, CaptureError>` — `org.kde.KWin.ScreenShot2.CaptureScreen` per screen over zbus (blocking via async-std like `portal.rs`), reading the pipe; layout from the method's returned metadata or xdg-output. Permission refused → `Denied`.
  - `kde::window_rects() -> Vec<WindowRect>` — load a small KWin script over D-Bus (`org.kde.kwin.Scripting.loadScript`) that sends `workspace.stackingOrder` frame geometries back to Windup over a D-Bus method Windup exports; unload after.
  - `portal::screenshot() -> Result<DesktopShot, CaptureError>` — ashpd `Screenshot` with `interactive(false)`; the returned image covers the whole desktop; split it into `MonitorShot`s by the xdg-output logical rects, scale = image width / union width.
- [ ] **Step 1:** unit-test the portal split as a pure function `fn split_desktop(img: RgbaImage, layout: &[(String, DRect)]) -> DesktopShot` (two monitors side by side, one 2× image).
- [ ] **Step 2:** `cargo test -p windup-desktop split_desktop` — PASS.
- [ ] **Step 3: Owner check** on KDE/GNOME (no CI for these): `windup --capture-test` — note in the commit message that it is untested in CI.
- [ ] **Step 4: Commit** `"KDE and GNOME: still capture"`

---

### Task 17: The capture view (launcher, targets, loupe, editor)

**Files:**
- Create: `crates/windup-ui/src/capture/{bar.rs,loupe.rs}`, fill `crates/windup-ui/src/capture/mod.rs`
- Create: `crates/windup-core/src/capture/session.rs` (pure state machine)

**Interfaces:**
- Consumes: Tasks 5–12.
- Produces (core):
  - `pub enum CaptureMode { Screenshot, Video, Steps }` (`Video`/`Steps` not selectable in phase 1; named so it does not clash with `theme::Mode`).
  - `pub enum Stage { Choosing, Editing }`.
  - `pub enum Outcome { Copy(RgbaImage), Save(RgbaImage), CopyAndSave(RgbaImage), Cancel }`.
  - `pub struct Session { pub shot: DesktopShot, pub windows: Vec<WindowRect>, pub mode: CaptureMode, pub target: TargetKind, pub stage: Stage, pub selection: Selection, pub doc: Doc, pub tool: Tool, pub colour: [u8;4], pub sizes: ToolSizes, pub loupe: bool, pub hover: Option<Tween>, .. }`; `Session::new(shot, windows, &ScreenshotConfig) -> Session`; `fn finish(&self, copy: bool, save: bool) -> Outcome` (uses `raster::render`).
- Produces (ui):
  - `pub struct CaptureView { .. }`; `pub fn monitor_ui(&mut self, ui: &mut Ui, session: &mut Session, monitor: usize, p: &Palette, keys: &EditorKeys) -> Option<Outcome>` — draws the frozen monitor image (texture cached per monitor), the dim (`backdrop` at `DIM_ALPHA`) outside the selection, ants, size chip (mono, `popover`), handles, window highlight (tweened), full-screen hover, loupe, annotations (blur/mosaic from `region_preview` textures), and the bar **only on the monitor under the pointer**. Pointer positions converted to desktop units with the monitor's `layout` and the surface's size. Returns `Some` once.
  - Keys: Esc → `Cancel` always (checked before anything else, not configurable); Enter/`keys.copy` → copy (and save if `also_save_on_copy`); `keys.save` → save; undo/redo; `keys.loupe` toggles the loupe; `keys.select_all` → `select_all` in Area; arrows nudge (Shift = 10); Delete removes the selected annotation; mouse wheel changes the active tool's size (or the selected annotation's), shown as a mono figure by the pointer for a moment; Shift while drawing a rectangle or ellipse constrains it to a square or circle; the pipette sets `colour` from the frozen pixel under the pointer and returns to the previous tool.
  - Bar (option C): segmented icon-only modes (Camera / Video / Footprints) with the selected one `control_active` + accent icon; Video/Steps drawn disabled at 45 % with tooltip "Arrives in a later version."; target dropdown (SquareDashed "Area", AppWindow "Window", Monitor "Full screen"); settings and close icon buttons; floating panel styling (`chrome` 96 %, radius `RADIUS_MODAL`, `shadow::floating`, `line_popover`). In `Editing`, the same bar shows: tools (MousePointer2, Brush, Square, Circle, blur, Grid3x3, Pipette), colour swatch (opens `colour::wheel` in a popover), Undo2, Download (save), primary **Copy** button, X.
  - Stage changes: Area → `Editing` after the first release with a rect; Window → click sets rect from the highlight then `Editing`; Full screen → click sets the monitor's layout then `Editing`.
- [ ] **Step 1: Write the failing core tests** for `Session`: Esc in any stage → `Cancel`; Enter with `also_save_on_copy` → `CopyAndSave`; Window click uses the hovered rect; Full screen click on monitor 1 selects monitor 1's layout; changing the selection after drawing leaves annotation positions unchanged (spec: pinned to the screen).
- [ ] **Step 2–4:** `cargo test -p windup-core capture::session` FAIL → implement → PASS; then the UI with an example `cargo run -p windup-ui --example capture-view` that builds a `Session` from two fake monitors (one at negative x, one 2×) and shows both slices side by side in one window, for looking at.
- [ ] **Step 5: Commit** `"Capture: session and the capture view"`

---

### Task 18: Overlay on Windows and X11

**Files:**
- Create: `crates/windup-desktop/src/overlay.rs`
- Modify: `crates/windup-desktop/src/main.rs`

**Interfaces:**
- Consumes: Task 14 `grab_all`, `window_rects`; Task 17 `Session`, `CaptureView`.
- Produces: `WindupApp.capture: Option<(Session, CaptureView)>`; `fn start_capture(&mut self, ctx, mode: CaptureMode)` on `HotkeyEvent::Capture` (launcher) and `Screenshot` (straight to `last_target` selection); `fn drive_capture(&mut self, ctx)` called from `update`.
- [ ] **Step 1:** `start_capture`: `grab_all()` **before** any Windup window appears; on error show `CaptureError::message` in a small floating card at the cursor with an OK button (the same viewport machinery as the clipboard popup; Enter and Esc close it) and return. One `show_viewport_immediate` per monitor: `ViewportId::from_hash_of(("capture", i))`, borderless, always-on-top, no taskbar, position/size = monitor layout converted to that monitor's points, invisible on the first frame then visible after setup (reuse the popup's flash-avoidance approach in `main.rs`), focus the monitor under the cursor. Windows: exclude nothing (screenshots are frozen anyway).
- [ ] **Step 2:** On `Outcome`: close viewports first, then Copy → set the clipboard image (`clipboard-rs` on Windows/X11, as `paste.rs` does for entries), Save → `output::encode` + `unique_path` in `save_dir.or(default_dir())`, creating the folder; a failed write shows its error in the same card as Step 1; persist `last_target`, `colour`, `sizes` via `Config::save`.
- [ ] **Step 3: Verify manually on Windows** with two monitors at different scales: Win+Shift+S → bar on the pointer's monitor; Area across both → chip shows the 2× pixel size; draw a rect, move the selection → rect stays put; Enter → paste into Paint shows it; Esc at every stage cancels; Print Screen goes straight to the last target. Window mode glides between windows.
- [ ] **Step 4:** `tools/linux-e2e.sh` gains an `x11` capture check: `windup --capture-test` on Xvfb produces a PNG of the Xvfb size (CI `e2e` job).
- [ ] **Step 5: Commit** `"Capture: overlay on Windows and X11"`

---

### Task 19: Overlay on Wayland (layer host and window host)

**Files:**
- Create: `crates/windup-desktop/src/linux/wayland/overlay.rs`
- Modify: `crates/windup-desktop/src/linux/wayland/mod.rs` (`Request::OpenCapture(Box<Session>)`), `crates/windup-desktop/src/linux/mod.rs` (`start_capture`), `crates/windup-desktop/src/platform.rs` (`Plan::needs_wayland` true for the window host when capture is enabled, or start the service for capture only)

**Interfaces:**
- Consumes: Tasks 15, 16, 17; the existing `Gpu`, `egui_key`, `egui_button`, fractional-scale and viewporter handling in `popup.rs`/`mod.rs`.
- Produces: `enum Role { Layer, Fullscreen }`; one surface per output — layer host: `zwlr_layer_shell` overlay layer, anchored to all edges, exclusive zone -1, keyboard interactivity exclusive; window host: `xdg_toplevel.set_fullscreen(Some(output))`. Each surface has its own egui context sharing the one `Session`; input converted to desktop units with the output's logical rect. `ServiceEvent::CaptureDone(Outcome)` back to the background loop, which copies (`linux::set_entry` with an image entry, `paste: false`) and/or saves.
- [ ] **Step 1:** `linux::start_capture(mode)`: grab with the `still_order` back ends (screencopy uses the service's connection when it runs, else its own), then `Request::OpenCapture`. On error, show `CaptureError::message` in a card the way each host shows the clipboard popup (layer card; its own small window on the window host), with OK; Enter and Esc close it.
- [ ] **Step 2:** e2e: headless Sway: `windup --capture-test` writes a PNG the size of the headless output (exercises Task 15); `windup --screenshot` then `wtype` Escape (started before, as the existing e2e does for the popup) → overlay closes, process alive; Weston: `--capture-test` exits 1 with the Unsupported message.
- [ ] **Step 3: Owner check** on COSMIC: Super+Shift+S and Print Screen (bound by `cosmic.rs` from Task 4), Area across two outputs, Enter → image in Windup's clipboard history.
- [ ] **Step 4: Commit** `"Wayland: capture overlay on the layer and window hosts"`

---

### Task 20: Window rectangles on Sway, Hyprland, KDE

**Files:**
- Create: `crates/windup-desktop/src/capture/ipcrects.rs`
- Modify: `crates/windup-desktop/src/capture/mod.rs` (`window_rects` dispatch on Task 13 `rects()`), `crates/windup-desktop/src/linux/kde.rs`

**Interfaces:**
- Produces: `fn sway_rects(json: &str) -> Vec<WindowRect>` and `fn hyprland_rects(json: &str) -> Vec<WindowRect>` — pure parsers (visible, on a visible workspace, floating above tiled, focused first) tested on recorded JSON fixtures in `crates/windup-desktop/tests/fixtures/`; the socket I/O around them is thin. `Edges` means `window_rects` returns empty and the view calls `pick::edge_rect` per hover.
- [ ] **Step 1:** fixtures + failing parser tests (two tiled windows and one floating: floating first; a hidden workspace's windows absent).
- [ ] **Step 2–4:** `cargo test -p windup-desktop ipcrects` FAIL → implement → PASS.
- [ ] **Step 5: Commit** `"Capture: window rectangles from Sway, Hyprland and KWin"`

---

### Task 21: Settings panes, docs, changelog

**Files:**
- Create: `crates/windup-ui/src/settings/capture_cfg.rs`, `screenshot_cfg.rs`
- Modify: `crates/windup-ui/src/settings/{mod.rs,overview.rs}`, `docs/linux.md`, `README.md` (feature list; no screenshots, see CLAUDE.md "Missing pictures"), `CHANGELOG.md` (unreleased section only; do not bump the version), `CLAUDE.md` ("Adding a tool" no longer has the hotkey gap; describe `hotkeys.rs` and `capture/`)

**Interfaces:**
- Produces: `SettingsPane::Capture` (icon `Camera`, "Capture launcher") and `SettingsPane::Screenshot` (icon `SquareDashed`). Rows: shortcut (with a **caution keycap** when `hotkeys::clashes` contains the tool — wording "Also used by Capture"), loupe starts on, save folder (rfd picker), name pattern, format (segmented PNG/JPEG), also save when copying, editor keys (each a `shortcut_recorder`; Esc shown as a fixed, non-editable keycap with the note "Always cancels."). On Wayland, a note under the shortcut rows: "Bind windup --capture and windup --screenshot in your desktop's keyboard settings." when the plan's hotkeys are `Command` and the desktop is not COSMIC.
- [ ] **Step 1:** test in `settings/mod.rs`: `SettingsPane::ALL` contains both new panes and each has a non-empty label and blurb ending in a full stop.
- [ ] **Step 2–4:** `cargo test -p windup-ui settings` FAIL → implement → PASS; `cargo test --workspace` PASS (including `tests/release.rs`).
- [ ] **Step 5: Commit** `"Settings: capture and screenshot panes; docs"`
