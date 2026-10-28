# Capture: screenshots, video, steps and the live editor — design

2026-10-28

## Intent

Windup gains a capture suite in the spirit of Windows 11's Snipping Tool and
Flameshot: one hotkey brings up a small bar over a frozen, dimmed screen, and
from there the owner takes a screenshot, records a video or records a series of
steps. A screenshot is edited live, on the frozen screen, with a handful of
tools (pen, rectangle, ellipse, blur, mosaic, colour picker), and Enter puts it
on the clipboard.

**The owner's first priority is compatibility, and they defined it as the same
experience on every desktop Windup supports**: Windows, X11, and every Wayland
host (COSMIC, KDE, wlroots, GNOME). Not "the feature exists everywhere in some
form". The way that is reached: capture the screen once, by whatever means the
platform allows, and from then on Windup draws everything itself — the bar, the
selection, the editor, the recording controls — so what the owner sees and does
is identical everywhere. Only the capture underneath differs.

The one difference the owner accepted: GNOME and KDE (and sandboxes generally)
will not let an app read the screen until the user grants it once through the
system's portal dialog. A one-time grant is acceptable; a dialog on every use is
not, wherever a remembered grant can avoid it.

## Goals

- A **capture launcher**: a floating bar at the top of a frozen, dimmed screen.
- **Screenshot** of a full screen, an area, or a window.
- A **live editor** on the frozen screen, Flameshot-style.
- **Video** recording of the same three targets, with a recording bar.
- **Steps**: a click-by-click recorder that produces a numbered, exportable
  document. Deliberately small in v1; expected to grow.
- Every hotkey configurable — which means the existing gap (hooks hardcoded to
  Win+V / Win+Shift+V, CLAUDE.md "Adding a tool") is closed as part of this
  work, for the existing tools as well.

## Non-goals (v1)

- Text and arrow annotation tools. The owner listed the tools they want; these
  were not among them. *(My call; easy to add later.)*
- Uploading or sharing anywhere. Files go to disk and the clipboard.
- macOS. The crates must still compile there (the existing rule), but capture
  may be a no-op.

## Anti-goals

- A capture path that looks or behaves differently per desktop, beyond the
  one-time permission grant.
- A system picker dialog (the portal's "choose a window/screen" chooser,
  GNOME's own screenshot UI, `slurp`) appearing in normal use.
- A control that lies (style guide rule 7): an encoder, container or audio
  device that is offered but cannot work on this machine.

## The flow

```
hotkey ──► capture every monitor once (frozen) ──► overlay on every monitor
                                                     │
                       launcher bar (mode: Screenshot / Video / Steps,
                                     target: Area / Window / Full screen)
                                                     │
        Screenshot ─► select ─► live editor (bar becomes tool bar)
                                  Enter = copy, Ctrl+S = save, Esc = cancel
        Video ──────► select ─► recording bar (overlay gone, screen live)
        Steps ──────► recording bar ─► each click = a step ─► review list ─► export
```

Each mode also has its own hotkey that skips the launcher and goes straight to
selection with the last-used target.

## The launcher bar

Decided with the owner from mockups (option "C, accent icon"):

- A floating panel centred at the top of the monitor the pointer is on (style
  guide §5 floating panel: `chrome` at 96 %, blur, radius 10, `shadow-float`,
  `line-popover` edge).
- Left: one segmented box of three **icon-only** mode buttons — Screenshot
  (Lucide `camera`), Video (`video`), Steps (`footprints`). The selected one has
  the `control-active` fill and its icon in `accent` (§2.4 "the selected tool's
  icon"). Not an accent fill: the owner saw both and chose this one.
- Then a dropdown trigger for the target — Area (`square-dashed`), Window
  (`app-window`), Full screen (`monitor`) — with its icon and label.
- Then settings (`settings`) and close (`x`) icon buttons.
- Every icon button tooltipped, with its shortcut in a keycap.
- The rest of the screen is the frozen image dimmed with `backdrop` at 60 %.
- The bar **changes in place** for later stages: into the editor's tool bar
  after a screenshot selection, into the recording bar during video and steps.
  One place for everything, never in the way of the selection.

Mockups: `.superpowers/brainstorm/*/content/launcher-c2.html` and
`editor-placement.html` (not committed; regenerate from this text if lost).

## Screenshot targets

**Full screen.** Every monitor is highlighted on hover; a click picks one.

**Area.** Drag a rectangle.

- Outline: Umber's marching ants, ported from
  `../Umber/crates/umber-app/src/ui.rs` `selection_outline`: a solid line in an
  underlay colour beneath, accent dashes sliding on top, dash 4 / gap 4 pt,
  moving 8 pt/s, repainted at ~16 fps (`ANT_FRAME_MS = 60`), width rounded to
  whole device pixels (`ant_width`). Windup's `Palette` has no
  `accent_underlay`; add it as Umber defines it (the far end of the lightness
  axis from the accent), and add it to `themelib.rs` `Token::ALL` only if it is
  a stored token rather than derived (in Umber it is derived).
  The style guide's dash rhythm is 5 / 4; the owner asked for Umber's effect,
  so 4 / 4 stands, and the comment says so.
- Outside the selection stays dimmed; inside is the undimmed frozen image.
- A size readout as a mono chip (`612 × 284`) above the selection.
- Handles to resize and drag to move after the first drag.
- **Zoom loupe**: a magnified pixel grid near the pointer with a black-and-white
  crosshair (derived ink, §2.6) and the pointer's pixel position in mono. A
  hotkey toggles it while selecting; a setting chooses whether it starts on.
- Arrow keys nudge the active edge or the selection by one pixel. *(My call.)*
- **An area can cross monitors** (the owner: "very important"). See
  "Across monitors" below.

### Across monitors

The selection, the annotations and the result live in one **desktop
coordinate space** covering every monitor, not in one monitor's.

- The overlay is still one surface per monitor (each at its own scale), but
  they share one selection and one annotation list. A drag that leaves one
  monitor continues on the next; the ants, the size chip and the transform
  boxes are drawn across the seam, each monitor drawing its part.
- The desktop space is the platform's own layout: Windows' virtual-screen
  physical pixels; X11's root window; on Wayland the outputs' logical
  positions (`xdg-output`) scaled to pixels.
- **Mixed scale.** When the crossed monitors have different scales, the
  result is rendered at the **highest** scale among them and the others are
  scaled up into it, so nothing loses detail. *(My call.)* The size chip shows
  the result's pixel size.
- Gaps (monitors of different heights, an L-shaped layout) are **transparent**
  in a PNG and black in JPEG and video. *(My call.)*
- Video crossing monitors captures each monitor's stream and composites them
  into one frame before encoding. The recording-bar placement rule applies to
  the union of the monitors involved.
- Full screen still means one monitor; an "all monitors" choice is the area
  mode dragged across them, or Ctrl+A in Area mode, which selects the whole
  desktop. *(Ctrl+A is my call.)*

**Window.** Hovering highlights the window under the pointer; a click captures
that window's rectangle from the frozen image. When the pointer moves to
another window the highlight **animates** to the new rectangle: about 160 ms
with the house easing (`--ease`, §13), no bounce, and no animation when the OS
asks for reduced motion.

Where the window rectangles come from, per platform:

| Platform | Source | Precision |
|---|---|---|
| Windows | `EnumWindows` + `DWMWA_EXTENDED_FRAME_BOUNDS`, z-ordered, cloaked windows skipped | exact |
| X11 | `_NET_CLIENT_LIST_STACKING` + frame extents | exact |
| Sway / wlroots with IPC | Sway IPC `get_tree` (Hyprland: `hyprctl clients -j`) | exact |
| KDE | a KWin script loaded over D-Bus that reports window geometry | exact |
| COSMIC | whatever `zcosmic_toplevel_info` / ext-foreign-toplevel offer; otherwise edges | partial |
| GNOME, anything else | edge detection on the frozen image | heuristic |

The edge detector is a pure function in `windup-core` (image + pointer →
rectangle) with tests on recorded screenshots. The owner accepted that it is
less precise on GNOME than elsewhere; the behaviour is the same.

## The live editor

Editing happens on the frozen screen, with the selection still adjustable.

- **Annotations are pinned to the screen, not to the selection.** Moving or
  resizing the selection changes only what is cropped; an annotation partly
  outside is cropped with it, not moved or lost.
- The bar becomes the tool bar: a segmented box of tools — select
  (`mouse-pointer-2`), pen (`brush`),
  rectangle (`square`), ellipse (`circle`), blur, mosaic (`grid-3x3`), colour
  picker (`pipette`) — then a colour swatch, undo, save (`download`), a primary
  **Copy** button, and close. Blur's icon is to be chosen from Lucide by the
  builder and checked on an icon sheet (§11 collisions).
- **Colour**: the swatch opens Umber's colour picker
  (`../Umber/crates/umber-app/src/colorpicker.rs`, GPL-3.0-or-later like
  Windup), ported into `windup-ui`. Wheel mode at minimum; the builder may bring
  the other modes if they come for free. The pipette tool picks a colour from
  the frozen screen.
- **Size**: one size per tool, changed with the mouse wheel while the tool is
  active and shown as a mono figure next to the pointer for a moment. Stroke
  width for pen/rectangle/ellipse, radius for blur, block size for mosaic.
  *(My call.)*
- Rectangle and ellipse are outlines; Shift constrains to square/circle.
  *(My call.)*
- Blur and mosaic are rectangles applied to the frozen pixels beneath them.
  They act on the original screen image, not on other annotations.
- **Placed shapes can be transformed** (the owner's requirement). Every
  annotation stays an object, not pixels: pen strokes, rectangles, ellipses,
  blur and mosaic regions. With the select tool, a click picks the topmost
  annotation under the pointer and shows a transform box (Umber's
  `transform_box` is the model): drag to move, handles to resize, Shift keeps
  proportions. Delete removes it. The swatch and the mouse wheel change the
  selected annotation's colour and size. Clicking an existing annotation with
  the tool that made it also selects it, so switching to the select tool is not
  required for a quick fix. *(That shortcut is my call.)* Rotation is not in
  v1. *(My call.)*
- Undo / redo: Ctrl+Z, Ctrl+Shift+Z and Ctrl+Y. Transforms, deletes and
  colour/size changes are undoable steps like drawing.
- **Enter** copies the result to the clipboard and closes. **Ctrl+S** saves to
  a file and closes. A setting, "Also save a file when copying", makes Enter
  do both. All rebindable except Esc (see "Escape" below).
- The same editor opens on a single step image from the Steps review list, in
  an ordinary window sized to the image. *(My call.)*

Saved files: PNG by default (JPEG as an option) in
`<Pictures>/Windup/` named `Screenshot YYYY-MM-DD HHMMSS.png`; folder and name
pattern in settings. *(My call.)* A copied screenshot lands in Windup's own
clipboard history like any other copied image.

## Video

- The same three targets as screenshots.
- When recording starts the overlay goes away and the screen is live; the bar
  becomes the **recording bar**: a red dot pulsing slowly, a mono timer, pause
  and stop. The dot uses `critical`. The style guide says nothing pulses
  (§13) and that `critical` means failed/destructive; this is a deliberate,
  commented exception because a pulsing red dot is what everyone reads as
  "recording".
- Space pauses and Enter stops **when the recording bar has focus**. They are
  not swallowed globally, because that would stop the user typing a space in
  whatever they are recording. Global hotkeys (defaults below) pause and stop
  from anywhere.
- **The bar is never in a recording.** The owner's rule, for every platform
  and every target. Only Windows can exclude a window from capture, so on
  Linux the rule is met by where the bar is, or by not showing it:

  | Situation | Recording bar |
  |---|---|
  | Windows | Where the launcher was, excluded with `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` |
  | Linux (COSMIC, KDE, wlroots, X11), area or window | Just outside the recorded rectangle: above it, else below it |
  | Linux, the rectangle leaves no room on its monitor | On another monitor, if there is one |
  | Linux, no room anywhere (full screen of the only monitor) | Not shown; hotkeys only, and the toast after saving |
  | GNOME, Weston (window host) | Not shown; hotkeys only. GNOME will neither place Windup's window nor keep it on top |

  Not a desktop notification with Pause/Stop actions: its banner would itself
  be recorded. A GNOME Shell extension with a top-bar indicator is a possible
  later improvement, not v1.
- **Window** for video records the rectangle the window had when recording
  started. It does not follow the window if it moves, and records whatever
  covers it. Same on every platform and needs no system picker. *(My call.)*
- On stop the file is saved, then a toast says where, with an "Open folder"
  action.
- **Audio**: computer sound and microphone, **both off by default**, each a
  toggle on the recording bar before starting, with the device chosen in
  settings.
- **Frame rate and bitrate** are the owner's to set, in settings **and on the
  bar**. After the target is chosen and before recording starts, the bar shows
  a frame-rate dropdown, a bitrate dropdown and the two audio toggles beside
  the start button. Once recording runs they become read-only chips (`30 fps`,
  `8 Mb/s`), because an encoder cannot change them mid-stream and a control
  that pretends otherwise lies (rule 7). *(Read-only while running is my
  call.)* Whatever is picked on the bar becomes the new default.
  - Frame rate: 15, 24, 30, 60, plus the monitor's refresh rate if higher.
    Default **30**.
  - Bitrate: **Auto** by default, or a fixed value. Auto scales with the
    recorded pixel count and frame rate: about 8 Mb/s for 1080p at 30 fps
    with H.264, ×1.5 at 60 fps, and about 60 % of the H.264 figure for H.265
    and AV1. *(The defaults are my call; the builder may tune them by
    looking at the output.)* GIF has no bitrate; its control is replaced by
    a colour-count choice.
- **Encoding**: FFmpeg, **bundled** with Windup on every platform so the codec
  list is the same everywhere (the owner accepted the larger download).
  - Default: MP4, H.264.
  - Codecs: H.264, H.265, AV1 — hardware first (NVENC, AMD AMF, Intel QSV,
    VA-API on Linux) when the machine has them, software otherwise (x264,
    x265, SVT-AV1), plus VP9 for WebM.
  - Containers: MP4, WebM, MOV, MKV, and GIF.
  - Settings offer only combinations that work (WebM takes VP9/AV1 and Opus;
    MOV takes H.264/H.265; GIF takes no audio) and only encoders that a probe
    at startup found working on this machine. A probe is a real one-frame
    encode, not a list from `ffmpeg -encoders`, because NVENC is listed on
    machines without an NVIDIA GPU.
- Saved to `<Videos>/Windup/Recording YYYY-MM-DD HHMMSS.mp4`. *(My call.)*

## Steps

- Start from the launcher or its hotkey; the recording bar shows a step count
  instead of a timer, and follows the same never-in-the-picture placement as
  video.
- **What takes a step**, decided with the owner:
  - A **step hotkey**, on every platform, rebindable. The marker goes where the
    pointer is. This is the one trigger that works the same everywhere.
  - **Mouse clicks**, where Windup can see them: on Windows (`WH_MOUSE_LL`) and
    X11 (XInput2 raw events) without setup. Wayland gives an app no way to see
    clicks in other apps, so on Linux settings offer an optional, explained,
    one-time setup ("Capture clicks too") that adds the user to the `input`
    group, after which clicks are read through libinput/evdev. The settings
    text says plainly that this group can also read the keyboard. Packages may
    ship a udev rule instead; the AppImage cannot, so it uses the group.
  - A setting chooses clicks, hotkey or both, where clicks are available.
- Each step is a screenshot of the pointer's monitor with a marker at the
  pointer. Marker options: ring, numbered badge or arrow; colour; size.
  Pointer position on Wayland comes from the capture's cursor metadata
  (`ext-image-copy-capture` cursor session, the portal's cursor metadata).
- Stop opens a **review list**: numbered steps, each removable, each openable
  in the editor, each with an **optional caption the user types**. Windup does
  not write captions itself (no accessibility back end).
- **Export**: a single self-contained HTML file (images embedded) by default,
  or PDF, or Markdown with an images folder, or just the raw numbered images.

## Hotkeys

| Action | Default |
|---|---|
| Launcher | Win+Shift+S (intercepted like Win+V, displacing Snipping Tool) |
| Screenshot, straight to selection | Print Screen |
| Video, straight to selection | Win+Shift+R |
| Steps | none |
| Take a step | to be chosen by the builder; must not collide with typing |
| Pause / stop recording | to be chosen by the builder; must not collide with typing |
| Toggle the zoom loupe, editor tools | keys inside the overlay, rebindable |
| Cancel | **Esc, always, not rebindable** |

### Escape

The owner's rule: Esc always cancels, everywhere in this feature, and cannot be
rebound or turned off. In the overlay and the editor it closes without saving
or copying. During a video or steps recording it **cancels the recording and
discards it** (no file, no partial steps). *(Discarding is my reading of
"cancel"; a stopped recording is what the stop key is for.)*

During a recording Esc is therefore taken globally for as long as the recording
runs, and the recorded app does not receive it. That is the accepted price of
"always". Windows: the existing `WH_KEYBOARD_LL` hook swallows it. X11: a
passive grab for the duration. Wayland: bound for the duration where the
desktop allows it (COSMIC: a temporary entry in its shortcut file, removed on
stop; the GlobalShortcuts portal elsewhere; the click-capture input path when
the user enabled it). Where none of these is available (GNOME without the
portal), the settings page says so; this is the one place the rule cannot be
kept, and it must be stated, not hidden.

Every one is editable in settings, and Windup honours what is configured. That
means replacing the hardcoded Windows hook branch and X11 grabs with a lookup
against `Config`'s shortcuts. On Wayland each action is a command
(`windup --capture`, `--screenshot`, `--record`, `--steps`, following the
existing `ipc::Command` pattern) bound in the desktop's settings, the
GlobalShortcuts portal where present, and on COSMIC written into its shortcut
file as `cosmic_shortcuts.rs` already does for Super+V. Where a default
collides with the desktop's own binding (Print Screen on GNOME and KDE), the
settings row shows the clash as a caution keycap (§7.13) rather than silently
failing.

## Capture back ends

What the overlay needs is one frozen image per monitor, at native resolution,
with its position and scale. What video needs is a stream of frames from a
monitor or a region of one.

| Platform | Still | Video | Dialog |
|---|---|---|---|
| Windows | DXGI Desktop Duplication or Windows.Graphics.Capture (no yellow border) | the same, into FFmpeg; WASAPI loopback for sound | never |
| X11 | `XShmGetImage` / `XGetImage` via x11rb | the same, or FFmpeg `x11grab` | never |
| COSMIC, wlroots | `ext-image-copy-capture-v1` (output source), else `zwlr_screencopy_v1` | the same protocol per frame | never |
| KDE | `org.kde.KWin.ScreenShot2` over D-Bus, allowed by declaring it in the desktop entry (`X-KDE-DBUS-Restricted-Interfaces`), else the Screenshot portal | ScreenCast portal + PipeWire, `persist_mode` restore token kept | once, if the KWin route fails |
| GNOME | Screenshot portal, non-interactive | ScreenCast portal + PipeWire, restore token kept | once (more if the portal ignores the token) |

**The one-time grant covers every monitor.** On KDE and GNOME the first
ScreenCast request asks for all monitors and keeps the restore token; Windup
crops from that. Otherwise recording a different monitor, or adding one, would
bring the dialog back. The first dialog asking for every screen is a little odd,
and is the lesser cost. *(My call.)* Per-click screenshots for Steps on GNOME
come from the ScreenCast stream, not repeated Screenshot portal calls.

Like the clipboard back end, the choice is made at runtime by a pure tested
function extending `platform::detect`. Sound on Linux comes from PipeWire /
PulseAudio (monitor source for computer sound).

The overlay surfaces reuse what exists: on the layer host a full-output overlay
layer surface per monitor (as the popup does); on the window host a fullscreen
window per output (`xdg_toplevel.set_fullscreen(output)`); on Windows and X11
borderless topmost windows per monitor, using the white-flash avoidance already
in `focus_and_clip_popup_window`.

## Settings

Four new `ToolId`s — `CaptureLauncher`, `Screenshot`, `Video`, `Steps` — each
with its settings pane, following "Adding a tool" in CLAUDE.md. Settings covered
by the above: hotkeys; loupe default; save folder, name pattern and image
format; "also save when copying"; editor keybinds; video container, codec,
encoder, frame rate, quality; audio devices; step marker style, colour and
size; steps export format. The editor's per-tool sizes and colour persist
between uses.

## Phases

Each phase ships on its own.

1. **Hotkeys and screenshots.** Configurable hotkeys for all tools; still
   capture on every platform; the overlay, launcher, three targets, loupe and
   the live editor.
2. **Video.** Bundled FFmpeg in every package (Windows MSI, deb, rpm, Arch,
   AppImage), the encoder probe, frame capture on every platform, audio, the
   recording bar.
3. **Steps.** Step hotkey, click capture (with the optional Linux setup),
   markers, review list with captions, the four exports.

## Known risks

- **Patents.** Bundling x264 and x265 (software H.264/H.265 encoders) in every
  package carries patent exposure in some countries; the GPL side is fine.
  Kept, because the owner chose the same codecs everywhere. If it ever has to
  go, the fallback is an FFmpeg with only VP9, AV1 and the hardware encoders,
  and the default would move off H.264. *(My call to keep it.)*
- **ARM64 Windows** is built but never tested. Video ships there only if an
  ARM64 FFmpeg build passes the encoder probe; otherwise the Video mode is
  drawn disabled with a tooltip saying why (rule 7). Screenshots and steps
  ship regardless. *(My call.)*

## Constraints

- Everything painted, tokens only, Lucide only (CLAUDE.md "UI conventions").
- Every Win32 block `cfg(windows)` with a no-op twin; Linux code under
  `windup-desktop/src/linux/`; the crates still compile on macOS.
- Pure decisions (back-end choice, edge detection, the desktop coordinate
  space and mixed-scale stitching, annotation hit-testing and transforms,
  auto bitrate, codec/container validity, file naming, export) live in
  `windup-core` with tests.
- `cosmic-protocols` stays out (GPL-3.0-only).
- CI stays warning-free on Windows x64, Linux and Windows ARM64. The Linux
  end-to-end script (`tools/linux-e2e.sh`) gains a still capture on headless
  Sway and Weston at least.

## Left to the builder

- Linking FFmpeg's libraries versus driving a bundled `ffmpeg` executable.
  Constraint: a 4K 60 fps stream should not go through a pipe as raw RGBA if
  that cannot keep up; zero-copy to the hardware encoder on Windows is worth
  having.
- DXGI Desktop Duplication versus Windows.Graphics.Capture.
- The edge-detection method for Window mode.
- Pause/stop global hotkey defaults.
- The Lucide icon for blur and for the step markers.
