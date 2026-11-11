"""The scroll gate in WebKitGTK, on the layout gate's fake data made long (`scroll` in layout-fit-fixture.tsx).

Each surface below is opened at 1440x900 with WebKit's compositing on, as the desktop window runs it (software GL on a private
virtual screen here, not the owner's GPU). Every element that scrolls on it is found and scrolled twice:

- drag: the page moves the scroll position from one end to the other in 120 frames and back in 60, as a scrollbar drag does;
- wheel: xdotool turns the real mouse wheel 24 notches each way over it, through GTK and WebKit's own wheel handling.

A script installed before the page's own records, while it scrolls: the time between animation frames, how long style and
layout took when forced at the start of each frame, every scroll and wheel handler of the page and how long it ran, every
ResizeObserver callback, and every DOM change (a change while only scrolling is a render the scroll caused). WebKitGTK gives
page script no style-recalc or layout counters, so "layout" is the forced flush time per frame, counted where it took over 1 ms.

The `-live` surfaces scroll while the task streams an update every 400 ms, and also time each update the page takes in.

A breach of LIMITS on any surface fails the run; KNOWN lists, per surface, what does not meet them yet and how far it may
go, and each use is printed. Numbers go to <label>-metrics.json, captures to <label>-<surface>.png. Arguments after the
label: `flat` turns compositing off (as the layout gate runs), `idle` instead watches each picked page untouched for 70 s
(past the 30 s facts poll) for long frames and DOM changes, and any other word picks surfaces by name.
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
label, args = sys.argv[2], sys.argv[3:]
os.environ.update(DISPLAY=":" + xvfb.stdout.readline().strip(), GDK_BACKEND="x11", NO_AT_BRIDGE="1", WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS="1")
if "flat" in args:
    os.environ["WEBKIT_DISABLE_COMPOSITING_MODE"] = "1"
picked = [a for a in args if a not in ("flat", "idle")]
import gi
gi.require_version("Gtk", "3.0")
gi.require_version("WebKit2", "4.1")
from gi.repository import Gtk, GLib, WebKit2, Gdk

evidence = Path("/evidence")
WIDTH, HEIGHT = 1440, 900

# Installed before any of the page's own script, so every listener and observer it creates is measured.
INSTRUMENT = r"""
(() => {
  const P = window.__perf = {on: false};
  let frames, last, flush, mutations, handlers, handlerMs, handlerMax, ro, roMs, roMax, scrolls, wheels, lat, wheelAt, slow, run = 0;
  const reset = () => { frames = []; last = 0; flush = []; mutations = 0; handlers = 0; handlerMs = 0; handlerMax = 0; ro = 0; roMs = 0; roMax = 0; scrolls = 0; wheels = 0; lat = []; wheelAt = null; slow = []; };
  reset();
  const add = EventTarget.prototype.addEventListener, remove = EventTarget.prototype.removeEventListener, wraps = new WeakMap();
  const timed = fn => { let w = wraps.get(fn); if (!w) { w = function (e) { if (!P.on) return fn.call(this, e); const t = performance.now(); try { return fn.call(this, e); } finally { const d = performance.now() - t; handlers++; handlerMs += d; if (d > handlerMax) handlerMax = d; } }; wraps.set(fn, w); } return w; };
  EventTarget.prototype.addEventListener = function (type, fn, o) { return add.call(this, type, (type === 'scroll' || type === 'wheel') && typeof fn === 'function' ? timed(fn) : fn, o); };
  EventTarget.prototype.removeEventListener = function (type, fn, o) { return remove.call(this, type, (typeof fn === 'function' && wraps.get(fn)) || fn, o); };
  const RO = window.ResizeObserver;
  window.ResizeObserver = class extends RO { constructor(cb) { super(function (entries, o) { if (!P.on) return cb.call(this, entries, o); const t = performance.now(); try { return cb.call(this, entries, o); } finally { const d = performance.now() - t; ro++; roMs += d; if (d > roMax) roMax = d; } }); } };
  new MutationObserver(list => { if (P.on) mutations += list.length; }).observe(document.documentElement, {subtree: true, childList: true, attributes: true, characterData: true});
  // A notch that moved nothing (the end was reached) is not waiting for a scroll any more once the next one comes.
  add.call(window, 'wheel', () => { if (!P.on) return; wheels++; const now = performance.now(); if (wheelAt === null || now - wheelAt > 100) wheelAt = now; }, {capture: true, passive: true});
  add.call(window, 'scroll', () => { if (!P.on) return; scrolls++; if (wheelAt !== null) { lat.push(performance.now() - wheelAt); wheelAt = null; } }, {capture: true, passive: true});
  const tick = (t, mine) => { if (!P.on || mine !== run) return; if (last) { frames.push(t - last); if (t - last > 25) slow.push(Math.round(t - last)); } last = t;
    const s = performance.now(); void document.documentElement.offsetHeight; flush.push(performance.now() - s); requestAnimationFrame(t => tick(t, mine)); };
  const q = (xs, p) => { const s = [...xs].sort((a, b) => a - b); return s.length ? +s[Math.min(s.length - 1, Math.floor(p * s.length))].toFixed(1) : 0; };
  const r1 = n => +n.toFixed(1);
  P.start = () => { reset(); P.on = true; const mine = ++run; requestAnimationFrame(t => tick(t, mine)); };
  P.stop = () => { P.on = false; return {frames: frames.length, p50: q(frames, .5), p95: q(frames, .95), max: r1(Math.max(0, ...frames)),
    dropped: frames.filter(f => f > 25).length, long: frames.filter(f => f > 50).length, slow: slow.slice(0, 12),
    layoutMs: r1(flush.reduce((a, b) => a + b, 0)), layoutMax: r1(Math.max(0, ...flush)), layoutFrames: flush.filter(f => f > 1).length,
    mutations, handlers, handlerMs: r1(handlerMs), handlerMax: r1(handlerMax), resizeCallbacks: ro, resizeMs: r1(roMs), resizeMax: r1(roMax),
    scrolls, wheels, latencyP95: q(lat, .95), latencyMax: r1(Math.max(0, ...lat))}; };
})();
"""

# What is on screen, and every element on it that scrolls far enough to measure.
FIND = r"""
const out = [];
for (const el of document.querySelectorAll('body *')) {
  if (el.closest('svg')) continue;
  const s = getComputedStyle(el), r = el.getBoundingClientRect();
  if (r.width < 120 || r.height < 120 || !el.getClientRects().length) continue;
  const y = (s.overflowY === 'auto' || s.overflowY === 'scroll') && el.scrollHeight > el.clientHeight + 300;
  const x = !y && (s.overflowX === 'auto' || s.overflowX === 'scroll') && el.scrollWidth > el.clientWidth + 300;
  if (!x && !y) continue;
  el.dataset.perf = String(out.length);
  const name = el.tagName.toLowerCase() + (typeof el.className === 'string' && el.className.trim() ? '.' + el.className.trim().split(/\s+/).slice(0, 2).join('.') : '');
  out.push({id: out.length, name, axis: y ? 'y' : 'x', range: y ? el.scrollHeight - el.clientHeight : el.scrollWidth - el.clientWidth,
    reverse: s.display.includes('flex') && s.flexDirection === 'column-reverse', box: [r.left, r.top, r.width, r.height].map(Math.round)});
}
return {scrollers: out, nodes: document.querySelectorAll('*').length, idleCallback: typeof requestIdleCallback, contentVisibility: CSS.supports('content-visibility', 'auto')};
"""

# The scrollbar-drag pass, in the page: one end to the other in 120 frames, back in 60.
DRAG = r"""
window.__bench = null;
const el = document.querySelector('[data-perf="ID"]'), axis = 'AXIS', range = RANGE, sign = REVERSE ? -1 : 1;
__perf.start();
let n = 0;
const seen = [];
const step = () => { const f = n < 120 ? n / 119 : 1 - (n - 120) / 59, v = Math.round(f * range);
  if (axis === 'y') el.scrollTop = sign * v; else el.scrollLeft = v;
  if (n % 30 === 0) seen.push(Math.round(Math.abs(axis === 'y' ? el.scrollTop : el.scrollLeft) - v));
  if (++n < 180) requestAnimationFrame(step); else requestAnimationFrame(() => { window.__bench = {...__perf.stop(), seen}; }); };
requestAnimationFrame(step);
return 1;
"""
POSITION = "const el=document.querySelector('[data-perf=\"ID\"]');return AXIS==='y'?el.scrollTop:el.scrollLeft"

at = lambda route: "route=" + quote(route, safe="")
FRAME = "document.querySelector('.titlebar') && !document.querySelector('.l-startup')"
on = lambda selector: FRAME + " && document.querySelector(" + json.dumps(selector) + ")"
# Surface name, query, ready expression, and for the sprint Graph the canvas the wheel pans (it scrolls nothing).
SURFACES = (
    ("conversation", "world=conversation&scroll=1&" + at("/m/<M>/t/c-work"), on(".timeline .ai"), None),
    ("worker-feed", "world=conversation&scroll=1&" + at("/m/<M>/t/c-work/workers/w-be2"), on(".stream .stream-in > *"), None),
    # The same while the task streams: an update every 400 ms, as a working task sends them (LIVE).
    ("conversation-live", "world=conversation&scroll=1&" + at("/m/<M>/t/c-work"), on(".timeline .ai"), None),
    ("worker-feed-live", "world=conversation&scroll=1&" + at("/m/<M>/t/c-work/workers/w-be2"), on(".stream .stream-in > *"), None),
    ("sprint-plan-list", "scroll=1&" + at("/m/<M>/p/p/s/big/plan"), on(".sp-plan:not([hidden]) .pt-row"), None),
    ("sprint-graph", "scroll=1&graph=1&" + at("/m/<M>/p/p/s/big/plan"), on(".gsv .n") + " && document.querySelector('.gv-band')", ".gv-canvas"),
    ("sprint-board", "scroll=1&" + at("/m/<M>/p/p/s/big/board"), on(".board-wrap .card"), None),
    ("project-board", "scroll=1&" + at("/m/<M>/p/p/board"), on(".board-wrap .card"), None),
    ("project-list", "scroll=1&" + at("/m/<M>/p/p/list"), on(".l-work .l-page-body > *"), None),
    ("git", "scroll=1&" + at("/m/<M>/p/p/git"), on(".git-sum") + " && document.querySelector('.hrow')", None),
    ("code", "scroll=1&" + at("/m/<M>/p/p/code?file=<FILE>"), on(".cm-content"), None),
    # The same 4,000 lines with no grammar: what is left of the editor's cost once syntax parsing is out of it.
    ("code-plain", "scroll=1&" + at("/m/<M>/p/p/code?file=docs%2Flong-notes.txt"), on(".cm-content"), None),
    # The 200-task sprint's board: rows rendered in flow must stay smooth at this size too.
    ("sprint-board-200", "scroll=1&graph=200&" + at("/m/<M>/p/p/s/huge/board"), on(".board-wrap .card"), None),
    # xterm scrolls its own buffer on the wheel; nothing in the page scrolls.
    ("terminal", "scroll=1&terminal=1&" + at("/m/<M>/p/p/git"), on(".xterm-rows > div"), ".terminal-screen"),
    ("review-changes", "world=review&" + at("/m/<M>/t/rg/changes"), on(".fhead"), None),
)
# Scrollers every page of the frame has, measured on the first page they are found on.
SHARED = ("div.sb-scroll",)
LIVE = ("conversation-live", "worker-feed-live")
# The gate. Frame intervals cannot fall below the 16.7 ms of a 60 Hz display (page timers here count whole milliseconds);
# a frame over 25 ms dropped at least one frame, one over 50 ms is a long task. p95 22 leaves room for the host's own load
# on this software-rendered screen, which is slower than the owner's GPU.
LIMITS = {"p95": 22, "max": 50, "long": 0, "dropped_share": .03, "handlerMax": 4, "layoutMax": 8, "resizeMax": 4, "updateMax": 8}
# What does not meet LIMITS yet, measured, with room for run-to-run spread. These are ratchets, not passes: each use is
# printed as KNOWN, and going past one fails the run. Key: (surface, scroller or "" for the surface, pass).
FIRST_JUMP = {"max": 170, "long": 1, "handlerMax": 10}  # one frame painting the far end of a long history no frame has painted yet
# 1,500 notes with an update every 400 ms, five times the service's pace; the scroll handler then finds its row after a change.
STREAM = {"p95": 34, "max": 60, "dropped_share": .10, "handlerMax": 8}
# CodeMirror renders and measures the lines it brings into view inside each scroll event; a drag moves 660 px a frame.
EDITOR = {"p95": 45, "max": 80, "long": 2, "handlerMax": 30, "dropped_share": .50}
# One frame painting part of a long card column no frame has shown yet; every other frame is on time.
COLUMN = {"max": 75, "long": 1}
KNOWN = {
    ("conversation", "div.timeline", "drag"): FIRST_JUMP, ("worker-feed", "div.stream", "drag"): FIRST_JUMP,
    ("conversation-live", "div.timeline", "drag"): {**FIRST_JUMP, "dropped_share": .06},
    ("conversation-live", "div.timeline", "dragAgain"): {"p95": 26, "dropped_share": .06}, ("conversation-live", "div.timeline", "wheel"): {"p95": 26, "dropped_share": .06},
    ("conversation-live", "", "update"): {"updateMax": 14},
    ("worker-feed-live", "div.stream", "drag"): {**STREAM, **FIRST_JUMP}, ("worker-feed-live", "div.stream", "dragAgain"): STREAM,
    ("worker-feed-live", "div.stream", "wheel"): STREAM, ("worker-feed-live", "", "update"): {"updateMax": 26},
    ("code", "div.cm-scroller", "drag"): EDITOR, ("code", "div.cm-scroller", "dragAgain"): EDITOR,
    ("code", "div.cm-scroller", "wheel"): {"p95": 24, "max": 45, "handlerMax": 16, "dropped_share": .06},
    ("code-plain", "div.cm-scroller", "drag"): EDITOR, ("code-plain", "div.cm-scroller", "dragAgain"): EDITOR,
    ("code-plain", "div.cm-scroller", "wheel"): {"p95": 22, "max": 45, "handlerMax": 16, "dropped_share": .03},
    ("sprint-board", "div.col-b", "wheel"): COLUMN, ("project-board", "div.col-b", "wheel"): COLUMN, ("sprint-board-200", "div.col-b", "wheel"): COLUMN,
}
win = Gtk.Window()
win.set_default_size(WIDTH, HEIGHT)
# Idle diagnosis only: keep the last two facts texts the port compares per project, to show what changed between polls.
FACTS_TEXTS = r"""
(() => { const s = JSON.stringify; window.__facts = {}; window.__factsCost = []; JSON.stringify = function (v, ...rest) { const t0 = performance.now(), out = s.call(this, v, ...rest);
  if (typeof out === 'string' && out.startsWith('{"projectId"') && v && v.repo !== undefined) { const k = v.projectId, l = (__facts[k] ??= []); l.push({at: Math.round(performance.now()), text: out}); if (l.length > 2) l.shift();
    __factsCost.push([Math.round(t0), Math.round(performance.now() - t0), out.length]); } return out; }; })();
"""
content = WebKit2.UserContentManager()
content.add_script(WebKit2.UserScript(INSTRUMENT + (FACTS_TEXTS if "idle" in args else ""), WebKit2.UserContentInjectedFrames.TOP_FRAME, WebKit2.UserScriptInjectionTime.START, None, None))
view = WebKit2.WebView(web_context=WebKit2.WebContext.new_ephemeral(), user_content_manager=content)
win.add(view)
win.show_all()
loads = 0
exit_code = 0


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


def wait(code, tries=200):
    for _ in range(tries):
        if js("return !!(" + code + ")"):
            return
        time.sleep(.1)
    raise TimeoutError(code)


def open_page(query, ready):
    global loads
    loads += 1
    main(lambda b, d: (view.load_uri(sys.argv[1] + f"?{query}&load={loads}"), d.set()))
    wait(f"window.fixture === '{loads}'")
    wait(ready)
    time.sleep(1.5)
    assert js("return [innerWidth, innerHeight]") == [WIDTH, HEIGHT]


def capture(name):
    def call(box, done):
        width, height = win.get_size()
        pixels = Gdk.pixbuf_get_from_window(win.get_window(), 0, 0, width, height)
        pixels.savev(str(evidence / f"{label}-{name}.png"), "png", [], [])
        done.set()
    main(call)


def origin():
    def call(box, done):
        o = win.get_window().get_origin()
        box["value"] = list(o[-2:])
        done.set()
    return main(call)


def settle(code, still=3):
    last, same = None, 0
    for _ in range(100):
        now = js(code)
        same = same + 1 if now == last else 0
        if same >= still:
            return now
        last = now
        time.sleep(.1)
    return last


def drag(s):
    code = DRAG.replace("ID", str(s["id"])).replace("AXIS", s["axis"]).replace("RANGE", str(s["range"])).replace("REVERSE", "true" if s["reverse"] else "false")
    js(code)
    wait("window.__bench", 300)
    return js("return window.__bench")


def wheel(box, axis, position):
    # The real wheel through X: 24 notches one way, a breath, 24 back. Buttons 5/4 scroll down/up, 7/6 right/left.
    ox, oy = origin()
    x, y = int(ox + box[0] + box[2] / 2), int(oy + box[1] + box[3] / 2)
    subprocess.run(["xdotool", "mousemove", "--sync", str(x), str(y)], check=True)
    time.sleep(.3)
    start = js(position) if position else 0
    js("__perf.start(); return 1")
    forward, back = ("5", "4") if axis == "y" else ("7", "6")
    subprocess.run(["xdotool", "click", "--repeat", "24", "--delay", "40", forward], check=True)
    middle = settle(position) if position else 0
    subprocess.run(["xdotool", "click", "--repeat", "24", "--delay", "40", back], check=True)
    if position:
        settle(position)
    else:
        time.sleep(1)
    result = js("return __perf.stop()")
    result["moved"] = abs(middle - start) if position else None
    return result


def brief(r):
    return f"p95 {r['p95']} max {r['max']} dropped {r['dropped']}/{r['frames']} long {r['long']} handlers {r['handlerMs']}ms(max {r['handlerMax']}) layout max {r['layoutMax']} resize {r['resizeMs']}ms dom {r['mutations']}"


known_used = []


def limits_for(key, where, values):
    allow = KNOWN.get(key, {})
    for k, v in allow.items():
        if values.get(k, 0) > LIMITS[k]:
            known_used.append(f"{where}: {k} {values[k]} (target {LIMITS[k]}, allowed {v})")
    return {**LIMITS, **allow}


def judge(where, r, problems, key=None):
    if "seen" in r and any(abs(d) > 2 for d in r["seen"][1:-1]):
        problems.append(f"{where}: the scroll position did not follow the drag: {r['seen']}")
    share = r["dropped"] / r["frames"] if r["frames"] else 0
    limits = limits_for(key, where, {**r, "dropped_share": round(share, 3)})
    for k in ("p95", "max", "handlerMax", "layoutMax", "resizeMax"):
        if r[k] > limits[k]:
            problems.append(f"{where}: {k} {r[k]} > {limits[k]}")
    if r["long"] > limits["long"]:
        problems.append(f"{where}: {r['long']} long frames (over 50 ms): {r['slow']}")
    if share > limits["dropped_share"]:
        problems.append(f"{where}: {r['dropped']} of {r['frames']} frames dropped one or more frames: {r['slow']}")


# 70 s with nothing touched, past the 30 s facts poll: when the page changed its DOM and how long each frame took then.
IDLE = r"""
window.__idle = {bursts: [], slow: []}; const t0 = performance.now(); let n = 0, last = 0;
const where = n => { const el = n.nodeType === 1 ? n : n.parentElement; const at = el?.closest('[aria-label], section, aside, nav, main, header'); return at ? at.tagName.toLowerCase() + (at.getAttribute('aria-label') ? '[' + at.getAttribute('aria-label').slice(0, 30) + ']' : '.' + String(at.className).split(' ')[0]) : '?'; };
new MutationObserver(list => { const t = Math.round((performance.now() - t0) / 100) / 10; let b = __idle.bursts.at(-1);
  if (!b || b.t !== t) __idle.bursts.push(b = {t, n: 0, added: 0, removed: 0, attributes: 0, text: 0, targets: {}});
  for (const r of list) { b.n++; if (r.type === 'childList') { b.added += r.addedNodes.length; b.removed += r.removedNodes.length; } else if (r.type === 'attributes') { b.attributes++; b.names = b.names ?? {}; b.names[r.attributeName] = (b.names[r.attributeName] ?? 0) + 1;
      const v = r.target.getAttribute(r.attributeName), k = r.target.tagName + ' ' + r.attributeName + ': ' + r.oldValue + ' -> ' + v; b.changes = b.changes ?? {}; if (Object.keys(b.changes).length < 12 || b.changes[k]) b.changes[k] = (b.changes[k] ?? 0) + 1; } else b.text++;
    const w = where(r.target); b.targets[w] = (b.targets[w] ?? 0) + 1; } }).observe(document.body, {subtree: true, childList: true, attributes: true, attributeOldValue: true, characterData: true});
const tick = t => { if (last && t - last > 25) __idle.slow.push([Math.round((t - t0) / 100) / 10, Math.round(t - last)]); last = t; if (++n < 4300) requestAnimationFrame(tick); else __idle.done = true; };
requestAnimationFrame(tick); return 1;
"""


def idle(results, problems):
    for name, query, ready, _ in SURFACES:
        if picked and name not in picked:
            continue
        open_page(query, ready)
        js(IDLE)
        wait("window.__idle.done", 900)
        found = js("return window.__idle")
        # The first difference between the last two texts of each project, if the poll sent the same project twice.
        found["factsChanged"] = js(r"""return Object.entries(window.__facts ?? {}).flatMap(([k, l]) => { if (l.length < 2 || l[0].text === l[1].text) return [];
          const a = l[0].text, b = l[1].text; let i = 0; while (i < a.length && a[i] === b[i]) i++; return [{project: k, at: [l[0].at, l[1].at], length: [a.length, b.length], before: a.slice(Math.max(0, i - 160), i + 120), after: b.slice(Math.max(0, i - 160), i + 120)}]; })""")
        results.setdefault("idle", {})[name] = found
        found["factsCost"] = js("const c = window.__factsCost ?? []; const by = {}; for (const [at, ms, n] of c) { const k = Math.round(at / 1000); (by[k] ??= [0, 0, 0]); by[k][0]++; by[k][1] += ms; by[k][2] += n; } return by")
        print(f"idle {name}: facts changed between polls: {found['factsChanged'][:2]}; facts compared per second (count, ms, chars): {found['factsCost']}", flush=True)
        for b in found["bursts"]:
            b["targets"] = dict(sorted(b["targets"].items(), key=lambda kv: -kv[1])[:8])
        print(f"idle {name}: slow frames {found['slow'][:20]} DOM bursts {[b for b in found['bursts'] if b['n'] > 20][:12]}", flush=True)
        for t, ms in found["slow"]:
            if ms > 50:
                problems.append(f"idle {name}: a {ms} ms frame at {t} s with nothing touched")


def run():
    global exit_code
    results = {"engine": "WebKitGTK", "version": [WebKit2.get_major_version(), WebKit2.get_minor_version(), WebKit2.get_micro_version()],
               "compositing": "flat" not in args, "window": [WIDTH, HEIGHT], "limits": LIMITS, "surfaces": {}}
    problems = []
    try:
        main(lambda b, d: (win.resize(WIDTH, HEIGHT), d.set()))
        if "idle" in args:
            idle(results, problems)
        shared = set()
        for name, query, ready, pan in () if "idle" in args else SURFACES:
            if picked and name not in picked:
                continue
            open_page(query, ready)
            capture(name)
            found = js(FIND)
            if name in LIVE:
                js("window.__updates=0;window.__live=setInterval(()=>{liveUpdate();__updates++},400);return 1")
            results.setdefault("engineFeatures", {"requestIdleCallback": found["idleCallback"], "contentVisibility": found["contentVisibility"]})
            surface = results["surfaces"][name] = {"nodes": found["nodes"], "scrollers": []}
            for s in found["scrollers"]:
                if s["name"] in SHARED:
                    if s["name"] in shared:
                        continue
                    shared.add(s["name"])
                where = f"{name} {s['name']}"
                position = POSITION.replace("ID", str(s["id"])).replace("AXIS", json.dumps(s["axis"]))
                # The first pass is the owner's first scroll through it; the second shows what is left once it has been seen.
                entry = {**s, "drag": drag(s)}
                time.sleep(.3)
                entry["dragAgain"] = drag(s)
                time.sleep(.3)
                entry["wheel"] = wheel(s["box"], s["axis"], position)
                surface["scrollers"].append(entry)
                for kind in ("drag", "dragAgain", "wheel"):
                    judge(f"{where} {kind}", entry[kind], problems, (name, s["name"], kind))
                if not entry["wheel"]["moved"]:
                    problems.append(f"{where}: the wheel did not move it")
                for kind in ("drag", "dragAgain", "wheel"):
                    print(f"{where} ({s['range']} px) {kind}: {brief(entry[kind])}", flush=True)
            if pan:
                box = js(f"const r=document.querySelector({json.dumps(pan)}).getBoundingClientRect();return [r.left,r.top,r.width,r.height].map(Math.round)")
                surface["pan"] = wheel(box, "y", None)
                judge(f"{name} {pan} wheel", surface["pan"], problems, (name, pan, "wheel"))
                print(f"{name} {pan} wheel: {brief(surface['pan'])}", flush=True)
            if not surface["scrollers"] and not pan:
                problems.append(f"{name}: nothing on it scrolls far enough to measure")
            if name in LIVE:
                surface["updates"] = js("clearInterval(window.__live);return window.__updates")
                surface["updateCost"] = js("const c=window.liveCost;return {updates:c.updates,dataMs:Math.round(c.data),appMs:Math.round(c.app),appMax:Math.round(c.appMax)}")
                print(f"{name}: {surface['updates']} updates arrived while scrolling; cost (fixture data, app) {surface['updateCost']}", flush=True)
                most = limits_for((name, "", "update"), name, {"updateMax": surface["updateCost"]["appMax"]})["updateMax"]
                if surface["updateCost"]["appMax"] > most:
                    problems.append(f"{name}: an update took {surface['updateCost']['appMax']} ms to take in > {most}")
                if surface["updates"] < 10:
                    problems.append(f"{name}: only {surface['updates']} updates arrived")
        results["problems"], results["known"] = problems, known_used
        (evidence / f"{label}-metrics.json").write_text(json.dumps(results, indent=2) + "\n")
        for k in known_used:
            print("KNOWN " + k, flush=True)
        for p in problems:
            print("PROBLEM " + p, flush=True)
        print(f"scroll gate: {len(problems)} problems, {len(known_used)} known shortfalls within their ratchets", flush=True)
        exit_code = 1 if problems else 0
    except Exception as error:
        exit_code = 1
        print(type(error).__name__ + ": " + str(error), file=sys.stderr, flush=True)
    finally:
        GLib.idle_add(Gtk.main_quit)


threading.Thread(target=run, daemon=True).start()
try:
    Gtk.main()
finally:
    win.destroy()
    xvfb.terminate()
    try:
        xvfb.wait(timeout=5)
    except subprocess.TimeoutExpired:
        xvfb.kill()
        xvfb.wait(timeout=5)
sys.exit(exit_code)
