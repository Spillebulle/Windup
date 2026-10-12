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
> paste. [What is not there yet](#what-is-not-there-yet) is honest about the
> rest.

## Install

**Windup 0.1.0.** Take the installer for your PC, or browse the
[release itself](https://github.com/Spillebulle/Windup/releases/latest) for the
notes.

| | x64 | ARM64 |
|---|---|---|
| Windows | [Installer](https://github.com/Spillebulle/Windup/releases/download/v0.1.0/windup-setup-0.1.0-x64.exe) | [Installer](https://github.com/Spillebulle/Windup/releases/download/v0.1.0/windup-setup-0.1.0-arm64.exe) |
| Windows, `.msi` to deploy | [`.msi`](https://github.com/Spillebulle/Windup/releases/download/v0.1.0/windup-0.1.0-x64.msi) | [`.msi`](https://github.com/Spillebulle/Windup/releases/download/v0.1.0/windup-0.1.0-arm64.msi) |
| Windows, no installer | [`.zip`](https://github.com/Spillebulle/Windup/releases/download/v0.1.0/windup-0.1.0-x86_64-pc-windows-msvc.zip) | [`.zip`](https://github.com/Spillebulle/Windup/releases/download/v0.1.0/windup-0.1.0-aarch64-pc-windows-msvc.zip) |

The installer asks Windows for permission once, installs for everyone on the
PC, adds Windup to the Start menu and starts it. Uninstall it from Settings,
Apps. The files are not signed yet, so SmartScreen may warn the first time;
choose More info, then Run anyway.

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

- **No start at sign-in or tray icon.** The settings for both are there and
  saved, but Windup does not act on them yet. Start it from the Start menu.
- **Unsigned.** The installer and program are not code-signed.
- **Windows only, in practice.** The code compiles on Linux and macOS with
  fallbacks for the hotkeys and paste, but nobody has used it there and there
  are no packages for either.
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
