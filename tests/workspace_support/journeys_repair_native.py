"""Rendered controls on the packaged binary. No product rows or page APIs are injected."""
import argparse
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import sys
import time

from empty_desktop_audit import Audit, stop, visible_text, open_card, savebar, ROOT


def visible_text(a, needle):
    return a.query("return JSON.stringify(document.body.innerText.replace(/\\s+/g,' ').toLowerCase().includes(" + json.dumps(needle.lower()) + "));")


class Native(Audit):
    def start_service(self, fake=False):
        extra = ["--fixture-launcher", str(ROOT / "tests/test_workspace_service_integration.py")] if fake else []
        self.service = self.spawn([str(self.bin / "cortex-workspace-service-dev"), str(self.scratch),
                                   sys.executable, str(ROOT if fake else self.runtime), *extra],
                                  stdout=subprocess.DEVNULL, stderr=(self.scratch / "service.stderr").open("ab"))
        self.wait(lambda:(self.scratch / "runtime/cortex-workspace-dev/service.sock").exists(), "service")

    def must(self, condition, label, **facts):
        self.record(label, passed=bool(condition), **facts)
        if not condition:
            self.snapshot(label + "-failed")
            raise AssertionError(label)

    def git(self, *args, repo=None):
        return subprocess.check_output(["git", "-c", "user.name=Audit", "-c", "user.email=audit@example.invalid",
                                        *args], cwd=repo or self.repo, env=self.env, stderr=subprocess.DEVNULL, text=True).strip()

    def product(self):
        files = list((self.scratch / "state/cortex-workspace-dev").glob("product/*.json"))
        return [json.loads(p.read_text()) for p in files]

    def launch_window(self):
        old = dict(self.env)
        self.env.update(LD_PRELOAD=str(self.evidence / "probe.so"), CW_PROBE_DIR=str(self.scratch / "probe"))
        self.app = self.spawn([str(self.bin / "cortex-workspace-dev")], stdout=subprocess.DEVNULL,
                              stderr=(self.scratch / "app.stderr").open("ab"), cwd=self.scratch / "home")
        self.env = old
        self.wait(lambda:self.js("return !!document.querySelector('.titlebar')?'yes':''") == "yes", "reopen", 40)

    def style(self, label):
        data = self.query("""
const visible=e=>e.getClientRects().length && getComputedStyle(e).visibility!=='hidden';
const fields=['outlineStyle','boxShadow','borderTopColor','borderTopWidth','backgroundColor','fontWeight'];
const look=e=>Object.fromEntries(fields.map(k=>[k,getComputedStyle(e)[k]]));
window.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true}));
const rows=[...document.querySelectorAll('button,a,input,textarea,[role=tab]')].filter(visible).map(e=>{
 const before=look(e);e.focus({preventScroll:true});const after=look(e);
 return {name:e.getAttribute('aria-label')||e.textContent.trim().slice(0,60),before,after};
});
return JSON.stringify({theme:document.documentElement.dataset.theme,rows});
""")
        failures = [r for r in data["rows"] if r["before"] != r["after"] or r["after"]["outlineStyle"] != "none"]
        (self.evidence / (label + ".json")).write_text(json.dumps(data, indent=2))
        self.must(not failures, label, controls=len(data["rows"]), failures=failures)

    def fresh_project(self, fake=False):
        p = self.project()
        self.git("config", "user.name", "Audit")
        self.git("config", "user.email", "audit@example.invalid")
        if fake:
            (p / ".git/workspace-safety-fixture").touch()
        self.add(p)
        self.board = self.js("return location.pathname;")
        self.click("Activate")
        self.wait(lambda:not visible_text(self, "Activate"), "activation")
        return p

    def new_task(self, goal, worktree=True):
        self.link(self.board)
        self.click("New task")
        self.fill("Describe the task", goal)
        if worktree:
            self.js("document.querySelector('input[name=checkout]').click();return 'yes';")
        self.click("Save as draft")
        self.wait(lambda:self.query("return JSON.stringify(!!document.querySelector('.card-link'));"), "draft")
        open_card(self, goal[0].upper() + goal[1:])
        path = self.js("return location.pathname;")
        self.click("Start")
        self.wait(lambda:visible_text(self, "Result v1") or visible_text(self, "Required failed check"), "checked-result", 40)
        self.snapshot("result-" + goal)
        return path


def settings(a):
    a.start(False)
    # Apply preset before activation, rather than manufacturing a missing product row.
    a.click("Settings")
    a.link("/settings/machine/local/orchestration/files")
    before = a.snapshot("files-before-activation")
    if any(c["name"] == "Apply preset" and not c["disabled"] for c in before["controls"]):
        a.click("Apply preset")
        a.wait(lambda:visible_text(a, "Every file exists"), "applied-preset")
        a.record("preset-applied", invoked=True)
    else:
        a.record("preset-apply-disabled", reason="Every file exists")
    a.click("Back to workspace")
    a.fresh_project(False)
    a.click("Settings")
    a.link("/settings/machine/local/overview")
    a.fill("Machine name", "Repair machine")
    a.wait(lambda:visible_text(a, "Repair machine"), "rename")
    a.snapshot("machine-renamed")
    a.link("/settings/machine/local/orchestration/defaults")
    a.click("English")
    savebar(a)
    a.must(not visible_text(a, "unsaved change"), "defaults-saved")
    a.link("/settings/machine/local/orchestration/profiles")
    a.click("Save as new…")
    a.fill("Profile name", "repair-profile")
    a.click("Save as new")
    a.wait(lambda:visible_text(a, "repair-profile"), "profile")
    a.snapshot("profile-created")
    a.link("/settings/machine/local/orchestration/files")
    a.click("Reset to default…")
    a.snapshot("reset-confirmation")
    a.click("Reset")
    a.wait(lambda:not a.query("return JSON.stringify(!!document.querySelector('dialog[open]'));"), "reset-done")
    a.snapshot("preset-reset")
    roots = list((a.scratch / "config").rglob("defaults.json"))
    a.must(len(roots) == 1 and json.loads(roots[0].read_text()).get("language") != "en", "reset-readback", defaults_count=len(roots))
    a.link("/settings/app/updates")
    a.wait(lambda:visible_text(a, "local builds"), "dev-update")
    a.must(not visible_text(a, "Check for updates"), "dev-update-feed-absent")
    a.snapshot("updates-dev")
    a.link("/settings/app/voice")
    a.wait(lambda:visible_text(a, "Russian"), "voice-settings")
    a.snapshot("voice-settings")
    a.link("/settings/machine/local/connections")
    a.click("Add connection…")
    a.snapshot("add-connection")
    a.click("Cancel")
    a.click("Back to workspace")
    a.link(a.board)
    a.click("Home")
    a.style("home-pointer-focus-dark")
    a.click("Settings")
    a.link("/settings/app/appearance")
    a.click("Light")
    a.style("appearance-pointer-focus-light")
    a.click("Back to workspace")
    a.link(a.board)
    a.style("project-pointer-focus-light")
    # State and colour from the actual sidebar toggle, then native shortcut.
    def panel():
        return a.query("""const e=document.querySelector('.titlebar .panel-t');return JSON.stringify({state:e.getAttribute('aria-pressed'),color:getComputedStyle(e).color,side:document.querySelector('.ws-app').dataset.sidebar});""")
    before = panel()
    a.click("Sidebar")
    after = panel()
    a.must(before["side"] != after["side"] and before["color"] != after["color"], "sidebar-click-state-colour", before=before, after=after)
    a.js("@focus")
    a.keys("ctrl+b")
    a.must(panel()["side"] == before["side"], "sidebar-shortcut")
    for key, needle in (("ctrl+k","Search commands"), ("ctrl+n","Describe the task")):
        a.keys(key)
        a.snapshot("shortcut-" + key.replace("+","-"))
        if key == "ctrl+k":
            a.keys("Escape")
        else:
            a.must(a.query("""return JSON.stringify(!!document.querySelector('[aria-label="Describe the task"]'));"""), "new-task-shortcut")
            a.link(a.board)
    a.keys("ctrl+i")
    a.must("/inbox" in a.js("return location.pathname;"), "inbox-shortcut")
    a.keys("ctrl+comma")
    a.must("/settings/" in a.js("return location.pathname;"), "settings-shortcut")


def clone(a):
    a.start(False)
    p = a.project()
    bare = a.scratch / "source.git"
    subprocess.run(["git", "clone", "--bare", str(p), str(bare)], env=a.env, check=True, capture_output=True)
    # The actual owner UI submits the local bare source to the production helper.
    a.click("Add project")
    a.option("Clone from GitHub")
    a.fill("GitHub repository URL", str(bare))
    target = a.scratch / "home/clone"
    a.fill("Clone destination", str(target))
    a.click("Clone repository")
    a.wait(lambda:visible_text(a, "Clone complete."), "clone", 30)
    a.must(a.git("rev-parse","HEAD",repo=target) == a.git("rev-parse","HEAD",repo=p), "clone-git-readback")
    a.click("Add cloned project")
    a.wait(lambda:"/p/" in a.js("return location.pathname;"), "clone-registered")
    a.snapshot("cloned-project")
    # A second project is registered through the same real UI.
    a.click("Home")
    a.add(p)
    a.click("Home")
    a.snapshot("multi-project-home")
    a.must(visible_text(a, "owner-repo") and visible_text(a, "clone"), "home-aggregation")
    old = a.app
    os.killpg(old.pid, signal.SIGKILL)
    old.wait(timeout=5)
    a.launch_window()
    a.wait(lambda:visible_text(a, "owner-repo") and visible_text(a, "clone"), "crash-recovery")
    a.must(a.service.poll() is None, "service-survives-window-crash")
    a.click("Close")
    a.wait(lambda:a.app.poll() is not None, "window-close", 10)
    a.launch_window()
    a.wait(lambda:visible_text(a, "owner-repo") and visible_text(a, "clone"), "close-recovery")
    a.record("window-close-recovery", passed=True)


def checked(a, target="main"):
    a.start(True)
    a.fresh_project(True)
    if target != "main":
        a.git("branch", target)
    path = a.new_task("checked-change")
    a.click("Changes")
    a.wait(lambda:visible_text(a, "Ready to merge"), "ready", 30)
    a.snapshot("ready")
    task = path.split("/t/")[1].split("/")[0]
    # Read-only Git/store evidence, never writes product rows.
    trees = a.git("worktree", "list", "--porcelain")
    a.must("cw/" + task in trees, "separate-worktree-created", task=task)
    a.click("Accept v1")
    a.wait(lambda:a.query("""return JSON.stringify([...document.querySelectorAll('button')].some(e=>e.textContent.trim()==='Merge…'&&e.getAttribute('aria-disabled')!=='true'));"""), "accepted")
    a.snapshot("accepted")
    a.click("Merge…")
    a.snapshot("merge-dialog")
    if target != "main":
        a.js("""document.querySelector('dialog[open] [popovertarget="m-merge-target"]').click();return 'yes';""")
        a.click("Other branch…")
        a.fill("Other target branch", target)
        a.keys("Return")
        a.wait(lambda:visible_text(a, "Confirm merge into " + target), "target-changed")
    a.click("Confirm merge into " + target)
    a.wait(lambda:visible_text(a,"Merged into " + target),"merged",25)
    a.must(a.git("show", target + ":a.txt") == "task", "merge-file-readback")
    a.must(a.git("rev-list","--count",target) == "2", "squash-commit-readback")
    a.must("cw/" + task not in a.git("worktree","list","--porcelain"), "separate-worktree-cleaned")
    a.record("checked-merge", passed=True, task=task, target=target, head=a.git("rev-parse",target))


def branch(a):
    checked(a, "release/repair")


def conflict(a, how="manual"):
    a.start(True)
    a.fresh_project(True)
    path = a.new_task("checked-change")
    task = path.split("/t/")[1].split("/")[0]
    source = next(Path(line[9:]) for line in a.git("worktree","list","--porcelain").splitlines()
                  if line.startswith("worktree ") and line[9:] != str(a.repo))
    a.click("Changes")
    a.click("Accept v1")
    a.wait(lambda:a.query("""return JSON.stringify([...document.querySelectorAll('button')].some(e=>e.textContent.trim()==='Merge…'&&e.getAttribute('aria-disabled')!=='true'));"""), "accepted")
    (a.repo / "a.txt").write_text("owner\n")
    a.git("add","a.txt")
    a.git("commit","-qm","Owner concurrent change")
    target_head = a.git("rev-parse","main")
    a.click("Merge…")
    a.snapshot("conflict-merge-dialog")
    a.click("Confirm merge into main")
    a.wait(lambda:visible_text(a,"conflict"), "real-conflict", 20)
    a.link(path + "/changes/conflict")
    a.wait(lambda:visible_text(a,"Resolve it yourself"), "conflict-screen")
    a.snapshot("actual-conflict")
    a.must(a.git("diff","--name-only","--diff-filter=U",repo=source) == "a.txt", "git-conflict-readback")
    if how == "abort":
        a.click("Abort merge")
        a.wait(lambda:visible_text(a,"No merge in progress"), "aborted")
        a.must(a.git("rev-parse","main") == target_head and (source/"a.txt").read_text() == "task\n", "abort-readback")
    elif how == "cortex":
        a.click("Resolve automatically…")
        a.click("Resolve automatically")
        a.wait(lambda:(source/"a.txt").read_text() == "resolved\n", "cortex-file-resolution", 35)
        a.must(a.git("rev-parse","main") == target_head, "cortex-keeps-target")
        a.wait(lambda:visible_text(a, "Result v2") or visible_text(a, "Cortex resolved the last block"), "cortex-rechecked", 35)
        a.snapshot("cortex-resolved")
    else:
        a.click("Accept ours")
        a.click("Finish merge")
        a.wait(lambda:not a.git("diff","--name-only","--diff-filter=U",repo=source), "manual-staged")
        a.must((source/"a.txt").read_text() == "task\n" and a.git("rev-parse","main") == target_head, "manual-resolution-readback")
        a.wait(lambda:not Path(a.git("rev-parse", "--git-path", "MERGE_HEAD", repo=source)).exists(), "merge-finished", 10)
        a.snapshot("manually-resolved")


def abort(a):
    conflict(a, "abort")


def cortex(a):
    conflict(a, "cortex")


def checks(a):
    a.start(True)
    a.fresh_project(True)
    path = a.new_task("technical-check")
    a.wait(lambda:visible_text(a, "Required skipped check"), "technical-check")
    a.must(visible_text(a, "Technical blocker:"), "technical-blocker-visible")
    task = path.split("/t/")[1].split("/")[0]
    def owner_checks():
        with sqlite3.connect(a.repo/".cortex-dev/cortex.sqlite3") as db:
            rows = db.execute("SELECT id,payload_json FROM runtime_events WHERE task_id=(SELECT runtime_task_id FROM workspace_executions WHERE task_id=?) AND type='WORKER_CHECK_FINISHED'",(task,)).fetchall()
        return [(key, json.loads(payload)) for key, payload in rows
                if json.loads(payload).get("request_key", "").startswith("owner-check:")]
    prior = {key for key, _ in owner_checks()}
    a.click("Rerun Required skipped check")
    a.wait(lambda:any(key not in prior for key, _ in owner_checks()), "forced-rerun")
    fresh = [payload for key, payload in owner_checks() if key not in prior]
    a.must(any(row["result"].get("blocker") for row in fresh), "technical-rerun-fresh-blocker", fresh_receipts=len(fresh))
    a.snapshot("technical-rerun")
    a.click("Accept v1")
    a.wait(lambda:a.query("return JSON.stringify(!!document.querySelector('dialog[open]'));"), "technical-accept-dialog")
    a.js("""document.querySelector('dialog[open] input[type=checkbox]').click();return 'yes';""")
    # The dialog's explicit Accept is a different control from the underlying page.
    a.js("""[...document.querySelectorAll('dialog[open] button')].find(e=>e.textContent.trim()==='Accept v1').click();return 'yes';""")
    a.wait(lambda:not a.query("return JSON.stringify(!!document.querySelector('dialog[open]'));"), "technical-accepted")
    a.wait(lambda:any(v.get("acceptance", {}).get("open", {}).get("not_run") == 1 for p in a.product() for v in p["tasks"].values()), "acceptance-persisted")
    a.snapshot("technical-accepted")
    a.link(a.board)
    path = a.new_task("skipped-check")
    a.must(not visible_text(a,"Required skipped check"), "automatic-skipped-check-closed")
    task = path.split("/t/")[1].split("/")[0]
    with sqlite3.connect(a.repo/".cortex-dev/cortex.sqlite3") as db:
        rows = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=(SELECT runtime_task_id FROM workspace_executions WHERE task_id=?) AND type='CHECK_EXECUTION_FINISHED'",(task,)).fetchall()
    a.must(any("skipped-check-executed" in row[0] for row in rows), "automatic-skipped-check-receipt", task=task)


def failed(a):
    a.start(True)
    a.fresh_project(True)
    path = a.new_task("failed-check")
    a.wait(lambda:visible_text(a, "Required failed check"), "failed-check")
    a.snapshot("failed-check-result")
    a.wait(lambda:visible_text(a, "The task stopped: worker_failed"), "failure-wait", 30)
    task = path.split("/t/")[1].split("/")[0]
    def receipts():
        with sqlite3.connect(a.repo/".cortex-dev/cortex.sqlite3") as db:
            rows = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=(SELECT runtime_task_id FROM workspace_executions WHERE task_id=?) AND type IN ('CHECK_EXECUTION_FINISHED','TOOL_COMPLETED') ORDER BY id",(task,)).fetchall()
        observed = []
        for row in rows:
            payload = json.loads(row[0])
            if payload.get("operation") == "command_run":
                result = payload["result"]
                observed.append(dict(result, command_ref=result["tool_call_ref"]))
            elif "command_ref" in payload:
                observed.append(payload)
        return observed
    before = receipts()
    refs = {row["command_ref"] for row in before}
    exact = ["python3", "-B", "-c", "raise SystemExit(7)"]
    a.must(any(row["argv"] == exact and row["exit_code"] == 7 for row in before), "initial-failed-receipt")
    a.fill("Message", 'Rerun the check "Required failed check" for this result. Do not skip it. Use the retained exact command and working directory, and report the new evidence. If it cannot run, report the concrete technical blocker; never mark it passed without evidence.')
    a.js("""document.querySelector('[aria-label="Message"]').focus();return 'yes';""")
    a.keys("Return")
    def fresh():
        return [row for row in receipts() if row["command_ref"] not in refs and row["argv"] == exact]
    a.wait(fresh, "failed-forced-receipt", 20)
    a.must(any(row["exit_code"] == 7 and row["complete"] for row in fresh()), "failed-rerun-still-fails", fresh_receipts=len(fresh()))
    a.snapshot("failed-check-rerun")


def archive(a):
    checked(a)
    path = a.js("return location.pathname;")
    task = path.split("/t/")[1].split("/")[0]
    def archive_current():
        a.click("Task actions")
        a.js("""const e=[...document.querySelectorAll('[role=menuitem]')].find(e=>e.textContent.trim()==='Archive');if(!e)throw Error('archive menu item missing');e.click();return 'yes';""")
        time.sleep(1)
    archive_current()
    a.link("/m/local/archive")
    a.wait(lambda:visible_text(a,"checked-change"), "archived-row")
    a.snapshot("archive-populated")
    a.click("Restore")
    a.wait(lambda:not a.query("return JSON.stringify(!!document.querySelector('.ar-cols:not(.th)'));"), "restored")
    a.link(a.board)
    open_card(a, "Checked-change")
    archive_current()
    a.link("/m/local/archive")
    a.wait(lambda:visible_text(a,"checked-change"), "rearchived")
    a.snapshot("archive-before-purge")
    a.js("""const e=[...document.querySelectorAll('.ar-cols:not(.th) button')].find(e=>e.getAttribute('aria-label')?.includes('More'));if(!e)throw Error('archive menu missing');e.click();return 'yes';""")
    a.click("Delete now…")
    a.click("Delete 1 task permanently")
    a.wait(lambda:not visible_text(a,"checked-change"), "purged")
    with sqlite3.connect(a.repo/".cortex-dev/cortex.sqlite3") as db:
        count = db.execute("SELECT COUNT(*) FROM workspace_tasks WHERE task_id=?",(task,)).fetchone()[0]
    a.must(count == 0, "purge-runtime-readback", task=task)
    reclaimed = next(v["reclamation"] for p in a.product() for k,v in p["tasks"].items() if k == task)
    a.must(reclaimed["vacuum_status"] in ("completed", "not_needed") and reclaimed["reclaimed_bytes"] > 0,
           "purge-space-reclaimed", **reclaimed)
    a.snapshot("archive-purged")


def unmerged(a):
    a.start(True)
    a.fresh_project(True)
    path = a.new_task("checked-change")
    task = path.split("/t/")[1].split("/")[0]
    def record():
        return next(v for p in a.product() for k,v in p["tasks"].items() if k == task)
    a.click("Task actions")
    a.click("Close task…")
    a.click("Close task")
    a.wait(lambda:record().get("status") == "done", "closed")
    def ask():
        a.click("Task actions")
        a.js("""[...document.querySelectorAll('[role=menuitem]')].find(e=>e.textContent.trim()==='Archive').click();return 'yes';""")
        a.wait(lambda:a.query("return JSON.stringify(!!document.querySelector('dialog[open] input[name=unmerged]'));"), "unmerged-question")
    ask()
    a.click("Keep the work")
    a.wait(lambda:record().get("archiveAskAfter") and not record().get("archive"), "keep-persisted")
    a.must(record().get("archiveQuestion") is False, "kept-out-of-archive")
    ask()
    a.js("""[...document.querySelectorAll('dialog[open] label')].find(e=>e.textContent.includes('Delete the work and archive')).click();return 'yes';""")
    a.js("""document.querySelector('dialog[open] .dlg-f .btn-danger').click();return 'yes';""")
    a.wait(lambda:record().get("archive") and record().get("discardRevision"), "discard-persisted")
    a.must(record().get("work") == "discard_confirmed", "discard-bound-to-revision")
    a.link("/m/local/archive")
    a.wait(lambda:visible_text(a,"checked-change"), "archive-row")
    a.click("Restore")
    a.wait(lambda:record().get("archive") is None, "restore-persisted")
    a.snapshot("unmerged-restored")


def foreign(a):
    a.start(False)
    p = a.scratch / "home/foreign"
    (p/".cortex").mkdir(parents=True, mode=0o700)
    marker = p/".cortex/preserved"
    marker.write_text("Unowned fixture store remains unchanged.")
    a.add(p)
    a.wait(lambda:visible_text(a, "Activate"), "release-store-project-registered")
    a.snapshot("release-store-coexists")
    registry = a.scratch/"state/cortex-workspace-dev/projects.json"
    a.must(registry.exists() and str(p) in registry.read_text(), "release-store-project-registered")
    a.must(marker.read_text() == "Unowned fixture store remains unchanged.", "foreign-unchanged")


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("--evidence",required=True)
    p.add_argument("--stage",choices=("settings","clone","checked","branch","conflict","abort","cortex","checks","archive","failed","unmerged","foreign"),required=True)
    args=p.parse_args()
    a=Native("/",args.evidence)
    try:
        globals()[args.stage](a)
    except Exception as error:
        a.record("journey-error", type=type(error).__name__, message=str(error)[:150])
        try:
            a.snapshot("failed")
        except Exception:
            pass
        raise
    finally:
        a.close()


if __name__ == "__main__":
    main()
