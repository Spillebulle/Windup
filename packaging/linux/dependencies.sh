#!/bin/sh
# The system libraries building Windup needs on Debian or Ubuntu, for CI and
# for a developer setting up a machine.
#
#   sh packaging/linux/dependencies.sh
#
# winit's Wayland and X11 backends, libxkbcommon (linked: the Wayland popup's
# keyboard), GTK 3 for rfd's file dialogs, libxdo for enigo's simulated
# paste on X11, and libpipewire with libclang for the `pipewire` feature
# (screen recording on KDE and GNOME; bindgen reads PipeWire's headers).
# packaging/linux/package.sh turns the build into packages.

set -eu

sudo apt-get update
sudo apt-get install -y \
    libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev \
    libx11-dev libxrandr-dev libxi-dev libxcursor-dev \
    libgtk-3-dev libxdo-dev \
    libpipewire-0.3-dev libclang-dev

# The PipeWire bindings use a field (spa_video_info_raw.flags) that SPA's
# headers gained in 0.3.65; Ubuntu 22.04, which releases are built on for its
# older glibc, has 0.3.48. SPA is headers only, so newer ones change nothing
# at run time: Windup still links the system's libpipewire, and runs with
# any 0.3. They go in /usr/local, which pkg-config searches first.
if ! pkg-config --atleast-version=0.3.65 libspa-0.2; then
    spa_tag=0.3.65
    spa_commit=9558a5d5e0360d8af822431c76ee858a8c7495ac
    spa_src=$(mktemp -d)
    git clone -q --depth 1 --branch "$spa_tag" --filter=blob:none --sparse \
        https://gitlab.freedesktop.org/pipewire/pipewire.git "$spa_src"
    [ "$(git -C "$spa_src" rev-parse HEAD)" = "$spa_commit" ] ||
        { echo "PipeWire's $spa_tag tag is not the commit expected" >&2; exit 1; }
    git -C "$spa_src" sparse-checkout set spa/include
    sudo mkdir -p /usr/local/include/spa-0.2 /usr/local/lib/pkgconfig
    sudo cp -r "$spa_src/spa/include/spa" /usr/local/include/spa-0.2/
    printf '%s\n' \
        'includedir=/usr/local/include/spa-0.2' \
        'Name: libspa' \
        "Description: PipeWire's SPA headers, $spa_tag (packaging/linux/dependencies.sh)" \
        "Version: $spa_tag" \
        'Cflags: -I${includedir} -D_REENTRANT' |
        sudo tee /usr/local/lib/pkgconfig/libspa-0.2.pc >/dev/null
    rm -rf "$spa_src"
fi
