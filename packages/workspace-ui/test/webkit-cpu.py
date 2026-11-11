"""Opt-in /proc CPU measurements; fake data on an owned software virtual screen."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time

scratch = tempfile.TemporaryDirectory(prefix="webkit-cpu-")
for key, sub in (("HOME", "home"), ("XDG_CONFIG_HOME", "config"), ("XDG_STATE_HOME", "state"),
                 ("XDG_CACHE_HOME", "cache"), ("XDG_DATA_HOME", "data"), ("XDG_RUNTIME_DIR", "runtime")):
    path = Path(scratch.name) / sub
    path.mkdir(mode=0o700)
    os.environ[key] = str(path)
xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "1440x1000x24",
                         "-nolisten", "tcp"], stdout=subprocess.PIPE, text=True)
os.environ.update(DISPLAY=":" + xvfb.stdout.readline().strip(), GDK_BACKEND="x11",
                  NO_AT_BRIDGE="1", LIBGL_ALWAYS_SOFTWARE="1", WEBKIT_DISABLE_COMPOSITING_MODE="1")
import gi
gi.require_version("Gtk", "3.0")
gi.require_version("WebKit2", "4.1")
from gi.repository import Gtk, GLib, WebKit2

win = Gtk.Window()
win.set_default_size(1440, 1000)
view = WebKit2.WebView(web_context=WebKit2.WebContext.new_ephemeral())
win.add(view)
win.show_all()
exit_code = 0


def js(code):
    done, box = threading.Event(), {}
    def call():
        def finish(source, result, _):
            try:
                box["value"] = json.loads(source.evaluate_javascript_finish(result).to_string())
            except Exception as error:
                box["error"] = str(error)
            done.set()
        view.evaluate_javascript("JSON.stringify((()=>{" + code + "})()??null)",
                                 -1, None, None, None, finish, None)
        return False
    GLib.idle_add(call)
    assert done.wait(5), "JavaScript callback timeout"
    if "error" in box:
        raise RuntimeError(box["error"])
    return box["value"]


def snapshot():
    rows = {}
    for p in Path("/proc").iterdir():
        if not p.name.isdigit():
            continue
        try:
            raw = (p / "stat").read_text()
            fields = raw[raw.rfind(")") + 2:].split()
            rows[int(p.name)] = (int(fields[1]), raw[raw.find("(") + 1:raw.rfind(")")],
                                int(fields[11]) + int(fields[12]))
        except OSError:
            pass
    return rows


def measure(state, seconds=3):
    a = snapshot()
    owned = {os.getpid(), xvfb.pid}
    for _ in range(6):
        owned |= {p for p, v in a.items() if v[0] in owned}
    start = time.monotonic()
    time.sleep(seconds)
    elapsed = time.monotonic() - start
    b = snapshot()
    hz = os.sysconf("SC_CLK_TCK")
    print(json.dumps(dict(state=state, seconds=elapsed, hz=hz, processes=[
        dict(pid=p, name=b[p][1], cpuPercent=100 * (b[p][2] - a[p][2]) / hz / elapsed)
        for p in sorted(owned) if p in a and p in b])), flush=True)


def run():
    global exit_code
    try:
        for _ in range(100):
            if js("return !!(window.fixture && document.querySelector('.titlebar'))"):
                break
            time.sleep(.1)
        else:
            raise TimeoutError("fixture readiness")
        running = "return document.getAnimations().filter(a=>a.playState==='running').map(a=>a.animationName)"
        # Nothing pending: the task has settled, so no animation may run at all.
        js("fixture.working(false)")
        time.sleep(1.5)
        print(json.dumps(dict(engine="WebKitGTK", version=[WebKit2.get_major_version(),
              WebKit2.get_minor_version(), WebKit2.get_micro_version()],
              nodes=js("return document.querySelectorAll('*').length"),
              animations=js("return document.getAnimations().map(a=>({name:a.animationName,state:a.playState}))"))), flush=True)
        assert js(running) == [], "An animation runs with nothing pending"
        measure("idle-nothing-pending")
        # A pressed control awaiting its reply: its bar sweeps after 150 ms, and stops the moment the reply settles.
        assert js("""const b = [...document.querySelectorAll('.titlebar .btn, .main .btn')].find(x => x.getClientRects().length);
          if (!b) return false; b.setAttribute('data-busy', ''); b.setAttribute('aria-busy', 'true'); window.__busy = b; return true;"""), "No visible button"
        time.sleep(.4)
        pressed = js(running)
        print(json.dumps(dict(state="button-pending", running=pressed)), flush=True)
        assert "ws-busy-run" in pressed, "A pending button shows no moving loader"
        measure("button-pending")
        js("__busy.removeAttribute('data-busy'); __busy.removeAttribute('aria-busy')")
        time.sleep(.2)
        assert js(running) == [], "A settled button keeps animating"
        # A running task: its long-running working status turns slowly, and only while the window is shown.
        js("fixture.working(true)")
        time.sleep(1)
        work = js("""return document.getAnimations().filter(a=>a.playState==='running').map(a=>({name:a.animationName,
          ms:a.effect.getTiming().duration,easing:a.effect.getTiming().easing}))""")
        print(json.dumps(dict(state="working-visible", running=work)), flush=True)
        assert work and all(a["name"] == "spin" for a in work), "A working task shows no slow loader"
        measure("working-visible")
        GLib.idle_add(win.hide)
        for _ in range(30):
            if js("return document.hidden"):
                break
            time.sleep(.1)
        if js("return document.hidden"):
            assert js(running) == [], "A hidden window keeps animating"
            measure("working-hidden")
        else:
            print(json.dumps(dict(state="working-hidden", result="not_observable",
                                  reason="document.hidden did not become true after hiding the window")), flush=True)
        GLib.idle_add(win.show_all)
        time.sleep(.5)
        js("window.cpuUpdates=0;window.cpuTimer=setInterval(()=>{fixture.emit(true);window.cpuUpdates++},1000)")
        measure("working-updates-one-per-second", 4)
        assert js("clearInterval(window.cpuTimer);return window.cpuUpdates") >= 3
    except Exception as error:
        exit_code = 1
        print(type(error).__name__ + ": " + str(error), file=sys.stderr, flush=True)
    finally:
        GLib.idle_add(Gtk.main_quit)


view.load_uri(sys.argv[1])
threading.Thread(target=run, daemon=True).start()
try:
    Gtk.main()
finally:
    win.destroy()
    xvfb.terminate()
    xvfb.wait(timeout=5)
    scratch.cleanup()
sys.exit(exit_code)
