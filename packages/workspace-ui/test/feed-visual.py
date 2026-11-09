"""Opt-in static-bundle render of the feed screens in WebKitGTK; fake data, private virtual screen."""
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
win.set_default_size(1440, 1000)
view = WebKit2.WebView(web_context=WebKit2.WebContext.new_ephemeral())
win.add(view)
win.show_all()
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


def load(name, width=1440, height=1000, theme="dark"):
    main(lambda b, d: (win.resize(width, height), d.set()))
    main(lambda b, d: (view.load_uri(sys.argv[1] + "?view=" + name), d.set()))
    wait("window.fixture && document.querySelector('.titlebar')")
    js("document.documentElement.dataset.theme=" + json.dumps(theme))
    time.sleep(.4)


def capture(name):
    time.sleep(.4)  # Let WebKit paint the observed state before photographing it.
    def call(box, done):
        width, height = win.get_size()
        pixels = Gdk.pixbuf_get_from_window(win.get_window(), 0, 0, width, height)
        pixels.savev(str(evidence / (label + "-" + name + ".png")), "png", [], [])
        box["value"] = [width, height]
        done.set()
    return main(call)


def scan():
    """Every visible scrollbar sits clear of the outer 5px the window keeps for edge resizing."""
    return js("""const W=innerWidth,H=innerHeight;const out=[];
      for(const el of document.querySelectorAll('*')){const s=getComputedStyle(el);if(el.offsetWidth===0)continue;
        const r=el.getBoundingClientRect();
        if(/auto|scroll/.test(s.overflowY)&&el.scrollHeight>el.clientHeight+1){if(r.right>W-5) out.push({c:el.className,axis:'y',gap:W-r.right});}
        if(/auto|scroll/.test(s.overflowX)&&el.scrollWidth>el.clientWidth+1){if(r.bottom>H-5) out.push({c:el.className,axis:'x',gap:H-r.bottom});}}
      return out""")


def run():
    global exit_code
    results = {}
    try:
        load("worker")
        wait("document.querySelector('.ws-worker-result')")
        results["order"] = js("const k=[...document.querySelector('.stream-in').children];return k[k.length-1].className")
        assert "ws-worker-result" in results["order"]
        results["agent_labels"] = js("""return [...document.querySelectorAll(".wcard .l3")].filter(e=>/opus|luna/.test(e.textContent)).map(e=>({text:e.textContent,visible:e.getBoundingClientRect().height>0&&getComputedStyle(e).display!=="none"}))""")
        assert len(results["agent_labels"]) == 2 and all(e["visible"] for e in results["agent_labels"]), results["agent_labels"]
        results["worker_size"] = capture("worker-collapsed-1440")
        js("document.querySelector('.stream button.fold').click()")
        time.sleep(.2)
        js("document.querySelector('.step-row').scrollIntoView()")
        capture("worker-open-1440")
        results["steps"] = js("return [...document.querySelectorAll('.step-row')].slice(0,3).map(r=>r.textContent)")
        results["scan_worker"] = scan()
        js("document.querySelectorAll('.stream .step-row')[2].click()")
        time.sleep(.2)
        capture("worker-shell-open-1440")
        load("worker", theme="light")
        capture("worker-light-1440")
        load("working")
        capture("worker-running-1440")
        load("coordinator")
        wait("document.querySelector('.livepane .ev-note')")
        results["coordinator"] = js("return {text:document.querySelector('.livepane').textContent.includes('Planning the page'),nothing:document.querySelector('.livepane').textContent.includes('Nothing to report'),model:document.querySelector('.d-grid').textContent}")
        capture("coordinator-1440")
        results["scan_coordinator"] = scan()
        load("conversation")
        js("document.querySelectorAll('.timeline button.fold')[0].click();document.querySelectorAll('.timeline button.fold')[1].click()")
        time.sleep(.2)
        capture("conversation-1440")
        results["scan_conversation"] = scan()
        load("review")
        wait("document.querySelector('.rv-bar')")
        js("document.querySelectorAll('details').forEach(e=>{if(e.querySelector('summary')?.textContent.includes('Command and output'))e.open=true})")
        capture("review-failed")
        results["review_failed"] = js("return document.querySelector('.rv-bar').textContent.includes('1 passed, 1 failed') && !!document.querySelector('[aria-label=\\\"Rerun Focused tests\\\"]')")
        assert results["review_failed"]
        js("document.querySelector('[aria-label=\\\"Rerun Focused tests\\\"]').click()")
        wait("document.body.textContent.includes('Fresh rerun: all assertions passed')")
        js("document.querySelectorAll('details').forEach(e=>{if(e.querySelector('summary')?.textContent.includes('Command and output'))e.open=true})")
        results["manual_accept"] = js("const b=[...document.querySelectorAll('button')].find(e=>e.textContent==='Accept v1');return !!b && !b.disabled && b.getAttribute('aria-disabled')!=='true' && document.body.textContent.includes('Check manually')")
        assert results["manual_accept"]
        js("const box=document.querySelector('.evid-b');box.scrollTop=box.scrollHeight")
        capture("review-fresh-output")
        js("document.querySelector('.evid-b').scrollTop=0")
        capture("review-rerun-passed")
        js("[...document.querySelectorAll('button')].find(e=>e.textContent==='Accept v1').click()")
        wait("document.querySelector('.ready') && [...document.querySelectorAll('.ready button')].some(e=>e.getClientRects().length && e.textContent==='Commit to main…')")
        results["check_output_widths"] = js("return [...document.querySelectorAll('.ready details.nv > summary')].map(e=>e.getBoundingClientRect().width)")
        assert len(results["check_output_widths"]) == 2 and min(results["check_output_widths"]) > 200, results["check_output_widths"]
        capture("same-branch-commit")
        results["same_branch"] = js("return ![...document.querySelectorAll('button')].some(b=>b.textContent==='Merge…')")
        assert results["same_branch"]
        load("settings")
        results["scan_settings"] = scan()
        load("board")
        results["scan_board"] = scan()
        for width, height in ((900, 1000), (390, 844)):
            load("worker", width, height)
            capture("worker-" + str(width))
        results["titles"] = js("return document.querySelectorAll('[title]').length")
        assert results["titles"] == 0
        (evidence / (label + "-metrics.json")).write_text(json.dumps(results, indent=2) + "\n")
        print(json.dumps(results), flush=True)
        for name in ("scan_worker", "scan_coordinator", "scan_conversation", "scan_settings", "scan_board"):
            assert results[name] == [], (name, results[name])
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
