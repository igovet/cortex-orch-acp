"""Text-only audit aid for an extracted dev desktop; never seeds product rows.

The packaged desktop/service/helper are unchanged. The optional fake launcher
replaces only engines. All product mutations below go through visible controls.
No screenshot or image decoding is performed. Run with an explicit package root.
"""
import argparse
import importlib.util
import itertools
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]


def stop(process):
    if process is None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
        process.wait(timeout=5)
    except ProcessLookupError:
        pass
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=5)


class Audit:
    def __init__(self, package, evidence):
        self.package = Path(package).resolve()
        self.evidence = Path(evidence).resolve()
        self.evidence.mkdir(parents=True, exist_ok=False)
        self.scratch = Path(tempfile.mkdtemp(prefix="cw-empty-audit-"))
        self.processes = []
        self.seq = itertools.count()
        self.events = []
        for name in ("home", "config", "state", "cache", "runtime", "probe"):
            (self.scratch / name).mkdir(mode=0o700)
        self.env = {
            "PATH": "/usr/local/bin:/usr/bin:/bin", "LANG": "C.UTF-8",
            "HOME": str(self.scratch / "home"),
            "XDG_CONFIG_HOME": str(self.scratch / "config"),
            "XDG_STATE_HOME": str(self.scratch / "state"),
            "XDG_CACHE_HOME": str(self.scratch / "cache"),
            "XDG_RUNTIME_DIR": str(self.scratch / "runtime"),
            "PYTHONDONTWRITEBYTECODE": "1", "NO_AT_BRIDGE": "1",
            "GDK_BACKEND": "x11",
        }
        self.runtime = self.package / "usr/lib/cortex-workspace-dev/runtime"
        self.bin = self.package / "usr/bin"
        self.app = self.service = None

    def spawn(self, argv, **kwargs):
        p = subprocess.Popen(argv, env=self.env, start_new_session=True,
                             stdin=subprocess.DEVNULL, **kwargs)
        self.processes.append(p)
        return p

    def record(self, step, **facts):
        row = {"step": step, **facts}
        self.events.append(row)
        (self.evidence / "observations.json").write_text(json.dumps(self.events, indent=2))
        print(json.dumps(row), flush=True)

    def wait(self, fn, label, seconds=20):
        deadline = time.monotonic() + seconds
        last = None
        while time.monotonic() < deadline:
            try:
                value = fn()
                if value:
                    return value
            except (TimeoutError, RuntimeError) as error:
                last = type(error).__name__
            time.sleep(.1)
        raise RuntimeError(f"wait:{label}:{last}")

    def js(self, body):
        probe = self.scratch / "probe"
        answer = probe / "response.json"
        answer.unlink(missing_ok=True)
        tmp = probe / f"request-{next(self.seq)}.tmp"
        tmp.write_text(body)
        tmp.rename(probe / "request.js")
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            if answer.exists():
                result = answer.read_text()
                if result.startswith("!"):
                    raise RuntimeError(result[:200])
                return result
            time.sleep(.03)
        raise TimeoutError("probe")

    def query(self, body):
        return json.loads(self.js(body))

    def snapshot(self, label):
        data = self.query("""
const seen = e => !!e.getClientRects().length && getComputedStyle(e).visibility !== 'hidden';
const controls = [...document.querySelectorAll('button,a,input,textarea,select,[role=switch]')].filter(seen).map(e => ({
 tag:e.tagName, name:e.getAttribute('aria-label')||e.textContent.trim()||e.title||e.placeholder||e.type,
 disabled:!!e.disabled||e.getAttribute('aria-disabled')==='true',reason:e.title||'',
 href:e.getAttribute('href'),value:e.matches('input,textarea,select')?e.value:undefined
}));
return JSON.stringify({path:location.pathname,viewport:[innerWidth,innerHeight],
 text:document.body.innerText,controls,theme:document.documentElement.dataset.theme});
""")
        (self.evidence / f"{label}.json").write_text(json.dumps(data, indent=2))
        self.record(label, path=data["path"], controls=len(data["controls"]),
                    disabled=[{"name":c["name"],"reason":c["reason"]} for c in data["controls"] if c["disabled"]])
        return data

    def click(self, name, optional=False):
        result = self.js(f"""
const seen=e=>!!e.getClientRects().length && getComputedStyle(e).visibility!=='hidden';
const n={json.dumps(name)};
const label=e=>{{const c=e.cloneNode(true);c.querySelectorAll('.key').forEach(x=>x.remove());return c.textContent.trim();}};
const e=[...document.querySelectorAll('button,a,[role=switch]')].filter(seen).find(e=>[e.getAttribute('aria-label'),e.textContent.trim(),label(e),e.title].includes(n));
if(!e) return 'missing'; if(e.disabled||e.getAttribute('aria-disabled')==='true') return 'disabled:'+e.title;
e.click(); return 'clicked';
""")
        self.record("click", name=name, result=result)
        if result == "missing" and not optional:
            raise RuntimeError("missing_control:" + name)
        time.sleep(.3)
        return result

    def link(self, href):
        result = self.js(f"""
const e=[...document.querySelectorAll('a')].find(e=>e.getAttribute('href')==={json.dumps(href)} && e.getClientRects().length);
if(!e)return 'missing';e.click();return 'clicked';""")
        if result != "clicked":
            raise RuntimeError("missing_link:" + href)
        time.sleep(.4)

    def fill(self, name, value):
        result = self.js(f"""
const e=[...document.querySelectorAll('input,textarea')].find(e=>(e.getAttribute('aria-label')==={json.dumps(name)}||e.closest('label')?.textContent.trim().startsWith({json.dumps(name)}))&&e.getClientRects().length);
if(!e)return 'missing';Object.getOwnPropertyDescriptor(e.constructor.prototype,'value').set.call(e,{json.dumps(value)});
e.dispatchEvent(new InputEvent('input',{{bubbles:true}}));e.dispatchEvent(new Event('change',{{bubbles:true}}));return 'filled';
""")
        if result != "filled":
            raise RuntimeError("missing_field:" + name)

    def start(self, fake=False):
        spec = importlib.util.spec_from_file_location("render_probe", ROOT / "packaging/container/check_rendered.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        source = self.evidence / "probe.c"
        source.write_text(self.probe_transform(module.PROBE) if hasattr(self, "probe_transform") else module.PROBE)
        if str(self.package) == "/":
            flags = subprocess.check_output(["pkg-config", "--cflags", "--libs", "webkit2gtk-4.1", "gtk+-3.0"], text=True).split()
            subprocess.run(["cc", "-shared", "-fPIC", str(source), "-o", str(self.evidence / "probe.so"), *flags, "-ldl"], check=True)
        else:
            subprocess.run(["docker", "run", "--rm", "--network=none", "--user", f"{os.getuid()}:{os.getgid()}",
                        "--mount", f"type=bind,source={self.evidence},target=/audit",
                        "--entrypoint", "/bin/sh", module.BUILDER, "-c",
                        "cc -shared -fPIC /audit/probe.c -o /audit/probe.so $(pkg-config --cflags --libs webkit2gtk-4.1 gtk+-3.0) -ldl"],
                       check=True, stdout=subprocess.DEVNULL)
        self.display = self.spawn(["Xvfb", "-displayfd", "1", "-screen", "0", getattr(self, "screen", "1600x1000x24"), "-nolisten", "tcp"],
                                 stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        self.env["DISPLAY"] = ":" + self.display.stdout.readline().decode().strip()
        self.wm = self.spawn(["openbox"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) if str(self.package) == "/" else None
        self.bus = self.spawn(["dbus-daemon", "--session", "--print-address", "--nofork", "--nopidfile"],
                             stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        self.env["DBUS_SESSION_BUS_ADDRESS"] = self.bus.stdout.readline().decode().strip()
        self.record("isolated_scratch", location=str(self.scratch))
        self.start_service(fake)
        old = dict(self.env)
        self.env.update(LD_PRELOAD=str(self.evidence / "probe.so"), CW_PROBE_DIR=str(self.scratch / "probe"))
        self.app = self.spawn([str(self.bin / "cortex-workspace-dev")], stdout=subprocess.DEVNULL,
                              stderr=(self.scratch / "app.stderr").open("ab"), cwd=self.scratch / "home")
        self.env = old
        self.wait(lambda:self.js("return !!document.querySelector('.titlebar')?'yes':''")=="yes","app",40)
        self.record("empty_start", registry_existed=False, seeded_rows=0, fake_engine=fake,
                    package=str(self.package), scratch=str(self.scratch))

    def start_service(self, fake=False):
        extra = ["--fixture-launcher", str(ROOT / "tests/test_workspace_journey.py")] if fake else []
        # The fake launcher imports the current runtime from the checkout, not an old installed version.
        runtime = ROOT if fake else self.runtime
        self.service = self.spawn([str(self.bin / "cortex-workspace-service-dev"), str(self.scratch),
                                   sys.executable, str(runtime), *extra],
                                  stdout=subprocess.DEVNULL, stderr=(self.scratch / "service.stderr").open("ab"))
        self.wait(lambda:(self.scratch / "runtime/cortex-workspace-dev/service.sock").exists(),"service")

    def project(self):
        p = self.scratch / "projects/owner-repo"
        p.mkdir(parents=True)
        (p / "README.md").write_text("# Empty owner project\n")
        (p / "a.txt").write_text("Disposable notes.\n")
        for args in (["init","-q","-b","main"],["add","README.md","a.txt"],
                     ["-c","user.name=Audit","-c","user.email=audit@example.invalid","commit","-q","-m","Initial history"]):
            subprocess.run(["git",*args],cwd=p,env=self.env,check=True,capture_output=True)
        assert not (p/".cortex").exists() and not (p/".cortex-dev").exists()
        self.repo=p
        return p

    def option(self, title):
        result = self.js(f"""
const e=[...document.querySelectorAll('dialog[open] [role=option]')].find(e=>e.querySelector('.grow').firstChild.textContent==={json.dumps(title)});
if(!e)return 'missing';e.click();return 'clicked';""")
        if result != "clicked":
            raise RuntimeError("missing_option:" + title)
        time.sleep(.2)

    def rows(self):
        return self.query("""
return JSON.stringify([...document.querySelectorAll('dialog[open] [role=option]')].map(e=>({
 title:e.querySelector('.grow').firstChild.textContent,text:e.textContent,selected:e.getAttribute('aria-selected')==='true'})));""")

    def keys(self, *names):
        subprocess.run(["xdotool","key","--clearmodifiers",*names],env=self.env,check=True)
        time.sleep(.15)

    def typed(self, text):
        subprocess.run(["xdotool","type","--clearmodifiers","--delay","20",text],env=self.env,check=True)

    def add(self, path):
        self.click("Add project")
        self.option("Search for folder")
        self.fill("Search for folder", str(path))
        self.wait(self.rows,"picker-row")
        self.keys("Return")
        time.sleep(2)

    def close(self):
        for p in reversed(self.processes):
            stop(p)
        self.record("cleanup", owned_processes_stopped=all(p.poll() is not None for p in self.processes))


def visible_text(a, needle):
    return a.js(f"return document.body.innerText.includes({json.dumps(needle)})?'yes':''") == "yes"


def open_card(a, text):
    result = a.js(f"const e=[...document.querySelectorAll('a.card-link')].find(e=>e.textContent.includes({json.dumps(text.rstrip("."))}));if(!e)return 'missing';e.click();return 'yes';")
    if result != "yes":
        raise RuntimeError("card_missing:" + text)
    time.sleep(.5)


def tasks(a):
    a.start(True)
    a.snapshot("00-empty")
    a.add(a.project())
    a.click("Activate")
    a.wait(lambda:not visible_text(a,"Activate"),"activated")
    a.click("New task")
    a.snapshot("10-new-task")
    a.fill("Describe the task","Explain this disposable project.")
    a.click("Save as draft")
    a.wait(lambda: a.js("return !!document.querySelector('.card-link')?'yes':''")=="yes","draft")
    a.snapshot("10b-saved-board")
    a.record("draft_no_engine_turn", no_turn_file=not (a.repo/"turns").exists())
    open_card(a,"Explain this disposable project.")
    a.snapshot("11-task-draft")
    a.click("Click to rename")
    a.fill("Task title","Owner audit task")
    a.js("document.querySelector('[aria-label=\"Task title\"]').dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true}));return 'yes';")
    time.sleep(.5)
    a.snapshot("12-renamed")
    a.click("Start")
    a.wait(lambda:visible_text(a,"Which direction"),"question")
    a.snapshot("13-question")
    a.click("…Other answer",optional=True)
    if not a.js("return !!document.querySelector('[aria-label=\"Your answer\"]')?'yes':''")=="yes":
        a.click("… Other answer")
    a.fill("Your answer","first line\nsecond line")
    a.snapshot("14-multiline")
    a.click("Send answer")
    a.wait(lambda:(a.repo/"streaming").exists() or visible_text(a,"changed on the machine"),"answer")
    if not (a.repo/"streaming").exists():
        a.click("Send answer")
        a.wait(lambda:(a.repo/"streaming").exists(),"explicit-answer-retry")
    a.snapshot("15-working")
    a.fill("Message","Queued from the audit")
    a.click("Queue")
    a.wait(lambda:a.js("return !!document.querySelector('.queue')?'yes':''")=="yes","queue")
    a.snapshot("16-queue")
    a.js("const e=[...document.querySelectorAll(\'.queue button\')].find(e=>e.textContent.trim()===\'Edit\');if(!e)throw Error(\'queue edit missing\');e.click();return \'yes\';")
    a.wait(lambda:a.js("""return !document.querySelector('.queue') && document.querySelector('[aria-label="Message"]').value==='Queued from the audit'?'yes':''""")=="yes","queue-edit-receipt")
    a.snapshot("17-edit")
    a.record("queue_edit", draft=a.js("return document.querySelector('[aria-label=\"Message Cortex\"]').value;"),
             queue_removed=a.js("return !document.querySelector('.queue')?'yes':'no'"))
    a.fill("Message","Focus on durable conversation")
    a.click("Steer")
    a.snapshot("18-steer")
    (a.repo/"release-stream").touch()  # Releases only the fake engine's turn barrier.
    a.wait(lambda:visible_text(a,"Runtime-owned journey result."),"result",30)
    a.snapshot("19-result")
    tabs=a.query("return JSON.stringify([...document.querySelectorAll('.tabs a')].map(e=>e.getAttribute('href')));")
    for i,href in enumerate(tabs):
        a.link(href)
        a.snapshot(f"task-tab-{i}")
    a.link(tabs[0])
    a.click("Task actions")
    a.snapshot("20-task-menu")
    a.js("document.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}));return 'yes';")
    # No rows are seeded; the next task asks the fake engine to delegate through real Cortex tools.
    (a.repo/"start-worker-scenario").touch()
    a.click("Task actions")
    a.link("/m/local/new")
    a.fill("Describe the task","Exercise worker controls")
    a.click("Save as draft")
    a.wait(lambda:a.js("return [...document.querySelectorAll('.card-link')].some(e=>e.textContent.includes('Exercise worker controls'))?'yes':''")=="yes","worker-draft")
    open_card(a,"Exercise worker controls")
    a.click("Start")
    a.wait(lambda:(a.repo/"worker-one").exists() and (a.repo/"worker-two").exists(),"real-dispatch",30)
    a.click("Workers")
    a.wait(lambda:visible_text(a,"Receive owner guidance"),"worker-rows")
    a.snapshot("21-live-workers")
    hrefs=a.query("return JSON.stringify([...document.querySelectorAll('.wcard')].map(e=>({text:e.textContent,href:e.getAttribute('href')})));")
    one=next(x["href"] for x in hrefs if "Receive owner guidance" in x["text"])
    two=next(x["href"] for x in hrefs if "Wait for command control" in x["text"])
    a.link(one)
    a.fill("Message backend-dev","Keep the worker message verbatim\nsecond line")
    a.click("Send to backend-dev")
    a.wait(lambda:(a.repo/"worker-guidance-delivered").exists(),"worker-message")
    a.wait(lambda:visible_text(a,"Worker guidance completed."),"worker-result",30)
    a.snapshot("22-finished-worker")
    a.click("Show full history",optional=True)
    a.snapshot("23-finished-history")
    a.link(two)
    a.wait(lambda:visible_text(a,"Stop command…"),"command-running",20)
    a.click("Stop command…")
    a.js("const e=document.querySelector('dialog[open] textarea');e.value='Focused owner stop';e.dispatchEvent(new Event('input',{bubbles:true}));return 'yes';")
    a.click("Stop command")
    a.wait(lambda:(a.repo/"worker-command-stopped").exists(),"command-stopped")
    a.snapshot("24-command-stopped")
    a.click("Stop…")
    a.click("Stop worker")
    a.wait(lambda:visible_text(a,"Cancelled"),"worker-cancelled")
    a.snapshot("25-worker-cancelled")


def section(a, name, fn):
    try:
        fn()
    except Exception as error:
        a.record("section_incomplete", section=name, type=type(error).__name__, message=str(error)[:150])
        a.snapshot(name+"-incomplete")
        a.js("const e=[...document.querySelectorAll(\'dialog[open] button\')].find(e=>[\'Cancel\',\'Stay\'].includes(e.textContent.trim()));if(e)e.click();return \'yes\';")


def savebar(a):
    names=a.query("return JSON.stringify([...document.querySelectorAll('.savebar button')].map(e=>e.textContent.trim()).filter(t=>t.startsWith('Save ')));")
    if len(names)!=1:
        raise RuntimeError("ambiguous_savebar")
    a.click(names[0])
    time.sleep(1)
    a.snapshot("saved-"+str(next(a.seq)))
    if visible_text(a,"unsaved change"):
        a.record("save_rejected", reason="Draft remains unsaved; see the preceding captured receipt.")
        a.click("Discard")


def code_git_terminal(a, board):
    a.link(board)
    a.click("Code")
    a.snapshot("code-file-tree")
    a.click("README.md",optional=True)
    a.snapshot("code-open-file")
    a.record("code_edit_controls", textarea_count=a.js("return String(document.querySelectorAll('.ed textarea,.ed [contenteditable=true]').length);"))
    a.click("Search")
    a.fill("Search files","Empty owner")
    a.snapshot("code-search")
    a.click("Terminal")
    a.wait(lambda:visible_text(a,"Running"),"shell-running")
    a.js("@focus")
    a.js("document.querySelector('.xterm-helper-textarea').focus();return 'yes';")
    subprocess.run(["xdotool","type","--clearmodifiers","--delay","2","--","printf 'OWNER_AUDIT_TERMINAL\\n'; printf 'owner work\\n' > owner-note.txt; git config user.name Audit; git config user.email audit@example.invalid"],env=a.env,check=True)
    subprocess.run(["xdotool","key","--clearmodifiers","Return"],env=a.env,check=True)
    a.wait(lambda:(a.repo/"owner-note.txt").exists(),"shell-output-file")
    a.snapshot("terminal-roundtrip")
    a.click("Hide terminal",optional=True)
    a.link(board)
    a.click("Git")
    a.wait(lambda:visible_text(a,"owner-note.txt"),"git-dirty")
    a.snapshot("git-dirty")
    a.js("@focus")
    a.js("""const e=[...document.querySelectorAll('.ws-gf')].find(e=>e.textContent.includes('owner-note.txt'));e.focus();return 'yes';""")
    subprocess.run(["xdotool","key","--clearmodifiers","Tab"],env=a.env,check=True)
    a.snapshot("git-keyboard-actions")
    a.click("Stage owner-note.txt")
    time.sleep(1)
    a.fill("Commit message","Owner changes through desktop")
    time.sleep(1)
    a.snapshot("git-message-filled")
    a.click("Commit")
    time.sleep(1)
    a.snapshot("git-after-commit")
    last=subprocess.check_output(["git","log","-1","--format=%s"],cwd=a.repo,env=a.env,text=True).strip()
    a.record("git_commit_readback", observed_subject=last)
    a.click("Create branch here")
    a.fill("Branch name","owner-branch")
    a.click("Create branch")
    time.sleep(1)
    a.snapshot("git-branch-created")
    a.record("branch_readback", branches=subprocess.check_output(["git","branch","--format=%(refname:short)"],cwd=a.repo,env=a.env,text=True).splitlines())


def extras(a):
    a.start(False)
    a.add(a.project())
    board=a.js("return location.pathname;")
    a.click("Activate")
    time.sleep(3)
    for label in ("Board","List","Worktrees","Knowledge and skills","Settings"):
        section(a,"project-"+label.replace(" ","-"),lambda label=label:(a.link(board),a.click(label),a.snapshot("project-"+label.replace(" ","-"))))
    section(a,"code-git-terminal",lambda:code_git_terminal(a,board))
    a.click("Settings")
    a.link("/settings/machine/local/orchestration/tools")
    def mcp_extra():
        a.click("Add server")
        a.fill("Server name","audit")
        a.fill("Command","/usr/bin/python3")
        a.click("Add argument")
        a.fill("Argument 1","/checkout/tests/workspace_support/discovery_only_mcp.py")
        a.click("Next: Connect")
        a.click("Connect and list tools")
        time.sleep(8)
        a.snapshot("mcp-connect-final")
        if visible_text(a,"Enter tool names by hand"):
            a.fill("Tool name","audit_read")
            a.js("""document.querySelector('[aria-label="Tool name"]').dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true}));return 'yes';""")
        a.click("Go to Tool access",optional=True)
        a.snapshot("mcp-matrix-final")
        if not visible_text(a,"Enter tool names by hand"):
            a.js("""const e=document.querySelector('input[aria-label="audit_read, writing workers, not granted"]');if(e)e.click();return 'yes';""")
        a.click("Save, keep off",optional=True)
        time.sleep(1)
        a.snapshot("mcp-save-final")
    section(a,"mcp-extra",mcp_extra)
    a.click("Back to workspace")
    a.link(board)
    a.click("New task")
    a.fill("Describe the task","Task requires only its description.")
    a.snapshot("newtask-controls")
    a.click("Voice input",optional=True)
    time.sleep(3)
    a.snapshot("voice-first-use")
    a.click("Cancel dictation",optional=True)
    # Do not save anything dictated or execute a paid engine.
    a.click("Cancel",optional=True)
    a.click("Home",optional=True)
    a.click("Add project")
    missing=a.scratch/"projects/new-folder"
    a.option("Search for folder")
    a.fill("Search for folder",str(missing))
    a.wait(a.rows,"missing-folder-row")
    a.keys("Return")
    time.sleep(1)
    a.snapshot("missing-project-folder")
    a.option("Create this folder")
    a.wait(lambda:missing.exists(),"new-folder-created")
    a.snapshot("created-project-folder")
    a.click("Project actions")
    a.click("Remove project…")
    a.snapshot("remove-project-confirm")
    a.click("Remove")
    time.sleep(1)
    a.snapshot("removed-project")
    a.record("removed_project_files_kept", folder_exists=missing.exists())


def editor(a):
    a.start(False)
    a.add(a.project())
    board=a.js("return location.pathname;")
    a.click("Activate")
    time.sleep(2)
    code_git_terminal(a,board)


def drop_text(a, selector, name, text):
    a.js(f"""
const target=document.querySelector({json.dumps(selector)});if(!target)throw Error('drop target missing');
const data=new DataTransfer();data.items.add(new File([{json.dumps(text)}],{json.dumps(name)},{{type:'text/plain'}}));
target.dispatchEvent(new DragEvent('drop',{{bubbles:true,cancelable:true,dataTransfer:data}}));return 'yes';
""")


def flows(a):
    a.start(True)
    a.add(a.project())
    board=a.js("return location.pathname;")
    a.click("Activate")
    time.sleep(1)
    a.click("New task")
    a.fill("Describe the task","Explain this disposable project.")
    a.click("Save as draft")
    a.wait(lambda:a.js("return !!document.querySelector('.card-link')?'yes':''")=="yes","draft")
    open_card(a,"Explain this disposable project.")
    a.click("Start")
    a.wait(lambda:visible_text(a,"Which direction"),"question")
    (a.repo/"expect-attachments").touch()
    a.click("…Other answer")
    a.fill("Your answer","first line\nsecond line")
    drop_text(a,".ws-dc-other textarea","notes.txt","answer fixture")
    a.snapshot("answer-attachment-chip")
    a.click("Send answer")
    a.wait(lambda:(a.repo/"answer-resource-received").exists() or visible_text(a,"changed on the machine"),"attachment-answer")
    if not (a.repo/"answer-resource-received").exists():
        a.click("Send answer")
        a.wait(lambda:(a.repo/"answer-resource-received").exists(),"attachment-explicit-retry")
    a.fill("Message","Focus on durable conversation")
    drop_text(a,".dock .composer textarea","queue.txt","queue fixture")
    a.click("Queue")
    a.wait(lambda:a.js("return !!document.querySelector('.queue')?'yes':''")=="yes","attachment-queue")
    a.snapshot("queue-attachment-chip")
    a.js("""[...document.querySelectorAll('.queue button')].find(e=>e.textContent.trim()==='Edit').click();return 'yes';""")
    a.wait(lambda:a.js("""return !document.querySelector('.queue')&&document.querySelector('.dock .composer .ws-att')?'yes':''""")=="yes","queue-edit-attachments")
    a.snapshot("queue-edit-attachments")
    a.click("Steer")
    (a.repo/"release-stream").touch()
    a.wait(lambda:(a.repo/"resources-received").exists(),"real-resource-readback",25)
    a.record("attachments_received", source_bytes_checked_by_fake_engine=True, native_drag_or_clipboard=False)
    a.wait(lambda:visible_text(a,"Runtime-owned journey result."),"result")
    a.click("Task actions")
    a.click("Close task…")
    a.snapshot("close-as-is-dialog")
    a.click("Close task")
    time.sleep(1)
    a.snapshot("closed-as-is")
    a.click("Task actions")
    a.js("""const e=[...document.querySelectorAll('[role=menuitem]')].find(e=>e.textContent.trim()==='Archive');if(!e)throw Error('archive menu item missing');e.click();return 'yes';""")
    time.sleep(2)
    a.snapshot("archive-attempt")
    a.js("""const e=[...document.querySelectorAll('dialog[open] button')].find(e=>e.textContent.includes('Keep')||e.textContent.includes('Cancel'));if(e)e.click();return 'yes';""")
    a.click("Home",optional=True)
    a.link(board)
    a.click("List")
    a.snapshot("list-with-task")
    a.link(board)
    a.click("New task")
    a.click("Sprint")
    a.fill("Describe the sprint","Plan a tiny owner sprint.")
    a.snapshot("sprint-creation")
    a.click("Start planning")
    time.sleep(2)
    a.snapshot("sprint-after-save")
    # Only controls actually present are exercised; no plan is injected.
    for name in ("Start planning","Start planning…","Run sprint","Automatic","Manual"):
        if a.click(name,optional=True)=="clicked":
            time.sleep(1)
            a.snapshot("sprint-"+name.replace(" ","-"))
    a.record("sprint_end", observed_text=a.js("return document.querySelector('.main')?.innerText||'';")[:1200])


def stop_cancel(a):
    a.start(True)
    a.add(a.project())
    a.click("Activate")
    time.sleep(1)
    a.click("New task")
    a.fill("Describe the task","Stop and cancel the owner task")
    a.click("Save as draft")
    a.wait(lambda:a.js("return !!document.querySelector('.card-link')?'yes':''")=="yes","stop-draft")
    open_card(a,"Stop and cancel")
    a.click("Start")
    a.wait(lambda:visible_text(a,"Which direction"),"stop-question")
    a.click("Stop")
    time.sleep(2)
    a.snapshot("task-after-stop")
    a.click("Task actions")
    a.click("Close task…",optional=True)
    a.snapshot("cancel-dialog")
    a.js("""const e=[...document.querySelectorAll('dialog[open] label')].find(e=>e.textContent.includes('Cancel task'));if(e)e.querySelector('input').click();return 'yes';""")
    a.click("Cancel task",optional=True)
    time.sleep(1)
    a.snapshot("task-after-cancel")


def local(a):
    a.start(False)
    a.snapshot("00-empty")
    a.add(a.project())
    board=a.js("return location.pathname;")
    project_links=a.query("return JSON.stringify([...document.querySelectorAll('.ptab')].map(e=>e.getAttribute('href')));")
    a.click("Activate")
    time.sleep(3)
    a.click("Settings")
    def preferences():
        for theme in ("Dark","Light","System"):
            a.click(theme)
            a.record("theme_changed", chosen=theme, effective=a.js("return document.documentElement.dataset.theme;"))
        a.link("/settings/app/voice")
        a.click("Provider: Not chosen")
        a.snapshot("voice-provider-options")
        a.js("document.querySelectorAll(':popover-open').forEach(e=>e.hidePopover());return 'yes';")
    section(a,"preferences",preferences)
    def defaults():
        a.link("/settings/machine/local/orchestration/defaults")
        a.click("English")
        a.snapshot("defaults-dirty")
        savebar(a)
        a.link("/settings/machine/local/orchestration/policy")
        a.fill("Attempts per task","3")
        savebar(a)
    section(a,"defaults",defaults)
    def routing():
        a.link("/settings/machine/local/orchestration/routing")
        hrefs=a.query("return JSON.stringify([...document.querySelectorAll('.tabs a')].map(e=>e.getAttribute('href')));")
        for i,href in enumerate(hrefs):
            a.link(href)
            a.snapshot("routing-subtab-"+str(i))
        a.link("/settings/machine/local/orchestration/effective")
        a.snapshot("route-preview")
    section(a,"routing",routing)
    def profiles():
        a.link("/settings/machine/local/orchestration/profiles")
        a.click("Save as new…")
        a.fill("Profile name","owner-audit")
        a.click("Save as new")
        a.wait(lambda:visible_text(a,"owner-audit"),"profile-created")
        a.snapshot("profile-created")
        a.click("Profile name: owner-audit")
        a.fill("Profile name","owner-audit-renamed")
        a.keys("Return")
        time.sleep(1)
        a.snapshot("profile-renamed")
    section(a,"profiles",profiles)
    def worktrees():
        a.link("/settings/machine/local/worktrees")
        a.snapshot("worktrees-default")
        a.fill("Worktree folder",str(a.scratch/"task-worktrees"))
        savebar(a)
        a.link("/settings/machine/local/storage")
        a.click("More: Retention")
        savebar(a)
        a.click("Run cleanup now")
        time.sleep(1)
        a.snapshot("storage-cleanup")
    section(a,"worktrees-storage",worktrees)
    def mcp():
        a.link("/settings/machine/local/orchestration/tools")
        a.click("Add server")
        a.fill("Server name","audit")
        a.fill("Command","/usr/bin/python3")
        a.snapshot("mcp-define")
        # A visible argument row, not a command or API injected into the application.
        a.click("Add argument",optional=True)
        a.fill("Argument 1","/checkout/tests/workspace_support/discovery_only_mcp.py")
        a.click("Next: Connect")
        a.click("Connect and list tools")
        time.sleep(6)
        a.snapshot("mcp-connect")
        if visible_text(a,"Go to Tool access"):
            a.click("Go to Tool access")
            a.snapshot("mcp-matrix-default")
            a.js("const e=document.querySelector('input[aria-label=\"audit_read, writing workers, not granted\"]');if(!e)throw Error('new tool not found');e.click();return 'yes';")
            a.click("Save, keep off")
            time.sleep(1)
            a.snapshot("mcp-saved")
    section(a,"mcp",mcp)
    def diagnostics():
        a.link("/settings/machine/local/diagnostics")
        a.click("Run")
        time.sleep(2)
        a.snapshot("diagnostics-result")
        a.link("/settings/machine/local/remote")
        a.js("document.querySelector('input[aria-label=\"Allow remote access\"]').click();return 'yes';")
        a.snapshot("remote-toggle")
    section(a,"diagnostics-remote",diagnostics)
    a.click("Back to workspace",optional=True)
    a.link(board)
    for i,href in enumerate(project_links):
        section(a,"project-view-"+str(i),lambda h=href,i=i:(a.link(h),a.snapshot("project-view-"+str(i))))
    section(a,"code-git-terminal",lambda:code_git_terminal(a,board))
    section(a,"home",lambda:(a.click("Home"),a.snapshot("home-project")))
    section(a,"inbox",lambda:(a.click("Inbox"),a.snapshot("inbox-empty")))
    section(a,"archive",lambda:(a.click("Archive"),a.snapshot("archive-empty")))
    def reconnect():
        stop(a.service)
        a.wait(lambda:visible_text(a,"Offline"),"offline",15)
        a.snapshot("machine-offline")
        a.start_service(False)
        a.wait(lambda:visible_text(a,"Connected") and not visible_text(a,"Offline"),"reconnect",20)
        a.snapshot("machine-reconnected")
    section(a,"reconnect",reconnect)
    a.record("config_files", files=sorted(str(p.relative_to(a.scratch)) for p in a.scratch.rglob("*.json") if p.is_file() and ".cortex/" not in str(p) and ".cortex-dev/" not in str(p)))


def journey(a, fake):
    a.start(fake)
    a.snapshot("01-empty-home")
    a.click("Add project")
    a.option("Search for folder")
    a.fill("Search for folder", "relative/path")
    time.sleep(.6)
    a.snapshot("02-relative-path")
    a.keys("Escape")
    a.keys("Escape")
    a.add(a.project())
    a.snapshot("03-project-registered")
    a.click("Activate", optional=True)
    time.sleep(4)
    a.snapshot("04-activated")
    # Discover settings through the UI, not a synthetic route.
    a.click("Settings", optional=True)
    a.snapshot("05-settings-entry")
    visited=set()
    for i in range(45):
        links=a.query("return JSON.stringify([...document.querySelectorAll('a')].filter(e=>e.getClientRects().length).map(e=>e.getAttribute('href')).filter(h=>h&&h.startsWith('/settings')));")
        href=next((h for h in links if h not in visited),None)
        if href is None: break
        visited.add(href)
        a.link(href)
        a.snapshot(f"settings-{i:02}")
    a.record("settings_visited", routes=sorted(visited))


def picker(a):
    """Add project from an empty home with real keys: fragments, typed paths, unreadable parents, refusals."""
    home=a.scratch/"home"
    def repo(path):
        path.mkdir(parents=True)
        (path/".git").mkdir()
    repo(home/"Code/cedar-notes")
    (home/"Code/cedar-app").mkdir(parents=True)
    (home/"Code/maple").mkdir(parents=True)
    repo(home/"vault/inner/locked-repo")
    (home/"Code/sealed/cedar-secret").mkdir(parents=True)
    repo(a.scratch/"disk/shared/outside-repo")
    (home/"Code/maple/back").symlink_to(home)
    (home/"vault").chmod(0o311)
    (home/"Code/sealed").chmod(0)
    failures=[]
    def must(ok,label,**facts):
        a.record("picker_"+label,ok=bool(ok),**facts)
        if not ok:
            failures.append(label)
    def look():
        return a.query("""
const d=document.querySelector('dialog[open]'),s=d&&getComputedStyle(d),i=d&&d.querySelector('input'),r=d&&d.querySelector('[aria-selected=true]');
const box=d&&d.getBoundingClientRect();
return JSON.stringify({open:!!d,height:box&&box.height,width:box&&box.width,top:box&&box.top,shadow:s&&s.boxShadow,
 text:d&&d.innerText,field:i&&i.value,focused:!!i&&document.activeElement===i,
 fieldOutline:i&&getComputedStyle(i).outlineStyle,rowOutline:r&&getComputedStyle(r).outlineStyle,rowShadow:r&&getComputedStyle(r).boxShadow,
 machine:document.querySelector('.machine .st')?.textContent});""")
    def rows_for(text,*wanted):
        started=time.monotonic()
        a.keys("ctrl+a","BackSpace")
        a.typed(text)
        titles=lambda:[r["title"] for r in a.rows()]
        a.wait(lambda:all(w in titles() for w in wanted) and "Searching..." not in (look()["text"] or ""),"rows:"+text)
        a.record("picker_rows",query=text,rows=a.rows(),seconds=round(time.monotonic()-started,2))
        return a.rows()
    def registered():
        files=list((a.scratch/"state").rglob("projects.json"))
        return [p["canonicalRoot"] for f in files for p in json.loads(f.read_text())]
    a.start(False)
    a.snapshot("picker-empty-home")
    a.click("Add project")
    first=look()
    must(first["open"] and "Search for folder" in first["text"] and "New folder" in first["text"],"ways_in",text=first["text"])
    a.option("Search for folder")
    field=look()
    must(field["focused"],"field_has_focus")
    must(field["height"]==first["height"] and field["top"]==first["top"],"page_change_moves_nothing",before=first["height"],after=field["height"])
    must(field["shadow"] in ("none",None),"no_shadow",shadow=field["shadow"])
    found=rows_for("cedar","~/Code/cedar-notes","~/Code/cedar-app")
    must("Git repository" in next(r["text"] for r in found if r["title"]=="~/Code/cedar-notes"),"git_marked")
    must("Git repository" not in next(r["text"] for r in found if r["title"]=="~/Code/cedar-app"),"plain_folder_not_marked")
    must(not any("secret" in r["text"] for r in found),"unreadable_skipped")
    busy=look()
    must("denied" not in busy["text"].lower() and busy["machine"]=="Connected","no_error_for_unreadable",machine=busy["machine"])
    must(busy["height"]==first["height"],"results_move_nothing")
    must(busy["rowOutline"] in ("none",None) and busy["fieldOutline"]=="none" and busy["rowShadow"]=="none","no_focus_marks",row=busy["rowOutline"],field=busy["fieldOutline"])
    count=len(found)
    index=lambda:[i for i,r in enumerate(a.rows()) if r["selected"]]
    must(index()==[0],"first_row_selected")
    a.keys("Down")
    must(index()==[1],"down_moves")
    a.keys("Up","Up")
    must(index()==[count-1],"up_wraps")
    a.keys("Down")
    must(index()==[0],"down_wraps")
    rows_for("~/Code/","~/Code/maple")
    must(a.rows()[0]["title"]=="~/Code","home_expands_and_folder_comes_first",rows=[r["title"] for r in a.rows()])
    rows_for("~/va","~/vault")
    opened=rows_for("~/vault/inner/lock","~/vault/inner/locked-repo")
    must(opened[0]["title"]=="~/vault/inner/locked-repo" and "Git repository" in opened[0]["text"],"found_through_unlistable_parent",rows=opened)
    a.keys("Return")
    a.wait(lambda:str(home/"vault/inner/locked-repo") in registered(),"registered_through_unlistable_parent")
    a.wait(lambda:not look()["open"],"dialog_closed")
    a.snapshot("picker-registered")
    must(a.query("return JSON.stringify({m:document.querySelector('.machine .st')?.textContent,p:location.pathname})")["m"]=="Connected","connected_after_add")
    # Palette entry, an absolute path outside the home, then the machine's refusal.
    a.keys("ctrl+k")
    a.typed(">add project")
    time.sleep(.3)
    a.keys("Return")
    time.sleep(.3)
    must("Search for folder" in (look()["text"] or ""),"palette_entry")
    a.option("Search for folder")
    outside=rows_for(str(a.scratch/"disk/shared/out"),str(a.scratch/"disk/shared/outside-repo"))
    must("Git repository" in outside[0]["text"],"absolute_path_outside_home",rows=outside)
    private=str(next((a.scratch/"state").glob("*")))
    rows_for(private,private)
    a.keys("Return")
    a.wait(lambda:"private to Cortex" in (look()["text"] or ""),"private_refusal")
    refused=look()
    must(refused["focused"] and refused["height"]==first["height"] and refused["machine"]=="Connected","refusal_keeps_everything",text=refused["text"])
    rows_for("~/Code/not-yet","~/Code/not-yet")
    a.keys("Return")
    a.wait(lambda:"does not exist yet" in (look()["text"] or ""),"missing_folder")
    a.keys("Return")
    a.wait(lambda:(home/"Code/not-yet").is_dir() and str(home/"Code/not-yet") in registered(),"created_missing")
    a.wait(lambda:not look()["open"],"created_missing_closed")
    # New folder: parent, name, then the same name again.
    for attempt in ("made","again"):
        a.click("Add project")
        a.option("New folder")
        must("Choose parent folder" in look()["text"],"parent_page")
        rows_for("~/Code/","~/Code")
        a.keys("Return")
        time.sleep(.3)
        must("Name folder" in look()["text"],"name_page")
        a.typed("fresh-project")
        a.wait(lambda:[r["title"] for r in a.rows()]==["~/Code/fresh-project"],"name_preview")
        a.keys("Return")
        if attempt=="made":
            a.wait(lambda:str(home/"Code/fresh-project") in registered(),"new_folder_registered")
            a.wait(lambda:not look()["open"],"new_folder_closed")
        else:
            a.wait(lambda:"already there" in (look()["text"] or ""),"name_taken")
    a.keys("Escape")
    must("Choose parent folder" in look()["text"],"escape_steps_back")
    a.keys("Escape")
    must("Search for folder" in look()["text"] and "Close" in look()["text"],"escape_steps_back_again")
    a.keys("Escape")
    a.wait(lambda:not look()["open"],"escape_closes")
    a.snapshot("picker-done")
    must(a.query("return JSON.stringify({m:document.querySelector('.machine .st')?.textContent})")["m"]=="Connected","connected_at_the_end")
    (home/"vault").chmod(0o700)
    (home/"Code/sealed").chmod(0o700)
    if failures:
        raise RuntimeError("picker:"+",".join(failures))
    a.record("picker_passed",registered=registered())


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package",required=True)
    parser.add_argument("--evidence",required=True)
    parser.add_argument("--fake",action="store_true")
    parser.add_argument("--stage",choices=("scan","tasks","local","extras","editor","flows","stop","picker"),default="scan")
    args=parser.parse_args()
    a=Audit(args.package,args.evidence)
    try:
        if args.stage == "tasks":
            tasks(a)
        elif args.stage == "local":
            local(a)
        elif args.stage == "extras":
            extras(a)
        elif args.stage == "editor":
            editor(a)
        elif args.stage == "flows":
            flows(a)
        elif args.stage == "stop":
            stop_cancel(a)
        elif args.stage == "picker":
            picker(a)
        else:
            journey(a,args.fake)
    except Exception as error:
        a.record("harness_error",type=type(error).__name__,message=str(error)[:200])
        raise
    finally:
        a.close()


if __name__=="__main__":
    main()
