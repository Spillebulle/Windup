#!/usr/bin/env python3
"""Copies text the way KeePassXC copies a password: as text, plus the
x-kde-passwordManagerHint type saying it is secret. Used by linux-e2e.sh,
since xclip and wl-copy offer only one type.

    secret-copy.py <text> [seconds-to-keep-it]

Needs python3-gi and gir1.2-gtk-4.0. On Wayland a client may only set the
clipboard while it has focus, so a small window opens for the duration.
"""

import sys

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, GLib, Gtk  # noqa: E402

text = sys.argv[1].encode()
keep = float(sys.argv[2]) if len(sys.argv) > 2 else 3.0


def copy(window):
    provider = Gdk.ContentProvider.new_union(
        [
            Gdk.ContentProvider.new_for_bytes("text/plain;charset=utf-8", GLib.Bytes.new(text)),
            Gdk.ContentProvider.new_for_bytes("text/plain", GLib.Bytes.new(text)),
            Gdk.ContentProvider.new_for_bytes("x-kde-passwordManagerHint", GLib.Bytes.new(b"secret")),
        ]
    )
    window.get_clipboard().set_content(provider)
    print("copied", flush=True)


def activate(app):
    window = Gtk.ApplicationWindow(application=app, title="secret-copy")
    done = []

    def on_active(*_):
        if window.is_active() and not done:
            done.append(True)
            # Active comes from the toplevel's state, a moment before the
            # keyboard focus whose serial the compositor checks the
            # clipboard request against.
            GLib.timeout_add(500, lambda: copy(window) or False)
            GLib.timeout_add(int(keep * 1000) + 500, app.quit)

    window.connect("notify::is-active", on_active)
    window.present()
    on_active()


app = Gtk.Application(application_id="io.github.spillebulle.windup.SecretCopy")
app.connect("activate", activate)
app.run([])
