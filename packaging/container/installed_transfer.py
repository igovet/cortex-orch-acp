"""External GTK drag source; data is offered only after real compositor input."""
from pathlib import Path
import sys
import gi
gi.require_version("Gtk", "3.0")
from gi.repository import Gtk, Gdk, GLib

if sys.argv[1] == "--clipboard":
    clipboard = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
    clipboard.set_text(sys.argv[3], -1)
    Path(sys.argv[2]).touch()
    Gtk.main()
    sys.exit(0)

fixture, ready = map(Path, sys.argv[1:3])
backend = sys.argv[3]
window = Gtk.Window(title="Installed attachment fixture")
box = Gtk.EventBox()
box.add(Gtk.Label(label="Drag this fixture"))
window.add(box)
box.drag_source_set(Gdk.ModifierType.BUTTON1_MASK, [Gtk.TargetEntry.new("text/uri-list", 0, 0)], Gdk.DragAction.COPY)
def supply(widget, context, data, info, timestamp):
    ready.with_suffix(".requested").touch()
    data.set_uris([fixture.as_uri()])
box.connect("drag-data-get", supply)
box.connect("drag-begin", lambda *args: ready.with_suffix(".began").touch())
box.connect("drag-end", lambda *args: ready.with_suffix(".ended").touch())
window.connect("destroy", Gtk.main_quit)
window.set_default_size(180, 100)
if backend == "x11":
    window.move(20, 70)
window.show_all()
def mark():
    if backend == "x11" or (window.get_allocated_width() >= 950 and window.get_allocated_height() >= 1100):
        ready.touch()
        return False
    return True
GLib.timeout_add(100, mark)
Gtk.main()
