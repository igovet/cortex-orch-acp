"""T019 design point 4: observed (not claimed) artifact revision binding.

Exercises ``delegation.revision`` directly against real temporary
directories/git repositories (never the real checkout -- see
``tests/conftest.py``'s T009 incident guard) and the completion gate's
stale_verification refusal.

The module under test must never execute git (or any other
repo-configurable program) to observe a revision -- a worker can write
into the shared workspace's ``.git/config``/``.gitattributes`` before a
capture runs, and plain git plumbing honors those files even under a
locked-down environment. These tests plant exactly that kind of poisoned
repo config and prove the marker program it would have run never runs,
and that no subprocess is ever spawned by a capture at all.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import time
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import completion as delegation_completion
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.delegation import revision as delegation_revision
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


def _git(args, cwd, env=None):
    full_env = dict(PATH="/usr/bin:/bin", HOME=str(cwd), GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL="/dev/null")
    if env:
        full_env.update(env)
    return subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True, check=True, env=full_env)


def _init_repo(root):
    _git(["init", "-q"], root)
    _git(["config", "user.email", "test@example.com"], root)
    _git(["config", "user.name", "Test"], root)
    (root / "a.txt").write_text("one\n")
    _git(["add", "a.txt"], root)
    _git(["commit", "-q", "-m", "init"], root)


def test_capture_revision_on_real_git_repo(tmp_path):
    _init_repo(tmp_path)
    captured = delegation_revision.capture_revision(tmp_path)
    assert captured["kind"] == "observed"
    assert len(captured["head"]) == 40
    clean_digest = captured["dirty_digest"]

    (tmp_path / "a.txt").write_text("changed\n")
    dirty = delegation_revision.capture_revision(tmp_path)
    assert dirty["kind"] == "observed" and dirty["head"] == captured["head"]
    assert dirty["dirty_digest"] != clean_digest


def test_capture_revision_non_git_directory_is_observed_with_no_head(tmp_path):
    """A plain (non-git) directory is not unavailable -- the tree digest
    does not require git at all; HEAD is simply absent."""
    (tmp_path / "a.txt").write_text("one\n")
    captured = delegation_revision.capture_revision(tmp_path)
    assert captured["kind"] == "observed"
    assert captured["head"] is None
    assert captured["dirty_digest"]


def test_capture_revision_never_spawns_a_subprocess(tmp_path, monkeypatch):
    """A capture must never execute anything -- no git, no filter driver,
    no shell -- ever, on any code path."""
    _init_repo(tmp_path)

    def _forbidden(*args, **kwargs):
        raise AssertionError("capture_revision must never spawn a process")

    monkeypatch.setattr(subprocess, "Popen", _forbidden)
    monkeypatch.setattr(os, "execvp", _forbidden, raising=False)
    monkeypatch.setattr(os, "execve", _forbidden, raising=False)
    monkeypatch.setattr(os, "posix_spawn", _forbidden, raising=False)
    monkeypatch.setattr(os, "system", _forbidden)

    captured = delegation_revision.capture_revision(tmp_path)
    assert captured["kind"] == "observed"


def test_capture_revision_never_executes_a_poisoned_fsmonitor_or_clean_filter(tmp_path):
    """A worker-writable ``.git/config`` naming a ``core.fsmonitor`` hook,
    and a ``.gitattributes`` naming a clean filter, must never run --
    proven by a marker file each would create that stays absent."""
    _init_repo(tmp_path)
    marker = tmp_path / "pwned.marker"
    hook = tmp_path / "fsmonitor-hook.sh"
    hook.write_text(f"#!/bin/sh\ntouch '{marker}'\necho '{{}}'\n")
    hook.chmod(0o755)
    git_config = tmp_path / ".git" / "config"
    git_config.write_text(
        git_config.read_text() + f"\n[core]\n\tfsmonitor = {hook}\n"
    )
    clean_hook = tmp_path / "clean-hook.sh"
    clean_hook.write_text(f"#!/bin/sh\ntouch '{marker}'\ncat\n")
    clean_hook.chmod(0o755)
    (tmp_path / ".gitattributes").write_text("a.txt filter=poison\n")
    with (tmp_path / ".git" / "config").open("a") as fh:
        fh.write(f"[filter \"poison\"]\n\tclean = {clean_hook}\n\tsmudge = cat\n")

    captured = delegation_revision.capture_revision(tmp_path)
    assert captured["kind"] == "observed"
    assert not marker.exists(), "capture_revision must never execute repo-configured programs"


def test_capture_revision_detects_add_modify_delete_and_symlink_changes(tmp_path):
    (tmp_path / "a.txt").write_text("one\n")
    (tmp_path / "sub").mkdir()
    (tmp_path / "sub" / "b.txt").write_text("two\n")
    base = delegation_revision.capture_revision(tmp_path)["dirty_digest"]

    (tmp_path / "a.txt").write_text("one-modified\n")
    assert delegation_revision.capture_revision(tmp_path)["dirty_digest"] != base

    (tmp_path / "a.txt").write_text("one\n")  # restore
    (tmp_path / "new.txt").write_text("added\n")
    added = delegation_revision.capture_revision(tmp_path)["dirty_digest"]
    assert added != base
    (tmp_path / "new.txt").unlink()
    assert delegation_revision.capture_revision(tmp_path)["dirty_digest"] == base

    (tmp_path / "link.txt").symlink_to("a.txt")
    with_link = delegation_revision.capture_revision(tmp_path)["dirty_digest"]
    assert with_link != base
    (tmp_path / "link.txt").unlink()
    (tmp_path / "link.txt").symlink_to("sub")  # retarget to a different name entirely
    retargeted = delegation_revision.capture_revision(tmp_path)["dirty_digest"]
    assert retargeted != with_link


def test_capture_revision_ignores_dot_git_and_dot_cortex_changes(tmp_path):
    _init_repo(tmp_path)
    base = delegation_revision.capture_revision(tmp_path)["dirty_digest"]
    (tmp_path / ".cortex").mkdir()
    (tmp_path / ".cortex" / "state.db").write_text("whatever\n")
    with_cortex = delegation_revision.capture_revision(tmp_path)["dirty_digest"]
    assert with_cortex == base
    (tmp_path / ".git" / "some-scratch-file").write_text("noise\n")
    with_git_noise = delegation_revision.capture_revision(tmp_path)["dirty_digest"]
    assert with_git_noise == base


def test_capture_revision_mode_independent(tmp_path):
    (tmp_path / "a.txt").write_text("one\n")
    before = delegation_revision.capture_revision(tmp_path)["dirty_digest"]
    (tmp_path / "a.txt").chmod(0o600)
    after = delegation_revision.capture_revision(tmp_path)["dirty_digest"]
    assert before == after


def test_capture_revision_unavailable_when_file_count_cap_exceeded(tmp_path, monkeypatch):
    (tmp_path / "a.txt").write_text("one\n")
    monkeypatch.setattr(delegation_revision, "_MAX_FILES", 0)
    captured = delegation_revision.capture_revision(tmp_path)
    assert captured["kind"] == "unavailable"
    assert captured["detail"] == "workspace_too_large"
    assert captured["head"] is None and captured["dirty_digest"] is None


def test_capture_revision_unavailable_when_time_budget_exceeded(tmp_path, monkeypatch):
    (tmp_path / "a.txt").write_text("one\n")
    monkeypatch.setattr(delegation_revision, "_TIME_BUDGET_SECONDS", -1.0)
    captured = delegation_revision.capture_revision(tmp_path)
    assert captured["kind"] == "unavailable"
    assert captured["detail"] == "capture_timed_out"


def test_capture_revision_unavailable_on_unreadable_entry(tmp_path):
    sub = tmp_path / "locked"
    sub.mkdir()
    (sub / "inner.txt").write_text("secret\n")
    sub.chmod(0o000)
    try:
        if os.access(str(sub), os.R_OK):
            pytest.skip("running as a user that bypasses directory permissions (e.g. root)")
        captured = delegation_revision.capture_revision(tmp_path)
        assert captured["kind"] == "unavailable"
        assert captured["detail"].startswith("unreadable_entry:")
    finally:
        sub.chmod(0o755)


def test_capture_revision_does_not_follow_a_symlinked_directory(tmp_path):
    outside = tmp_path.parent / f"revision-outside-{uuid.uuid4().hex}"
    outside.mkdir()
    (outside / "secret.txt").write_text("outside\n")
    try:
        (tmp_path / "escape").symlink_to(outside, target_is_directory=True)
        captured = delegation_revision.capture_revision(tmp_path)
        assert captured["kind"] == "observed"  # recorded as a symlink entry, never traversed
        (outside / "secret.txt").write_text("changed\n")
        captured2 = delegation_revision.capture_revision(tmp_path)
        assert captured2["dirty_digest"] == captured["dirty_digest"]
    finally:
        import shutil
        shutil.rmtree(outside, ignore_errors=True)


def test_capture_revision_detached_head_sha(tmp_path):
    _init_repo(tmp_path)
    head = _git(["rev-parse", "HEAD"], tmp_path).stdout.strip()
    _git(["checkout", "-q", head], tmp_path)
    captured = delegation_revision.capture_revision(tmp_path)
    assert captured["kind"] == "observed"
    assert captured["head"] == head


def test_capture_revision_packed_refs_head(tmp_path):
    _init_repo(tmp_path)
    expected = _git(["rev-parse", "HEAD"], tmp_path).stdout.strip()
    _git(["pack-refs", "--all"], tmp_path)
    branch_ref = tmp_path / ".git" / "refs" / "heads"
    for entry in list(branch_ref.iterdir()) if branch_ref.exists() else []:
        entry.unlink()
    captured = delegation_revision.capture_revision(tmp_path)
    assert captured["kind"] == "observed"
    assert captured["head"] == expected


def test_capture_revision_worktree_gitdir_pointer(tmp_path):
    main_repo = tmp_path / "main"
    main_repo.mkdir()
    _init_repo(main_repo)
    worktree = tmp_path / "wt"
    _git(["worktree", "add", "-q", str(worktree), "-b", "wt-branch"], main_repo)
    expected = _git(["rev-parse", "HEAD"], worktree).stdout.strip()
    captured = delegation_revision.capture_revision(worktree)
    assert captured["kind"] == "observed"
    assert captured["head"] == expected


@pytest.mark.skipif(not hasattr(os, "mkfifo"), reason="FIFOs require a POSIX platform")
def test_capture_revision_tolerates_fifo_swapped_in_for_a_regular_file(tmp_path, monkeypatch):
    """A path classified as a regular file by scandir/lstat can be swapped
    for a FIFO before the hashing open runs (a worker's in-flight write
    into the shared workspace can do exactly this). The capture must
    never block on it -- it must return promptly as unavailable, never
    hang past the wall-clock budget."""
    fifo_path = tmp_path / "swapped.txt"
    os.mkfifo(fifo_path)
    monkeypatch.setattr(delegation_revision, "_classify", lambda entry: "f")

    started = time.monotonic()
    captured = delegation_revision.capture_revision(tmp_path)
    elapsed = time.monotonic() - started

    assert elapsed < 2.0, f"capture blocked on a FIFO for {elapsed}s"
    assert captured["kind"] == "unavailable"
    assert captured["detail"].startswith("unreadable_entry:")


@pytest.mark.skipif(not hasattr(os, "mkfifo"), reason="FIFOs require a POSIX platform")
def test_capture_revision_fifo_as_head_returns_promptly_with_no_head(tmp_path):
    _init_repo(tmp_path)
    head_path = tmp_path / ".git" / "HEAD"
    head_path.unlink()
    os.mkfifo(head_path)

    started = time.monotonic()
    captured = delegation_revision.capture_revision(tmp_path)
    elapsed = time.monotonic() - started

    assert elapsed < 2.0, f"capture blocked reading a FIFO HEAD for {elapsed}s"
    assert captured["kind"] == "observed"
    assert captured["head"] is None


@pytest.mark.skipif(not hasattr(os, "mkfifo"), reason="FIFOs require a POSIX platform")
def test_capture_revision_fifo_as_loose_ref_returns_promptly_with_no_head(tmp_path):
    _init_repo(tmp_path)
    branch = (tmp_path / ".git" / "HEAD").read_text().strip()
    assert branch.startswith("ref:")
    ref_name = branch[len("ref:"):].strip()
    ref_path = tmp_path / ".git" / ref_name
    ref_path.unlink()
    os.mkfifo(ref_path)

    started = time.monotonic()
    captured = delegation_revision.capture_revision(tmp_path)
    elapsed = time.monotonic() - started

    assert elapsed < 2.0, f"capture blocked reading a FIFO loose ref for {elapsed}s"
    assert captured["kind"] == "observed"
    assert captured["head"] is None


@pytest.mark.skipif(not hasattr(os, "mkfifo"), reason="FIFOs require a POSIX platform")
def test_capture_revision_fifo_as_packed_refs_returns_promptly_with_no_head(tmp_path):
    _init_repo(tmp_path)
    _git(["pack-refs", "--all"], tmp_path)
    branch_ref = tmp_path / ".git" / "refs" / "heads"
    for entry in list(branch_ref.iterdir()) if branch_ref.exists() else []:
        entry.unlink()
    packed = tmp_path / ".git" / "packed-refs"
    packed.unlink()
    os.mkfifo(packed)

    started = time.monotonic()
    captured = delegation_revision.capture_revision(tmp_path)
    elapsed = time.monotonic() - started

    assert elapsed < 2.0, f"capture blocked reading a FIFO packed-refs for {elapsed}s"
    assert captured["kind"] == "observed"
    assert captured["head"] is None


@pytest.mark.skipif(not hasattr(os, "mkfifo"), reason="FIFOs require a POSIX platform")
def test_capture_revision_pre_existing_fifo_is_typed_never_opened(tmp_path, monkeypatch):
    (tmp_path / "a.txt").write_text("one\n")
    fifo_path = tmp_path / "a.fifo"
    os.mkfifo(fifo_path)

    def _forbidden_open(*args, **kwargs):
        raise AssertionError("a pre-existing FIFO must never be opened")

    real_open = os.open

    def _guarded_open(path, *args, **kwargs):
        if os.fspath(path) == str(fifo_path):
            _forbidden_open()
        return real_open(path, *args, **kwargs)

    monkeypatch.setattr(os, "open", _guarded_open)
    captured = delegation_revision.capture_revision(tmp_path)
    assert captured["kind"] == "observed"


def test_capture_revision_missing_root_is_unavailable(tmp_path):
    missing = tmp_path / "does-not-exist"
    captured = delegation_revision.capture_revision(missing)
    assert captured["kind"] == "unavailable"
    assert captured["head"] is None and captured["dirty_digest"] is None


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build.", "owner-1")
    return store, result.task_id


def _record_result_with_observed_check(store, task_id, assignment_ref, report_ref):
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute("INSERT INTO reports VALUES (?,?,?,?,?)", (report_ref, task_id, "report", f"{report_ref}.md", store._now()))
        db.commit()
    with store.connection() as db:
        row = db.execute("SELECT project_root FROM tasks WHERE id=?", (task_id,)).fetchone()
    from cortex_runtime.store import project_task_directory, private_directory
    task_dir = private_directory(project_task_directory(row["project_root"]) / task_id)
    (task_dir / f"{report_ref}.md").write_text("# done\n", encoding="utf-8")
    delegation_events.append_event(
        store, task_id, "delegation_requested", f"delreq-{assignment_ref}",
        dict(title="T", brief="B", role="backend-dev", acceptance="A"), origin="coordinator", assignment_ref=assignment_ref,
    )
    delegation_events.append_event(
        store, task_id, "worker_result", f"pub-{assignment_ref}",
        dict(report_ref=report_ref, summary="done", checks=[dict(name="tests", outcome="passed", tool_call_ref=None, evidence="observed")]),
        origin="worker", assignment_ref=assignment_ref,
    )
    delegation_events.claim_unconsumed(store, task_id, "cont-" + assignment_ref)
    return delegation_events.list_since(store, task_id)[-1]


def test_gate_accepts_when_workspace_unchanged_since_observed_capture(tmp_path):
    _init_repo(tmp_path)
    store, task_id = _task(tmp_path)
    assignment_ref = "req1"
    event = _record_result_with_observed_check(store, task_id, assignment_ref, "r_" + "a" * 12)
    delegation_revision.record_attempt_revision(store, assignment_ref, f"worker_result:{event.id}", tmp_path)
    ok, reasons = delegation_completion.gate(store, task_id)
    assert ok, reasons


def test_gate_refuses_stale_verification_when_workspace_changed(tmp_path):
    _init_repo(tmp_path)
    store, task_id = _task(tmp_path)
    assignment_ref = "req2"
    event = _record_result_with_observed_check(store, task_id, assignment_ref, "r_" + "b" * 12)
    delegation_revision.record_attempt_revision(store, assignment_ref, f"worker_result:{event.id}", tmp_path)
    # A file changes in the shared workspace root after the observed check
    # was captured -- the earlier verification no longer covers what is
    # actually on disk now.
    (tmp_path / "a.txt").write_text("mutated after verification\n")
    ok, reasons = delegation_completion.gate(store, task_id)
    assert not ok
    assert any(reason == f"stale_verification:{assignment_ref}" for reason in reasons)


def test_gate_refuses_stale_verification_on_head_change_alone(tmp_path):
    """Even with an unchanged tree digest, a differing HEAD (e.g. a commit
    landed on top of the exact same working tree) is treated as stale."""
    _init_repo(tmp_path)
    store, task_id = _task(tmp_path)
    assignment_ref = "req-head-only"
    event = _record_result_with_observed_check(store, task_id, assignment_ref, "r_" + "e" * 12)
    delegation_revision.record_attempt_revision(store, assignment_ref, f"worker_result:{event.id}", tmp_path)
    (tmp_path / "a.txt").write_text("one\n")  # restore original content exactly
    _git(["commit", "-q", "--allow-empty", "-m", "empty"], tmp_path)
    ok, reasons = delegation_completion.gate(store, task_id)
    assert not ok
    assert any(reason == f"stale_verification:{assignment_ref}" for reason in reasons)


def test_gate_labels_unavailable_revision_unknown_never_refuses(tmp_path, monkeypatch):
    store, task_id = _task(tmp_path)
    assignment_ref = "req3"
    event = _record_result_with_observed_check(store, task_id, assignment_ref, "r_" + "c" * 12)
    (tmp_path / "some-file.txt").write_text("content\n")
    monkeypatch.setattr(delegation_revision, "_MAX_FILES", 0)
    captured = delegation_revision.record_attempt_revision(store, assignment_ref, f"worker_result:{event.id}", tmp_path)
    assert captured["kind"] == "unavailable"
    ok, reasons = delegation_completion.gate(store, task_id)
    assert ok, reasons  # never refused for an inconclusive (unavailable) comparison


def test_label_formats_observed_and_unavailable():
    assert delegation_revision.label(None) == "revision: not captured"
    assert delegation_revision.label(
        dict(kind="observed", head="a" * 40, dirty_digest="b" * 64, detail=None)
    ) == f"revision: {'a' * 12} tree={'b' * 8}"
    assert delegation_revision.label(
        dict(kind="observed", head=None, dirty_digest="b" * 64, detail=None)
    ) == f"revision: no-head tree={'b' * 8}"
    assert delegation_revision.label(
        dict(kind="unavailable", head=None, dirty_digest=None, detail="x")
    ) == "revision: unknown (unavailable)"


# -- T035 FINDING-12 closure: the coordinator-turn backstop --------------


def _observed(digest="a" * 64, head=None):
    return dict(kind="observed", head=head, dirty_digest=digest, detail=None)


def _unavailable():
    return dict(kind="unavailable", head=None, dirty_digest=None, detail="x")


def test_workspace_changed_false_for_identical_digests():
    assert delegation_revision.workspace_changed(_observed(), _observed()) is False


def test_workspace_changed_true_for_different_digests():
    assert delegation_revision.workspace_changed(_observed("a" * 64), _observed("b" * 64)) is True


def test_workspace_changed_ignores_head_only_movement():
    """A HEAD move with an identical tree (e.g. an empty commit) is not
    itself a workspace mutation this backstop cares about."""
    before = _observed(digest="a" * 64, head="1" * 40)
    after = _observed(digest="a" * 64, head="2" * 40)
    assert delegation_revision.workspace_changed(before, after) is False


def test_workspace_changed_fails_closed_when_either_capture_is_unavailable():
    assert delegation_revision.workspace_changed(_unavailable(), _observed()) is True
    assert delegation_revision.workspace_changed(_observed(), _unavailable()) is True
    assert delegation_revision.workspace_changed(_unavailable(), _unavailable()) is True


def test_workspace_changed_real_capture_before_and_after_a_write(tmp_path):
    """Exercises the real, pure-Python capture_revision walk (not a stub),
    against a real temporary directory -- proves the wiring end to end,
    not just the comparison function in isolation."""
    (tmp_path / "a.txt").write_text("one\n")
    before = delegation_revision.capture_revision(tmp_path)
    assert delegation_revision.workspace_changed(before, delegation_revision.capture_revision(tmp_path)) is False
    (tmp_path / "a.txt").write_text("two\n")
    after = delegation_revision.capture_revision(tmp_path)
    assert delegation_revision.workspace_changed(before, after) is True
