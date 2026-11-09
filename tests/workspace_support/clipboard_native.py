"""Physical text drag and Ctrl+C in the packaged Tauri window; no images."""
import argparse
import json
import subprocess
import sys
import time
from owner_walkthrough import Walk
from journeys_finish_native import visible_text


def copy(a, label):
    a.js("@focus")
    target = a.query("""
const candidates=[...document.querySelectorAll(document.querySelector('.ws-result-copy') ? '.ws-result-copy *' : 'main *, .set-body *')].filter(e=>{
 if(e.closest('button,a,input,textarea,.titlebar,.sidebar'))return false;
 const range=document.createRange();range.selectNodeContents(e);
 const r=range.getBoundingClientRect(),s=getComputedStyle(e);
 return e.children.length===0 && e.textContent.trim().length>=8 && e.textContent.trim().length<=80
  && e.textContent===e.textContent.trim() && range.getClientRects().length===1 && r.width>30 && r.width<550
  && r.height>8 && r.height<30 && r.x>0 && r.y>40 && r.right<innerWidth && r.bottom<innerHeight
  && s.webkitUserSelect==='text' && s.visibility==='visible';
});
const e=candidates[0];if(!e)throw Error('no selectable one-line text');
const range=document.createRange();range.selectNodeContents(e);const r=range.getBoundingClientRect();
return JSON.stringify({text:e.textContent,rect:[r.x,r.y,r.width,r.height]});
""")
    window = subprocess.check_output(["xdotool","search","--onlyvisible","--pid",str(a.app.pid)],env=a.env,text=True).splitlines()[-1]
    x,y,w,h=target["rect"]
    subprocess.run(["xdotool","windowactivate","--sync",window,"mousemove","--window",window,str(round(x)),str(round(y+h/2)),
                    "mousedown","1","mousemove","--window",window,str(round(x+w)),str(round(y+h/2)),"mouseup","1"],
                   env=a.env,check=True)
    time.sleep(.2)
    selected=a.js("return window.getSelection().toString();")
    a.check(selected==target["text"], "native-select-"+label, characters=len(selected))
    a.keys("ctrl+c")
    result=subprocess.run([sys.executable,"-B","-c",
        "import gi; gi.require_version('Gtk','3.0'); from gi.repository import Gtk,Gdk; import json; print(json.dumps(Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD).wait_for_text()))"],
        env=a.env,capture_output=True,text=True,timeout=8,check=True)
    a.check(json.loads(result.stdout)==target["text"], "native-clipboard-"+label, characters=len(target["text"]))


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("--evidence",required=True)
    a=Walk(p.parse_args().evidence)
    try:
        a.start(True)
        copy(a,"empty-home")
        a.fresh_project()
        path=a.new_task("checked-change")
        a.wait(lambda:visible_text(a,"Result v1"),"result")
        copy(a,"task-result")
        a.click("Settings")
        a.link("/settings/machine/local/worktrees")
        copy(a,"settings")
        a.record("clipboard-summary",failures=a.failures,screenshots=0,paid_calls=0)
        return 1 if a.failures else 0
    finally:
        a.close()


if __name__=="__main__":
    raise SystemExit(main())
