"""Supplemental installed-window screenshots, real pointer/keyboard, fake engines.

The preload supplies DOM geometry and scrolls controls into view. Product commands
use XTEST pointer/keyboard input, not JS-dispatched clicks or events.
The separate installed-package gate remains authoritative for clean/upgrade input.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

from owner_walkthrough import Walk
from journeys_repair_native import visible_text


class Human(Walk):
    def start_service(self, fake=False):
        original = self.env["PATH"]
        # Deliberately absent npm in this disposable service, to exercise recovery advice.
        self.env["PATH"] = "/usr/bin:/bin"
        try:
            super().start_service(fake)
        finally:
            self.env["PATH"] = original

    def point(self, expression):
        box = self.query("const e=" + expression + "; if(!e) return JSON.stringify(null); e.scrollIntoView({block:'nearest'}); const r=e.getBoundingClientRect();return JSON.stringify([r.x+r.width/2,r.y+r.height/2]);")
        if not box:
            raise RuntimeError("missing pointer target: " + expression)
        wid = subprocess.check_output(["xdotool", "search", "--onlyvisible", "--pid", str(self.app.pid)], env=self.env, text=True).splitlines()[-1]
        subprocess.run(["xdotool", "windowactivate", "--sync", wid, "mousemove", "--window", wid, str(round(box[0])), str(round(box[1])), "click", "1"], env=self.env, check=True)
        time.sleep(.35)

    def click(self, name, optional=False):
        expression = """[...document.querySelectorAll('button,a,[role=switch],[role=menuitem]')].find(e=>e.getClientRects().length && [e.getAttribute('aria-label'),e.title,e.textContent.trim(),(()=>{const n=e.cloneNode(true);n.querySelectorAll('.key').forEach(x=>x.remove());return n.textContent.trim()})()].includes(""" + json.dumps(name) + "))"
        if optional and not self.query("return JSON.stringify(!!(" + expression + "));"):
            return "missing"
        self.wait(lambda: self.query("const e=" + expression + ";return JSON.stringify(!!e && !e.disabled && e.getAttribute('aria-disabled')!=='true')"), "enabled " + name)
        self.point(expression)
        self.record("pointer", control=name)

    def link(self, href):
        if self.js("return location.pathname;") == href:
            return
        target = "[...document.querySelectorAll('a')].find(e=>e.getClientRects().length && e.getAttribute('href')===" + json.dumps(href) + ")"
        if not self.query("return JSON.stringify(!!(" + target + "))"):
            if "/workers/" in href:
                base = href.split("/workers/")[0]
                self.link(base)
                self.link(base + "/workers")
            elif href.startswith("/settings/"):
                self.click("Settings")
        self.point(target)
        self.wait(lambda:self.js("return location.pathname;") == href, "navigation " + href)

    def fill(self, name, value):
        self.point("[...document.querySelectorAll('input,textarea')].find(e=>e.getClientRects().length && (e.getAttribute('aria-label')===" + json.dumps(name) + "||e.closest('label')?.textContent.trim().startsWith(" + json.dumps(name) + ")))")
        self.keys("ctrl+a")
        subprocess.run(["xdotool", "type", "--clearmodifiers", "--delay", "15", "--", value], env=self.env, check=True)
        time.sleep(.3)

    def option(self, title):
        self.point("[...document.querySelectorAll('dialog[open] [role=option]')].find(e=>e.querySelector('.grow').firstChild.textContent===" + json.dumps(title) + ")")

    def capture(self, label):
        self.measure(label)
        self.snapshot(label)
        wid = subprocess.check_output(["xdotool", "search", "--onlyvisible", "--pid", str(self.app.pid)], env=self.env, text=True).splitlines()[-1]
        time.sleep(.4)
        # Keep native drag active; ordinary captures normalize the private window.
        if not label.startswith("drag-"):
            subprocess.run(["xdotool", "windowmove", wid, "0", "0"], env=self.env, check=True)
        # A separate GDK client photographs pixels on the private Xvfb screen.
        width, height = self.query("return JSON.stringify([innerWidth,innerHeight])")
        code = "import gi;gi.require_version('Gdk','3.0');from gi.repository import Gdk; p=Gdk.pixbuf_get_from_window(Gdk.get_default_root_window(),0,0," + str(width) + "," + str(height) + ");assert p;p.savev(" + repr(str(self.evidence / (label + ".png"))) + ",'png',[],[])"
        subprocess.run(["python3", "-B", "-c", code], env=self.env, check=True)
        self.record("screenshot", file=label + ".png", width=width, height=height)

    def matrix(self, label):
        current = self.js("return location.pathname;")
        for theme in ("Light", "Dark"):
            self.click("Settings")
            self.link("/settings/app/appearance")
            self.click(theme)
            self.click("Back to workspace")
            self.link(current)
            if label == "worker-steps":
                summary = "document.querySelector('.stream details:not([open]) summary')"
                if self.query("return JSON.stringify(!!(" + summary + "))"):
                    self.point(summary)
            for width, height in ((1280, 800), (1920, 1080)):
                self.resize(width, height)
                self.capture(label + "-" + theme.lower() + "-" + str(width))

    def draft(self, goal, worktree=False):
        before = {key for p in self.product() for key in p["tasks"]}
        self.link(self.board)
        self.click("New task")
        self.fill("Describe the task", goal)
        if worktree:
            self.point("document.querySelector('input[name=checkout]')")
        self.click("Save as draft")
        self.wait(lambda:visible_text(self,goal), "draft card")
        return self.wait(lambda: next((key for p in self.product() for key in p["tasks"] if key not in before), None), "new saved task identity")

    def task(self, task):
        return self.task_record(task)

    def open_task(self, task):
        self.link("/m/local/t/" + task)

    def accept(self, task):
        self.click("Changes")
        self.click("Review the diff")
        self.wait(lambda:visible_text(self, "Accept v1"), "accept control", 45)
        self.must(not self.query("return JSON.stringify(document.querySelector('.rv-sum').innerText.includes(String.fromCharCode(92)))"), "readable-task-result")
        self.capture("review-" + task[-6:])
        self.click("Accept v1")
        self.wait(lambda:self.task(task).get("acceptance") and visible_text(self,"You accepted v1"), "persisted acceptance")
        self.capture("accepted-" + task[-6:])


def run(a):
    a.start(True)
    a.resize(1920,1080)
    repo = a.fresh_project()
    a.must(not a.git("remote"), "local-only-no-remote")
    (repo / ".git/human-walkthrough-fixture").touch()
    a.capture("project-added")
    # Native launcher discovery: fake code sits at the observed system location.
    a.click("New task")
    a.click("Sprint")
    a.fill("Describe the sprint", "Two local delivery stages")
    a.click("Start planning")
    a.wait(lambda:visible_text(a,"Plan two dependent tasks?"), "planning question", 45)
    a.capture("sprint-question")
    a.point("[...document.querySelectorAll('button')].find(e=>e.getClientRects().length && e.textContent.includes('Continue') && !e.textContent.includes('queue'))")
    a.wait(lambda:visible_text(a, "Create 2 tasks"), "planned tasks", 45)
    a.click("Create 2 tasks from Plan v1")
    a.wait(lambda:any(p["sprints"] and len(p["sprints"][0]["taskIds"]) == 2 for p in a.product()), "materialized sprint")
    a.link(a.board)
    a.matrix("sprint-board")
    # The standalone task is deliberately held by the fake engine for live inspection.
    task = a.draft("human-local-change")
    (repo / ".git/human-hold").touch()
    a.point("document.querySelector('.card[data-task=" + task + "] button.btn')")
    a.wait(lambda:a.query("return JSON.stringify(!!document.querySelector('.card[data-task=" + task + "] button.btn')?.textContent.includes('Stop'))"), "board Start projected", 45)
    a.capture("board-start-working")
    a.open_task(task)
    a.link(a.js("return document.querySelector('.tabs a[href$=\"/workers\"]').getAttribute('href')"))
    a.wait(lambda:a.query("return JSON.stringify([...document.querySelectorAll('.wcard')].some(e=>/\\/workers\\/[a-f0-9]{64}$/.test(e.getAttribute('href'))))"), "worker card", 30)
    worker = a.query("return JSON.stringify([...document.querySelectorAll('.wcard')].map(e=>e.getAttribute('href')).find(h=>/\\/workers\\/[a-f0-9]{64}$/.test(h)))")
    a.link(worker)
    a.wait(lambda:visible_text(a,"Fixture command complete"), "worker command output", 30)
    a.must(not visible_text(a,"Show full history") and not a.query("return JSON.stringify(!!document.querySelector('[aria-label=\"Conversation detail\"]'))"), "direct-worker-no-switch")
    # Open the meaningful edit and command detail controls by their visible summaries.
    for needle in ("View edit", "python3"):
        expression = "[...document.querySelectorAll('summary,button')].find(e=>e.getClientRects().length && e.textContent.includes(" + json.dumps(needle) + "))"
        if a.query("return JSON.stringify(!!(" + expression + "))"):
            a.point(expression)
    a.matrix("worker-steps")
    (repo / ".git/human-hold").unlink()
    a.open_task(task)
    a.wait(lambda:visible_text(a,"Accept v1"), "runtime review and verification", 60)
    a.link(worker)
    a.wait(lambda:visible_text(a, "Worker result"), "finished worker result", 30)
    a.must(not a.query("return JSON.stringify(/result_submit|Submit result|\\\\_/.test(document.body.innerText))"), "readable-single-result")
    widths = a.query("return JSON.stringify([...document.querySelectorAll('.ev-note .tx')].map(e=>e.getBoundingClientRect().width))")
    a.must(bool(widths) and all(width > 240 for width in widths), "worker-notes-readable-width", widths=widths)
    a.hover(".stream")
    subprocess.run(["xdotool", "click", "--repeat", "50", "--delay", "5", "4"], env=a.env, check=True)
    a.capture("worker-finished")
    a.open_task(task)
    a.accept(task)
    a.must(not visible_text(a,"Merge…"), "same-branch-no-merge")
    a.click("Commit to main…")
    message = a.query("return JSON.stringify(document.querySelector('dialog[open] textarea').value)")
    a.must(message.isascii(), "english-task-commit", message=message)
    a.capture("commit-to-main")
    a.click("Confirm commit to main")
    a.wait(lambda:a.task(task).get("status") == "done" and not a.query("return JSON.stringify(!!document.querySelector('dialog[open]'))"), "same branch committed", 30)
    a.must(a.git("show","main:a.txt") == "task", "same-branch-file")
    a.must(a.query("return JSON.stringify(document.querySelector('.inspector .ins-sec').innerText.match(/Committed to main/g)?.length)") == 1, "one-committed-status")
    a.capture("same-branch-done")
    # Worktree task starts from a different owner baseline and merges locally.
    (repo / "a.txt").write_text("Disposable notes.\n")
    a.git("add","a.txt"); a.git("commit","-qm","Prepare separate checkout")
    separate = a.draft("human-worktree-change", True)
    a.point("document.querySelector('.card[data-task=" + separate + "] button.btn')")
    a.open_task(separate)
    a.wait(lambda:visible_text(a,"Accept v1"), "worktree result", 60)
    a.accept(separate)
    a.click("Merge…")
    a.capture("worktree-merge-dialog")
    a.click("Confirm merge into main")
    a.wait(lambda:a.task(separate).get("status") == "done" and not a.query("return JSON.stringify(!!document.querySelector('dialog[open]'))"), "worktree merged", 35)
    a.must(a.git("show","main:a.txt") == "task", "worktree-file")
    a.capture("worktree-merged")
    # Dragging a draft to Done must ask, not silently accept.
    closed = a.draft("Completed in another editor")
    a.resize(1920,1080)
    wid = subprocess.check_output(["xdotool","search","--onlyvisible","--pid",str(a.app.pid)],env=a.env,text=True).splitlines()[-1]
    for destination in ("work", "done"):
        source, target = a.query("const s=document.querySelector('.card[data-task=" + closed + "]'),t=document.querySelector('[data-col=" + destination + "]');const p=(e,y)=>{const r=e.getBoundingClientRect();return [r.x+r.width/2,r.y+y]};return JSON.stringify([p(s,40),p(t,480)])")
        subprocess.run(["xdotool","mousemove","--window",wid,str(round(source[0])),str(round(source[1]))],env=a.env,check=True)
        subprocess.run(["xdotool","mousedown","1"],env=a.env,check=True)
        for i in range(1,21):
            x=source[0]+(target[0]-source[0])*i/20; y=source[1]+(target[1]-source[1])*i/20
            subprocess.run(["xdotool","mousemove","--window",wid,str(round(x)),str(round(y))],env=a.env,check=True)
            time.sleep(.025)
        a.capture("drag-" + destination)
        subprocess.run(["xdotool","mouseup","1"],env=a.env,check=True)
        if destination == "work":
            time.sleep(.5)
            a.must(not a.query("return JSON.stringify(!!document.querySelector('dialog[open]'))") and a.task(closed).get("status") != "done", "non-done-drop-refused")
    a.wait(lambda:visible_text(a,"Done outside"), "external completion confirmation")
    a.capture("done-confirmation")
    a.click("Close task")
    a.wait(lambda:a.task(closed).get("status") == "done" and not a.query("return JSON.stringify(!!document.querySelector('dialog[open]'))"), "closed task")
    a.must(not a.task(closed).get("acceptance") and not a.task(closed).get("integration"), "external-close-does-not-certify")
    a.open_task(closed)
    a.capture("closed-task")
    a.link(a.board)
    a.click("Git")
    (repo / "notes.txt").write_text("Local Git tab fixture.\n")
    a.wait(lambda:visible_text(a,"notes.txt"), "git status", 15)
    a.click("Generate")
    a.wait(lambda:a.query("return JSON.stringify(document.querySelector('[aria-label=\"Commit message\"]').value.startsWith('Update '))"), "generated commit")
    a.capture("git-tab-commit")
    generated = a.query("return JSON.stringify(document.querySelector('[aria-label=\"Commit message\"]').value)")
    a.click("Commit")
    a.wait(lambda:a.git("log","-1","--pretty=%s") == generated, "Git tab committed")
    a.must(a.git("show","main:notes.txt") == "Local Git tab fixture.", "git-tab-file")
    a.must(a.git("log","-1","--pretty=%s").isascii(), "english-git-message")
    a.click("Settings")
    a.link("/settings/machine/local/worktrees")
    custom=str(a.scratch/"external/.cortex/worktrees")
    a.fill("Worktree folder",custom)
    a.wait(lambda:a.query("return JSON.stringify(document.querySelector('[aria-label=\"Worktree folder\"]').value)") == custom, "complete typed folder")
    a.click("Save Worktrees on This machine")
    a.wait(lambda:visible_text(a,"Saved on This machine") and a.query("return JSON.stringify(document.querySelector('[aria-label=\"Worktree folder\"]').value)") == custom, "settings saved")
    a.link("/settings/machine/local/overview")
    a.link("/settings/machine/local/worktrees")
    a.must(a.query("return JSON.stringify(document.querySelector('[aria-label=\"Worktree folder\"]').value)") == custom, "saved-settings-readback")
    a.capture("settings-saved")
    a.link("/settings/machine/local/diagnostics")
    a.click("Run")
    a.wait(lambda:visible_text(a,"Needs attention"), "diagnostic reason")
    a.matrix("diagnostics")
    a.click("Back to workspace")
    a.link(a.board)
    a.open_task(task)
    a.click("Task actions")
    a.click("Open in VS Code")
    a.wait(lambda:Path("/tmp/editor-called.json").is_file(), "native editor recorder")
    called=json.loads(Path("/tmp/editor-called.json").read_text())
    a.must(str(repo) in called, "editor-main-folder", argv=called)
    a.capture("editor-opened")
    plain=a.scratch/"projects/plain-folder"; plain.mkdir()
    a.add(plain); a.click("Activate")
    a.wait(lambda:not visible_text(a,"Activate"), "plain project activated")
    a.click("Git")
    a.wait(lambda:visible_text(a,"Git"), "plain git status")
    a.must(not (plain/".git").exists(), "plain-folder-stays-non-git")
    a.capture("plain-folder")
    a.record("human-summary", passed=not a.failures, screenshots=len(list(a.evidence.glob("*.png"))), paid_calls=0,
             binary_sha256=hashlib.sha256(Path("/usr/bin/cortex-workspace-dev").read_bytes()).hexdigest())
    return not a.failures


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("--evidence",required=True);args=p.parse_args()
    a=Human(args.evidence)
    # Container-only fake editor replaces no owner program or installed package file.
    assert Path("/usr/bin/code").is_file()
    try:
        return 0 if run(a) else 1
    except Exception as error:
        if a.app is not None and a.app.poll() is None:
            a.capture("incomplete")
        a.record("human-incomplete", error=type(error).__name__, reason=str(error)[:220])
        raise
    finally:
        a.close()
        import re
        failures = 0
        for name in ("app.stderr", "service.stderr"):
            path = a.scratch / name
            if path.exists():
                failures += len(re.findall(r"(?im)^(?:corrupted double-linked list|corrupted size vs[.] prev_size|double free or corruption|free[(][)]:|malloc[(][)]:|Segmentation fault|Aborted)", path.read_text(errors="replace")))
        a.record("native-health", fatal_diagnostics=failures)
        assert failures == 0, "native crash diagnostic"


if __name__ == "__main__":
    raise SystemExit(main())
