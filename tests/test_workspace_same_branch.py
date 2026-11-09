"""Shared checkout integration through real Git and recorded gateway file receipts."""
import hashlib
import json
import sqlite3

import pytest

from test_workspace_task_git import git_machine as machine, helper, task_git
from test_workspace_machine_tools import machine as base_machine


@pytest.fixture
def shared(machine, tmp_path, monkeypatch):
    dbpath = tmp_path / "evidence.sqlite3"
    db = sqlite3.connect(dbpath)
    db.executescript("""
        CREATE TABLE workspace_tasks(task_id,execution_id,project_id);
        CREATE TABLE workspace_executions(execution_id,runtime_task_id);
        CREATE TABLE trusted_channels(channel_id,task_id);
        CREATE TABLE runtime_events(id INTEGER PRIMARY KEY,task_id,type,actor,payload_json,created_at DEFAULT '2000-01-01T00:00:00+00:00');
        INSERT INTO workspace_tasks VALUES('first','e1','wp_1'),('second','e2','wp_1');
        INSERT INTO workspace_executions VALUES('e1','r1'),('e2','r2');
        INSERT INTO trusted_channels VALUES('c1','r1'),('c2','r2');
    """)
    db.commit()
    def reader(_):
        connection = sqlite3.connect(dbpath)
        connection.row_factory = sqlite3.Row
        return connection
    monkeypatch.setattr(helper, "store_reader", reader)
    base = helper.out(machine.repo, "rev-parse", "HEAD").strip()
    product = helper.product_of(tmp_path / "state", "wp_1")
    for key in ("first", "second"):
        product["tasks"][key] = dict(status="ready_to_merge", checkout=dict(kind="main", branch="main"),
                                    baseRevision=base, targetBranch="main", acceptance=dict(digest="reviewed"))
    def save():
        helper.write_json(helper.product_path(tmp_path / "state", "wp_1"), product)
    save()
    def write(task, path, text):
        (machine.repo / path).write_text(text)
        result = dict(full_content_sha256=hashlib.sha256(text.encode()).hexdigest(), size_bytes=len(text.encode()))
        db.execute("INSERT INTO runtime_events(task_id,type,actor,payload_json) VALUES (?,?,?,?)",
                   ("r1" if task == "first" else "r2", "TOOL_COMPLETED", "c1" if task == "first" else "c2",
                    json.dumps(dict(operation="project_write", path=path, result=result))))
        db.commit()
    def facts(task="first"):
        return helper.product_read(str(machine.repo), tmp_path / "state", "wp_1")["tasks"][task]["integrationFacts"]
    yield machine, product, save, write, facts
    db.close()


@pytest.mark.parametrize("baseline", [True, False])
def test_committed_task_ignores_other_tasks_dirty_and_later_same_file_edits(shared, baseline):
    machine, product, save, write, facts = shared
    if not baseline:
        product["tasks"]["first"].pop("baseRevision")
        save()
    write("first", "a.txt", "first task\n")
    machine.git("add", "a.txt"); machine.git("commit", "-m", "First task")
    write("second", "other.txt", "second task\n")
    assert facts()["alreadyInTarget"] is True
    assert facts("second")["alreadyInTarget"] is False
    write("second", "a.txt", "second task later edits same file\n")
    assert facts()["alreadyInTarget"] is True
    machine.git("add", "a.txt"); machine.git("commit", "-m", "Later edit")
    assert facts()["alreadyInTarget"] is True


def test_commit_only_own_pending_files_preserves_other_staging(shared):
    machine, product, save, write, facts = shared
    write("first", "mine.txt", "mine\n")
    write("second", "other.txt", "other\n")
    machine.git("add", "other.txt")
    assert facts()["sameBranch"] is True
    assert facts()["pendingPaths"] == ["mine.txt"]
    assert facts()["otherUncommitted"] is True
    (machine.repo / ".cortex-dev").mkdir()
    (machine.repo / ".cortex-dev/private").write_text("private")
    before = helper.out(machine.repo, "rev-parse", "HEAD")
    result = task_git.integrate(helper, str(machine.repo), "wp_1", machine.repo.parent / "state",
        dict(taskId="first", method="commit", targetBranch="main", message="Only mine",
             confirmed=True, digest="reviewed"))
    assert result["done"] is True
    assert helper.out(machine.repo, "diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD").splitlines() == ["mine.txt"]
    assert helper.out(machine.repo, "diff", "--cached", "--name-only").splitlines() == ["other.txt"]
    assert helper.out(machine.repo, "rev-parse", "HEAD") != before
    saved = helper.product_of(machine.repo.parent / "state", "wp_1")["tasks"]["first"]
    assert saved["integration"]["method"] == "commit"


def test_mixed_committed_and_pending_paths(shared):
    machine, _, _, write, facts = shared
    write("first", "a.txt", "first committed\n")
    machine.git("add", "a.txt"); machine.git("commit", "-m", "Part one")
    write("first", "pending.txt", "part two\n")
    write("second", "other.txt", "other\n")
    assert facts()["alreadyInTarget"] is False
    assert facts()["pendingPaths"] == ["pending.txt"]
    assert facts()["commitAvailable"] is True


def test_unknown_or_overlapping_pending_work_has_honest_exit(shared):
    machine, product, save, write, facts = shared
    product["tasks"]["first"].pop("baseRevision")
    save()
    assert facts()["alreadyInTarget"] is False
    assert facts()["commitAvailable"] is False
    assert facts()["reason"]
    write("first", "a.txt", "mine pending\n")
    write("second", "a.txt", "other replaced mine\n")
    assert facts()["alreadyInTarget"] is False
    assert facts()["commitAvailable"] is False
    before = helper.out(machine.repo, "rev-parse", "HEAD")
    with pytest.raises(helper.Refused):
        task_git.integrate(helper, str(machine.repo), "wp_1", machine.repo.parent / "state",
            dict(taskId="first", method="commit", targetBranch="main", message="Unsafe",
                 confirmed=True, digest="reviewed"))
    assert helper.out(machine.repo, "rev-parse", "HEAD") == before
    assert (machine.repo / "a.txt").read_text() == "other replaced mine\n"


@pytest.mark.parametrize("sibling", [False, True])
def test_own_unverifiable_file_is_not_reported_as_other_tasks(shared, sibling):
    machine, _, _, write, facts = shared
    write("first", "a.txt", "recorded\n")
    (machine.repo / "a.txt").write_text("edited after the recorded write\n")
    if sibling:
        write("second", "other.txt", "sibling\n")
    result = facts()
    assert result["pendingPaths"] == [] and result["commitAvailable"] is False
    assert result["reason"]
    assert result["otherUncommitted"] is sibling


@pytest.mark.parametrize("baseline", [True, False])
def test_uncommitted_task_can_be_committed_without_touching_sibling(shared, baseline):
    machine, product, save, write, facts = shared
    if not baseline:
        product["tasks"]["first"].pop("baseRevision")
        save()
    write("first", "mine.txt", "mine pending\n")
    write("second", "other.txt", "sibling\n")
    assert facts()["commitAvailable"] is True
    assert facts()["otherUncommitted"] is True


def test_failed_commit_restores_index_and_can_retry(shared, tmp_path, monkeypatch):
    machine, _, _, write, facts = shared
    write("first", "mine.txt", "mine\n")
    write("second", "other.txt", "sibling\n")
    machine.git("add", "other.txt")
    index = machine.repo / ".git/index"
    before = index.read_bytes()
    hook = machine.repo / ".git/hooks/pre-commit"
    hook.write_text("#!/bin/sh\nexit 1\n"); hook.chmod(0o755)
    # The real helper disables repository hooks; inject a failing command route,
    # just as the separate-worktree rollback regression does.
    real_git = helper.git
    hooks = machine.repo / ".git/hooks"
    monkeypatch.setattr(helper, "git", lambda cwd, *args, **kw: real_git(cwd, "-c", f"core.hooksPath={hooks}", *args, **kw))
    params = dict(taskId="first", method="commit", targetBranch="main", message="Only mine",
                  confirmed=True, digest="reviewed")
    with pytest.raises(helper.Refused, match="integration_failed"):
        task_git.integrate(helper, str(machine.repo), "wp_1", tmp_path / "state", params)
    assert index.read_bytes() == before
    assert (machine.repo / "mine.txt").read_text() == "mine\n"
    hook.unlink()
    assert facts()["commitAvailable"] is True
    assert task_git.integrate(helper, str(machine.repo), "wp_1", tmp_path / "state", params)["done"]
    assert helper.out(machine.repo, "diff", "--cached", "--name-only").splitlines() == ["other.txt"]


def test_same_file_foreign_staged_version_is_not_overwritten(shared):
    machine, _, _, write, facts = shared
    write("second", "a.txt", "foreign staged\n"); machine.git("add", "a.txt")
    write("first", "a.txt", "mine pending\n")
    assert facts()["commitAvailable"] is False
    assert helper.out(machine.repo, "show", ":a.txt") == "foreign staged\n"


def test_real_helper_process_refreshes_legacy_record_without_repair(shared, tmp_path):
    import shutil
    machine, product, save, write, _ = shared
    product["tasks"]["first"].pop("baseRevision"); save()
    write("first", "a.txt", "landed\n")
    machine.git("add", "a.txt"); machine.git("commit", "-m", "Landed")
    write("second", "other.txt", "sibling pending\n")
    store = machine.repo / ".cortex"; store.mkdir()
    shutil.copyfile(tmp_path / "evidence.sqlite3", store / "cortex.sqlite3")
    reply = machine("product.read")
    assert reply["ok"], reply
    record = reply["result"]["tasks"]["first"]
    assert record["integrationFacts"]["alreadyInTarget"] is True
    assert record["status"] == "ready_to_merge"
    assert helper.product_of(tmp_path / "state", "wp_1")["tasks"]["first"].get("baseRevision") is None


def test_landed_file_stays_landed_after_sibling_replaces_it_with_a_link(shared, tmp_path):
    machine, _, _, write, facts = shared
    write("first", "a.txt", "landed\n")
    machine.git("add", "a.txt"); machine.git("commit", "-m", "Landed")
    outside = tmp_path / "not-read.txt"; outside.write_text("other")
    (machine.repo / "a.txt").unlink()
    (machine.repo / "a.txt").symlink_to(outside)
    assert facts()["alreadyInTarget"] is True


def test_same_branch_squash_is_never_silently_a_commit(shared):
    machine, _, _, write, _ = shared
    write("first", "a.txt", "pending\n")
    with pytest.raises(helper.Refused, match="same_branch"):
        task_git.integrate(helper, str(machine.repo), "wp_1", machine.repo.parent / "state",
            dict(taskId="first", method="squash", targetBranch="main", message="Wrong action",
                 confirmed=True, digest="reviewed"))
