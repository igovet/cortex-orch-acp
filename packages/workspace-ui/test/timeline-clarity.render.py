"""Disposable WebKitGTK screenshots and real-input checks; fake port, no model calls.
Run inside the rendered-check container with /site and /evidence mounted.
"""
import json
import os
from pathlib import Path
import subprocess
import threading
import time

xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "1920x1200x24", "-nolisten", "tcp"],
                        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
display = xvfb.stdout.readline().strip()
os.environ.update(DISPLAY=":" + display, GDK_BACKEND="x11", NO_AT_BRIDGE="1",
                  WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS="1", WEBKIT_DISABLE_COMPOSITING_MODE="1")
subprocess.Popen(["openbox"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
import gi
gi.require_version("Gtk", "3.0")
gi.require_version("WebKit2", "4.1")
from gi.repository import GLib, Gtk, WebKit2, Gdk
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
win = Gtk.Window()
win.set_decorated(False)
win.set_default_size(1440, 1000)
view = WebKit2.WebView(web_context=WebKit2.WebContext.new_ephemeral())
win.add(view)
win.show_all()
win.move(0, 0)

def on_main(fn):
    done, box = threading.Event(), {}
    def run():
        fn(box, done)
        return False
    GLib.idle_add(run)
    if not done.wait(12):
        raise TimeoutError("WebKit did not answer")
    return box.get("value")

def js(body):
    def call(box, done):
        def finish(source, result, _):
            try:
                box["value"] = source.evaluate_javascript_finish(result).to_string()
            except Exception as error:
                box["value"] = json.dumps({"error": str(error)})
            done.set()
        view.evaluate_javascript("JSON.stringify((() => {" + body + "})() ?? null)", -1, None, None, None, finish, None)
    return json.loads(on_main(call))

def load(path):
    on_main(lambda b, d: (view.load_uri("http://127.0.0.1:" + str(server.server_address[1]) + path), d.set()))
    for _ in range(70):
        time.sleep(.1)
        if js("return !!window.fixture && !!document.querySelector('.titlebar')"):
            time.sleep(.2)
            return
    raise TimeoutError("Fixture failed to load: " + json.dumps(js("return {errors:window.fixtureErrors,fixture:!!window.fixture,body:document.body.textContent.slice(0,300)}")))

def click(selector):
    point = js("const el=document.querySelector(" + json.dumps(selector) + "); el.scrollIntoView({block:'nearest'}); const r=el.getBoundingClientRect(); return [r.x+r.width/2,r.y+r.height/2]")
    subprocess.run(["xdotool", "mousemove", str(round(point[0])), str(round(point[1])), "click", "1"], check=True)
    time.sleep(.15)

def shot(name):
    def take(box, done):
        width, height = win.get_size()
        pixbuf = Gdk.pixbuf_get_from_window(Gdk.get_default_root_window(), 0, 0, width, height)
        pixbuf.savev("/evidence/" + name + ".png", "png", [], [])
        done.set()
    on_main(take)

def main():
    facts = {}
    try:
        load("/m/local/p/p/board")
        click('.card-a .btn-primary')
        facts["busy"] = js("return {busy:!!document.querySelector('.card [aria-busy=true]'),commands:fixture.commands.length}")
        shot("desktop-board-busy")
        assert facts["busy"]["busy"], "Start did not stay busy: " + str(facts["busy"])
        time.sleep(3)
        shot("desktop-board-refused")
        facts["refusal"] = js("return {inline:document.querySelector('.card [role=alert]')?.textContent,global:document.querySelector('.ws-receipt')?.textContent,errors:fixtureErrors}")
        assert facts["refusal"].get("global"), "No visible refusal: " + str(facts["refusal"])
        facts["board_start_commands"] = js("return fixture.commands.length")
        assert facts["board_start_commands"] == 1
        # Real drag events: retain the explicit image geometry when WebKit requests it.
        js("""window.dragFacts=[];const original=DataTransfer.prototype.setDragImage;DataTransfer.prototype.setDragImage=function(el,x,y){const r=el.getBoundingClientRect();dragFacts.push([r.width,r.height]);return original.call(this,el,x,y)};return true""")
        point = js("const el=document.querySelector('.card-link'); const r=el.getBoundingClientRect();window.beforeDrag=[el.closest('.card').getBoundingClientRect().width,el.closest('.card').getBoundingClientRect().height];return[r.x+20,r.y+20]")
        subprocess.run(["xdotool","mousemove",str(round(point[0])),str(round(point[1])),"mousedown","1","mousemove","--sync",str(round(point[0]+90)),str(round(point[1]+40))],check=True)
        time.sleep(.3)
        shot("desktop-board-drag")
        facts["drag_image"] = js("return {source:beforeDrag,images:dragFacts}")
        drop = js("const r=document.querySelector('[data-col=done]').getBoundingClientRect();return[r.x+r.width/2,r.y+40]")
        subprocess.run(["xdotool","mousemove",str(round(drop[0])),str(round(drop[1])),"mouseup","1"],check=True)
        time.sleep(.8)
        subprocess.run(["xdotool","key","Escape"],check=True)
        time.sleep(.3)
        assert facts["drag_image"]["images"], "No native drag event observed"
        assert abs(facts["drag_image"]["source"][0] - facts["drag_image"]["images"][0][0]) < 2
        load("/m/local/t/live")
        shot("desktop-conversation")
        click(".wrow")
        facts["worker_open"] = js("return {path:location.pathname,stream:!!document.querySelector('.livepane .stream'),errors:fixtureErrors}")
        assert facts["worker_open"]["stream"], "Worker did not open: " + str(facts["worker_open"])
        click(".ws-step-detail details summary")
        shot("desktop-worker")
        facts["worker"] = js("return {steps:document.querySelectorAll('.ev').length,raw:document.querySelector('.livepane').textContent.includes('mcp__'),command:document.querySelector('.livepane').textContent.includes('python3 -B check.py'),viewport:[innerWidth,innerHeight]}")
        assert not facts["worker"]["raw"] and facts["worker"]["command"]
        click(".ws-step-path")
        time.sleep(.5)
        facts["file-open"] = js("""return {path:location.pathname+location.search,selected:!!document.querySelector('[role=tab][aria-label="index.html"]'),panel:document.querySelector('.task-panel')?.textContent.slice(0,200),errors:fixtureErrors}""")
        shot("desktop-read-file")
        assert facts["file-open"]["selected"], "Read did not open its file: " + str(facts["file-open"])
        load("/m/local/t/live/workers/w")
        click(".ws-step-detail details summary")
        click('[aria-label="Sidebar"]')
        for width, name in [(768, "tablet-worker"), (390, "mobile-worker")]:
            on_main(lambda b, d: (win.resize(width, 1000), d.set()))
            time.sleep(.4)
            subprocess.run(["xdotool", "mousemove", "1900", "1190"], check=True)
            time.sleep(.3)
            shot(name)
            facts[name] = js("return {viewport:[innerWidth,innerHeight],client:document.documentElement.clientWidth,scroll:document.documentElement.scrollWidth}")
            assert facts[name]["client"] == facts[name]["scroll"], "Page overflow at " + name
        on_main(lambda b, d: (win.resize(1440, 1000), d.set()))
        load("/m/local/p/p/s/sp/planning")
        facts["single-node-graph"] = js("const g=document.querySelector('.depg svg');const r=g?.getBoundingClientRect(); return {width:r?.width,height:r?.height,viewBox:g?.getAttribute('viewBox'),errors:fixtureErrors}")
        assert 0 < facts["single-node-graph"]["width"] < 400, "Graph was enlarged: " + str(facts["single-node-graph"])
        shot("desktop-single-node-graph")
        print(json.dumps(facts, indent=2), flush=True)
        code = 0
    except Exception as error:
        print(json.dumps({"failure": repr(error), "facts": facts}), flush=True)
        code = 1
    os._exit(code)

threading.Thread(target=main, daemon=True).start()
Gtk.main()
