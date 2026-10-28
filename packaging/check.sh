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
grep -q '^X-KDE-DBUS-Restricted-Interfaces=.*org\.kde\.KWin\.ScreenShot2' "$desktop" ||
    fail "the desktop entry does not list org.kde.KWin.ScreenShot2 as a restricted interface KDE may grant"
grep -q "sed -i 's|^Exec=windup|Exec=/usr/bin/windup|'" packaging/linux/package.sh ||
    fail "package.sh does not give the system packages an absolute Exec (KWin needs it)"
ok "desktop entry asks KWin for ScreenShot2, with an absolute Exec in system packages"

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

# The archive enrolment (Spillebulle/packages ENROLMENT.md). The fingerprint
# the Arch scriptlet trusts must be the key the packages carry, and every
# path must be the contract's: a mismatch installs fine and silently never
# updates.
fpr=$(sed -n 's/^ *local fpr=\([0-9A-F]*\)$/\1/p' packaging/linux/windup.install)
if command -v gpg >/dev/null 2>&1; then
    key_fpr=$(gpg --show-keys --with-colons packaging/linux/spillebulle-archive.asc 2>/dev/null |
        awk -F: '/^fpr:/ { print $10; exit }')
    [ "$fpr" = "$key_fpr" ] || fail "windup.install trusts $fpr, the key is $key_fpr"
fi
grep -q 'pacman-key --add /usr/share/windup/spillebulle-archive.asc' packaging/linux/windup.install ||
    fail "windup.install does not add the key the package ships"
grep -q 'share/windup/$ARCHIVE_KEYRING.asc' packaging/linux/package.sh ||
    fail "the Arch package does not ship /usr/share/windup/spillebulle-archive.asc"
grep -q 'install -m644 /usr/share/windup/$ARCHIVE_KEYRING.gpg' packaging/linux/package.sh ||
    fail "the postinst does not copy the key the .deb ships"
if grep -q 'deb/usr/share/keyrings/' packaging/linux/package.sh; then
    fail "the .deb ships /usr/share/keyrings/, which a second application's .deb would conflict with"
fi
ok "archive enrolment matches ENROLMENT.md"
