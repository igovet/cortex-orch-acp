"""Local machine controls, without paid engines or owner data."""
from pathlib import Path
import pytest
from test_workspace_machine_tools import machine, ok
from test_workspace_task_git import helper


def test_worktree_root_allows_external_cortex_folder(machine, tmp_path):
    path = tmp_path / ".cortex/worktrees"
    ok(machine("machine.update", {"patch": {"worktreeRoot": str(path)}}))
    assert ok(machine("overview"))["worktreeRoot"] == str(path)
    assert not path.exists(), "Saving a setting must not create the folder"


def test_worktree_root_rejects_files_repositories_and_registered_plain_projects(machine, tmp_path):
    file = tmp_path / "file"
    file.write_text("not a directory")
    for path in [file / "trees", machine.repo / "trees"]:
        reply = machine("machine.update", {"patch": {"worktreeRoot": str(path)}})
        assert reply["code"] == "unsafe_worktree_root"
        assert reply["message"]
    # A registered non-Git folder is also a project, not a valid worktree home.
    plain = tmp_path / "plain"
    plain.mkdir()
    with pytest.raises(helper.Refused, match="unsafe_worktree_root") as refused:
        helper.machine_update(tmp_path / "direct-state", {"patch": {"worktreeRoot": str(plain / "trees")}},
                              [{"root": str(plain)}])
    assert "outside" in refused.value.message


def test_missing_diagnostic_tool_has_reason_and_recovery(tmp_path, monkeypatch):
    tmp_path.mkdir(exist_ok=True)
    monkeypatch.setattr(helper.shutil, "which", lambda name: None if name == "npm" else "/usr/bin/" + name)
    run = helper.diagnostics(tmp_path, [])
    item = next(x for x in run["items"] if x["name"] == "npm")
    assert run["status"] == "degraded"
    assert "not" in item["detail"].lower()
    assert "npm" in item["action"]
    assert "PATH" not in item["action"], "Use plain words rather than environment internals"


def test_revert_keeps_unrelated_staging(machine):
    (machine.repo / "a.txt").write_text("changed\n")
    machine.git("add", "a.txt"); machine.git("commit", "-m", "Change")
    sha = helper.out(machine.repo, "rev-parse", "HEAD").strip()
    (machine.repo / "other.txt").write_text("owner staging\n"); machine.git("add", "other.txt")
    reply = machine("git", {"action": "revert_commit", "sha": sha, "confirmed": True})
    assert reply["code"] == "dirty_index"
    assert helper.out(machine.repo, "rev-parse", "HEAD").strip() == sha
    assert helper.out(machine.repo, "diff", "--cached", "--name-only").strip() == "other.txt"


def test_generated_revert_message_is_english(machine):
    (machine.repo / "a.txt").write_text("changed\n")
    machine.git("add", "a.txt"); machine.git("commit", "-m", "Русское описание")
    sha = helper.out(machine.repo, "rev-parse", "HEAD").strip()
    ok(machine("git", {"action": "revert_commit", "sha": sha, "confirmed": True}))
    message = helper.out(machine.repo, "log", "-1", "--format=%B")
    assert message.startswith("Revert commit ")
    assert not any("\u0400" <= c <= "\u04ff" for c in message)


def test_local_snapshot_commit_and_editor_location(machine, tmp_path):
    repo = ok(machine("repo.snapshot"))
    assert repo["remote"] == ""
    assert repo["defaultBranch"] == "main"
    assert repo["commits"][0]["pushed"] is False
    assert ok(machine("repo.locate"))["root"] == str(machine.repo)
    tree = tmp_path / "tree"
    machine.git("worktree", "add", "-b", "task", str(tree))
    checkout = next(x for x in ok(machine("repo.snapshot"))["checkouts"] if x["kind"] == "worktree")
    assert ok(machine("repo.locate", {"checkoutId": checkout["id"]}))["root"] == str(tree)
    (machine.repo / "a.txt").write_text("local change\n")
    ok(machine("git", {"action": "stage", "path": "a.txt", "staged": True}))
    ok(machine("git", {"action": "commit", "message": "Save local changes"}))
    snapshot = ok(machine("repo.snapshot"))
    assert snapshot["commits"][0]["message"] == "Save local changes"
    assert snapshot["commits"][0]["pushed"] is False
    assert not snapshot["checkouts"][0]["files"]
