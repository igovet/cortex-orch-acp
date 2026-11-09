"""Opt-in rendered check in WebKitGTK: long names never wrap, collide or leave their pane. Fake data, private virtual screen."""
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import time

xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "1920x1200x24", "-nolisten", "tcp"], stdout=subprocess.PIPE, text=True)
os.environ.update(DISPLAY=":" + xvfb.stdout.readline().strip(), GDK_BACKEND="x11", NO_AT_BRIDGE="1", WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS="1", WEBKIT_DISABLE_COMPOSITING_MODE="1")
import gi
gi.require_version("Gtk", "3.0")
gi.require_version("WebKit2", "4.1")
from gi.repository import Gtk, GLib, WebKit2, Gdk

label = sys.argv[2]
evidence = Path("/evidence")
win = Gtk.Window()
win.set_default_size(1280, 800)
view = WebKit2.WebView(web_context=WebKit2.WebContext.new_ephemeral())
win.add(view)
win.show_all()
exit_code = 0
loads = 0

# Screens and sizes the owner works at. Every row class here must stay one line, and every pane must keep its width.
SIZES = ((1280, 800), (1920, 1080))
PROBE = r"""
const skip = 'dialog, [popover], .sidebar, .titlebar, .cm-editor, .ws-live';
const rowSel = '.tn, .gf, .gg, .res-file, .etab, .hrow, .git-sum, .ecrumb, .side-h, .side-opts, .chg-tools, .gr, .sp-plan-h, .pt-row';
const clipOf = (el, self = false) => { let c = {l: 0, t: 0, r: innerWidth, b: innerHeight};
  for (let a = self ? el : el.parentElement; a && a !== document.documentElement; a = a.parentElement) { const s = getComputedStyle(a);
    if (s.overflowX !== 'visible' || s.overflowY !== 'visible') { const r = a.getBoundingClientRect(); c = {l: Math.max(c.l, r.left), t: Math.max(c.t, r.top), r: Math.min(c.r, r.right), b: Math.min(c.b, r.bottom)}; } }
  return c; };
const seen = el => { if (el.closest(skip)) return false; const s = getComputedStyle(el); return s.visibility !== 'hidden' && s.display !== 'none' && +s.opacity !== 0; };
const cut = (r, c) => ({l: Math.max(r.left, c.l), t: Math.max(r.top, c.t), r: Math.min(r.right, c.r), b: Math.min(r.bottom, c.b)});
const items = [];
const walk = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
for (let n = walk.nextNode(); n; n = walk.nextNode()) { const p = n.parentElement; if (!n.nodeValue.trim() || !p || !seen(p)) continue;
  const g = document.createRange(); g.selectNodeContents(n); const c = clipOf(p, true);
  for (const r of g.getClientRects()) { const x = cut(r, c); if (x.r - x.l > 1 && x.b - x.t > 1) items.push({node: p, kind: 'text', label: n.nodeValue.trim().slice(0, 32), ...x}); } }
for (const el of document.querySelectorAll('button, a, input, textarea')) { if (!seen(el)) continue; const x = cut(el.getBoundingClientRect(), clipOf(el)); if (x.r - x.l > 1 && x.b - x.t > 1) items.push({node: el, kind: 'control', label: (el.getAttribute('aria-label') || el.textContent || '').trim().slice(0, 32), ...x}); }
const problems = [];
for (let i = 0; i < items.length && problems.length < 40; i++) for (let j = i + 1; j < items.length && problems.length < 40; j++) { const a = items[i], b = items[j];
  if (a.node === b.node || a.node.contains(b.node) || b.node.contains(a.node)) continue;
  const w = Math.min(a.r, b.r) - Math.max(a.l, b.l), h = Math.min(a.b, b.b) - Math.max(a.t, b.t);
  if (w > 1 && h > 1) problems.push(`overlap ${a.kind} "${a.label}" with ${b.kind} "${b.label}" (${Math.round(w)}x${Math.round(h)})`); }
for (const row of document.querySelectorAll(rowSel)) { if (!seen(row) || row.offsetWidth === 0) continue;
  if (row.scrollHeight > row.clientHeight + 1) problems.push(`row taller than its box: ${row.className} "${row.textContent.trim().slice(0, 32)}" (${row.scrollHeight}>${row.clientHeight})`);
  const tw = document.createTreeWalker(row, NodeFilter.SHOW_TEXT);
  for (let n = tw.nextNode(); n; n = tw.nextNode()) { if (!n.nodeValue.trim() || !n.parentElement || !seen(n.parentElement)) continue; const g = document.createRange(); g.selectNodeContents(n); const tops = new Set([...g.getClientRects()].filter(r => r.width > 1).map(r => Math.round(r.top))); if (tops.size > 1) problems.push(`wrapped text in ${row.className}: "${n.nodeValue.trim().slice(0, 32)}"`); } }
for (const el of document.querySelectorAll('.git-sum [data-why], .ecrumb [data-why], .sp-plan-h [data-why], .commit-box [data-why]')) { const c = getComputedStyle(el, '::after').content; if (c && c !== 'none' && c !== 'normal') problems.push(`inline reason inside a one-line toolbar: ${c.slice(0, 40)}`); }
for (const pane of document.querySelectorAll('.side-pane, .git-right, .ed')) { if (!seen(pane)) continue; const p = pane.getBoundingClientRect();
  for (const it of items) if (pane.contains(it.node) && (it.l < p.left - 1 || it.r > p.right + 1 || it.t < p.top - 1 || it.b > p.bottom + 1)) problems.push(`left pane ${pane.className}: "${it.label}"`);
  if (pane.matches('.side-pane') && p.width < 240) problems.push(`pane too narrow: ${pane.className} ${p.width}`);
  if (pane.matches('.git-right, .ed') && p.width < 480) problems.push(`pane too narrow: ${pane.className} ${p.width}`); }
for (const el of document.querySelectorAll('.ecrumb button, .etab-tools button, .git-sum button, .chg-tools button')) { const r = el.getBoundingClientRect(), c = clipOf(el); if (r.left < c.l - 1 || r.right > c.r + 1 || r.top < c.t - 1 || r.bottom > c.b + 1) problems.push(`control cut off: ${(el.textContent || el.getAttribute('aria-label')).trim().slice(0, 32)}`); }
return {problems: [...new Set(problems)], rows: document.querySelectorAll(rowSel).length, controls: items.filter(i => i.kind === 'control').length};
"""


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


def load(name, width, height, ready):
    global loads
    loads += 1
    main(lambda b, d: (win.resize(width, height), d.set()))
    main(lambda b, d: (view.load_uri(sys.argv[1] + f"?view={name}&load={loads}"), d.set()))
    wait(f"window.fixture === '{loads}' && document.querySelector('.titlebar')")
    wait(ready)
    time.sleep(.5)
    actual = js("return [innerWidth, innerHeight]")
    assert actual == [width, height], (actual, width, height)


def capture(name):
    time.sleep(.3)  # Let WebKit paint the observed state before photographing it.
    def call(box, done):
        width, height = win.get_size()
        pixels = Gdk.pixbuf_get_from_window(win.get_window(), 0, 0, width, height)
        pixels.savev(str(evidence / (label + "-" + name + ".png")), "png", [], [])
        box["value"] = [width, height]
        done.set()
    return main(call)


def check(results, name):
    result = js(PROBE)
    results[name] = result
    capture(name)
    return result["problems"]


def click(selector, text=None):
    needle = "e.textContent.trim()===" + json.dumps(text) if text else "true"
    wait("[...document.querySelectorAll(" + json.dumps(selector) + ")].some(e=>" + needle + ")")
    js("[...document.querySelectorAll(" + json.dumps(selector) + ")].find(e=>" + needle + ").click()")
    time.sleep(.3)


def run():
    global exit_code
    results, failures = {}, {}
    try:
        for width, height in SIZES:
            size = f"{width}x{height}"
            load("git", width, height, "document.querySelector('.git-sum') && document.querySelector('.hrow')")
            failures[f"git-{size}"] = check(results, f"git-{size}")
            click(".chg-tools button", "Branches and worktrees")
            wait("document.querySelector('.gr')")
            failures[f"branches-{size}"] = check(results, f"branches-{size}")
            load("code", width, height, "document.querySelector('.cm-content') && document.querySelector('.ecrumb button')")
            click(".side-h .seg button", "Files")
            wait("document.querySelector('.tn')")
            failures[f"code-{size}"] = check(results, f"code-{size}")
            for name in ("canonical-exact", "canonical-pricing", "audit-company", "check-model"):
                js("[...document.querySelectorAll('.tn')].find(e=>e.textContent.includes(" + json.dumps(name) + ")).click()")
                time.sleep(.2)
            wait("document.querySelectorAll('.etab').length >= 4")
            failures[f"code-tabs-{size}"] = check(results, f"code-tabs-{size}")
            click(".side-h .seg button", "Search")
            js("const i=document.querySelector('.ws-find-in');i.value='line';i.dispatchEvent(new Event('input',{bubbles:true}))")
            wait("document.querySelector('.res-file')")
            failures[f"search-{size}"] = check(results, f"search-{size}")
        load("sprint", 1280, 800, "document.querySelector('.brief')")
        failures["sprint-1280x800"] = check(results, "sprint-1280x800")
        results["sprint"] = js("return {brief: document.querySelector('.brief').innerHTML, ol: document.querySelectorAll('.brief ol li').length, br: document.querySelectorAll('.brief br').length, note: document.querySelectorAll('.sp-conv .ai br').length, mine: document.querySelectorAll('.sp-conv .mine br').length}")
        capture("sprint-brief")
        click(".ws-more-l")
        wait("document.querySelector('.ws-read-d')")
        failures["read-more-1280x800"] = check(results, "read-more-1280x800")
        results["read_more"] = js("return {h1: document.querySelectorAll('.ws-read-d h1').length, ol: document.querySelectorAll('.ws-read-d ol li').length, br: document.querySelectorAll('.ws-read-d br').length, accept: document.querySelectorAll('.ws-read-d li').length}")
        capture("read-more")
        load("task", 1280, 800, "document.querySelector('.brief')")
        results["task"] = js("return {ol: document.querySelectorAll('.brief ol li').length, br: document.querySelectorAll('.brief br').length}")
        capture("task-brief")
        failures["text"] = [] if results["sprint"]["ol"] == 2 and results["sprint"]["br"] >= 1 and results["sprint"]["note"] >= 1 and results["sprint"]["mine"] >= 1 and results["read_more"]["h1"] == 1 and results["read_more"]["ol"] == 2 and results["read_more"]["br"] >= 1 and results["read_more"]["accept"] >= 4 and results["task"]["ol"] == 2 and results["task"]["br"] >= 1 else [json.dumps({k: results[k] for k in ("sprint", "read_more", "task")})]
        # The probe proves it can fail: the same page with reasons back inside the toolbar and names free to wrap must be caught.
        load("git", 1280, 800, "document.querySelector('.git-sum')")
        js("const s=document.createElement('style');s.textContent='.git-sum [data-why]::after{content:attr(data-why)!important}.git-sum .cap{white-space:normal!important;overflow:visible!important}.ws-gf .p,.hrow .trunc{white-space:normal!important;overflow:visible!important}';document.head.append(s)")
        time.sleep(.3)
        results["probe_catches_faults"] = len(js(PROBE)["problems"])
        failures["probe"] = [] if results["probe_catches_faults"] > 0 else ["the probe did not notice an injected fault"]
        results["failures"] = {k: v for k, v in failures.items() if v}
        (evidence / (label + "-metrics.json")).write_text(json.dumps(results, indent=2) + "\n")
        print(json.dumps({"checked": list(failures), "failed": {k: {"count": len(v), "first": v[:4]} for k, v in results["failures"].items()}}, indent=1), flush=True)
        assert not results["failures"], "rendered layout problems"
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
