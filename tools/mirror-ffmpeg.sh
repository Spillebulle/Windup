#!/bin/sh
# Mirror the FFmpeg archives pinned in packaging/ffmpeg.lock as the assets of
# a release `ffmpeg-<tag>` in this repository, which tools/fetch-ffmpeg.sh
# tries before BtbN (whose old autobuild tags get pruned).
#
#   sh tools/mirror-ffmpeg.sh            # say what would be published
#   sh tools/mirror-ffmpeg.sh --publish  # publish it with gh
#
# Run it from the repository root once per lock update, by hand: it changes
# GitHub. Each archive comes from BtbN (or the fetch cache) and is checked
# against the lock's SHA-256 before it is uploaded. The release is marked
# not latest, so the download page keeps pointing at Windup itself, and its
# tag does not match `v*`, so no release build runs. FFMPEG_MIRROR names
# another owner/repo, as for the fetch script.

set -eu

publish=no
case "${1:-}" in
    "") ;;
    --publish) publish=yes ;;
    *) echo "usage: $0 [--publish]" >&2; exit 2 ;;
esac

lock=${FFMPEG_LOCK:-packaging/ffmpeg.lock}
cache=${FFMPEG_CACHE:-target/ff-dl}
repo=${FFMPEG_MIRROR:-Spillebulle/Windup}
[ "$repo" != none ] || { echo "FFMPEG_MIRROR is none: nothing to mirror to" >&2; exit 2; }

tag=$(awk '$1 == "tag" { print $2 }' "$lock")
[ -n "$tag" ] || { echo "no tag line in $lock" >&2; exit 2; }
mkdir -p "$cache"

sum() { sha256sum "$1" | cut -d' ' -f1; }

files=""
sums=""
for platform in win64 winarm64 linux64; do
    set -- $(awk -v p="$platform" '$1 == p { print $2, $3 }' "$lock")
    [ $# -eq 2 ] || { echo "no '$platform' line in $lock" >&2; exit 2; }
    url=$1
    want=$2
    archive=$cache/${url##*/}
    if [ ! -f "$archive" ] || [ "$(sum "$archive")" != "$want" ]; then
        echo "==> downloading ${url##*/} from BtbN"
        curl -fsSL --retry 3 -o "$archive.part" "$url"
        mv "$archive.part" "$archive"
    fi
    got=$(sum "$archive")
    if [ "$got" != "$want" ]; then
        echo "checksum mismatch for ${url##*/}: expected $want, got $got" >&2
        rm -f "$archive"
        exit 1
    fi
    files="$files $archive"
    sums="$sums    $want  ${url##*/}
"
done

notes="BtbN's GPL static FFmpeg builds from release $tag, unchanged, mirrored for
Windup's release builds (tools/fetch-ffmpeg.sh) in case BtbN prunes the tag.
Not a Windup release.

The corresponding source and build recipe are BtbN's build scripts at that
tag: https://github.com/BtbN/FFmpeg-Builds/tree/$tag
Original release: https://github.com/BtbN/FFmpeg-Builds/releases/tag/$tag

SHA-256 (as pinned in packaging/ffmpeg.lock):
$sums"

if [ "$publish" = no ]; then
    echo "==> would run: gh release create ffmpeg-$tag --repo $repo --latest=false --title \"FFmpeg $tag\" --notes <below>$files"
    printf '%s' "$notes"
    echo "==> run again with --publish to create it"
    exit 0
fi

# shellcheck disable=SC2086 # $files is a list of paths without spaces
gh release create "ffmpeg-$tag" --repo "$repo" --latest=false \
    --title "FFmpeg $tag" --notes "$notes" $files
echo "==> published ffmpeg-$tag on $repo"
