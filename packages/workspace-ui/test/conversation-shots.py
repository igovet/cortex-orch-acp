"""Captures of the task conversation in WebKitGTK on a private virtual screen. Run through conversation-shots.mjs."""
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import time
from urllib.parse import quote

xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "1920x1200x24", "-nolisten", "tcp"], stdout=subprocess.PIPE, text=True)
os.environ.update(DISPLAY=":" + xvfb.stdout.readline().strip(), GDK_BACKEND="x11", NO_AT_BRIDGE="1", WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS="1", WEBKIT_DISABLE_COMPOSITING_MODE="1")
import gi
gi.require_version("Gtk", "3.0")
gi.require_version("WebKit2", "4.1")
from gi.repository import Gtk, GLib, WebKit2, Gdk

evidence = Path("/evidence")
win = Gtk.Window()
win.set_default_size(1280, 800)
view = WebKit2.WebView(web_context=WebKit2.WebContext.new_ephemeral())
win.add(view)
win.show_all()
loads = 0

# The three states the owner reads a task in, the coordinator's streamed record in Work details, and the sizes: the desktop
# window's usual, large and smallest, and a phone.
STATES = (
    ("question", "/m/<M>/t/c-ask", ".dc .dc-q"),
    ("working", "/m/<M>/t/c-work", ".queue .qrow"),
    ("result", "/m/<M>/t/c-done", ".rcard"),
    ("coordinator", "/m/<M>/t/c-work/workers/cortex", ".livepane .ev-note"),
)
SIZES = ((1280, 800), (1920, 1080), (960, 540), (390, 844))


def main(fn):
    done, box = threading.Event(), {}
    def run():
        try:
            fn(box, done)
        except Exception as error:
            box["error"] = str(error)
            done.set()
        return False
    GLib.idle_add(run)
    assert done.wait(20), "WebKit callback timeout"
    if "error" in box:
        raise RuntimeError(box["error"])
    return box.get("value")


def js(code):
    def call(box, done):
        def finish(source, result, _):
            try:
                box["value"] = json.loads(source.evaluate_javascript_finish(result).to_string())
            except Exception as error:
                box["error"] = str(error)
            done.set()
        view.evaluate_javascript("JSON.stringify((()=>{" + code + "})()??null)", -1, None, None, None, finish, None)
    return main(call)


def wait(code):
    for _ in range(100):
        if js("return !!(" + code + ")"):
            return
        time.sleep(.1)
    raise TimeoutError(code)


def capture(name):
    time.sleep(.4)
    def call(box, done):
        width, height = win.get_size()
        pixels = Gdk.pixbuf_get_from_window(win.get_window(), 0, 0, width, height)
        pixels.savev(str(evidence / (name + ".png")), "png", [], [])
        box["value"] = [width, height]
        done.set()
    return main(call)


# Every control row the owner reads together: the heights of its visible controls, which must be one value per row.
ROWS = r"""
const rows = {'title band': '.tb-actions', 'composer tools': '.dock .mc .c-bar', 'answer tools': '.dc .mc .c-bar', 'queued row': '.queue .qrow', 'task bar': '.taskbar'};
const out = {};
for (const [name, sel] of Object.entries(rows)) { const row = document.querySelector(sel); if (!row) continue;
  const hs = [...row.querySelectorAll('.btn, .ib, .sel, .seg, .chip')].filter(e => e.getClientRects().length && !e.closest('.seg > *') && getComputedStyle(e).visibility !== 'hidden').map(e => Math.round(e.getBoundingClientRect().height * 10) / 10);
  out[name] = [...new Set(hs)]; }
// What runs past the right edge of the window, if anything: the deepest such boxes, by class.
out['past the right edge'] = [...document.querySelectorAll('body *')].filter(e => e.getClientRects().length && e.getBoundingClientRect().right > innerWidth + 1 && ![...e.children].some(c => c.getBoundingClientRect().right > innerWidth + 1)).slice(0, 6).map(e => e.tagName.toLowerCase() + '.' + String(e.className).split(' ').slice(0, 2).join('.') + ' ' + Math.round(e.getBoundingClientRect().right));
return out;
"""


def run():
    global loads
    made = []
    for theme in ("", "&dark=1"):
        for state, route, ready in STATES:
            for width, height in (SIZES if not theme else ((1280, 800),)):
                loads += 1
                main(lambda b, d: (win.resize(width, height), d.set()))
                main(lambda b, d: (view.load_uri(sys.argv[1] + "?world=conversation&route=" + quote(route, safe="") + theme + f"&load={loads}"), d.set()))
                wait(f"window.fixture === '{loads}'")
                wait(f"document.querySelector({json.dumps(ready)})")
                time.sleep(.6)
                actual = js("return [innerWidth, innerHeight, document.documentElement.scrollWidth > innerWidth + 1]")
                rows = js(ROWS)
                name = f"{state}-{width}x{height}" + ("-dark" if theme else "")
                size = capture(name)
                made.append({"file": name + ".png", "window": size, "viewport": actual[:2], "page_scrolls_sideways": actual[2], "rows": rows})
    print(json.dumps(made), flush=True)


def worker():
    code = 0
    try:
        run()
    except Exception as error:
        print(repr(error), file=sys.stderr)
        code = 1
    finally:
        print(f"capture exit {code}", file=sys.stderr)
        sys.stdout.flush()
        sys.stderr.flush()
        xvfb.terminate()
    os._exit(code)


threading.Thread(target=worker, daemon=True).start()
Gtk.main()
