"""Owner-scale packaged UI exercise; synthetic engines, real service, runtime and Git.

Client time advances through sixty simulated minutes; OS/service clocks do not.
Measurements are DOM/geometry evidence, not a screenshot-based visual verdict.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time

from journeys_finish_native import Finish, visible_text, open_card, stop


class Walk(Finish):
    def __init__(self, evidence):
        super().__init__("/", evidence)
        self.failures = []

    def check(self, ok, label, **facts):
        self.record(label, passed=bool(ok), **facts)
        if not ok:
            self.failures.append(label)

    def resize(self, width, height):
        window=subprocess.check_output(["xdotool","search","--onlyvisible","--pid",str(self.app.pid)],env=self.env,text=True).splitlines()[-1]
        subprocess.run(["xdotool","windowsize",window,str(width),str(height)],env=self.env,check=True)
        self.wait(lambda:self.query(f"return JSON.stringify(innerWidth==={width})"), "resize")
        time.sleep(.3)

    def measure(self, label):
        data = self.query("""
const visible=e=>e.getClientRects().length && getComputedStyle(e).visibility!=='hidden';
const rect=e=>{const r=e.getBoundingClientRect();return [r.x,r.y,r.width,r.height].map(Math.round)};
const boxes=[...document.querySelectorAll('input[type=checkbox]:not(.sw input),.cb')].filter(visible).map(e=>rect(e).slice(2));
// Diagnostics intentionally labels a connection identifier for support; it is not an unlabeled task ID.
const opaque=/\\b(?:xr|cn|wt|wst|t|pq|xa)_[a-f0-9]{8,}\\b/g;
const support=[...document.querySelectorAll('.srow')].filter(e=>e.querySelector('.d')?.textContent==='Identifier for support.').map(e=>e.innerText).join(' ');
const ids=(document.body.innerText.match(opaque)||[]).length-(support.match(opaque)||[]).length;
const zero=[...document.querySelectorAll('.badge,.count,.outc .v')].filter(visible).filter(e=>/^0$/.test(e.textContent.trim())).length;
return JSON.stringify({width:innerWidth,height:innerHeight,overflow:document.documentElement.scrollWidth-innerWidth,boxes,ids,zero,
  tiny:[...document.querySelectorAll('button')].filter(visible).filter(e=>e.getBoundingClientRect().width<12).length,
  gutters:[...document.querySelectorAll('.ws-read-b,.tl-in,.sp-scroll,.ins-scroll')].filter(visible).map(e=>{const s=getComputedStyle(e);return {name:e.className,left:s.paddingLeft,right:s.paddingRight,gutter:e.offsetWidth-e.clientWidth}}),
  panels:[...document.querySelectorAll('.sp-scroll,.inspector,.timeline,.feed')].filter(visible).map(e=>({name:e.className,client:e.clientHeight,scroll:e.scrollHeight}))});
""")
        self.check(data["overflow"] <= 1 and data["ids"] == 0 and data["zero"] == 0
                   and all(abs(w-h) <= 1 for w,h in data["boxes"]), "layout-" + label, **data)

    def stable(self, selector, label, hover=False):
        self.js("""
window.__stableRoot=document.querySelector(""" + json.dumps(selector) + """);
if(!__stableRoot)throw Error('missing stability target');
window.__rows=[...__stableRoot.querySelectorAll('.pt-row')];
window.__mutations=0;window.__kinds={};
window.__observer=new MutationObserver(ms=>{__mutations+=ms.length;ms.forEach(m=>{__kinds[m.type+':'+(m.attributeName||'')]=(__kinds[m.type+':'+(m.attributeName||'')]||0)+1})});
__observer.observe(__stableRoot,{subtree:true,childList:true,attributes:true,characterData:true});
window.__rects=__rows.map(e=>JSON.stringify([e.getBoundingClientRect().x,e.getBoundingClientRect().y,e.getBoundingClientRect().width,e.getBoundingClientRect().height]));
return 'watching';
""")
        if hover:
            self.hover(selector + " .pt-row:not(.th)")
        time.sleep(5)
        result = self.query("""
__observer.disconnect();
return JSON.stringify({mutations:__mutations,kinds:__kinds,same:__stableRoot.isConnected && __rows.every(e=>e.isConnected),
shift:__rows.filter((e,i)=>__rects[i]!==JSON.stringify([e.getBoundingClientRect().x,e.getBoundingClientRect().y,e.getBoundingClientRect().width,e.getBoundingClientRect().height])).length,
hover:!!document.querySelector('.pt-row:hover')});
""")
        self.check(result["mutations"] == 0 and result["same"] and result["shift"] == 0,
                   "stable-" + label, **result)

    def busy_click(self, label):
        result = self.query("""
const e=[...document.querySelectorAll('button')].find(e=>e.getClientRects().length && e.textContent.trim()===""" + json.dumps(label) + """);
if(!e)throw Error('missing busy target');e.click();
const busy=e.getAttribute('aria-busy')==='true';
const loader=getComputedStyle(e,'::after').content;
e.click();return JSON.stringify({busy,loader});
""")
        self.check(result["busy"], "busy-" + label, **result)
        time.sleep(.3)


def run(a):
    started = time.monotonic()
    a.start(True)
    a.measure("empty-home")
    repo = a.project()
    a.git("config", "user.name", "Audit")
    a.git("config", "user.email", "audit@example.invalid")
    # A real initialized release Store exists before the dev UI opens the project.
    sys.path.insert(0, "/checkout/src")
    from cortex_runtime.store import Store
    release = Store(repo / ".cortex", project_root=repo)
    with release.connection() as db:
        db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    release_files = {str(p.relative_to(repo)): hashlib.sha256(p.read_bytes()).hexdigest()
                     for p in (repo/".cortex").rglob("*") if p.is_file()}
    (repo / ".git/workspace-safety-fixture").touch()
    (repo / ".git/workspace-finish-fixture").touch()
    (repo / ".git/owner-walkthrough-fixture").touch()
    a.add(repo)
    a.board = a.js("return location.pathname;")
    a.click("Activate")
    a.wait(lambda:not visible_text(a, "Activate"), "activated")
    a.check((repo/".cortex-dev/cortex.sqlite3").exists(), "dev-with-release-store")
    a.click("New task")
    a.click("Sprint")
    description = "Deliver a recoverable export pipeline.\n\n" + (
        "Keep the existing project intact. Each delivery stage must explain its prerequisites, "
        "failure handling, rollback and verification to the owner.\n" * 65)
    a.fill("Describe the sprint", description)
    a.busy_click("Start planning")
    a.wait(lambda:visible_text(a,"Planning decision 1"), "three-questions", 45)
    planning = a.js("return location.pathname;")
    a.check(a.query("return JSON.stringify(document.querySelectorAll('.dc-tab').length)") == 3,
            "all-three-question-tabs")
    a.measure("long-question")
    a.link("/m/local/inbox")
    a.wait(lambda:not visible_text(a,"Nothing needs you right now"), "inbox-question-projection")
    a.check(visible_text(a,"Planning decision") or visible_text(a,"3 questions"), "question-away-inbox")
    a.link(a.board)
    a.check(a.query("return JSON.stringify(!!document.querySelector('a[href*=\"/planning\"]'))"),
            "planning-in-all-work")
    a.link(planning)
    for i in range(1,4):
        a.click(f"Question {i} of 3")
        a.focus()
        a.keys("1")
        time.sleep(.5)
    a.wait(lambda:visible_text(a, "Create 6 tasks"), "six-task-plan", 45)
    a.measure("six-long-plan")
    a.stable(".sp-plan", "idle")
    a.stable(".sp-plan", "hover", True)
    a.click("Read more about task 6")
    a.check(visible_text(a, "Preserve existing work"), "read-more-full-description")
    a.measure("read-more")
    a.resize(1280,800)
    a.measure("read-more-normal-window")
    a.resize(2000,1100)
    a.click("Close task details")
    a.busy_click("Create 6 tasks from Plan v1")
    a.wait(lambda:visible_text(a, "Start task 1"), "six-materialized", 35)
    product=a.product()[0]
    sprint=product["sprints"][0]
    ids=sprint["taskIds"]
    a.check(len(ids)==6 and all(product["tasks"][v]["dependencies"]==([ids[i-1]] if i else []) for i,v in enumerate(ids)),
            "six-dependency-readback")
    a.link(a.board)
    a.measure("all-work-six")
    a.check(a.query("return JSON.stringify([...document.querySelectorAll('.card')].every(e=>e.innerText.includes('Deliver a recoverable')))"),
            "sprint-name-on-every-card")
    first = a.query("return JSON.stringify([...document.querySelectorAll('.card-link')].map(e=>e.getAttribute('href')))")
    a.link(first[0])
    taskpath=a.js("return location.pathname;").split("/conversation")[0]
    a.busy_click("Start")
    a.wait(lambda:visible_text(a,"Workers"), "workers-running", 35)
    a.link(a.js("""return document.querySelector('.tabs a[href$="/workers"]').getAttribute('href');"""))
    a.wait(lambda:a.query("""return JSON.stringify([...document.querySelectorAll('.wcard')].some(e=>/\\/workers\\/[a-f0-9]{64}$/.test(e.getAttribute('href'))))"""), "worker-rows", 40)
    a.measure("many-workers")
    workers=a.query("return JSON.stringify([...document.querySelectorAll('.wcard')].map(e=>e.getAttribute('href')))")
    a.link(next(h for h in workers if h.rsplit("/",1)[-1] not in ("cortex","verification")))
    a.wait(lambda:visible_text(a,"Inspect recovery boundary"), "long-worker-log", 35)
    a.wait(lambda:visible_text(a,"Load older activity"), "paged-worker-history")
    a.click("Load older activity")
    a.wait(lambda:a.query("""return JSON.stringify([...document.querySelectorAll('.feed')].some(e=>e.scrollHeight>e.clientHeight+50))"""), "scrollable-long-worker-history")
    a.measure("long-worker-log")
    a.check(a.query("return JSON.stringify(document.body.innerText.includes('Inspect recovery boundary'))"), "worker-history-visible")
    (repo/".git/walkthrough-release").touch()
    a.wait(lambda:a.task_record(ids[0]).get("status")=="ready_to_merge", "eight-workers-reviewed", 90)
    a.link(a.js("""return document.querySelector('.tabs a[href$="/workers"]').getAttribute('href');"""))
    a.check(a.query("""return JSON.stringify([...document.querySelectorAll('.wcard')].filter(e=>!/(cortex|verification)$/.test(e.getAttribute('href'))).length)""") >= 8,
            "eight-finished-workers-visible")
    def open_planning():
        a.link(a.board)
        a.link(a.js("""return [...document.querySelectorAll('a')].find(e=>/\\/s\\/[^/]+\\/board$/.test(e.getAttribute('href')) && e.getClientRects().length).getAttribute('href');"""))
        a.click("Sprint planning")
    open_planning()
    a.click("Automatic")
    control=a.wait(lambda:a.query("""return JSON.stringify([...document.querySelectorAll('button')].filter(e=>e.getClientRects().length).map(e=>e.textContent.trim()).find(t=>['Run sprint','Continue queue','Pause queue'].includes(t))||'')""") or (all(a.task_record(v).get("status")=="ready_to_merge" for v in ids) and "complete"), "automatic-control")
    if control in ("Run sprint","Continue queue"):
        a.click(control)
    a.link(a.board)
    a.js("""window.__flights=0;window.__flightWatch=new MutationObserver(ms=>ms.forEach(m=>m.addedNodes.forEach(n=>{if(n instanceof Element && n.classList.contains('ws-fly'))__flights++})));__flightWatch.observe(document.body,{childList:true});return 'watching moves';""")
    a.wait(lambda:all(a.task_record(v).get("status")=="ready_to_merge" for v in ids), "six-ready", 150)
    flights=a.query("return JSON.stringify(__flights)")
    a.js("__flightWatch.disconnect();return 'finished watching';")
    a.check(flights>0, "native-board-transitions", flights=flights)
    a.check(a.git("rev-list","--count","main")=="1", "automatic-never-merges")
    a.link(a.board)
    a.wait(lambda:a.query("""return JSON.stringify(document.querySelectorAll('.card.need-you').length===6)"""), "ready-card-projection")
    a.check(a.query("""return JSON.stringify(document.querySelectorAll('.card.need-you').length===6)"""),
            "ready-cards-need-owner")
    # Sixty simulated minutes, with real polling, navigation and persistent state.
    # Only the client wall clock advances. There is no host clock mutation.
    a.js("window.__realNow=Date.now;window.__minute=0;Date.now=()=>__realNow()+__minute*60000;return 'clock ready';")
    initial=a.product()[0]["tasks"][ids[0]]
    initial_record=json.dumps(initial, sort_keys=True)
    for minute in range(1,61):
        a.js(f"window.__minute={minute};return 'advanced';")
        if minute % 10 == 0:
            open_planning()
            a.measure(f"minute-{minute}-plan")
            a.link(a.board)
            a.measure(f"minute-{minute}-board")
        time.sleep(.1)
    time.sleep(2)
    a.check(json.dumps(a.product()[0]["tasks"][ids[0]], sort_keys=True)==initial_record,
            "poll-does-not-reset-recorded-time", simulated_minutes=60)
    ages=a.query("""return JSON.stringify([...document.querySelectorAll('.ws-card-age .ws-none')].map(e=>e.textContent))""")
    a.check(len(ages)==6 and all(v=="1h" for v in ages), "visible-hour-ages", ages=ages)
    a.js("Date.now=__realNow;return 'clock restored';")
    # Owner commits a new baseline, then creates a separate task checkout.
    (repo/"a.txt").write_text("Disposable notes.\n")
    a.git("add","a.txt")
    a.git("commit","--allow-empty","-qm","Record sprint checkpoint")
    conflictpath=a.new_task("checked-change")
    conflictid=conflictpath.split("/t/")[1].split("/")[0]
    a.link(conflictpath)
    a.measure("checked-result-identifiers")
    a.check(a.query("""return JSON.stringify(!!document.querySelector('.rcard-b .ws-result-copy h3'))"""), "result-heading-not-raw-markup")
    a.stable(".rcard-b .ws-result-copy", "result-idle")
    a.click("Changes")
    a.click("Accept v1")
    a.wait(lambda:a.task_record(conflictid).get("acceptance"), "accept-first")
    a.wait(lambda:a.query("""return JSON.stringify([...document.querySelectorAll('button')].some(e=>e.textContent.trim()==='Merge…' && e.getAttribute('aria-disabled')!=='true'))"""), "accepted-ui")
    (repo/"a.txt").write_text("owner\n")
    a.git("add","a.txt")
    a.git("commit","-qm","Owner concurrent change")
    head=a.git("rev-parse","main")
    a.click("Merge…")
    a.check(not visible_text(a,"with open checks"), "no-false-open-check-warning")
    a.click("Confirm merge into main")
    a.wait(lambda:a.task_record(conflictid).get("conflict")=="conflict", "real-conflict")
    a.wait(lambda:a.query("""return JSON.stringify(!!document.querySelector('a[href$="/changes/conflict"]'))"""), "conflict-link")
    a.link(conflictpath+"/changes/conflict")
    a.wait(lambda:visible_text(a,"Resolve it yourself"), "conflict-page")
    a.measure("real-conflict")
    a.click("Accept ours")
    a.click("Finish merge")
    a.wait(lambda:not a.task_record(conflictid).get("conflict")=="conflict", "conflict-finished", 35)
    a.check(a.git("rev-parse","main")==head, "conflict-preserves-target")
    source=Path(a.task_record(conflictid)["checkout"]["path"])
    a.check(not a.git("diff","--name-only","--diff-filter=U",repo=source)
            and (source/"a.txt").read_text()=="task\n" and (repo/"a.txt").read_text()=="owner\n",
            "conflict-index-and-file-readback")
    a.check(all((repo/p).exists() and hashlib.sha256((repo/p).read_bytes()).hexdigest()==h for p,h in release_files.items()),
            "release-store-byte-identical", files=len(release_files))
    a.restart_window()
    a.measure("restart-restored")
    a.record("walkthrough-summary", failures=a.failures, simulated_minutes=60,
             actual_seconds=round(time.monotonic()-started), screenshots=0, paid_calls=0,
             binary_sha256=hashlib.sha256(Path("/usr/bin/cortex-workspace-dev").read_bytes()).hexdigest())
    return not a.failures


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("--evidence",required=True)
    args=p.parse_args()
    a=Walk(args.evidence)
    try:
        return 0 if run(a) else 1
    except Exception as error:
        a.snapshot("incomplete-state")
        a.record("walkthrough-incomplete", error=type(error).__name__, reason=str(error)[:200])
        raise
    finally:
        a.close()


if __name__=="__main__":
    raise SystemExit(main())
