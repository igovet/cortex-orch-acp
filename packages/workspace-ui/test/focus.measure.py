#!/usr/bin/env python3
"""Text-only focus and panel-toggle check in real WebKitGTK. jsdom has no style cascade or focus heuristics, so Vitest cannot answer this.

It builds the demo workspace and opens it in a real WebKit view on a private X display inside the rendered-check image, drives
it with real mouse and keyboard events from xdotool, and reads computed styles and rectangles. No screenshot is taken and no
image is read. Exit code 1 means a press, click or route change left a highlight or moved something, or a panel toggle lied.

    python3 -B packages/workspace-ui/test/focus.measure.py
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import time

IMAGE = "cortex-workspace-rendered:ubuntu26.04"
WINDOW = (1880, 1100)
PARK = (1915, 1195)  # Outside the window, so nothing is hovered.
LOOK = ("outlineStyle", "outlineWidth", "outlineColor", "boxShadow", "borderTopColor", "borderTopWidth", "backgroundColor", "color", "fontWeight")

PAGE = r"""
window.__m = {
  look(el) { const s = getComputedStyle(el); return Object.fromEntries(LOOK.map(k => [k, s[k]])); },
  box(el) { const r = el.getBoundingClientRect(); return [r.x, r.y, r.width, r.height].map(n => Math.round(n * 100) / 100).join(","); },
  check(el) {
    const r = el.getBoundingClientRect(), s = getComputedStyle(el);
    if (r.width < 4 || r.height < 4 || s.visibility === "hidden" || r.top < 0 || r.left < 0 || r.right > innerWidth || r.bottom > innerHeight) return false;
    const hit = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2);
    return !!hit && (hit === el || el.contains(hit) || hit.closest("label")?.contains(el));
  },
  find(selector) { return [...document.querySelectorAll(selector)].find(el => this.check(el)) ?? null; },
  frame() { return [...document.querySelectorAll(".titlebar button, .sidebar .nav, .sidebar .proj, .main [role=tab]")].map(el => this.box(el)).join(";"); },
};
""".replace("LOOK", json.dumps(list(LOOK)))

# Real clicks on what a click leaves unchanged. Every other control is pressed and released away, so it takes focus and :active without acting.
CLICKS = [
    ("home", ".nav[aria-current]", None), ("board", ".proj[aria-current]", None), ("task", '.tab[aria-selected="true"]', None),
    ("task", ".composer textarea", ".composer"), ("new", ".composer textarea", ".composer"), ("new", "textarea", None), ("archive", ".inp", None), ("settings_machine", ".inp", None),
]
# Moving the pointer with the button down would resize or reorder these.
DRAGGED = ".ws-resize, .grip, [draggable='true'], [role='separator']"
SWEEP = r"""
const seen = new Map();
for (const el of document.querySelectorAll("button, a, input, textarea, label, summary, [tabindex], [role='tab'], [role='option'], [role='button'], [role='treeitem']")) {
  if (el.matches(DRAGGED) || el.closest(DRAGGED) || el.matches('main[tabindex], [type=file], .pt')) continue;
  const sign = [el.tagName.toLowerCase(), ...[...el.classList].sort().map(c => '.' + c), ...['aria-current', 'aria-selected', 'aria-pressed', 'type'].filter(a => el.hasAttribute(a)).map(a => `[${a}="${el.getAttribute(a)}"]`)].join('');
  if (seen.has(sign) || !__m.check(el)) continue;
  el.dataset.m = String(seen.size);
  seen.set(sign, `[data-m="${seen.size}"]`);
}
return [...seen];
""".replace("DRAGGED", json.dumps(DRAGGED))


def outside():
    ROOT = Path(__file__).resolve().parents[3]
    desktop = ROOT / "prototypes/cortex-workspace/desktop"
    site = ROOT / ".workspace-build/focus-measure/site"
    site.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(["npx", "vite", "build", "--logLevel", "error", "--emptyOutDir", "--outDir", str(site)], cwd=desktop, check=True)
    command = ["docker", "run", "--rm", "--network", "none", "-e", "HOME=/tmp", "-e", "XDG_RUNTIME_DIR=/tmp", "-e", f"MEASURE_ONLY={os.environ.get('MEASURE_ONLY', '')}", "-v", f"{site}:/site:ro", "-v", f"{Path(__file__).resolve()}:/measure.py:ro",
               IMAGE, "python3", "-B", "/measure.py", "--inside"]
    return subprocess.run(command, timeout=1500).returncode


def inside():
    import gi
    gi.require_version("Gtk", "3.0")
    gi.require_version("WebKit2", "4.1")
    from gi.repository import GLib, Gtk, WebKit2
    from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
    from functools import partial

    class Site(SimpleHTTPRequestHandler):
        def do_GET(self):
            if not (Path("/site") / self.path.split("?")[0].lstrip("/")).is_file():
                self.path = "/index.html"
            super().do_GET()
        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), partial(Site, directory="/site"))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    origin = f"http://127.0.0.1:{server.server_address[1]}"

    win = Gtk.Window()
    win.set_decorated(False)
    win.set_default_size(*WINDOW)
    view = WebKit2.WebView(web_context=WebKit2.WebContext.new_ephemeral())
    win.add(view)
    win.show_all()
    win.move(0, 0)
    failures, passed, facts = [], [], {}

    def on_main(call):
        done, box = threading.Event(), {}
        def run():
            call(box, done)
            return False
        GLib.idle_add(run)
        if not done.wait(15):
            raise TimeoutError("page did not answer")
        return box.get("value")

    def js(body):
        def call(box, done):
            def finished(source, result, _):
                try:
                    box["value"] = source.evaluate_javascript_finish(result).to_string()
                except GLib.Error as error:
                    box["value"] = json.dumps({"error": error.message})
                done.set()
            view.evaluate_javascript(f"JSON.stringify((() => {{ {body} }})() ?? null)", -1, None, None, None, finished, None)
        return json.loads(on_main(call))

    def x(*args):
        subprocess.run(["xdotool", *map(str, args)], check=True)
        time.sleep(.07)

    def load(path):
        on_main(lambda box, done: (view.load_uri(origin + path), done.set()))
        for _ in range(100):
            time.sleep(.1)
            try:
                if js("return document.readyState === 'complete' && !!document.querySelector('.titlebar') && document.querySelectorAll('.nav').length + document.querySelectorAll('.snav').length > 0"):
                    break
            except json.JSONDecodeError:
                pass
        else:
            raise TimeoutError("no workspace at " + path)
        time.sleep(.3)
        js(PAGE + "document.querySelector('.ws-demo-banner')?.remove(); return 1")  # The product has no demo row above its title band.

    def size(width, height):
        on_main(lambda box, done: (win.resize(width, height), done.set()))
        time.sleep(.5)

    def check(name, ok, **details):
        (passed if ok else failures).append(name if ok else {"check": name, **details})

    def diff(a, b):
        return {k: [a[k], b[k]] for k in a if a[k] != b[k]}

    def measure(theme, page, selector, watch, click, label=None):
        name = f"{theme} {page} {label or selector}"
        x("mousemove", *PARK)
        js("document.activeElement?.blur(); return 1")
        point = js(f"""const el = __m.find({json.dumps(selector)}); if (!el) return null; el.scrollIntoView({{ block: 'nearest' }}); window.__el = el; window.__w = {json.dumps(watch)} ? (el.closest({json.dumps(watch or 'x')}) ?? el.parentElement.querySelector({json.dumps(watch or 'x')})) : el;
          const r = el.getBoundingClientRect(); return [Math.round(r.x + r.width / 2), Math.round(r.y + r.height / 2)];""")
        if point is None:
            facts.setdefault("not_on_screen", []).append(name)
            return
        sample = "return { look: __m.look(__w), own: __m.box(__el), frame: __m.frame(), active: document.activeElement.tagName, ring: __m.look(document.activeElement).outlineStyle, visible: document.activeElement.matches(':focus-visible'), kb: document.documentElement.hasAttribute('data-keyboard') };"
        rest = js(sample)
        x("mousemove", *point)
        hover = js(sample)
        if click:
            x("click", 1)
            held = js(sample)
            check(name + " click on hover", not diff(hover["look"], held["look"]), changed=diff(hover["look"], held["look"]))
            x("mousemove", *PARK)
        else:
            x("mousedown", 1)
            held = js(sample)
            check(name + " press", not diff(hover["look"], held["look"]), changed=diff(hover["look"], held["look"]))
            x("mousemove", *PARK)
            x("mouseup", 1)
        after = js(sample)
        check(name + " after", not diff(rest["look"], after["look"]), changed=diff(rest["look"], after["look"]))
        check(name + " focus line", after["ring"] == "none" and not after["kb"], active=after["active"], ring=after["ring"], kb=after["kb"])
        check(name + " no shift", rest["own"] == after["own"] and rest["frame"] == after["frame"], own=[rest["own"], after["own"]])
        facts["mouse_left_focus_on"] = sorted({*facts.get("mouse_left_focus_on", []), after["active"]})
        facts["controls_measured"] = facts.get("controls_measured", 0) + 1
        if after["visible"]:
            facts.setdefault("engine_says_focus_visible_after_mouse", []).append(name)

    def linear(channel):
        c = channel / 255
        return c / 12.92 if c <= .04045 else ((c + .055) / 1.055) ** 2.4

    def luminance(colour):
        r, g, b = [float(n) for n in colour[colour.index("(") + 1:colour.index(")")].replace(",", " ").split()[:3]]
        return .2126 * linear(r) + .7152 * linear(g) + .0722 * linear(b)

    def ratio(a, b):
        hi, lo = sorted((luminance(a), luminance(b)), reverse=True)
        return round((hi + .05) / (lo + .05), 2)

    def panel(theme, label, panel_selector, width):
        name = f"{theme} {label} @{width}"
        probe = f"""const b = document.querySelector('.panel-t[aria-label={json.dumps(label)}]'); if (!b) return null;
          const p = document.querySelector({json.dumps(panel_selector)}), r = b.getBoundingClientRect(), s = getComputedStyle(b), box = p ? p.getBoundingClientRect() : null;
          let under = b.parentElement; while (under && ['rgba(0, 0, 0, 0)', 'transparent'].includes(getComputedStyle(under).backgroundColor)) under = under.parentElement;
          return {{ pressed: b.getAttribute('aria-pressed'), disabled: b.getAttribute('aria-disabled'), color: s.color, background: s.backgroundColor, outline: s.outlineStyle, shadow: s.boxShadow, opacity: s.opacity, at: __m.box(b), point: [Math.round(r.x + r.width / 2), Math.round(r.y + r.height / 2)],
            open: !!box && box.width > 0 && box.height > 0 && getComputedStyle(p).display !== 'none', under: getComputedStyle(under ?? document.body).backgroundColor }};"""
        first = js(probe)
        if first is None:
            facts.setdefault("toggle_absent", []).append(name)
            return
        if first["disabled"]:
            facts.setdefault("toggle_disabled", []).append(name)
            return
        x("mousemove", *first["point"])
        x("click", 1)
        hovering = js(probe)
        x("mousemove", *PARK)
        second = js(probe)
        x("mousemove", *first["point"])
        x("click", 1)
        x("mousemove", *PARK)
        third = js(probe)
        states = {s["pressed"]: s for s in (first, second)}
        check(name + " state is the panel's", all((s["pressed"] == "true") == s["open"] for s in (first, second, third)), states=[(s["pressed"], s["open"]) for s in (first, second, third)])
        check(name + " comes back", third["pressed"] == first["pressed"] and third["open"] == first["open"] and third["color"] == first["color"], states=[(s["pressed"], s["open"], s["color"]) for s in (first, second, third)])
        check(name + " hover adds nothing", hovering["color"] == second["color"] and hovering["background"] == second["background"], hover=hovering["color"], rest=second["color"])
        check(name + " no other mark", all(s["outline"] == "none" and s["shadow"] == "none" and s["background"] == "rgba(0, 0, 0, 0)" for s in (first, second, third, hovering)))
        check(name + " stays put", first["at"] == second["at"] == third["at"], at=[first["at"], second["at"]])
        if set(states) == {"true", "false"}:
            on, off = states["true"], states["false"]
            lighter = luminance(on["color"]) > luminance(off["color"])
            check(name + " lighter when open", lighter, on=on["color"], off=off["color"])
            facts.setdefault("toggles", []).append({"toggle": name, "open": on["color"], "closed": off["color"], "between": ratio(on["color"], off["color"]), "open_on_band": ratio(on["color"], on["under"]), "closed_on_band": ratio(off["color"], off["under"])})
        else:
            check(name + " changes state", False, states=list(states))

    # A switch hides its input under the track it paints. A real click anywhere on that track, with or without a
    # label around the switch, must turn it; a second click turns it back.
    def switches(theme, page):
        # Found again by name before every read: a toggle may render its row anew.
        named = js("const seen = {}; return [...document.querySelectorAll('.sw')].flatMap(s => { const i = s.querySelector('input'), name = i?.getAttribute('aria-label'); if (!name) return []; const k = seen[name] = (seen[name] ?? -1) + 1; return !i.disabled && __m.check(s) ? [[name, k]] : []; });")
        for n, (label, k) in enumerate(named):
            probe = f"""const i = [...document.querySelectorAll('.sw input')].filter(e => e.getAttribute('aria-label') === {json.dumps(label)})[{k}]; if (!i) return null;
              const s = i.parentElement; s.scrollIntoView({{ block: 'nearest' }}); const r = s.getBoundingClientRect(), y = Math.round(r.y + r.height / 2);
              return {{ on: i.checked, name: i.getAttribute('aria-label'), at: [[Math.round(r.x + r.width / 2), y], [Math.round(r.right - 3), y]] }};"""
            states = [js(probe)]
            if states[0] is None:
                facts.setdefault("not_on_screen", []).append(f"{theme} {page} switch {n}")
                continue
            for point in states[0]["at"]:
                x("mousemove", *point)
                x("click", 1)
                time.sleep(.15)
                states.append(js(probe))
            x("mousemove", *PARK)
            turned = None not in states and states[1]["on"] != states[0]["on"] and states[2]["on"] == states[0]["on"]
            check(f"{theme} {page} switch {states[0]['name']} turns on a click", turned, states=[s and s["on"] for s in states])
            facts["switches_clicked"] = facts.get("switches_clicked", 0) + 1

    # What Tab landed on. A switch hides its input and draws the line on the track beside it, so the mark is read where it is painted.
    STOP = """const a = document.activeElement, text = a.matches('textarea, [contenteditable], input:not([type=checkbox], [type=radio])');
      const mark = a.matches('.sw input') ? a.nextElementSibling : a, s = getComputedStyle(mark), r = mark.getBoundingClientRect(), reach = (parseFloat(s.outlineWidth) || 0) + (parseFloat(s.outlineOffset) || 0), clipped = [];
      for (let p = mark.parentElement; p; p = p.parentElement) {
        const c = getComputedStyle(p), b = p.getBoundingClientRect(), left = b.x + p.clientLeft, top = b.y + p.clientTop;
        if (c.overflowX !== 'visible' && (r.left - reach < left - .5 || r.right + reach > left + p.clientWidth + .5)) clipped.push(p.className || p.tagName);
        if (c.overflowY !== 'visible' && (r.top - reach < top - .5 || r.bottom + reach > top + p.clientHeight + .5)) clipped.push(p.className || p.tagName);
      }
      return { tag: a.tagName, name: a.getAttribute('aria-label') || a.textContent.trim().slice(0, 40), text, style: s.outlineStyle, width: s.outlineWidth, colour: s.outlineColor, clipped, kb: document.documentElement.hasAttribute('data-keyboard') };"""

    def drawn(state):
        return state["kb"] and (state["style"] == "none" if state["text"] else state["style"] == "solid" and state["width"] == "1px" and not state["clipped"])

    def keyboard(theme, path, start):
        load(path)
        x("mousemove", *PARK)
        js(start)
        seen = {}
        for _ in range(45):
            x("key", "Tab")
            state = js(STOP)
            if state["tag"] == "BODY":
                continue
            kind = "text field" if state["text"] else "control"
            look = f"{state['style']} {state['width']} {state['colour']}"
            seen.setdefault(kind, set()).add(look)
            check(f"{theme} {path} Tab {kind}", drawn(state), state=state)
        for kind, looks in seen.items():
            facts.setdefault("keyboard", {}).setdefault(theme, {}).setdefault(kind, set()).update(looks)
        js("return 1")
        x("mousemove", 900, 700)
        x("click", 1)
        cleared = js("const a = document.activeElement; return { kb: document.documentElement.hasAttribute('data-keyboard'), ring: getComputedStyle(a).outlineStyle };")
        check(f"{theme} a click ends the line", not cleared["kb"] and cleared["ring"] == "none", state=cleared)
        x("mousemove", *PARK)

    def run():
        load("/")
        machine = js("return location.pathname.split('/')[2]")
        project = js("return document.querySelector('.proj').getAttribute('href').split('/')[4]")
        load(f"/m/{machine}/p/{project}/board")
        task = js("return document.querySelector('.card-link').getAttribute('href').split('/')[4]")
        at, pj, tk, st = f"/m/{machine}", f"/m/{machine}/p/{project}", f"/m/{machine}/t/{task}", f"/settings/machine/{machine}"
        routes = {"home": at, "inbox": f"{at}/inbox", "archive": f"{at}/archive", "new": f"{at}/new", "new_sprint": f"{at}/new?kind=sprint", "board": f"{pj}/board", "list": f"{pj}/list", "code": f"{pj}/code", "git": f"{pj}/git",
                  "worktrees": f"{pj}/worktrees", "project_settings": f"{pj}/settings", "task": tk, "task_workers": f"{tk}/workers", "task_changes": f"{tk}/changes", "task_code": f"{tk}/code",
                  "appearance": "/settings/app/appearance", "voice": "/settings/app/voice", "shortcuts": "/settings/app/shortcuts", "machines": "/settings/app/machines", "settings_machine": f"{st}/overview",
                  "connections": f"{st}/connections", "storage": f"{st}/storage", "machine_worktrees": f"{st}/worktrees", "defaults": f"{st}/orchestration/defaults", "models": f"{st}/orchestration/models",
                  "routing": f"{st}/orchestration/routing", "tools": f"{st}/orchestration/tools", "profiles": f"{st}/orchestration/profiles", "policy": f"{st}/orchestration/policy"}
        only = os.environ.get("MEASURE_ONLY")
        for theme in ("dark", "light"):
            load("/")
            js(f"localStorage.setItem('cw.theme', {json.dumps(theme)}); return 1")
            for page, path in routes.items():
                if only and page != only:
                    continue
                load(path)
                check(f"{theme} theme applied", js("return document.documentElement.dataset.theme") == theme)
                for shown, selector, watch in CLICKS:
                    if shown == page:
                        measure(theme, page, selector, watch, True)
                for sign, selector in js(SWEEP):
                    measure(theme, page, selector, None, False, sign)
                switches(theme, page)

            # Route change by a real click: focus falls to the main area, which draws nothing.
            load(routes["home"])
            nav = js("const r = __m.find('.nav:not([aria-current])').getBoundingClientRect(); return [Math.round(r.x + 20), Math.round(r.y + r.height / 2)]")
            x("mousemove", *nav)
            x("click", 1)
            time.sleep(.3)
            x("mousemove", *PARK)
            routed = js("const a = document.activeElement, s = getComputedStyle(a); return { tag: a.tagName, ring: s.outlineStyle, shadow: s.boxShadow };")
            check(f"{theme} route change", routed["ring"] == "none" and routed["shadow"] == "none", state=routed)
            facts["route_change_focus"] = routed["tag"]

            # A popover menu and a dialog opened with the mouse.
            load(routes["home"])
            x("mousemove", *js("const r = __m.find('.machine').getBoundingClientRect(); return [Math.round(r.x + 40), Math.round(r.y + 14)]"))
            x("click", 1)
            item = js("const el = __m.find('[popover]:popover-open .mi'); if (!el) return null; window.__el = window.__w = el; const r = el.getBoundingClientRect(); return [Math.round(r.x + 40), Math.round(r.y + r.height / 2)];")
            if item:
                x("mousemove", *item)
                hover = js("return __m.look(__w)")
                x("mousedown", 1)
                held = js("return __m.look(__w)")
                check(f"{theme} menu item press", not diff(hover, held), changed=diff(hover, held))
                x("mousemove", *PARK)
                x("mouseup", 1)
                ring = js("return { ring: getComputedStyle(document.activeElement).outlineStyle, kb: document.documentElement.hasAttribute('data-keyboard') }")
                check(f"{theme} menu item focus", ring["ring"] == "none" and not ring["kb"], state=ring)
            else:
                facts.setdefault("not_on_screen", []).append(f"{theme} menu item")
            x("key", "Escape")

            load("/settings/app/machines")
            opener = js("const el = [...document.querySelectorAll('.main button')].find(b => /connect|add/i.test(b.textContent)); if (!el) return null; const r = el.getBoundingClientRect(); return [Math.round(r.x + r.width / 2), Math.round(r.y + r.height / 2)];")
            if opener:
                x("mousemove", *opener)
                x("click", 1)
                time.sleep(.2)
                shown_dialog = js("const a = document.activeElement, s = getComputedStyle(a); return { open: !!document.querySelector('dialog[open]'), tag: a.tagName, ring: s.outlineStyle, visible: a.matches(':focus-visible') };")
                check(f"{theme} dialog from a click", shown_dialog["open"] and shown_dialog["ring"] == "none", state=shown_dialog)
                facts["dialog_focus"] = shown_dialog
                for _ in range(4):  # Past a dialog's last control the engine rests on the document and on the dialog itself, which draw nothing.
                    x("key", "Tab")
                    inside_dialog = js(STOP)
                    if inside_dialog["tag"] not in ("BODY", "DIALOG"):
                        break
                check(f"{theme} Tab inside a dialog", drawn(inside_dialog), state=inside_dialog)
                x("key", "Escape")
            else:
                facts.setdefault("not_on_screen", []).append(f"{theme} dialog opener")

            # Panels, against the state of the real panel.
            load(routes["task"])
            for label, selector in (("Sidebar", ".sidebar"), ("Inspector", ".inspector"), ("Worker pane", ".detail"), ("Terminal", ".terminal")):
                panel(theme, label, selector, WINDOW[0])
            size(1300, 900)
            load(routes["task"])
            for label, selector in (("Sidebar", ".sidebar"), ("Inspector", ".inspector")):
                panel(theme, label, selector, 1300)
            size(*WINDOW)
            load(routes["board"])
            panel(theme, "Terminal", ".terminal", WINDOW[0])
            # From the top of the window, then from the page itself, because the sidebar alone takes most of the walk.
            for page in ("new", "home", "board", "task", "git", "appearance", "routing"):
                keyboard(theme, routes[page], "document.activeElement?.blur(); return 1")
            for page in ("home", "inbox", "board", "list", "code", "git", "worktrees", "task", "task_workers", "task_changes", "settings_machine", "routing", "tools", "profiles"):
                keyboard(theme, routes[page], "document.querySelector('main').focus(); return 1")

    def main():
        code = 1
        try:
            time.sleep(1)
            run()
            code = 1 if failures else 0
        except Exception as error:  # A broken run is a failed run, never a pass.
            failures.append({"check": "harness", "error": repr(error)})
        print(json.dumps({"engine": f"WebKitGTK {WebKit2.get_major_version()}.{WebKit2.get_minor_version()}.{WebKit2.get_micro_version()}", "passed": len(passed), "failed": len(failures), "facts": facts, "failures": failures}, indent=1, default=sorted))
        sys.stdout.flush()
        os._exit(code)

    threading.Thread(target=main, daemon=True).start()
    Gtk.main()


def container():
    xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "1920x1200x24", "-nolisten", "tcp"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    display = xvfb.stdout.readline().strip()
    env = {**os.environ, "DISPLAY": f":{display}", "GDK_BACKEND": "x11", "NO_AT_BRIDGE": "1", "WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS": "1", "WEBKIT_DISABLE_COMPOSITING_MODE": "1"}
    os.environ.update(env)
    subprocess.Popen(["openbox"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(.5)
    inside()


if __name__ == "__main__":
    if "--inside" in sys.argv:
        container()
    else:
        sys.exit(outside())
