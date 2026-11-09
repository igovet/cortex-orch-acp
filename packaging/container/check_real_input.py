#!/usr/bin/env python3
"""Click the shipped desktop window the way a person does, and fail if a click changes nothing.

Every click is a real pointer event from the X server (XTEST through xdotool), so it passes through
hit-testing, CSS pointer rules, overlays, inert regions and the page's own handlers exactly like a
mouse. Nothing is dispatched into the page: no DOM event, no LD_PRELOAD probe, no accessible action.
AT-SPI is only read, to find where a control is and to see what the screen shows after the click.
No screenshot is taken and no image is read.

Each scenario runs the installed binaries with a disposable HOME, XDG_RUNTIME_DIR, session bus and
X server, and leaves nothing running. The "plan without runtime tasks" scenario is the state the
owner's machine was in: a project whose runtime is away while its plan names tasks. A scenario only
clicks once the machine reads Connected and its fixture is on screen, so a dead service cannot pass.

    /usr/bin/python3 -B packaging/container/check_real_input.py [--prefix /usr] [--channel dev] [--app PATH]

The service accepts only its installed runtime or a source checkout, so a build that is not installed
is checked by passing its desktop binary as --app; the window is the part a person clicks.

Needs Xvfb, xdotool, dbus-run-session and python3-gi with Atspi on the host python.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
CHANNELS = json.loads((ROOT / "crates/workspace-protocol/channels.json").read_text())
CLICKABLE = ("push button", "button", "toggle button", "link", "page tab", "menu item")
# What a working window shows after each click: the control to press and what must then be on screen.
JOURNEY = [
    ("Inbox", ("heading", "Inbox")),
    ("Archive", ("entry", "Filter archived tasks")),
    ("Settings", ("link", "Back to workspace")),
    ("Back to workspace", ("link", "Inbox")),
    ("Sidebar", None),
]


def seed_plan_without_runtime_tasks(home: Path, channel: dict) -> None:
    """A registered project that is not running, whose saved plan names two tasks the runtime has never listed."""
    state = home / ".local/state" / channel["state_namespace"]
    project, folder = "wp_" + "0" * 31 + "1", home / "fixture-project"
    folder.mkdir(parents=True)
    (state / "product").mkdir(parents=True, mode=0o700)
    os.chmod(state, 0o700)
    registry = [{"projectId": project, "canonicalRoot": str(folder), "displayName": "Fixture project", "activation": None}]
    sprint = {"id": "sp_fixture", "projectId": project, "description": "Fixture sprint", "title": "Fixture sprint", "runMode": "automatic",
              "targetBranch": None, "planning": "created", "conversation": [], "plan": None, "materializedVersion": 1,
              "taskIds": ["wt_not_loaded_1", "wt_not_loaded_2"], "queueState": "idle", "pauseReason": None}
    for path, value in ((state / "projects.json", registry), (state / "product" / f"{project}.json", {"tasks": {}, "sprints": [sprint]})):
        path.write_text(json.dumps(value))
        os.chmod(path, 0o600)


def inner(prefix: Path, channel: dict, base: Path, app: Path, fixture: str | None) -> dict:
    import gi
    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi

    env, display = dict(os.environ), os.environ["DISPLAY"]
    os.makedirs(Path(env["XDG_RUNTIME_DIR"]) / channel["runtime_namespace"], mode=0o700, exist_ok=True)
    runtime = prefix / "lib" / channel["package"] / "runtime"
    procs = [subprocess.Popen([str(prefix / "bin" / channel["service_binary"]), "/", "/usr/bin/python3", str(runtime)], env=env,
                              stdout=open(base / "service.log", "w"), stderr=subprocess.STDOUT)]
    app_env = dict(env, GDK_BACKEND="x11")
    app_env.pop("WAYLAND_DISPLAY", None)
    procs.append(subprocess.Popen([str(app)], env=app_env,
                                  stdout=open(base / "app.log", "w"), stderr=subprocess.STDOUT))
    steps, ok = [], True

    def tree():
        desk = Atspi.get_desktop(0)
        app = next((desk.get_child_at_index(i) for i in range(desk.get_child_count())
                    if (desk.get_child_at_index(i).get_name() or "") == channel["desktop_binary"]), None)
        found, stack = [], [app] if app else []
        while stack and len(found) < 5000:
            node = stack.pop()
            try:
                states = node.get_state_set()
                if states.contains(Atspi.StateType.SHOWING):
                    try:
                        box = node.get_extents(Atspi.CoordType.SCREEN)
                    except Exception:
                        box = Atspi.Rect()
                    found.append((node.get_role_name(), node.get_name() or "", box.x, box.y, box.width, box.height))
                stack.extend(node.get_child_at_index(i) for i in reversed(range(node.get_child_count())))
            except Exception:
                continue
        return found

    def shown(role: str, name: str, nodes) -> bool:
        return any(r == role and n == name for r, n, *_ in nodes)

    def wait_for(test, seconds: float):
        deadline = time.monotonic() + seconds
        while True:
            nodes = tree()
            if test(nodes) or time.monotonic() > deadline:
                return nodes
            time.sleep(0.25)

    try:
        ready = lambda n: shown("link", "Inbox", n) and any(name == "This machine Connected" for _, name, *_ in n) \
            and (fixture is None or any(fixture in name for _, name, *_ in n))
        nodes = wait_for(ready, 60)
        if not ready(nodes):
            return {"ok": False, "steps": [{"step": "window", "ok": False, "reason": "no sidebar, no connected machine or no fixture on screen"}]}
        # The service and the page settle (projects, plan) before the first click, as they would for a person.
        time.sleep(4)
        for label, expect in JOURNEY:
            nodes = tree()
            target = next(((x, y, w, h) for r, n, x, y, w, h in nodes if n == label and r in CLICKABLE and w > 0 and h > 0), None)
            if target is None:
                steps.append({"step": label, "ok": False, "reason": "control not on screen"})
                ok = False
                continue
            x, y = target[0] + target[2] // 2, target[1] + target[3] // 2
            subprocess.run(["xdotool", "mousemove", "--sync", str(x), str(y)], env=env, check=True)
            time.sleep(0.2)
            subprocess.run(["xdotool", "click", "1"], env=env, check=True)
            if expect is None:  # The sidebar toggle hides the sidebar: its Inbox link leaves the screen.
                after = wait_for(lambda n: not shown("link", "Inbox", n), 5)
                passed = not shown("link", "Inbox", after)
            else:
                after = wait_for(lambda n, e=expect: shown(*e, n), 5)
                passed = shown(*expect, after)
            steps.append({"step": label, "at": [x, y], "ok": passed, **({} if passed else {"reason": f"after the click no {expect or 'hidden sidebar'}"})})
            ok &= passed
        return {"ok": ok, "steps": steps}
    finally:
        for p in reversed(procs):
            if p.poll() is None:
                p.send_signal(signal.SIGTERM)
            try:
                p.wait(10)
            except subprocess.TimeoutExpired:
                p.kill()
                p.wait()


def scenario(name: str, prefix: Path, channel: dict, display: str, app: Path) -> dict:
    base = Path(tempfile.mkdtemp(prefix="cw-real-input-"))
    try:
        home, run = base / "home", base / "run"
        home.mkdir()
        run.mkdir(mode=0o700)
        fixture = None
        if name == "plan without runtime tasks":
            seed_plan_without_runtime_tasks(home, channel)
            fixture = "Fixture project"
        env = {"HOME": str(home), "XDG_RUNTIME_DIR": str(run), "DISPLAY": display, "PATH": "/usr/local/bin:/usr/bin:/bin",
               "LANG": os.environ.get("LANG", "C.UTF-8")}
        out = subprocess.run(["dbus-run-session", "--", "/usr/bin/python3", "-I", "-B", __file__, "--inner", str(prefix), channel["name"], str(base),
                              str(app), fixture or ""],
                             env=env, capture_output=True, text=True, timeout=300, cwd=base)
        line = next((l for l in out.stdout.splitlines() if l.startswith("RESULT ")), None)
        result = json.loads(line[7:]) if line else {"ok": False, "steps": [{"step": "run", "ok": False, "reason": f"no result, exit {out.returncode}"}]}
        # Counted markers only: logs can carry private paths, so no body is retained.
        app_log = (base / "app.log").read_text(errors="replace") if (base / "app.log").exists() else ""
        result["app_panics"] = app_log.count("panicked")
        return {"scenario": name, **result}
    finally:
        shutil.rmtree(base, ignore_errors=True)


def main() -> int:
    if len(sys.argv) > 1 and sys.argv[1] == "--inner":
        print("RESULT " + json.dumps(inner(Path(sys.argv[2]), CHANNELS[sys.argv[3]], Path(sys.argv[4]), Path(sys.argv[5]), sys.argv[6] or None)), flush=True)
        return 0
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--prefix", type=Path, default=Path("/usr"), help="the directory holding bin/ and lib/ of the package (installed: /usr)")
    parser.add_argument("--channel", choices=sorted(CHANNELS), default="dev")
    parser.add_argument("--app", type=Path, help="the desktop binary to click (default: the one under --prefix)")
    args = parser.parse_args()
    channel = CHANNELS[args.channel]
    app = (args.app or args.prefix / "bin" / channel["desktop_binary"]).resolve()
    for tool in ("Xvfb", "xdotool", "dbus-run-session"):
        if not shutil.which(tool):
            print(f"missing {tool}", file=sys.stderr)
            return 2
    read, write = os.pipe()
    xvfb = subprocess.Popen(["Xvfb", "-displayfd", str(write), "-screen", "0", "1600x1000x24", "-nolisten", "tcp"],
                            pass_fds=(write,), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    os.close(write)
    try:
        display = ":" + os.read(read, 32).decode().strip()
        os.close(read)
        results = [scenario(name, args.prefix, channel, display, app) for name in ("fresh home", "plan without runtime tasks")]
    finally:
        xvfb.terminate()
        xvfb.wait(10)
    print(json.dumps(results, indent=1))
    return 0 if all(r["ok"] and not r["app_panics"] for r in results) else 1


if __name__ == "__main__":
    sys.exit(main())
