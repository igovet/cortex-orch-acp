"""Opt-in render of the tab strip in WebKitGTK on a private virtual screen: real layout, scroll and hit testing, fake data."""
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


def capture(name):
    time.sleep(.4)  # Let WebKit paint the observed state before photographing it.
    def call(box, done):
        width, height = win.get_size()
        Gdk.pixbuf_get_from_window(win.get_window(), 0, 0, width, height).savev(str(evidence / (name + ".png")), "png", [], [])
        box["value"] = [width, height]
        done.set()
    return main(call)


TIMELINE = ".ws-pane:not([data-parked]) .timeline"


def run():
    global exit_code
    facts = {}
    try:
        main(lambda b, d: (view.load_uri(sys.argv[1] + "?view=conversation"), d.set()))
        wait("window.fixture && document.querySelector('.ws-tabstrip')")
        js("document.documentElement.dataset.theme='dark'")
        wait(f"document.querySelector('{TIMELINE}')")
        time.sleep(.4)
        js(f"const t=document.querySelector('{TIMELINE}');const f=t.querySelector('button.fold');if(f)f.click();t.scrollTop=90")
        time.sleep(.3)
        facts["before"] = js(f"const t=document.querySelector('{TIMELINE}');return {{scroll:t.scrollTop,height:t.scrollHeight,open:t.querySelectorAll('button.fold[aria-expanded=\"true\"]').length}}")
        js("document.querySelector('.sidebar a.nav').click()")
        wait("document.querySelector('.ws-pane:not([data-parked]) textarea')")
        js("const a=document.querySelector('.ws-pane:not([data-parked]) textarea');a.focus();document.execCommand('insertText',false,'A draft I will come back to')")
        js("document.querySelector('button[aria-label=\"New tab\"]').click()")
        wait("document.querySelectorAll('.ws-tab').length===2")
        time.sleep(.2)
        js("document.querySelector('.sidebar a.nav[href*=\"inbox\"]').click()")
        time.sleep(.3)
        js("const n=document.querySelectorAll('.ws-tab-name')[0];n.click()")
        time.sleep(.4)
        facts["strip"] = js("return {titles:[...document.querySelectorAll('.ws-tab-name')].map(e=>e.textContent),selected:[...document.querySelectorAll('.ws-tab-name')].map(e=>e.getAttribute('aria-selected')),rect:(()=>{const r=document.querySelector('.ws-tabstrip').getBoundingClientRect();return [r.left,r.top,r.width,r.height]})()}")
        facts["size_new_task"] = capture("strip-new-task-1440")
        facts["draft"] = js("return {value:document.querySelector('.ws-pane:not([data-parked]) textarea')?.value}")
        assert facts["draft"]["value"] == "A draft I will come back to", facts
        # The first tab, on the conversation, was moved to the new-task route in place by the sidebar: open a task in tab 2 to prove the scroll.
        js("document.querySelectorAll('.ws-tab-name')[1].click()")
        time.sleep(.2)
        js("const a=document.querySelector('.sidebar a.task-row');if(a)a.click()")
        wait(f"document.querySelector('{TIMELINE}')")
        time.sleep(.4)
        main(lambda b, d: (win.resize(1200, 460), d.set()))  # Short enough that the conversation scrolls.
        time.sleep(.4)
        js(f"const t=document.querySelector('{TIMELINE}');const f=t.querySelector('button.fold');if(f&&f.getAttribute('aria-expanded')!=='true')f.click();t.scrollTop=90")
        time.sleep(.3)
        facts["task_before"] = js(f"const t=document.querySelector('{TIMELINE}');return {{scroll:t.scrollTop,open:t.querySelectorAll('button.fold[aria-expanded=\"true\"]').length}}")
        facts["size_task"] = capture("strip-task-1440")
        js("document.querySelectorAll('.ws-tab-name')[0].click()")
        time.sleep(.4)
        facts["parked"] = js("const p=document.querySelector('.ws-pane[data-parked]');return {visibility:getComputedStyle(p).visibility,width:p.offsetWidth>0}")
        js("document.querySelectorAll('.ws-tab-name')[1].click()")
        time.sleep(.4)
        facts["task_after"] = js(f"const t=document.querySelector('{TIMELINE}');return {{scroll:t.scrollTop,open:t.querySelectorAll('button.fold[aria-expanded=\"true\"]').length}}")
        assert facts["task_after"]["open"] == facts["task_before"]["open"], facts
        assert facts["task_before"]["scroll"] > 0 and abs(facts["task_after"]["scroll"] - facts["task_before"]["scroll"]) < 2, facts
    except Exception as error:
        facts["failure"] = repr(error)
        exit_code = 1
        try:
            capture("failure")
        except Exception:
            pass
    print(json.dumps(facts, indent=1))
    main(lambda b, d: (Gtk.main_quit(), d.set()))


threading.Thread(target=run, daemon=True).start()
Gtk.main()
xvfb.kill()
sys.exit(exit_code)
