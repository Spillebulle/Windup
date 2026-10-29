# Sourced (not run) by the AppImage's AppRun before Windup starts; package.sh
# puts it in AppDir/apprun-hooks/, which linuxdeploy's AppRun reads.
#
# Windup links libpipewire, which only recording on KDE and GNOME uses. A
# desktop without PipeWire (a window manager on PulseAudio, say) has no
# libpipewire-0.3.so.0, and without one Windup would not start at all. The
# AppImage carries Ubuntu's copy for that case only: a host with PipeWire
# keeps its own, whose modules match it.

windup_host_has_pipewire() {
    for ldconfig in ldconfig /sbin/ldconfig /usr/sbin/ldconfig; do
        if command -v "$ldconfig" >/dev/null 2>&1; then
            if "$ldconfig" -p 2>/dev/null | grep -q 'libpipewire-0\.3\.so\.0'; then
                return 0
            fi
            break
        fi
    done
    # Distributions without ldconfig on the PATH, or with a stale cache.
    for dir in /usr/lib64 /usr/lib /lib64 /lib \
        /usr/lib/x86_64-linux-gnu /lib/x86_64-linux-gnu \
        /usr/lib/aarch64-linux-gnu /lib/aarch64-linux-gnu /usr/local/lib; do
        if [ -e "$dir/libpipewire-0.3.so.0" ]; then
            return 0
        fi
    done
    return 1
}

if [ -n "${APPDIR:-}" ] && ! windup_host_has_pipewire; then
    LD_LIBRARY_PATH="$APPDIR/usr/lib/pipewire-fallback${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    export LD_LIBRARY_PATH
fi
unset -f windup_host_has_pipewire
