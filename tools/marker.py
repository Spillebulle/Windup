#!/usr/bin/env python3
"""A solid red window titled "windup-marker", for linux-e2e.sh: Sway puts it
in the top-left corner of the screen, and a capture of a rotated output
must show it there.

    marker.py [seconds-to-stay]

Needs python3-gi and gir1.2-gtk-4.0, as secret-copy.py does.
"""

import sys

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, GLib, Gtk  # noqa: E402

stay = float(sys.argv[1]) if len(sys.argv) > 1 else 60.0
RULE = "window { background: #ff0000; }"


def activate(app):
    css = Gtk.CssProvider()
    # load_from_string arrived in GTK 4.12; older ones take bytes.
    if hasattr(css, "load_from_string"):
        css.load_from_string(RULE)
    else:
        css.load_from_data(RULE.encode())
    Gtk.StyleContext.add_provider_for_display(
        Gdk.Display.get_default(), css, Gtk.STYLE_PROVIDER_PRIORITY_USER
    )
    window = Gtk.ApplicationWindow(application=app, title="windup-marker")
    window.set_decorated(False)
    window.set_default_size(300, 300)
    window.present()
    print("shown", flush=True)
    GLib.timeout_add(int(stay * 1000), app.quit)


app = Gtk.Application(application_id="io.github.spillebulle.windup.Marker")
app.connect("activate", activate)
app.run([])
