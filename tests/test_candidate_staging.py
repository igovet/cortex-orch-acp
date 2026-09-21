import json
import os
import shutil

import pytest

from cortex_runtime.delegation.candidates import CandidateConflict, CandidateError, _digest_manifest, _manifest, get_candidate, prepare_candidate
from cortex_runtime.delegation.workspace_isolation import capture_and_copy_workspace
from cortex_runtime.cleanup import finish_deletions
from cortex_runtime.store import Store


def _fixture(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    (project / "base.txt").write_text("base\n")
    store = Store(project / ".cortex", project_root=str(project))
    task = "task-candidate"
    with store.connection() as db:
        db.execute("INSERT INTO tasks(id,project_root,created_at) VALUES(?,?,?)", (task, str(project), "now"))
    return project, store, task


def _attempt(tmp_path, store, task, project, attempt, edit):
    source = tmp_path / attempt
    source.mkdir()
    base = source / "base"
    root = source / "root"
    manifest = capture_and_copy_workspace(str(project), str(base))
    shutil.copytree(base, root)
    edit(root)
    with store.connection() as db:
        db.execute(
            "INSERT INTO workspace_snapshots VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (f"wb-{attempt}", task, attempt, str(project), str(root), str(base), "isolated_snapshot",
             json.dumps(manifest), "dirty", None, "base-digest", "now", None, None),
        )


def test_candidate_stages_two_independent_attempts_without_touching_project(tmp_path):
    project, store, task = _fixture(tmp_path)
    _attempt(tmp_path, store, task, project, "a1", lambda root: (root / "one.txt").write_text("one\n"))
    _attempt(tmp_path, store, task, project, "a2", lambda root: (root / "two.txt").write_text("two\n"))

    candidate = prepare_candidate(store, task, ["a1", "a2"])
    stage = project / ".cortex" / "candidates" / candidate["candidate_id"] / "stage"
    assert candidate["status"] == "ready"
    assert (stage / "one.txt").read_text() == "one\n"
    assert (stage / "two.txt").read_text() == "two\n"
    assert not (project / "one.txt").exists()
    assert not (project / "two.txt").exists()
    assert candidate["content_digest"]
    assert candidate["project_base_digest"] == _digest_manifest(_manifest(str(project)))


def test_candidate_records_conflicting_overlap_and_preserves_stage(tmp_path):
    project, store, task = _fixture(tmp_path)
    _attempt(tmp_path, store, task, project, "a1", lambda root: (root / "shared.txt").write_text("one\n"))
    _attempt(tmp_path, store, task, project, "a2", lambda root: (root / "shared.txt").write_text("two\n"))

    with pytest.raises(CandidateConflict) as caught:
        prepare_candidate(store, task, ["a1", "a2"])
    candidate = caught.value.candidate
    assert candidate["status"] == "conflict"
    assert candidate["conflict_paths"] == ["shared.txt"]
    stored = get_candidate(store, candidate["candidate_id"])
    assert stored["status"] == "conflict"
    assert (project / ".cortex" / "candidates" / candidate["candidate_id"] / "stage").is_dir()
    assert not (project / "shared.txt").exists()


def test_candidate_rejects_shared_root_and_keeps_failure_record(tmp_path):
    project, store, task = _fixture(tmp_path)
    with store.connection() as db:
        db.execute(
            "INSERT INTO workspace_snapshots VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            ("wb-shared", task, "shared", str(project), None, None, "serialized_shared_root", "{}",
             "unavailable", None, None, "now", None, None),
        )
    with pytest.raises(Exception):
        prepare_candidate(store, task, ["shared"])
    with store.connection() as db:
        row = db.execute("SELECT status,error FROM candidate_staging").fetchone()
    assert row["status"] == "failed"
    assert row["error"]


def test_candidate_snapshots_direct_project_root_result(tmp_path):
    """A Zed-backed worker result is already in the user's project root."""
    project, store, task = _fixture(tmp_path)
    (project / "base.txt").write_text("worker result\n")
    manifest = _manifest(str(project))
    with store.connection() as db:
        db.execute(
            "INSERT INTO workspace_snapshots VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            ("wb-direct", task, "direct", str(project), str(project), None,
             "shared_project_root", json.dumps(manifest), "dirty", None,
             "direct-base", "now", None, None),
        )
    candidate = prepare_candidate(store, task, ["direct"], candidate_id="candidate-direct")
    stage = project / ".cortex" / "candidates" / "candidate-direct" / "stage"
    assert candidate["status"] == "ready"
    assert (stage / "base.txt").read_text() == "worker result\n"
    assert candidate["project_base_digest"] == _digest_manifest(manifest)
    assert (project / "base.txt").read_text() == "worker result\n"


def test_candidate_detects_user_change_since_attempt_base(tmp_path):
    project, store, task = _fixture(tmp_path)
    _attempt(tmp_path, store, task, project, "a1", lambda root: (root / "base.txt").write_text("worker\n"))
    (project / "base.txt").write_text("user\n")

    with pytest.raises(CandidateConflict) as caught:
        prepare_candidate(store, task, ["a1"])
    assert caught.value.candidate["conflict_paths"] == ["base.txt"]
    assert (project / "base.txt").read_text() == "user\n"


def test_candidate_replay_is_idempotent_and_reuse_is_rejected(tmp_path):
    project, store, task = _fixture(tmp_path)
    _attempt(tmp_path, store, task, project, "a1", lambda root: (root / "one.txt").write_text("one\n"))
    first = prepare_candidate(store, task, ["a1"], candidate_id="candidate-fixed")
    second = prepare_candidate(store, task, ["a1"], candidate_id="candidate-fixed")
    assert first["content_digest"] == second["content_digest"]
    with pytest.raises(CandidateError, match="candidate_id_reused"):
        prepare_candidate(store, task, ["other"], candidate_id="candidate-fixed")


def test_candidate_stages_dependent_attempt_from_workspace_safety_lineage(tmp_path):
    project, store, task = _fixture(tmp_path)
    _attempt(tmp_path, store, task, project, "parent", lambda root: (root / "parent.txt").write_text("parent\n"))
    parent = prepare_candidate(store, task, ["parent"], candidate_id="candidate-parent")
    parent_stage = project / ".cortex" / "candidates" / "candidate-parent" / "stage"

    source = tmp_path / "dependent"
    source.mkdir()
    base = source / "base"
    root = source / "root"
    base_manifest = _manifest(str(parent_stage))
    shutil.copytree(parent_stage, base)
    shutil.copytree(base, root)
    (root / "child.txt").write_text("child\n")
    with store.connection() as db:
        db.execute(
            "INSERT INTO workspace_snapshots VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            ("wb-dependent", task, "dependent", str(project), str(root), str(base), "isolated_snapshot",
             json.dumps(base_manifest), "dirty", None, "dependent-base", "now", None, None),
        )
        db.execute("INSERT INTO workspace_snapshot_lineage(workspace_binding_id,candidate_id,created_at) VALUES (?,?,?)",
                   ("wb-dependent", "candidate-parent", "now"))

    dependent = prepare_candidate(store, task, ["dependent"], candidate_id="candidate-dependent")
    stage = project / ".cortex" / "candidates" / "candidate-dependent" / "stage"
    assert dependent["status"] == "ready"
    assert dependent["lineage_candidate_id"] == "candidate-parent"
    assert dependent["project_base_digest"] == parent["project_base_digest"]
    assert (stage / "parent.txt").read_text() == "parent\n"
    assert (stage / "child.txt").read_text() == "child\n"
    assert not (project / "parent.txt").exists()


def test_candidate_conflict_is_retained_until_explicit_task_retention(tmp_path):
    project, store, task = _fixture(tmp_path)
    _attempt(tmp_path, store, task, project, "a1", lambda root: (root / "one.txt").write_text("one\n"))
    candidate = prepare_candidate(store, task, ["a1"], candidate_id="candidate-retention")
    stage_parent = project / ".cortex" / "candidates" / "candidate-retention"
    with store.connection() as db:
        db.execute("UPDATE candidate_staging SET status='conflict',error='held for review' WHERE candidate_id=?",
                   (candidate["candidate_id"],))
        db.execute("INSERT INTO pending_deletions(task_id) VALUES (?)", (task,))
    assert stage_parent.is_dir()
    assert get_candidate(store, candidate["candidate_id"])["status"] == "conflict"
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        finish_deletions(store, db, task)
        db.commit()
    assert not stage_parent.exists()
    assert get_candidate(store, candidate["candidate_id"]) is None


def test_candidate_directory_mode_change_preserves_unchanged_children(tmp_path):
    project, store, task = _fixture(tmp_path)
    (project / "nested").mkdir()
    (project / "nested" / "keep.txt").write_text("keep\n")
    _attempt(tmp_path, store, task, project, "mode", lambda root: os.chmod(root / "nested", 0o755))

    candidate = prepare_candidate(store, task, ["mode"], candidate_id="candidate-mode")
    stage = project / ".cortex" / "candidates" / candidate["candidate_id"] / "stage" / "nested"
    assert (stage / "keep.txt").read_text() == "keep\n"
    assert (stage.stat().st_mode & 0o777) == 0o755


def test_candidate_nested_deletion_applies_children_before_parent(tmp_path):
    project, store, task = _fixture(tmp_path)
    (project / "nested" / "deep").mkdir(parents=True)
    (project / "nested" / "deep" / "remove.txt").write_text("remove\n")
    _attempt(tmp_path, store, task, project, "delete", lambda root: shutil.rmtree(root / "nested"))

    candidate = prepare_candidate(store, task, ["delete"], candidate_id="candidate-delete")
    stage = project / ".cortex" / "candidates" / candidate["candidate_id"] / "stage"
    assert not (stage / "nested").exists()
    assert (project / "nested" / "deep" / "remove.txt").exists()
