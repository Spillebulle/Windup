#!/usr/bin/env python3
"""Copies text the way KeePassXC copies a password: as text, plus the
x-kde-passwordManagerHint type saying it is secret. Used by linux-e2e.sh,
since xclip and wl-copy offer only one type.

    secret-copy.py <text> [seconds-to-keep-it]

Needs python3-gi and gir1.2-gtk-4.0. On Wayland a client may only set the
clipboard in answer to input, so a small window opens and copies again when
a key is pressed in it ("copied (key)"); X11 takes the copy straight away
("copied (timer)").
"""

import sys

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, GLib, Gtk  # noqa: E402

text = sys.argv[1].encode()
keep = float(sys.argv[2]) if len(sys.argv) > 2 else 3.0


def copy(window, why):
    provider = Gdk.ContentProvider.new_union(
        [
            Gdk.ContentProvider.new_for_bytes("text/plain;charset=utf-8", GLib.Bytes.new(text)),
            Gdk.ContentProvider.new_for_bytes("text/plain", GLib.Bytes.new(text)),
            Gdk.ContentProvider.new_for_bytes("x-kde-passwordManagerHint", GLib.Bytes.new(b"secret")),
        ]
    )
    window.get_clipboard().set_content(provider)
    print(f"copied ({why})", flush=True)


def activate(app):
    window = Gtk.ApplicationWindow(application=app, title="secret-copy")

    keys = Gtk.EventControllerKey()
    keys.connect("key-pressed", lambda *_: copy(window, "key") or False)
    window.add_controller(keys)

    # X11 needs no focus to take the clipboard, and a window manager with
    # no input devices may never give any.
    window.present()
    GLib.timeout_add(500, lambda: copy(window, "timer") or False)
    GLib.timeout_add(int(keep * 1000) + 500, app.quit)


app = Gtk.Application(application_id="io.github.spillebulle.windup.SecretCopy")
app.connect("activate", activate)
app.run([])
