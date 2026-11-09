"""Real WebKitGTK startup/streaming regression; only disposable content and Xvfb."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import threading
import time

xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "1920x1200x24", "-nolisten", "tcp"], stdout=subprocess.PIPE, text=True)
os.environ.update(DISPLAY=":" + xvfb.stdout.readline().strip(), GDK_BACKEND="x11", NO_AT_BRIDGE="1",
    WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS="1", WEBKIT_DISABLE_COMPOSITING_MODE="1")
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
    for _ in range(180):
        if js("return !!(" + code + ")"):
            return
        time.sleep(.1)
    raise TimeoutError(code)

def load(name, width=1440, height=1000, length=240):
    main(lambda b,d: (win.resize(width,height),view.load_uri(sys.argv[1]+"?view="+name+"&length="+str(length)),d.set()))
    wait("window.fixture && document.querySelector('.titlebar')")
    time.sleep(.4)

def capture(name):
    time.sleep(.15)
    def take(box, done):
        width,height = win.get_size()
        pixel = Gdk.pixbuf_get_from_window(win.get_window(),0,0,width,height)
        assert pixel is not None, "native capture unavailable"
        pixel.savev(str(evidence / (name+".png")),"png",[],[])
        done.set()
    main(take)

def measure(selector):
    return js("const b=document.querySelector("+json.dumps(selector)+");return {top:b.scrollTop,height:b.scrollHeight,client:b.clientHeight,bottom:(getComputedStyle(b).display==='flex'&&getComputedStyle(b).flexDirection==='column-reverse')?Math.abs(b.scrollTop):Math.abs(b.scrollHeight-b.clientHeight-b.scrollTop),following:document.body.dataset.follow};")

def stream_test(name):
    load(name)
    selector = ".stream" if name=="worker" else ".timeline"
    wait("document.querySelector("+json.dumps(selector)+")")
    time.sleep(.2)
    js("window.b=document.querySelector("+json.dumps(selector)+");window.reverse=(getComputedStyle(b).display==='flex'&&getComputedStyle(b).flexDirection==='column-reverse');")
    results = {"initial":measure(selector)}
    js("fixture.stream();fixture.append();")
    time.sleep(.2)
    results["pinned"] = measure(selector)
    js("b.scrollTop=reverse?-550:b.scrollHeight-b.clientHeight-550;")
    time.sleep(.15)
    js("const y=b.getBoundingClientRect().top;window.anchor=[...b.firstElementChild.children].find(r=>r.getBoundingClientRect().bottom>y+80);window.anchorY=anchor.getBoundingClientRect().top;window.readTop=b.scrollTop;fixture.stream();fixture.append();")
    time.sleep(.2)
    results["reading"] = js("return {topDelta:b.scrollTop-readTop,anchorDelta:anchor.getBoundingClientRect().top-anchorY};")
    js("window.anchorY=anchor.getBoundingClientRect().top;fixture.expand();")
    time.sleep(.2)
    results["expand"] = js("return {anchorDelta:anchor.getBoundingClientRect().top-anchorY};")
    capture(name+"-reading-1440")
    js("b.scrollTop=reverse?0:b.scrollHeight-b.clientHeight;")
    time.sleep(.1)
    js("fixture.stream();")
    time.sleep(.2)
    results["resumed"] = measure(selector)
    # Real XTEST input grabs the scrollbar, rather than calling its handlers.
    assert shutil.which("xdotool"), "XTEST pointer driver unavailable"
    xy = js("const r=b.getBoundingClientRect();return [Math.round(r.right-5),Math.round(r.bottom-80)];")
    subprocess.run(["xdotool","mousemove",str(xy[0]),str(xy[1]),"mousedown","1"],check=True)
    time.sleep(.1)
    drag_top = js("return b.scrollTop;")
    js("fixture.stream();fixture.append();")
    time.sleep(.15)
    results["dragAppendDelta"] = js("return b.scrollTop;") - drag_top
    subprocess.run(["xdotool","mousemove",str(xy[0]),str(xy[1]-120)],check=True)
    time.sleep(.15)
    moved = js("return b.scrollTop;")
    subprocess.run(["xdotool","mouseup","1"],check=True)
    results["dragMoved"] = abs(moved-drag_top)
    # Frame intervals while content grows; no timing claim inferred from exit code.
    js("window.frames=null;const intervals=[];let i=0,last=performance.now();const tick=t=>{intervals.push(t-last);last=t;if(i%20===0)fixture.stream();if(++i<120)requestAnimationFrame(tick);else frames=intervals;};requestAnimationFrame(tick);")
    wait("Array.isArray(window.frames)")
    values = sorted(js("return frames;"))
    results["frames"] = {"p95":values[int(len(values)*.95)],"max":max(values),"count":len(values)}
    capture(name+"-bottom-1440")
    if label != "before":
        assert results["pinned"]["bottom"] <= 1, results
        assert abs(results["reading"]["topDelta"]) <= 1 and abs(results["reading"]["anchorDelta"]) <= 1, results
        assert abs(results["expand"]["anchorDelta"]) <= 1, results
        assert results["resumed"]["bottom"] <= 1, results
        assert abs(results["dragAppendDelta"]) <= 1 and results["dragMoved"] > 5, results
    return results

def run():
    global exit_code
    results = {"engine":"WebKitGTK","version":[WebKit2.get_major_version(),WebKit2.get_minor_version(),WebKit2.get_micro_version()],"label":label}
    try:
        load("startup")
        capture("startup-pending-1440")
        wait("fixture.done?.()")
        results["startup"] = js("return {samples:fixture.samples,taskCount:fixture.info.taskCount,errors:fixture.errors??[]};")
        assert any(s["cards"] for s in results["startup"]["samples"]), results["startup"]["errors"]
        capture("startup-ready-1440")
        if label != "before":
            # Planning and archived tasks must not appear even for one observed frame.
            for sample in results["startup"]["samples"]:
                assert not any("Planning fixture" in c or "Archived fixture" in c for c in sample["cards"]), sample
        results["startup"]["firstCardsMs"] = next((s["ms"] for s in results["startup"]["samples"] if s["cards"]),None)
        samples=results["startup"].pop("samples")
        (evidence/"startup-samples.json").write_text(json.dumps(samples))
        results["startup"]["frames"] = len(samples)
        results["startup"]["wrongFrames"] = sum(any("Planning fixture" in c or "Archived fixture" in c for c in s["cards"]) for s in samples)
        js("fixture.refresh();")
        wait("fixture.done?.()")
        results["refresh"] = js("return {frames:fixture.samples.length,wrongFrames:fixture.samples.filter(s=>s.cards.some(c=>/Planning fixture|Archived fixture/.test(c))).length,emptyFrames:fixture.samples.filter(s=>!s.cards.length).length};")
        if label != "before":
            assert results["refresh"]["wrongFrames"] == results["refresh"]["emptyFrames"] == 0, results["refresh"]
        if label != "before":
            js("fixture.failRefresh();")
            wait("document.querySelector('[role=alert]')?.textContent.includes('Workspace refresh failed')")
            capture("refresh-error-1440")
            results["refreshError"]=js("return {cards:document.querySelectorAll('.card').length,connected:document.querySelector('.machine .st')?.textContent};")
            assert results["refreshError"] == {"cards":3,"connected":"Connected"}, results["refreshError"]
            js("fixture.recover();")
            wait("!document.querySelector('[role=alert]')")
            load("startup-error")
            wait("document.querySelector('[role=status]')?.textContent.includes('could not be loaded')")
            capture("startup-error-1440")
            js("fixture.recover();")
            wait("document.querySelectorAll('.card').length===3")
            results["retryRecovered"] = True
        results["worker"] = stream_test("worker")
        results["conversation"] = stream_test("conversation")
        for width,height in ((900,1000),(390,844)):
            load("conversation",width,height)
            capture("conversation-"+str(width))
            results["viewport"+str(width)] = js("return {width:innerWidth,height:innerHeight,overflow:document.documentElement.scrollWidth-innerWidth};")
        for name in ("tools","conversation-tools"):
            load(name)
            wait("document.querySelector('button.fold')")
            capture(name+"-collapsed-1440")
            js("document.querySelector('button.fold').click();")
            time.sleep(.2)
            capture(name+"-open-1440")
            rows=js("return [...document.querySelectorAll('.step-row')].map(e=>e.textContent);")
            assert len(rows)==4 and not any("mcp__" in r or "Read skill" in r or "Submit result" in r for r in rows), rows
            results[name]={"rows":rows}
        load("conversation",1440,1000,2400)
        wait("document.querySelectorAll('.timeline .ai').length===2400")
        js("window.frames=null;const b=document.querySelector('.timeline'),intervals=[];let i=0,last=performance.now();const reverse=getComputedStyle(b).display==='flex';const tick=t=>{intervals.push(t-last);last=t;b.scrollTop=(reverse?-1:1)*((++i*591)%(b.scrollHeight-b.clientHeight));if(i<150)requestAnimationFrame(tick);else frames=intervals;};requestAnimationFrame(tick);")
        wait("Array.isArray(window.frames)")
        values=sorted(js("return frames;"))
        results["longHistory"]={"entries":2400,"p95":values[int(len(values)*.95)],"max":max(values),"frames":len(values)}
        capture("conversation-long-1440")
    except Exception as error:
        exit_code=1
        results["error"]=type(error).__name__+": "+str(error)
    finally:
        if "samples" in results.get("startup", {}):
            samples=results["startup"].pop("samples")
            (evidence/"startup-samples.json").write_text(json.dumps(samples))
            results["startup"].update(frames=len(samples),wrongFrames=sum(any("Planning fixture" in c or "Archived fixture" in c for c in s["cards"]) for s in samples))
        (evidence/"metrics.json").write_text(json.dumps(results,indent=2)+"\n")
        print(json.dumps(results),flush=True)
        GLib.idle_add(Gtk.main_quit)

threading.Thread(target=run,daemon=True).start()
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
