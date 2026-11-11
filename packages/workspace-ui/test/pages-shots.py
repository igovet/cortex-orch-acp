"""Captures of every page in WebKitGTK on a private virtual screen, with measurements. Run through pages-shots.mjs.

Optional arguments after the URL name the pages to capture; without them every page is captured. CW_SET=polish captures five
window sizes in both themes; CW_LABEL_FIT is label-fit.js, the rule for wrapped and off-centre labels.
"""
from collections import Counter
import json
import os
import re
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

M = "/settings/machine/<M>"
PAGES = (
    ("home", "/m/<M>"), ("inbox", "/m/<M>/inbox"), ("archive", "/m/<M>/archive"),
    ("new-task", "/m/<M>/new?project=p"), ("new-sprint", "/m/<M>/new?kind=sprint&project=p"),
    ("board", "/m/<M>/p/p/board"), ("list", "/m/<M>/p/p/list"),
    ("sprint-planning", "/m/<M>/p/p/s/sp/planning"), ("sprint-plan", "/m/<M>/p/p/s/sp/plan"), ("sprint-summary", "/m/<M>/p/p/s/sp/summary"),
    ("workers", "/m/<M>/t/c-work/workers"), ("worker", "/m/<M>/t/c-work/workers/w-be2"),
    ("changes", "/m/<M>/t/c-done/changes"), ("ready", "/m/<M>/t/c-accepted"),
    ("knowledge", "/m/<M>/p/p/knowledge"), ("project-settings", "/m/<M>/p/p/settings"),
    ("appearance", "/settings/app/appearance"), ("voice", "/settings/app/voice"), ("updates", "/settings/app/updates"),
    ("shortcuts", "/settings/app/shortcuts"), ("machines", "/settings/app/machines"),
    ("machine", M + "/overview"), ("connections", M + "/connections"), ("worktrees", M + "/worktrees"),
    ("storage", M + "/storage"), ("diagnostics", M + "/diagnostics"), ("remote", M + "/remote"),
    ("orch-defaults", M + "/orchestration/defaults"), ("orch-models", M + "/orchestration/models"),
    ("orch-routing", M + "/orchestration/routing"), ("orch-preview", M + "/orchestration/effective"),
    ("orch-limits", M + "/orchestration/policy"), ("orch-tools", M + "/orchestration/tools"),
    ("orch-profiles", M + "/orchestration/profiles"), ("orch-files", M + "/orchestration/files"),
    ("not-found", "/m/<M>/no-such-page"),
)
# CW_SET=review-fixes: the screens a review asked to see, some after a press: (name, route, more query, what to press).
# `sprint=running` is a sprint already at work with a task not yet started; `sprint=closed` is the same sprint after Close sprint.
CLICK = "const e = [...document.querySelectorAll(%s)].find(x => x.getClientRects().length || %s); e.click(); return !!e;"
REVIEW = (
    ("board", "/m/<M>/p/p/board", "", None), ("list", "/m/<M>/p/p/list", "", None),
    # Only where the menu button is shown (a narrow pane); a wide pane shows every area as a tab.
    ("areas-menu", "/m/<M>/p/p/board", "", "const e = [...document.querySelectorAll(\"[aria-label='More project areas']\")].find(x => x.getClientRects().length); if (e) e.click(); return true;"),
    ("knowledge", "/m/<M>/p/p/knowledge", "", None), ("project-settings", "/m/<M>/p/p/settings", "", None),
    ("code-areas-menu", "/m/<M>/p/p/code", "", "const e = [...document.querySelectorAll(\"[aria-label='More project areas']\")].find(x => x.getClientRects().length); if (e) e.click(); return true;"),
    ("sprint-running", "/m/<M>/p/p/s/sp/board", "&sprint=running", None),
    # A task not started yet, in a sprint already at work: Edit task… in its card's menu and on its own page.
    ("edit-task", "/m/<M>/p/p/s/sp/board", "&sprint=running", "document.querySelector(\"[aria-label='Actions for Add rate limits to the login endpoint']\").click(); const e = [...document.querySelectorAll('.mi')].find(x => x.textContent.startsWith('Edit task') && x.getClientRects().length); e.click(); return !!e;"),
    ("card-menu", "/m/<M>/p/p/s/sp/board", "&sprint=running", CLICK % (json.dumps("[aria-label='Actions for Add rate limits to the login endpoint']"), "false")),
    ("draft-task", "/m/<M>/t/c-draft", "&sprint=running", None),
    ("sprint-summary-running", "/m/<M>/p/p/s/sp/summary", "&sprint=running", None),
    ("sprint-menu", "/m/<M>/p/p/s/sp/summary", "&sprint=running", "document.querySelector(\"[aria-label='Sprint actions']\").click(); return true;"),
    # Close sprint… stands in the band when it has room, and in the sprint's menu when it does not.
    ("close-sprint", "/m/<M>/p/p/s/sp/summary", "&sprint=running", "const shown = () => [...document.querySelectorAll('button')].find(x => x.textContent.trim().startsWith('Close sprint') && x.getClientRects().length); if (!shown()) document.querySelector(\"[aria-label='Sprint actions']\").click(); const e = shown(); e.click(); return !!e;"),
    ("archive-closed-sprint", "/m/<M>/archive", "&sprint=closed", None),
    ("sprint-closed", "/m/<M>/p/p/s/sp/summary", "&sprint=closed", None),
)
REVIEWING = os.environ.get("CW_SET") == "review-fixes"
POLISH = os.environ.get("CW_SET") == "polish" or REVIEWING
# CW_SET=ui-fixes: the owner's three widths in both themes.
BOTH = POLISH or os.environ.get("CW_SET") == "ui-fixes"
SIZES = ((1920, 1080), (1280, 800), (960, 540), (820, 1180), (390, 844)) if POLISH else ((1280, 800), (960, 540), (390, 844))
THEMES = (("", ""), ("-dark", "&dark=1")) if BOTH else (("", ""),)
WANTED = set(sys.argv[2:])
LABEL_FIT = os.environ["CW_LABEL_FIT"]


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
    time.sleep(.3)
    def call(box, done):
        width, height = win.get_size()
        pixels = Gdk.pixbuf_get_from_window(win.get_window(), 0, 0, width, height)
        pixels.savev(str(evidence / (name + ".png")), "png", [], [])
        box["value"] = pixels
        done.set()
    return main(call)


# The letters as drawn: a label with capitals or ascenders and nothing below its baseline inks from its top to its baseline, so
# the middle of its inked rows is the middle the eye sees. It must lie within 1 px of the middle of its box.
NO_DESCENDER = re.compile(r"^[^gjpqyQ,;()\[\]{}_|@$дуфрщцДЦЩ]+$")
TALL = re.compile(r"[A-Z0-9bdfhkltА-ЯЁ]")


def ink_centres(pixels, entries):
    data, stride, channels = pixels.get_pixels(), pixels.get_rowstride(), pixels.get_n_channels()
    width, height = pixels.get_width(), pixels.get_height()
    found = []
    for e in entries:
        if len(found) >= 160 or len(e["text"]) < 2 or not NO_DESCENDER.match(e["text"]) or not TALL.search(e["text"]):
            continue
        x0, x1 = max(0, int(e["run"][0])), min(width, int(e["run"][2] + .999))
        # The run's own box holds all its letters; a box's border or fill edge just outside it is not ink.
        y0, y1 = max(0, int(e["box"][1]) + 1, int(e["run"][1]) - 1), min(height, int(e["box"][3]) - 1, int(e["run"][3] + .999) + 1)
        if x1 - x0 < 4 or y1 - y0 < 8:
            continue
        at = lambda x, y: data[y * stride + x * channels: y * stride + x * channels + 3]
        ground = Counter(at(x, y) for y in range(y0, y1) for x in range(x0, x1)).most_common(1)[0][0]
        rows = [y for y in range(y0, y1) if any(max(abs(a - b) for a, b in zip(at(x, y), ground)) > 64 for x in range(x0, x1))]
        if rows:
            found.append({"kind": e["kind"], "label": e["label"], "rows": [rows[0], rows[-1] + 1], "mid": round(e["mid"], 2), "off": round((rows[0] + rows[-1] + 1) / 2 - e["mid"], 2), "metric": e["ink"]})
    return found


# Controls that stand side by side in one container must share one height; the window must not scroll sideways; the
# product's name must not be shown in text, a label or a tooltip; no text or control overlaps another; labels hold label-fit.js.
MEASURE = r"""
const CTL = '.btn, .ib, .sel, .seg, .chip, .pick, select, input:not([type=checkbox]):not([type=radio]):not([type=range])';
const seen = e => e.getClientRects().length && getComputedStyle(e).visibility !== 'hidden' && !e.closest('[hidden], [inert]');
const mixed = [];
const parents = new Set([...document.querySelectorAll(CTL)].filter(seen).map(e => e.parentElement));
for (const p of parents) {
  const kids = [...p.children].filter(e => e.matches(CTL) && seen(e) && !e.closest('.seg > *'));
  if (kids.length < 2) continue;
  const rows = {};
  for (const k of kids) { const r = k.getBoundingClientRect(); const line = Math.round(r.top + r.height / 2); (rows[line] ??= new Set()).add(Math.round(r.height)); }
  for (const hs of Object.values(rows)) if (Math.max(...hs) - Math.min(...hs) > 1) mixed.push(p.tagName.toLowerCase() + '.' + String(p.className).split(' ').slice(0, 2).join('.') + ' ' + JSON.stringify([...hs]));
}
const named = [];
const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
for (let n = walker.nextNode(); n; n = walker.nextNode()) if (/cortex/i.test(n.textContent) && n.parentElement && seen(n.parentElement) && !n.parentElement.closest('pre, code, .cm-editor, textarea')) named.push(n.textContent.trim().slice(0, 80));
for (const e of document.querySelectorAll('[aria-label], [title], [placeholder], [data-why]')) for (const a of ['aria-label', 'title', 'placeholder', 'data-why']) { const v = e.getAttribute(a); if (v && /cortex/i.test(v)) named.push(a + ': ' + v.slice(0, 80)); }
const past = [...document.querySelectorAll('body *')].filter(e => seen(e) && e.getBoundingClientRect().right > innerWidth + 1 && !e.closest('.l-region, [data-scroll=sideways], pre, .cm-editor') && ![...e.children].some(c => c.getBoundingClientRect().right > innerWidth + 1)).slice(0, 6).map(e => e.tagName.toLowerCase() + '.' + String(e.className).split(' ').slice(0, 2).join('.') + ' ' + Math.round(e.getBoundingClientRect().right));
const fit = (LABEL_FIT)(document.querySelector('dialog[open]') || document.body);
// No text or control runs into another that is not its own ancestor or descendant, measured where each is drawn (inside every
// box that cuts it). The sprint graph's drawing is its own.
const clipOf = el => { let c = {l: 0, t: 0, r: innerWidth, b: innerHeight}; for (let a = el; a && a !== document.documentElement; a = a.parentElement) { const s = getComputedStyle(a); if (s.overflowX !== 'visible' || s.overflowY !== 'visible') { const q = a.getBoundingClientRect(); c = {l: Math.max(c.l, q.left), t: Math.max(c.t, q.top), r: Math.min(c.r, q.right), b: Math.min(c.b, q.bottom)}; } } return c; };
const cutTo = (r, c) => ({l: Math.max(r.left, c.l), t: Math.max(r.top, c.t), r: Math.min(r.right, c.r), b: Math.min(r.bottom, c.b)});
const runs = [], scopeEl = document.querySelector('dialog[open]') || document.body;
const tw = document.createTreeWalker(scopeEl, NodeFilter.SHOW_TEXT);
for (let n = tw.nextNode(); n; n = tw.nextNode()) { const p = n.parentElement; if (!n.nodeValue.trim() || !p || !seen(p) || p.closest('script, style, .gsv, .ws-live')) continue; const g = document.createRange(); g.selectNodeContents(n); const c = clipOf(p); for (const r of g.getClientRects()) { const x = cutTo(r, c); if (x.r - x.l > 1 && x.b - x.t > 1) runs.push({node: p, label: n.nodeValue.trim().slice(0, 32), ...x}); } }
for (const el of scopeEl.querySelectorAll('button, a, input, textarea')) { if (!seen(el) || el.closest('.gsv')) continue; const x = cutTo(el.getBoundingClientRect(), clipOf(el.parentElement)); if (x.r - x.l > 1 && x.b - x.t > 1) runs.push({node: el, label: (el.getAttribute('aria-label') || el.textContent || '').trim().slice(0, 32), ...x}); }
// A bar pinned over a scroller (a sticky footer with its own fill) hides what scrolls under it; that is not a collision.
const pinnedOf = el => { for (let a = el; a && a !== document.documentElement; a = a.parentElement) { const s = getComputedStyle(a); if ((s.position === 'sticky' || s.position === 'fixed') && s.backgroundColor !== 'rgba(0, 0, 0, 0)' && s.backgroundColor !== 'transparent') return a; } return null; };
const overlaps = [];
for (let i = 0; i < runs.length && overlaps.length < 12; i++) for (let j = i + 1; j < runs.length && overlaps.length < 12; j++) { const a = runs[i], b = runs[j]; if (a.node === b.node || a.node.contains(b.node) || b.node.contains(a.node)) continue; if (pinnedOf(a.node) !== pinnedOf(b.node)) continue; const w = Math.min(a.r, b.r) - Math.max(a.l, b.l), h = Math.min(a.b, b.b) - Math.max(a.t, b.t); if (w > 1 && h > 1) overlaps.push(`overlap "${a.label}" with "${b.label}" (${Math.round(w)}x${Math.round(h)})`); }
return {viewport: [innerWidth, innerHeight], sideways: document.documentElement.scrollWidth > innerWidth + 1, mixed: [...new Set(mixed)].slice(0, 12), named: [...new Set(named)].slice(0, 12), past, labels: [...fit.problems.slice(0, 30), ...overlaps], labelCount: fit.problems.length, worst: fit.labels, entries: fit.entries, covers: [...document.querySelectorAll(':popover-open')].map(p => { const r = p.getBoundingClientRect(); return [r.left, r.top, r.right, r.bottom]; })};
""".replace("LABEL_FIT", LABEL_FIT)


def run():
    global loads
    made, problems = [], 0
    for page, route, query, press in (REVIEW if REVIEWING else ((p, r, "", None) for p, r in PAGES)):
        if WANTED and page not in WANTED:
            continue
        for (width, height), (theme, more) in ((s, t) for s in SIZES for t in THEMES):
            loads += 1
            main(lambda b, d: (win.resize(width, height), d.set()))
            main(lambda b, d: (view.load_uri(sys.argv[1] + "?world=pages&route=" + quote(route, safe="") + query + more + f"&load={loads}"), d.set()))
            wait(f"window.fixture === '{loads}'")
            wait("document.querySelector('.ws-main, main, .l-page') && !document.querySelector('.l-startup')")
            time.sleep(.8)
            if press:
                assert js(press), f"{page}: nothing to press"
                time.sleep(.6)
            facts = js(MEASURE)
            name = f"{page}-{width}x{height}{theme}"
            facts["file"] = name + ".png"
            pixels = capture(name)
            facts["window"] = [pixels.get_width(), pixels.get_height()]
            # An open menu is painted over the page: a label it partly covers has the menu's pixels in its box, not its own ink.
            covers = facts.pop("covers")
            under = lambda b, c: b[0] < c[2] and b[2] > c[0] and b[1] < c[3] and b[3] > c[1] and not (b[0] >= c[0] and b[2] <= c[2] and b[1] >= c[1] and b[3] <= c[3])
            facts["drawn"] = ink_centres(pixels, [e for e in facts.pop("entries") if not any(under(e["box"], c) for c in covers)])
            facts["labels"] += [f"letters drawn off centre: {d['kind']} \"{d['label']}\" {'low' if d['off'] > 0 else 'high'} by {abs(d['off'])}px" for d in facts["drawn"] if abs(d["off"]) > 1]
            problems += bool(facts["sideways"] or facts["mixed"] or facts["named"] or facts["labels"])
            made.append(facts)
            if facts["sideways"] or facts["mixed"] or facts["named"] or facts["labels"]:
                print(json.dumps({k: facts[k] for k in ("file", "sideways", "mixed", "named", "labels")}, ensure_ascii=False), flush=True)
    (evidence / "measurements.json").write_text(json.dumps(made, indent=1))
    print(f"pages {len(made)} problems {problems}", flush=True)
    return 1 if problems else 0


def worker():
    code = 0
    try:
        code = run()
    except Exception as error:
        print(repr(error), file=sys.stderr)
        code = 2
    finally:
        print(f"capture exit {code}", file=sys.stderr)
        sys.stdout.flush()
        sys.stderr.flush()
        xvfb.terminate()
    os._exit(code)


threading.Thread(target=worker, daemon=True).start()
Gtk.main()
