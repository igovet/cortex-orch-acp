import pytest

from cortex_runtime.delegation.check_evidence import (
    CheckEvidenceError,
    applicable_check_evidence,
    get_check_evidence,
    invalidate_check_evidence,
    invalidate_requirement_revision,
    list_check_evidence,
    record_check_evidence,
)
from cortex_runtime.delegation.candidates import CANDIDATE_SCHEMA
from cortex_runtime.cleanup import finish_deletions
from cortex_runtime.store import Store


def _store(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=str(project))
    with store.connection() as db:
        db.execute("INSERT INTO tasks(id,project_root,created_at) VALUES(?,?,?)", ("task", str(project), "now"))
        db.executescript(CANDIDATE_SCHEMA)
        db.execute("INSERT INTO candidate_staging(candidate_id,task_id,project_root,stage_path,attempt_ids_json,source_bindings_json,base_identity_json,project_base_manifest_json,project_base_digest,content_digest,status,created_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                    ("cand", "task", str(project), str(project), "[]", "[]", "[]", "{}", "base", "digest", "ready", "now"))
    return store


def _record(store, **kwargs):
    values = dict(
        task_id="task", attempt_id="attempt-1", candidate_id="cand",
        candidate_content_digest="digest", requirement_bindings={"req": {"revision": 1}},
        environment={"python": "3.13", "platform": "linux"},
        command={"argv": ["pytest", "tests/test_api.py"]},
        observed_result={"status": "completed", "exit_code": 0},
        assessment={"status": "pass", "reason": "acceptance criterion satisfied"},
        checked_files=["src/api.py"], idempotency_key="write-1",
    )
    values.update(kwargs)
    return record_check_evidence(store, **values)


def test_record_keeps_observation_and_semantic_assessment_separate(tmp_path):
    store = _store(tmp_path)
    item = _record(store)
    assert item["observed_result"]["exit_code"] == 0
    assert item["assessment"]["status"] == "pass"
    assert item["applicable"] == 1
    assert applicable_check_evidence(store, "task", "cand", "digest",
                                     {"req": {"revision": 1}},
                                     {"platform": "linux", "python": "3.13"})[0]["evidence_id"] == item["evidence_id"]


def test_completed_command_without_semantic_assessment_is_rejected(tmp_path):
    store = _store(tmp_path)
    with pytest.raises(CheckEvidenceError, match="semantic_assessment_required"):
        _record(store, assessment={})


def test_idempotent_replay_and_changed_file_invalidation(tmp_path):
    store = _store(tmp_path)
    first = _record(store)
    replay = _record(store)
    assert replay["evidence_id"] == first["evidence_id"]
    invalidated = invalidate_check_evidence(store, "task", "cand",
                                            candidate_content_digest="new-digest",
                                            changed_files=["src/api.py"])
    assert invalidated == [first["evidence_id"]]
    assert get_check_evidence(store, first["evidence_id"])["invalidated_reason"] == "candidate_changed"
    assert list_check_evidence(store, "task", applicable_only=True) == []


def test_unrelated_candidate_change_does_not_invalidate_scoped_check(tmp_path):
    store = _store(tmp_path)
    first = _record(store)
    assert invalidate_check_evidence(store, "task", "cand",
                                     candidate_content_digest="new-digest",
                                     changed_files=["docs/readme.md"]) == []
    assert get_check_evidence(store, first["evidence_id"])["applicable"] == 1
    assert invalidate_check_evidence(store, "task", "cand",
                                     requirement_bindings={"req": {"revision": 2}}) == [first["evidence_id"]]


def test_candidate_digest_is_verified_before_recording(tmp_path):
    store = _store(tmp_path)
    with pytest.raises(CheckEvidenceError, match="candidate_digest_mismatch"):
        _record(store, candidate_content_digest="wrong")


def test_requirement_revision_invalidation_is_scoped_and_conservative(tmp_path):
    store = _store(tmp_path)
    bound = _record(store, idempotency_key="bound")
    unbound = _record(store, idempotency_key="unbound", requirement_bindings={"other": {"revision": 1}})
    assert invalidate_requirement_revision(store, "task", "req", 2) == [bound["evidence_id"]]
    assert get_check_evidence(store, bound["evidence_id"])["applicable"] == 0
    assert get_check_evidence(store, unbound["evidence_id"])["applicable"] == 1


def test_evidence_catalogue_is_bounded_and_retention_removes_all_rows(tmp_path):
    store = _store(tmp_path)
    first = _record(store)
    second = _record(store, idempotency_key="write-2")
    with pytest.raises(CheckEvidenceError, match="invalid_evidence_limit"):
        list_check_evidence(store, "task", limit=501)
    page = list_check_evidence(store, "task", limit=1)
    assert len(page) == 1
    assert list_check_evidence(store, "task", limit=10, cursor=page[0]["evidence_id"])[0]["evidence_id"] == second["evidence_id"]

    stage = tmp_path / "project" / ".cortex" / "candidates" / "cand" / "stage"
    stage.mkdir(parents=True)
    old = "2000-01-01T00:00:00+00:00"
    with store.connection() as db:
        db.execute("UPDATE tasks SET created_at=? WHERE id=?", (old, "task"))
        db.execute("INSERT INTO pending_deletions VALUES (?)", ("task",))
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        finish_deletions(store, db, "task")
        db.commit()
        assert db.execute("SELECT 1 FROM check_evidence WHERE task_id=?", ("task",)).fetchone() is None
        assert db.execute("SELECT 1 FROM check_evidence_operations WHERE task_id=?", ("task",)).fetchone() is None
