# Windup on Linux

2026-10-12

Windup works on Wayland and on X11, on every desktop. What differs between
desktops is how much each lets an application do, so Windup looks at what
the desktop offers when it starts and uses the best route there is. This page
says what that means on yours, and how to bind the shortcut where you have to.

## What works where

| | Super+V | Watches the clipboard | Popup | Pastes into your window |
|---|---|---|---|---|
| **COSMIC** | by itself | yes | at the cursor | yes |
| **KDE Plasma 6** | asks on first start | yes | at the cursor | yes, or asks once |
| **Sway, Hyprland, river, Wayfire, labwc, niri** | bind once (Hyprland asks) | yes | at the cursor | yes |
| **GNOME 48+** (Wayland) | asks on first start | best effort, through XWayland | middle of the screen | asks once |
| **GNOME before 48** (Wayland) | bind once | best effort, through XWayland | middle of the screen | asks once |
| **Any X11 session** (XFCE, MATE, Cinnamon, i3, Openbox…) | by itself | yes | at the cursor | yes |

"Asks" is the desktop's own permission dialog (an XDG portal). Windup asks
for two things at most: to bind its two shortcuts, and to press Ctrl+V for
you where the desktop has no other way to let it. The second permission is
remembered across logins on desktops that support that.

GNOME is the one desktop that keeps the clipboard and the overlay layer to
itself, so there Windup reads the clipboard through XWayland and opens the
popup as an ordinary window in the middle of the screen.

## Binding the shortcuts

Bind **Super+V** to `windup --popup` and **Super+Shift+V** to
`windup --paste-plain`. If Windup is not running, either command starts it.

### COSMIC

Windup adds both shortcuts to COSMIC's custom shortcuts the first time it
starts, as "Clipboard history" and "Paste plain text"; they show in Settings →
Keyboard → Keyboard shortcuts → Custom shortcuts, where they can be changed or
removed. It does this once, and never takes a key you have already bound: if
Super+V does something else on your machine, bind `windup --popup` to another
key there. To have Windup check again, delete
`~/.config/windup/cosmic-shortcuts`.

### GNOME

GNOME 48 and newer ask on Windup's first start; accept. Older GNOME:
Settings → Keyboard → View and Customise Shortcuts → Custom Shortcuts → **+**.
GNOME uses Super+V for the notification list, so first move that one
(System → Show the notification list) to another key.

### KDE Plasma

Plasma asks on Windup's first start; accept. Plasma's own clipboard
(Klipper) also wants Meta+V: in System Settings → Keyboard → Shortcuts →
Klipper, clear "Show clipboard items at mouse position" first. To bind by
hand instead: Shortcuts → **Add New** → Command or Script.

### Sway and i3

```
bindsym Mod4+v exec windup --popup
bindsym Mod4+Shift+v exec windup --paste-plain
exec windup --background
```

(On i3, an X11 window manager, Windup grabs the keys itself and only the
`exec` line is needed.)

### Hyprland

Hyprland's portal asks on first start. Or:

```
bind = SUPER, V, exec, windup --popup
bind = SUPER SHIFT, V, exec, windup --paste-plain
exec-once = windup --background
```

### niri

```
binds {
    Mod+V { spawn "windup" "--popup"; }
    Mod+Shift+V { spawn "windup" "--paste-plain"; }
}
spawn-at-startup "windup" "--background"
```

### river

```
riverctl map normal Super V spawn 'windup --popup'
riverctl map normal Super+Shift V spawn 'windup --paste-plain'
```

### labwc, Wayfire and others

Any key binding that runs a command will do: run `windup --popup` and
`windup --paste-plain`.

## Starting at login

Turn on **Launch at login** in Windup's General settings. It writes
`~/.config/autostart/io.github.spillebulle.windup.desktop`, which every
desktop with a session manager reads. Window managers without one need the
`windup --background` line shown above for theirs.

Closing the settings window does not stop Windup: the history and the
shortcuts keep working. Run `windup` again, or pick Windup in the app menu,
to bring the window back.

## When something does not work

Start it from a terminal with logging, and look for the line that says which
route it chose:

```sh
pkill windup; RUST_LOG=windup=info windup
```

```
backend: Plan { host: Layer, watch: DataControl(Ext), paste: VirtualKeyboard, hotkeys: Command }
```

- **Super+V does nothing.** The binding is missing, or another program owns
  the key (Klipper, GNOME's notification list). Run `windup --popup` in a
  terminal: if the popup opens, it is the binding.
- **The popup opens in the middle of the screen on a desktop listed above as
  "at the cursor".** The compositor did not report the pointer until it
  moved. Move the mouse a little before pressing the shortcut; please report
  it with the log.
- **The entry is on the clipboard but was not pasted.** The paste permission
  was refused, or the window you were in does not accept Ctrl+V (terminals
  want Ctrl+Shift+V). Paste it yourself; it is already on the clipboard.
- **A password showed up in the history.** The password manager did not mark
  it secret. KeePassXC, Bitwarden and 1Password do by default. Windup also
  remembers each marked password while it runs, so a clipboard manager that
  keeps the text after the password manager quits (GNOME's, Klipper) does
  not sneak it in. Weston is the exception: it can replace the marked copy
  with its own before Windup has looked.

## No Flatpak, no Snap

Both sandboxes hide the clipboard and keyboard protocols from the apps
inside them, on purpose, and those protocols are Windup's whole job. The
`.deb`, `.rpm`, Arch package and AppImage are the ways to install it.

## Checking a new desktop

A short list to run through on a desktop Windup has not been tried on, in
the order things depend on each other:

1. `RUST_LOG=windup=info windup` prints a `backend:` line; note it.
2. Copy some text in another app. Open the popup: it is at the top.
3. Copy an image (a screenshot tool's "copy"). It shows with a preview.
4. Copy a password from a password manager. It does **not** show.
5. Open the popup over a text field, pick the second entry with the arrow
   keys and Enter. It is pasted there, and the field has focus again.
6. Same with the mouse, and with Escape and a click outside to dismiss.
7. Paste plain text from a web page into a rich editor: no formatting.
8. Close the settings window; the popup and history keep working.
9. Turn on Launch at login, log out and in: Super+V works straight away.
10. At a fractional scale (125 %, 150 %) the popup is sharp, not blurred.
