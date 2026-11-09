#!/usr/bin/env python3
"""Grab the scrollbar and the window edge of the real desktop window with real pointer input.

Runs inside the rendered-check image against a built window and service binary, on a private
Xvfb with Openbox, a disposable HOME and no network. Pointer input is XTEST through xdotool, so it
passes through GTK and the undecorated-window edge handlers exactly like a mouse. The same
read-only probe as the rendered journey reports positions; it dispatches nothing.

    docker run --rm --network=none --user UID:GID --shm-size=512m --cap-drop=ALL \
      --security-opt=seccomp=unconfined --security-opt=apparmor=unconfined \
      --mount type=bind,source=$PWD/.workspace-build/worker-feed-ui/build/app,target=/artifacts,readonly \
      --mount type=bind,source=$PWD,target=/checkout,readonly \
      --mount type=bind,source=$PWD/.workspace-build/worker-feed-ui,target=/evidence \
      cortex-workspace-rendered:ubuntu26.04 python3 -B /checkout/packages/workspace-ui/test/scrollbar.measure.py new
"""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path("/checkout")
ARTIFACTS = Path("/artifacts")
EVIDENCE = Path("/evidence")
SCREEN = (1600, 1000)
CHANNEL = json.loads((ROOT / "crates/workspace-protocol/channels.json").read_text())["dev"]


def run(args, **kw):
    return subprocess.run(args, capture_output=True, text=True, timeout=30, **kw)


def session(label):
    sys.path.insert(0, str(ROOT / "packaging/container"))
    from check_rendered import PROBE
    scratch = Path(os.environ["CW_SCRATCH"])
    probe_dir = scratch / "probe"
    probe_dir.mkdir()
    source = scratch / "probe.c"
    source.write_text(PROBE)
    shim = scratch / "probe.so"
    flags = subprocess.check_output(["pkg-config", "--cflags", "--libs", "webkit2gtk-4.1", "gtk+-3.0"], text=True).split()
    subprocess.run(["gcc", "-shared", "-fPIC", "-O1", "-w", "-o", str(shim), str(source), *flags], check=True)
    xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", f"{SCREEN[0]}x{SCREEN[1]}x24", "-nolisten", "tcp"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    os.environ["DISPLAY"] = ":" + xvfb.stdout.readline().strip()
    os.environ["GDK_BACKEND"] = "x11"
    os.environ["WEBKIT_DISABLE_COMPOSITING_MODE"] = "1"
    wm = subprocess.Popen(["openbox", "--sm-disable"], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(1.5)
    (scratch / "projects").mkdir()
    procs = [xvfb, wm]
    service = subprocess.Popen([str(ARTIFACTS / CHANNEL["service_binary"]), str(scratch / "projects"), "/usr/bin/python3", str(ROOT), "--fixture-launcher", str(ROOT / "tests/test_workspace_journey.py")], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=(scratch / "service.log").open("ab"))
    procs.append(service)
    socket = scratch / "runtime" / CHANNEL["runtime_namespace"] / "service.sock"
    for _ in range(100):
        if socket.exists():
            break
        time.sleep(.1)
    app_env = {**os.environ, "LD_PRELOAD": str(shim), "CW_PROBE_DIR": str(probe_dir)}
    log = (scratch / "app.stderr").open("wb")
    app = subprocess.Popen([str(ARTIFACTS / CHANNEL["desktop_binary"])], env=app_env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=log)
    procs.append(app)
    results = {"binary": label, "service_up": socket.exists()}
    seq = [0]

    def js(body, seconds=15):
        answer = probe_dir / "response.json"
        answer.unlink(missing_ok=True)
        seq[0] += 1
        temporary = probe_dir / f"request-{seq[0]}.tmp"
        temporary.write_text(body)
        temporary.rename(probe_dir / "request.js")
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if answer.exists():
                text = answer.read_text()
                if text.startswith("!"):
                    raise RuntimeError(text)
                return text
            if app.poll() is not None:
                raise RuntimeError("app exited")
            time.sleep(.03)
        raise TimeoutError(body[:60])

    def query(body):
        return json.loads(js(body))

    def wait_js(body, seconds=30):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            try:
                if js(body) == "yes":
                    return
            except (RuntimeError, TimeoutError):
                pass
            time.sleep(.2)
        raise TimeoutError("page: " + body[:60])

    def win_id():
        for _ in range(100):
            out = run(["xdotool", "search", "--onlyvisible", "--pid", str(app.pid)]).stdout.split()
            if out:
                return out[-1]
            out = run(["xdotool", "search", "--onlyvisible", "--name", ".*"]).stdout.split()
            for w in reversed(out):
                if "Cortex" in run(["xdotool", "getwindowname", w]).stdout:
                    return w
            time.sleep(.2)
        raise TimeoutError("window")

    def geometry(window):
        out = run(["xdotool", "getwindowgeometry", "--shell", window]).stdout
        values = dict(line.split("=", 1) for line in out.splitlines() if "=" in line)
        return int(values["X"]), int(values["Y"]), int(values["WIDTH"]), int(values["HEIGHT"])

    def pointer(x, y):
        run(["xdotool", "mousemove", "--sync", str(x), str(y)])

    def drag(x0, y0, x1, y1):
        pointer(x0, y0)
        time.sleep(.2)
        run(["xdotool", "mousedown", "1"])
        time.sleep(.2)
        for i in range(1, 13):
            pointer(x0 + (x1 - x0) * i // 12, y0 + (y1 - y0) * i // 12)
            time.sleep(.03)
        time.sleep(.2)
        run(["xdotool", "mouseup", "1"])
        time.sleep(.5)

    def click(x, y):
        pointer(x, y)
        time.sleep(.2)
        run(["xdotool", "click", "1"])
        time.sleep(.5)

    def screenshot(name):
        import gi
        gi.require_version("Gdk", "3.0")
        from gi.repository import Gdk
        root = Gdk.get_default_root_window()
        Gdk.pixbuf_get_from_window(root, 0, 0, SCREEN[0], SCREEN[1]).savev(str(EVIDENCE / "shots" / f"real-{label}-{name}.png"), "png", [], [])

    try:
        wait_js("return document.readyState === 'complete' && document.querySelector('.titlebar') ? 'yes' : ''")
        js("await document.fonts.ready; return 'yes';")
        window = win_id()
        run(["xdotool", "windowsize", "--sync", window, "1100", "640"])
        run(["xdotool", "windowmove", window, "100", "60"])
        time.sleep(1)
        results["window"] = geometry(window)
        # The Settings link, then the longest settings page, by real clicks.
        x, y, w, h = geometry(window)
        box = query("const el=document.querySelector('a[aria-label=\"Settings\"]');const r=el.getBoundingClientRect();return JSON.stringify([r.x+r.width/2,r.y+r.height/2])")
        click(x + int(box[0]), y + int(box[1]))
        wait_js("return document.querySelector('.set-main') ? 'yes' : ''")
        nav = query("const a=[...document.querySelectorAll('.set-nav a, .set-side a, nav a')].find(a=>/Shortcuts/.test(a.textContent));const r=a.getBoundingClientRect();return JSON.stringify([r.x+r.width/2,r.y+r.height/2])")
        click(x + int(nav[0]), y + int(nav[1]))
        wait_js("return document.querySelector('.set-main') && /Shortcuts/.test(document.querySelector('.set-main').textContent) ? 'yes' : ''")
        time.sleep(.5)
        # Test scaffolding only: this disposable page must be longer than the window. Input still comes from the real pointer.
        js("const f=document.createElement('div');f.style.height='2400px';document.querySelector('.set-main').append(f);return 'yes';")
        time.sleep(.3)
        state = lambda: query("const el=document.querySelector('.set-main');const r=el.getBoundingClientRect();return JSON.stringify({top:el.scrollTop,sh:el.scrollHeight,ch:el.clientHeight,left:r.left,right:r.right,bar:el.offsetWidth-el.clientWidth,iw:innerWidth,ih:innerHeight,edge:getComputedStyle(document.documentElement).getPropertyValue('--ws-edge'),dpr:devicePixelRatio})")
        before = state()
        results["before"] = before
        assert before["sh"] > before["ch"] + 50, "page must scroll for this check"
        win_before = geometry(window)
        screenshot("before")
        # The thumb is 5px wide, 3px in from each side of the 11px bar: grab both sides of it.
        bar_left = before["right"] - before["bar"]
        thumb_top = before["ch"] * before["top"] / before["sh"]
        attempts = {}
        for side, offset in (("thumb-left", 3), ("thumb-right", before["bar"] - 4)):
            s0, g0 = state(), geometry(window)
            grab_x = x + int(bar_left + offset)
            grab_y = y + 150 + int(thumb_top)
            # A hand drifts sideways while dragging; a pure vertical move would hide an edge resize.
            drag(grab_x, grab_y, grab_x - 8, grab_y + 90)
            s1, g1 = state(), geometry(window)
            attempts[side] = {"scrollDelta": s1["top"] - s0["top"], "grabbedAt": grab_x - x, "windowFrom": g0, "windowTo": g1, "resized": g0 != g1}
            pointer(5, 5)
            time.sleep(.2)
            if g0 == g1:
                # Back to the top for the next grab, by real wheel.
                run(["xdotool", "click", "--repeat", "20", "--delay", "10", "4"])
                time.sleep(.3)
        results["scrollbar"] = attempts
        screenshot("after-drag")
        # The right edge itself still resizes the window.
        g0 = geometry(window)
        edge_x = g0[0] + g0[2] - 2
        drag(edge_x, g0[1] + g0[3] // 2, edge_x + 60, g0[1] + g0[3] // 2)
        g1 = geometry(window)
        results["edge_resize_right"] = {"from": g0, "to": g1}
        results["ok"] = all(a["scrollDelta"] > 40 and not a["resized"] for a in attempts.values()) and (g1[2] - g0[2] >= 40)
        results["after"] = state()
        screenshot("after-resize")
    except Exception as error:
        results["error"] = repr(error)
        results["ok"] = False
    finally:
        for proc in reversed(procs):
            if proc.poll() is None:
                proc.send_signal(signal.SIGTERM)
                try:
                    proc.wait(5)
                except subprocess.TimeoutExpired:
                    proc.kill()
    (EVIDENCE / f"scrollbar-{label}.json").write_text(json.dumps(results, indent=2) + "\n")
    print(json.dumps(results))
    return 0 if results.get("ok") else 1


def main():
    label = sys.argv[1] if len(sys.argv) > 1 else "new"
    if os.environ.get("CW_SCRATCH"):
        return session(label)
    with tempfile.TemporaryDirectory(prefix="cw-scroll-") as directory:
        scratch = Path(directory)
        for name in ("home", "config", "state", "cache", "runtime"):
            (scratch / name).mkdir(mode=0o700)
        env = {"PATH": "/usr/local/bin:/usr/bin:/bin", "LANG": "C.UTF-8", "HOME": str(scratch / "home"), "XDG_CONFIG_HOME": str(scratch / "config"), "XDG_STATE_HOME": str(scratch / "state"), "XDG_CACHE_HOME": str(scratch / "cache"), "XDG_RUNTIME_DIR": str(scratch / "runtime"), "PYTHONDONTWRITEBYTECODE": "1", "CW_SCRATCH": str(scratch)}
        return subprocess.run(["dbus-run-session", "--", sys.executable, "-B", __file__, label], env=env).returncode


if __name__ == "__main__":
    raise SystemExit(main())
