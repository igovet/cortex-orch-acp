#!/usr/bin/env python3
"""Text-only sweep of the whole app's interface rules in real WebKitGTK. jsdom has no layout or cascade, so Vitest cannot answer this.

It builds the demo workspace, opens every screen in a real WebKit view on a private X display inside the rendered-check image,
and reads text, rectangles and computed styles. Both themes, two window sizes, every popover, and the dialogs a screen opens with
a "…" button. No screenshot is taken and no image is read. Exit code 1 means a rule was broken somewhere.

    python3 -B packages/workspace-ui/test/sweep.measure.py
    SWEEP_ONLY=settings python3 -B packages/workspace-ui/test/sweep.measure.py
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
SMALL = (1280, 800)

# Every rule is one question put to the rendered page. A hit is a broken rule, never a warning.
AUDIT = r"""
window.__audit = (root, page) => {
  const out = [], seen = new Set();
  const sign = el => { const c = typeof el.className === 'string' ? el.className.trim().split(/\s+/).slice(0, 3).join('.') : ''; return el.tagName.toLowerCase() + (c ? '.' + c : ''); };
  const add = (rule, el, detail) => { const key = rule + '|' + sign(el) + '|' + String(detail).slice(0, 60); if (!seen.has(key)) { seen.add(key); out.push({ rule, at: sign(el), detail: String(detail).slice(0, 160) }); } };
  const shown = el => { const r = el.getBoundingClientRect(), s = getComputedStyle(el); return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none'; };
  const ID = /\b(?:xr|cn|xa|es|pq|ch|ar|ara|cr|cg|lo|ws)_[0-9a-f]{6,}\b|\b[trd]_[0-9a-f]{8,}\b|\b[0-9a-f]{20,}\b|sha256|mock-(?:machine|project|task)/;
  const CORTEX = /\bCortex\b/;
  const FILLER = /^(?:—|-|–|n\/a|N\/A|null|undefined|NaN|All 0|0\/0)$/;
  const diagnostics = /\/diagnostics$/.test(page);
  const say = (el, text) => { if (!diagnostics && ID.test(text)) add('technical id', el, text); if (CORTEX.test(text) && !el.closest('.ws-user')) add('says Cortex', el, text); };

  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    const el = n.parentElement, text = n.nodeValue.trim();
    if (!text || !el || el.closest('script, style, .ws-demo-banner, .ws-demo-menu, .xterm, .cm-editor, .ws-live')) continue;
    if (el.closest('[popover]:not(:popover-open)') || el.closest('.ws-hint-pop')) continue;
    if (!shown(el)) continue;
    say(el, text);
    if (FILLER.test(text) && !el.closest('.dl, .sl, .sbs, .cl, .code')) add('empty filler', el, text);
    if (/[Ѐ-ӿ]/.test(text) && el.closest('.set-main, .set-side, .ws-hint-pop')) add('not English', el, text);
    // One piece that breaks across lines: a path, a name, an id-like value.
    if (!/\s/.test(text) && text.length >= 8 && !el.closest('pre, code, .code, .sbs, .cl, .dl, .args, .ai, .ws-pre, .mine, .tl-in .md, .md, .feed-in, .stream-in')) {
      const range = document.createRange(); range.selectNodeContents(n);
      const tops = new Set([...range.getClientRects()].map(r => Math.round(r.top)));
      if (tops.size > 1) add('one piece wraps', el, text);
    }
    if (el.closest('.main, .inspector, dialog, .set-main, [popover]') && !el.closest('button, a, label, summary, [role=button], [role=tab], [role=option], [role=menuitem], [role=treeitem], [draggable=true], .titlebar, .sidebar, .col-h, .ws-resize, .kbd, .cl > .n, .dl > .n, .sl > .n')) {
      const s = getComputedStyle(el);
      if ((s.webkitUserSelect ?? s.userSelect) === 'none' && (s.userSelect ?? 'none') === 'none') add('text cannot be selected', el, text);
    }
  }
  for (const el of root.querySelectorAll('[title], [aria-label], [placeholder]')) {
    if (el.closest('.ws-demo-banner, .ws-demo-menu')) continue;
    for (const a of ['title', 'aria-label', 'placeholder']) { const v = el.getAttribute(a); if (v) say(el, v); }
  }
  for (const el of root.querySelectorAll('input:not([type=checkbox]):not([type=radio]):not([type=file])')) if (shown(el) && el.value && !diagnostics && ID.test(el.value)) add('technical id', el, el.value);
  for (const el of root.querySelectorAll('.ws-hint-pop')) { if (/[Ѐ-ӿ]/.test(el.textContent)) add('not English', el, el.textContent.slice(0, 80)); if (CORTEX.test(el.textContent)) add('says Cortex', el, el.textContent.match(/.{0,50}\bCortex\b.{0,30}/)?.[0]); }

  for (const el of root.querySelectorAll('.count, .n, .sig, .badge, .task-row.all')) if (shown(el) && /^(?:All\s+)?0(?:\/0)?$/.test(el.textContent.trim())) add('zero counter', el, el.textContent.trim());

  for (const el of root.querySelectorAll('input[type=checkbox], input[type=radio]')) {
    if (el.closest('.sw') || !shown(el)) continue;
    const r = el.getBoundingClientRect();
    if (Math.abs(r.width - r.height) > .5 || r.width > 18) add('box is not square', el, `${r.width}x${r.height}`);
  }

  // Left and right gaps inside the nearest thing with an edge. A row flush on both sides is fine, one flush side never is.
  const framed = p => { const s = getComputedStyle(p); return parseFloat(s.borderLeftWidth) > 0 || parseFloat(s.borderRightWidth) > 0 || s.backgroundColor !== 'rgba(0, 0, 0, 0)' || /auto|scroll/.test(s.overflowY); };
  const ROWS = '.nav, .snav, .proj-wrap, .task-row, .mi, .lrow, .card, .opt, .ws-pal-o, .pt-row, .surface, .strip, .note-box, .composer, .dc, .sb-usage, .tree-row, .frow, .wrow, .gf, .rc, .ws-add, .col-add, .snav-h, .sb-h';
  for (const el of root.querySelectorAll(ROWS)) {
    if (!shown(el) || el.closest('.v-row[style*="display: none"]')) continue;
    let p = el.parentElement; while (p && p !== document.body && !framed(p)) p = p.parentElement;
    if (!p || p === document.body) continue;
    const r = el.getBoundingClientRect(), b = p.getBoundingClientRect(), s = getComputedStyle(p);
    const left = r.left - b.left - parseFloat(s.borderLeftWidth), right = b.right - parseFloat(s.borderRightWidth) - r.right, inner = b.width - parseFloat(s.borderLeftWidth) - parseFloat(s.borderRightWidth);
    if (r.width < inner * .5 || left > 48 || right > 48 || left < -1 || right < -1) continue;
    if (Math.abs(left - right) > 1) add('uneven gaps', el, `left ${left.toFixed(1)} right ${right.toFixed(1)} in ${sign(p)}`);
  }
  for (const el of root.querySelectorAll('.gr, .srow, .mi, .nav, .snav:not(.sub), .opt, .card-t, .strip')) {
    if (!shown(el)) continue;
    const s = getComputedStyle(el);
    if (Math.abs(parseFloat(s.paddingLeft) - parseFloat(s.paddingRight)) > 1) add('uneven padding', el, `${s.paddingLeft} ${s.paddingRight}`);
  }

  // A label is one line, and whatever is cut has a way to be read whole.
  for (const el of root.querySelectorAll('.btn, .chip, .tab, .st, .seg button, .pick, .sig, .kbd, .gr.th > span, .set-h')) {
    if (!shown(el) || !el.textContent.trim()) continue;
    const range = document.createRange(); range.selectNodeContents(el);
    const tops = new Set([...range.getClientRects()].filter(r => r.width > 1 && r.height > 6).map(r => Math.round(r.top + r.height / 2)));
    if (tops.size > 1 && el.getBoundingClientRect().height > 40) add('label wraps', el, el.textContent.trim());
  }
  for (const el of root.querySelectorAll('*')) {
    if (el.children.length > 3 || !shown(el) || el.closest('.xterm, .cm-editor')) continue;
    const s = getComputedStyle(el);
    if (s.textOverflow !== 'ellipsis' || s.overflowX === 'visible' || el.scrollWidth <= el.clientWidth + 1) continue;
    el.dispatchEvent(new MouseEvent('mouseover', { bubbles: true }));
    const whole = el.textContent.trim(), help = el.closest('[title]')?.getAttribute('title') ?? '';
    if (!help.includes(whole)) add('cut with no way to read it', el, whole);
    if (el.closest('.orrow, .okrow')) add('routing table cuts a value', el, whole);
  }
  return out;
};
"""


def outside():
    root = Path(__file__).resolve().parents[3]
    site = root / ".workspace-build/sweep-measure/site"
    site.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(["npx", "vite", "build", "--logLevel", "error", "--emptyOutDir", "--outDir", str(site)], cwd=root / "prototypes/cortex-workspace/desktop", check=True)
    command = ["docker", "run", "--rm", "--network", "none", "-e", "HOME=/tmp", "-e", "XDG_RUNTIME_DIR=/tmp", "-e", f"SWEEP_ONLY={os.environ.get('SWEEP_ONLY', '')}", "-e", f"SWEEP_QUICK={os.environ.get('SWEEP_QUICK', '')}",
               "-v", f"{site}:/site:ro", "-v", f"{Path(__file__).resolve()}:/measure.py:ro", IMAGE, "python3", "-B", "/measure.py", "--inside"]
    return subprocess.run(command, timeout=1700).returncode


def inside():
    import gi
    gi.require_version("Gtk", "3.0")
    gi.require_version("Gdk", "3.0")
    gi.require_version("WebKit2", "4.1")
    from gi.repository import Gdk, GLib, Gtk, WebKit2
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
    broken, facts, visited = {}, {}, []
    only = os.environ.get("SWEEP_ONLY", "")

    def on_main(call):
        done, box = threading.Event(), {}
        def run():
            call(box, done)
            return False
        GLib.idle_add(run)
        if not done.wait(20):
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
            time.sleep(.08)
            try:
                if js("return document.readyState === 'complete' && !!document.querySelector('.titlebar') && document.querySelectorAll('.nav, .snav').length > 0"):
                    break
            except json.JSONDecodeError:
                pass
        else:
            raise TimeoutError("no workspace at " + path)
        time.sleep(.25)
        js(AUDIT + "document.querySelector('.ws-demo-banner')?.remove(); return 1")

    def size(width, height):
        on_main(lambda box, done: (win.resize(width, height), done.set()))
        time.sleep(.4)

    def note(where, hits):
        for hit in hits or []:
            rule = broken.setdefault(hit["rule"], {})
            key = f"{hit['at']} {hit['detail']}"
            rule.setdefault(key, where)

    def audit(where, root="document.body"):
        note(where, js(f"return __audit({root}, location.pathname)"))

    def popovers(where):
        ids = js("return [...document.querySelectorAll('[popover]')].map((p, i) => { p.dataset.sw = i; return i; })")
        for i in ids:
            hits = js(f"""const p = document.querySelector('[data-sw="{i}"]'); if (!p) return []; try {{ p.showPopover(); }} catch {{ return []; }} const out = __audit(p, location.pathname); p.hidePopover(); return out;""")
            note(f"{where} popover", hits)

    def dialogs(path):
        names = js("return [...document.querySelectorAll('.main button, .set-main button, .inspector button, .titlebar button')].filter(b => /…$/.test(b.textContent.trim()) && b.getAttribute('aria-disabled') !== 'true' && !b.disabled).map(b => b.textContent.trim()).slice(0, 6)") or []
        for name in dict.fromkeys(names):
            load(path)
            opened = js(f"""const b = [...document.querySelectorAll('.main button, .set-main button, .inspector button, .titlebar button')].find(b => b.textContent.trim() === {json.dumps(name)}); if (!b) return false; b.click(); return true;""")
            if not opened:
                continue
            time.sleep(.2)
            if js("return !!document.querySelector('dialog[open]')"):
                audit(f"{path} dialog {name}", "document.querySelector('dialog[open]')")
                facts["dialogs"] = facts.get("dialogs", 0) + 1

    def screens():
        load("/")
        machine = js("return location.pathname.split('/')[2]")
        project = js("return document.querySelector('.proj').getAttribute('href').split('/')[4]")
        at, pj = f"/m/{machine}", f"/m/{machine}/p/{project}"
        load(f"{pj}/board")
        tasks = js("""const seen = new Map(); for (const c of document.querySelectorAll('.card[data-task]')) { const col = c.closest('.col')?.dataset.col ?? ''; if (!seen.has(col)) seen.set(col, c.dataset.task); } return [...seen.values()];""") or []
        load(at)
        sprint = js("const a = [...document.querySelectorAll('.task-row')].find(a => /\\/s\\//.test(a.getAttribute('href') ?? '')); return a ? a.getAttribute('href').replace(/\\/(board|list)$/, '') : null")
        settings = f"/settings/machine/{machine}"
        pages = [at, f"{at}/inbox", f"{at}/archive", f"{at}/new", f"{at}/new?kind=sprint"]
        pages += [pj, f"{pj}/list", f"{pj}/code", f"{pj}/git", f"{pj}/worktrees", f"{pj}/knowledge", f"{pj}/settings"]
        if sprint:
            pages += [f"{sprint}/planning", f"{sprint}/summary", f"{sprint}/board", f"{sprint}/list"]
        for task in tasks:
            base = f"{at}/t/{task}"
            pages += [base, f"{base}/workers", f"{base}/changes", f"{base}/code"]
        pages += [f"/settings/app/{s}" for s in ("appearance", "voice", "updates", "shortcuts", "machines")]
        pages += [f"{settings}/{s}" for s in ("overview", "orchestration", "connections", "worktrees", "storage", "diagnostics", "remote")]
        pages += [f"{settings}/orchestration/{p}" for p in ("defaults", "models", "routing", "routing?tab=kinds", "routing?tab=quota", "routing?tab=rules", "effective", "policy", "tools", "profiles", "files")]
        return [p for p in pages if only in p]

    def text_copies():
        """Ctrl+C after a real selection, read back from the real clipboard."""
        clip = lambda: on_main(lambda box, done: (box.__setitem__("value", Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD).wait_for_text()), done.set()))
        load("/")
        machine = js("return location.pathname.split('/')[2]")
        project = js("return document.querySelector('.proj').getAttribute('href').split('/')[4]")
        load(f"/m/{machine}/p/{project}/board")
        task = js("return document.querySelector('.card[data-task]')?.dataset.task ?? null")
        spots = [(f"/m/{machine}/t/{task}", ".tl-in .md, .tl-in .ai, .tl-in p, .tl-in .brief, .tl-in div"), (f"/settings/machine/{machine}/overview", ".srow .d"), (f"/settings/app/appearance", ".set-lead")]
        for path, selector in spots:
            load(path)
            at = js(f"""const el = [...document.querySelectorAll({json.dumps(selector)})].find(e => !e.children.length && e.textContent.trim().length > 12 && e.getClientRects().length); if (!el) return null; el.scrollIntoView({{ block: 'center' }}); const range = document.createRange(); range.selectNodeContents(el); const r = range.getClientRects()[0]; return [Math.round(r.x + 6), Math.round(r.y + r.height / 2), Math.round(Math.min(r.right - 4, r.x + 200)), el.textContent.trim().slice(0, 40)];""")
            if at is None:
                facts.setdefault("no_copy_spot", []).append(path)
                continue
            x("mousemove", at[0], at[1])
            x("mousedown", 1)
            x("mousemove", at[2], at[1])
            x("mouseup", 1)
            picked = js("return getSelection().toString()")
            x("key", "ctrl+c")
            time.sleep(.3)
            got = clip()
            ok = bool(picked) and got == picked
            facts.setdefault("copied", []).append({"path": path.split("/")[-1], "selected": len(picked or ""), "same": ok})
            if not ok:
                broken.setdefault("Ctrl+C does not copy", {})[f"{path} selected {picked!r} clipboard {got!r}"] = path

    def scrollers():
        """Whether a reserved scrollbar is really reserved, so a gap measured here holds with and without a thumb."""
        load("/")
        machine = js("return location.pathname.split('/')[2]")
        project = js("return document.querySelector('.proj').getAttribute('href').split('/')[4]")
        load(f"/m/{machine}/p/{project}/board")
        facts["scrollers"] = js("""return [...document.querySelectorAll('.sb-scroll, .col-b')].slice(0, 3).map(e => { const s = getComputedStyle(e); return { el: e.className, gutter: e.offsetWidth - e.clientWidth, overflow: s.overflowY, sg: s.scrollbarGutter, pl: s.paddingLeft, pr: s.paddingRight }; })""")

    def run():
        load("/")
        themes = ("dark",) if os.environ.get("SWEEP_QUICK") else ("dark", "light")
        for theme in themes:
            load("/")
            js(f"localStorage.setItem('cw.theme', {json.dumps(theme)}); return 1")
            for path in screens():
                load(path)
                audit(f"{theme} {path}")
                popovers(f"{theme} {path}")
                visited.append(path)
                if theme == "dark":
                    dialogs(path)
        if not os.environ.get("SWEEP_QUICK"):
            size(*SMALL)
            load("/")
            js("localStorage.setItem('cw.theme', 'dark'); return 1")
            for path in screens():
                load(path)
                audit(f"small {path}")
                visited.append(path)
            size(*WINDOW)
        scrollers()
        text_copies()

    def main():
        code = 1
        try:
            time.sleep(1)
            run()
            code = 1 if broken else 0
        except Exception as error:  # A broken run is a failed run, never a pass.
            broken.setdefault("harness", {})[repr(error)] = "run"
        report = {rule: {"count": len(hits), "first": dict(list(hits.items())[:40])} for rule, hits in broken.items()}
        print(json.dumps({"engine": f"WebKitGTK {WebKit2.get_major_version()}.{WebKit2.get_minor_version()}.{WebKit2.get_micro_version()}", "screens": len(visited), "distinct": len(set(visited)), "broken_rules": len(broken), "facts": facts, "broken": report}, indent=1, ensure_ascii=False))
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
