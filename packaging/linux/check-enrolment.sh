#!/bin/sh
# Installs a package in a clean container and checks that it enrolled the
# machine in the Spillebulle archive exactly as Spillebulle/packages
# ENROLMENT.md says, then asks the live archive for its index with the key the
# package installed, which is the only proof that the key and paths work.
#
#   check-enrolment.sh deb|deb-legacy|rpm|arch <dist-dir>
#
# Run inside debian, fedora or archlinux containers by the CI's Linux
# packages job, with the packages mounted at <dist-dir>.

set -eu
mode=$1
dist=$2
fpr=7A9EA69E033F3C3EF4F53EB4A72BC0FD5E061ADA
fail() { echo "FAIL: $*" >&2; exit 1; }

case $mode in
deb)
    apt-get update -qq
    apt-get install -y -qq "$dist"/windup_*.deb >/dev/null
    windup --help >/dev/null
    test -s /etc/apt/keyrings/spillebulle-archive.gpg || fail "no shared keyring"
    grep -qx 'Signed-By: /etc/apt/keyrings/spillebulle-archive.gpg' \
        /etc/apt/sources.list.d/spillebulle.sources || fail "source missing or wrong"
    test ! -e /usr/share/keyrings/spillebulle-archive.gpg || fail "still ships the shared path"

    # A second application enrolling beside it: the defect ENROLMENT.md exists
    # for only shows when two are installed. Windup's own package, renamed and
    # holding only its key, under the second application's name.
    work=$(mktemp -d)
    dpkg-deb -R "$dist"/windup_*.deb "$work/w"
    mkdir -p "$work/s/DEBIAN" "$work/s/usr/share/second"
    cp "$work/w/usr/share/windup/spillebulle-archive.gpg" "$work/s/usr/share/second/"
    sed 's|/usr/share/windup/|/usr/share/second/|g' "$work/w/DEBIAN/postinst" >"$work/s/DEBIAN/postinst"
    chmod 755 "$work/s/DEBIAN/postinst"
    printf '%s\n' 'Package: second' 'Version: 1' 'Architecture: all' \
        'Maintainer: test <test@example.com>' 'Description: a second enrolling package' \
        >"$work/s/DEBIAN/control"
    dpkg-deb --build --root-owner-group "$work/s" "$work/second.deb" >/dev/null
    dpkg -i "$work/second.deb" >/dev/null || fail "a second enrolling package does not install"

    apt-get -o APT::Update::Error-Mode=any update >/dev/null || fail "the archive does not verify"
    ;;
deb-legacy)
    # A machine enrolled by a package from before ENROLMENT.md: its source
    # names the old keyring path, which the upgrade removes.
    printf '%s\n' 'Types: deb' 'URIs: https://spillebulle.github.io/packages/deb/' \
        'Suites: ./' 'Signed-By: /usr/share/keyrings/spillebulle-archive.gpg' \
        >/etc/apt/sources.list.d/spillebulle.sources
    # Fails for the archive (its key is not there yet) but fetches Debian's.
    apt-get update -qq 2>/dev/null || true
    apt-get install -y -qq "$dist"/windup_*.deb >/dev/null
    grep -qx 'Signed-By: /etc/apt/keyrings/spillebulle-archive.gpg' \
        /etc/apt/sources.list.d/spillebulle.sources || fail "old Signed-By not migrated"
    apt-get -o APT::Update::Error-Mode=any update >/dev/null || fail "the archive does not verify"
    ;;
rpm)
    dnf install -y -q "$dist"/windup-*.rpm
    windup --help >/dev/null
    test -s /etc/pki/rpm-gpg/RPM-GPG-KEY-spillebulle || fail "no key"
    grep -q '^gpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-spillebulle$' \
        /etc/yum.repos.d/spillebulle.repo || fail "repo file missing or wrong"
    dnf -y -q makecache --repo spillebulle || fail "the archive does not verify"
    ;;
arch)
    # The archlinux image ships its keyring without the local master key,
    # which every installed system has (`pacman-key --init` at install time)
    # and which a local signature needs.
    pacman-key --init >/dev/null 2>&1
    pacman -Sy --noconfirm --needed archlinux-keyring >/dev/null
    pacman -U --noconfirm "$dist"/windup-bin-*.pkg.tar.zst >/dev/null
    windup --help >/dev/null
    test "$(grep -c '^\[spillebulle\]' /etc/pacman.conf)" = 1 || fail "no [spillebulle] section"
    grep -qx 'Include = /etc/pacman.d/spillebulle' /etc/pacman.conf || fail "no Include"
    grep -qx 'SigLevel = Required DatabaseRequired' /etc/pacman.d/spillebulle || fail "settings"
    pacman-key --list-keys "$fpr" >/dev/null 2>&1 || fail "key not in pacman's keyring"
    # An upgrade runs the scriptlet again; nothing may be added twice.
    pacman -U --noconfirm "$dist"/windup-bin-*.pkg.tar.zst >/dev/null
    test "$(grep -c '^\[spillebulle\]' /etc/pacman.conf)" = 1 || fail "section added twice"
    pacman -Sy --noconfirm >/dev/null || fail "the archive does not verify"
    ;;
*)
    echo "usage: check-enrolment.sh deb|deb-legacy|rpm|arch <dist-dir>" >&2
    exit 2
    ;;
esac
echo "ok: $mode enrols the machine"
