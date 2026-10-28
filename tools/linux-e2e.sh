#!/bin/bash
# End-to-end test of Windup on Linux, against real compositors with no
# screen: a headless Sway (Wayland: layer-shell, data-control, virtual
# keyboard, the same protocols COSMIC offers), a headless Weston (Wayland
# with none of them, as on GNOME) and Xvfb (X11).
#
#   tools/linux-e2e.sh <path-to-windup> [wayland|weston|x11|all] [artefact-dir]
#
# Needs, on Debian/Ubuntu: sway swaybg wl-clipboard wtype grim zenity xvfb xdotool
# xclip x11-utils openbox imagemagick mesa-vulkan-drivers python3-gi
# gir1.2-gtk-4.0 weston xwayland. CI installs them;
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

# Waits for the popup to reach the screen for the $1th time. The first one
# creates the GPU device, which on a software renderer takes a while.
popup_shown() {
    wait_for 150 sh -c '[ "$(grep -c "popup: shown" "$1")" -ge "$2" ]' _ "$art/wayland-windup.log" "$1" ||
        echo "  (popup $1 was not shown in time)"
    sleep 0.3
}

# The colour of the pixel at $2,$3 of the picture $1, as "r,g,b".
pixel() {
    convert "$1" -format '%[fx:int(255*r)],%[fx:int(255*g)],%[fx:int(255*b)]' \
        -crop "1x1+$2+$3" info: 2>/dev/null
}

# The width x height in the header of the PNG $1.
png_size() {
    od -An -tu1 -j16 -N8 "$1" |
        awk '{ printf "%dx%d", (($1*256+$2)*256+$3)*256+$4, (($5*256+$6)*256+$7)*256+$8 }'
}

# Types into the popup: wtype arguments, run after a $1 ms pause. Started
# before the popup opens, because Sway hands keyboard focus back to the
# focused window whenever a keyboard device appears, and each wtype is a new
# device. A wtype started after the popup would take its focus away.
keys_later() {
    wtype -s "$@" &
    keys_pid=$!
    sleep 0.3
}

# Copies $2 as a password manager would, and checks it stays out of the
# history while the copy is really marked (the case proves nothing if not).
secret_case() {
    local session=$1 secret=$2 types
    # Without data-control, Windup reads the X clipboard; copy there.
    local backend=$GDK_BACKEND
    [ "$session" = weston ] && backend=x11
    GDK_BACKEND=$backend python3 "$(dirname "$0")/secret-copy.py" "$secret" 3 >"$art/$session-secret.log" 2>&1 &
    local pid=$!
    if [ "$session" = wayland ]; then
        # GTK claims the Wayland clipboard with the serial of the last key
        # press; give it one.
        wait_for 80 grep -q "copied (timer)" "$art/$session-secret.log"
        wtype -k space
        local copied="copied (key)"
    else
        local copied="copied (timer)"
    fi
    if ! wait_for 80 grep -qF "$copied" "$art/$session-secret.log"; then
        bad "$session: the secret copy helper did not copy"
        cat "$art/$session-secret.log"
        kill $pid 2>/dev/null
        return
    fi
    if [ "$session" = wayland ]; then
        types=$(wl-paste --list-types)
    else
        types=$(xclip -o -selection clipboard -t TARGETS)
    fi
    sleep 1.5
    local later
    if [ "$session" = wayland ]; then
        later=$(wl-paste --list-types)
    else
        later=$(xclip -o -selection clipboard -t TARGETS)
    fi
    if ! printf '%s' "$types" | grep -q x-kde-passwordManagerHint; then
        bad "$session: the secret copy was not marked: $types"
    elif ! printf '%s' "$later" | grep -q x-kde-passwordManagerHint; then
        # Weston's clipboard module replaces every selection with its own
        # text-only copy at once, before a 250 ms poll can see the mark.
        # Mutter and Klipper only take over when the owner quits, which is
        # the case below. Known; see docs/linux.md.
        echo "  skip  $session: the compositor replaced the marked copy at once (offers now: $(echo $later))"
        wait $pid 2>/dev/null
        return
    elif history_has "$secret"; then
        bad "$session: a password manager's copy was recorded"
    else
        ok "$session: a password manager's copy is not recorded"
    fi
    # A clipboard manager keeps the text when the password manager quits
    # and offers it again, unmarked.
    wait $pid 2>/dev/null
    sleep 1.5
    if history_has "$secret"; then
        bad "$session: the password was recorded once its owner quit"
    else
        ok "$session: nor once the password manager quits"
    fi
}

history_has() { [ -f "$history" ] && grep -q -- "$1" "$history"; }

# Starts a GTK text field and prints what it held when Enter was pressed.
# Its window takes focus, which is what a paste goes back to.
field() {
    zenity --entry --title=target --text=target >"$1" 2>>"$art/field.log" &
    field_pid=$!
    if [ -n "${SWAYSOCK:-}" ]; then
        wait_for 100 sh -c 'swaymsg -t get_tree | grep -q "\"name\": \"target\""' ||
            echo "  (the text field did not appear; see field.log)"
    fi
    sleep 0.5
}

# Waits up to five seconds for the field to close, then closes it, so a paste
# that never arrived fails the case instead of hanging the run.
field_done() {
    for _ in $(seq 50); do
        kill -0 "$field_pid" 2>/dev/null || break
        sleep 0.1
    done
    kill "$field_pid" 2>/dev/null
    wait "$field_pid" 2>/dev/null
}

start_windup() {
    RUST_LOG=info,wgpu_hal=error,wgpu_core=error,naga=error \
        "$bin" --background >"$art/$1-windup.log" 2>&1 &
    windup_pid=$!
    # The glob is evaluated on every try: the socket does not exist yet.
    if wait_for 100 sh -c 'for s in "$XDG_RUNTIME_DIR"/windup*.sock; do [ -S "$s" ] && exit 0; done; exit 1'; then
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
output HEADLESS-1 resolution 1600x1000 position 0 0 bg #3a6ea5 solid_color
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
    # GTK 4 wants a GPU for its default renderer; there is none here.
    export GSK_RENDERER=cairo

    start_windup wayland || { kill $sway_pid; return; }

    # The socket is bound before the backend is chosen and logged.
    if wait_for 150 grep -q "host: Layer" "$art/wayland-windup.log"; then
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

    secret_case wayland "hunter2-$$"

    # The first popup creates the GPU device, which on a software renderer
    # takes seconds; get that out of the way so the timed keys below land.
    "$bin" --popup
    popup_shown 1
    "$bin" --popup
    wait_for 50 grep -q "popup: closed" "$art/wayland-windup.log"

    # Popup, pick the second entry, and it is pasted into the focused field.
    wl-copy "first-$$"
    wait_for 30 history_has "first-$$"
    wl-copy "second-$$"
    wait_for 30 history_has "second-$$"
    field "$home/field1"
    grim "$art/wayland-field.png" 2>/dev/null
    keys_later 3000 -k Down -s 200 -k Return
    "$bin" --popup
    popup_shown 2
    grim "$art/wayland-popup.png" 2>/dev/null && ok "wayland: screenshot of the popup taken"
    # The popup covers the output to learn where the pointer is. Around the
    # card the window underneath must still show: on drivers whose GPU
    # surfaces cannot be transparent, and from the second popup on anywhere,
    # it went black. The card itself, in the middle, must show too.
    under=$(pixel "$art/wayland-field.png" 10 990)
    around=$(pixel "$art/wayland-popup.png" 10 990)
    if [ -z "$under" ] || [ "$under" = "0,0,0" ]; then
        echo "  (no window colour to compare the popup with: '$under')"
    elif [ "$around" = "$under" ]; then
        ok "wayland: the window under the popup still shows around the card"
    else
        bad "wayland: around the card the screen is $around, the window under it $under"
    fi
    if [ "$(pixel "$art/wayland-popup.png" 800 500)" != "$(pixel "$art/wayland-field.png" 800 500)" ]; then
        ok "wayland: the card is drawn"
    else
        bad "wayland: no card in the middle of the screen"
    fi
    wait $keys_pid
    sleep 0.8
    wtype -k Return
    field_done
    got=$(cat "$home/field1")
    if [ "$got" = "first-$$" ]; then
        ok "wayland: popup pasted the picked entry into the focused window"
    else
        bad "wayland: popup paste gave '$got', wanted 'first-$$'"
    fi

    # Escape closes the popup and leaves the clipboard alone.
    wl-copy "keep-$$"
    sleep 0.4
    keys_later 3000 -k Escape
    "$bin" --popup
    popup_shown 3
    wait $keys_pid
    sleep 0.4
    if [ "$(wl-paste -n)" = "keep-$$" ]; then
        ok "wayland: Escape dismissed the popup without touching the clipboard"
    else
        bad "wayland: clipboard changed after Escape: '$(wl-paste -n)'"
    fi

    # Both popups closed by their own keys, not by losing focus.
    if grep -q "lost the keyboard" "$art/wayland-windup.log"; then
        bad "wayland: a popup lost the keyboard while open"
    else
        ok "wayland: the popup kept the keyboard until it was done"
    fi

    # Paste as plain text, from the shortcut, into the focused window.
    printf 'plain-%s' "$$" | wl-copy --type text/plain
    sleep 0.4
    field "$home/field2"
    sleep 1.5
    "$bin" --paste-plain
    sleep 0.8
    grim "$art/wayland-paste-plain.png" 2>/dev/null
    wtype -k Return
    field_done
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

# --- Wayland without layer-shell ----------------------------------------------
#
# Weston offers neither layer-shell nor data-control nor a virtual keyboard,
# like GNOME: Windup keeps no window, reads the clipboard through XWayland
# and opens the popup as a window of its own. Weston has no way to inject
# keys, so this checks the parts that differ: the backend, the copy, and
# that the shortcut opens and closes the popup while nothing else is shown.

weston_session() {
    echo "== Wayland without layer-shell (headless Weston + XWayland)"
    fresh_home
    unset DISPLAY SWAYSOCK
    weston --backend=headless --renderer=pixman --width=1600 --height=1000 \
        --socket=wayland-w --xwayland >"$art/weston.log" 2>&1 &
    weston_pid=$!
    if ! wait_for 100 test -S "$XDG_RUNTIME_DIR/wayland-w"; then
        bad "weston: did not start"
        cat "$art/weston.log"
        return
    fi
    export WAYLAND_DISPLAY=wayland-w GDK_BACKEND=wayland
    # Weston starts XWayland on the first X client; it names the display
    # in its log as soon as it listens.
    wait_for 50 grep -q "xserver listening on display" "$art/weston.log"
    DISPLAY=$(grep -o "listening on display :[0-9]*" "$art/weston.log" | grep -o ":[0-9]*" | head -1)
    export DISPLAY

    start_windup weston || { kill $weston_pid; return; }
    log=$art/weston-windup.log

    if wait_for 150 grep -q "host: Window" "$log"; then
        ok "weston: chose a window of its own for the popup"
    else
        bad "weston: did not choose the window host"
        grep -i backend "$log"
    fi

    printf 'walpha-%s' "$$" | xclip -selection clipboard
    if wait_for 30 history_has "walpha-$$"; then
        ok "weston: copied text is recorded through XWayland"
    else
        bad "weston: copied text was not recorded"
    fi

    secret_case weston "whunter2-$$"

    # Windup draws nothing until asked, then the popup has to come up and
    # take the second press of the shortcut as "close".
    "$bin" --popup
    if wait_for 150 grep -q "popup window: shown" "$log"; then
        ok "weston: the shortcut opened the popup window"
    else
        bad "weston: the popup window did not open"
    fi
    "$bin" --popup
    if wait_for 50 grep -q "popup window: closed" "$log"; then
        ok "weston: the shortcut again closed it"
    else
        bad "weston: the popup window did not close"
    fi
    "$bin" --popup
    if wait_for 150 sh -c '[ "$(grep -c "popup window: shown" "$1")" -ge 2 ]' _ "$log"; then
        ok "weston: and opens it again"
    else
        bad "weston: the popup window did not open a second time"
    fi
    "$bin" --popup

    "$bin"
    if wait_for 80 sh -c 'pgrep -f -- "--settings" >/dev/null'; then
        ok "weston: a second launch opened the settings window"
    else
        bad "weston: the settings window did not open"
    fi
    pkill -f -- "$bin --settings"
    if kill -0 $windup_pid 2>/dev/null; then
        ok "weston: keeps running"
    else
        bad "weston: stopped"
    fi

    kill $windup_pid $weston_pid 2>/dev/null
    wait 2>/dev/null
}

# --- X11 --------------------------------------------------------------------

x11_session() {
    echo "== X11 (Xvfb + openbox)"
    fresh_home
    unset WAYLAND_DISPLAY SWAYSOCK GSK_RENDERER
    export DISPLAY=:77 GDK_BACKEND=x11
    local screen=1600x1000
    Xvfb $DISPLAY -screen 0 ${screen}x24 >"$art/x11-xvfb.log" 2>&1 &
    xvfb_pid=$!
    wait_for 50 xdpyinfo || { bad "x11: Xvfb did not start"; return; }
    openbox >"$art/x11-openbox.log" 2>&1 &
    wm_pid=$!
    sleep 0.5

    # The capture back end: the whole root window as one PNG of its size.
    if "$bin" --capture-test "$art/x11.png" >"$art/x11-capture.log" 2>&1; then
        local size
        size=$(png_size "$art/x11.png")
        if [ "$size" = "$screen" ]; then
            ok "x11: --capture-test wrote the whole screen ($size)"
        else
            bad "x11: --capture-test wrote $size, wanted $screen"
        fi
    else
        bad "x11: --capture-test failed (see x11-capture.log)"
    fi

    start_windup x11 || { kill $wm_pid $xvfb_pid; return; }

    printf 'xalpha-%s' "$$" | xclip -selection clipboard
    if wait_for 30 history_has "xalpha-$$"; then
        ok "x11: copied text is recorded"
    else
        bad "x11: copied text was not recorded"
    fi

    secret_case x11 "xhunter2-$$"

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
    field_done
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
    field_done
    got=$(cat "$home/xfield2")
    if [ "$got" = "xplain-$$" ]; then
        ok "x11: paste-plain pasted into the focused window"
    else
        bad "x11: paste-plain gave '$got', wanted 'xplain-$$'"
    fi

    # Started in the background, the window is hidden; `windup` shows it,
    # closing it hides it again without stopping Windup, and it comes back.
    visible() { xdotool search --onlyvisible --name '^Windup$' >/dev/null 2>&1; }
    "$bin"
    if wait_for 50 visible; then
        ok "x11: a second launch showed the window"
    else
        bad "x11: a second launch did not show the window"
    fi
    xdotool search --onlyvisible --name '^Windup$' windowactivate --sync key --clearmodifiers alt+F4 2>/dev/null
    if wait_for 30 sh -c '! xdotool search --onlyvisible --name "^Windup$" >/dev/null 2>&1' &&
        kill -0 $windup_pid 2>/dev/null; then
        ok "x11: closing the window hid it and Windup kept running"
    else
        bad "x11: closing the window did not hide it, or Windup stopped"
    fi
    "$bin"
    if wait_for 50 visible; then
        ok "x11: and it comes back"
    else
        bad "x11: the window did not come back"
    fi

    kill $windup_pid $wm_pid $xvfb_pid 2>/dev/null
    wait 2>/dev/null
}

case $which in
    wayland) wayland_session ;;
    x11) x11_session ;;
    weston) weston_session ;;
    all) wayland_session; weston_session; x11_session ;;
    *) echo "unknown session '$which'" >&2; exit 2 ;;
esac

echo
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
