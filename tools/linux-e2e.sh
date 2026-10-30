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
# in a real GTK text field or in the history file. The capture cases check
# `windup --capture-test` pictures (sizes, and a marker window on a rotated
# output) and that `windup --screenshot` opens an overlay Escape closes. The
# recording cases check a `windup --record-test` file's size and length, and
# a recording made through `windup --record` and `--record-stop`, with
# ffprobe, and that Escape throws a recording away on X11. ffprobe (like
# ffmpeg) must sit next to the binary:
# `tools/fetch-ffmpeg.sh linux64 <bin-dir> --with-ffprobe`.
# Screenshots and logs go to the artefact directory.

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

# Whether the pixel at $2,$3 of the picture $1 is the marker's red.
is_red() {
    pixel "$1" "$2" "$3" | awk -F, 'NF == 3 && $1 > 200 && $2 < 80 && $3 < 80 { red = 1 } END { exit !red }'
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

# A package update replaces the program file while Windup runs. The old
# process must notice and hand its socket to the new one, or every shortcut
# after the update keeps reaching the old code.
update_case() {
    local dir=$home/update old_pid new_pid
    mkdir -p "$dir"
    cp "$bin" "$dir/windup"
    RUST_LOG=info "$dir/windup" --background >"$art/$1-update.log" 2>&1 &
    old_pid=$!
    if ! wait_for 100 sh -c 'for s in "$XDG_RUNTIME_DIR"/windup*.sock; do [ -S "$s" ] && exit 0; done; exit 1'; then
        bad "$1: the copy to update never opened its socket"
        return
    fi
    # As pacman and dpkg do: the new file is written beside, then renamed over.
    cp "$bin" "$dir/windup.new" && mv -f "$dir/windup.new" "$dir/windup"
    if wait_for 150 sh -c '! kill -0 "$1" 2>/dev/null' _ "$old_pid"; then
        ok "$1: the old Windup stepped aside after an update"
    else
        bad "$1: the old Windup kept running after an update"
        kill "$old_pid" 2>/dev/null
        return
    fi
    if wait_for 100 pgrep -f "$dir/windup --background"; then
        new_pid=$(pgrep -f "$dir/windup --background" | head -1)
        wl-copy "updated-$$"
        if wait_for 30 history_has "updated-$$"; then
            ok "$1: the new Windup took over"
        else
            bad "$1: the new Windup started but does not record copies"
        fi
        kill "$new_pid" 2>/dev/null
    else
        bad "$1: no new Windup started after an update"
    fi
}

# --- Wayland ----------------------------------------------------------------

# The Wayland capture back end on Sway (wlr-screencopy), on the output as it
# is and turned a quarter: the picture must come out upright, the marker (a
# red window Sway puts in the top-left corner) still top-left.
sway_capture() {
    local log=$art/wayland-capture.log size
    if "$bin" --capture-test "$art/wayland-capture.png" >"$log" 2>&1; then
        size=$(png_size "$art/wayland-capture.png")
        if [ "$size" = 1600x1000 ]; then
            ok "wayland: --capture-test wrote the whole output ($size)"
        else
            bad "wayland: --capture-test wrote $size, wanted 1600x1000"
        fi
    else
        bad "wayland: --capture-test failed (see wayland-capture.log)"
    fi

    # One Steps picture: HEADLESS-1 alone. A headless Sway has no pointer
    # device, so the place may be unknown (the cursor painted in instead).
    local step_out
    if step_out=$("$bin" --step-shot-test "$art/wayland-step.png" 2>"$art/wayland-step.log"); then
        size=$(png_size "$art/wayland-step.png")
        if [ "$size" = 1600x1000 ] && echo "$step_out" | grep -Eqx 'pointer (unknown|-?[0-9]+,-?[0-9]+)'; then
            ok "wayland: --step-shot-test pictured HEADLESS-1 ($size, $step_out)"
        else
            bad "wayland: --step-shot-test wrote $size and said '$step_out', wanted 1600x1000 and a pointer line"
        fi
    else
        bad "wayland: --step-shot-test failed (see wayland-step.log)"
    fi

    python3 "$(dirname "$0")/marker.py" 120 >"$art/wayland-marker.log" 2>&1 &
    local marker=$!
    wait_for 100 sh -c 'swaymsg -t get_tree | grep -q "\"name\": \"windup-marker\""' ||
        echo "  (the marker window did not appear; see wayland-marker.log)"
    swaymsg '[title="windup-marker"] floating enable, resize set 300 300, move position 0 0' >/dev/null
    sleep 0.5
    # The marker is mapped and floating on the visible workspace, so the
    # window rectangles read from Sway's get_tree must include it.
    local rects_out
    rects_out=$("$bin" --window-rects-test 2>>"$log")
    if [ "$(echo "$rects_out" | sed -n 's/^rects: //p')" -ge 1 ] 2>/dev/null; then
        ok "wayland: --window-rects-test found windows ($(echo "$rects_out" | head -1))"
    else
        bad "wayland: --window-rects-test found no windows ($rects_out)"
    fi
    # Only a marker seen where it was put proves anything about rotation.
    local marked=false
    if "$bin" --capture-test "$art/wayland-marker.png" >>"$log" 2>&1 &&
        is_red "$art/wayland-marker.png" 100 100 && ! is_red "$art/wayland-marker.png" 1500 900; then
        marked=true
    else
        echo "  (no marker in the top-left corner of the upright capture; checking the rotated size only)"
    fi

    swaymsg output HEADLESS-1 transform 90 >/dev/null
    sleep 0.5
    swaymsg '[title="windup-marker"] move position 0 0' >/dev/null
    sleep 0.5
    if "$bin" --capture-test "$art/wayland-rotated.png" >>"$log" 2>&1; then
        size=$(png_size "$art/wayland-rotated.png")
        if [ "$size" = 1000x1600 ]; then
            ok "wayland: a rotated output is captured upright ($size)"
        else
            bad "wayland: the rotated output came out $size, wanted 1000x1600"
        fi
        local f=$art/wayland-rotated.png
        if ! $marked; then
            :
        elif is_red "$f" 100 100 && ! is_red "$f" 900 100 && ! is_red "$f" 100 1500 && ! is_red "$f" 900 1500; then
            ok "wayland: on the rotated output the marker is still top-left"
        else
            bad "wayland: rotated corners are $(pixel "$f" 100 100) $(pixel "$f" 900 100) $(pixel "$f" 100 1500) $(pixel "$f" 900 1500); the marker belongs top-left only"
        fi
    else
        bad "wayland: --capture-test failed on the rotated output (see wayland-capture.log)"
    fi
    swaymsg output HEADLESS-1 transform normal >/dev/null
    kill $marker 2>/dev/null
    wait $marker 2>/dev/null
    sleep 0.5
}

# A recording through the Wayland frame stream (wlr-screencopy on Sway): the
# file must have the area's size and about the time asked for. The output is
# scale 1, so the area's logical size is its pixel size.
sway_record() {
    local log=$art/wayland-record.log out=/tmp/windup-e2e-record-$$.mp4 probe info size dur
    probe=$(dirname "$bin")/ffprobe
    if timeout 30 "$bin" --record-test "$out" --seconds 2 --area 0,0,320,240 >"$log" 2>&1; then
        info=$("$probe" -v error -select_streams v:0 -show_entries stream=width,height:format=duration \
            -of csv=p=0 "$out" 2>>"$log")
        size=$(echo "$info" | grep , | head -1 | tr , x)
        dur=$(echo "$info" | grep -v , | head -1)
        if [ "$size" = 320x240 ]; then
            ok "wayland: --record-test recorded the area at its size ($size)"
        else
            bad "wayland: --record-test recorded $size, wanted 320x240 ($info)"
        fi
        if awk -v d="$dur" 'BEGIN { exit !(d >= 1.5 && d <= 2.5) }'; then
            ok "wayland: the recording lasts about two seconds (${dur}s)"
        else
            bad "wayland: the recording lasts '${dur}'s, wanted 1.5 to 2.5"
        fi
    else
        bad "wayland: --record-test failed (see wayland-record.log)"
    fi
    rm -f "$out"

    # A monitor unplugged mid-recording ends it with that reason and keeps
    # what was recorded. --record-test exits 0 whenever it saved a file, so
    # the message and the file are what is checked, not the exit code.
    if ! swaymsg create_output >/dev/null 2>&1; then
        echo "  (sway cannot create an output; skipping the unplug case)"
        return
    fi
    local second rec unplug_log=$art/wayland-unplug.log
    second=$(swaymsg -t get_outputs -r | python3 -c \
        'import json, sys; print(next(o["name"] for o in json.load(sys.stdin) if o["name"] != "HEADLESS-1"))')
    swaymsg output "$second" resolution 800x600 position 1600 0 >/dev/null
    sleep 0.5
    out=/tmp/windup-e2e-unplug-$$.mp4
    timeout 30 "$bin" --record-test "$out" --seconds 8 --area 1600,0,320,240 >"$unplug_log" 2>&1 &
    rec=$!
    # Long enough for the recording to be under way: it starts at once,
    # with no encoder probe to wait for.
    sleep 3
    swaymsg output "$second" unplug >/dev/null
    wait $rec
    if grep -q "A monitor was disconnected." "$unplug_log"; then
        ok "wayland: unplugging the recorded monitor says so"
    else
        bad "wayland: unplugging the recorded monitor gave: $(tail -1 "$unplug_log")"
    fi
    if [ -s "$out" ]; then
        ok "wayland: what was recorded before the unplug is kept"
    else
        bad "wayland: nothing was kept after the unplug (see wayland-unplug.log)"
    fi
    rm -f "$out"
}

# A recording as a person makes one: `windup --record` opens the launcher in
# Video, Ctrl+A takes the whole output and Enter records it; two seconds
# later `windup --record-stop` stops it. The file must land in the Videos
# folder with a video stream, and the notice must say so. The whole output is
# recorded, so the bar has no room and is not shown; the hotkey stops it.
sway_record_live() {
    local videos=$home/Videos log=$art/wayland-windup.log probe file notices
    probe=$(dirname "$bin")/ffprobe
    # Where `directories` finds the Videos folder on Linux.
    mkdir -p "$videos" "$XDG_CONFIG_HOME"
    echo "XDG_VIDEOS_DIR=\"$videos\"" >"$XDG_CONFIG_HOME/user-dirs.dirs"
    export XDG_VIDEOS_DIR=$videos
    # Video is offered once the encoder probe has finished.
    if ! wait_for 600 test -s "$XDG_CONFIG_HOME/windup/encoders.json"; then
        bad "wayland: the encoder probe did not finish; no recording case"
        return
    fi
    notices=$(grep -c "notice: shown" "$log")
    keys_later 4000 -M ctrl -k a -m ctrl -s 400 -k Return
    "$bin" --record
    wait $keys_pid
    if wait_for 100 grep -q "capture: closed (record)" "$log"; then
        ok "wayland: --record opened the launcher in Video and Enter recorded"
    else
        bad "wayland: --record did not end in a recording"
        grep -E "capture|notice|recording" "$log"
        return
    fi
    if wait_for 150 grep -q "recording: started" "$log"; then
        ok "wayland: the recording started"
    else
        bad "wayland: the recording did not start"
        grep -E "recording|notice" "$log"
        return
    fi
    sleep 2
    "$bin" --record-stop
    if wait_for 300 grep -q "recording: over" "$log"; then
        ok "wayland: --record-stop stopped the recording"
    else
        bad "wayland: the recording did not stop"
        grep -E "recording|notice" "$log"
        return
    fi
    file=$(ls "$videos"/Windup/*.mp4 2>/dev/null | head -1)
    if [ -n "$file" ] &&
        [ "$("$probe" -v error -select_streams v:0 -show_entries stream=codec_type -of csv=p=0 "$file")" = video ]; then
        ok "wayland: the recording was saved to Videos/Windup with a video stream"
    else
        bad "wayland: no recording with a video stream in $videos/Windup: $(ls -R "$videos")"
    fi
    if wait_for 100 sh -c '[ "$(grep -c "notice: shown" "$1")" -gt "$2" ]' _ "$log" "$notices" &&
        grep -q "capture: notice: Saved to " "$log"; then
        ok "wayland: the notice says where the recording went"
    else
        bad "wayland: no notice for the saved recording"
        grep -E "notice" "$log"
    fi
    # Put the notice away: a new keyboard device sends focus elsewhere, and
    # the notice closes when it loses the keyboard, if not on Escape.
    wtype -k Escape
    sleep 0.4
    rm -rf "$videos"/Windup
}

# Esc cancels a recording from anywhere: on X11 the recording's keys are
# grabbed from its start to its end. Super+Shift+R (the Video default) opens
# the launcher in Video, Ctrl+A takes the whole screen and Enter records it;
# Escape, pressed with the recorded screen in front, must throw the recording
# away and leave no file in the Videos folder.
x11_record_cancel() {
    local videos=$home/Videos log=$art/x11-windup.log
    mkdir -p "$videos" "$XDG_CONFIG_HOME"
    echo "XDG_VIDEOS_DIR=\"$videos\"" >"$XDG_CONFIG_HOME/user-dirs.dirs"
    export XDG_VIDEOS_DIR=$videos
    if ! wait_for 600 test -s "$XDG_CONFIG_HOME/windup/encoders.json"; then
        bad "x11: the encoder probe did not finish; no recording case"
        return
    fi
    xdotool key super+shift+r
    sleep 2
    xdotool key ctrl+a
    sleep 0.4
    xdotool key Return
    if wait_for 150 grep -q "recording: started" "$log"; then
        ok "x11: Super+Shift+R and Enter started a recording"
    else
        bad "x11: Super+Shift+R did not end in a recording"
        grep -E "capture|recording" "$log"
        return
    fi
    if grep -q "recording keys bound" "$log"; then
        ok "x11: the recording's keys were grabbed"
    else
        bad "x11: the recording's keys were not grabbed"
    fi
    sleep 1.5
    xdotool key Escape
    if wait_for 150 grep -q "recording: cancelled" "$log" &&
        wait_for 150 grep -q "recording: over (None)" "$log"; then
        ok "x11: Escape cancelled the recording"
    else
        bad "x11: Escape did not cancel the recording"
        grep -E "recording" "$log"
        return
    fi
    if wait_for 50 grep -q "recording keys released" "$log"; then
        ok "x11: the recording's keys were let go"
    else
        bad "x11: the recording's keys are still grabbed"
    fi
    if [ -z "$(ls -A "$videos"/Windup 2>/dev/null)" ]; then
        ok "x11: the cancelled recording left no file"
    else
        bad "x11: the cancelled recording left $(ls "$videos"/Windup)"
    fi
    # Put the notice away.
    xdotool key Escape
    sleep 0.4
    rm -rf "$videos"/Windup
}

# Steps on X11: Super+Shift+G starts a session, two clicks and the step key
# take three steps, Super+Shift+G again opens the review. The session folder
# exports, the review's close asks and Return discards, which removes the
# folder. A second session cancelled with Escape leaves nothing.
x11_steps() {
    local log=$art/x11-windup.log root=$home/.cache/windup/steps dir
    xdotool key super+shift+g
    if wait_for 150 grep -q "steps: started" "$log"; then
        ok "x11: Super+Shift+G started a Steps session"
    else
        bad "x11: Super+Shift+G did not start a Steps session"
        grep -E "steps" "$log"
        return
    fi
    sleep 0.5
    xdotool mousemove 300 400 click 1
    sleep 0.6
    xdotool click 3
    sleep 0.6
    xdotool key super+shift+x
    if wait_for 100 grep -q "steps: step 3 written" "$log"; then
        ok "x11: two clicks and the step key took three steps"
    else
        bad "x11: the clicks and the step key did not take three steps"
        grep -E "steps" "$log"
    fi
    xdotool key super+shift+g
    if wait_for 100 grep -q "steps: review opened" "$log"; then
        ok "x11: Super+Shift+G again stopped and opened the review"
    else
        bad "x11: the review did not open"
        grep -E "steps" "$log"
        return
    fi
    dir=$(find "$root" -mindepth 1 -maxdepth 1 -type d | head -n 1)
    # Every step marks the pointer, the one taken with the key too.
    if [ -n "$dir" ] && python3 - "$dir/steps.json" <<'PY'
import json, sys
steps = json.load(open(sys.argv[1]))["steps"]
assert len(steps) == 3, len(steps)
assert all(s["pointer"] is not None for s in steps), steps
PY
    then
        ok "x11: steps.json holds the three steps, each with the pointer"
    else
        bad "x11: steps.json is not the three steps (${dir:-no folder})"
    fi
    local html=$home/x11-steps.html
    if [ -n "$dir" ] && "$bin" --steps-export "$dir" html "$html" >/dev/null 2>&1 &&
        [ "$(grep -o '<img' "$html" | wc -l)" -eq 3 ]; then
        ok "x11: --steps-export wrote a page with the three pictures"
    else
        bad "x11: --steps-export did not write the three pictures"
    fi
    xdotool search --name '^Windup steps$' windowactivate --sync key alt+F4 2>/dev/null
    sleep 0.6
    xdotool key Return
    if [ -n "$dir" ] && wait_for 50 sh -c "! test -e '$dir'"; then
        ok "x11: closing the review and Return discarded the steps"
    else
        bad "x11: the review's discard left ${dir:-nothing to check}"
    fi

    xdotool key super+shift+g
    if wait_for 150 sh -c "[ \$(grep -c 'steps: started' '$log') -ge 2 ]"; then
        sleep 0.5
        xdotool mousemove 300 400 click 1
        sleep 0.6
        xdotool key Escape
        if wait_for 100 grep -q "steps: cancelled" "$log" &&
            wait_for 50 sh -c "[ -z \"\$(ls -A '$root' 2>/dev/null)\" ]"; then
            ok "x11: Escape cancelled the Steps session and left no folder"
        else
            bad "x11: Escape did not cancel the Steps session cleanly"
            ls -la "$root" 2>/dev/null
        fi
    else
        bad "x11: a second Steps session did not start"
    fi
    if wait_for 50 sh -c "[ \$(grep -c 'steps keys released' '$log') -ge 2 ]"; then
        ok "x11: the Steps keys were let go"
    else
        bad "x11: the Steps keys are still grabbed"
    fi
}

# Steps on the layer host, by its commands alone: `--steps` starts at once,
# `--step` twice takes two steps (by screencopy), `--steps-stop` opens the
# review as a child process, and the session exports as HTML with both
# pictures and no script. Clicks are not watched here: no evdev access.
sway_steps() {
    local root=$home/.cache/windup/steps log=$art/wayland-windup.log dir out=/tmp/windup-e2e-steps-$$.html
    "$bin" --steps
    wait_for 100 grep -q "steps: started" "$log" || { bad "wayland: --steps did not start"; grep steps "$log"; return; }
    ok "wayland: --steps started at once"
    "$bin" --step; wait_for 50 grep -q "steps: step 1 written" "$log"
    "$bin" --step
    if wait_for 50 grep -q "steps: step 2 written" "$log"; then ok "wayland: --step twice took two steps"; else bad "wayland: --step did not take two steps"; fi
    "$bin" --steps-stop
    if wait_for 100 grep -q "steps: review opened" "$log"; then ok "wayland: --steps-stop opened the review"; else bad "wayland: no review"; return; fi
    dir=$(ls -d "$root"/*/ | head -1)
    if python3 -c 'import json,sys; assert len(json.load(open(sys.argv[1]))["steps"]) == 2' "$dir/steps.json"; then
        ok "wayland: steps.json lists two steps"; else bad "wayland: steps.json: $(cat "$dir/steps.json")"; fi
    if timeout 30 "$bin" --steps-export "$dir" html "$out" && [ "$(grep -o '<img' "$out" | wc -l)" -eq 2 ] && ! grep -q '<script' "$out"; then
        ok "wayland: the HTML export has two pictures and no script"; else bad "wayland: the HTML export is wrong"; fi
    pkill -f -- '--steps-review' ; rm -rf "$root" "$out"
}

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

    sway_capture
    sway_record

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

    # The screenshot overlay opens over the output, and Escape closes it
    # without ending Windup. The GPU device exists by now (the popups made
    # it), so the keys are timed as for the popup.
    keys_later 4000 -k Escape
    "$bin" --screenshot
    if wait_for 150 grep -q "capture: output 0 shown" "$art/wayland-windup.log"; then
        ok "wayland: --screenshot opened the overlay"
        grim "$art/wayland-overlay.png" 2>/dev/null
    else
        bad "wayland: --screenshot did not open the overlay"
        grep -E "capture|notice" "$art/wayland-windup.log"
    fi
    wait $keys_pid
    if wait_for 50 grep -q "capture: closed (cancel)" "$art/wayland-windup.log" &&
        kill -0 $windup_pid 2>/dev/null; then
        ok "wayland: Escape closed the overlay and Windup kept running"
    else
        bad "wayland: the overlay did not close on Escape, or Windup stopped"
    fi

    sway_record_live
    sway_steps

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

    kill $windup_pid 2>/dev/null
    wait $windup_pid 2>/dev/null
    update_case wayland

    kill $sway_pid 2>/dev/null
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

    # Weston offers no capture protocol a client may use, and with no D-Bus
    # session there is no screenshot portal either (a desktop without one):
    # nothing can read the screen, and the test says so.
    DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/no-bus" \
        "$bin" --capture-test "$art/weston.png" >"$art/weston-capture.log" 2>&1
    local code=$?
    if [ $code -eq 1 ] &&
        grep -qF "This desktop does not let applications read the screen." "$art/weston-capture.log"; then
        ok "weston: --capture-test says the screen cannot be read"
    else
        bad "weston: --capture-test exited $code: $(cat "$art/weston-capture.log")"
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

    # One Steps picture: the pointer's monitor (the whole screen here) and
    # the pointer in its pixels.
    xdotool mousemove 400 300
    local step_out
    if step_out=$("$bin" --step-shot-test "$art/x11-step.png" 2>"$art/x11-step.log"); then
        local size
        size=$(png_size "$art/x11-step.png")
        if [ "$size" = "$screen" ] && [ "$step_out" = "pointer 400,300" ]; then
            ok "x11: --step-shot-test pictured the monitor with the pointer ($size, $step_out)"
        else
            bad "x11: --step-shot-test wrote $size and said '$step_out', wanted $screen and 'pointer 400,300'"
        fi
    else
        bad "x11: --step-shot-test failed (see x11-step.log)"
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

    x11_record_cancel
    x11_steps

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
