#!/bin/bash
# End-to-end test of Windup on Linux, against real compositors with no
# screen: a headless Sway (Wayland: layer-shell, data-control, virtual
# keyboard, the same protocols COSMIC offers) and Xvfb (X11).
#
#   tools/linux-e2e.sh <path-to-windup> [wayland|x11|all] [artefact-dir]
#
# Needs, on Debian/Ubuntu: sway wl-clipboard wtype grim zenity xvfb xdotool
# xclip x11-utils openbox imagemagick mesa-vulkan-drivers. CI installs them;
# see the e2e job in ci.yml.
#
# Each case copies something, drives Windup the way a person would (a
# shortcut runs `windup --popup`, keys pick an entry), and checks what landed
# in a real GTK text field or in the history file. Screenshots and logs go to
# the artefact directory.

set -uo pipefail

bin=$(realpath "${1:?usage: linux-e2e.sh <windup> [wayland|x11|all] [artefacts]}")
which=${2:-all}
art=$(mkdir -p "${3:-e2e-artefacts}" && realpath "${3:-e2e-artefacts}")

pass=0
fail=0
ok() { echo "  ok    $*"; pass=$((pass + 1)); }
bad() { echo "  FAIL  $*"; fail=$((fail + 1)); }

# A fresh home for every session, so config and history start empty.
fresh_home() {
    home=$(mktemp -d)
    export HOME=$home XDG_CONFIG_HOME=$home/.config XDG_DATA_HOME=$home/.local/share
    history=$XDG_CONFIG_HOME/windup/clipboard_history.json
    runtime=$(mktemp -d)
    chmod 700 "$runtime"
    export XDG_RUNTIME_DIR=$runtime
}

# Waits up to $1 tenths of a second for the command after it to succeed.
wait_for() {
    local tries=$1
    shift
    for _ in $(seq "$tries"); do
        "$@" >/dev/null 2>&1 && return 0
        sleep 0.1
    done
    return 1
}

history_has() { [ -f "$history" ] && grep -q -- "$1" "$history"; }

# Starts a GTK text field and prints what it held when Enter was pressed.
# Its window takes focus, which is what a paste goes back to.
field() {
    zenity --entry --title=target --text=target >"$1" 2>/dev/null &
    field_pid=$!
}

start_windup() {
    RUST_LOG=info,wgpu_hal=error,wgpu_core=error,naga=error \
        "$bin" --background >"$art/$1-windup.log" 2>&1 &
    windup_pid=$!
    if wait_for 100 test -S "$(ls "$XDG_RUNTIME_DIR"/windup*.sock 2>/dev/null | head -1)"; then
        ok "$1: Windup started and is listening"
    else
        bad "$1: Windup never opened its socket"
        cat "$art/$1-windup.log"
        return 1
    fi
}

# --- Wayland ----------------------------------------------------------------

wayland_session() {
    echo "== Wayland (headless sway)"
    fresh_home
    unset DISPLAY
    cat >"$home/sway.conf" <<'EOF'
output HEADLESS-1 resolution 1600x1000 position 0 0
default_border none
EOF
    WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman \
        sway -c "$home/sway.conf" >"$art/wayland-sway.log" 2>&1 &
    sway_pid=$!
    if ! wait_for 100 sh -c 'ls "$XDG_RUNTIME_DIR"/wayland-* | grep -qv lock'; then
        bad "wayland: sway did not start"
        cat "$art/wayland-sway.log"
        return
    fi
    WAYLAND_DISPLAY=$(basename "$(ls "$XDG_RUNTIME_DIR"/wayland-* | grep -v lock | head -1)")
    export WAYLAND_DISPLAY
    export SWAYSOCK=$(ls "$XDG_RUNTIME_DIR"/sway-ipc.*.sock 2>/dev/null | head -1)
    export GDK_BACKEND=wayland

    start_windup wayland || { kill $sway_pid; return; }

    if grep -q "host: Layer" "$art/wayland-windup.log"; then
        ok "wayland: chose the layer-shell host"
    else
        bad "wayland: did not choose the layer-shell host"
        grep -i backend "$art/wayland-windup.log"
    fi

    # A copy is recorded without Windup having focus or a window.
    wl-copy "alpha-$$"
    if wait_for 30 history_has "alpha-$$"; then
        ok "wayland: copied text is recorded"
    else
        bad "wayland: copied text was not recorded"
    fi

    # An image too.
    convert -size 8x6 xc:red "$home/red.png"
    wl-copy --type image/png <"$home/red.png"
    if wait_for 30 history_has '"Image"'; then
        ok "wayland: copied image is recorded"
    else
        bad "wayland: copied image was not recorded"
    fi

    # Popup, pick the second entry, and it is pasted into the focused field.
    wl-copy "first-$$"
    wait_for 30 history_has "first-$$"
    wl-copy "second-$$"
    wait_for 30 history_has "second-$$"
    field "$home/field1"
    sleep 1.5
    "$bin" --popup
    sleep 0.8
    grim "$art/wayland-popup.png" 2>/dev/null && ok "wayland: screenshot of the popup taken"
    wtype -k Down
    sleep 0.2
    wtype -k Return
    sleep 0.8
    wtype -k Return
    wait $field_pid 2>/dev/null
    got=$(cat "$home/field1")
    if [ "$got" = "first-$$" ]; then
        ok "wayland: popup pasted the picked entry into the focused window"
    else
        bad "wayland: popup paste gave '$got', wanted 'first-$$'"
    fi

    # Escape closes the popup and leaves the clipboard alone.
    wl-copy "keep-$$"
    sleep 0.4
    "$bin" --popup
    sleep 0.6
    wtype -k Escape
    sleep 0.4
    if [ "$(wl-paste -n)" = "keep-$$" ]; then
        ok "wayland: Escape dismissed the popup without touching the clipboard"
    else
        bad "wayland: clipboard changed after Escape: '$(wl-paste -n)'"
    fi

    # Paste as plain text, from the shortcut, into the focused window.
    printf 'plain-%s' "$$" | wl-copy --type text/plain
    sleep 0.4
    field "$home/field2"
    sleep 1.5
    "$bin" --paste-plain
    sleep 0.8
    wtype -k Return
    wait $field_pid 2>/dev/null
    got=$(cat "$home/field2")
    if [ "$got" = "plain-$$" ]; then
        ok "wayland: paste-plain pasted into the focused window"
    else
        bad "wayland: paste-plain gave '$got', wanted 'plain-$$'"
    fi

    # A second `windup` does not start a second copy; it opens settings.
    "$bin"
    if [ $? -eq 0 ] && [ "$(pgrep -c -f "$bin --background")" -eq 1 ]; then
        ok "wayland: a second launch handed over to the first"
    else
        bad "wayland: a second launch did not hand over"
    fi
    if wait_for 80 sh -c 'swaymsg -t get_tree | grep -q "\"app_id\": \"io.github.spillebulle.windup\""'; then
        ok "wayland: the settings window opened"
        grim "$art/wayland-settings.png" 2>/dev/null
    else
        bad "wayland: the settings window did not open"
    fi

    # Survives the settings window closing.
    pkill -f "$bin --settings"
    sleep 0.3
    wl-copy "after-$$"
    if wait_for 30 history_has "after-$$" && kill -0 $windup_pid 2>/dev/null; then
        ok "wayland: keeps running after the settings window closes"
    else
        bad "wayland: stopped with the settings window"
    fi

    kill $windup_pid $sway_pid 2>/dev/null
    wait 2>/dev/null
}

# --- X11 --------------------------------------------------------------------

x11_session() {
    echo "== X11 (Xvfb + openbox)"
    fresh_home
    unset WAYLAND_DISPLAY
    export DISPLAY=:77 GDK_BACKEND=x11
    Xvfb $DISPLAY -screen 0 1600x1000x24 >"$art/x11-xvfb.log" 2>&1 &
    xvfb_pid=$!
    wait_for 50 xdpyinfo || { bad "x11: Xvfb did not start"; return; }
    openbox >"$art/x11-openbox.log" 2>&1 &
    wm_pid=$!
    sleep 0.5

    start_windup x11 || { kill $wm_pid $xvfb_pid; return; }

    printf 'xalpha-%s' "$$" | xclip -selection clipboard
    if wait_for 30 history_has "xalpha-$$"; then
        ok "x11: copied text is recorded"
    else
        bad "x11: copied text was not recorded"
    fi

    printf 'xfirst-%s' "$$" | xclip -selection clipboard
    wait_for 30 history_has "xfirst-$$"
    printf 'xsecond-%s' "$$" | xclip -selection clipboard
    wait_for 30 history_has "xsecond-$$"
    field "$home/xfield1"
    sleep 1.5
    xdotool key super+v
    sleep 1
    import -window root "$art/x11-popup.png" 2>/dev/null && ok "x11: screenshot of the popup taken"
    xdotool key Down
    sleep 0.2
    xdotool key Return
    sleep 0.8
    xdotool key Return
    wait $field_pid 2>/dev/null
    got=$(cat "$home/xfield1")
    if [ "$got" = "xfirst-$$" ]; then
        ok "x11: Super+V popup pasted the picked entry into the focused window"
    else
        bad "x11: popup paste gave '$got', wanted 'xfirst-$$'"
    fi

    # The command route works on X11 too.
    printf 'xplain-%s' "$$" | xclip -selection clipboard
    sleep 0.4
    field "$home/xfield2"
    sleep 1.5
    "$bin" --paste-plain
    sleep 0.8
    xdotool key Return
    wait $field_pid 2>/dev/null
    got=$(cat "$home/xfield2")
    if [ "$got" = "xplain-$$" ]; then
        ok "x11: paste-plain pasted into the focused window"
    else
        bad "x11: paste-plain gave '$got', wanted 'xplain-$$'"
    fi

    kill $windup_pid $wm_pid $xvfb_pid 2>/dev/null
    wait 2>/dev/null
}

case $which in
    wayland) wayland_session ;;
    x11) x11_session ;;
    all) wayland_session; x11_session ;;
    *) echo "unknown session '$which'" >&2; exit 2 ;;
esac

echo
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
