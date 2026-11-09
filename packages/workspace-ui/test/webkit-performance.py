"""Opt-in static-bundle WebKitGTK measurements; fake data, private virtual screen."""
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import time

xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "1920x1200x24",
                         "-nolisten", "tcp"], stdout=subprocess.PIPE, text=True)
os.environ.update(DISPLAY=":" + xvfb.stdout.readline().strip(), GDK_BACKEND="x11",
                  NO_AT_BRIDGE="1", WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS="1",
                  WEBKIT_DISABLE_COMPOSITING_MODE="1")
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
        view.evaluate_javascript("JSON.stringify((()=>{" + code + "})()??null)",
                                 -1, None, None, None, finish, None)
    return main(call)


def wait(code):
    for _ in range(150):
        if js("return !!(" + code + ")"):
            return
        time.sleep(.1)
    raise TimeoutError(code)


def load(query=""):
    main(lambda b, d: (view.load_uri(sys.argv[1] + query), d.set()))
    wait("window.fixture && document.querySelector('.titlebar')")


def capture(name):
    # Capture the private native window; no optional cairo foreign converter needed.
    assert hasattr(Gdk, "pixbuf_get_from_window"), "GDK capture API unavailable"
    def call(box, done):
        width, height = win.get_size()
        pixels = Gdk.pixbuf_get_from_window(win.get_window(), 0, 0, width, height)
        assert pixels is not None, "Native window capture unavailable"
        pixels.savev(str(evidence / (label + "-" + name + ".png")), "png", [], [])
        done.set()
    main(call)


def bench(selector):
    js("""const el=document.querySelector(""" + json.dumps(selector) + """);
      window.stats=null; const intervals=[],costs=[];let mutations=0,frames=0,last=performance.now();
      const observer=new MutationObserver(r=>mutations+=r.length);observer.observe(el,{subtree:true,childList:true,characterData:true,attributes:true});
      const reverse=getComputedStyle(el).flexDirection==='column-reverse';
      const tick=now=>{intervals.push(now-last);last=now;
        if(frames===30 || frames===90) fixture.emit(true);
        const start=performance.now();el.scrollTop=(reverse?-1:1)*((frames/119)*(el.scrollHeight-el.clientHeight));
        el.getBoundingClientRect();costs.push(performance.now()-start);
        if(++frames<120)requestAnimationFrame(tick);else{observer.disconnect();const sorted=[...intervals].sort((a,b)=>a-b);
          window.stats={frames,slowFrames:intervals.flatMap((ms,frame)=>ms>50?[{frame,ms}]:[]),p50:sorted[Math.floor(sorted.length*.5)],p95:sorted[Math.floor(sorted.length*.95)],max:Math.max(...sorted),over50:sorted.filter(n=>n>50).length,scrollCostMax:Math.max(...costs),mutations,nodes:el.querySelectorAll('*').length,height:el.scrollHeight};}};
      requestAnimationFrame(tick);""")
    wait("window.stats")
    result = js("return window.stats")
    assert result["frames"] == 120 and result["height"] > 10000 and result["nodes"] > 1000
    return result


def report_at(width, height):
    main(lambda b, d: (win.resize(width, height), d.set()))
    wait("innerWidth===" + str(width) + " && innerHeight===" + str(height))
    js("document.querySelector('dialog[open] .dlg-b').scrollTop=0")
    time.sleep(.1)
    capture("report-" + str(width))
    geometry = js("""const b=document.querySelector('dialog[open] .dlg-b');
      const copy=b.querySelector('.ws-result-copy'), r=copy.getBoundingClientRect();
      return {width:innerWidth,height:innerHeight,pageOverflow:document.documentElement.scrollWidth-innerWidth,
        reportOverflow:b.scrollWidth-b.clientWidth,copyWidth:r.width,bodyWidth:b.clientWidth,
        font:getComputedStyle(copy).fontFamily,lineHeight:getComputedStyle(copy).lineHeight};""")
    assert geometry["reportOverflow"] <= 1, geometry
    js("const b=document.querySelector('dialog[open] .dlg-b'); b.scrollTop=b.scrollHeight")
    time.sleep(.1)
    capture("report-end-" + str(width))
    return geometry


def run():
    global exit_code
    try:
        results = {"engine": "WebKitGTK", "version": [WebKit2.get_major_version(),
                   WebKit2.get_minor_version(), WebKit2.get_micro_version()],
                   "sources": json.loads(sys.argv[3]), "label": label}
        load()
        wait("document.querySelector('.ws-worker-result button')")
        results["worker"] = bench(".stream")
        capture("worker-1440")
        js("fixture.openedAt=performance.now(); document.querySelector('.ws-worker-result button').click(); const painted=()=>{if(document.querySelector('dialog[open] .ws-result-copy')) fixture.openMs=performance.now()-fixture.openedAt; else requestAnimationFrame(painted)}; requestAnimationFrame(painted)")
        wait("document.querySelector('dialog[open] .ws-result-copy') && fixture.openMs")
        results["openMs"] = js("return fixture.openMs")
        results["report"] = bench("dialog[open] .dlg-b")
        results["reportShape"] = js("const d=document.querySelector('dialog[open]');return {characters:fixture.characters,reads:fixture.reads.length,headings:d.querySelectorAll('h1,h2,h3').length,tables:d.querySelectorAll('table').length,lists:d.querySelectorAll('ol').length,codes:d.querySelectorAll('pre').length,last:d.textContent.includes('Final report paragraph.')}")
        shape = results["reportShape"]
        assert shape["tables"] == shape["lists"] == shape["codes"] == 1800 and shape["last"]
        assert js("return getComputedStyle(document.querySelector('dialog[open] ol')).listStyleType") == "decimal"
        assert js("return getComputedStyle(document.querySelector('.ev-note ul')).listStyleType") == "disc"
        assert js("return getComputedStyle(document.querySelector('dialog[open] .ws-result-copy h1')).marginTop") == "8px"
        assert shape["reads"] > 1 and not js("return document.querySelector('dialog[open]').textContent.includes('Next page')")
        results["viewports"] = [report_at(w, h) for w, h in [(1440, 1000), (900, 1000), (390, 844)]]
        main(lambda b, d: (win.resize(1440, 1000), d.set()))
        wait("innerWidth===1440")
        load("?view=conversation")
        wait("document.querySelectorAll('.timeline .ai').length===2400")
        results["conversation"] = bench(".timeline")
        capture("conversation-1440")
        # A same-content read must retain the node and the reader's place.
        results["stableRefresh"] = js("const box=document.querySelector('.timeline'), row=box.querySelector('.ai'), at=box.scrollTop; fixture.emit(); return row===box.querySelector('.ai') && box.scrollTop===at")
        assert results["stableRefresh"]
        (evidence / (label + "-metrics.json")).write_text(json.dumps(results, indent=2) + "\n")
        print(json.dumps(results), flush=True)
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
