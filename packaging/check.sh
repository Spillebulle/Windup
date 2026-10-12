#!/bin/sh
# Checks that the Linux packaging metadata agrees with itself.
#
#   sh packaging/check.sh
#
# The desktop entry, the AppStream component, the icons and the Wayland app id
# the program sets must all be one name. `appstreamcli` follows the
# component's launchable to the desktop entry and its Icon key to the icon,
# and drops the whole component when a hop misses; a compositor matches the
# window to the desktop entry (and so to its icon) by app id. Nothing else
# notices either mismatch.

set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() { printf 'packaging: %s\n' "$1" >&2; exit 1; }
ok()   { printf '  ok  %s\n' "$1"; }

APP_ID=io.github.spillebulle.windup
metainfo="packaging/$APP_ID.metainfo.xml"
desktop="packaging/$APP_ID.desktop"

[ -f "$metainfo" ] || fail "no AppStream file at $metainfo"
[ -f "$desktop" ] || fail "no desktop entry at $desktop"
ok "both files are named for $APP_ID"

id=$(sed -n 's/.*<id>\(.*\)<\/id>.*/\1/p' "$metainfo" | head -1)
[ "$id" = "$APP_ID" ] || fail "<id> is '$id', expected '$APP_ID'"
launchable=$(sed -n 's/.*<launchable[^>]*>\(.*\)<\/launchable>.*/\1/p' "$metainfo" | head -1)
[ "$launchable" = "$APP_ID.desktop" ] || fail "<launchable> is '$launchable'"
ok "AppStream id and launchable"

icon=$(sed -n 's/^Icon=\(.*\)$/\1/p' "$desktop" | head -1)
[ "$icon" = "$APP_ID" ] || fail "Icon is '$icon', the icons install as $APP_ID.png"
wmclass=$(sed -n 's/^StartupWMClass=\(.*\)$/\1/p' "$desktop" | head -1)
[ "$wmclass" = "$APP_ID" ] || fail "StartupWMClass is '$wmclass'"
ok "desktop entry Icon and StartupWMClass"

grep -q "const APP_ID: &str = \"$APP_ID\";" crates/windup-desktop/src/main.rs ||
    fail "main.rs does not set the window's app id to $APP_ID"
grep -q "pub const APP_ID: &str = \"$APP_ID\";" crates/windup-desktop/src/linux/autostart.rs ||
    fail "the autostart entry does not use $APP_ID"
ok "the program uses $APP_ID for its window and its autostart entry"

for size in 16 32 48 64 128 256; do
    [ -f "assets/icons/hicolor/$size.png" ] ||
        fail "assets/icons/hicolor/$size.png is missing (cargo run -p windup-ui --example make-art)"
done
ok "all six icon sizes are present"

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
grep -q "<release version=\"$version\"" "$metainfo" ||
    fail "the AppStream file has no <release> for $version"
ok "AppStream lists release $version"
