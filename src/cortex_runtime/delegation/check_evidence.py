"""Durable, candidate-bound verification evidence.

This module intentionally does not decide whether a command result is good.
The runtime records what was observed, while the coordinator records a
separate semantic assessment.  A check is reusable only while its candidate,
requirement revisions, and environment identity still match.
"""
from __future__ import annotations

from datetime import datetime, timezone
import hashlib
import json
import uuid

from ..contracts import StoreError
from .candidates import get_candidate


class CheckEvidenceError(StoreError):
    """The evidence record is incomplete, ambiguous, or cannot be bound."""


CHECK_EVIDENCE_SCHEMA = """
CREATE TABLE IF NOT EXISTS check_evidence (
 evidence_id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 attempt_id TEXT NOT NULL,
 candidate_id TEXT NOT NULL,
 candidate_content_digest TEXT NOT NULL,
 requirement_bindings_json TEXT NOT NULL,
 environment_json TEXT NOT NULL,
 environment_digest TEXT NOT NULL,
 checked_files_json TEXT NOT NULL,
 command_json TEXT NOT NULL,
 observed_result_json TEXT NOT NULL,
 assessment_json TEXT NOT NULL,
 applicable INTEGER NOT NULL DEFAULT 1 CHECK(applicable IN (0,1)),
 invalidated_reason TEXT,
 created_at TEXT NOT NULL,
 invalidated_at TEXT
);
CREATE INDEX IF NOT EXISTS check_evidence_by_candidate
 ON check_evidence(task_id, candidate_id, candidate_content_digest);
CREATE INDEX IF NOT EXISTS check_evidence_by_attempt
 ON check_evidence(task_id, attempt_id);
CREATE TABLE IF NOT EXISTS check_evidence_operations (
 task_id TEXT NOT NULL REFERENCES tasks(id),
 idempotency_key TEXT NOT NULL,
 request_json TEXT NOT NULL,
 evidence_id TEXT NOT NULL,
 created_at TEXT NOT NULL,
 PRIMARY KEY(task_id, idempotency_key)
);
"""


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def _canonical(value) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def _digest(value) -> str:
    return hashlib.sha256(_canonical(value).encode("utf-8")).hexdigest()


def _ensure_schema(store) -> None:
    with store.connection() as db:
        db.executescript(CHECK_EVIDENCE_SCHEMA)


def _files(value) -> list[str]:
    if value is None:
        return []
    if not isinstance(value, (list, tuple, set)):
        raise CheckEvidenceError("checked_files_must_be_array")
    result = sorted({str(item) for item in value})
    if any(not item or item.startswith("/") or ".." in item.split("/") for item in result):
        raise CheckEvidenceError("unsafe_checked_file")
    return result


def _row(row) -> dict | None:
    if row is None:
        return None
    result = dict(row)
    for source, target in (
        ("requirement_bindings_json", "requirement_bindings"),
        ("environment_json", "environment"),
        ("checked_files_json", "checked_files"),
        ("command_json", "command"),
        ("observed_result_json", "observed_result"),
        ("assessment_json", "assessment"),
    ):
        result[target] = json.loads(result.pop(source))
    return result


def _candidate_binding(store, task_id: str, candidate_id: str, digest: str) -> dict:
    candidate = get_candidate(store, candidate_id)
    if candidate is None or candidate.get("task_id") != task_id:
        raise CheckEvidenceError("candidate_task_mismatch")
    if candidate.get("status") != "ready" or not candidate.get("content_digest"):
        raise CheckEvidenceError("candidate_not_ready")
    if candidate["content_digest"] != digest:
        raise CheckEvidenceError("candidate_digest_mismatch")
    return candidate


def record_check_evidence(
    store,
    task_id: str,
    attempt_id: str,
    candidate_id: str,
    candidate_content_digest: str,
    requirement_bindings,
    environment,
    command,
    observed_result,
    assessment,
    *,
    checked_files=None,
    evidence_id: str | None = None,
    idempotency_key: str | None = None,
) -> dict:
    """Record one exact check, or replay the same idempotent write.

    ``observed_result`` and ``assessment`` are deliberately separate.  In
    particular, a completed tool call never becomes a passing assessment here.
    The assessment must explicitly contain a semantic ``status``.
    """
    if not isinstance(attempt_id, str) or not attempt_id:
        raise CheckEvidenceError("attempt_required")
    if not isinstance(requirement_bindings, dict):
        raise CheckEvidenceError("requirement_bindings_must_be_object")
    if not isinstance(environment, dict):
        raise CheckEvidenceError("environment_must_be_object")
    if not isinstance(command, dict):
        raise CheckEvidenceError("command_must_be_object")
    if not isinstance(observed_result, dict) or not observed_result.get("status"):
        raise CheckEvidenceError("observed_result_status_required")
    if not isinstance(assessment, dict) or assessment.get("status") not in {
        "pass", "fail", "inconclusive", "unassessed"
    }:
        raise CheckEvidenceError("semantic_assessment_required")
    if not isinstance(candidate_content_digest, str) or not candidate_content_digest:
        raise CheckEvidenceError("candidate_digest_required")
    checked_files = _files(checked_files)
    environment_digest = _digest(environment)
    request = {
        "attempt_id": attempt_id, "candidate_id": candidate_id,
        "candidate_content_digest": candidate_content_digest,
        "requirement_bindings": requirement_bindings, "environment": environment,
        "checked_files": checked_files, "command": command,
        "observed_result": observed_result, "assessment": assessment,
        "evidence_id": evidence_id,
    }
    key = idempotency_key or uuid.uuid4().hex
    eid = evidence_id or f"check_{uuid.uuid4().hex}"
    _candidate_binding(store, task_id, candidate_id, candidate_content_digest)
    _ensure_schema(store)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            prior = db.execute(
                "SELECT evidence_id,request_json FROM check_evidence_operations WHERE task_id=? AND idempotency_key=?",
                (task_id, key),
            ).fetchone()
            if prior is not None:
                if prior[1] != _canonical(request):
                    raise CheckEvidenceError("idempotency_request_changed")
                row = db.execute("SELECT * FROM check_evidence WHERE evidence_id=?", (prior[0],)).fetchone()
                db.commit()
                return _row(row)
            now = _now()
            db.execute(
                "INSERT INTO check_evidence VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                (eid, task_id, attempt_id, candidate_id, candidate_content_digest,
                 _canonical(requirement_bindings), _canonical(environment), environment_digest,
                 _canonical(checked_files), _canonical(command), _canonical(observed_result),
                 _canonical(assessment), 1, None, now, None),
            )
            db.execute(
                "INSERT INTO check_evidence_operations VALUES(?,?,?,?,?)",
                (task_id, key, _canonical(request), eid, now),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return get_check_evidence(store, eid)


def get_check_evidence(store, evidence_id: str) -> dict | None:
    _ensure_schema(store)
    with store.connection() as db:
        return _row(db.execute("SELECT * FROM check_evidence WHERE evidence_id=?", (evidence_id,)).fetchone())


def list_check_evidence(store, task_id: str, *, candidate_id: str | None = None,
                        applicable_only: bool = False, limit: int = 100,
                        cursor: str | None = None) -> list[dict]:
    """Read a bounded catalogue of evidence.

    ``cursor`` is an evidence ID from a previous page. Ordering is stable by
    creation time and ID; callers never receive an unbounded context payload.
    """
    if type(limit) is not int or not 1 <= limit <= 500:
        raise CheckEvidenceError("invalid_evidence_limit")
    _ensure_schema(store)
    clauses = ["task_id=?"]
    params: list[object] = [task_id]
    if candidate_id is not None:
        clauses.append("candidate_id=?"); params.append(candidate_id)
    if applicable_only:
        clauses.append("applicable=1")
    if cursor is not None:
        with store.connection() as db:
            marker = db.execute("SELECT created_at,evidence_id FROM check_evidence WHERE evidence_id=? AND task_id=?",
                                (cursor, task_id)).fetchone()
        if marker is None:
            raise CheckEvidenceError("unknown_evidence_cursor")
        clauses.append("(created_at > ? OR (created_at = ? AND evidence_id > ?))")
        params.extend((marker[0], marker[0], marker[1]))
    with store.connection() as db:
        rows = db.execute("SELECT * FROM check_evidence WHERE " + " AND ".join(clauses) +
                          " ORDER BY created_at,evidence_id LIMIT ?", (*params, limit)).fetchall()
    return [_row(row) for row in rows]


def invalidate_check_evidence(
    store, task_id: str, candidate_id: str, *,
    candidate_content_digest: str | None = None,
    changed_files=None,
    requirement_bindings: dict | None = None,
    environment: dict | None = None,
    reason: str = "evidence_context_changed",
) -> list[str]:
    """Mark stale checks in place while retaining their original evidence.

    A changed candidate digest only invalidates checks that covered a changed
    file; a check with no file scope is conservatively treated as whole-
    candidate. Requirement or environment changes invalidate matching checks.
    """
    changed = set(_files(changed_files)) if changed_files is not None else None
    new_req = _canonical(requirement_bindings) if requirement_bindings is not None else None
    new_env_digest = _digest(environment) if environment is not None else None
    _ensure_schema(store)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            rows = db.execute(
                "SELECT * FROM check_evidence WHERE task_id=? AND candidate_id=? AND applicable=1",
                (task_id, candidate_id),
            ).fetchall()
            ids = []
            for row in rows:
                stale = False
                stale_reason = reason
                if candidate_content_digest is not None and row["candidate_content_digest"] != candidate_content_digest:
                    covered = set(json.loads(row["checked_files_json"]))
                    stale = changed is None or not covered or bool(changed & covered)
                    if stale: stale_reason = "candidate_changed"
                if new_req is not None and row["requirement_bindings_json"] != new_req:
                    stale = True; stale_reason = "requirements_changed"
                if new_env_digest is not None and row["environment_digest"] != new_env_digest:
                    stale = True; stale_reason = "environment_changed"
                if stale:
                    ids.append(row["evidence_id"])
                    db.execute("UPDATE check_evidence SET applicable=0,invalidated_reason=?,invalidated_at=? WHERE evidence_id=?",
                               (stale_reason, _now(), row["evidence_id"]))
            db.commit()
            return ids
        except BaseException:
            db.rollback(); raise


def applicable_check_evidence(store, task_id: str, candidate_id: str,
                              candidate_content_digest: str,
                              requirement_bindings: dict, environment: dict) -> list[dict]:
    """Return evidence whose complete binding still matches the requested context."""
    env_digest = _digest(environment)
    req = _canonical(requirement_bindings)
    _ensure_schema(store)
    with store.connection() as db:
        rows = db.execute(
            "SELECT * FROM check_evidence WHERE task_id=? AND candidate_id=? AND candidate_content_digest=? "
            "AND requirement_bindings_json=? AND environment_digest=? AND applicable=1 ORDER BY created_at,evidence_id",
            (task_id, candidate_id, candidate_content_digest, req, env_digest),
        ).fetchall()
    return [_row(row) for row in rows]


def invalidate_requirement_revision(store, task_id: str, requirement_id: str,
                                    new_revision: int) -> list[str]:
    """Invalidate evidence made stale by one requirement revision.

    Evidence explicitly bound to an older revision is invalidated. Evidence
    with no binding for this requirement is also invalidated conservatively:
    its scope is unknown, so it cannot safely gate completion after the
    requirement changes.
    """
    if not isinstance(requirement_id, str) or not requirement_id or type(new_revision) is not int or new_revision < 1:
        raise CheckEvidenceError("invalid_requirement_revision")
    _ensure_schema(store)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            rows = db.execute("SELECT evidence_id,requirement_bindings_json FROM check_evidence WHERE task_id=? AND applicable=1", (task_id,)).fetchall()
            ids = []
            for row in rows:
                bindings = json.loads(row["requirement_bindings_json"])
                binding = bindings.get(requirement_id) if isinstance(bindings, dict) else None
                revision = binding.get("revision") if isinstance(binding, dict) else binding
                # A nonempty binding to other requirements is scoped and is
                # unaffected by this revision. Only an unscoped check, or an
                # explicit older binding for this requirement, is stale.
                if (not isinstance(bindings, dict) or not bindings) or (
                        requirement_id in bindings and
                        (revision is None or (isinstance(revision, int) and revision < new_revision))):
                    ids.append(row["evidence_id"])
                    db.execute("UPDATE check_evidence SET applicable=0,invalidated_reason=?,invalidated_at=? WHERE evidence_id=?",
                               (f"requirement_changed:{requirement_id}", _now(), row["evidence_id"]))
            db.commit()
            return ids
        except BaseException:
            db.rollback(); raise


__all__ = [
    "CHECK_EVIDENCE_SCHEMA", "CheckEvidenceError", "applicable_check_evidence",
    "get_check_evidence", "invalidate_check_evidence", "list_check_evidence",
    "invalidate_requirement_revision", "record_check_evidence",
]
