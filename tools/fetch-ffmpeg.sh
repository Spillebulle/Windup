#!/bin/sh
# Download the FFmpeg build pinned in packaging/ffmpeg.lock into a folder.
#
#   sh tools/fetch-ffmpeg.sh <platform> <dest-dir> [--with-ffprobe]
#   sh tools/fetch-ffmpeg.sh win64 target/ff
#
# <platform> is win64, winarm64 or linux64. Writes ffmpeg[.exe], LICENSE.txt
# and FFMPEG-SOURCE.txt into <dest-dir>. Packages never ship ffprobe;
# --with-ffprobe is for the end-to-end job, which inspects what it recorded.
#
# The archive is checked against the lock's SHA-256 before anything is
# extracted. Downloads are cached in target/ff-dl (FFMPEG_CACHE overrides),
# FFMPEG_LOCK points at a different lock file (the tests use a tampered copy).
#
# Run from the repository root.

set -eu

if [ $# -lt 2 ] || [ $# -gt 3 ]; then
    echo "usage: $0 <win64|winarm64|linux64> <dest-dir> [--with-ffprobe]" >&2
    exit 2
fi

platform=$1
dest=$2
probe=no
if [ $# -eq 3 ]; then
    [ "$3" = "--with-ffprobe" ] || { echo "unknown option '$3'" >&2; exit 2; }
    probe=yes
fi

lock=${FFMPEG_LOCK:-packaging/ffmpeg.lock}
cache=${FFMPEG_CACHE:-target/ff-dl}

tag=$(awk '$1 == "tag" { print $2 }' "$lock")
set -- $(awk -v p="$platform" '$1 == p { print $2, $3 }' "$lock")
[ $# -eq 2 ] || { echo "no '$platform' line in $lock" >&2; exit 2; }
url=$1
want=$2
[ -n "$tag" ] || { echo "no tag line in $lock" >&2; exit 2; }

case "$platform" in
    win64|winarm64) exe=ffmpeg.exe; probe_exe=ffprobe.exe ;;
    linux64)        exe=ffmpeg;     probe_exe=ffprobe ;;
    *) echo "unknown platform '$platform' (want win64, winarm64 or linux64)" >&2; exit 2 ;;
esac

archive=$cache/${url##*/}
mkdir -p "$cache" "$dest"

sum() { sha256sum "$1" | cut -d' ' -f1; }

if [ ! -f "$archive" ] || [ "$(sum "$archive")" != "$want" ]; then
    echo "==> downloading ${url##*/}"
    curl -fsSL --retry 3 -o "$archive.part" "$url"
    mv "$archive.part" "$archive"
fi
got=$(sum "$archive")
if [ "$got" != "$want" ]; then
    echo "checksum mismatch for ${url##*/}: expected $want, got $got" >&2
    rm -f "$archive"
    exit 1
fi

# Extract into a scratch folder, then copy out only what is wanted: the
# archives hold ffplay too, and nearly 200 MB per executable.
#
# The scratch folder sits beside the cache, not under /tmp: 7z.exe is a Windows
# program that cannot read a POSIX temporary path from Git Bash. cygpath
# (where there is one) turns what we hand it into a path it can read.
scratch=$(mktemp -d "$cache/extract.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
native() { if command -v cygpath >/dev/null 2>&1; then cygpath -m "$1"; else printf '%s
' "$1"; fi; }
case "$archive" in
    *.zip)
        # FFMPEG_EXTRACTOR=7z forces the fallback, to test it where unzip exists.
        if [ "${FFMPEG_EXTRACTOR:-}" != 7z ] && command -v unzip >/dev/null 2>&1; then
            unzip -q "$archive" -d "$scratch"
        else
            7z x -y -o"$(native "$scratch")" "$(native "$archive")" >/dev/null
        fi ;;
    *.tar.xz) tar -xJf "$archive" -C "$scratch" ;;
    *) echo "do not know how to unpack $archive" >&2; exit 1 ;;
esac

top=$(find "$scratch" -mindepth 1 -maxdepth 1 -type d | head -n 1)
[ -f "$top/bin/$exe" ] || { echo "no bin/$exe in ${url##*/}" >&2; exit 1; }
cp "$top/bin/$exe" "$dest/$exe"
cp "$top/LICENSE.txt" "$dest/LICENSE.txt"
if [ "$probe" = yes ]; then
    cp "$top/bin/$probe_exe" "$dest/$probe_exe"
fi
chmod 755 "$dest/$exe"

# The GPL asks for the source, or an offer of it. BtbN's tag names the exact
# build scripts, and the FFmpeg version in the archive name names the source.
version=${url##*/}
version=${version#ffmpeg-}
version=${version%%-"$platform"-*}
cat >"$dest/FFMPEG-SOURCE.txt" <<TXT
This FFmpeg is BtbN's GPL static build, unmodified, from release $tag.

  Build: https://github.com/BtbN/FFmpeg-Builds/releases/tag/$tag
  Build scripts: https://github.com/BtbN/FFmpeg-Builds
  FFmpeg source: https://git.ffmpeg.org/ffmpeg.git (revision: $version)
  Archive: $url
  SHA-256: $want

Windup runs it as a separate program. Its licence is in LICENSE.txt.
TXT
echo "==> $dest: $exe, LICENSE.txt, FFMPEG-SOURCE.txt"
