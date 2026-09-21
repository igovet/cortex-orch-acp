"""Durable candidate verification request ledger.

This ledger records the exact check identity before an external command is
started.  Reclaiming a request left in ``running`` after a process crash is a
terminal ``unknown`` transition; callers must create a new request explicitly
after investigating the old one.
"""
from __future__ import annotations

from datetime import datetime, timezone
import json
import uuid

from ..contracts import StoreError


CHECK_REQUEST_SCHEMA = """
CREATE TABLE IF NOT EXISTS check_requests (
 check_id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 candidate_id TEXT NOT NULL,
 candidate_digest TEXT NOT NULL,
 attempt_id TEXT,
 request_key TEXT NOT NULL UNIQUE,
 argv_json TEXT NOT NULL,
 requirement_bindings_json TEXT NOT NULL DEFAULT '{}',
 checked_files_json TEXT NOT NULL DEFAULT '[]',
 status TEXT NOT NULL CHECK(status IN ('queued','running','completed','failed','unknown')),
 result_json TEXT,
 error TEXT,
 created_at TEXT NOT NULL,
 started_at TEXT,
 finished_at TEXT
);
CREATE INDEX IF NOT EXISTS check_requests_by_task ON check_requests(task_id, created_at);
CREATE INDEX IF NOT EXISTS check_requests_by_candidate ON check_requests(candidate_id, created_at);
"""


class CheckRequestError(StoreError):
    """Invalid or non-recoverable check request transition."""


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def _json(value) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def _ensure_schema(store) -> None:
    with store.connection() as db:
        db.executescript(CHECK_REQUEST_SCHEMA)
        columns = {row["name"] for row in db.execute("PRAGMA table_info(check_requests)")}
        if "requirement_bindings_json" not in columns:
            db.execute("ALTER TABLE check_requests ADD COLUMN requirement_bindings_json TEXT NOT NULL DEFAULT '{}'")
        if "checked_files_json" not in columns:
            db.execute("ALTER TABLE check_requests ADD COLUMN checked_files_json TEXT NOT NULL DEFAULT '[]'")


def _row(row):
    if row is None:
        return None
    value = dict(row)
    value["argv"] = json.loads(value.pop("argv_json"))
    value["requirement_bindings"] = json.loads(value.pop("requirement_bindings_json"))
    value["checked_files"] = json.loads(value.pop("checked_files_json"))
    if value.get("result_json") is not None:
        value["result"] = json.loads(value.pop("result_json"))
    else:
        value.pop("result_json", None)
        value["result"] = None
    return value


def get_check_request(store, check_id: str | None = None, *, request_key: str | None = None) -> dict | None:
    if (check_id is None) == (request_key is None):
        raise CheckRequestError("check_identity_required")
    _ensure_schema(store)
    with store.connection() as db:
        column, value = ("check_id", check_id) if check_id is not None else ("request_key", request_key)
        return _row(db.execute(f"SELECT * FROM check_requests WHERE {column}=?", (value,)).fetchone())


def find_reusable_passed_check(
    store,
    *,
    task_id: str,
    candidate_id: str,
    candidate_digest: str,
    argv,
    checked_files,
) -> dict | None:
    """Find an exact, already assessed pass for a candidate check.

    A coordinator may bind an already verified candidate check to a newly
    resolved requirement after the command has completed.  The command is
    still the same observation when candidate digest, argv, and file scope
    match, so running it again only burns time and can introduce a second
    inconsistent result.  Reuse is deliberately narrower than ordinary
    request idempotency: the prior request must be completed successfully,
    its evidence must remain applicable, and a durable ``assess_check(pass)``
    event must name that exact evidence.  Unknown, failed, unassessed, or
    malformed rows are never reusable.
    """
    if not isinstance(argv, (list, tuple)) or not isinstance(checked_files, (list, tuple, set)):
        return None
    wanted_argv = [str(value) for value in argv]
    wanted_files = sorted({str(value) for value in checked_files})
    _ensure_schema(store)
    from .check_evidence import get_check_evidence

    with store.connection() as db:
        rows = db.execute(
            "SELECT * FROM check_requests WHERE task_id=? AND candidate_id=? "
            "AND candidate_digest=? AND status='completed' ORDER BY finished_at DESC, created_at DESC",
            (task_id, candidate_id, candidate_digest),
        ).fetchall()
        for raw in rows:
            prior = _row(raw)
            if prior["argv"] != wanted_argv or prior["checked_files"] != wanted_files:
                continue
            observed = (prior.get("result") or {}).get("observed_result") or {}
            if observed.get("status") != "completed" or observed.get("exit_code") != 0:
                continue
            if observed.get("output_truncated"):
                continue
            evidence_id = (prior.get("result") or {}).get("evidence_id")
            if not isinstance(evidence_id, str):
                continue
            evidence = get_check_evidence(store, evidence_id)
            if evidence is None or not evidence.get("applicable"):
                continue
            assessments = db.execute(
                "SELECT payload_json FROM task_events WHERE task_id=? "
                "AND event_type='progress' AND dedup_key LIKE ? ORDER BY id DESC",
                (task_id, f"assess_check:{prior['check_id']}:%"),
            ).fetchall()
            passed = False
            for assessment_row in assessments:
                try:
                    payload = json.loads(assessment_row["payload_json"])
                except (TypeError, ValueError):
                    continue
                if (payload.get("kind") == "candidate_check_assessed"
                        and payload.get("assessment") == "pass"
                        and payload.get("evidence_ref") == evidence_id
                        and payload.get("candidate_digest") == candidate_digest):
                    passed = True
                    break
            if passed:
                return {"request": prior, "evidence": evidence}
    return None


def enqueue_check(
    store,
    *,
    task_id: str,
    candidate_id: str,
    candidate_digest: str,
    argv,
    attempt_id: str | None = None,
    request_key: str | None = None,
    check_id: str | None = None,
    requirement_bindings: dict | None = None,
    checked_files=(),
) -> dict:
    """Durably enqueue one exact check, or return its idempotent prior row."""
    _ensure_schema(store)
    if not candidate_id or not candidate_digest or not isinstance(argv, (list, tuple)) or not argv:
        raise CheckRequestError("check_identity_incomplete")
    argv = [str(value) for value in argv]
    if requirement_bindings is not None and not isinstance(requirement_bindings, dict):
        raise CheckRequestError("requirement_bindings_must_be_object")
    requirement_bindings = requirement_bindings or {}
    if not isinstance(checked_files, (list, tuple, set)):
        raise CheckRequestError("checked_files_must_be_array")
    checked_files = sorted({str(value) for value in checked_files})
    request_key = request_key or f"check-{uuid.uuid4().hex}"
    check_id = check_id or f"chk_{uuid.uuid4().hex}"
    created = _now()
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        task = db.execute("SELECT 1 FROM tasks WHERE id=?", (task_id,)).fetchone()
        if task is None:
            db.rollback()
            raise CheckRequestError("check_task_not_found")
        existing = db.execute("SELECT * FROM check_requests WHERE request_key=?", (request_key,)).fetchone()
        if existing is not None:
            prior = _row(existing)
            identity = (prior["task_id"], prior["candidate_id"], prior["candidate_digest"], prior["attempt_id"], prior["argv"], prior["requirement_bindings"], prior["checked_files"])
            expected = (task_id, candidate_id, candidate_digest, attempt_id, argv, requirement_bindings, checked_files)
            if identity != expected:
                db.rollback()
                raise CheckRequestError("check_request_reused")
            db.commit()
            return prior
        try:
            db.execute(
                "INSERT INTO check_requests(check_id,task_id,candidate_id,candidate_digest,attempt_id,request_key,argv_json,requirement_bindings_json,checked_files_json,status,created_at) "
                "VALUES(?,?,?,?,?,?,?,?,?,?,?)",
                (check_id, task_id, candidate_id, candidate_digest, attempt_id, request_key, _json(argv), _json(requirement_bindings), _json(checked_files), "queued", created),
            )
        except Exception:
            db.rollback()
            raise
        db.commit()
    return get_check_request(store, check_id)


def claim_check(store, check_id: str) -> dict:
    """Claim queued work; reclaiming running work makes it permanently unknown."""
    _ensure_schema(store)
    now = _now()
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        row = db.execute("SELECT * FROM check_requests WHERE check_id=?", (check_id,)).fetchone()
        if row is None:
            db.rollback()
            raise CheckRequestError("check_not_found")
        status = row["status"]
        if status == "queued":
            db.execute("UPDATE check_requests SET status='running',started_at=? WHERE check_id=?", (now, check_id))
        elif status == "running":
            db.execute("UPDATE check_requests SET status='unknown',error=?,finished_at=? WHERE check_id=?",
                       ("process_crash_or_lost_owner", now, check_id))
        db.commit()
    return get_check_request(store, check_id)


def complete_check(store, check_id: str, result, *, failed: bool = False, error: str | None = None) -> dict:
    """Complete only a currently-owned running request."""
    _ensure_schema(store)
    status = "failed" if failed else "completed"
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        row = db.execute("SELECT status FROM check_requests WHERE check_id=?", (check_id,)).fetchone()
        if row is None:
            db.rollback()
            raise CheckRequestError("check_not_found")
        if row["status"] != "running":
            db.rollback()
            raise CheckRequestError(f"check_not_running:{row['status']}")
        db.execute("UPDATE check_requests SET status=?,result_json=?,error=?,finished_at=? WHERE check_id=?",
                   (status, _json(result), error, _now(), check_id))
        db.commit()
    return get_check_request(store, check_id)


def fail_check(store, check_id: str, error: str, result=None) -> dict:
    return complete_check(store, check_id, result if result is not None else {}, failed=True, error=error)


def mark_unknown(store, check_id: str, reason: str = "process_crash_or_lost_owner") -> dict:
    _ensure_schema(store)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        row = db.execute("SELECT status FROM check_requests WHERE check_id=?", (check_id,)).fetchone()
        if row is None:
            db.rollback()
            raise CheckRequestError("check_not_found")
        if row["status"] not in {"queued", "running"}:
            db.rollback()
            raise CheckRequestError(f"check_not_recoverable:{row['status']}")
        db.execute("UPDATE check_requests SET status='unknown',error=?,finished_at=? WHERE check_id=?",
                   (reason, _now(), check_id))
        db.commit()
    return get_check_request(store, check_id)


__all__ = ["CHECK_REQUEST_SCHEMA", "CheckRequestError", "claim_check", "complete_check", "enqueue_check", "fail_check", "find_reusable_passed_check", "get_check_request", "mark_unknown"]
