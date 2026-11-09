"""Text-only WebKitGTK checks of create/worker OS inputs and stop/report controls."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import time

from empty_desktop_audit import Audit, stop, visible_text, open_card


def run(a, answer_queue=False):
    a.start(True)
    project = a.project()
    a.add(project)
    project_route = a.js("return location.pathname;")
    a.click("Activate")
    a.wait(lambda: not visible_text(a, "Activate"), "activated")
    a.click("New task")
    a.fill("Describe the task", "Creation with native resources")
    (project / "expect-create-resources").touch()

    def window_id(process):
        rows = subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(process.pid)],
            env=a.env, capture_output=True, text=True, check=True).stdout.split()
        return rows[-1]
    def point(selector):
        return a.query("const r=document.querySelector(" + json.dumps(selector) + ").getBoundingClientRect(); return JSON.stringify([r.x+r.width/2,r.y+r.height/2]);")
    def pointer(selector):
        x, y = point(selector)
        subprocess.run(["xdotool", "mousemove", "--window", window_id(a.app), str(round(x)), str(round(y)), "click", "1"], env=a.env, check=True)
    def expect_chips(selector, number):
        a.wait(lambda: a.js("return document.querySelectorAll(" + json.dumps(selector + " .ws-att") + ").length===" + str(number) + "?'yes':'';") == "yes", "attachment-count")

    def inputs(selector, field, prefix):
        fixture = a.scratch / (prefix + ".txt")
        fixture.write_text(prefix + " fixture")
        drag_source = a.scratch / "drag_source.py"
        drag_source.write_text('''import sys,gi
gi.require_version("Gtk","3.0")
from gi.repository import Gtk,Gdk
w=Gtk.Window(title="CW native fixture")
w.set_default_size(180,100)
area=Gtk.EventBox()
area.add(Gtk.Label(label="Drag fixture"))
w.add(area)
area.drag_source_set(Gdk.ModifierType.BUTTON1_MASK,[Gtk.TargetEntry.new("text/uri-list",0,0)],Gdk.DragAction.COPY)
area.connect("drag-data-get",lambda widget,context,data,info,timestamp:data.set_uris([sys.argv[1]]))
w.show_all()
Gtk.main()
''')
        dragger = a.spawn([sys.executable, "-B", str(drag_source), fixture.as_uri()], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            sid = a.wait(lambda: subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(dragger.pid)], env=a.env, capture_output=True, text=True).stdout.strip(), "drag-source")
            sid = sid.splitlines()[-1]
            subprocess.run(["xdotool", "windowmove", sid, "1400", "80"], env=a.env, check=True)
            # Read coordinates of our exact fixture/app windows only.
            def origin(wid):
                raw = subprocess.check_output(["xdotool", "getwindowgeometry", "--shell", wid], env=a.env, text=True)
                values = dict(line.split("=",1) for line in raw.splitlines() if "=" in line)
                return int(values["X"]), int(values["Y"])
            sx, sy = origin(sid)
            ox, oy = origin(window_id(a.app))
            tx, ty = point(field)
            ax, ay, bx, by = sx+90, sy+50, ox+round(tx), oy+round(ty)
            subprocess.run(["xdotool", "mousemove", str(ax), str(ay), "mousedown", "1"], env=a.env, check=True)
            for step in range(1,21):
                subprocess.run(["xdotool", "mousemove", str(ax+(bx-ax)*step//20), str(ay+(by-ay)*step//20)], env=a.env, check=True)
                time.sleep(.04)
            time.sleep(.2)
            subprocess.run(["xdotool", "mouseup", "1"], env=a.env, check=True)
            expect_chips(selector, 1)
            a.record("native_file_drop", surface=prefix, source="external GTK text/uri-list")
        finally:
            stop(dragger)

        clipboard_script = a.scratch / "clipboard.py"
        clipboard_script.write_text('''import sys,gi
from pathlib import Path
gi.require_version("Gtk","3.0")
from gi.repository import Gtk,Gdk,GdkPixbuf
pixel=GdkPixbuf.Pixbuf.new(GdkPixbuf.Colorspace.RGB,True,8,1,1)
pixel.fill(0x506070ff)
Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD).set_image(pixel)
Path(sys.argv[1]).touch()
Gtk.main()
''')
        ready = a.scratch / (prefix + "-clipboard-ready")
        clipboard = a.spawn([sys.executable, "-B", str(clipboard_script), str(ready)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            a.wait(ready.exists, "clipboard-ready")
            pointer(field)
            subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+v"], env=a.env, check=True)
            expect_chips(selector, 2)
            a.record("native_image_paste", surface=prefix, generated=True, inspected_image=False)
        finally:
            stop(clipboard)

        picked = a.scratch / (prefix + "-picked.txt")
        picked.write_text(prefix + " picker")
        pointer(selector + ' button[aria-label="Attach files"]')
        time.sleep(.4)
        subprocess.run(["xdotool","key","--clearmodifiers","ctrl+l"],env=a.env,check=True)
        subprocess.run(["xdotool","type","--clearmodifiers","--delay","2","--",str(picked)],env=a.env,check=True)
        subprocess.run(["xdotool","key","--clearmodifiers","Return"],env=a.env,check=True)
        time.sleep(.5)
        subprocess.run(["xdotool","key","--clearmodifiers","Return"],env=a.env,check=True)
        expect_chips(selector, 3)
        a.record("native_file_picker", surface=prefix)

    inputs(".create .composer", '[aria-label="Describe the task"]', "creation")
    a.click("Save as draft")
    a.wait(lambda: visible_text(a,"Creation with native resources"), "created")
    open_card(a,"Creation with native resources")
    a.click("Start")
    a.wait(lambda: (project / "creation-resources-received").exists(), "creation-resources-delivered")
    a.wait(lambda: visible_text(a,"Which direction"), "creation-question")
    if answer_queue:
        (project / "expect-native-answer-queue").touch()
        a.click("…Other answer")
        inputs(".ws-dc-other", '[aria-label="Your answer"]', "answer")
        a.fill("Your answer","first line\nsecond line")
        a.click("Send answer")
        a.wait(lambda:(project/"native-answer-resources-received").exists(), "native-answer-bytes")
        a.wait(lambda:(project/"streaming").exists(), "answer-streaming")
        inputs(".dock .composer", '[aria-label="Message"]', "queue")
        a.fill("Message","Focus on durable conversation")
        a.click("Queue")
        a.wait(lambda:a.query("return JSON.stringify(document.querySelectorAll('.queue .ws-att').length===3);"), "native-queue-files")
        a.js("""[...document.querySelectorAll('.queue button')].find(e=>e.textContent.trim()==='Edit').click();return 'yes';""")
        a.wait(lambda:a.query("return JSON.stringify(!document.querySelector('.queue')&&document.querySelectorAll('.dock .composer .ws-att').length===3);"), "native-queue-edit-bytes")
        a.click("Steer")
        (project/"release-stream").touch()
        a.wait(lambda:(project/"native-steer-resources-received").exists(), "native-steer-bytes")
        a.wait(lambda:visible_text(a,"Runtime-owned journey result."), "native-answer-queue-result")
        a.record("native-answer-queue-delivered", answer_files=3, queue_files=3, native_os=True, text_bytes_checked=True, inspected_images=False)
        return
    before = a.js("return document.querySelector('.ws-app').dataset.detail;")
    a.keys("ctrl+backslash")
    a.wait(lambda: a.js("return document.querySelector('.ws-app').dataset.detail;") != before, "worker-pane-shortcut")
    a.keys("ctrl+backslash")
    a.record("shortcut", key="Ctrl+\\", context="task worker pane", passed=True)
    a.focus()
    a.keys("ctrl+period")
    time.sleep(1)
    a.record("stop_reply", facts=a.query("return JSON.stringify({receipt:document.querySelector(\'.ws-receipt\')?.textContent,dialogs:[...document.querySelectorAll(\'dialog[open]\')].map(e=>e.textContent),phase:document.querySelector(\'.planbar\')?.textContent});"))
    a.wait(lambda: visible_text(a,"Resume"), "paused")
    assert visible_text(a,"Preserved while paused")
    a.click("Resume")
    a.wait(lambda: not visible_text(a,"Preserved while paused"), "question-resumed")
    a.record("paused_question_preserved_and_resumed")
    a.click("Stop")
    a.wait(lambda: visible_text(a,"Resume"), "paused-again")
    a.click("Close task…")
    a.js("document.querySelector(\'dialog[open] label.rc.danger input\').click(); return \'yes\';")
    a.click("Cancel task")
    a.wait(lambda: visible_text(a,"Cancelled"), "task-cancelled")
    (project / "expect-create-resources").unlink()
    (project / "start-worker-scenario").touch()
    (project / "expect-worker-resources").touch()
    a.link(project_route)
    a.click("New task")
    a.fill("Describe the task", "Exercise native worker inputs")
    a.click("Save as draft")
    a.wait(lambda: visible_text(a,"Exercise native worker inputs"), "worker-draft")
    open_card(a,"Exercise native worker inputs")
    a.click("Start")
    time.sleep(1)
    a.record("worker_start_reply", facts=a.query("return JSON.stringify({receipt:document.querySelector(\'.ws-receipt\')?.textContent,phase:document.querySelector(\'.planbar\')?.textContent});"))
    a.wait(lambda: (project / "worker-one").exists(), "workers-started")
    a.js("const e=[...document.querySelectorAll(\'.tabs a\')].find(e=>e.textContent.trim().startsWith(\'Workers\')); if(!e)throw new Error(\'Workers tab missing\'); e.click(); return \'yes\';")
    a.wait(lambda: a.js("const e=[...document.querySelectorAll('.wcard')].find(e=>e.textContent.includes('Receive owner guidance'));if(!e)return '';e.click();return 'yes';")=="yes", "select-worker")
    a.wait(lambda: a.js("return document.querySelector('[aria-label=\"Message backend-dev\"]')?.disabled===false?'yes':'';")=="yes", "worker-composer")
    inputs(".detail .composer", '[aria-label="Message backend-dev"]', "worker")
    a.fill("Message backend-dev","Keep the worker message verbatim\nsecond line")
    a.js("""document.querySelector('[aria-label="Message backend-dev"]').focus();return 'yes';""")
    a.keys("ctrl+Return")
    a.wait(lambda: (project / "worker-resources-received").exists(), "worker-resources-delivered")
    a.wait(lambda: visible_text(a,"Open report"), "worker-finished")
    a.click("Open report")
    a.wait(lambda: a.js("return document.querySelector('dialog[open] pre')?.textContent.includes('Guidance result')?'yes':'';")=="yes", "worker-report")
    a.record("worker_report_opened", private_state_read=False)
    a.click("Close report")
    assert not (project / "worker-errors").exists()
    a.record("native_inputs_delivered", task_creation=3, direct_worker=3, engine="fake")


def main():
    p=argparse.ArgumentParser()
    p.add_argument("--evidence",required=True)
    args=p.parse_args()
    a=Audit("/",args.evidence)
    try:
        run(a)
        return 0
    finally:
        a.close()


if __name__=="__main__":
    raise SystemExit(main())
