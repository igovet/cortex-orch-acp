"""Remaining packaged journeys: real UI/service/runtime, synthetic engines only."""
import argparse
import json
from pathlib import Path
import sqlite3
import subprocess
import time

from journeys_repair_native import Native, checked, visible_text, stop, open_card, savebar


class Finish(Native):
    def start(self, fake=False):
        self.screen = "2200x1200x24"
        super().start(fake)
        window = subprocess.check_output(["xdotool", "search", "--onlyvisible", "--pid", str(self.app.pid)], env=self.env, text=True).splitlines()[-1]
        subprocess.run(["xdotool", "windowsize", window, "2000", "1100"], env=self.env, check=True)
        self.wait(lambda:self.query("return JSON.stringify(innerWidth>=1800);"), "wide-window")
    def fresh_project(self, fake=True):
        p = self.project()
        self.git("config", "user.name", "Audit")
        self.git("config", "user.email", "audit@example.invalid")
        if fake:
            (p / ".git/workspace-safety-fixture").touch()
            (p / ".git/workspace-finish-fixture").touch()
        self.add(p)
        self.board = self.js("return location.pathname;")
        self.click("Activate")
        self.wait(lambda:not visible_text(self, "Activate"), "activation")
        return p

    def task_record(self, task):
        return next(v for p in self.product() for k, v in p["tasks"].items() if k == task)

    def clock(self, at):
        p = self.repo / ".git/maintenance-clock.json"
        temp = p.with_suffix(".tmp")
        temp.write_text(json.dumps(at))
        temp.replace(p)
        self.record("maintenance-clock", at=at, host_clock_changed=False, seeded_rows=0)

    def hover(self, selector):
        x, y = self.query("const e=document.querySelector(" + json.dumps(selector) + ");const r=e.getBoundingClientRect();return JSON.stringify([Math.round(r.x+r.width/2),Math.round(r.y+r.height/2)]);")
        window = subprocess.check_output(["xdotool", "search", "--onlyvisible", "--pid", str(self.app.pid)], env=self.env, text=True).splitlines()[-1]
        subprocess.run(["xdotool", "mousemove", "--window", window, str(x), str(y)], env=self.env, check=True)
        time.sleep(.15)

    def focus(self):
        self.js("@focus")
        self.js("document.activeElement?.blur();document.querySelector('main')?.focus();return 'yes';")

    def route_style(self, label):
        data = self.query("""
const visible=e=>e.getClientRects().length&&getComputedStyle(e).visibility!=='hidden';
const fields=['outlineStyle','outlineWidth','boxShadow','borderTopColor','borderTopWidth','backgroundColor','fontWeight'];
const look=e=>({...Object.fromEntries(fields.map(k=>[k,getComputedStyle(e)[k]])),width:e.getBoundingClientRect().width,height:e.getBoundingClientRect().height});
window.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true}));
const rows=[...document.querySelectorAll('button,a,input,textarea,select,[role=tab]')].filter(visible).map(e=>{
 const before=look(e);e.focus({preventScroll:true});const focused=look(e);
 window.dispatchEvent(new KeyboardEvent('keydown',{key:'Tab',bubbles:true}));const keyboard=look(e);
 window.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true}));
 e.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true}));const pressed=look(e);
 e.dispatchEvent(new PointerEvent('pointerup',{bubbles:true}));
 return {name:e.getAttribute('aria-label')||e.textContent.trim().slice(0,70),before,focused,keyboard,pressed};
});
return JSON.stringify({path:location.pathname,theme:document.documentElement.dataset.theme,rows});
""")
        (self.evidence / (label + ".json")).write_text(json.dumps(data, indent=2))
        # Keyboard movement may add the quiet focus line and nothing else, and the next press takes it away.
        line = lambda r:{k for k in r["focused"] if r["focused"][k] != r["keyboard"][k]} <= {"outlineStyle","outlineWidth"} and r["keyboard"]["outlineStyle"] in ("none","solid")
        failures = [r for r in data["rows"] if r["before"] != r["focused"] or not line(r) or r["focused"] != r["pressed"] or r["focused"]["outlineStyle"] != "none"]
        self.must(not failures, "style-" + label, controls=len(data["rows"]), failures=[{"name":r["name"], "changed":[k for k in r["before"] if any(r["before"][k] != r[s][k] for s in ("focused","keyboard","pressed"))]} for r in failures])

    def restart_window(self):
        stop(self.app)
        self.launch_window()
        self.wait(lambda:self.query("""return JSON.stringify([...document.querySelectorAll('.sidebar a')].some(e=>e.getClientRects().length&&e.getAttribute('href')?.includes('/p/')));"""), "restored-project-registry")


def retention(a):
    checked(a)
    task = a.js("return location.pathname;").split("/t/")[1].split("/")[0]
    done = a.task_record(task)["doneAt"]
    a.clock(done + 7 * 86400000 - 1)
    time.sleep(3)
    a.must(not a.task_record(task).get("archive"), "before-seven-days")
    a.clock(done + 7 * 86400000)
    a.wait(lambda:a.task_record(task).get("archive"), "automatic-archive")
    archived = a.task_record(task)["archive"]["at"]
    a.link(a.board)
    a.link("/m/local/archive")
    a.wait(lambda:visible_text(a, "checked-change"), "auto-archive-visible")
    a.must(archived == done + 7 * 86400000, "archive-exact-boundary")
    a.clock(archived + 30 * 86400000 - 1)
    time.sleep(3)
    a.must(not a.task_record(task).get("purged"), "before-thirty-days")
    a.clock(archived + 30 * 86400000)
    a.wait(lambda:a.task_record(task).get("purged"), "automatic-purge")
    r = a.task_record(task)["reclamation"]
    with sqlite3.connect(a.repo / ".cortex-dev/cortex.sqlite3") as db:
        count = db.execute("SELECT COUNT(*) FROM workspace_tasks WHERE task_id=?", (task,)).fetchone()[0]
    a.must(count == 0 and r["vacuum_status"] == "completed" and r["reclaimed_bytes"] > 0, "automatic-space-reclaimed", **r)
    a.wait(lambda:not visible_text(a, "checked-change"), "purged-ui")
    a.snapshot("retention-finished")


def unmerged(a):
    a.start(True)
    a.fresh_project()
    path = a.new_task("checked-change")
    task = path.split("/t/")[1].split("/")[0]
    a.click("Task actions")
    a.click("Close task…")
    a.click("Close task")
    a.wait(lambda:a.task_record(task).get("doneAt"), "closed")
    done = a.task_record(task)["doneAt"]
    a.clock(done + 7 * 86400000)
    a.wait(lambda:a.task_record(task).get("archiveQuestion"), "automatic-unmerged-question")
    a.link(a.board)
    a.snapshot("unmerged-question-board")
    a.click("Task actions", optional=True)
    # Reopen the actual task and its existing owner question control.
    open_card(a, "Checked-change")
    a.click("Task actions")
    a.js("""[...document.querySelectorAll('[role=menuitem]')].find(e=>e.textContent.trim()==='Archive').click();return 'yes';""")
    a.wait(lambda:a.query("return JSON.stringify(!!document.querySelector('dialog[open] input[name=unmerged]'));"), "unmerged-dialog")
    # Return maintenance to the wall-clock domain for the helper's owner decision.
    a.clock(int(time.time()*1000))
    a.click("Keep the work")
    a.wait(lambda:a.task_record(task).get("archiveAskAfter") and not a.task_record(task).get("archiveQuestion"), "keep")
    keep = a.task_record(task)["archiveAskAfter"]
    a.restart_window()
    a.must(a.task_record(task)["archiveAskAfter"] == keep and not a.task_record(task).get("archive"), "keep-reopen-persistence")
    a.clock(keep - 1)
    time.sleep(3)
    a.must(not a.task_record(task).get("archiveQuestion"), "keep-defers-question")
    a.clock(keep)
    a.wait(lambda:a.task_record(task).get("archiveQuestion"), "question-reappears")
    a.link(a.board)
    open_card(a, "Checked-change")
    a.click("Task actions")
    a.js("""[...document.querySelectorAll('[role=menuitem]')].find(e=>e.textContent.trim()==='Archive').click();return 'yes';""")
    a.wait(lambda:a.query("return JSON.stringify(!!document.querySelector('dialog[open] input[name=unmerged]'));"), "delete-question")
    a.js("""[...document.querySelectorAll('dialog[open] label')].find(e=>e.textContent.includes('Delete the work and archive')).click();return 'yes';""")
    a.js("document.querySelector('dialog[open] .dlg-f .btn-danger').click();return 'yes';")
    a.wait(lambda:a.task_record(task).get("discardRevision") and a.task_record(task).get("archive"), "delete-persisted")
    revision = a.task_record(task)["discardRevision"]
    a.restart_window()
    a.must(a.task_record(task)["discardRevision"] == revision and a.task_record(task)["work"] == "discard_confirmed", "delete-reopen-persistence")
    a.snapshot("unmerged-finished")


def routing(a):
    a.start(True)
    a.fresh_project()
    a.click("Settings")
    a.link("/settings/machine/local/orchestration/routing")
    a.wait(lambda:a.query("""return JSON.stringify(!!document.querySelector('[aria-label="Edit pin: All roles"]'));"""), "routing")
    a.snapshot("routing-before")
    a.click("Edit pin: All roles")
    a.click("Done")
    savebar(a)
    routing_file = next((a.scratch / "config").rglob("routing.json"))
    rules = json.loads(routing_file.read_text())
    a.must("fixture-model" in rules.get("pin", {}).get("*", ""), "routing-pin-persisted")
    a.link("/settings/machine/local/orchestration/effective")
    a.wait(lambda:visible_text(a, "Would run"), "eligible-preview")
    a.must(visible_text(a, "Fixture model") and visible_text(a, "xr_"), "preview-route-identity")
    a.snapshot("ranked-preview")
    a.click("All roles")
    a.snapshot("preview-all-roles")
    a.click("Back to workspace")
    a.link(a.board)
    a.click("New task")
    a.fill("Describe the task", "checked-change")
    # Every selection is made with the actual route controls.
    a.js("document.querySelector('[aria-label^=\"Coordinator model:\"]').click();return 'yes';")
    a.js("[...document.querySelectorAll('[role=menuitem]')].find(e=>e.getClientRects().length&&e.textContent.includes('Fixture model')).click();return 'yes';")
    a.js("document.querySelector('[aria-label^=\"Reasoning effort:\"]').click();return 'yes';")
    a.click("High")
    a.js("document.querySelector('[aria-label^=\"Access mode:\"]').click();return 'yes';")
    a.js("""[...document.querySelectorAll('#m-mode-new [role=menuitem]')].find(e=>e.textContent.trim().startsWith('Auto')).click();return 'yes';""")
    a.js("document.querySelector('[aria-label^=\"Message language:\"]').click();return 'yes';")
    a.js("""[...document.querySelectorAll('#m-lang-new [role=menuitem]')].find(e=>e.textContent.trim()==='English').click();return 'yes';""")
    a.click("Start now")
    a.wait(lambda:visible_text(a, "Result v1"), "selected-start", 45)
    task = a.js("return location.pathname;").split("/t/")[1].split("/")[0]
    with sqlite3.connect(a.repo / ".cortex-dev/cortex.sqlite3") as db:
        options = json.loads(db.execute("SELECT options_json FROM workspace_task_options WHERE task_id=?", (task,)).fetchone()[0])
        runtime_task = db.execute("SELECT runtime_task_id FROM workspace_executions WHERE task_id=?", (task,)).fetchone()[0]
        configs = db.execute("SELECT route_id,parameters FROM configuration_revisions WHERE task_id=?", (runtime_task,)).fetchall()
        mode = db.execute("SELECT mode FROM access_mode_events WHERE task_id=? ORDER BY id DESC LIMIT 1", (runtime_task,)).fetchone()[0]
        language = db.execute("SELECT language FROM task_language WHERE task_id=?", (runtime_task,)).fetchone()[0]
    a.must(mode == "auto" and language == "en", "started-runtime-mode-language", mode=mode, language=language)
    a.must(options["settings"]["effort"] == "high" and options["settings"]["mode"] == "auto" and options["settings"]["language"] == "en", "task-choices-persisted", settings=options["settings"])
    a.must(any(route == options["settings"]["model"] and json.loads(params).get("thought_level") == "high" for route, params in configs), "started-route-config", revisions=len(configs))
    a.snapshot("routing-started")


def styles(a):
    a.start(True)
    a.fresh_project()
    path = a.new_task("checked-change")
    task_path = path
    board = a.board
    def panels(label, names):
        for name in names:
            def fact():
                return a.query("""const e=document.querySelector('.titlebar [aria-label='+JSON.stringify(""" + json.dumps(name) + """)+']');return JSON.stringify({pressed:e.getAttribute('aria-pressed'),color:getComputedStyle(e).color,rect:[e.offsetWidth,e.offsetHeight]});""")
            before = fact()
            a.click(name)
            after = fact()
            a.must(before["pressed"] != after["pressed"] and before["rect"] == after["rect"], label+"-"+name+"-state", before=before, after=after)
            def light(color):
                import re
                rgb = [float(x) for x in re.findall(r"[0-9.]+", color)[:3]]
                return sum(rgb)
            opened, closed = (after, before) if after["pressed"] == "true" else (before, after)
            a.must(light(opened["color"]) > light(closed["color"]), label+"-"+name+"-lighter-open")
            a.click(name)
    for theme in ("Dark", "Light"):
        a.click("Settings")
        a.link("/settings/app/appearance")
        a.click(theme)
        routes = a.query("""return JSON.stringify([...document.querySelectorAll('a')].filter(e=>e.getClientRects().length).map(e=>e.getAttribute('href')).filter(h=>h?.startsWith('/settings/')));""")
        for i, href in enumerate(dict.fromkeys(routes)):
            a.link(href)
            a.route_style(theme.lower()+"-settings-"+str(i))
        a.click("Back to workspace")
        a.link(board)
        for label in ("Code", "Git", "Worktrees", "Knowledge and skills"):
            a.click(label)
            a.route_style(theme.lower()+"-"+label.split()[0])
        a.link(board)
        a.route_style(theme.lower()+"-board")
        panels(theme.lower()+"-project", ["Sidebar", "Inspector"])
        a.click("New task")
        a.route_style(theme.lower()+"-new-task")
        a.link(board)
        open_card(a, "Checked-change")
        a.route_style(theme.lower()+"-conversation")
        panels(theme.lower()+"-task", ["Sidebar", "Worker pane"])
        a.click("Changes")
        a.route_style(theme.lower()+"-changes")
        a.click("Home")
        a.route_style(theme.lower()+"-home")
        a.click("Inbox")
        a.route_style(theme.lower()+"-inbox")
        a.click("Archive")
        a.route_style(theme.lower()+"-archive")
    a.snapshot("styles-finished")


def shortcuts(a):
    a.start(True)
    a.fresh_project()
    def key(key, condition, label):
        a.focus()
        a.keys(key)
        a.wait(condition, label)
        a.record("shortcut", key=key, passed=True, context=a.js("return location.pathname;"))
    key("ctrl+k", lambda:a.query("return JSON.stringify(!!document.querySelector('dialog[open] .ws-pal-in'));"), "palette")
    a.keys("Escape")
    key("ctrl+shift+n", lambda:a.query("""return JSON.stringify(!!document.querySelector('[aria-label="Describe the sprint"]'));"""), "new-sprint")
    a.link(a.board)
    key("ctrl+n", lambda:a.query("""return JSON.stringify(!!document.querySelector('[aria-label="Describe the task"]'));"""), "new-task")
    a.fill("Describe the task", "first draft")
    a.js("""document.querySelector('[aria-label="Describe the task"]').focus();return 'yes';""")
    a.keys("ctrl+Return")
    a.wait(lambda:visible_text(a, "First draft"), "draft-shortcut")
    a.click("New task")
    a.fill("Describe the task", "second draft")
    a.click("Save as draft")
    a.wait(lambda:a.query("return JSON.stringify(document.querySelectorAll('.card-link').length===2);"), "two-cards")
    a.focus()
    a.keys("slash")
    a.must(a.js("return document.activeElement?.id;") == "ws-filter", "board-filter-shortcut")
    a.keys("Escape")
    a.js("document.querySelector('.card-link').focus();return 'yes';")
    a.keys("space")
    a.must(a.query("return JSON.stringify(!!document.querySelector('.card[aria-current=true]'));"), "space-preview")
    a.keys("Escape", "Down")
    a.must(a.query("return JSON.stringify(document.activeElement===document.querySelectorAll('.card-link')[1]);"), "card-arrow-down")
    a.keys("Up")
    a.must(a.query("return JSON.stringify(document.activeElement===document.querySelector('.card-link'));"), "card-arrow-up")
    before = a.js("return document.querySelector('.ws-app').dataset.sidebar;")
    key("ctrl+b", lambda:a.js("return document.querySelector('.ws-app').dataset.sidebar;") != before, "sidebar")
    a.keys("ctrl+b")
    before = a.js("return document.querySelector('.ws-app').dataset.inspector;")
    key("ctrl+alt+b", lambda:a.js("return document.querySelector('.ws-app').dataset.inspector;") != before, "inspector")
    a.keys("ctrl+alt+b")
    a.js("document.querySelector('.card-link').focus();return 'yes';")
    a.keys("Return")
    a.wait(lambda:"/t/" in a.js("return location.pathname;"), "card-enter")
    a.record("shortcut", key="Return", passed=True, context="board-card")
    key("alt+Left", lambda:a.js("return location.pathname;") == a.board, "back")
    key("alt+Right", lambda:"/t/" in a.js("return location.pathname;"), "forward")
    a.link(a.board)
    a.click("Code")
    key("ctrl+p", lambda:a.query("""return JSON.stringify(!!document.querySelector('[aria-label="Go to file"]'));"""), "go-file")
    a.typed("a.txt")
    a.keys("Return")
    a.wait(lambda:a.query("return JSON.stringify(!!document.querySelector('.cm-content'));"), "file-open")
    key("ctrl+shift+f", lambda:a.query("""return JSON.stringify(document.activeElement?.getAttribute('aria-label')==='Search files');"""), "search-files")
    a.fill("Search files", "Disposable")
    a.wait(lambda:visible_text(a, "a.txt"), "search-result")
    a.link(a.board)
    key("ctrl+j", lambda:a.query("return JSON.stringify(document.querySelector('.ws-app').dataset.terminal==='open');"), "terminal")
    a.keys("ctrl+j")
    key("ctrl+i", lambda:"/inbox" in a.js("return location.pathname;"), "inbox")
    key("ctrl+comma", lambda:"/settings/" in a.js("return location.pathname;"), "settings")
    a.click("Back to workspace")
    key("question", lambda:a.query("""return JSON.stringify(document.querySelector('dialog[open]')?.getAttribute('aria-label')==='Help and shortcuts');"""), "help")
    a.keys("Escape")
    a.snapshot("shortcuts-finished")


def picker(a):
    from empty_desktop_audit import picker as run
    run(a)


def editor(a):
    from empty_desktop_audit import Audit
    from editor_native import prepare, edit_and_save, conflict_found_on_disk, conflict_met_by_save, navigation_keeps_text, large_file, autosave
    a.start_service = Audit.start_service.__get__(a)
    project, board = prepare(a)
    file = project / "README.md"
    edit_and_save(a, file)
    conflict_found_on_disk(a, file)
    conflict_met_by_save(a, file)
    navigation_keeps_text(a, file)
    large_file(a, project / "big.txt")
    autosave(a, file, board)
    a.snapshot("editor-finished")


def profiles(a):
    a.start(True)
    a.fresh_project()
    a.click("Settings")
    a.link("/settings/machine/local/orchestration/policy")
    a.fill("Attempts per task", "5")
    a.fill("Fallback budget", "2")
    a.js("""document.querySelector('[aria-label="Allow work to move to another provider"]').click();return 'yes';""")
    savebar(a)
    defaults = next((a.scratch / "config").rglob("defaults.json"))
    value = json.loads(defaults.read_text())
    a.must(value["ceiling"]["limits"]["attempts"] == 5 and value["ceiling"]["fallback_policy"]["budget"] == 2, "limits-persisted")
    a.link("/settings/machine/local/orchestration/profiles")
    a.click("Save as new…")
    a.fill("Profile name", "finish-profile")
    a.click("Save as new")
    a.wait(lambda:visible_text(a, "finish-profile"), "profile-created")
    a.must(visible_text(a, "MCP usage policy") and visible_text(a, "session limits") and not visible_text(a, "Stores only"), "profile-sections-visible")
    a.click("Profile name: finish-profile")
    a.fill("Profile name", "renamed-profile")
    a.keys("Return")
    a.wait(lambda:visible_text(a, "renamed-profile") and not visible_text(a, "finish-profile"), "profile-renamed")
    a.click("More for renamed-profile")
    a.js("""[...document.querySelectorAll('[role=menuitem]')].find(e=>e.getClientRects().length&&e.textContent.trim().startsWith('Detach')).click();return 'yes';""")
    a.wait(lambda:visible_text(a, "No profile is active"), "profile-before-activation")
    a.click("Activate…")
    a.snapshot("activate-dialog")
    if a.query("return JSON.stringify(!!document.querySelector('dialog[open]'));"):
        a.js("""const e=[...document.querySelectorAll('dialog[open] button')].find(e=>e.textContent.trim()==='Activate');if(e)e.click();return 'yes';""")
    a.wait(lambda:a.query("""return JSON.stringify([...document.querySelectorAll('.ofrow .sub')].some(e=>e.textContent==='Active'));"""), "profile-active")
    a.click("More for renamed-profile")
    a.js("""[...document.querySelectorAll('[role=menuitem]')].find(e=>e.getClientRects().length&&e.textContent.trim().startsWith('Detach')).click();return 'yes';""")
    a.wait(lambda:visible_text(a, "No profile is active"), "profile-detached")
    a.click("More for renamed-profile")
    a.click("Delete…")
    a.click("Delete")
    a.wait(lambda:not visible_text(a, "renamed-profile"), "profile-deleted")
    a.restart_window()
    a.click("Settings")
    a.link("/settings/machine/local/orchestration/profiles")
    a.must(not visible_text(a, "renamed-profile"), "profile-deletion-reopen")
    a.snapshot("profiles-finished")


def git(a):
    a.start(True)
    a.fresh_project()
    a.click("Git")
    (a.repo / "a.txt").write_text("Owner local edit\\n")
    a.wait(lambda:a.query("""return JSON.stringify(!!document.querySelector('[aria-label="Stage a.txt"]'));"""), "git-file")
    a.hover(".gf:has([aria-label='Stage a.txt'])")
    a.click("Stage a.txt")
    a.wait(lambda:visible_text(a, "Staged"), "staged")
    a.fill("Commit message", "Native shortcut commit")
    a.js("""document.querySelector('[aria-label="Commit message"]').focus();return 'yes';""")
    a.keys("ctrl+Return")
    a.wait(lambda:a.git("log", "-1", "--format=%s") == "Native shortcut commit", "committed")
    a.must(not a.git("status", "--porcelain"), "commit-file-readback")
    a.record("shortcut", key="Ctrl+Enter", context="Git commit", passed=True)
    a.fill("Commit message", "Native amended message")
    a.click("More commit options")
    a.click("Amend last commit…")
    a.snapshot("amend-dialog")
    a.click("Amend")
    a.wait(lambda:a.git("log", "-1", "--format=%s") == "Native amended message", "amended")
    a.must(a.git("rev-list", "--count", "HEAD") == "2", "amend-not-new-commit")
    (a.repo / "a.txt").write_text("Unwanted edit\\n")
    a.wait(lambda:a.query("""return JSON.stringify(!!document.querySelector('[aria-label="Revert a.txt"]'));"""), "revert-file")
    a.hover(".gf:has([aria-label='Revert a.txt'])")
    a.click("Revert a.txt")
    a.click("Revert")
    a.wait(lambda:(a.repo / "a.txt").read_text() == "Owner local edit\\n", "reverted")
    a.snapshot("git-finished")


def pr(a):
    a.start(True)
    a.fresh_project()
    bare = a.scratch / "offline-origin.git"
    subprocess.run(["git", "clone", "--bare", str(a.repo), str(bare)], env=a.env, check=True, capture_output=True)
    a.git("remote", "add", "origin", str(bare))
    path = a.new_task("checked-change")
    task = path.split("/t/")[1].split("/")[0]
    a.click("Changes")
    a.click("Accept v1")
    a.wait(lambda:a.query("""return JSON.stringify(!![...document.querySelectorAll('button')].find(e=>e.textContent.trim()==='Create PR…'));"""), "pr-control")
    state = a.query("""const e=[...document.querySelectorAll('button')].find(e=>e.textContent.trim()==='Create PR…');return JSON.stringify({off:e.getAttribute('aria-disabled')==='true',reason:e.title});""")
    a.must(state["off"] and "GitHub CLI" in state["reason"], "pr-disabled-exact-prerequisite", **state)
    a.must(a.git("remote", "get-url", "origin") == str(bare), "pr-local-bare-only")
    a.must(a.git("rev-list", "--all", "--count", repo=bare) == "1", "pr-no-push")
    a.must(not a.task_record(task).get("prAttempt"), "pr-no-remote-attempt")
    a.snapshot("pr-prerequisites")


def sprint(a):
    a.start(True)
    a.fresh_project()
    a.click("New task")
    a.click("Sprint")
    a.fill("Describe the sprint", "Plan two offline changes")
    a.click("Start planning")
    a.wait(lambda:visible_text(a, "Plan two dependent tasks?"), "planning-question", 35)
    a.focus()
    a.keys("1")
    a.wait(lambda:visible_text(a, "Create 2 tasks"), "plan-ready", 35)
    a.snapshot("sprint-plan")
    a.click("Create 2 tasks from Plan v1")
    a.wait(lambda:visible_text(a, "Start task 1"), "materialized", 30)
    product = a.product()[0]
    planned = product["sprints"][0]
    ids = planned["taskIds"]
    a.must(len(ids) == 2 and product["tasks"][ids[1]]["dependencies"] == [ids[0]], "sprint-linked-tasks")
    a.click("Start task 1")
    a.link(a.board)
    open_card(a, "Question-first")
    a.wait(lambda:visible_text(a, "Continue the first task?"), "first-started", 35)
    # The fixture asks on the first task; answer through the task, not a seeded store.
    if not a.task_record(ids[0]).get("status") == "ready_to_merge":
        a.wait(lambda:visible_text(a, "Continue the first task?"), "task-question")
        a.focus()
        a.keys("1")
        a.wait(lambda:a.task_record(ids[0]).get("status") == "ready_to_merge", "first-ready", 40)
    with sqlite3.connect(a.repo / ".cortex-dev/cortex.sqlite3") as db:
        neighbour_starts = db.execute("SELECT COUNT(*) FROM workspace_executions WHERE task_id=?", (ids[1],)).fetchone()[0]
    a.must(neighbour_starts == 0, "manual-does-not-start-neighbour")
    a.link(a.board)
    a.snapshot("manual-sprint-board")
    # Open the sprint via its actual planning link, then switch the queue mode.
    hrefs = a.query("""return JSON.stringify([...document.querySelectorAll('a')].map(e=>e.getAttribute('href')).filter(h=>h?.includes('/s/')&&h.endsWith('/planning')));""")
    sprint_board = next(h for h in a.query("""return JSON.stringify([...document.querySelectorAll('a')].map(e=>e.getAttribute('href')).filter(h=>h?.includes('/s/')&&h.endsWith('/board')));"""))
    a.link(sprint_board)
    a.click("Sprint planning")
    a.click("Automatic")
    time.sleep(1)
    a.snapshot("automatic-sprint-mode")
    a.wait(lambda:a.query("""return JSON.stringify([...document.querySelectorAll('button')].some(e=>e.textContent.trim()==='Automatic'&&e.getAttribute('aria-current')==='true'));"""), "automatic-mode-confirmed")
    label=a.wait(lambda:a.query("""return JSON.stringify([...document.querySelectorAll('button')].filter(e=>e.getClientRects().length).map(e=>e.textContent.trim()).find(t=>['Run sprint','Continue queue','Pause queue'].includes(t))||'');""") or (a.task_record(ids[1]).get("status")=="ready_to_merge" and "complete"), "automatic-run-control")
    if label in ("Run sprint","Continue queue"):
        a.click(label)
    a.wait(lambda:a.task_record(ids[1]).get("status") == "ready_to_merge", "automatic-dependent-ready", 45)
    a.must(all(a.task_record(i).get("status") == "ready_to_merge" for i in ids), "sprint-stops-at-ready-not-merged")
    a.snapshot("sprint-finished")


def resources(a):
    from empty_desktop_audit import Audit
    from stop_reports_native import run
    a.start_service = Audit.start_service.__get__(a)
    run(a)


def mcp(a):
    a.start(True)
    a.fresh_project()
    a.click("Settings")
    a.link("/settings/machine/local/orchestration/tools")
    a.click("Add server")
    a.fill("Server name", "finish-mcp")
    a.fill("Command", "/usr/bin/python3")
    a.fill("Paste a command line", "/usr/bin/python3 /checkout/tests/workspace_support/discovery_only_mcp.py")
    a.click("Split")
    a.click("Next: Connect")
    a.click("Connect and list tools")
    a.wait(lambda:visible_text(a, "Go to Tool access") or visible_text(a, "Enter tool names by hand"), "mcp-list", 25)
    snap = a.snapshot("mcp-discovery")
    if not visible_text(a, "audit_read"):
        a.record("discovered-access-row", outcome="blocked", reason="The native container cannot establish sandboxed MCP listing; actual UI failure retained.")
        a.js("""const e=[...document.querySelectorAll('input')].find(e=>e.placeholder==='exact_tool_name'||e.getAttribute('aria-label')?.startsWith('Tool name'));if(e)e.focus();return 'yes';""")
        a.fill("Tool name", "audit_read")
        a.keys("Return")
    a.click("Go to Tool access")
    a.click("Choose")
    a.js("""document.querySelector('[aria-label^="audit_read, coordinator,"]').click();return 'yes';""")
    a.route_style("mcp-access")
    a.click("Save and turn on")
    a.wait(lambda:visible_text(a, "All servers"), "mcp-saved")
    p = next((a.scratch / "config").rglob("mcp-servers.json"))
    data = json.loads(p.read_text())
    a.must("audit_read" in data["servers"]["finish-mcp"]["coordinator_tools"], "mcp-cascade-persisted")
    a.click("Connection")
    a.fill("Command", "/usr/bin/python3")
    a.click("Tools")
    a.js("""document.querySelector('[aria-label^="audit_read, writing workers,"]').click();return 'yes';""")
    a.must(visible_text(a, "Also removed from read-only workers and the coordinator.") and visible_text(a, "Grant at least one tool."), "mcp-empty-grant-validation")
    a.click("Discard")
    a.wait(lambda:a.query("""return JSON.stringify(!!document.querySelector('[aria-label^="audit_read, read-only workers,"]'));"""), "mcp-discard-restores")
    a.js("""document.querySelector('[aria-label^="audit_read, read-only workers,"]').click();return 'yes';""")
    savebar(a)
    data = json.loads(p.read_text())["servers"]["finish-mcp"]
    a.must(data["allowed_tools"] == ["audit_read"] and not data["read_only_tools"] and not data.get("coordinator_tools"), "mcp-removal-cascades")
    a.click("Remove…")
    a.click("Remove")
    savebar(a)
    a.must("finish-mcp" not in json.loads(p.read_text())["servers"], "mcp-delete-persisted")
    a.snapshot("mcp-finished")


def queue(a):
    from empty_desktop_audit import Audit, drop_text
    a.start_service = Audit.start_service.__get__(a)
    a.start(True)
    a.add(a.project())
    board = a.js("return location.pathname;")
    a.click("Activate")
    a.wait(lambda:not visible_text(a,"Activate"), "activated")
    a.click("New task")
    a.fill("Describe the task", "Queue persistence audit")
    a.click("Save as draft")
    a.wait(lambda:visible_text(a,"Queue persistence audit"), "queue-draft")
    open_card(a,"Queue persistence audit")
    a.click("Start")
    a.wait(lambda:visible_text(a,"Which direction"), "queue-question")
    a.click("…Other answer")
    a.fill("Your answer","first line\nsecond line")
    a.js("""document.querySelector('[aria-label="Your answer"]').focus();return 'yes';""")
    a.keys("ctrl+Return")
    a.wait(lambda:(a.repo/"streaming").exists(), "answer-shortcut")
    def rows():
        return a.query("""return JSON.stringify([...document.querySelectorAll('.qrow .grip')].map(e=>e.getAttribute('aria-label').slice(9)));""")
    for i in range(3):
        a.fill("Message", "queued-"+str(i))
        if i == 0:
            drop_text(a, ".dock .composer textarea", "queue.txt", "queue fixture")
        a.js("""document.querySelector('[aria-label="Message"]').focus();return 'yes';""")
        a.keys("ctrl+Return")
        a.wait(lambda:len(rows()) == i+1 and a.query("""return JSON.stringify(document.querySelector('[aria-label="Message"]').value==='');"""), "queued-"+str(i))
        time.sleep(.3)
    a.js("""document.querySelector('.qrow .grip').focus();return 'yes';""")
    a.keys("Down")
    a.wait(lambda:rows()==["queued-1","queued-0","queued-2"], "reorder")
    a.restart_window()
    a.link(board)
    open_card(a,"Queue persistence audit")
    a.wait(lambda:rows()==["queued-1","queued-0","queued-2"], "queue-reopen-order")
    a.must(a.query("return JSON.stringify(document.querySelectorAll('.queue .ws-att').length===1);"), "queue-reopen-attachment")
    a.click("Remove from queue: queued-2")
    a.wait(lambda:rows()==["queued-1","queued-0"], "queue-remove")
    a.js("""[...document.querySelectorAll('.qrow')].find(e=>e.textContent.includes('queued-0')).querySelector('.btn').click();return 'yes';""")
    a.wait(lambda:rows()==["queued-1"] and a.query("""return JSON.stringify(document.querySelector('[aria-label="Message"]').value==='queued-0'&&!!document.querySelector('.dock .composer .ws-att'));"""), "queue-edit-restores-bytes")
    a.fill("Message","Focus on durable conversation")
    a.js("""document.querySelector('[aria-label="Message"]').focus();return 'yes';""")
    a.keys("Return")
    a.wait(lambda:not a.query("return JSON.stringify(!!document.querySelector('.dock .composer .ws-att'));"), "steer-clears-attachments")
    a.click("Remove from queue: queued-1")
    a.wait(lambda:not rows(), "queue-empty")
    (a.repo/"release-stream").touch()
    a.wait(lambda:visible_text(a,"Runtime-owned journey result."), "queue-result", 35)
    a.link(board)
    a.click("List")
    a.must(visible_text(a,"Queue persistence audit"), "list-shows-task")
    a.snapshot("queue-finished")


def worker_controls(a):
    from empty_desktop_audit import Audit
    a.start_service = Audit.start_service.__get__(a)
    a.start(True)
    a.add(a.project())
    (a.repo/"start-worker-scenario").touch()
    a.click("Activate")
    a.wait(lambda:not visible_text(a,"Activate"), "activated")
    a.click("New task")
    a.fill("Describe the task","Worker controls audit")
    a.click("Save as draft")
    a.wait(lambda:visible_text(a,"Worker controls audit"), "worker-draft")
    open_card(a,"Worker controls audit")
    a.click("Start")
    a.wait(lambda:(a.repo/"worker-one").exists() and (a.repo/"worker-two").exists(), "worker-dispatch", 35)
    a.js("""[...document.querySelectorAll('.tabs a')].find(e=>e.textContent.trim().startsWith('Workers')).click();return 'yes';""")
    a.wait(lambda:visible_text(a,"Receive owner guidance"), "worker-cards")
    links=a.query("""return JSON.stringify([...document.querySelectorAll('.wcard')].map(e=>({text:e.textContent,href:e.getAttribute('href')})));""")
    one=next(x["href"] for x in links if "Receive owner guidance" in x["text"])
    two=next(x["href"] for x in links if "Wait for command control" in x["text"])
    a.link(one)
    a.fill("Message backend-dev","Keep the worker message verbatim\nsecond line")
    a.click("Send to backend-dev")
    a.wait(lambda:(a.repo/"worker-guidance-delivered").exists(), "worker-message")
    a.wait(lambda:visible_text(a,"Worker guidance completed."), "worker-result", 35)
    a.click("Show full history")
    a.snapshot("worker-history")
    a.link(two)
    a.wait(lambda:visible_text(a,"Stop command…"), "worker-command", 20)
    a.click("Stop command…")
    a.js("""const e=document.querySelector('dialog[open] textarea');e.value='Focused owner stop';e.dispatchEvent(new Event('input',{bubbles:true}));return 'yes';""")
    a.click("Stop command")
    a.wait(lambda:(a.repo/"worker-command-stopped").exists(), "worker-command-stopped")
    a.click("Stop…")
    a.click("Stop worker")
    a.wait(lambda:visible_text(a,"Cancelled"), "worker-cancelled")
    a.snapshot("worker-controls-finished")


def owner_controls(a):
    a.start(True)
    a.fresh_project()
    custom = a.scratch / "home/custom-worktrees"
    a.click("Settings")
    a.link("/settings/machine/local/worktrees")
    default = a.js("""return document.querySelector('[aria-label="Worktree folder"]').value;""")
    a.fill("Worktree folder",str(custom))
    savebar(a)
    a.click("Back to workspace")
    a.link(a.board)
    path=a.new_task("checked-change")
    source=next(Path(line[9:]) for line in a.git("worktree","list","--porcelain").splitlines() if line.startswith("worktree ") and line[9:]!=str(a.repo))
    a.must(source.is_relative_to(custom), "custom-root-used", path=str(source))
    a.click("Worktree details")
    a.must(a.query("""const e=[...document.querySelectorAll('button')].find(e=>e.textContent==='Open worktree terminal');return JSON.stringify(e?.getAttribute('aria-disabled')==='true'&&e.title.includes('project shells only'));"""), "worktree-terminal-disabled-reason")
    a.keys("Escape")
    a.click("Terminal")
    a.wait(lambda:a.query("return JSON.stringify(!!document.querySelector('.xterm-helper-textarea'));"), "task-terminal")
    a.wait(lambda:visible_text(a,"Running"), "shell-running")
    a.js("@focus")
    a.js("document.querySelector('.xterm-helper-textarea').focus();return 'yes';")
    a.typed("pwd > worktree-cwd.txt")
    a.keys("Return")
    a.wait(lambda:(a.repo/"worktree-cwd.txt").exists(), "shell-readback")
    a.must((a.repo/"worktree-cwd.txt").read_text().strip()==str(a.repo), "project-terminal-cwd")
    a.restart_window()
    a.wait(lambda:a.query("return JSON.stringify(!![...document.querySelectorAll('a')].find(e=>e.getClientRects().length&&e.getAttribute('href')==="+json.dumps(a.board)+"));"), "project-after-reopen")
    a.link(a.board)
    open_card(a,"Checked-change")
    if not a.query("return JSON.stringify(document.querySelector(\'.ws-app\').dataset.terminal===\'open\');"):
        a.click("Terminal")
    a.wait(lambda:visible_text(a,"Running"), "terminal-after-reopen")
    a.click("Close shell")
    a.wait(lambda:visible_text(a,"Closed") or visible_text(a,"Shell exited"), "terminal-close-shell")
    a.click("Settings")
    a.link("/settings/machine/local/worktrees")
    a.click("Use default")
    savebar(a)
    a.must(a.js("""return document.querySelector('[aria-label="Worktree folder"]').value;""")==default and source.is_dir(), "root-reset-keeps-existing")
    a.link("/settings/machine/local/connections")
    a.click("More for Offline fixture")
    a.click("Maximum sessions…")
    a.fill("Sessions, 1 to 64","2")
    a.click("Save")
    a.wait(lambda:visible_text(a,"2 sessions max"), "connection-sessions-persisted")
    a.link("/settings/machine/local/diagnostics")
    a.click("Run")
    a.wait(lambda:visible_text(a,"Last run"), "diagnostics-ran")
    a.snapshot("diagnostics-run")
    a.click("Copy report")
    report=subprocess.check_output(["python3","-B","-c","import gi;gi.require_version('Gtk','3.0');from gi.repository import Gtk,Gdk;print(Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD).wait_for_text())"],env=a.env,text=True)
    a.must(report.startswith("Cortex Workspace diagnostics.") and "Runtime:" in report and "Result:" in report, "diagnostics-copy")
    a.link("/settings/app/appearance")
    a.must(a.query("""const e=document.querySelector('[aria-label="Desktop notifications"]');return JSON.stringify(e.disabled&&document.body.innerText.includes('Desktop notification delivery is not implemented in this build'));"""), "notifications-disabled-exact-reason")
    a.click("Back to workspace")
    a.link(a.board)
    for _ in range(10):
        a.must(a.service.poll() is None and visible_text(a,"Connected") and not visible_text(a,"unavailable"), "connected-poll")
        time.sleep(1)
    a.click("Project actions")
    a.click("Remove project…")
    a.click("Remove")
    a.wait(lambda:a.js("return location.pathname;")=="/m/local", "project-removed")
    a.must((a.repo/".cortex-dev/cortex.sqlite3").exists() and (a.repo/"worktree-cwd.txt").exists(), "remove-keeps-history-files")
    a.add(a.repo)
    a.wait(lambda:visible_text(a,"Activate") or visible_text(a,"Checked-change"), "readded-registration")
    a.click("Activate", optional=True)
    a.wait(lambda:visible_text(a,"Checked-change"), "readd-keeps-task")
    a.snapshot("owner-controls-finished")


def batched(a):
    a.start(True)
    a.fresh_project()
    a.click("New task")
    a.fill("Describe the task","batched-questions")
    a.click("Start now")
    a.wait(lambda:visible_text(a,"Question 1"), "batch-open", 35)
    a.link("/m/local/inbox")
    a.wait(lambda:visible_text(a,"Batched-questions") and not visible_text(a,"Nothing needs you right now"), "nonempty-inbox")
    a.snapshot("inbox-questions")
    # Open the task from its registered project; answer the real pending batch.
    a.link(a.board)
    open_card(a,"Batched-questions")
    for i in range(1,9):
        a.wait(lambda:visible_text(a,"Question "+str(i)), "batch-question-"+str(i))
        a.focus()
        a.keys(str(i))
        time.sleep(.5)
    a.wait(lambda:visible_text(a,"Result v1"), "batch-completed", 45)
    a.must(all(visible_text(a,"Choice "+str(i)) for i in range(1,9)), "numeric-shortcuts-one-through-eight")
    a.snapshot("batch-finished")


def press_styles(a):
    a.start(True)
    a.fresh_project()
    a.new_task("checked-change")
    for theme in ("Dark","Light"):
        a.click("Settings")
        a.link("/settings/app/appearance")
        a.click(theme)
        a.click("Back to workspace")
        a.link(a.board)
        open_card(a,"Checked-change")
        for name in ("Sidebar","Inspector","Worker pane"):
            selector='.titlebar [aria-label="'+name+'"]'
            a.hover(selector)
            def fact():
                return a.query("const e=document.querySelector("+json.dumps(selector)+");const s=getComputedStyle(e),r=e.getBoundingClientRect();return JSON.stringify([s.color,s.backgroundColor,s.boxShadow,s.outlineStyle,s.fontWeight,r.width,r.height]);")
            before=fact()
            subprocess.run(["xdotool","mousedown","1"],env=a.env,check=True)
            try:
                pressed=fact()
            finally:
                subprocess.run(["xdotool","mouseup","1"],env=a.env,check=True)
            a.must(before==pressed, "native-held-pointer-"+theme+"-"+name)
            a.click(name)
    a.snapshot("native-press-finished")


def git_upstream(a):
    a.start(False)
    a.fresh_project(False)
    seed=a.scratch/"upstream-source"
    bare=a.scratch/"upstream.git"
    a.git("clone","-q",str(a.repo),str(seed))
    baseline="".join("line "+str(n)+"\n" for n in range(30))
    (seed/"a.txt").write_text(baseline)
    a.git("add","a.txt",repo=seed)
    a.git("commit","-qm","Incoming local change",repo=seed)
    a.git("clone","--bare","-q",str(seed),str(bare))
    a.git("remote","add","origin",str(bare))
    a.git("config","branch.main.remote","origin")
    a.git("config","branch.main.merge","refs/heads/main")
    a.click("Git")
    a.click("Fetch")
    a.wait(lambda:a.git("rev-parse","origin/main")==a.git("rev-parse","main",repo=seed), "local-fetch")
    a.wait(lambda:a.query("""return JSON.stringify([...document.querySelectorAll('button')].some(e=>(e.getAttribute('aria-label')==='Pull'||e.title==='Pull')&&e.getAttribute('aria-disabled')!=='true'));"""), "upstream-pull-enabled")
    a.js("""[...document.querySelectorAll('button')].find(e=>(e.getAttribute('aria-label')==='Pull'||e.title==='Pull')).click();return 'yes';""")
    a.wait(lambda:(a.repo/"a.txt").read_text()==baseline, "local-pull-readback")
    a.must(a.git("remote","get-url","origin")==str(bare), "no-network-remote")
    changed=baseline.replace("line 1\n","head edit\n").replace("line 28\n","tail edit\n")
    (a.repo/"a.txt").write_text(changed)
    a.wait(lambda:a.query("return JSON.stringify(!!document.querySelector('.ws-gf'));"), "dirty-file-row")
    a.js("document.querySelector('.ws-gf').click();return 'yes';")
    a.click("Inline diff")
    a.wait(lambda:a.query("return JSON.stringify(document.querySelectorAll('.hunk').length===2);"), "two-hunks")
    a.hover(".hunk")
    a.click("Stage hunk")
    a.wait(lambda:"head edit" in a.git("diff","--cached") and "tail edit" not in a.git("diff","--cached"), "stage-one-hunk")
    a.click("Unstage hunk")
    a.wait(lambda:not a.git("diff","--cached"), "unstage-one-hunk")
    a.click("Revert hunk")
    a.click("Revert")
    a.wait(lambda:"head edit" not in (a.repo/"a.txt").read_text() and "tail edit" in (a.repo/"a.txt").read_text(), "revert-one-hunk")
    a.wait(lambda:a.query("return JSON.stringify(!document.querySelector(\'dialog[open]\'));"), "revert-dialog-acknowledged")
    a.click("Git")
    a.click("Create branch here")
    a.wait(lambda:a.query("return JSON.stringify(!!document.querySelector(\'dialog[open] input\'));"), "branch-dialog-ready")
    a.fill("Branch name","owner-branch")
    a.click("Create branch")
    a.wait(lambda:"owner-branch" in a.git("branch","--format=%(refname:short)"), "branch-readback")
    a.snapshot("git-upstream-finished")


def feedback(a):
    a.start(True)
    a.fresh_project()
    path=a.new_task("checked-change")
    task=path.split("/t/")[1].split("/")[0]
    a.click("Changes")
    a.click("Accept v1")
    a.wait(lambda:a.task_record(task).get("acceptance"), "accepted-before-feedback")
    a.link(path)
    a.click("Request changes…")
    a.fill("What should change","Inspect the result again without merging")
    a.click("Send and reopen")
    a.wait(lambda:not a.task_record(task).get("acceptance"), "feedback-clears-acceptance")
    a.wait(lambda:visible_text(a,"Inspect the result again without merging"), "feedback-recorded", 25)
    a.record("feedback-new-result", outcome="needs_real_engine", reason="The fake coordinator deliberately only acknowledges completed-task feedback; it does not publish a new result.")
    a.must(a.git("rev-list","--count","main")=="1", "feedback-does-not-merge")
    a.snapshot("feedback-finished")


def comments(a):
    a.start(True)
    a.fresh_project()
    path=a.new_task("checked-change")
    task=path.split("/t/")[1].split("/")[0]
    a.wait(lambda:a.task_record(task).get("status")=="ready_to_merge", "comment-result-ready")
    a.click("Changes")
    a.wait(lambda:visible_text(a,"Review the diff"), "ready-panel-confirmed")
    a.click("Review the diff")
    a.wait(lambda:a.query("return JSON.stringify(!!document.querySelector('.dl.add'));"), "current-commentable-diff")
    a.hover(".dl.add")
    a.click("Comment on line 1")
    a.wait(lambda:a.query("return JSON.stringify(!!document.querySelector(\'[aria-label=Comment]\'));"), "comment-field-ready")
    a.fill("Comment","Review this exact line")
    a.click("Save comment")
    a.wait(lambda:visible_text(a,"Your draft comment"), "comment-saved")
    a.js("""[...document.querySelectorAll('.chg-foot button')].find(e=>e.textContent.includes('Send')).focus();return 'yes';""")
    a.keys("ctrl+Return")
    a.wait(lambda:visible_text(a,"Sent"), "diff-comments-shortcut")
    a.snapshot("comments-finished")


def answer_queue_os(a):
    from empty_desktop_audit import Audit
    from stop_reports_native import run
    a.start_service = Audit.start_service.__get__(a)
    run(a, answer_queue=True)


def browse(a):
    native = r"""
static GtkWidget *test_button;
static void test_walk(GtkWidget *widget, const char *label) {
  if (GTK_IS_BUTTON(widget) && gtk_widget_get_visible(widget)) {
    const char *text = gtk_button_get_label(GTK_BUTTON(widget));
    if (text && g_str_equal(text, label)) test_button = widget;
  }
  if (GTK_IS_CONTAINER(widget)) {
    GList *children = gtk_container_get_children(GTK_CONTAINER(widget));
    for (GList *p = children; p; p = p->next) test_walk(GTK_WIDGET(p->data), label);
    g_list_free(children);
  }
}
static void test_native_button(const char *label) {
  test_button = NULL;
  GList *windows = gtk_window_list_toplevels();
  for (GList *p = windows; p; p = p->next)
    if (gtk_widget_get_visible(GTK_WIDGET(p->data))) test_walk(GTK_WIDGET(p->data), label);
  g_list_free(windows);
  if (test_button) { gtk_button_clicked(GTK_BUTTON(test_button)); publish("yes"); }
  else publish("missing");
}
"""
    def probe(source):
        source = source.replace("static gboolean poll_request", native+"\nstatic gboolean poll_request")
        return source.replace('if (g_str_equal(body, "@native")) native();', 'if (g_str_has_prefix(body, "@gtk-button ")) test_native_button(body+12); else if (g_str_equal(body, "@native")) native();')
    a.probe_transform=probe
    a.start(False)
    def native_button(label):
        a.wait(lambda:a.js("@gtk-button "+label)=="yes", "owned-gtk-button-"+label, 8)
    project=a.project()
    a.click("Add project")
    a.option("Browse")
    time.sleep(.5)
    a.keys("ctrl+l")
    a.typed(str(project))
    native_button("Choose")
    a.wait(lambda:"/p/" in a.js("return location.pathname;"), "browse-registers-real-folder")
    registry=next((a.scratch/"state").rglob("projects.json"))
    records=json.loads(registry.read_text())
    a.must(len(records)==1 and records[0]["canonicalRoot"]==str(project), "native-folder-chooser-project")
    a.click("Add project")
    a.option("Browse")
    native_button("Cancel")
    a.keys("Escape")
    a.wait(lambda:not a.query("return JSON.stringify(!!document.querySelector('dialog[open]'));"), "browse-cancel")
    a.must(json.loads(registry.read_text())==records, "browse-cancel-keeps-registry")
    a.snapshot("browse-finished")


def automatic_sprint(a):
    a.start(True)
    a.fresh_project()
    a.click("New task")
    a.click("Sprint")
    a.fill("Describe the sprint","Automatic question and dependency audit")
    a.click("Start planning")
    a.wait(lambda:visible_text(a,"Plan two dependent tasks?"), "auto-plan-question", 35)
    a.focus()
    a.keys("1")
    a.wait(lambda:visible_text(a,"Create 2 tasks"), "auto-plan-ready", 35)
    a.click("Create 2 tasks from Plan v1")
    a.wait(lambda:visible_text(a,"Start task 1"), "auto-plan-materialized")
    ids=a.product()[0]["sprints"][0]["taskIds"]
    a.click("Automatic")
    a.wait(lambda:visible_text(a,"Run sprint"), "auto-run-control")
    a.click("Run sprint")
    a.link(a.board)
    open_card(a,"Question-first")
    a.wait(lambda:visible_text(a,"Continue the first task?"), "auto-first-question", 35)
    with sqlite3.connect(a.repo/".cortex-dev/cortex.sqlite3") as db:
        started=db.execute("SELECT COUNT(*) FROM workspace_executions WHERE task_id=?",(ids[1],)).fetchone()[0]
    a.must(started==0, "automatic-dependency-not-started-before-answer")
    a.focus()
    a.keys("1")
    a.wait(lambda:all(a.task_record(i).get("status")=="ready_to_merge" for i in ids), "auto-resumes-after-answer-and-runs-dependent", 60)
    a.must(a.git("rev-list","--count","main")=="1", "automatic-does-not-merge")
    a.snapshot("automatic-sprint-finished")


def preferences(a):
    a.start(False)
    a.fresh_project(False)
    a.click("Settings")
    a.link("/settings/machine/local/overview")
    a.fill("Machine name","Audit machine")
    a.wait(lambda:visible_text(a,"Audit machine"), "machine-renamed")
    a.link("/settings/app/appearance")
    a.must(a.query("""return JSON.stringify([...document.querySelectorAll('[aria-label=Theme] button')].some(e=>e.textContent==='System'&&e.getAttribute('aria-current')==='true'));"""), "system-theme-default")
    a.click("Dark")
    a.click("System")
    a.must(a.query("""return JSON.stringify(document.documentElement.dataset.theme===(matchMedia('(prefers-color-scheme: dark)').matches?'dark':'light'));"""), "system-theme-observed-webview")
    a.restart_window()
    a.click("Settings")
    a.link("/settings/machine/local/overview")
    a.wait(lambda:a.js("""return document.querySelector('[aria-label="Machine name"]').value;""")=="Audit machine", "machine-facts-reloaded")
    a.must(a.js("""return document.querySelector('[aria-label="Machine name"]').value;""")=="Audit machine", "machine-rename-reopen")
    a.link("/settings/app/appearance")
    a.must(a.query("""return JSON.stringify([...document.querySelectorAll('[aria-label=Theme] button')].some(e=>e.textContent==='System'&&e.getAttribute('aria-current')==='true'));"""), "system-theme-reopen")
    a.snapshot("preferences-finished")


def matrix(a):
    for stage in ("comments","retention","unmerged","routing","shortcuts","styles","profiles","git","pr","sprint","picker","editor","resources","mcp","queue","worker_controls","owner_controls","batched","press_styles","git_upstream","feedback"):
        child=Finish("/",a.evidence/stage)
        try:
            globals()[stage](child)
        except Exception:
            child.snapshot("failed")
            raise
        finally:
            child.close()
        a.record("matrix-row",row=stage,passed=True)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--evidence", required=True)
    p.add_argument("--stage", required=True)
    args = p.parse_args()
    a = Finish("/", args.evidence)
    try:
        globals()[args.stage](a)
    except Exception as error:
        a.record("journey-error", type=type(error).__name__, message=str(error)[:180])
        try:
            a.snapshot("failed")
        except Exception:
            pass
        raise
    finally:
        a.close()


if __name__ == "__main__":
    main()
