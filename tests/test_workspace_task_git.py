"""Task Git safety against disposable repositories; no engines or remote writes."""
import importlib.util
from pathlib import Path

import pytest

from test_workspace_machine_tools import machine as base_machine, ok

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("workspace_machine", ROOT / "crates/workspace-service/machine_tools.py")
helper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helper)
sys_module = __import__("sys")
sys_module.modules[spec.name] = helper
sys_module.path.insert(0, str(ROOT / "crates/workspace-service"))
import task_git


@pytest.fixture(name="machine")
def git_machine(base_machine, monkeypatch):
    # Git-rule tests inject the runtime-result validator; they do not claim
    # runtime verification. Runtime binding is tested separately in API/journey checks.
    monkeypatch.setattr(task_git, "runtime_result", lambda *args: None)
    def call(op, params=None, project="wp_1"):
        if op != "product.integrate":
            return base_machine(op, params, project)
        try:
            result = task_git.integrate(helper, str(base_machine.repo), project, base_machine.repo.parent / "state", params)
            return {"ok":True,"result":result}
        except helper.Refused as error:
            return {"ok":False,"code":error.code,"message":error.message}
    call.repo, call.git = base_machine.repo, base_machine.git
    return call


def test_helpers_are_never_shown_or_staged(machine):
    (machine.repo / ".cortex").mkdir()
    import sqlite3
    db = sqlite3.connect(machine.repo / ".cortex/cortex.sqlite3")
    db.execute("CREATE TABLE private(value)")
    db.commit()
    db.close()
    (machine.repo / ".cortex-workspace-dev.json").write_text("{}")
    (machine.repo / "a.txt").write_text("owner change\n")
    snapshot = ok(machine("repo.snapshot"))
    assert [f["path"] for f in snapshot["checkouts"][0]["files"]] == ["a.txt"]
    assert ".cortex-workspace-dev.json" not in snapshot["tree"]
    assert machine("git", {"action": "stage", "path": ".cortex-workspace-dev.json", "staged": True})["code"] == "invalid_path"


@pytest.mark.parametrize("checkout", ["main", "worktree"])
def test_ignored_helpers_do_not_break_merge(machine, tmp_path, checkout):
    path = task_tree(machine, tmp_path) if checkout == "worktree" else machine.repo
    if checkout == "main":
        with helper.Locked(tmp_path / "state"):
            product = helper.product_of(tmp_path / "state", "wp_1")
            product["tasks"]["task"] = {"status": "ready_to_merge", "checkout": {"kind": "main"},
                "acceptance": {"digest": "reviewed"}, "baseRevision": helper.out(path, "rev-parse", "HEAD").strip()}
            helper.write_json(helper.product_path(tmp_path / "state", "wp_1"), product)
    exclude = Path(helper.out(path, "rev-parse", "--git-path", "info/exclude").strip())
    if not exclude.is_absolute():
        exclude = path / exclude
    with exclude.open("a") as stream:
        stream.write("\n.cortex/\n.cortex-dev/\n")
    for name in (".cortex", ".cortex-dev"):
        (path / name).mkdir(exist_ok=True)
        (path / name / "private").write_text("never stage")
    (path / "new file.py").write_text("def subtract(a, b):\n    return a - b\n")
    before = helper.out(machine.repo, "rev-parse", "HEAD")
    reply = machine("product.integrate", {"taskId": "task", "method": "squash", "targetBranch": "main",
        "message": "Arithmetic", "confirmed": True, "digest": "reviewed"})
    if checkout == "main":
        # A shared checkout no longer silently commits every file as a squash.
        assert reply["code"] == "same_branch"
        assert helper.out(machine.repo, "rev-parse", "HEAD") == before
        return
    ok(reply)
    assert helper.out(machine.repo, "rev-parse", "HEAD") != before
    assert helper.out(machine.repo, "show", "HEAD:new file.py") == "def subtract(a, b):\n    return a - b\n"
    assert not any(task_git.helper_path(p) for p in helper.out(path, "ls-files").splitlines())


def test_commit_failure_restores_index_and_keeps_user_files(machine, tmp_path, monkeypatch):
    path = task_tree(machine, tmp_path)
    (path / "a.txt").write_text("staged owner selection\n")
    helper.git(path, "add", "--", "a.txt")
    (path / "a.txt").write_text("later unstaged edit\n")
    (path / "new.py").write_text("new implementation\n")
    index = Path(helper.out(path, "rev-parse", "--git-path", "index").strip())
    before = index.read_bytes()
    head = helper.out(path, "rev-parse", "HEAD")
    hooks = tmp_path / "hooks"
    hooks.mkdir()
    machine.git("config", "core.hooksPath", str(hooks))
    hook = hooks / "pre-commit"
    hook.write_text("#!/bin/sh\necho 'RAW advice: use git add -f .cortex-dev' >&2\nexit 1\n")
    hook.chmod(0o755)
    real_git = helper.git
    monkeypatch.setattr(helper, "git", lambda cwd, *args, **kw: real_git(cwd, "-c", f"core.hooksPath={hooks}", *args, **kw))
    reply = machine("product.integrate", {"taskId": "task", "method": "squash", "targetBranch": "main",
        "message": "Arithmetic", "confirmed": True, "digest": "reviewed"})
    assert reply["ok"] is False
    assert index.read_bytes() == before
    assert helper.out(path, "rev-parse", "HEAD") == head
    assert (path / "new.py").read_text() == "new implementation\n"
    assert (path / "a.txt").read_text() == "later unstaged edit\n"
    assert "RAW" not in reply["message"] and ".cortex-dev" not in reply["message"]
    assert "hook" in reply["message"].lower() and "retry" in reply["message"].lower()


def test_untracked_text_is_reviewed_as_full_additions(machine, tmp_path):
    path = task_tree(machine, tmp_path)
    (path / "subtract.py").write_text("def subtract(a, b):\n    return a - b\n")
    snapshot = ok(machine("repo.snapshot"))
    file = next(f for f in snapshot["taskDiffs"]["task"]["files"] if f["path"] == "subtract.py")
    assert file["add"] == 2 and file["del"] == 0
    assert [line["text"] for h in file["hunks"] for line in h["lines"]] == [
        "def subtract(a, b):", "    return a - b"]
    assert all(line["kind"] == "add" and line["old"] is None
               for h in file["hunks"] for line in h["lines"])
    assert not helper.out(path, "diff", "--cached", "--name-only")


def test_worktree_root_cannot_be_a_project(machine):
    assert machine("machine.update", {"patch": {"worktreeRoot": str(machine.repo / "trees")}})["code"] == "unsafe_worktree_root"


def task_tree(machine, tmp_path):
    path = tmp_path / "task-tree"
    machine.git("worktree", "add", "-b", "cw/task", str(path))
    with helper.Locked(tmp_path / "state"):
        product = helper.product_of(tmp_path / "state", "wp_1")
        product["tasks"]["task"] = {"status": "ready_to_merge", "baseRevision": helper.out(machine.repo, "rev-parse", "HEAD").strip(), "checkout": {"kind": "worktree", "path": str(path), "branch": "cw/task"}, "acceptance": {"digest": "reviewed"}, "targetBranch": "main"}
        helper.write_json(helper.product_path(tmp_path / "state", "wp_1"), product)
    return path


def test_squash_into_target_preserves_source_and_excludes_helpers(machine, tmp_path):
    path = task_tree(machine, tmp_path)
    (path / "a.txt").write_text("task change\n")
    (path / ".cortex-workspace-dev.json").write_text("{}")
    ok(machine("product.integrate", {"taskId": "task", "method": "squash", "targetBranch": "main", "message": "Task", "confirmed": True, "digest": "reviewed"}))
    assert (machine.repo / "a.txt").read_text() == "task change\n"
    assert (path / "a.txt").read_text() == "task change\n"
    assert ok(machine("repo.snapshot"))["commits"][0]["message"] == "Task"
    assert ".cortex-workspace-dev.json" not in helper.out(machine.repo, "ls-files")


def test_merge_refuses_dirty_target_and_unaccepted_result(machine, tmp_path):
    path = task_tree(machine, tmp_path)
    (path / "a.txt").write_text("task change\n")
    (machine.repo / "a.txt").write_text("owner work\n")
    reply = machine("product.integrate", {"taskId": "task", "method": "squash", "targetBranch": "main", "message": "Task", "confirmed": True, "digest": "reviewed"})
    assert reply["code"] == "dirty_target"
    assert (machine.repo / "a.txt").read_text() == "owner work\n"
    assert machine("product.integrate", {"taskId": "task", "method": "squash", "targetBranch": "main", "message": "Task", "confirmed": True, "digest": "old"})["code"] == "stale_result"


def test_conflict_is_retained_without_changing_target(machine, tmp_path):
    path = task_tree(machine, tmp_path)
    (path / "a.txt").write_text("task change\n")
    (machine.repo / "a.txt").write_text("owner change\n")
    machine.git("add", "a.txt")
    machine.git("commit", "-m", "owner")
    reply = machine("product.integrate", {"taskId": "task", "method": "squash", "targetBranch": "main", "message": "Task", "confirmed": True, "digest": "reviewed"})
    assert reply["code"] == "merge_conflict"
    assert (machine.repo / "a.txt").read_text() == "owner change\n"
    record = ok(machine("product.read"))["tasks"]["task"]
    assert record["conflict"] == "conflict"
    assert record["merge"]["files"][0]["path"] == "a.txt"


def test_target_update_generates_english_merge_message(machine, tmp_path):
    path = task_tree(machine, tmp_path)
    (path / "a.txt").write_text("task change\n")
    (machine.repo / "other.txt").write_text("target change\n")
    machine.git("add", "other.txt"); machine.git("commit", "-m", "Русская задача")
    reply = machine("product.integrate", {"taskId": "task", "method": "squash", "targetBranch": "main",
        "message": "Task", "confirmed": True, "digest": "reviewed"})
    assert reply["code"] == "review_required"
    assert helper.out(path, "log", "-1", "--format=%s").strip() == "Update task branch from target"
    assert ok(machine("product.read"))["tasks"]["task"]["acceptance"] is None


def test_manual_conflict_resolution_keeps_target_and_requires_new_review(machine, tmp_path):
    path = task_tree(machine, tmp_path)
    (path / "a.txt").write_text("task change\n")
    (machine.repo / "a.txt").write_text("owner change\n")
    machine.git("add","a.txt")
    machine.git("commit","-m","owner")
    assert machine("product.integrate", {"taskId":"task","method":"squash","targetBranch":"main","message":"Task","confirmed":True,"digest":"reviewed"})["code"] == "merge_conflict"
    assert not machine("product.task", {"action":"finish_merge","taskId":"task"})["ok"]
    ok(machine("product.task", {"action":"resolve_block","taskId":"task","path":"a.txt","blockId":"a.txt","choice":"ours"}))
    ok(machine("product.task", {"action":"finish_merge","taskId":"task"}))
    record = ok(machine("product.read"))["tasks"]["task"]
    assert record["acceptance"] is None and record["resolvedBy"] == "you"
    assert (machine.repo / "a.txt").read_text() == "owner change\n"
    assert (path / "a.txt").read_text() == "task change\n"
    assert helper.out(path, "log", "-1", "--format=%s").strip() == "Resolve task merge conflicts"


def test_cleanup_never_forces_dirty_worktree(machine, tmp_path):
    path = task_tree(machine, tmp_path)
    (path / "a.txt").write_text("unmerged")
    checkout = next(c for c in ok(machine("repo.snapshot"))["checkouts"] if c["path"] == str(path))
    assert not machine("git", {"action": "delete_worktree", "checkoutId": checkout["id"], "confirmed": True, "discard": True})["ok"]
    assert path.exists()


def test_modify_delete_conflict_is_never_replaced_with_an_empty_file(machine, tmp_path):
    path = task_tree(machine, tmp_path)
    (path / "a.txt").write_text("task change\n")
    machine.git("rm", "a.txt")
    machine.git("commit", "-m", "owner delete")
    assert machine("product.integrate", {"taskId":"task","method":"squash","targetBranch":"main","message":"Task","confirmed":True,"digest":"reviewed"})["code"] == "merge_conflict"
    before = (path / "a.txt").read_bytes()
    reply = machine("product.task", {"action":"resolve_block","taskId":"task","path":"a.txt","blockId":"a.txt","choice":"theirs"})
    assert reply["code"] == "unsafe_conflict"
    assert (path / "a.txt").read_bytes() == before
    assert not (machine.repo / "a.txt").exists()


def test_large_conflict_keeps_bounded_overlay_and_manual_instructions(machine, tmp_path):
    path = task_tree(machine, tmp_path)
    (path / "a.txt").write_text("task" * 40_000)
    (machine.repo / "a.txt").write_text("owner" * 40_000)
    machine.git("add", "a.txt")
    machine.git("commit", "-m", "owner")
    reply = machine("product.integrate", {"taskId":"task","method":"squash","targetBranch":"main","message":"Task","confirmed":True,"digest":"reviewed"})
    assert reply["code"] == "merge_conflict"
    record = ok(machine("product.read"))["tasks"]["task"]
    block = record["merge"]["files"][0]["blocks"][0]
    assert "Git" in block["cortexNote"]
    assert len(__import__("json").dumps(record)) < 20_000
    before = (path / "a.txt").read_bytes()
    reply = machine("product.task", {"action":"resolve_block","taskId":"task","path":"a.txt","blockId":"a.txt","choice":"ours"})
    assert reply["code"] == "unsafe_conflict"
    assert (path / "a.txt").read_bytes() == before


def test_confirmed_merge_removes_clean_task_worktree_but_keeps_branch(machine, tmp_path):
    path = task_tree(machine, tmp_path)
    (path / "a.txt").write_text("task change\\n")
    ok(machine("product.integrate", {"taskId":"task","method":"squash","targetBranch":"main","message":"Task","confirmed":True,"digest":"reviewed","deleteWorktree":True}))
    assert not path.exists()
    machine.git("show-ref", "--verify", "refs/heads/cw/task")
    assert ok(machine("repo.snapshot"))["taskDiffs"]["task"]["files"][0]["path"] == "a.txt"
    assert ok(machine("product.read"))["tasks"]["task"]["integration"]["worktreeCleanup"] == "removed"
    ok(machine("product.task", {"taskId":"task","action":"archive"}))
    ok(machine("product.task", {"taskId":"task","action":"purge","confirmed":True}))


def test_pull_request_uses_fake_gh_with_local_bare_remote_only(machine, tmp_path, monkeypatch):
    import os
    import subprocess
    path = task_tree(machine, tmp_path)
    (path / "a.txt").write_text("task change\n")
    helper.git(path, "add", "a.txt")
    helper.git(path, "commit", "-m", "task")
    remote = tmp_path / "remote.git"
    subprocess.run(["git", "init", "--bare", str(remote)], check=True, capture_output=True)
    machine.git("remote", "add", "origin", str(remote))
    helper.git(path, "push", "-u", "origin", "cw/task")
    bin = tmp_path / "bin"
    bin.mkdir()
    gh = bin / "gh"
    receipt = tmp_path / "gh-receipt.json"
    gh.write_text("#!/usr/bin/python3\nimport json, sys\nfrom pathlib import Path\nPath(" + repr(str(receipt)) + ").write_text(json.dumps(sys.argv[1:]))\nprint('https://fixture.invalid/pr/1')\n")
    gh.chmod(0o700)
    monkeypatch.setenv("PATH", str(bin) + os.pathsep + os.environ["PATH"])
    ok(machine("product.integrate", {"taskId":"task","method":"pr","targetBranch":"main","message":"Fixture PR","body":"Fixture body","confirmed":True,"digest":"reviewed"}))
    assert __import__("json").loads(receipt.read_text()) == ["pr","create","--base","main","--head","cw/task","--title","Fixture PR","--body","Fixture body"]
    assert machine("product.integrate", {"taskId":"task","method":"pr","targetBranch":"main","message":"Fixture PR","confirmed":True,"digest":"reviewed"})["code"] == "unknown_outcome"

