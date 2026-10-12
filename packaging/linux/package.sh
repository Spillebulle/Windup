#!/bin/bash
# Turns a built `windup` into the Linux packages.
#
#   sh packaging/linux/package.sh <version> <arch> <bin-dir> <out-dir>
#
#   version   0.1.0
#   arch      amd64 | arm64      (Debian's spelling; the others are derived)
#   bin-dir   the directory holding the release `windup`
#
# Writes into <out-dir>:
#
#   windup_<v>_<arch>.deb                    Debian, Ubuntu, Mint, Pop!_OS
#   windup-<v>-1.<rpm-arch>.rpm              Fedora, openSUSE, RHEL
#   windup-bin-<v>-1-<arch>.pkg.tar.zst      Arch, CachyOS, EndeavourOS, Manjaro
#   Windup-<v>-<arch>.AppImage               anything else
#
# Laid out by hand with dpkg-deb, rpmbuild and makepkg rather than cargo-deb
# and friends, after Muster's packaging/linux/build-packages.sh: the trees are
# readable here, and the libraries that matter are dlopened, so no automatic
# dependency scan would find them (see DEPENDS). The Arch package is built in
# an archlinux container when makepkg is not on the machine, which is how the
# Ubuntu release runner makes it. Runnable on any Debian-ish box with the
# tools installed, not only in CI.

# The workflow runs every packager as `sh package.sh`; on Ubuntu that is dash.
[ -n "${BASH_VERSION:-}" ] || exec bash "$0" "$@"

set -euo pipefail

if [ $# -lt 4 ]; then
    sed -n '2,16p' "$0" >&2
    exit 2
fi

version=$1
arch=$2
bindir=$3
outdir=$4

root=$(cd -- "$(dirname -- "$0")/../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

mkdir -p "$outdir"
outdir=$(cd "$outdir" && pwd)
binary=$(cd "$bindir" && pwd)/windup

case "$arch" in
    amd64) rpm_arch=x86_64;  pacman_arch=x86_64;  appimage_arch=x86_64  ;;
    arm64) rpm_arch=aarch64; pacman_arch=aarch64; appimage_arch=aarch64 ;;
    *) echo "unknown arch '$arch' (want amd64 or arm64)" >&2; exit 2 ;;
esac

[ -x "$binary" ] || { echo "no executable at '$binary'" >&2; exit 1; }

APP_ID=io.github.spillebulle.windup

# Linked, so in the ELF's NEEDED list: libxkbcommon (keyboard input on the
# popup) and libxdo (X11 paste, through enigo). Dlopened, so invisible to any
# scan: the Wayland and X11 client libraries winit opens, and the Vulkan
# loader wgpu opens. A package missing the dlopened ones would install
# cleanly and then fail to open a window.
DEB_DEPENDS="libc6, libgcc-s1, libxkbcommon0, libxdo3, libwayland-client0, libx11-6, libxcursor1, libxrandr2, libxi6, libvulkan1"
# By soname, not package name, so one .rpm installs on Fedora and openSUSE
# alike (they name the same libraries differently).
RPM_SONAMES="libxkbcommon.so.0 libxdo.so.3 libwayland-client.so.0 libX11.so.6 libXcursor.so.1 libXrandr.so.2 libXi.so.6 libvulkan.so.1"
PACMAN_DEPENDS="'glibc' 'gcc-libs' 'libxkbcommon' 'xdotool' 'wayland' 'libx11' 'libxcursor' 'libxrandr' 'libxi' 'vulkan-icd-loader'"

DESCRIPTION_SHORT="Clipboard history and plain-text paste behind a hotkey"

# --- the house archive (STYLE-GUIDE §18.2) ----------------------------------
#
# The .deb and .rpm carry the archive's public key and write its source on
# install, so a machine that installs one package from the release page
# upgrades Windup with everything else from then on. The source file is
# written by the scriptlet, never shipped, created only when absent, and
# never removed: it is shared with every other app from the same archive.
ARCHIVE_KEYRING=spillebulle-archive
ARCHIVE_BASE=https://spillebulle.github.io/packages
archive_key=$root/packaging/linux/$ARCHIVE_KEYRING.asc
if [ -f "$archive_key" ] && command -v gpg >/dev/null 2>&1; then
    gpg --dearmor <"$archive_key" >"$work/$ARCHIVE_KEYRING.gpg"
    echo "==> packages will enrol the machine in $ARCHIVE_BASE"
else
    archive_key=
    echo "!! no archive key or no gpg: these packages will not enrol the machine" >&2
fi

# --- the shared tree --------------------------------------------------------

stage_tree() {
    local prefix=$1
    install -Dm755 "$binary" "$prefix/bin/windup"
    install -Dm644 "$root/packaging/$APP_ID.desktop" \
        "$prefix/share/applications/$APP_ID.desktop"
    install -Dm644 "$root/packaging/$APP_ID.metainfo.xml" \
        "$prefix/share/metainfo/$APP_ID.metainfo.xml"
    for size in 16 32 48 64 128 256; do
        install -Dm644 "$root/assets/icons/hicolor/$size.png" \
            "$prefix/share/icons/hicolor/${size}x${size}/apps/$APP_ID.png"
    done
    install -Dm644 "$root/LICENSE" "$prefix/share/licenses/windup/LICENSE"
    install -Dm644 "$root/README.md" "$prefix/share/doc/windup/README.md"
    install -Dm644 "$root/CHANGELOG.md" "$prefix/share/doc/windup/CHANGELOG.md"
    install -Dm644 "$root/docs/linux.md" "$prefix/share/doc/windup/linux.md"
}

# --- .deb -------------------------------------------------------------------

deb_name="windup_${version}_${arch}.deb"
echo "==> $deb_name"
deb=$work/deb
stage_tree "$deb/usr"
if [ -n "$archive_key" ]; then
    install -Dm644 "$work/$ARCHIVE_KEYRING.gpg" "$deb/usr/share/keyrings/$ARCHIVE_KEYRING.gpg"
fi
mkdir -p "$deb/DEBIAN"
cat >"$deb/DEBIAN/control" <<EOF
Package: windup
Version: $version
Section: utils
Priority: optional
Architecture: $arch
Depends: $DEB_DEPENDS
Installed-Size: $(du -ks "$deb/usr" | cut -f1)
Maintainer: Spillebulle <spillebulle@gmail.com>
Homepage: https://github.com/Spillebulle/Windup
Description: $DESCRIPTION_SHORT
 Press Super+V and a popup opens at your cursor with everything you have
 copied, images included; pick an entry and Windup pastes it into the window
 you were in. Super+Shift+V pastes the clipboard as plain text.
 .
 Works on Wayland (COSMIC, KDE Plasma, GNOME, Sway, Hyprland and other
 wlroots desktops) and on X11.
EOF
cat >"$deb/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t /usr/share/icons/hicolor || true
fi
EOF
if [ -n "$archive_key" ]; then
    cat >>"$deb/DEBIAN/postinst" <<EOF

sources=/etc/apt/sources.list.d/spillebulle.sources
if [ ! -e "\$sources" ] && [ ! -e "\$sources.disabled" ]; then
    cat > "\$sources" <<'SOURCES'
Types: deb
URIs: $ARCHIVE_BASE/deb/
Suites: ./
Signed-By: /usr/share/keyrings/$ARCHIVE_KEYRING.gpg
SOURCES
fi
EOF
fi
cat >"$deb/DEBIAN/postrm" <<'EOF'
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications || true
fi
EOF
chmod 755 "$deb/DEBIAN/postinst" "$deb/DEBIAN/postrm"
dpkg-deb --build --root-owner-group "$deb" "$outdir/$deb_name" >/dev/null

# --- .rpm -------------------------------------------------------------------

echo "==> windup-${version}-1.${rpm_arch}.rpm"
rpmroot=$work/rpm
mkdir -p "$rpmroot"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
tree=$work/rpmtree
stage_tree "$tree/usr"
rpm_key=/etc/pki/rpm-gpg/RPM-GPG-KEY-spillebulle
[ -n "$archive_key" ] && install -Dm644 "$archive_key" "$tree$rpm_key"
{
    echo "Name:           windup"
    echo "Version:        $version"
    echo "Release:        1"
    echo "Summary:        $DESCRIPTION_SHORT"
    echo "License:        GPL-3.0-or-later"
    echo "URL:            https://github.com/Spillebulle/Windup"
    echo "BuildArch:      $rpm_arch"
    for so in $RPM_SONAMES; do echo "Requires:       ${so}()(64bit)"; done
    echo "%global debug_package %{nil}"
    echo
    echo "%description"
    echo "Press Super+V and a popup opens at your cursor with everything you have"
    echo "copied; pick an entry and Windup pastes it into the window you were in."
    echo "Super+Shift+V pastes the clipboard as plain text. Wayland and X11."
    echo
    echo "%install"
    echo "cp -a $tree/usr %{buildroot}/"
    [ -n "$archive_key" ] && echo "cp -a $tree/etc %{buildroot}/"
    echo
    echo "%post"
    echo "command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q /usr/share/applications || :"
    if [ -n "$archive_key" ]; then
        echo "repo=/etc/yum.repos.d/spillebulle.repo"
        echo "if [ ! -e \"\$repo\" ] && [ ! -e \"\$repo.disabled\" ]; then"
        echo "    cat > \"\$repo\" <<'REPO'"
        echo "[spillebulle]"
        echo "name=Spillebulle"
        echo "baseurl=$ARCHIVE_BASE/rpm/"
        echo "enabled=1"
        echo "gpgcheck=1"
        echo "repo_gpgcheck=1"
        echo "gpgkey=file://$rpm_key"
        echo "REPO"
        echo "fi"
    fi
    echo
    echo "%postun"
    echo "command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q /usr/share/applications || :"
    echo
    echo "%files"
    [ -n "$archive_key" ] && echo "$rpm_key"
    echo "/usr/bin/windup"
    echo "/usr/share/applications/$APP_ID.desktop"
    echo "/usr/share/metainfo/$APP_ID.metainfo.xml"
    echo "/usr/share/icons/hicolor/*/apps/$APP_ID.png"
    echo "/usr/share/licenses/windup/"
    echo "/usr/share/doc/windup/"
} >"$rpmroot/SPECS/windup.spec"
rpmbuild --define "_topdir $rpmroot" --define "_buildhost windup-release" \
    -bb "$rpmroot/SPECS/windup.spec" >/dev/null
find "$rpmroot/RPMS" -name '*.rpm' -exec cp {} "$outdir/" \;

# --- Arch -------------------------------------------------------------------
#
# pacman does not care about the archive (it is apt and rpm only); CachyOS
# and friends install this with `sudo pacman -U`. Built from the same staged
# tree, so it is the same files as the other two.

pkg_name="windup-bin-${version}-1-${pacman_arch}.pkg.tar.zst"
echo "==> $pkg_name"
arch_dir=$work/arch
mkdir -p "$arch_dir"
stage_tree "$arch_dir/tree/usr"
tar -C "$arch_dir/tree" -cf "$arch_dir/windup-tree.tar" usr
cat >"$arch_dir/PKGBUILD" <<EOF
# Generated by packaging/linux/package.sh from the release binary.
pkgname=windup-bin
pkgver=$version
pkgrel=1
pkgdesc="$DESCRIPTION_SHORT"
arch=('$pacman_arch')
url="https://github.com/Spillebulle/Windup"
license=('GPL-3.0-or-later')
provides=('windup')
conflicts=('windup')
depends=($PACMAN_DEPENDS)
options=('!strip' '!debug')
source=('windup-tree.tar')
sha256sums=('SKIP')

package() {
    cp -a "\$srcdir/usr" "\$pkgdir/"
}
EOF
if command -v makepkg >/dev/null 2>&1; then
    (cd "$arch_dir" && PKGDEST="$outdir" makepkg --nodeps --noconfirm -f >/dev/null)
elif command -v docker >/dev/null 2>&1; then
    chmod -R a+rwX "$arch_dir"
    docker run --rm -v "$arch_dir:/build" -v "$outdir:/out" archlinux:base-devel bash -c '
        set -e
        useradd -m builder
        chown -R builder /build
        su builder -c "cd /build && makepkg --nodeps --noconfirm -f"
        cp /build/*.pkg.tar.zst /out/
    ' >/dev/null
else
    echo "!! neither makepkg nor docker: no Arch package" >&2
    exit 1
fi

# --- AppImage ---------------------------------------------------------------
#
# The one format that runs on a distribution nobody chose, so it carries its
# linked libraries. The dlopened display and Vulkan libraries are the host's,
# deliberately: bundled ones would talk to the wrong driver.

appimage_name="Windup-${version}-${appimage_arch}.AppImage"
echo "==> $appimage_name"
appdir=$work/AppDir
stage_tree "$appdir/usr"
cp "$root/packaging/$APP_ID.desktop" "$appdir/$APP_ID.desktop"
cp "$root/assets/icons/hicolor/256.png" "$appdir/$APP_ID.png"
tools=${APPIMAGE_TOOL_DIR:-$work/tools}
mkdir -p "$tools"
if [ ! -x "$tools/linuxdeploy" ]; then
    curl -fsSL -o "$tools/linuxdeploy" \
        "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-${appimage_arch}.AppImage"
    chmod +x "$tools/linuxdeploy"
fi
export APPIMAGE_EXTRACT_AND_RUN=1 VERSION=$version
OUTPUT="$outdir/$appimage_name" "$tools/linuxdeploy" \
    --appdir "$appdir" \
    --desktop-file "$appdir/$APP_ID.desktop" \
    --icon-file "$appdir/$APP_ID.png" \
    --output appimage >/dev/null

echo
echo "built into $outdir:"
ls -1 "$outdir"
