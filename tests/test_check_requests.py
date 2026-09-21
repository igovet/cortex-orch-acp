import pytest
import json

from cortex_runtime.cleanup import finish_deletions
from cortex_runtime.delegation import events
from cortex_runtime.delegation.candidates import CANDIDATE_SCHEMA
from cortex_runtime.delegation.check_evidence import CHECK_EVIDENCE_SCHEMA
from cortex_runtime.delegation.check_requests import (
    CheckRequestError,
    claim_check,
    complete_check,
    enqueue_check,
    get_check_request,
    mark_unknown,
    find_reusable_passed_check,
)
from cortex_runtime.store import Store


def _store(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=str(project))
    task = "task-check"
    with store.connection() as db:
        db.execute("INSERT INTO tasks(id,project_root,created_at) VALUES(?,?,?)", (task, str(project), "now"))
    return store, task


def test_check_request_is_idempotent_and_records_exact_identity(tmp_path):
    store, task = _store(tmp_path)
    first = enqueue_check(store, task_id=task, candidate_id="cand-1", candidate_digest="digest-1",
                          argv=["pytest", "-q"], attempt_id="attempt-1", request_key="request-1",
                          requirement_bindings={"req": {"revision": 1}}, checked_files=["src/api.py"])
    second = enqueue_check(store, task_id=task, candidate_id="cand-1", candidate_digest="digest-1",
                           argv=["pytest", "-q"], attempt_id="attempt-1", request_key="request-1",
                           requirement_bindings={"req": {"revision": 1}}, checked_files=["src/api.py"])
    assert first["check_id"] == second["check_id"]
    assert first["status"] == "queued"
    assert first["argv"] == ["pytest", "-q"]
    assert first["requirement_bindings"] == {"req": {"revision": 1}}
    assert first["checked_files"] == ["src/api.py"]
    with pytest.raises(CheckRequestError, match="check_request_reused"):
        enqueue_check(store, task_id=task, candidate_id="cand-2", candidate_digest="digest-1",
                      argv=["pytest"], request_key="request-1")


def test_reclaiming_running_check_becomes_unknown_and_cannot_rerun(tmp_path):
    store, task = _store(tmp_path)
    request = enqueue_check(store, task_id=task, candidate_id="cand-1", candidate_digest="digest-1",
                            argv=["python3", "-m", "pytest"], request_key="request-1")
    assert claim_check(store, request["check_id"])["status"] == "running"
    recovered = claim_check(store, request["check_id"])
    assert recovered["status"] == "unknown"
    assert recovered["error"] == "process_crash_or_lost_owner"
    with pytest.raises(CheckRequestError, match="check_not_running:unknown"):
        complete_check(store, request["check_id"], {"passed": True})
    with pytest.raises(CheckRequestError, match="check_not_recoverable:unknown"):
        mark_unknown(store, request["check_id"])


def test_completed_and_failed_are_terminal_and_retained_until_task_retention(tmp_path):
    store, task = _store(tmp_path)
    completed = enqueue_check(store, task_id=task, candidate_id="cand-1", candidate_digest="digest-1",
                              argv=["check"], request_key="request-completed")
    failed = enqueue_check(store, task_id=task, candidate_id="cand-1", candidate_digest="digest-1",
                           argv=["check", "--other"], request_key="request-failed")
    claim_check(store, completed["check_id"])
    claim_check(store, failed["check_id"])
    assert complete_check(store, completed["check_id"], {"passed": True})["status"] == "completed"
    assert complete_check(store, failed["check_id"], {}, failed=True, error="exit_1")["status"] == "failed"
    with pytest.raises(CheckRequestError, match="check_not_running:completed"):
        complete_check(store, completed["check_id"], {})
    with store.connection() as db:
        db.execute("INSERT INTO pending_deletions(task_id) VALUES (?)", (task,))
        db.execute("BEGIN IMMEDIATE")
        finish_deletions(store, db, task)
        db.commit()
    assert get_check_request(store, completed["check_id"]) is None
    assert get_check_request(store, failed["check_id"]) is None


def test_only_exact_previously_assessed_pass_is_reusable(tmp_path):
    store, task = _store(tmp_path)
    project = tmp_path / "project"
    with store.connection() as db:
        db.executescript(CANDIDATE_SCHEMA)
        db.execute(
            "INSERT INTO candidate_staging(candidate_id,task_id,project_root,stage_path,"
            "attempt_ids_json,source_bindings_json,base_identity_json,project_base_manifest_json,"
            "project_base_digest,content_digest,status,created_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            ("cand-1", task, str(project), str(project), "[]", "[]", "[]", "{}",
             "base", "digest-1", "ready", "now"),
        )
        db.executescript(CHECK_EVIDENCE_SCHEMA)
    first = enqueue_check(
        store, task_id=task, candidate_id="cand-1", candidate_digest="digest-1",
        argv=["python3", "tests/check.py"], request_key="request-1",
        checked_files=["index.html"],
    )
    claim_check(store, first["check_id"])
    evidence_id = "evidence-1"
    with store.connection() as db:
        db.execute(
            "INSERT INTO check_evidence VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (evidence_id, task, first["check_id"], "cand-1", "digest-1", "{}",
             json.dumps({"execution": "direct-user-v2"}, sort_keys=True), "env-digest",
             json.dumps(["index.html"]), json.dumps({"argv": ["python3", "tests/check.py"]}),
             json.dumps({"status": "completed", "exit_code": 0, "output_truncated": False}),
             json.dumps({"status": "unassessed"}), 1, None, "now", None),
        )
    complete_check(store, first["check_id"], {
        "evidence_id": evidence_id,
        "observed_result": {"status": "completed", "exit_code": 0, "output_truncated": False},
    })
    events.append_event(
        store, task, "progress", f"assess_check:{first['check_id']}:pass",
        {"kind": "candidate_check_assessed", "check_ref": first["check_id"],
         "candidate_digest": "digest-1", "evidence_ref": evidence_id, "assessment": "pass"},
        origin="coordinator",
    )
    reusable = find_reusable_passed_check(
        store, task_id=task, candidate_id="cand-1", candidate_digest="digest-1",
        argv=["python3", "tests/check.py"], checked_files=["index.html"],
    )
    assert reusable is not None
    assert reusable["request"]["check_id"] == first["check_id"]
    assert reusable["evidence"]["evidence_id"] == evidence_id
    assert find_reusable_passed_check(
        store, task_id=task, candidate_id="cand-1", candidate_digest="digest-1",
        argv=["python3", "tests/other.py"], checked_files=["index.html"],
    ) is None
