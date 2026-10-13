# Linux support — design

2026-10-12

## Intent

Every Windup function works on Linux: clipboard history behind a hotkey, a
popup at the cursor, pasting the picked entry into the window that was active,
and paste-as-plain-text. The owner's own machine — CachyOS, COSMIC 1.9 on
Wayland — is the first-class target and is what "working" is measured on.
Every other distribution, desktop and window manager gets the best behaviour
its compositor allows, and an honest message where it allows nothing.

What the owner said: all functions, all distributions, desktops and window
managers. What was decided with them: COSMIC first; the popup appears at the
mouse cursor as on Windows; password-manager entries are not recorded.

## What the owner's machine offers

Measured with `wayland-info` and `busctl` on COSMIC 1.9.0:

- `ext_data_control_manager_v1`, `zwlr_data_control_manager_v1`
- `zwp_virtual_keyboard_manager_v1`
- `zwlr_layer_shell_v1`
- `zcosmic_toplevel_info_v1`, `zcosmic_toplevel_manager_v1`
- the RemoteDesktop portal, but **no** GlobalShortcuts portal
- `/dev/uinput` not writable by the user (and not wanted)

## Backends

Chosen at runtime, because one binary runs on X11 and on Wayland.

| | Wayland, full (COSMIC, KDE, Sway, Hyprland, river, Wayfire…) | Wayland, GNOME (no data-control, no layer-shell) | X11, any WM |
|---|---|---|---|
| Hotkey | `windup --popup` bound in the desktop's shortcut settings, delivered over a socket; GlobalShortcuts portal where present | GlobalShortcuts portal (GNOME 48+), else the bound command | `XGrabKey` (`global-hotkey`), and the command |
| Watch clipboard | data-control (ext, else wlr), event-driven | XWayland selection polling, best effort | polling (`clipboard-rs`) |
| Set clipboard | data-control source | XWayland / `clipboard-rs` | `clipboard-rs` |
| Popup | full-output transparent layer surface on the overlay layer, card drawn at the pointer | ordinary egui window of its own per popup (no hidden window between: Wayland cannot hide one, and a window never shown never draws), placed by the compositor | egui window at the cursor (`XQueryPointer`, `_NET_WORKAREA`) |
| Focus target | `zcosmic_toplevel_info` activated toplevel; elsewhere focus returns when the layer surface unmaps | compositor restores focus | `_NET_ACTIVE_WINDOW`, restored with an EWMH client message |
| Ctrl+V | `zwp_virtual_keyboard_v1` | RemoteDesktop portal (one consent, restore token kept) | XTest (`enigo`) |

Detection is a pure function `platform::detect(env, globals) -> Backend` with
tests, fed by the environment (`WAYLAND_DISPLAY`, `DISPLAY`,
`XDG_SESSION_TYPE`) and the globals a short registry roundtrip reports.

## Components

**`windup-core`** gains pure, tested pieces only:

- `ipc` — the line protocol (`popup`, `paste-plain`, `show`), socket path
  resolution under `$XDG_RUNTIME_DIR`, and CLI argument parsing.
- `placement` — popup placement from cursor + work area + size, with the
  flip-left / flip-up rules currently inline in `main.rs`. Windows uses it too.
- `clipboard::capture` — the dedupe that the polling monitor does today,
  lifted into an `Ingest` value both the poller and the data-control listener
  feed, plus MIME selection (image over text, `text/plain;charset=utf-8` over
  `UTF8_STRING` over `text/plain`) and the password-manager rule:
  `x-kde-passwordManagerHint` with content `secret` means the offer is skipped.
  History semantics are unchanged: a new copy inserts at the front and never
  reorders.

**`windup-desktop`**:

- `platform/` — `detect`, the `Target` type that replaces `target_hwnd:
  isize` (an HWND on Windows, an X11 window, or a Wayland toplevel handle id),
  and the backend trait objects.
- `ipc_server` — the Unix socket listener; one connection carries one command
  and becomes a `HotkeyEvent`. Also single-instance: a second `windup` with no
  arguments sends `show` and exits.
- `wayland/` — one thread, one connection: data-control device (watch and
  serve), virtual keyboard, toplevel tracking, and the popup layer surface
  rendered with `egui-wgpu` and drawn by the existing `windup_ui::PopupView`.
- `x11` — pointer, work area, active window, and activation over `x11rb`.
- `portal` — GlobalShortcuts and RemoteDesktop via `ashpd`, used only when the
  compositor lacks the protocol route.
- `autostart` — writes or removes `~/.config/autostart/<app-id>.desktop` from
  `Config::start_at_login` on Linux.

## Popup on Wayland

A layer surface anchored to all four edges, exclusive keyboard interactivity,
overlay layer, no exclusive zone, transparent. It covers the output the
compositor puts it on (the focused one). The pointer `enter` event gives the
cursor position in surface coordinates; the card is placed with
`placement::place`, using the surface size as the work area. If no `enter`
arrives within 120 ms the card is centred. A click outside the card, Escape,
or losing keyboard focus dismisses. The surface is destroyed on close so the
compositor hands focus back.

## Paste on Wayland

Set our data-control source, activate the recorded toplevel when the
compositor supports it, wait 60 ms, then send Ctrl down, V, Ctrl up through a
virtual keyboard with a keymap containing only those keys and explicit
modifier state, so a Super still physically held does not leak into the
target's view. Our own source echoes back as a selection event; `Ingest`
treats it exactly as the Windows poller treats our own `SetClipboardData`.

## Failure behaviour

Every backend decision is logged at `info`. When a function cannot work on
this session — no data-control and no XWayland, no virtual keyboard and the
portal refused — the settings window's Overview shows one sentence saying
which function and why, rather than failing silently.

## Packaging

`.deb`, `.rpm`, AppImage and an Arch `PKGBUILD`, built by
`packaging/linux/package.sh` after Muster's layout, with the desktop entry,
AppStream metadata and hicolor icons under the app id
`io.github.spillebulle.windup`. The `.deb` and `.rpm` enrol the machine in the
house archive (STYLE-GUIDE §18.2). Built on Ubuntu 22.04 for glibc 2.35.
No Flatpak: the sandbox hides data-control and the virtual keyboard from
sandboxed clients by design (security-context-v1), which removes the app's
whole purpose.

## Testing

- Unit tests for every pure piece in core and for `platform::detect`.
- Cross type-check for `x86_64-unknown-linux-gnu` from Windows during work.
- CI on Ubuntu: fmt, clippy, tests, and an end-to-end job under headless Sway
  (layer-shell, ext/wlr data-control, virtual keyboard): copy with `wl-copy`,
  assert history; `windup --popup`, screenshot with `grim`, pick with a key,
  assert the text arrived in a GTK entry; paste-plain likewise. And an X11 job
  under Xvfb.
- On the owner's COSMIC machine: the checklist in `docs/linux.md`.
