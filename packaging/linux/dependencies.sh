#!/bin/sh
# The system libraries building Windup needs on Debian or Ubuntu, for CI and
# for a developer setting up a machine.
#
#   sh packaging/linux/dependencies.sh
#
# winit's Wayland and X11 backends, libxkbcommon (linked: the Wayland popup's
# keyboard), GTK 3 for rfd's file dialogs, and libxdo for enigo's simulated
# paste on X11. packaging/linux/package.sh turns the build into packages.

set -eu

sudo apt-get update
sudo apt-get install -y \
    libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev \
    libx11-dev libxrandr-dev libxi-dev libxcursor-dev \
    libgtk-3-dev libxdo-dev
