# Packaging and releases

2026-10-12

How a release is built, what each platform ships, and how to add a platform.
Windows and Linux (x86-64) are released.

## Cutting a release

1. Bump `version` in `[workspace.package]` in `Cargo.toml`.
2. Add a `## X.Y.Z` section at the top of `CHANGELOG.md`: three to eight
   bullets, each a change a user would notice. It is published verbatim as the
   release notes.
3. Update the version in the README's download table.
4. Commit, then `pwsh tools/release.ps1 X.Y.Z`.

`crates/windup-desktop/tests/release.rs` fails CI if steps 2 or 3 were
missed. `release.ps1` runs the same gates CI runs, pushes `main`, waits for
CI to pass on that commit, then pushes the tag `vX.Y.Z`. The tag starts
`.github/workflows/release.yml`, which builds every platform, packages it and
publishes a GitHub release titled with the bare version.

To rehearse a change to the workflow without a tag, run it by hand from the
Actions tab (`workflow_dispatch`). It builds and packages everything and
leaves the files as workflow artefacts, and publishes nothing.

## How the workflow is shaped

Each row of the `build` matrix is one target. Every row runs the same steps:

1. install the platform's build dependencies,
2. test (where the runner can run the target),
3. `cargo build --release -p windup-desktop --target <target>`,
   then `tools/fetch-ffmpeg.sh` into the same folder (see FFmpeg below),
4. archive the bare binary with the readme, changelog and licence,
5. `sh packaging/<package>/package.sh <version> <arch> <bin-dir> dist`.

Step 5 is the only platform-specific one, and it lives in the repository, not
in the YAML, so it can be run and debugged locally. Whatever a packager
writes to `dist/` is published.

`ci.yml`'s `check` matrix must cover every target `release.yml` builds, so a
green CI on the commit being tagged means the release will build.

## FFmpeg

The screen recorder drives an `ffmpeg` executable that every package carries;
Windup never looks for the system's. `packaging/ffmpeg.lock` pins BtbN's GPL
static builds (`FFmpeg-Builds`) by dated `autobuild-*` tag, URL and SHA-256 for
`win64`, `winarm64` and `linux64`. `tools/fetch-ffmpeg.sh <platform> <dir>`
downloads one (cached in `target/ff-dl`), refuses it with "checksum mismatch"
if the sum differs, and leaves `ffmpeg[.exe]`, `LICENSE.txt` and
`FFMPEG-SOURCE.txt` (the GPL source offer) in `<dir>`. `--with-ffprobe` also
extracts ffprobe, for CI's end-to-end job only; packages never ship it.
`FFMPEG_LOCK` points the script at another lock file.

`release.yml` fetches into the build's bin dir before Archive and Package, and
both package scripts fail with a pointer to the fetch script when the three
files are missing. Where Windup finds it at run time is in `docs/linux.md` and
the capture design: `WINDUP_FFMPEG`, next to the executable, then
`<exe>/../lib/windup/ffmpeg`.

To update FFmpeg: pick a newer `autobuild-*` release of
`BtbN/FFmpeg-Builds` with version-named assets (the `latest` names move), put
the three URLs and the tag in the lock, recompute the sums from the downloaded
files, and run `--encoders-test` (CI does, on Windows). Then mirror the new
archives (below). The Windows packages grow by about 60 MB compressed for it.

BtbN prunes old autobuild tags, which would leave a pinned URL dead and the
next release unbuildable. So the same three archives are kept as the assets
of a release `ffmpeg-<tag>` in this repository, and the fetch script tries
that mirror first and BtbN second; the lock's sums decide either way, so the
mirror cannot change what ships. In the workflows the mirror is read through
`gh` with the job's token (`GH_TOKEN`), which works while the repository is
private; elsewhere it is a plain download, and `FFMPEG_MIRROR=none` skips it.
To make the mirror for the current lock:

```sh
sh tools/mirror-ffmpeg.sh            # downloads and checks the archives, prints the gh command and notes
sh tools/mirror-ffmpeg.sh --publish  # gh release create ffmpeg-<tag> --latest=false ... <the three archives>
```

Its tag does not match `v*`, so it starts no release build, and `--latest=false`
keeps Windup's own release the latest one. The notes name BtbN's tag and
build scripts (the GPL source offer) and the sums.

## Windows

| File | What it is |
|---|---|
| `windup-setup-X.Y.Z-<arch>.exe` | `windup.exe` with the MSI appended. Opens Windup's own install window. |
| `windup-X.Y.Z-<arch>.msi` | The package itself, for deployment tools or `msiexec`. |
| `windup-X.Y.Z-<target>.zip` | The bare program. |

`packaging/windows/package.sh` builds the MSI with WiX 5 from `windup.wxs`
(`ffmpeg.exe` beside `windup.exe`; its licence and source note in
`licenses\ffmpeg\`, with their own pinned component GUIDs),
then runs the `make-setup` example in `windup-core` to append it to the
executable. The setup executable decides it is the installer by finding that
payload on its own file (`windup_core::install::payload`): a double-clicked
file has no arguments. It then runs msiexec silently and elevated
(`crates/windup-desktop/src/setup.rs`) behind the window drawn by
`windup_ui::setup`.

The icon is compiled into `windup.exe` by `crates/windup-desktop/build.rs`
from `assets/icons/windup.ico`. That file and the two WiX bitmaps are
generated: `cargo run -p windup-ui --example make-art`, then commit the
results.

ARM64 is cross-compiled on the x64 runner, because GitHub's ARM runners are
not free for private repositories. It is built and packaged but not tested.
If the repository goes public, move it to `windows-11-arm` and let it test.

To build the installer locally:

```sh
dotnet tool install --global wix --version 5.0.2
wix extension add -g WixToolset.UI.wixext/5.0.2
wix extension add -g WixToolset.Util.wixext/5.0.2
cargo build --release -p windup-desktop
sh tools/fetch-ffmpeg.sh win64 target/release
sh packaging/windows/package.sh 0.1.0 x64 target/release dist
```

## Linux

| File | What it is |
|---|---|
| `windup_X.Y.Z_amd64.deb` | Debian, Ubuntu and derivatives. Enrols the machine in the house archive. |
| `windup-X.Y.Z-1.x86_64.rpm` | Fedora, openSUSE, RHEL. Enrols the machine likewise. |
| `windup-bin-X.Y.Z-1-x86_64.pkg.tar.zst` | Arch and derivatives, `pacman -U`. |
| `Windup-X.Y.Z-x86_64.AppImage` | Anything else. |
| `windup-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz` | The bare program. |

`packaging/linux/package.sh` stages one tree (binary, `/usr/lib/windup/ffmpeg`
with its licence under `/usr/share/licenses/windup/ffmpeg/`, desktop entry, AppStream
file, hicolor icons from `assets/icons/hicolor/`, docs) and builds all four
from it: `dpkg-deb`, `rpmbuild`, `makepkg` (in an `archlinux:base-devel`
container when the host has none), and linuxdeploy. The dependency lists in
it are hand-written because most of the libraries are dlopened; CI's
`linux-packages` job installs each package in a clean Debian, Fedora and Arch
container to prove they resolve. `packaging/check.sh` keeps the app id
`io.github.spillebulle.windup` the same in the desktop entry, the AppStream
file, the icons and the program.

The `.deb` and `.rpm` carry `packaging/linux/spillebulle-archive.asc` and write
the archive's source on install (STYLE-GUIDE §18.2). `Spillebulle/Windup` is
listed in `../Packages/projects`, and the last step of `release.yml` asks the
archive to rebuild once the release exists. That step needs the
`ARCHIVE_DISPATCH_TOKEN` secret (see the archive's README) and is skipped
without it; the archive's weekly run then picks the release up instead.

Built on Ubuntu 22.04, so the binary needs glibc 2.35 or newer. No ARM64:
GitHub's ARM runners are not free for private repositories, and the Arch image
is x86-64 only. No Flatpak or Snap, deliberately; `docs/linux.md` says why.

To build them locally on Debian or Ubuntu:

```sh
sh packaging/linux/dependencies.sh
sudo apt-get install -y rpm
cargo build --release -p windup-desktop
sh tools/fetch-ffmpeg.sh linux64 target/release
sh packaging/linux/package.sh 0.1.0 amd64 target/release dist
```

### The pointer rule

On Wayland no application sees another's clicks, so Steps reads the mouse and
touchpad event nodes itself (evdev, `crates/windup-desktop/src/linux/steps/`).
The `.deb`, `.rpm` and Arch package ship
`packaging/linux/70-windup-pointer.rules` to
`/usr/lib/udev/rules.d/`. It tags mice and touchpads `uaccess`, which gives
the logged-in seat user an ACL on their nodes; any device that is also a
keyboard is left out, so keys stay unreadable. The name must sort before
`73-seat-late.rules`, which turns the tag into the ACL. Install, upgrade and
removal run `udevadm control --reload-rules` and a `change` trigger on the
input subsystem (the deb `postinst`/`postrm`, the rpm `%post`/`%postun`,
`_windup_udev` in `windup.install`), each ending in `|| true`, so the rule
applies to devices already plugged in and a container without udev still
installs.

The AppImage cannot install a rule. There the settings offer to add the user
to the `input` group through `pkexec usermod` (`linux/steps/setup.rs`), say
that the group can read the keyboard too, and that it counts from the next
login. The settings tell the two routes apart from whether the nodes are
readable and whether the process has the group (`ClickRoute`).

`packaging/check.sh` refuses a rule that matches on `ID_INPUT_KEY` or grants
`uaccess` on a line without `ENV{ID_INPUT_KEYBOARD}!="1"`, and
`check-enrolment.sh` checks the file is installed by each of the three
packages.

## Adding a platform

A new platform needs:

1. **A matrix row in both workflows.** In `release.yml`, for example
   `{ os: ubuntu-22.04, target: x86_64-unknown-linux-gnu, arch: amd64, package: linux, test: true }`,
   and the same target in `ci.yml`'s `check` matrix. Build Linux on the oldest
   supported Ubuntu so the binary runs on newer glibc, not the other way round.
2. **`packaging/<os>/package.sh`** taking `<version> <arch> <bin-dir> <out-dir>`
   and writing its packages to `<out-dir>`. Build dependencies go in a
   `packaging/<os>/dependencies.sh` the workflow can call
   (`packaging/linux/dependencies.sh` already exists).
3. **The platform's own identity files.** The executable icon is a Windows
   resource; elsewhere it is packaging. On Linux, a desktop entry and
   AppStream metadata named after an app id (`io.github.spillebulle.windup`
   would match the family), with icons from `make-art`, and the same id passed
   to `ViewportBuilder::with_app_id` so Wayland finds the icon. On macOS, an
   `.app` bundle with an `Info.plist` and an `.icns`.
4. **The file names in `ASSETS`** in `crates/windup-desktop/tests/release.rs`,
   and a row in the README's download table.

Muster (`../Muster/packaging/linux/`) has a working `.deb`, `.rpm`, AppImage,
Flatpak and Arch setup to start from. Change every identifier it carries:
copying an application id or a GUID from a sibling app makes the system treat
the two as one product.

The setup executable and `windup_core::install` are Windows-only by design.
Other platforms install through their own package formats, so they need no
installer window.

## Not done yet

- Code signing. The setup file and `windup.exe` are unsigned, so SmartScreen
  warns on first run.
- An in-app update check.
- A tray icon, and start at sign-in on Windows. (Linux writes an XDG
  autostart entry.)
