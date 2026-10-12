<p align="center">
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="docs/images/banner-paper.png">
    <img src="docs/images/banner.png" alt="Windup" width="560">
  </picture>
</p>

<p align="center">
  Small Windows tools that live behind a <b>hotkey</b>, in the spirit of PowerToys.
</p>

<p align="center">
  Clipboard history at your cursor · plain-text paste · one settings window ·
  runs in the background
</p>

> **Early days.** Two tools work on Windows: clipboard history and plain-text
> paste. There is no installer yet. [What is not there yet](#what-is-not-there-yet)
> is honest about the rest.

## Install

There is no release yet. Build it from source (below); the result is a
single `windup.exe` with nothing to install.

You need Windows 10 or 11 and a GPU with Direct3D 12 or Vulkan, which is
essentially any machine from the last ten years.

## Clipboard history

Press Win+V and a popup opens at your cursor with everything you have copied,
images included, with previews. Pick an entry and Windup pastes it into the
window you were in. It replaces the Windows clipboard panel rather than
sitting beside it.

## Paste plain text

Press Win+Shift+V to paste what is on the clipboard with its fonts, colours
and links stripped. Useful when copying out of a web page or a document into
anything that would otherwise keep the formatting.

## What is not there yet

- **No installer or release.** Building from source is the only way in.
- **Windows only, in practice.** The code compiles on Linux and macOS with
  fallbacks for the hotkeys and paste, but nobody has used it there.
- **Two tools.** The settings window is built for more; none of the others
  exist yet.

## Controls

| Shortcut | Does |
|---|---|
| Win+V | Open clipboard history at the cursor |
| Win+Shift+V | Paste the clipboard as plain text |
| Type | Search the history |
| Up, Down | Move through the history |
| Enter | Paste the selected entry |
| Delete | Remove the selected entry |
| Esc | Close the popup |

Both shortcuts can be changed or switched off in the settings window.

## Building from source

```sh
git clone https://github.com/Spillebulle/Windup.git
cd Windup
cargo build --release      # target/release/windup.exe
```

Rust 1.90 or newer.

## Licence

GPL-3.0-or-later, see [`LICENSE`](LICENSE). The Archivo typeface is under the
SIL Open Font Licence ([`assets/OFL.txt`](assets/OFL.txt)); icons are from
[Lucide](https://lucide.dev) (ISC).
