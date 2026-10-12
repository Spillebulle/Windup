# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Windup is a Windows-first desktop utility (Rust + egui/eframe/wgpu) that runs a settings window plus a set of global-hotkey "tools". Two tools exist today: **Clipboard history** (Win+V → floating popup at the cursor) and **Paste plain text** (Win+Shift+V → strips formatting and pastes). There is no README.

## Commands

```bash
cargo run                    # runs windup-desktop (workspace default-member)
cargo run --release
cargo build --release        # -> target/release/windup.exe
cargo test --workspace       # tests live in windup-core::clipboard::history and windup-ui::lucide
cargo test -p windup-core capacity_limit_truncates_oldest_items   # single test
cargo clippy --workspace --all-targets
cargo fmt
```

Debug builds keep a console window (`windows_subsystem = "windows"` is release-only), so `RUST_LOG=debug cargo run` is the way to see logs. Default filter is `warn,wgpu_hal=error,wgpu_core=error`.

Rust edition 2024, MSRV 1.90. `let`-chains (`if let Some(x) = a && let Some(y) = b`) are used throughout — that is edition-2024 syntax, not a mistake.

## Crate layout

- **`windup-core`** — no UI, no windowing. Clipboard capture/history/persistence, `Config`, `Shortcut`, `ToolId`.
- **`windup-ui`** — all egui drawing. Depends on core, never on desktop. Knows nothing about Win32.
- **`windup-desktop`** — the binary. Owns the eframe app, Win32 hooks, window focus/paste plumbing, and wires core ↔ ui together.

Win32 code (`windows-sys`) lives in `windup-desktop` (plus a small amount in core's Cargo manifest). Every Win32 block is `#[cfg(target_os = "windows")]` with a no-op `#[cfg(not(...))]` twin — keep that pairing when adding platform code; the crates are expected to still compile on Linux/macOS (fallbacks use `global-hotkey`, `enigo`, `clipboard-rs`).

## Architecture notes worth knowing before editing

**Single event loop, two viewports.** `WindupApp::update` (`crates/windup-desktop/src/main.rs`) draws the settings window in a `CentralPanel`, and when `popup_open` is set it also drives the clipboard popup as an `egui::show_viewport_immediate` secondary viewport. The popup is a real borderless OS window: its position is computed from the cursor and monitor work area (`cursor.rs`), flipping left/up when it would overflow, and it is rounded + focused by `focus_and_clip_popup_window` via DWM + `SetWindowRgn` + `AttachThreadInput`. The first frame is created invisible and shown only after the region/DWM setup, to avoid a white flash. Don't "simplify" the visibility dance without testing the flash.

**Hotkeys are an interception, not a registration.** On Windows a `WH_KEYBOARD_LL` hook thread (`hotkey.rs`) watches for V with Win held (state read via `GetAsyncKeyState`, not the hook's own modifier state) and returns `1` to swallow the key, which is what stops the Start menu and the OS clipboard from appearing. It injects a dummy keyup (`0xFF`) so the shell doesn't treat Win as a bare press, and ignores injected keystrokes (`flags & 1`) so simulated pastes don't re-trigger it. Events reach the UI thread through an `mpsc` channel polled in `update`; the foreground HWND at press time is captured as `target_hwnd` so paste can restore focus later.

**Pasting is a simulated Ctrl+V** (`paste.rs`), on a spawned thread: restore foreground to `target_hwnd`, wait for Enter/Win/V to be physically released (300 ms cap), sleep 60 ms for the target's message loop, then `SendInput`. Timing constants here are load-bearing.

**Clipboard capture** is a 250 ms polling thread (`core/clipboard/monitor.rs`) — images take priority over text, deduped by a `DefaultHasher` hash / last-text comparison. History semantics are deliberate and covered by tests: re-copying an existing value **inserts a new entry at the front and never reorders** existing ones. Live settings reach that thread via `Arc<AtomicUsize>` / `Arc<AtomicBool>` mirrored from config every frame.

**Persistence** goes through `directories::ProjectDirs("com", "spillebulle", "windup")`: `config.json`, `clipboard_history.json` (images serialized as raw RGBA + thumbnail — large), and `themes/`. `Config::save()` is called eagerly by the setters, so settings changes persist immediately; history is written when `(len, newest_id)` changes.

## UI conventions

The UI is **painted, not composed from egui widgets** — nearly everything in `windup-ui/src/widgets/` allocates a rect and draws with `Painter`. Follow that style rather than reaching for stock `egui` widgets.

- `theme.rs` holds all design tokens: `metrics::*` (sizes/spacing), `text::*` (font sizes), and `Palette` (colors, built from OKLCH with a configurable accent hue). Never hardcode a size or color; add a token.
- UI follows `../Design-Principles/STYLE-GUIDE.md` (a private sibling repo, Spillebulle/Design-Principles); accent hue is `28`. Source comments cite its sections. The README, banner and repo settings follow its §17; redraw the banner with `python ../Design-Principles/banner.py --mark assets/icons/windup-256.png --name WINDUP --out docs/images`.
- Icons are Lucide path data transcribed verbatim into `icons.rs` and flattened to polylines by `lucide.rs`. To add an icon: paste Lucide's elements as `Node::Path`/`Circle`/`Rect` in the same order as lucide.dev, add the variant to `Icon::ALL`. No font, no SVG rasterizer. Lucide is ISC; the notice in `lucide.rs` must stay.
- The app icon is rasterized at runtime from the Lucide wrench (`create_app_icon` in `main.rs`) and regenerated whenever the palette accent changes.
- `themelib.rs` reads/writes `.umbertheme` files: first line `Umber theme`, then `name`, `base` (`paper` = light, else dark), then 27 `token = #rrggbb` lines. Adding a `Palette` field does not automatically add a theme token — `Token::ALL` and its `get`/`set` arms must be updated too.

## Adding a tool

A tool is a `ToolId` variant + a config struct + a settings pane + a hotkey branch. Touch points: `core/tool.rs` (`ALL`, `as_str`, `display_name`, `description`, `default_shortcut`), `core/config.rs` (config struct, `Config` field, `is_tool_enabled`/`set_tool_enabled`/`tool_shortcut`/`set_tool_shortcut`), `ui/settings/mod.rs` (`SettingsPane` variant + `ALL`/`label`/`icon`/`blurb` + `render_content` arm) plus a new pane module, and `desktop/hotkey.rs` + the `HotkeyEvent` match in `main.rs`.

Note the current gap: `Config` stores per-tool shortcuts, but the Windows hook hardcodes Win+V / Win+Shift+V. Configured shortcuts are not yet honored on Windows.

## License

GPL-3.0-or-later.
