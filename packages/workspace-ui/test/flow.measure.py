#!/usr/bin/env python3
"""Text-only check of the task and board flows in real WebKitGTK. jsdom has no layout, cascade or animation, so Vitest cannot answer this.

It builds the demo workspace, opens it in a real WebKit view on a private X display inside the rendered-check image, drives it
with real mouse events from xdotool, and reads rectangles, computed styles and running animations. No screenshot is taken and
no image is read. Exit code 1 means something blinked, moved, wrapped or vanished.

    python3 -B packages/workspace-ui/test/flow.measure.py
"""
import json
from pathlib import Path
import subprocess
import sys
import threading
import time

IMAGE = "cortex-workspace-rendered:ubuntu26.04"
WINDOW = (1880, 1100)
PARK = (1915, 1195)  # Outside the window, so nothing is hovered.


def outside():
    root = Path(__file__).resolve().parents[3]
    site = root / ".workspace-build/flow-measure/site"
    site.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(["npx", "vite", "build", "--logLevel", "error", "--emptyOutDir", "--outDir", str(site)], cwd=root / "prototypes/cortex-workspace/desktop", check=True)
    command = ["docker", "run", "--rm", "--network", "none", "-e", "HOME=/tmp", "-e", "XDG_RUNTIME_DIR=/tmp", "-v", f"{site}:/site:ro", "-v", f"{Path(__file__).resolve()}:/measure.py:ro", IMAGE, "python3", "-B", "/measure.py", "--inside"]
    return subprocess.run(command, timeout=300).returncode


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
        time.sleep(.05)

    def load(path):
        on_main(lambda box, done: (view.load_uri(origin + path), done.set()))
        for _ in range(100):
            time.sleep(.1)
            try:
                if js("return document.readyState === 'complete' && !!document.querySelector('.titlebar') && document.querySelectorAll('.nav').length > 0"):
                    break
            except json.JSONDecodeError:
                pass
        else:
            raise TimeoutError("no workspace at " + path)
        time.sleep(.4)
        js("document.querySelector('.ws-demo-banner')?.remove(); window.__box = el => { const r = el.getBoundingClientRect(); return [r.x, r.y, r.width, r.height].map(n => Math.round(n * 100) / 100); }; return 1")

    def check(name, ok, **details):
        (passed if ok else failures).append(name if ok else {"check": name, **details})

    def centre(selector):
        return js(f"const el = document.querySelector({json.dumps(selector)}); if (!el) return null; el.scrollIntoView({{ block: 'nearest' }}); const r = el.getBoundingClientRect(); return [Math.round(r.x + r.width / 2), Math.round(r.y + r.height / 2)];")

    def board(pj):
        load(f"{pj}/board")
        seen = js("""const groups = [...document.querySelectorAll('.col-grp')].map(g => g.textContent), named = [...document.querySelectorAll('.card .ws-card-age .trunc')].map(s => s.textContent);
          const tint = c => getComputedStyle(c).backgroundColor, need = document.querySelector('.card.need-you'), plain = document.querySelector('.card:not(.need-you):not(.bad)');
          return { groups, named: named.length, need: need && tint(need), plain: plain && tint(plain), review: !![...document.querySelectorAll('.col[data-col="review"] .card')].length, reviewMarked: [...document.querySelectorAll('.col[data-col="review"] .card')].every(c => c.classList.contains('need-you')), stamps: /Confirmed|\\bold\\b/.test(document.querySelector('.main').textContent) };""")
        check("all work names its sprints", len(seen["groups"]) > 0 and seen["named"] > 0, seen=seen)
        check("a task waiting for the owner reads differently", seen["need"] is not None and seen["need"] != seen["plain"], seen=seen)
        check("every Review card is marked", not seen["review"] or seen["reviewMarked"], seen=seen)
        check("no stamp on a live board", not seen["stamps"], seen=seen)
        facts["sprint_heads"] = seen["groups"]

    def move(pj):
        load(f"{pj}/board")
        js("""window.__seen = { flew: 0, frames: [] }; const tick = () => { const g = document.querySelector('.ws-fly'); if (g) { __seen.flew++; const m = new DOMMatrix(getComputedStyle(g).transform); __seen.frames.push(Math.round(m.m41) + ',' + Math.round(m.m42)); } requestAnimationFrame(tick); }; requestAnimationFrame(tick); return 1""")
        at = js("""const b = [...document.querySelectorAll('.card-a button')].find(b => b.textContent === 'Start'); if (!b) return null; const card = b.closest('.card'); window.__id = card.dataset.task; window.__from = card.closest('.col').dataset.col; window.__start = __box(card); const r = b.getBoundingClientRect(); return [Math.round(r.x + r.width / 2), Math.round(r.y + r.height / 2)];""")
        if at is None:
            facts.setdefault("not_on_screen", []).append("a startable card")
            return
        x("mousemove", *at)
        x("click", 1)
        time.sleep(.7)
        x("mousemove", *PARK)
        after = js("""const card = document.querySelector(`.card[data-task="${__id}"]`); return { flew: __seen.flew, frames: [...new Set(__seen.frames)].length, from: __from, to: card?.closest('.col')?.dataset.col, left: document.querySelectorAll('.ws-fly').length, hidden: card && getComputedStyle(card).visibility, flying: document.querySelector('.board-wrap').hasAttribute('data-flying') };""")
        check("a card flies between columns in several frames", after["from"] != after["to"] and after["frames"] >= 3, after=after)
        check("the flight ends clean", after["left"] == 0 and after["hidden"] == "visible" and not after["flying"], after=after)
        facts["flight"] = after

    def plan(sprint):
        load(sprint)
        rows = js("""const rows = [...document.querySelectorAll('.pt-row:not(.th)')]; return rows.length ? { n: rows.length, tall: Math.max(...rows.map(r => r.getBoundingClientRect().height)), more: document.querySelectorAll('.ws-more-l').length, text: rows[0].textContent.length } : null;""")
        if rows is None:
            facts.setdefault("not_on_screen", []).append("a plan")
            return
        check("a plan row is one line, however long its task", rows["tall"] <= 40, rows=rows)
        check("every row offers Read more", rows["more"] == rows["n"], rows=rows)
        x("mousemove", *centre(".pt-row:not(.th)"))
        js("""window.__rows = [...document.querySelectorAll('.pt-row:not(.th)')]; window.__changes = 0; window.__watch = new MutationObserver(l => { __changes += l.filter(m => m.type === 'childList').length; }); __watch.observe(document.querySelector('.sp-plan'), { subtree: true, childList: true }); return 1""")
        time.sleep(4)
        same = js("""const now = [...document.querySelectorAll('.pt-row:not(.th)')]; return { changes: __changes, same: now.length === __rows.length && now.every((r, i) => r === __rows[i]), hover: now.some(r => r.matches(':hover')) };""")
        check("the plan does not redraw under a resting pointer", same["changes"] == 0 and same["same"], same=same)
        check("the pointer really rests on it", same["hover"], same=same)
        before = js("return { conv: __box(document.querySelector('.sp-conv')) };")
        x("mousemove", *centre(".ws-more-l"))
        x("click", 1)
        time.sleep(.3)
        read = js("""const p = document.querySelector('.ws-read'); if (!p) return null; const plan = document.querySelector('.sp-plan').getBoundingClientRect(), r = p.getBoundingClientRect(), b = document.querySelector('.ws-read-b'); return { right: r.left >= plan.right - 1, inside: r.right <= innerWidth, conv: __box(document.querySelector('.sp-conv')), title: document.querySelector('.ws-read-t').textContent.length > 0, scrolls: getComputedStyle(b).overflowY };""")
        check("Read more opens a right sidebar", read is not None and read["right"] and read["inside"] and read["title"], read=read)
        check("the conversation stays put", read is not None and read["conv"] == before["conv"], read=read, before=before)
        check("the full text scrolls on its own", read is not None and read["scrolls"] in ("auto", "scroll"), read=read)
        x("key", "Escape")
        time.sleep(.2)
        check("Escape closes it", js("return !document.querySelector('.ws-read')"))
        x("mousemove", *PARK)

    def css():
        """The pieces no demo state reaches, drawn with the app's own stylesheet."""
        tabs = js("""const host = document.createElement('div'); host.style.width = '380px'; host.innerHTML = `<div class="dc"><div class="dc-tabs"><button class="dc-tab" id="a"><span>1</span><i class="i s12 i-check ws-off"></i></button><button class="dc-tab" id="b"><span>2</span><i class="i s12 i-check"></i></button></div><div class="dc-panels"><div class="dc-panel" id="p1"><div class="dc-q">A short one</div></div><div class="dc-panel" id="p2" aria-hidden="true"><div class="dc-q">${'A much longer question that wraps over several lines. '.repeat(4)}</div></div></div></div>`; document.querySelector('.main').append(host);
          const panels = host.querySelector('.dc-panels'), one = panels.getBoundingClientRect().height, p1 = host.querySelector('#p1'), p2 = host.querySelector('#p2'); p1.setAttribute('aria-hidden', 'true'); p2.removeAttribute('aria-hidden'); const two = panels.getBoundingClientRect().height;
          const a = host.querySelector('#a').getBoundingClientRect(), b = host.querySelector('#b').getBoundingClientRect(), hidden = getComputedStyle(p1).visibility; host.remove(); return { one, two, a: a.width, b: b.width, hidden };""")
        check("switching a question tab moves nothing", tabs["one"] == tabs["two"] and tabs["one"] > 60, tabs=tabs)
        check("a tab keeps its width with or without a check", tabs["a"] == tabs["b"], tabs=tabs)
        check("a hidden question is not shown", tabs["hidden"] == "hidden", tabs=tabs)
        qa = js("""const host = document.createElement('div'); host.style.width = '380px'; host.innerHTML = `<div class="qa"><i class="i s14 i-question"></i><div class="qa-b"><div class="qa-q">Какой визуальный стиль ближе?</div><div class="qa-a">Тёплый и домашний: кофейные оттенки, уют, и ещё немного длинного текста, чтобы он занял две строки</div></div><span class="qa-t">23:22</span></div>`; document.querySelector('.main').append(host);
          const b = host.querySelector('.qa-b').getBoundingClientRect(), t = host.querySelector('.qa-t').getBoundingClientRect(), q = host.querySelector('.qa-q').getBoundingClientRect(), a = host.querySelector('.qa-a').getBoundingClientRect(); host.remove(); return { stacked: a.top >= q.bottom, beside: t.left >= b.right, width: b.width };""")
        check("a settled question stacks its answer under it, the time beside both", qa["stacked"] and qa["beside"] and qa["width"] > 250, qa=qa)
        busy = js("""const b = [...document.querySelectorAll('.titlebar .btn-primary, .main .btn')].find(x => x.getClientRects().length); if (!b) return null; const before = __box(b); b.setAttribute('data-busy', ''); b.setAttribute('aria-busy', 'true'); window.__b = b; window.__was = before; return { before, now: __box(b), label: b.textContent };""")
        if busy is None:
            facts.setdefault("not_on_screen", []).append("a button")
        else:
            time.sleep(.3)
            late = js("""const s = getComputedStyle(__b), a = getComputedStyle(__b, '::after'); const out = { box: __box(__b), color: s.color, after: a.position, opacity: a.opacity, width: a.width, spin: a.animationName }; __b.removeAttribute('data-busy'); __b.removeAttribute('aria-busy'); return out;""")
            check("a loader never changes its button's box", busy["before"] == busy["now"] == late["box"], busy=busy, late=late)
            check("a loader shows once it is worth showing", late["after"] == "absolute" and late["opacity"] == "1" and "ws-busy-run" in late["spin"], late=late)
            check("a busy button keeps its label readable", late["color"] not in ("transparent", "rgba(0, 0, 0, 0)"), late=late)
            check("the label gives way to the loader", late["color"] == "rgba(0, 0, 0, 0)", late=late)

    def inbox(at):
        load(f"{at}/inbox")
        rows = js("""const rows = [...document.querySelectorAll('.need')]; return { n: rows.length, ages: rows.map(r => { const a = r.querySelector('.age'), s = getComputedStyle(a); return { text: a.textContent, tall: a.getBoundingClientRect().height, wide: a.scrollWidth <= a.clientWidth + 1 }; }), row: rows.length ? Math.max(...rows.map(r => r.getBoundingClientRect().height)) : 0 };""")
        check("an Inbox row holds one line", rows["n"] > 0 and rows["row"] <= 44 and all(a["tall"] <= 20 and a["wide"] and "Confirmed" not in a["text"] for a in rows["ages"]), rows=rows)

    def boxes():
        drawn = js("""const host = document.createElement('div'); host.style.width = '300px'; host.innerHTML = `<input type="checkbox" class="chk" id="c1"><input type="checkbox" class="ws-ck" id="c2"><label class="ck"><input type="checkbox" id="c3"></label><div class="chk" id="row"><i class="i s14 i-help"></i><span class="trunc">Visual review</span><span class="src">Unverified</span></div>`; document.querySelector('.main').append(host);
          const size = id => { const r = host.querySelector('#' + id).getBoundingClientRect(); return [r.width, r.height]; }, row = host.querySelector('#row'), s = getComputedStyle(row), out = { boxes: ['c1', 'c2', 'c3'].map(size), row: size('row'), border: s.borderTopWidth, name: host.querySelector('.trunc').getBoundingClientRect().width }; host.remove(); return out;""")
        check("a checkbox is square", all(w == h for w, h in drawn["boxes"]), drawn=drawn)
        check("a check row is a row with its name showing", drawn["row"][0] == 300 and drawn["border"] == "0px" and drawn["name"] > 60, drawn=drawn)

    def run():
        load("/")
        machine = js("return location.pathname.split('/')[2]")
        project = js("return document.querySelector('.proj').getAttribute('href').split('/')[4]")
        at, pj = f"/m/{machine}", f"/m/{machine}/p/{project}"
        for theme in ("dark", "light"):
            load("/")
            js(f"localStorage.setItem('cw.theme', {json.dumps(theme)}); return 1")
            board(pj)
            load(at)
            sprint = js("const a = [...document.querySelectorAll('.task-row')].find(a => /\\/s\\//.test(a.getAttribute('href') ?? '')); return a ? a.getAttribute('href').replace(/\\/(board|list)$/, '/planning') : null")
            if sprint:
                plan(sprint)
            else:
                facts.setdefault("not_on_screen", []).append("a sprint")
            load(pj)
            css()
            inbox(at)
            boxes()
        move(pj)

    def main():
        code = 1
        try:
            time.sleep(1)
            run()
            code = 1 if failures else 0
        except Exception as error:  # A broken run is a failed run, never a pass.
            failures.append({"check": "harness", "error": repr(error)})
        print(json.dumps({"engine": f"WebKitGTK {WebKit2.get_major_version()}.{WebKit2.get_minor_version()}.{WebKit2.get_micro_version()}", "passed": len(passed), "failed": len(failures), "facts": facts, "failures": failures}, indent=1))
        sys.stdout.flush()
        import os
        os._exit(code)

    threading.Thread(target=main, daemon=True).start()
    Gtk.main()


def container():
    import os
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
