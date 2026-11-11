"""The machine helper against a real Git repository and a private configuration root."""
from concurrent.futures import ThreadPoolExecutor
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import stat
import subprocess

import pytest

ROOT = Path(__file__).resolve().parents[1]
TOOLS = ROOT / "crates/workspace-service/machine_tools.py"


@pytest.fixture
def machine(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    state = tmp_path / "state"
    env = {"PATH": os.environ["PATH"], "HOME": str(tmp_path), "XDG_CONFIG_HOME": str(tmp_path / "config"), "XDG_STATE_HOME": str(tmp_path / "xdg-state")}

    def git(*args):
        subprocess.run(["git", *args], cwd=repo, check=True, capture_output=True, env=env)

    git("init", "-b", "main")
    git("config", "user.email", "owner@example.invalid")
    git("config", "user.name", "Owner")
    (repo / "a.txt").write_text("one\ntwo\n")
    git("add", ".")
    git("commit", "-m", "first")

    def call(op, params=None, project="wp_1"):
        request = {"op": op, "projectId": project, "root": str(repo), "stateDir": str(state), "params": params, "projects": [{"id": project, "root": str(repo), "name": "Repo"}]}
        done = subprocess.run(["python3", "-I", "-B", str(TOOLS)], input=json.dumps(request), text=True, capture_output=True, env=env, check=True)
        return json.loads(done.stdout)

    call.repo, call.git = repo, git
    return call


def ok(reply):
    assert reply["ok"], reply
    return reply["result"]


def test_durable_queue_and_comments_read_back_across_helper_processes(machine):
    message = dict(id="msg-1", taskId="task-1", workerId=None, text="line one\nline two",
                   mode="queue", delivery="queued", at=1, deliveredAt=None, failure=None)
    ok(machine("product.message", dict(action="add", message=message)))
    assert ok(machine("product.read"))["messages"] == [message]
    ok(machine("product.message", dict(action="add", message=message)))
    assert len(ok(machine("product.read"))["messages"]) == 1
    assert not machine("product.message", dict(action="add", message=dict(message, text="changed")))["ok"]
    comment = dict(id="comment-1", path="a.txt", line=2, text="Please fix this", sent=False)
    ok(machine("product.comment", dict(action="add", taskId="task-1", comment=comment)))
    assert ok(machine("product.read"))["comments"]["task-1"] == [comment]
    ok(machine("product.message", dict(action="remove", messageId="msg-1")))
    assert ok(machine("product.read"))["messages"] == []


def test_queue_reorder_remove_and_reopen_are_durable(machine):
    def add(key, task="task"):
        item = dict(id=key, taskId=task, workerId=None, text=key,
                    mode="queue", delivery="queued", at=1, deliveredAt=None, failure=None)
        ok(machine("product.message", dict(action="add", message=item)))
    add("one"); add("two"); add("foreign", "other-task")
    ok(machine("product.message", dict(action="move", messageId="two", before="one")))
    assert [m["id"] for m in ok(machine("product.read"))["messages"]] == ["two", "one", "foreign"]
    ok(machine("product.message", dict(action="move", messageId="two", before="two")))
    assert [m["id"] for m in ok(machine("product.read"))["messages"]] == ["two", "one", "foreign"]
    before = ok(machine("product.read"))["messages"]
    assert not machine("product.message", dict(action="move", messageId="one", before="foreign"))["ok"]
    assert ok(machine("product.read"))["messages"] == before
    assert not machine("product.message", dict(action="move", messageId="one", before="gone"))["ok"]
    ok(machine("product.message", dict(action="remove", messageId="two")))
    assert [m["id"] for m in ok(machine("product.read"))["messages"]] == ["one", "foreign"]


def test_status_diff_and_history_read_the_real_repository(machine):
    (machine.repo / "a.txt").write_text("one\nchanged\n")
    (machine.repo / "b.txt").write_text("new\n")
    repo = ok(machine("repo.snapshot"))
    main = repo["checkouts"][0]
    assert (repo["openBranch"], repo["defaultBranch"], main["kind"], main["branch"]) == ("main", "main", "main", "main")
    assert repo["tree"] == ["a.txt", "b.txt"]
    assert [commit["message"] for commit in repo["commits"]] == ["first"]
    files = {file["path"]: file for file in main["files"]}
    assert files["b.txt"]["stage"] == "untracked"
    edited = files["a.txt"]
    assert (edited["stage"], edited["add"], edited["del"]) == ("unstaged", 1, 1)
    assert [(line["kind"], line["text"]) for line in edited["hunks"][0]["lines"]] == [("ctx", "one"), ("del", "two"), ("add", "changed")]


def test_status_includes_nested_untracked_cache_paths_unless_git_ignores_them(machine):
    paths = [
        "new/deep/file.txt",
        "prototypes/app/node_modules/.vite/vitest/hash/results.json",
        "prototypes/app/node_modules/.vite/vitest/CACHEDIR.TAG",
        "prototypes/app/node_modules/.vitest-cache/_metadata.json",
        ".venv/new.txt",
        "nested/__pycache__/new.txt",
    ]
    for name in paths + ["ignored/cache.txt", ".cortex/private.txt"]:
        file = machine.repo / name
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text("new\n")
    (machine.repo / ".gitignore").write_text("ignored/\n")
    machine.git("add", ".gitignore")
    (machine.repo / "a.txt").unlink()
    snapshot = ok(machine("repo.snapshot"))
    files = snapshot["checkouts"][0]["files"]
    untracked = {f["path"] for f in files if f["stage"] == "untracked"}
    assert untracked == set(paths)
    assert set(paths) <= set(snapshot["tree"])
    assert any(f["path"] == "a.txt" and f["status"] == "D" and f["stage"] == "unstaged" for f in files)
    assert any(f["path"] == ".gitignore" and f["stage"] == "staged" for f in files)
    assert all(f["status"] == "U" and f["add"] == 1 for f in files if f["stage"] == "untracked")


def test_large_untracked_dependency_tree_collapses_without_hiding_other_files(machine):
    (machine.repo / "app").mkdir()
    (machine.repo / "app/package.json").write_text("{}\n")
    machine.git("add", "app/package.json")
    machine.git("commit", "-m", "app")
    packages = [f"app/node_modules/pkg{i}/index.js" for i in range(2_100)]
    small = ["new.txt", "cache/.vitest-cache/_metadata.json", "cache/.vite/CACHEDIR.TAG"]
    for name in packages + small:
        file = machine.repo / name
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text("new\n")
    snapshot = ok(machine("repo.snapshot"))
    untracked = [f for f in snapshot["checkouts"][0]["files"] if f["stage"] == "untracked"]
    # The dependency tree is one entry; smaller untracked directories and loose files stay individual.
    assert sorted(f["path"] for f in untracked) == sorted(small + ["app/node_modules"])
    assert next(f for f in untracked if f["path"] == "app/node_modules")["hunks"] == []
    assert set(small) | {"app/package.json"} <= set(snapshot["tree"])
    assert not any(name.startswith("app/node_modules") for name in snapshot["tree"]) and not snapshot["treeTruncated"]
    # Committing with an empty index still includes everything listed, collapsed directories too.
    ok(machine("git", {"action": "commit", "message": "everything"}))
    committed = subprocess.run(["git", "ls-files"], cwd=machine.repo, check=True, capture_output=True, text=True).stdout.split()
    assert set(packages + small) <= set(committed)


def test_stage_commit_and_revert_change_the_repository(machine):
    (machine.repo / "a.txt").write_text("one\nchanged\n")
    ok(machine("git", {"action": "stage", "path": "a.txt", "staged": True}))
    assert ok(machine("repo.snapshot"))["checkouts"][0]["files"][0]["stage"] == "staged"
    ok(machine("git", {"action": "commit", "message": "second"}))
    assert [c["message"] for c in ok(machine("repo.snapshot"))["commits"]] == ["second", "first"]
    (machine.repo / "a.txt").write_text("broken\n")
    assert not machine("git", {"action": "revert", "path": "a.txt"})["ok"]
    ok(machine("git", {"action": "revert", "path": "a.txt", "confirmed": True}))
    assert (machine.repo / "a.txt").read_text() == "one\nchanged\n"


def test_commit_without_remote_stages_listed_changes_only(machine):
    (machine.repo / "new.txt").write_text("new file")
    (machine.repo / "a.txt").write_text("changed")
    (machine.repo / ".cortex-dev").mkdir()
    (machine.repo / ".cortex-dev/private.json").write_text("{}")
    ok(machine("git", {"action": "commit", "message": "local only"}))
    assert ok(machine("repo.snapshot"))["commits"][0]["message"] == "local only"
    assert ok(machine("repo.snapshot"))["checkouts"][0]["files"] == []
    assert machine("git", {"action": "push", "remote": "origin", "branch": "main",
                            "confirmed": True})["code"] == "unknown_remote"
    assert machine("git", {"action": "pull"})["code"] == "upstream_required"


@pytest.mark.parametrize("selection", ["name", "display_url"])
def test_push_configured_remote_on_first_request(machine, tmp_path, selection):
    remote = tmp_path / "remote.git"
    machine.git("init", "--bare", str(remote))
    machine.git("remote", "add", "origin", str(remote))
    machine.git("branch", "-m", "dev")
    machine.git("push", "-u", "origin", "dev")
    (machine.repo / "a.txt").write_text("second\n")
    machine.git("commit", "-am", "second")
    snapshot = ok(machine("repo.snapshot"))
    checkout = snapshot["checkouts"][0]
    assert (checkout["upstream"], checkout["ahead"], checkout["behind"]) == ("origin/dev", 1, 0)
    # The dialog sends ProjectRepo.remote (a URL), not the name "origin".
    request = {"action": "push", "checkoutId": checkout["id"], "confirmed": True,
               "remote": snapshot["remote"] if selection == "display_url" else "origin", "branch": "dev"}
    ok(machine("git", request))
    assert ok(machine("repo.snapshot"))["checkouts"][0]["ahead"] == 0
    pushed = subprocess.run(["git", "--git-dir", str(remote), "log", "-1", "--format=%s", "dev"],
                            check=True, capture_output=True, text=True)
    assert pushed.stdout.strip() == "second"


def test_push_rejects_stale_unconfigured_and_ambiguous_display_urls(machine, tmp_path):
    remote = tmp_path / "remote.git"
    machine.git("init", "--bare", str(remote))
    machine.git("remote", "add", "origin", str(remote))
    displayed = ok(machine("repo.snapshot"))["remote"]
    request = {"action": "push", "remote": displayed, "branch": "main", "confirmed": True}
    assert machine("git", dict(request, confirmed=False))["code"] == "confirmation_required"
    machine.git("remote", "set-url", "origin", str(tmp_path / "changed.git"))
    assert machine("git", request)["code"] == "unknown_remote"
    machine.git("remote", "set-url", "origin", displayed)
    machine.git("remote", "add", "duplicate", displayed)
    assert machine("git", request)["code"] == "ambiguous_remote"
    assert machine("git", dict(request, remote=str(tmp_path / "unconfigured.git")))["code"] == "unknown_remote"


def test_ssh_display_url_resolves_to_name_without_network_or_retry(machine, monkeypatch):
    machine.git("branch", "-m", "dev")
    machine.git("remote", "add", "origin", "git@github.com:igovet/cortex-orch-acp.git")
    spec = importlib.util.spec_from_file_location("push_machine", TOOLS)
    tools = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(tools)
    original_git, pushes = tools.git, []

    def no_network(path, *args, **kwargs):
        if args[0] == "push":
            pushes.append(args)
            raise tools.Refused("git_timeout", "Git did not answer in time.")
        return original_git(path, *args, **kwargs)

    monkeypatch.setattr(tools, "git", no_network)
    request = {"action": "push", "checkoutId": "wp_1:main", "confirmed": True, "branch": "dev",
               "remote": "git@github.com:igovet/cortex-orch-acp.git"}
    with pytest.raises(tools.Refused) as failure:
        tools.git_action(machine.repo, "wp_1", machine.repo.parent / "state", request)
    assert failure.value.code == "git_timeout"
    assert pushes == [("push", "--", "origin", "dev")]


@pytest.mark.parametrize("branch", ["main", "--force"])
def test_push_never_forces_a_non_fast_forward(machine, tmp_path, branch):
    remote = tmp_path / "remote.git"
    machine.git("init", "--bare", str(remote))
    machine.git("remote", "add", "origin", str(remote))
    machine.git("push", "-u", "origin", "main")
    machine.git("checkout", "-b", "incoming")
    (machine.repo / "a.txt").write_text("incoming\n")
    machine.git("commit", "-am", "incoming")
    machine.git("push", "origin", "incoming:main")
    machine.git("checkout", "main")
    (machine.repo / "a.txt").write_text("divergent\n")
    machine.git("commit", "-am", "divergent")
    request = {"action": "push", "checkoutId": "wp_1:main", "confirmed": True,
               "remote": ok(machine("repo.snapshot"))["remote"], "branch": branch}
    assert machine("git", request)["code"] == "git_failed"
    pushed = subprocess.run(["git", "--git-dir", str(remote), "log", "-1", "--format=%s", "main"],
                            check=True, capture_output=True, text=True)
    assert pushed.stdout.strip() == "incoming"


def test_fetch_and_pull_use_git_tracking_without_a_remote_parameter(machine, tmp_path):
    remote = tmp_path / "remote.git"
    machine.git("init", "--bare", str(remote))
    machine.git("remote", "add", "origin", str(remote))
    machine.git("push", "-u", "origin", "main")
    machine.git("checkout", "-b", "incoming")
    (machine.repo / "a.txt").write_text("incoming\n")
    machine.git("commit", "-am", "incoming")
    machine.git("push", "origin", "incoming:main")
    machine.git("checkout", "main")
    ok(machine("git", {"action": "fetch"}))
    assert ok(machine("repo.snapshot"))["checkouts"][0]["behind"] == 1
    ok(machine("git", {"action": "pull", "checkoutId": "wp_1:main"}))
    assert (machine.repo / "a.txt").read_text() == "incoming\n"
    assert ok(machine("repo.snapshot"))["checkouts"][0]["behind"] == 0


def test_commit_preserves_explicit_staging_selection(machine):
    (machine.repo / "a.txt").write_text("selected")
    (machine.repo / "unselected.txt").write_text("keep unstaged")
    ok(machine("git", {"action": "stage", "path": "a.txt", "staged": True}))
    ok(machine("git", {"action": "commit", "message": "selected only"}))
    files = ok(machine("repo.snapshot"))["checkouts"][0]["files"]
    assert [(f["path"], f["stage"]) for f in files] == [("unselected.txt", "untracked")]


def test_plain_folder_snapshot_and_git_init(machine):
    import shutil
    shutil.rmtree(machine.repo / ".git")  # This fixture's disposable repository only.
    snapshot = ok(machine("repo.snapshot"))
    assert snapshot["hasGit"] is False and snapshot["tree"] == ["a.txt"]
    assert ok(machine("product.read"))["hasGit"] is False
    assert ok(machine("repo.file", {"file": "a.txt"}))["text"] == "one\ntwo\n"
    ok(machine("git_init"))
    snapshot = ok(machine("repo.snapshot"))
    assert snapshot["hasGit"] is True and snapshot["commits"] == []
    assert snapshot["checkouts"][0]["files"][0]["stage"] == "untracked"
    assert machine("git_init")["code"] == "repository_exists"


def test_one_hunk_stages_alone(machine):
    (machine.repo / "long.txt").write_text("".join(f"{n}\n" for n in range(40)))
    machine.git("add", ".")
    machine.git("commit", "-m", "long")
    lines = [f"{n}\n" for n in range(40)]
    lines[1], lines[35] = "first\n", "second\n"
    (machine.repo / "long.txt").write_text("".join(lines))
    ok(machine("git", {"action": "stage", "path": "long.txt", "staged": True, "hunk": 0}))
    stages = {f["stage"]: f for f in ok(machine("repo.snapshot"))["checkouts"][0]["files"]}
    assert stages["staged"]["hunks"][0]["lines"][-5]["text"] != "second" and stages["unstaged"]["add"] == 1


@pytest.mark.parametrize("path", ["../outside", "/etc/passwd", ".git/config", ".cortex/cortex.sqlite3", "a/../../b"])
def test_paths_stay_inside_the_checkout(machine, path):
    assert machine("repo.file", {"file": path})["code"] == "invalid_path"


def test_a_link_cannot_leave_the_checkout(machine, tmp_path):
    (tmp_path / "secret").write_text("secret")
    (machine.repo / "link").symlink_to(tmp_path / "secret")
    assert machine("repo.file", {"file": "link"})["code"] == "invalid_path"


def test_a_checkout_outside_the_project_is_refused(machine):
    assert machine("repo.file", {"file": "a.txt", "checkoutId": "wp_1:wt:0000000000000000"})["code"] == "unknown_checkout"


def sha(data):
    return hashlib.sha256(data).hexdigest()


def save(machine, text, revision, file="a.txt", **extra):
    return machine("repo.file.save", {"file": file, "text": text, "revision": revision, **extra})


def test_a_save_is_bound_to_the_revision_the_owner_read(machine):
    read = ok(machine("repo.file", {"file": "a.txt"}))
    assert read["revision"] == sha(b"one\ntwo\n") and read["readOnly"] is None and read["writer"] is None
    saved = ok(save(machine, "one\nthree\n", read["revision"]))
    assert saved == {"outcome": "saved", "revision": sha(b"one\nthree\n"), "size": 10}
    assert (machine.repo / "a.txt").read_bytes() == b"one\nthree\n"
    assert not list(machine.repo.glob(".cw-save-*"))


def test_a_change_on_disk_is_a_conflict_and_nothing_is_written(machine):
    base = ok(machine("repo.file", {"file": "a.txt"}))["revision"]
    (machine.repo / "a.txt").write_bytes(b"worker\n")
    assert ok(save(machine, "owner\n", base)) == {"outcome": "conflict", "revision": sha(b"worker\n")}
    assert (machine.repo / "a.txt").read_bytes() == b"worker\n"
    # Keeping the owner's text is a new save against what they were shown, never a forced write.
    assert ok(save(machine, "owner\n", sha(b"worker\n")))["outcome"] == "saved"
    assert (machine.repo / "a.txt").read_bytes() == b"owner\n"


def test_a_deleted_file_comes_back_only_on_an_absent_precondition(machine):
    base = ok(machine("repo.file", {"file": "a.txt"}))["revision"]
    (machine.repo / "a.txt").unlink()
    assert ok(save(machine, "owner\n", base)) == {"outcome": "conflict", "revision": None}
    assert not (machine.repo / "a.txt").exists()
    assert ok(save(machine, "owner\n", None))["outcome"] == "saved"
    assert (machine.repo / "a.txt").read_bytes() == b"owner\n"
    assert ok(save(machine, "again\n", None))["outcome"] == "conflict"
    assert (machine.repo / "a.txt").read_bytes() == b"owner\n"


def test_a_save_keeps_the_mode_and_the_exact_bytes(machine):
    script = machine.repo / "run.sh"
    script.write_bytes(b"#!/bin/sh\r\necho hi\r\n")
    script.chmod(0o755)
    ok(save(machine, "#!/bin/sh\r\necho bye\r\n﻿", sha(script.read_bytes()), file="run.sh"))
    assert script.read_bytes() == "#!/bin/sh\r\necho bye\r\n﻿".encode()
    assert stat.S_IMODE(script.stat().st_mode) == 0o755


@pytest.mark.parametrize("path", ["../outside", "/etc/passwd", ".git/config", ".cortex/cortex.sqlite3", "a/../../b", "sub/.git/config", "sub/.cortex/x"])
def test_a_save_stays_inside_the_checkout(machine, path):
    assert save(machine, "x", None, file=path)["code"] == "invalid_path"


def test_a_save_through_a_link_is_refused_even_inside_the_checkout(machine, tmp_path):
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "secret").write_text("secret")
    (machine.repo / "file-link").symlink_to(outside / "secret")
    (machine.repo / "dir-link").symlink_to(outside)
    (machine.repo / "inner-link").symlink_to("a.txt")
    (machine.repo / "git-link").symlink_to(".git")
    for file in ("file-link", "dir-link/secret", "dir-link/new", "inner-link", "git-link/config"):
        assert save(machine, "x", sha(b"secret"), file=file)["code"] == "symlink_refused", file
    assert (outside / "secret").read_text() == "secret" and not (outside / "new").exists()
    assert (machine.repo / "a.txt").read_bytes() == b"one\ntwo\n"
    read = ok(machine("repo.file", {"file": "inner-link"}))
    assert read["text"] == "one\ntwo\n" and "link" in read["readOnly"]


def test_a_save_is_bounded_and_refuses_binary(machine):
    base = sha(b"one\ntwo\n")
    assert save(machine, "x" * (512 * 1024 + 1), base)["code"] == "file_too_large"
    assert save(machine, "a\0b", base)["code"] == "binary_file"
    assert save(machine, "\ud800", base)["code"] == "invalid_text"
    (machine.repo / "blob").write_bytes(b"x\0y")
    assert ok(machine("repo.file", {"file": "blob"}))["text"] is None
    assert save(machine, "text", sha(b"x\0y"), file="blob")["code"] == "binary_file"
    (machine.repo / "big").write_bytes(b"x" * (512 * 1024 + 1))
    assert save(machine, "text", "0" * 64, file="big")["code"] == "file_too_large"
    (machine.repo / "latin").write_bytes(b"caf\xe9\n")
    assert "UTF-8" in ok(machine("repo.file", {"file": "latin"}))["readOnly"]
    assert save(machine, "cafe\n", sha(b"caf\xe9\n"), file="latin")["code"] == "not_text"
    assert (machine.repo / "latin").read_bytes() == b"caf\xe9\n"
    assert (machine.repo / "a.txt").read_bytes() == b"one\ntwo\n"


def test_a_save_needs_a_base_revision(machine):
    assert machine("repo.file.save", {"file": "a.txt", "text": "x"})["code"] == "invalid_request"
    assert (machine.repo / "a.txt").read_bytes() == b"one\ntwo\n"


def test_two_saves_on_one_base_cannot_both_win(machine):
    base = sha(b"one\ntwo\n")
    with ThreadPoolExecutor(6) as pool:
        replies = list(pool.map(lambda n: ok(save(machine, f"writer {n}\n", base)), range(6)))
    outcomes = sorted(r["outcome"] for r in replies)
    assert outcomes == ["conflict"] * 5 + ["saved"]
    winner = next(r for r in replies if r["outcome"] == "saved")
    assert sha((machine.repo / "a.txt").read_bytes()) == winner["revision"]


def load_tools():
    spec = importlib.util.spec_from_file_location("machine_tools_under_test", TOOLS)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_a_save_interrupted_before_the_rename_leaves_the_file_alone(machine, tmp_path, monkeypatch):
    tools = load_tools()
    target = machine.repo / "a.txt"

    def interrupted(*_args, **_kwargs):
        raise OSError("injected")

    monkeypatch.setattr(tools.os, "rename", interrupted)
    with pytest.raises(OSError):
        tools.repo_file_save(str(machine.repo), "wp_1", str(tmp_path / "state"), {"file": "a.txt", "text": "new\n", "revision": sha(b"one\ntwo\n")})
    assert target.read_bytes() == b"one\ntwo\n"
    assert not list(machine.repo.glob(".cw-save-*"))


def test_a_change_while_the_new_copy_is_written_is_still_a_conflict(machine, tmp_path, monkeypatch):
    tools = load_tools()
    target = machine.repo / "a.txt"
    real = tools.os.fsync

    def late_writer(fd):
        target.write_bytes(b"worker\n")
        real(fd)

    monkeypatch.setattr(tools.os, "fsync", late_writer)
    reply = tools.repo_file_save(str(machine.repo), "wp_1", str(tmp_path / "state"), {"file": "a.txt", "text": "new\n", "revision": sha(b"one\ntwo\n")})
    assert reply == {"outcome": "conflict", "revision": sha(b"worker\n")}
    assert target.read_bytes() == b"worker\n" and not list(machine.repo.glob(".cw-save-*"))


def runtime_task(repo, task, state, options=None):
    """The rows the runtime keeps for one started task, with the columns the rule reads."""
    (repo / ".cortex").mkdir(exist_ok=True)
    db = sqlite3.connect(repo / ".cortex/cortex.sqlite3")
    db.executescript("""CREATE TABLE IF NOT EXISTS workspace_tasks(sequence INTEGER PRIMARY KEY AUTOINCREMENT,task_id TEXT,project_id TEXT,execution_id TEXT);
        CREATE TABLE IF NOT EXISTS workspace_task_options(task_id TEXT PRIMARY KEY,options_json TEXT);
        CREATE TABLE IF NOT EXISTS workspace_executions(execution_id TEXT PRIMARY KEY,task_id TEXT,runtime_task_id TEXT);
        CREATE TABLE IF NOT EXISTS runtime_tasks(task_id TEXT PRIMARY KEY,state TEXT);""")
    db.execute("INSERT INTO workspace_tasks(task_id,project_id,execution_id) VALUES (?,?,?)", (task, "wp_1", "e-" + task))
    db.execute("INSERT INTO workspace_executions VALUES (?,?,?)", ("e-" + task, task, "r-" + task))
    db.execute("INSERT INTO workspace_task_options VALUES (?,?)", (task, json.dumps(options or {"checkout": {"kind": "main"}})))
    db.execute("INSERT INTO runtime_tasks VALUES (?,?)", ("r-" + task, state))
    db.commit()
    db.close()


def set_runtime_state(repo, task, state):
    db = sqlite3.connect(repo / ".cortex/cortex.sqlite3")
    db.execute("UPDATE runtime_tasks SET state=? WHERE task_id=?", (state, "r-" + task))
    db.commit()
    db.close()


@pytest.mark.parametrize("state", ["PLANNING", "EXECUTING", "REVIEWING", "VERIFYING", "WAITING_USER", "RECOVERING", "BLOCKED"])
def test_no_owner_save_lands_until_the_runtime_confirms_the_writer_stopped(machine, state):
    base = sha(b"one\ntwo\n")
    runtime_task(machine.repo, "task-1", state)
    assert ok(machine("repo.file", {"file": "a.txt"}))["writer"] == "task-1"
    assert ok(save(machine, "owner\n", base)) == {"outcome": "writer_active", "writer": "task-1"}
    assert (machine.repo / "a.txt").read_bytes() == b"one\ntwo\n"
    set_runtime_state(machine.repo, "task-1", "PAUSED")
    assert ok(machine("repo.file", {"file": "a.txt"}))["writer"] is None
    assert ok(save(machine, "owner\n", base))["outcome"] == "saved"


@pytest.mark.parametrize("state", ["COMPLETED", "FAILED", "CANCELLED", "PAUSED"])
def test_a_stopped_task_leaves_the_checkout_free(machine, state):
    runtime_task(machine.repo, "task-1", state)
    assert ok(save(machine, "owner\n", sha(b"one\ntwo\n")))["outcome"] == "saved"


def test_a_task_only_holds_its_own_checkout(machine, tmp_path):
    tree = tmp_path / "tree"
    machine.git("worktree", "add", "-b", "cw/task", str(tree))
    runtime_task(machine.repo, "task-1", "EXECUTING", {"checkout": {"kind": "worktree", "path": str(tree)}})
    assert ok(save(machine, "main owner\n", sha(b"one\ntwo\n")))["outcome"] == "saved"
    checkout = next(c["id"] for c in ok(machine("repo.snapshot"))["checkouts"] if c["kind"] == "worktree")
    assert ok(save(machine, "tree owner\n", sha(b"one\ntwo\n"), checkoutId=checkout))["outcome"] == "writer_active"
    assert (tree / "a.txt").read_bytes() == b"one\ntwo\n"
    set_runtime_state(machine.repo, "task-1", "COMPLETED")
    assert ok(save(machine, "tree owner\n", sha(b"one\ntwo\n"), checkoutId=checkout))["outcome"] == "saved"
    assert (tree / "a.txt").read_bytes() == b"tree owner\n"
    assert (machine.repo / "a.txt").read_bytes() == b"main owner\n"
    assert ok(machine("repo.locate", {"checkoutId": checkout, "file": "a.txt"})) == {"root": str(tree.resolve()), "path": str((tree / "a.txt").resolve())}


def test_a_start_without_a_runtime_row_holds_the_checkout_and_a_draft_does_not(machine):
    runtime_task(machine.repo, "draft", "COMPLETED")
    db = sqlite3.connect(machine.repo / ".cortex/cortex.sqlite3")
    db.execute("UPDATE workspace_tasks SET execution_id=NULL WHERE task_id='draft'")
    db.execute("INSERT INTO workspace_tasks(task_id,project_id,execution_id) VALUES ('other','other-project','e-other')")
    db.commit()
    assert ok(machine("repo.file", {"file": "a.txt"}))["writer"] is None
    db.execute("INSERT INTO workspace_tasks(task_id,project_id,execution_id) VALUES ('starting','wp_1','e-starting')")
    db.commit()
    db.close()
    assert ok(save(machine, "owner\n", sha(b"one\ntwo\n"))) == {"outcome": "writer_active", "writer": "starting"}


def test_a_store_that_cannot_be_read_is_not_evidence_of_quiet(machine):
    (machine.repo / ".cortex").mkdir()
    (machine.repo / ".cortex/cortex.sqlite3").write_bytes(b"not a database" * 100)
    assert ok(save(machine, "owner\n", sha(b"one\ntwo\n"))) == {"outcome": "writer_active", "writer": "unconfirmed"}
    assert (machine.repo / "a.txt").read_bytes() == b"one\ntwo\n"


def test_locating_a_file_follows_the_same_confinement(machine):
    assert ok(machine("repo.locate", {}))["root"] == str(machine.repo)
    assert machine("repo.locate", {"file": "../outside"})["code"] == "invalid_path"
    assert machine("repo.locate", {"file": "missing.txt"})["code"] == "file_missing"


def test_files_are_bounded_and_binary_is_named(machine):
    (machine.repo / "big.bin").write_bytes(b"\0" * 10)
    assert ok(machine("repo.file", {"file": "a.txt"}))["text"] == "one\ntwo\n"
    assert ok(machine("repo.file", {"file": "big.bin"}))["text"] is None


def test_task_decisions_persist_and_finishing_is_required_to_archive(machine):
    task = {"taskId": "t"}
    ok(machine("product.task", {**task, "action": "rename", "title": "Named"}))
    assert not machine("product.task", {**task, "action": "archive"})["ok"]
    ok(machine("product.task", {**task, "action": "close", "choice": "as_is", "confirmed": True}))
    ok(machine("product.task", {**task, "action": "archive"}))
    ok(machine("product.task", {**task, "action": "answer_archive", "discard": True}))
    record = ok(machine("product.read"))["tasks"]["t"]
    assert (record["title"], record["status"], record["closedAs"]) == ("Named", "done", "as_is")
    assert record["archive"]["previousStatus"] == "done"
    ok(machine("product.task", {**task, "action": "restore"}))
    assert ok(machine("product.read"))["tasks"]["t"]["archive"] is None
    assert not machine("product.task", {**task, "action": "purge"})["ok"]


def test_closing_a_checked_ready_result_keeps_it_verified(machine):
    # A folder without Git finishes Ready to merge by closing; the checked result must not turn Not verified.
    ok(machine("product.task", {"taskId": "ready", "action": "auto_ready"}))
    ok(machine("product.task", {"taskId": "ready", "action": "close", "choice": "as_is", "confirmed": True}))
    ok(machine("product.task", {"taskId": "stopped", "action": "close", "choice": "as_is", "confirmed": True}))
    tasks = ok(machine("product.read"))["tasks"]
    assert (tasks["ready"]["status"], tasks["ready"]["closedAs"], tasks["ready"]["notVerifiedByCortex"]) == ("done", "as_is", False)
    assert tasks["stopped"]["notVerifiedByCortex"] is True


def test_merging_commits_on_the_open_branch_only(machine):
    (machine.repo / "a.txt").write_text("done\n")
    wrong = machine("product.integrate", {"taskId": "t", "method": "squash", "confirmed": True, "message": "m", "targetBranch": "other"})
    assert wrong["code"] == "stale_result"
    assert not machine("product.task", {"taskId":"t", "action":"accept", "digest":"d", "version":1})["ok"]
    assert ok(machine("repo.snapshot"))["commits"][0]["message"] == "first"


def test_sprints_hold_a_plan_and_refuse_unknown_changes(machine):
    made = ok(machine("product.sprint", {"action": "create", "description": "Do it", "runMode": "manual"}))
    ok(machine("product.sprint", {"action": "patch", "sprintId": made["sprintId"], "patch": {"planning": "ready"}}))
    assert ok(machine("product.read"))["sprints"][0]["planning"] == "ready"
    assert not machine("product.sprint", {"action": "patch", "sprintId": made["sprintId"], "patch": {"id": "x"}})["ok"]


def test_machine_settings_are_validated_and_reported(machine):
    assert not machine("machine.update", {"patch": {"retentionDays": 0}})["ok"]
    assert not machine("machine.update", {"patch": {"unknown": 1}})["ok"]
    assert not machine("machine.update", {"patch": {"worktreeRoot": "relative"}})["ok"]
    ok(machine("machine.update", {"patch": {"retentionDays": 14, "worktreeRoot": "~/trees", "name": "Desk"}}))
    seen = ok(machine("overview"))
    assert (seen["retentionDays"], seen["worktreeRoot"], seen["name"]) == (14, str(machine.repo.parent / "trees"), "Desk")


def test_diagnostics_and_cleanup_record_their_runs(machine):
    assert ok(machine("diagnostics.run"))["checks"] >= 5
    assert ok(machine("cleanup.run"))["removed"] == 0
    seen = ok(machine("overview"))
    assert seen["diagnostics"]["status"] in ("ok", "degraded") and seen["cleanup"]["asked"] == 0


def test_cleanup_archives_then_removes_on_the_machines_day_counts(machine, tmp_path):
    product = tmp_path / "state/product/wp_1.json"
    old = 1000
    product.parent.mkdir(parents=True)
    product.write_text(json.dumps({"tasks": {"done": {"status": "done", "doneAt": old, "work": "merged"}, "kept": {"status": "done", "doneAt": old, "work": "unmerged"},
                                             "stale": {"status": "done", "work": "merged", "archive": {"at": old, "previousStatus": "done"}}}}))
    run = ok(machine("cleanup.run"))
    assert (run["removed"], run["asked"]) == (1, 1)
    tasks = ok(machine("product.read"))["tasks"]
    assert tasks["done"]["archive"] and tasks["kept"]["archiveQuestion"] and tasks["stale"]["purged"]


def test_configuration_round_trips_through_the_cortex_api(machine):
    seen = ok(machine("config.read"))
    assert seen["config"]["surfaces"]["user_defaults"]["language"] == "auto"
    assert seen["connections"] == [] and seen["usage"] == []
    rules = seen["config"]["surfaces"]["routing"]
    rules["deny"] = ["claude-agent-acp/*"]
    ok(machine("config.save", {"surface": "routing", "value": rules, "tokens": seen["tokens"]}))
    again = ok(machine("config.read"))
    assert again["config"]["surfaces"]["routing"]["deny"] == ["claude-agent-acp/*"]
    assert again["config"]["revisions"]["routing"] != 0
    stale = machine("config.save", {"surface": "routing", "value": rules, "tokens": seen["tokens"]})
    assert stale["code"] == "revision_conflict"


def test_mcp_servers_keep_their_secrets_through_a_save(machine):
    server = {"name": "docs", "command": "/usr/bin/true", "args": ["--serve"], "enabled": False, "env": [{"name": "TOKEN", "stored": False, "action": "replace", "value": "s3cret"}], "allowedTools": ["search"], "readOnlyTools": ["search"], "readPaths": [], "writePaths": [], "network": False, "runtimeDirEnv": []}
    ok(machine("config.save", {"surface": "mcp_servers", "value": [server]}))
    seen = ok(machine("config.read"))
    assert "s3cret" not in json.dumps(seen)
    stored = seen["config"]["surfaces"]["mcp_servers"][0]
    assert stored["env"] == [{"name": "TOKEN", "stored": True, "action": "keep", "value": ""}]
    ok(machine("config.save", {"surface": "mcp_servers", "value": [{**stored, "args": ["--other"]}]}))
    assert ok(machine("config.read"))["config"]["surfaces"]["mcp_servers"][0]["env"][0]["stored"] is True


def test_project_routing_and_ceiling_are_real_and_revision_checked(machine):
    from cortex_runtime.store import Store
    Store(machine.repo / ".cortex", project_root=machine.repo)
    current = ok(machine("project.config.read"))
    rules = ok(machine("config.read"))["config"]["surfaces"]["routing"]
    ok(machine("project.config.save", {"surface":"routing", "value":rules, "tokens":current["tokens"]}))
    saved = ok(machine("project.config.read"))
    assert saved["value"]["routing"] == rules
    assert not machine("project.config.save", {"surface":"routing", "value":rules, "tokens":current["tokens"]})["ok"]
    ceiling = ok(machine("config.read"))["config"]["surfaces"]["user_defaults"]["ceiling"]
    ok(machine("project.config.save", {"surface":"project_ceiling", "value":ceiling, "tokens":saved["tokens"]}))
    assert "ceiling" in ok(machine("project.config.read"))["value"]


def test_logs_have_only_fixed_events_and_rotate(machine, tmp_path):
    import importlib.util
    spec = importlib.util.spec_from_file_location("logs_machine", TOOLS)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    state = tmp_path / "logs-state"
    for _ in range(2200):
        module.service_log(state, "helper_ok")
    with pytest.raises(ValueError):
        module.service_log(state, "prompt-secret")
    folder = module.log_folder(state)
    files = list(folder.glob("service.log*"))
    assert len(files) == 2 and sum(p.stat().st_size for p in files) <= 131_200
    for path in files:
        assert path.stat().st_mode & 0o777 == 0o600
        for line in path.read_text().splitlines():
            assert set(json.loads(line)) == {"at", "event"}
            assert json.loads(line)["event"] == "helper_ok"
    assert folder.stat().st_mode & 0o777 == 0o700
    assert ok(machine("logs.folder"))["folder"].endswith("/logs")


def test_persistent_reclamation_failure_count_includes_manual_purges(machine, tmp_path):
    product = tmp_path / "state/product/wp_1.json"
    product.parent.mkdir(parents=True)
    product.write_text(json.dumps({"tasks":{
        "failed":{"purged":True,"reclamation":{"vacuum_status":"failed"}},
        "ok":{"purged":True,"reclamation":{"vacuum_status":"completed"}}
    }}))
    assert ok(machine("overview"))["reclamationFailures"] == 1


def test_sprint_target_inheritance_does_not_overwrite_task_override(machine):
    sprint = ok(machine("product.sprint", {"action":"create","description":"Sprint","targetBranch":"release"}))["sprintId"]
    ok(machine("product.task", {"action":"sprint","taskId":"first","sprintId":sprint}))
    ok(machine("product.task", {"action":"sprint","taskId":"second","sprintId":sprint}))
    ok(machine("product.task", {"action":"target","taskId":"second","branch":"owner-choice"}))
    ok(machine("product.sprint", {"action":"patch","sprintId":sprint,"patch":{"targetBranch":"next-release"}}))
    tasks = ok(machine("product.read"))["tasks"]
    assert tasks["first"]["targetBranch"] == "next-release"
    assert tasks["second"]["targetBranch"] == "owner-choice"


@pytest.mark.parametrize("enabled", [True, False])
def test_every_connection_route_remains_in_the_catalogue(tmp_path, monkeypatch, enabled):
    import importlib.util
    from types import SimpleNamespace
    from cortex_runtime.configuration import management
    from cortex_runtime.connections import registry
    spec = importlib.util.spec_from_file_location("catalogue_machine", TOOLS)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    routes = [{"engine":"codex-acp","selectable":True,"in_selector":enabled,"enabled":enabled,"disabled":not enabled,"model":"model","route_id":r} for r in ("connection-one:model","connection-two:model")]
    monkeypatch.setattr(management, "read_all", lambda *_: {})
    monkeypatch.setattr(registry, "read_registry", lambda: SimpleNamespace(connections={}))
    monkeypatch.setattr(module, "config_call", lambda request: {"routes":routes} if request["surface"] == "route_catalog" else {})
    monkeypatch.setattr(module, "plan_usage", lambda: ([],{}))
    config = module.config_read(tmp_path)["config"]
    models = config["catalogue"]["codex-acp"]["models"]
    assert config["surfaces"]["routing"]["engines"]["codex-acp"]["enabled"] == []
    assert [m["routeId"] for m in models] == ["connection-one:model","connection-two:model"]
    assert models[0]["label"] != models[1]["label"]
    assert not any(m["routeId"] in m["label"] for m in models)


def test_a_model_label_is_its_name_and_never_carries_a_route_id():
    spec = importlib.util.spec_from_file_location("label_machine", TOOLS)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    models = [{"label": "Opus 5.5", "routeId": "xr_6b08617e228a"}, {"label": "Opus 5.5", "routeId": "xr_e91c9bdc9d95"}, {"label": "Sonnet 5", "routeId": "xr_3633a742026f"}]
    module.tell_apart(models, {"xr_6b08617e228a": "Work", "xr_e91c9bdc9d95": "Home"})
    assert [m["label"] for m in models] == ["Opus 5.5 · Work", "Opus 5.5 · Home", "Sonnet 5"]


def test_a_refused_defaults_save_does_not_write_workspace_only_values(machine):
    old = ok(machine("config.read"))
    ok(machine("config.action", {"action": "apply_default_preset"}))
    before = ok(machine("config.read"))["config"]["surfaces"]["user_defaults"]
    value = {**old["config"]["surfaces"]["user_defaults"], "mode": "auto", "checkout": "worktree"}
    assert machine("config.save", {"surface": "user_defaults", "value": value, "tokens": old["tokens"]})["code"] == "revision_conflict"
    after = ok(machine("config.read"))["config"]["surfaces"]["user_defaults"]
    assert (after["mode"], after["checkout"]) == (before["mode"], before["checkout"])


def test_revert_one_hunk_keeps_the_other_unstaged_change(machine):
    original = [f"{n}\n" for n in range(40)]
    (machine.repo / "long.txt").write_text("".join(original))
    machine.git("add", "long.txt")
    machine.git("commit", "-m", "long")
    changed = original[:]
    changed[1], changed[35] = "first\n", "second\n"
    (machine.repo / "long.txt").write_text("".join(changed))
    ok(machine("git", {"action": "revert", "path": "long.txt", "hunk": 0, "confirmed": True}))
    result = (machine.repo / "long.txt").read_text().splitlines()
    assert result[1] == "1" and result[35] == "second"
    assert ok(machine("repo.snapshot"))["checkouts"][0]["files"][0]["add"] == 1


def test_amend_uses_the_message_and_keeps_one_commit(machine):
    before = ok(machine("repo.snapshot"))["commits"][0]["sha"]
    ok(machine("git", {"action": "commit", "message": "Amended subject\n\nPasted body", "amend": True}))
    rows = ok(machine("repo.snapshot"))["commits"]
    assert len(rows) == 1 and rows[0]["sha"] != before
    result = subprocess.run(["git", "log", "-1", "--format=%B"], cwd=machine.repo, capture_output=True, text=True, check=True)
    assert result.stdout.strip() == "Amended subject\n\nPasted body"


def test_an_unknown_operation_is_refused(machine):
    assert machine("shell", {"line": "true"})["code"] == "unsupported_action"


def test_coordinator_helper_process_reopen_pages_and_project_binding(machine):
    from cortex_runtime.acp.replay_store import append_host_transcript, append_host_update
    from test_workspace_api import api_for, create_command, runtime_binding
    api = api_for(machine.repo)
    task = api.handle(create_command())["target"]
    runtime_binding(api, task)
    for n in range(105):
        append_host_transcript(api.store, "session-1", "agent_message", f"Coordinator note {n}")
    append_host_update(api.store, "session-1", dict(sessionUpdate="tool_call", toolCallId="edit",
        title="Edit", kind="edit", rawInput={"path": "index.html", "old_string": "old", "new_string": "new"}))
    params = dict(taskIds=[task], workerId="coordinator:" + task)
    first = ok(machine("workers", params, project=api.project_id))[0]
    assert first["activityPartial"] and first["activityCursor"]
    append_host_transcript(api.store, "session-1", "agent_message", "Live tail after snapshot")
    # Each machine() runs a fresh helper, so continuation never relies on process memory.
    second = ok(machine("workers", {**params, "cursor": first["activityCursor"]}, project=api.project_id))[0]
    assert not second["activityPartial"] and second["activityCursor"] is None
    activity = first["activity"] + second["activity"]
    assert len(activity) == 106 and len({a["sequence"] for a in activity}) == 106
    assert activity[-1]["diff"]["lines"] == ["-old", "+new"]
    assert activity[-1]["text"] == "Edit"
    assert "Live tail after snapshot" not in str(activity)
    assert ok(machine("workers", params, project="another-project")) == []
    assert machine("workers", {**params, "taskIds": ["other-task"]}, project=api.project_id)["code"] == "unknown_task"
    assert machine("workers", {**params, "cursor": "wa2.foreign.1.1"}, project=api.project_id)["code"] == "invalid_cursor"
