#!/bin/sh
# Package a built windup.exe for Windows: the MSI, then the setup executable
# that carries it.
#
#   sh packaging/windows/package.sh <version> <arch> <bin-dir> <out-dir>
#   sh packaging/windows/package.sh 0.1.0 x64 target/release dist
#
# <arch> is WiX's name for it: x64 or arm64.
#
# Every OS has a packaging/<os>/package.sh taking these same four arguments,
# and .github/workflows/release.yml calls the one for the runner it is on. See
# docs/packaging.md for adding one.
#
# Needs the WiX 5 dotnet tool and its UI and Util extensions:
#
#   dotnet tool install --global wix --version 5.0.2
#   wix extension add -g WixToolset.UI.wixext/5.0.2
#   wix extension add -g WixToolset.Util.wixext/5.0.2
#
# Run from the repository root.

set -eu

if [ $# -ne 4 ]; then
    echo "usage: $0 <version> <arch> <bin-dir> <out-dir>" >&2
    exit 2
fi

version=$1
arch=$2
bin=$3
out=$4
here=packaging/windows

[ -f "$bin/windup.exe" ] || { echo "no windup.exe in $bin" >&2; exit 1; }
mkdir -p "$out"

# WiX wants its inputs in one folder, and its licence page wants RTF. Under
# target/ and relative, because wix.exe is a Windows program and cannot read a
# POSIX temporary path from Git Bash.
assets=target/wixassets
rm -rf "$assets"
mkdir -p "$assets"
cp assets/icons/windup.ico "$here/banner.bmp" "$here/dialog.bmp" "$assets/"
sh "$here/make-licence-rtf.sh" LICENSE "$assets/licence.rtf"

msi="$out/windup-$version-$arch.msi"

# -pdbtype none: otherwise a .wixpdb lands beside the MSI and is published.
wix build "$here/windup.wxs" \
    -arch "$arch" \
    -ext WixToolset.UI.wixext \
    -ext WixToolset.Util.wixext \
    -pdbtype none \
    -d Version="$version" \
    -d BinDir="$bin" \
    -d DocDir="." \
    -d AssetDir="$assets" \
    -o "$msi"

# The payload follows the MSI because it is the MSI. make-setup checks that
# the written file is recognised as an installer.
cargo run --quiet --release -p windup-core --example make-setup -- \
    "$bin/windup.exe" "$msi" "$out/windup-setup-$version-$arch.exe"
