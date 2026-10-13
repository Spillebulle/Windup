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
4. archive the bare binary with the readme, changelog and licence,
5. `sh packaging/<package>/package.sh <version> <arch> <bin-dir> dist`.

Step 5 is the only platform-specific one, and it lives in the repository, not
in the YAML, so it can be run and debugged locally. Whatever a packager
writes to `dist/` is published.

`ci.yml`'s `check` matrix must cover every target `release.yml` builds, so a
green CI on the commit being tagged means the release will build.

## Windows

| File | What it is |
|---|---|
| `windup-setup-X.Y.Z-<arch>.exe` | `windup.exe` with the MSI appended. Opens Windup's own install window. |
| `windup-X.Y.Z-<arch>.msi` | The package itself, for deployment tools or `msiexec`. |
| `windup-X.Y.Z-<target>.zip` | The bare program. |

`packaging/windows/package.sh` builds the MSI with WiX 5 from `windup.wxs`,
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

`packaging/linux/package.sh` stages one tree (binary, desktop entry, AppStream
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
sh packaging/linux/package.sh 0.1.0 amd64 target/release dist
```

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
