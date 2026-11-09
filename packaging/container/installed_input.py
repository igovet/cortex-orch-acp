"""Read accessibility geometry; send input exclusively through XTEST or Mutter."""
import os
from pathlib import Path
import subprocess
import time
import xml.etree.ElementTree as ET

import gi
gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, Gio, GLib

WIDTH, HEIGHT = 1920, 1200


def wait(test, label, seconds=30):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        value = test()
        if value and time.monotonic() <= end:
            return value
        time.sleep(.15)
    raise AssertionError("deadline: " + label)


class Input:
    def __init__(self, backend):
        self.backend = backend
        self.closing = False
        self.position = None
        self.down = False
        self.bus = Gio.bus_get_sync(Gio.BusType.SESSION)
        if backend == "wayland":
            wait(lambda: self.bus.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus",
                 "org.freedesktop.DBus", "NameHasOwner", GLib.Variant("(s)", ("org.gnome.Mutter.RemoteDesktop",)),
                 None, Gio.DBusCallFlags.NONE, 1000, None).unpack()[0], "Mutter remote input bus ready")
            self.remote = self.call("/org/gnome/Mutter/RemoteDesktop",
                                    "org.gnome.Mutter.RemoteDesktop", "CreateSession").unpack()[0]
            self.interface = "org.gnome.Mutter.RemoteDesktop.Session"
            xml = self.call(self.remote, "org.freedesktop.DBus.Introspectable", "Introspect").unpack()[0]
            methods = {n.attrib["name"] for n in ET.fromstring(xml).iter("method")}
            assert {"Start", "NotifyPointerMotionRelative", "NotifyPointerButton", "NotifyKeyboardKeysym"} <= methods
            self.call(self.remote, self.interface, "Start")
            # Create both virtual devices before GTK starts.
            self.move(WIDTH - 10, HEIGHT - 10)
            self.key("Escape")

    def call(self, path, interface, method, value=None):
        return self.bus.call_sync("org.gnome.Mutter.RemoteDesktop", path, interface, method, value,
                                  None, Gio.DBusCallFlags.NONE, 5000, None)

    def notify(self, method, signature, value):
        self.call(self.remote, self.interface, method, GLib.Variant(signature, value))

    def move(self, x, y):
        assert 0 <= x < WIDTH and 0 <= y < HEIGHT, "target outside screen"
        if self.backend == "x11":
            subprocess.run(["xdotool", "mousemove", "--sync", str(int(x)), str(int(y))], check=True)
        else:
            if self.position is None or not self.down:
                self.notify("NotifyPointerMotionRelative", "(dd)", (10000., 10000.))
                time.sleep(.03)
                self.position = (WIDTH - 1, HEIGHT - 1)
            self.notify("NotifyPointerMotionRelative", "(dd)", (float(x - self.position[0]), float(y - self.position[1])))
            self.position = (x, y)

    def button(self, down):
        if self.backend == "x11":
            subprocess.run(["xdotool", "mousedown" if down else "mouseup", "1"], check=True)
        else:
            self.notify("NotifyPointerButton", "(ib)", (0x110, down))
        self.down = down

    def click_at(self, x, y):
        self.move(x, y)
        time.sleep(.1)
        self.button(True)
        time.sleep(.06)
        self.button(False)

    def key(self, name):
        if self.backend == "x11":
            subprocess.run(["xdotool", "key", "--clearmodifiers", name], check=True)
        else:
            symbols = {"Escape": 0xff1b, "Return": 0xff0d, "Tab": 0xff09,
                       "ctrl": 0xffe3, "alt": 0xffe9, "super": 0xffeb, "F10": 0xffc7,
                       "Up": 0xff52, "Down": 0xff54, "Left": 0xff51, "Right": 0xff53, "BackSpace": 0xff08}
            keys = [symbols.get(part, ord(part) if len(part) == 1 else 0) for part in name.split("+")]
            assert all(keys), "unsupported key"
            for sym in keys:
                self.notify("NotifyKeyboardKeysym", "(ub)", (sym, True))
            for sym in reversed(keys):
                self.notify("NotifyKeyboardKeysym", "(ub)", (sym, False))

    def type(self, text):
        if self.backend == "x11":
            subprocess.run(["xdotool", "type", "--clearmodifiers", "--delay", "3", "--", text], check=True)
        else:
            for char in text:
                sym = 0xff0d if char == "\n" else ord(char)
                self.notify("NotifyKeyboardKeysym", "(ub)", (sym, True))
                self.notify("NotifyKeyboardKeysym", "(ub)", (sym, False))

    def paste(self, text):
        if self.backend == "x11":
            ready = Path.home() / ("clipboard-ready-" + str(time.monotonic_ns()))
            clipboard = subprocess.Popen(["python3", "-B", "/harness/installed_transfer.py",
                                          "--clipboard", str(ready), text],
                                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            wait(ready.exists, "GTK OS clipboard owner")
        else:
            clipboard = subprocess.Popen(["wl-copy", "--foreground", "--type", "text/plain;charset=utf-8"],
                                         stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            clipboard.stdin.write(text.encode())
            clipboard.stdin.close()
            time.sleep(.4)
        read = (["xclip", "-selection", "clipboard", "-o"] if self.backend == "x11" else ["wl-paste", "--no-newline"])
        assert subprocess.check_output(read, timeout=5).decode() == text, "OS clipboard readback"
        self.key("ctrl+v")
        return clipboard

    def nodes(self, *, check_errors=True):
        desktop = Atspi.get_desktop(0)
        found, stack, count = [], [], 0
        for i in range(desktop.get_child_count()):
            app = desktop.get_child_at_index(i)
            if app and "cortex-workspace" in (app.get_name() or ""):
                stack.append(app)
        while stack:
            node = stack.pop()
            if node is None:
                continue
            count += 1
            assert count < 12000, "accessibility tree truncated"
            try:
                states = node.get_state_set()
                if states.contains(Atspi.StateType.DEFUNCT):
                    continue
                if states.contains(Atspi.StateType.SHOWING):
                    coord = Atspi.CoordType.SCREEN if self.backend == "x11" else Atspi.CoordType.WINDOW
                    interfaces = {i.rsplit(".", 1)[-1] for i in node.get_interfaces()}
                    rect = node.get_extents(coord) if "Component" in interfaces else Atspi.Rect()
                    text = ""
                    if "Text" in interfaces:
                        try:
                            text = Atspi.Text.get_text(node, 0, -1)
                        except Exception:
                            pass
                    found.append(dict(role=node.get_role_name(), name=node.get_name() or "", text=text,
                                      x=rect.x, y=rect.y, w=rect.width, h=rect.height,
                                      enabled=states.contains(Atspi.StateType.ENABLED), focused=states.contains(Atspi.StateType.FOCUSED), selected=states.contains(Atspi.StateType.SELECTED)))
                stack.extend(node.get_child_at_index(i) for i in reversed(range(node.get_child_count())))
            except GLib.Error as error:
                # A page update may invalidate one read, never an input action.
                stale = ("does not exist" in str(error).lower() or "application no longer exists" in str(error).lower()
                         or ("no such interface" in str(error).lower() and "org.a11y.atspi." in str(error)))
                closed = self.closing and "disconnected from message bus" in str(error).lower()
                if not (stale or closed):
                    raise
        errors = [n for n in found if n["role"] == "alert" and (n["name"] or n["text"])]
        if check_errors:
            assert not errors, "visible error alert"
            assert not any("This task changed on the machine. Refresh it before trying again." in (n["name"] + " " + n["text"]) for n in found), "visible stale-action error"
        return found

    def find(self, name, roles=None, contains=False):
        return next((n for n in self.nodes() if (name in n["name"] if contains else name == n["name"])
                     and (not roles or n["role"] in roles) and n["w"] > 0 and n["h"] > 0), None)

    def seen(self, text):
        return any(text in (n["name"] + " " + n["text"]) for n in self.nodes())

    def click(self, name, roles=None, contains=False):
        roles = roles or ("push button", "button", "toggle button", "link", "page tab", "menu item", "radio button", "check box")
        def enabled():
            node = self.find(name, roles, contains)
            return node if node and node["enabled"] else None
        n = wait(enabled, "enabled control " + name)
        self.click_at(n["x"] + n["w"] / 2, n["y"] + n["h"] / 2)

    def fill(self, name, text):
        self.click(name, ("entry", "text", "combo box"))
        wait(lambda: any(n["name"] == name and n["focused"] for n in self.nodes()), "field focus " + name)
        self.key("ctrl+a")
        self.type(text)

    def stable(self, name):
        # Repeated geometry/text changes of a hovered stationary control are a
        # regression; paint-only flicker is not observable through accessibility.
        n = wait(lambda: self.find(name), "stable " + name)
        self.move(n["x"] + n["w"] / 2, n["y"] + n["h"] / 2)
        expected = tuple(n[k] for k in ("name", "x", "y", "w", "h"))
        for _ in range(8):
            time.sleep(.15)
            current = self.find(name)
            assert current and tuple(current[k] for k in ("name", "x", "y", "w", "h")) == expected, "blink/shift: " + name
