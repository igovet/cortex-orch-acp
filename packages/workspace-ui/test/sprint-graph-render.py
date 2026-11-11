"""The sprint plan's Graph view in WebKitGTK, on the layout gate's fake data and private virtual screen.

It captures the screens the owner reviews (Graph, Full view, a search match, a selection, a collapsed layer and the light
theme at four window sizes), checks that no two regions of the screen overlap, and times pan, zoom, selection and filter on
a 200-task plan. Captures and numbers go to the evidence folder; a region overlap or a missing screen fails the run. The
frame times are measured, not judged: they are for this software-rendered virtual screen, not for the owner's desktop.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import time
from urllib.parse import quote

xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "1920x1200x24", "-nolisten", "tcp"], stdout=subprocess.PIPE, text=True)
# `bench` alone times the 200-task plan without the captures; `composited` keeps WebKit's accelerated compositing on (software
# GL here), as the desktop window has it, where the layout gate turns it off for stable pixels.
modes = sys.argv[2:]
os.environ.update(DISPLAY=":" + xvfb.stdout.readline().strip(), GDK_BACKEND="x11", NO_AT_BRIDGE="1", WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS="1")
if "composited" not in modes:
    os.environ["WEBKIT_DISABLE_COMPOSITING_MODE"] = "1"
import gi
gi.require_version("Gtk", "3.0")
gi.require_version("WebKit2", "4.1")
from gi.repository import Gtk, GLib, WebKit2, Gdk

evidence = Path("/evidence")
win = Gtk.Window()
win.set_default_size(1440, 900)
view = WebKit2.WebView(web_context=WebKit2.WebContext.new_ephemeral())
win.add(view)
win.show_all()
loads = 0


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
    assert done.wait(30), "WebKit callback timeout"
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


def wait(code, tries=150):
    for _ in range(tries):
        if js("return !!(" + code + ")"):
            return
        time.sleep(.1)
    raise TimeoutError(code)


def open_page(query, width, height, ready):
    global loads
    loads += 1
    main(lambda b, d: (win.resize(width, height), d.set()))
    main(lambda b, d: (view.load_uri(sys.argv[1] + f"?{query}&load={loads}"), d.set()))
    wait(f"window.fixture === '{loads}'")
    wait(ready)
    time.sleep(.6)
    actual = js("return [innerWidth, innerHeight]")
    assert actual == [width, height], (actual, width, height)


def capture(name):
    time.sleep(.4)
    def call(box, done):
        width, height = win.get_size()
        pixels = Gdk.pixbuf_get_from_window(win.get_window(), 0, 0, width, height)
        pixels.savev(str(evidence / (name + ".png")), "png", [], [])
        box["value"] = [width, height]
        done.set()
    return main(call)


route = lambda sprint: "route=" + quote(f"/m/<M>/p/p/s/{sprint}/plan", safe="")
READY = "document.querySelector('.gsv .n') && document.querySelector('.gv-band') && !document.querySelector('.l-startup')"
PRESS = "const n=document.querySelector(%s);for(const t of ['pointerdown','pointerup'])n.dispatchEvent(new PointerEvent(t,{bubbles:true,button:0,clientX:0,clientY:0}));return 1"
CLICK = "[...document.querySelectorAll(%s)].find(e=>e.textContent.trim()===%s).click();return 1"
SEARCH = "const f=document.querySelector('[aria-label=\"Search tasks in the graph\"]');f.value=%s;f.dispatchEvent(new Event('input',{bubbles:true}));return 1"
# The regions of the screen, which must sit side by side or stacked and never on top of one another. The minimap is the one
# thing drawn on the canvas, by design, so it is not a region.
REGIONS = r"""
const pick = {sidebar: '.sidebar', conversation: '.sp-conv', plan: '.sp-plan', details: '.ws-read', toolbar: '.gv-bar', canvas: '.gv-canvas', band: '.gv-band', head: '.sp-plan-h', tabs: '.tabs', strip: '.l-lane'};
const out = {}, boxes = [];
for (const [k, s] of Object.entries(pick)) { const el = document.querySelector(s); if (!el || !el.getClientRects().length) continue; const r = el.getBoundingClientRect(); if (r.width < 1 || r.height < 1) continue; out[k] = [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)]; boxes.push([k, el, r]); }
const overlaps = [];
for (let i = 0; i < boxes.length; i++) for (let j = i + 1; j < boxes.length; j++) { const [a, ea, ra] = boxes[i], [b, eb, rb] = boxes[j]; if (ea.contains(eb) || eb.contains(ea)) continue;
  const w = Math.min(ra.right, rb.right) - Math.max(ra.left, rb.left), h = Math.min(ra.bottom, rb.bottom) - Math.max(ra.top, rb.top); if (w > 1 && h > 1) overlaps.push(`${a} over ${b} (${Math.round(w)}x${Math.round(h)})`); }
const z = document.querySelector('.gv-zoom')?.textContent, lod = document.querySelector('.gsv')?.dataset.lod;
const title = document.querySelector('.gsv .n .l1, .gsv .n .ts'); const fs = title && getComputedStyle(title).display !== 'none' ? title.getBoundingClientRect().height : null;
return {regions: out, overlaps, zoom: z, lod, sidebar: document.querySelector('.ws-app').dataset.sidebar, full: !!document.querySelector('.sp-body[data-full]'), map: !document.querySelector('.gv-map')?.hidden};
"""
# Frame intervals over 120 frames while a synthetic wheel moves the view each frame, and one-off times for a selection and
# a filter until the next frame is painted. Run in the page; the result lands in window.bench.
BENCH = r"""
window.bench = null;
const canvas = document.querySelector('.gv-canvas'), r = canvas.getBoundingClientRect();
const frames = (kind) => new Promise(resolve => { const out = []; let last = 0, n = 0;
  const tick = t => { if (last) out.push(t - last); last = t; if (++n > 125) return resolve(out.slice(5));
    const t0 = performance.now();
    canvas.dispatchEvent(kind === 'zoom' ? new WheelEvent('wheel', {deltaY: n % 40 < 20 ? -25 : 25, ctrlKey: true, clientX: r.left + r.width / 2, clientY: r.top + r.height / 2, bubbles: true, cancelable: true})
      : new WheelEvent('wheel', {deltaY: n % 40 < 20 ? 18 : -18, deltaX: n % 60 < 30 ? 6 : -6, bubbles: true, cancelable: true}));
    handler.push(performance.now() - t0);
    requestAnimationFrame(tick); };
  requestAnimationFrame(tick); });
const painted = run => new Promise(resolve => { const t0 = performance.now(); run(); const sync = performance.now() - t0; requestAnimationFrame(() => requestAnimationFrame(() => resolve({sync, frame: performance.now() - t0}))); });
const stats = xs => { const s = [...xs].sort((a, b) => a - b), q = p => s[Math.min(s.length - 1, Math.floor(p * s.length))]; return {n: s.length, median: +q(.5).toFixed(2), p95: +q(.95).toFixed(2), max: +s[s.length - 1].toFixed(2)}; };
let handler = [];
(async () => {
  const settle = () => new Promise(res => setTimeout(res, 300));
  const pan = await frames('pan'); const panHandler = stats(handler); handler = []; await settle();
  const zoom = await frames('zoom'); const zoomHandler = stats(handler); handler = []; await settle();
  const press = el => { for (const t of ['pointerdown', 'pointerup']) el.dispatchEvent(new PointerEvent(t, {bubbles: true, button: 0})); };
  const picks = []; for (const k of [5, 40, 77, 120, 160, 199]) { picks.push(await painted(() => press(document.querySelector(`.gsv .n[data-id="t:huge-${k}"]`)))); await new Promise(r => setTimeout(r, 400)); }
  press(canvas); await settle();
  const boxes = [...document.querySelectorAll('[aria-label="Show tasks by status"] input[type=checkbox]')];
  const filters = []; for (const b of boxes.slice(0, 3)) for (const on of [true, false]) { filters.push(await painted(() => { b.checked = on; b.dispatchEvent(new Event('change', {bubbles: true})); })); await settle(); }
  const keyZoom = []; for (const k of ['0', '-', '-', '+', 'f']) { const n = document.querySelector('.gsv .n[tabindex="0"]'); n.focus(); keyZoom.push(await painted(() => n.dispatchEvent(new KeyboardEvent('keydown', {key: k, bubbles: true, cancelable: true})))); await settle(); }
  window.bench = {nodes: document.querySelectorAll('.gsv .n').length, edges: document.querySelectorAll('.gsv path.e').length, canvas: [Math.round(r.width), Math.round(r.height)],
    pan: stats(pan), panHandler, zoom: stats(zoom), zoomHandler, selection: {sync: stats(picks.map(p => p.sync)), frame: stats(picks.map(p => p.frame))},
    filter: {sync: stats(filters.map(p => p.sync)), frame: stats(filters.map(p => p.frame))}, settleZoom: {sync: stats(keyZoom.map(p => p.sync)), frame: stats(keyZoom.map(p => p.frame))}};
})();
return 1;
"""


def run():
    results, problems = {"screens": {}}, []
    sizes = () if "bench" in modes else ((1920, 1080), (1440, 900), (1280, 720), (1024, 640))
    for width, height in sizes:
        tag = f"{width}x{height}"
        dark = "graph=1&dark=1&" + route("big")
        open_page(dark, width, height, READY)
        def shot(name):
            found = js(REGIONS)
            results["screens"][f"{name}-{tag}"] = found
            problems.extend(f"{name}-{tag}: {o}" for o in found["overlaps"])
            capture(f"{name}-{tag}")
        shot("graph")
        js(CLICK % (json.dumps(".gv-bar button"), json.dumps("Full view")))
        wait("document.querySelector('.sp-body[data-full]')")
        time.sleep(.4)
        shot("full-view")
        js(CLICK % (json.dumps(".gv-bar button"), json.dumps("Exit full view")))
        wait("!document.querySelector('.sp-body[data-full]')")
        js(SEARCH % json.dumps("export"))
        wait("document.querySelector('.gsv .n.cur')")
        time.sleep(.4)
        shot("search-match")
        js(SEARCH % json.dumps(""))
        wait("!document.querySelector('.gsv .n.dim')")
        js(PRESS % json.dumps('.gsv .n[data-id="t:big-12"]'))
        wait("document.querySelector('.gsv .n.picked')")
        shot("selection")
        js(PRESS % json.dumps(".gv-canvas"))
        wait("!document.querySelector('.gsv .n.picked')")
        js(PRESS % json.dumps('.gsv .lh[data-l="2"] rect'))
        wait("document.querySelector('.gsv .band')")
        shot("collapsed-layer")
        open_page("graph=1&" + route("big"), width, height, READY)
        shot("light-theme")
    # Frame times on 200 tasks, in the pane at 1440x900 with the planning conversation beside it.
    open_page("graph=200&dark=1&" + route("huge"), 1440, 900, READY)
    js("const t0=performance.now();document.querySelector('.sp-plan .seg button').click();requestAnimationFrame(()=>requestAnimationFrame(()=>{window.listAt=performance.now()-t0}));return 1")
    wait("window.listAt !== undefined")
    js("window.graphAt=undefined;const t0=performance.now();[...document.querySelectorAll('.sp-plan .seg button')].find(b=>b.textContent==='Graph').click();requestAnimationFrame(()=>requestAnimationFrame(()=>{window.graphAt=performance.now()-t0}));return 1")
    wait("window.graphAt !== undefined && document.querySelector('.gsv .n')")
    results["open_graph_200_ms"] = js("return +window.graphAt.toFixed(1)")
    time.sleep(.5)
    js(BENCH)
    wait("window.bench", tries=600)
    results["bench_200"] = js("return window.bench")
    capture("bench-200-1440x900" + "".join("-" + m for m in modes))
    results["problems"] = problems
    (evidence / ("render-metrics" + "".join("-" + m for m in modes) + ".json")).write_text(json.dumps(results, indent=2) + "\n")
    print(json.dumps({"bench_200": results["bench_200"], "open_graph_200_ms": results["open_graph_200_ms"], "screens": {k: {"zoom": v["zoom"], "lod": v["lod"], "sidebar": v["sidebar"], "full": v["full"], "map": v["map"], "canvas": v["regions"].get("canvas")} for k, v in results["screens"].items()}, "problems": problems}, indent=1), flush=True)
    return 1 if problems else 0


exit_code = 1


def worker():
    global exit_code
    try:
        exit_code = run()
    except Exception as error:
        print(type(error).__name__ + ": " + str(error), file=sys.stderr, flush=True)
    finally:
        GLib.idle_add(Gtk.main_quit)


threading.Thread(target=worker, daemon=True).start()
try:
    Gtk.main()
finally:
    win.destroy()
    xvfb.terminate()
    try:
        xvfb.wait(timeout=5)
    except subprocess.TimeoutExpired:
        xvfb.kill()
sys.exit(exit_code)
