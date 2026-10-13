# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Windup is a desktop utility for Windows and Linux (Rust + egui/eframe/wgpu) that runs a settings window plus a set of global-hotkey "tools". Two tools exist today: **Clipboard history** (Win+V → floating popup at the cursor) and **Paste plain text** (Win+Shift+V → strips formatting and pastes).

## Commands

```bash
cargo run                    # runs windup-desktop (workspace default-member)
cargo run --release
cargo build --release        # -> target/release/windup.exe
cargo test --workspace
cargo test -p windup-core capacity_limit_truncates_oldest_items   # single test
cargo clippy --workspace --all-targets
cargo fmt
cargo run -p windup-ui --example make-art          # regenerate windup.ico, Linux hicolor PNGs + WiX bitmaps, then commit them
sh packaging/windows/package.sh 0.1.0 x64 target/release dist   # MSI + setup exe (needs WiX 5)
sh packaging/linux/package.sh 0.1.0 amd64 target/release dist   # deb, rpm, Arch, AppImage (Linux)
sh packaging/check.sh                              # Linux app id consistency
bash tools/linux-e2e.sh target/debug/windup all    # real end-to-end on headless Sway + Xvfb (Linux)
pwsh tools/release.ps1 X.Y.Z                       # cut a release (see docs/packaging.md)
```

CI (`.github/workflows/ci.yml`) runs fmt, clippy and tests with `RUSTFLAGS=-D warnings` on Windows x64 and Linux, and builds Windows ARM64. Keep all three warning-free; Linux-only unused variables from `cfg(windows)` blocks are the usual way to break it.

Debug builds keep a console window (`windows_subsystem = "windows"` is release-only), so `RUST_LOG=debug cargo run` is the way to see logs. Default filter is `warn,wgpu_hal=error,wgpu_core=error`.

Rust edition 2024, MSRV 1.90. `let`-chains (`if let Some(x) = a && let Some(y) = b`) are used throughout — that is edition-2024 syntax, not a mistake.

## Crate layout

- **`windup-core`** — no UI, no windowing. Clipboard capture/history/persistence, `Config`, `Shortcut`, `ToolId`, and `install` (the Windows setup model and payload format).
- **`windup-ui`** — all egui drawing. Depends on core, never on desktop. Knows nothing about Win32.
- **`windup-desktop`** — the binary. Owns the eframe app, Win32 hooks, window focus/paste plumbing, and wires core ↔ ui together.

Win32 code (`windows-sys`) lives in `windup-desktop` (plus a small amount in core's Cargo manifest). Every Win32 block is `#[cfg(target_os = "windows")]` with a no-op `#[cfg(not(...))]` twin — keep that pairing when adding platform code; the crates are expected to still compile on macOS (fallbacks use `global-hotkey`, `enigo`, `clipboard-rs`). Linux code is all under `windup-desktop/src/linux/` (`#[cfg(target_os = "linux")] mod linux`).

To type-check or clippy the Linux build from Windows: `cargo clippy --target x86_64-unknown-linux-gnu` works once `PKG_CONFIG` points at a script that prints `-lxkbcommon` and `PKG_CONFIG_ALLOW_CROSS=1` is set (sctk's build script insists on pkg-config). Tests still need Linux: CI, or WSL.

## Architecture notes worth knowing before editing

**Single event loop, two viewports.** `WindupApp::update` (`crates/windup-desktop/src/main.rs`) draws the settings window in a `CentralPanel`, and when `popup_open` is set it also drives the clipboard popup as an `egui::show_viewport_immediate` secondary viewport. The popup is a real borderless OS window: its position is computed from the cursor and monitor work area (`cursor.rs`), flipping left/up when it would overflow, and it is rounded + focused by `focus_and_clip_popup_window` via DWM + `SetWindowRgn` + `AttachThreadInput`. The first frame is created invisible and shown only after the region/DWM setup, to avoid a white flash. Don't "simplify" the visibility dance without testing the flash.

**Hotkeys are an interception, not a registration.** On Windows a `WH_KEYBOARD_LL` hook thread (`hotkey.rs`) watches for V with Win held (state read via `GetAsyncKeyState`, not the hook's own modifier state) and returns `1` to swallow the key, which is what stops the Start menu and the OS clipboard from appearing. It injects a dummy keyup (`0xFF`) so the shell doesn't treat Win as a bare press, and ignores injected keystrokes (`flags & 1`) so simulated pastes don't re-trigger it. Events reach the UI thread through an `mpsc` channel polled in `update`; the foreground HWND at press time is captured as `target_hwnd` so paste can restore focus later.

**Pasting is a simulated Ctrl+V** (`paste.rs`), on a spawned thread: restore foreground to `target_hwnd`, wait for Enter/Win/V to be physically released (300 ms cap), sleep 60 ms for the target's message loop, then `SendInput`. Timing constants here are load-bearing.

**Clipboard capture** is a 250 ms polling thread (`core/clipboard/monitor.rs`) — images take priority over text, deduped by a `DefaultHasher` hash / last-text comparison. History semantics are deliberate and covered by tests: re-copying an existing value **inserts a new entry at the front and never reorders** existing ones. Live settings reach that thread via `Arc<AtomicUsize>` / `Arc<AtomicBool>` mirrored from config every frame.

**Persistence** goes through `directories::ProjectDirs("com", "spillebulle", "windup")`: `config.json`, `clipboard_history.json` (images serialized as raw RGBA + thumbnail — large), and `themes/`. `Config::save()` is called eagerly by the setters, so settings changes persist immediately; history is written when `(len, newest_id)` changes.

## Linux

Design: `docs/superpowers/specs/2026-10-12-linux-support-design.md`; user-facing: `docs/linux.md`.

- **The backend is chosen at runtime** by `platform::detect(env, globals)`, a pure tested function of `WAYLAND_DISPLAY`/`DISPLAY` and the Wayland globals a registry roundtrip reports. The result is a `Plan { host, watch, paste, hotkeys }`.
- **Layer host** (layer-shell + data-control: COSMIC, KDE, wlroots): `linux::run_background` runs with **no winit at all**, because Wayland cannot hide a window. A single Wayland thread (`linux/wayland/`) owns data-control (watch + serve the selection), the virtual keyboard (Ctrl+V) and the popup, a full-output transparent overlay layer surface drawing `PopupView` through egui-wgpu; the card is placed where the pointer *enters* that surface (Wayland gives no global cursor position), centred after 150 ms otherwise. The settings window is a child process `windup --settings` (settings-only `WindupApp`, never writes history); the host reloads `config.json` when its mtime changes.
- **Window host** (Wayland without layer-shell: GNOME, Weston): the same windowless background process (`linux::run_background`), with the polling monitor reading the clipboard through XWayland and the popup as a window of its own per use (`linux/window.rs`, eframe `run_and_return`). Not a hidden eframe app: Wayland cannot hide a window, and one never shown never draws, so `update` would never run.
- **Viewport host** (X11): the eframe app as on Windows; pointer, work area and focus via `linux/x11.rs` (EWMH/RandR over x11rb). Closing the window hides it instead of quitting.
- **Hotkeys on Wayland are commands**: `windup --popup` / `--paste-plain` / `--show` connect to `$XDG_RUNTIME_DIR/windup-<display>.sock` (protocol in `windup_core::ipc`, plumbing in `linux/ipc.rs`), starting Windup in the background if needed. The same socket makes Windup single-instance. Portals (`linux/portal.rs`, ashpd on async-std, already in the tree via rfd) add GlobalShortcuts and RemoteDesktop paste where the compositor lacks the protocols.
- Clipboard ingest is shared: `windup_core::clipboard::capture::{Ingest, pick}`; offers carrying `x-kde-passwordManagerHint` are never recorded. Our own selection is tagged with `application/x-windup-owned` so its echo is recognised without reading our own pipe (which would deadlock the thread).
- `cosmic-protocols` is deliberately not used: it is GPL-3.0-only.
- Tested end to end by `tools/linux-e2e.sh` (CI job `e2e`): headless Sway exercises the wlr data-control, virtual-keyboard and layer-shell paths; headless Weston the window host; Xvfb + openbox X11. COSMIC uses ext-data-control, which only the owner's machine exercises. Sway re-sends keyboard focus to the focused window whenever a keyboard device appears, so keys for the popup come from a `wtype` started before it opens.
- sctk calls `new_seat` only for seats announced after startup; `adopt_seat` handles the ones already there. The virtual keyboard is created with the seat, not at the first paste.

## UI conventions

The UI is **painted, not composed from egui widgets** — nearly everything in `windup-ui/src/widgets/` allocates a rect and draws with `Painter`. Follow that style rather than reaching for stock `egui` widgets.

- `theme.rs` holds all design tokens: `metrics::*` (sizes/spacing), `text::*` (font sizes), and `Palette` (colors, built from OKLCH with a configurable accent hue). Never hardcode a size or color; add a token.
- UI follows `../Design-Principles/STYLE-GUIDE.md` (a private sibling repo, Spillebulle/Design-Principles); accent hue is `28`. Source comments cite its sections. The README, banner and repo settings follow its §17; redraw the banner with `python ../Design-Principles/banner.py --mark assets/icons/windup-256.png --name WINDUP --out docs/images`.
- Icons are Lucide path data transcribed verbatim into `icons.rs` and flattened to polylines by `lucide.rs`. To add an icon: paste Lucide's elements as `Node::Path`/`Circle`/`Rect` in the same order as lucide.dev, add the variant to `Icon::ALL`. No font, no SVG rasterizer. Lucide is ISC; the notice in `lucide.rs` must stay.
- The mark as pixels is `windup_ui::art::mark`. The window icon uses it at runtime and is regenerated whenever the palette accent changes; `examples/make-art.rs` uses it with the default dark palette to write `assets/icons/windup.ico` (compiled into the exe by `windup-desktop/build.rs`) and `packaging/windows/{banner,dialog}.bmp`. Change the mark → re-run make-art and commit.
- `themelib.rs` reads/writes `.umbertheme` files: first line `Umber theme`, then `name`, `base` (`paper` = light, else dark), then 27 `token = #rrggbb` lines. Adding a `Palette` field does not automatically add a theme token — `Token::ALL` and its `get`/`set` arms must be updated too.

## Adding a tool

A tool is a `ToolId` variant + a config struct + a settings pane + a hotkey branch. Touch points: `core/tool.rs` (`ALL`, `as_str`, `display_name`, `description`, `default_shortcut`), `core/config.rs` (config struct, `Config` field, `is_tool_enabled`/`set_tool_enabled`/`tool_shortcut`/`set_tool_shortcut`), `ui/settings/mod.rs` (`SettingsPane` variant + `ALL`/`label`/`icon`/`blurb` + `render_content` arm) plus a new pane module, and `desktop/hotkey.rs` + the `HotkeyEvent` match in `main.rs`.

Note the current gap: `Config` stores per-tool shortcuts, but the Windows hook hardcodes Win+V / Win+Shift+V and X11 grabs the same two. Configured shortcuts are not yet honored. On Wayland a tool also needs an `ipc::Command` and a `windup --flag`.

## Packaging and releases

Full write-up, including how to add an OS: `docs/packaging.md`. The short version:

- Linux: `.deb`/`.rpm`/Arch/AppImage from `packaging/linux/package.sh`, x86-64 only, built on Ubuntu 22.04; the app id `io.github.spillebulle.windup` must match across desktop entry, metainfo, icons, `main.rs` and `autostart.rs` (`packaging/check.sh`).
- A tag `vX.Y.Z` triggers `release.yml`. Each matrix row builds `--target <triple>` then calls `packaging/<package>/package.sh <version> <arch> <bin-dir> dist`; adding an OS is a row in both workflows plus that script. Platform logic stays in the script, not the YAML.
- Release notes are the `## X.Y.Z` section of `CHANGELOG.md` verbatim; the title is the bare version. `windup-desktop/tests/release.rs` fails if the changelog or the README download table does not match `Cargo.toml`'s version.
- **The setup exe is `windup.exe` with the MSI appended** (`windup_core::install::payload`). `main` checks the payload before anything else and runs `setup::run()` instead of the app, because a double-clicked file has no arguments. Setup runs `msiexec /qn` elevated via `ShellExecuteExW("runas")`, then starts the installed exe un-elevated. Pure decisions (`Step`, `Command`, paths) are in `windup_core::install`, the window in `windup_ui::setup`, Win32 in `windup-desktop/src/setup.rs`.
- `packaging/windows/windup.wxs`: the `UpgradeCode` and component GUIDs are Windup's own and pinned by tests in `windup_core::install`; never copy them from a sibling app. XML comments may not contain `--`.
- ARM64 Windows is cross-compiled on the x64 runner (private repo: no free ARM runners), so it is built but never tested.
- `*.sh` files are LF via `.gitattributes`; CI runs them with Git Bash on Windows.

## License

GPL-3.0-or-later.

## Missing pictures

The README has no screenshots yet. §17.1 wants a whole-window picture under the
sentence (`docs/images/window.png`) and §17.3 a cropped one beside each feature
(the clipboard popup, the paste-plain settings). They were not taken because the
MSI Afterburner / RTSS overlay draws itself into every graphics backend and runs
elevated; turn the OSD off before capturing. Do not capture the popup with real
clipboard history in it.
