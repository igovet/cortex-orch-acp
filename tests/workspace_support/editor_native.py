"""Text-only WebKitGTK journey of the Code editor: real keys in, real bytes read back from disk.

Engines are fake, and the service, helper, bridge and window are the real builds. No screenshot is taken.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time

from empty_desktop_audit import Audit, visible_text


def text_of(a):
    return a.js("return document.querySelector('.cm-content')?.innerText ?? '';")


def facts(a):
    return a.query("""
const e=document.querySelector('.cm-content'), save=[...document.querySelectorAll('.ecrumb button')].find(b=>(b.getAttribute('aria-label')||'').startsWith('Save '));
return JSON.stringify({label:e?.getAttribute('aria-label')??'',editable:e?.getAttribute('contenteditable')??'',status:document.querySelector('.ws-save-state')?.textContent??'',
 tabs:[...document.querySelectorAll('.etab')].map(t=>t.getAttribute('aria-label')),save:save?{off:save.getAttribute('aria-disabled')==='true',reason:save.title}:null,
 alert:document.querySelector('.ed .strip[role=alert]')?.textContent??'',warn:document.querySelector('.strip.warn:not([role=alert])')?.textContent??''});""")


def visit(a, path):
    a.js("history.pushState(null,''," + json.dumps(path) + ");dispatchEvent(new PopStateEvent('popstate'));return 'yes';")
    time.sleep(.4)


def focus(a):
    a.js("@focus")
    a.js("document.querySelector('.cm-content').focus();return 'yes';")


def type_at_end(a, text):
    focus(a)
    a.keys("ctrl+End")
    a.typed(text)
    a.wait(lambda: text in text_of(a), "typed")


def must(a, ok, step, /, **seen):
    a.record(step, ok=bool(ok), **seen)
    if not ok:
        raise AssertionError(step)


def disk(path):
    return path.read_bytes()


def code_ready(a):
    a.wait(lambda: a.js("return !!document.querySelector('.cm-content')?'yes':''") == "yes", "editor")


def edit_and_save(a, file):
    code_ready(a)
    first = facts(a)
    must(a, first["label"] == "Edit README.md" and first["editable"] == "true" and first["save"]["off"], "labelled_editor", **first)
    a.snapshot("10-open")
    type_at_end(a, "Owner typed this")
    seen = facts(a)
    must(a, seen["status"] == "Unsaved" and seen["tabs"] == ["README.md, unsaved"] and disk(file) == b"# Empty owner project\n", "unsaved_until_saved", **seen)
    a.keys("Left", "Right")
    quiet = a.query("""
const look=s=>{const c=getComputedStyle(document.querySelector(s));return {outline:c.outlineStyle,shadow:c.boxShadow};};
return JSON.stringify({keyboard:document.documentElement.hasAttribute('data-keyboard'),content:look('.cm-content'),editor:look('.cm-editor')});""")
    must(a, quiet["keyboard"] and all(v["outline"] == "none" and v["shadow"] == "none" for v in (quiet["content"], quiet["editor"])), "no_focus_mark_or_shadow_after_keyboard_movement", **quiet)
    a.keys("ctrl+s")
    a.wait(lambda: facts(a)["status"] == "Saved", "saved")
    bytes_now = disk(file)
    must(a, bytes_now == b"# Empty owner project\nOwner typed this", "bytes_read_back_from_disk", sha256=hashlib.sha256(bytes_now).hexdigest(), size=len(bytes_now), tabs=facts(a)["tabs"])


def conflict_found_on_disk(a, file):
    before = disk(file)
    type_at_end(a, " and more")
    file.write_bytes(b"A worker wrote this\n")
    a.wait(lambda: "changed on disk" in facts(a)["alert"], "disk-change-seen")
    seen = facts(a)
    must(a, seen["status"] == "Changed on disk" and seen["save"]["off"] and "Owner typed this and more" in text_of(a) and disk(file) == b"A worker wrote this\n", "conflict_keeps_owner_text_and_disk", **seen)
    a.snapshot("20-conflict")
    a.click("Keep mine")
    a.wait(lambda: facts(a)["status"] == "Saved", "kept")
    must(a, disk(file) == before + b" and more", "keep_mine_writes_owner_text", bytes=disk(file).decode())


def conflict_met_by_save(a, file):
    owner = disk(file)
    type_at_end(a, "!")
    file.write_bytes(b"Second worker text\n")
    a.keys("ctrl+s")
    a.wait(lambda: "changed on disk" in facts(a)["alert"], "save-conflict")
    must(a, disk(file) == b"Second worker text\n", "save_over_a_newer_file_wrote_nothing", bytes=disk(file).decode())
    a.click("Reload from disk")
    a.wait(lambda: "Second worker text" in text_of(a), "reloaded")
    must(a, facts(a)["tabs"] == ["README.md"] and not facts(a)["alert"], "reload_took_the_disk_version", **facts(a))
    focus(a)
    a.keys("ctrl+z")
    a.wait(lambda: "Owner typed this and more!" in text_of(a), "undo-restores-owner-text")
    must(a, facts(a)["tabs"] == ["README.md, unsaved"], "undo_brings_back_the_owner_text", **facts(a))
    a.keys("ctrl+s")
    a.wait(lambda: facts(a)["status"] == "Saved", "saved-after-undo")
    must(a, disk(file) == owner + b"!", "owner_text_saved_after_undo", bytes=disk(file).decode())


def navigation_keeps_text(a, file):
    type_at_end(a, " nav")
    a.click("Git")
    a.wait(lambda: a.js("return document.querySelector('.cm-content')?'':'yes'") == "yes", "left-editor")
    a.click("Code")
    code_ready(a)
    a.wait(lambda: " nav" in text_of(a), "text-back")
    must(a, facts(a)["tabs"] == ["README.md, unsaved"] and not disk(file).endswith(b" nav"), "navigation_keeps_the_text", **facts(a))
    a.click("Close README.md")
    must(a, a.js("return document.querySelector('dialog[open]')?.getAttribute('aria-label')??'';") == "Unsaved changes", "closing_a_dirty_tab_asks")
    a.click("Cancel")
    must(a, " nav" in text_of(a), "cancel_keeps_the_text")
    focus(a)
    a.keys("ctrl+s")
    a.wait(lambda: facts(a)["status"] == "Saved", "saved-after-return")


def large_file(a, file):
    a.js("[...document.querySelectorAll('.tn')].find(b=>b.textContent.includes('big.txt')).click();return 'yes';")
    a.wait(lambda: facts(a)["label"] == "Edit big.txt", "big-open")
    focus(a)
    a.keys("ctrl+f")
    a.wait(lambda: a.js("return document.querySelector('.cm-search')?'yes':''") == "yes", "find-panel")
    done = a.query("""
const set=(name,value)=>{const e=document.querySelector(`.cm-search input[name=${name}]`);e.value=value;e.dispatchEvent(new InputEvent('input',{bubbles:true}));e.dispatchEvent(new KeyboardEvent('keyup',{bubbles:true}));};
set('search','needle');set('replace','thread');
await new Promise(r=>setTimeout(r,200));
const heard=[];const watch=new MutationObserver(()=>{const t=document.querySelector('.cm-announced')?.textContent?.trim();if(t)heard.push(t);});
watch.observe(document.querySelector('.cm-editor'),{subtree:true,childList:true,characterData:true});
[...document.querySelectorAll('.cm-search button')].find(b=>b.textContent.trim()==='replace all').click();
await new Promise(r=>setTimeout(r,300));
watch.disconnect();
return JSON.stringify({announced:heard.join(' | ')});""")
    a.keys("Escape")
    focus(a)
    a.keys("ctrl+s")
    a.wait(lambda: facts(a)["status"] == "Saved", "big-saved")
    held = disk(file)
    must(a, held.count(b"thread") == 5000 and b"needle" not in held and "5000" in done["announced"], "replace_all_counts_every_occurrence", announced=done["announced"], replaced=held.count(b"thread"))
    timing = a.query("""
const c=document.querySelector('.cm-content');c.focus();const samples=[];
for(let i=0;i<40;i++){const t=performance.now();document.execCommand('insertText',false,'x');await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));samples.push(performance.now()-t);}
samples.sort((x,y)=>x-y);return JSON.stringify({samples:samples.length,median:samples[20],worst:samples[39]});""")
    a.record("input_to_second_frame_ms_5000_lines", method="execCommand insertText to two animation frames, software rendering under Xvfb, not reference hardware", **timing)
    a.click("Close big.txt")
    a.click("Discard changes")
    a.wait(lambda: "big.txt, unsaved" not in facts(a)["tabs"], "big-discarded", 10)
    must(a, disk(file) == held, "discard_is_the_owners_choice_and_writes_nothing")


def autosave(a, file, board):
    visit(a, "/settings/app/appearance")
    a.wait(lambda: visible_text(a, "Autosave"), "settings")
    a.js("document.querySelector('[aria-label=\"Autosave files\"]').click();return 'yes';")
    visit(a, board)
    a.click("Code")
    code_ready(a)
    a.js("[...document.querySelectorAll('.tn')].find(b=>b.textContent.includes('README.md')).click();return 'yes';")
    a.wait(lambda: facts(a)["label"] == "Edit README.md", "readme-back")
    before = disk(file)
    type_at_end(a, " auto")
    a.wait(lambda: facts(a)["status"] == "Saved", "autosaved", 8)
    must(a, disk(file) == before + b" auto", "autosave_wrote_without_a_save_action", bytes=disk(file).decode())
    visit(a, "/settings/app/appearance")
    a.wait(lambda: visible_text(a, "Autosave"), "settings-again")
    a.js("document.querySelector('[aria-label=\"Autosave files\"]').click();return 'yes';")
    visit(a, board)
    a.click("Code")
    code_ready(a)


def worktree_of(a):
    rows = subprocess.check_output(["git", "worktree", "list", "--porcelain"], cwd=a.repo, env=a.env, text=True)
    paths = [line[9:] for line in rows.splitlines() if line.startswith("worktree ")]
    return Path(paths[-1]) if len(paths) > 1 else a.repo


def writers(a, board):
    visit(a, board)
    a.click("New task")
    a.fill("Describe the task", "Hold this checkout.")
    a.click("Start now")
    a.wait(lambda: visible_text(a, "Which direction"), "question", 40)
    a.click("Code")
    code_ready(a)
    a.wait(lambda: "Cortex is working" in facts(a)["warn"], "writer-strip")
    target = worktree_of(a) / "README.md"
    locked = facts(a)
    must(a, locked["label"].startswith("Read "), "editor_locked_while_a_writer_is_active", **locked)
    a.snapshot("30-writer")
    before = disk(target)
    a.click("Edit anyway")
    a.wait(lambda: facts(a)["label"].startswith("Edit "), "unlocked")
    type_at_end(a, "held text")
    a.keys("ctrl+s")
    time.sleep(1)
    seen = facts(a)
    must(a, seen["save"]["off"] and "waits until the task stops" in seen["save"]["reason"] and disk(target) == before and any("unsaved" in t for t in seen["tabs"]), "no_save_lands_while_a_writer_is_active", **seen)
    a.click("Stop")
    a.wait(lambda: not facts(a)["warn"], "runtime-confirmed-stop", 30)
    focus(a)
    a.keys("ctrl+s")
    a.wait(lambda: facts(a)["status"] == "Saved", "saved-after-stop")
    must(a, disk(target) == before + b"held text", "save_lands_after_the_runtime_confirms_quiet", path_is_worktree=target.parent != a.repo, bytes=disk(target).decode())


def closing(a, file, board):
    visit(a, board)
    a.click("Code")
    code_ready(a)
    a.js("[...document.querySelectorAll('.tn')].find(b=>b.textContent.includes('README.md')).click();return 'yes';")
    a.wait(lambda: facts(a)["label"] == "Edit README.md", "readme-for-close")
    before = disk(file)
    type_at_end(a, " closing")
    a.js("document.querySelector('.win button:last-child').click();return 'yes';")
    a.wait(lambda: a.js("return document.querySelector('dialog[open]')?.getAttribute('aria-label')??'';") == "Unsaved files", "close-asks")
    must(a, a.app.poll() is None and disk(file) == before, "window_button_asks_first", dialog=a.js("return document.querySelector('dialog[open]').innerText;"))
    a.click("Keep working")
    a.js("@focus")
    a.keys("alt+F4")
    a.wait(lambda: a.js("return document.querySelector('dialog[open]')?.getAttribute('aria-label')??'';") == "Unsaved files", "wm-close-asks")
    must(a, a.app.poll() is None, "window_manager_close_asks_first")
    a.click("Save all and close")
    a.wait(lambda: a.app.poll() is not None, "app-closed", 15)
    must(a, disk(file) == before + b" closing", "save_all_and_close_wrote_the_bytes", exit_code=a.app.returncode, bytes=disk(file).decode())


def prepare(a, big=True):
    a.start(True)
    project = a.project()
    if big:
        (project / "big.txt").write_text("".join(f"needle {n}\n" for n in range(5000)))
        for args in (["add", "big.txt"], ["-c", "user.name=Audit", "-c", "user.email=audit@example.invalid", "commit", "-q", "-m", "Large fixture"]):
            subprocess.run(["git", *args], cwd=project, env=a.env, check=True, capture_output=True)
    a.add(project)
    board = a.js("return location.pathname;")
    a.click("Activate")
    a.wait(lambda: not visible_text(a, "Activate"), "activated", 40)
    a.click("Code")
    return project, board


def journey(a):
    project, board = prepare(a)
    file = project / "README.md"
    edit_and_save(a, file)
    conflict_found_on_disk(a, file)
    conflict_met_by_save(a, file)
    navigation_keeps_text(a, file)
    large_file(a, project / "big.txt")
    autosave(a, file, board)
    openers(a, "vscode", board)
    writers(a, board)
    closing(a, file, board)


def external(a):
    return a.query("""
const b=[...document.querySelectorAll('.ecrumb button')].find(b=>/^Open in /.test(b.textContent.trim()));
return JSON.stringify({name:b.textContent.trim(),off:b.getAttribute('aria-disabled')==='true',reason:b.title});""")


def logs(a):
    visit(a, "/settings/machine/local/diagnostics")
    a.wait(lambda: visible_text(a, "Open logs"), "logs-row")
    return a.query("""
const b=[...document.querySelectorAll('button')].find(b=>b.textContent.trim()==='Open logs');
return JSON.stringify({off:b.getAttribute('aria-disabled')==='true',reason:b.title});""")


def calls(path="/evidence/opener-calls.txt"):
    file = Path(path)
    return file.read_text().splitlines() if file.exists() else []


def openers(a, stage, board):
    code_ready(a)
    button = external(a)
    if stage == "none":
        must(a, button["name"] == "Open in VS Code" and button["off"] and "No external editor was found" in button["reason"], "external_editor_says_why_not", **button)
        off = logs(a)
        must(a, off["off"] and "No folder opener was found" in off["reason"], "open_logs_says_why_not", **off)
        return
    name = "Open in VS Code" if stage == "vscode" else "Open in default app"
    must(a, button["name"] == name and not button["off"], "external_editor_is_offered", **button)
    a.js("[...document.querySelectorAll('.ecrumb button')].find(b=>/^Open in /.test(b.textContent.trim())).click();return 'yes';")
    a.wait(lambda: len(calls()) >= 1, "editor-called")
    opened = calls()
    expected = ["--reuse-window", "--goto"] if stage == "vscode" else []
    must(a, all(word in opened for word in expected) and any(row.endswith("README.md") or row.endswith("README.md:1") for row in opened), "external_editor_got_the_checkout_file", arguments=[Path(row).name if row.startswith("/") else row for row in opened])
    on = logs(a)
    must(a, not on["off"], "open_logs_is_offered", **on)
    count = len(calls())
    a.click("Open logs")
    a.wait(lambda: len(calls()) > count, "logs-called")
    must(a, any(row.endswith("/logs") for row in calls()[count:]), "open_logs_got_the_private_folder")
    visit(a, board)
    a.click("Code")
    code_ready(a)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--evidence", required=True)
    parser.add_argument("--stage", choices=["journey", "none", "system"], required=True)
    args = parser.parse_args()
    a = Audit("/", args.evidence)
    try:
        if args.stage == "journey":
            journey(a)
        else:
            _, board = prepare(a, big=False)
            openers(a, args.stage, board)
        a.record("finished", stage=args.stage)
        return 0
    finally:
        a.close()


if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).parent))
    raise SystemExit(main())
