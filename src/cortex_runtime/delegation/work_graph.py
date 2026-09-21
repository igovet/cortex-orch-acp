"""Durable, model-owned requirements and assignment graph.

This module deliberately contains no scheduling policy.  The coordinator chooses
requirements, predecessors and candidates; :class:`WorkGraph` only records those
choices and rejects references which cannot be made exact.  The small schema is
additive and is provisioned lazily, which keeps it usable with pre-existing v11
stores without changing the store format version.
"""
from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone
import hashlib
import json
import sqlite3
import uuid

from ..contracts import StoreError


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def _json(value) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def _digest(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


WORK_GRAPH_SCHEMA = """
CREATE TABLE IF NOT EXISTS work_requirements (
 requirement_id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 current_revision INTEGER NOT NULL,
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS work_requirements_by_task ON work_requirements(task_id);
CREATE TABLE IF NOT EXISTS work_requirement_revisions (
 requirement_id TEXT NOT NULL REFERENCES work_requirements(requirement_id),
 revision INTEGER NOT NULL,
 statement TEXT NOT NULL,
 criteria_json TEXT NOT NULL,
 digest TEXT NOT NULL,
 created_at TEXT NOT NULL,
 PRIMARY KEY(requirement_id, revision)
);
CREATE TABLE IF NOT EXISTS work_assignments (
 assignment_id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 node_key TEXT NOT NULL,
 requirement_bindings_json TEXT NOT NULL,
 candidate_ref TEXT,
 status TEXT NOT NULL,
 idempotency_key TEXT NOT NULL,
 created_at TEXT NOT NULL,
 UNIQUE(task_id, idempotency_key)
);
CREATE INDEX IF NOT EXISTS work_assignments_by_task ON work_assignments(task_id);
CREATE TABLE IF NOT EXISTS work_assignment_predecessors (
 task_id TEXT NOT NULL REFERENCES tasks(id),
 assignment_id TEXT NOT NULL REFERENCES work_assignments(assignment_id),
 predecessor_assignment_id TEXT NOT NULL REFERENCES work_assignments(assignment_id),
 predecessor_candidate_ref TEXT NOT NULL,
 PRIMARY KEY(assignment_id, predecessor_assignment_id)
);
CREATE INDEX IF NOT EXISTS work_predecessors_by_predecessor
 ON work_assignment_predecessors(predecessor_assignment_id);
CREATE TABLE IF NOT EXISTS work_assignment_attempts (
 attempt_id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 assignment_id TEXT NOT NULL REFERENCES work_assignments(assignment_id),
 attempt_no INTEGER NOT NULL,
 predecessor_attempt_id TEXT REFERENCES work_assignment_attempts(attempt_id),
 input_candidate_ref TEXT,
 requirement_bindings_json TEXT NOT NULL,
 status TEXT NOT NULL,
 failure_reason TEXT,
 idempotency_key TEXT NOT NULL,
 created_at TEXT NOT NULL,
 UNIQUE(task_id, idempotency_key),
 UNIQUE(assignment_id, attempt_no)
);
CREATE INDEX IF NOT EXISTS work_attempts_by_assignment
 ON work_assignment_attempts(assignment_id, attempt_no);
CREATE TABLE IF NOT EXISTS work_graph_operations (
 task_id TEXT NOT NULL REFERENCES tasks(id),
 idempotency_key TEXT NOT NULL,
 operation TEXT NOT NULL,
 request_json TEXT NOT NULL DEFAULT '{}',
 response_json TEXT NOT NULL,
 created_at TEXT NOT NULL,
 PRIMARY KEY(task_id, idempotency_key)
);
"""


class WorkGraphError(StoreError):
    """Invalid or non-replayable work-graph input."""


class WorkGraph:
    """The durable graph API used by the coordinator and future MCP wiring."""

    def __init__(self, store):
        self.store = store

    def ensure_schema(self) -> None:
        with self.store.connection() as db:
            self._ensure_schema(db)

    @staticmethod
    def _ensure_schema(db) -> None:
        db.executescript(WORK_GRAPH_SCHEMA)
        columns = {row[1] for row in db.execute("PRAGMA table_info(work_graph_operations)")}
        if "request_json" not in columns:
            db.execute("ALTER TABLE work_graph_operations ADD COLUMN request_json TEXT NOT NULL DEFAULT '{}'")

    def _begin(self):
        db = self.store.connection()
        conn = db.__enter__()
        self._ensure_schema(conn)
        conn.execute("BEGIN IMMEDIATE")
        return db, conn

    @staticmethod
    def _finish(handle, conn, error=None):
        try:
            if error is None:
                conn.commit()
            else:
                conn.rollback()
        finally:
            if error is None:
                handle.__exit__(None, None, None)
            else:
                handle.__exit__(type(error), error, getattr(error, "__traceback__", None))

    @staticmethod
    def _task(conn, task_id: str) -> None:
        if conn.execute("SELECT 1 FROM tasks WHERE id=?", (task_id,)).fetchone() is None:
            raise WorkGraphError("unknown_task")

    @staticmethod
    def _replay(conn, task_id: str, key: str, operation: str, request):
        row = conn.execute(
            "SELECT operation,response_json,request_json FROM work_graph_operations "
            "WHERE task_id=? AND idempotency_key=?", (task_id, key)
        ).fetchone()
        if row is None:
            return None
        if row[0] != operation:
            raise WorkGraphError("idempotency_key_reused")
        if row[2] != _json(request):
            raise WorkGraphError("idempotency_request_changed")
        return json.loads(row[1])

    @staticmethod
    def _record(conn, task_id: str, key: str, operation: str, request, result):
        conn.execute(
            "INSERT INTO work_graph_operations(task_id,idempotency_key,operation,request_json,response_json,created_at) "
            "VALUES(?,?,?,?,?,?)", (task_id, key, operation, _json(request), _json(result), _now())
        )

    def create_requirement(self, task_id: str, statement: str, *, criteria=None,
                           requirement_id: str | None = None,
                           idempotency_key: str | None = None) -> dict:
        """Create a requirement at revision 1, or replay the same operation."""
        key = idempotency_key or uuid.uuid4().hex
        rid = requirement_id or uuid.uuid4().hex
        handle, db = self._begin()
        try:
            self._task(db, task_id)
            request = {"statement": statement, "criteria": criteria, "requirement_id": requirement_id}
            replay = self._replay(db, task_id, key, "create_requirement", request)
            if replay is not None:
                self._finish(handle, db); return replay
            now = _now(); criteria = [] if criteria is None else criteria
            digest = _digest(_json({"statement": statement, "criteria": criteria}))
            db.execute("INSERT INTO work_requirements VALUES(?,?,?,?)", (rid, task_id, 1, now))
            db.execute("INSERT INTO work_requirement_revisions VALUES(?,?,?,?,?,?)",
                       (rid, 1, statement, _json(criteria), digest, now))
            result = {"requirement_id": rid, "task_id": task_id, "revision": 1,
                      "statement": statement, "criteria": criteria, "digest": digest}
            self._record(db, task_id, key, "create_requirement", request, result)
            self._finish(handle, db); return result
        except BaseException as error:
            self._finish(handle, db, error); raise

    def revise_requirement(self, requirement_id: str, statement: str, *, criteria=None,
                           idempotency_key: str | None = None) -> dict:
        key = idempotency_key or uuid.uuid4().hex
        handle, db = self._begin()
        try:
            row = db.execute("SELECT task_id,current_revision FROM work_requirements WHERE requirement_id=?",
                             (requirement_id,)).fetchone()
            if row is None: raise WorkGraphError("unknown_requirement")
            task_id, current = row[0], row[1]
            request = {"requirement_id": requirement_id, "statement": statement, "criteria": criteria}
            replay = self._replay(db, task_id, key, "revise_requirement", request)
            if replay is not None: self._finish(handle, db); return replay
            criteria = [] if criteria is None else criteria; revision = current + 1; now = _now()
            digest = _digest(_json({"statement": statement, "criteria": criteria}))
            db.execute("UPDATE work_requirements SET current_revision=? WHERE requirement_id=?", (revision, requirement_id))
            db.execute("INSERT INTO work_requirement_revisions VALUES(?,?,?,?,?,?)",
                       (requirement_id, revision, statement, _json(criteria), digest, now))
            result = {"requirement_id": requirement_id, "task_id": task_id, "revision": revision,
                      "statement": statement, "criteria": criteria, "digest": digest}
            self._record(db, task_id, key, "revise_requirement", request, result)
            self._finish(handle, db); return result
        except BaseException as error:
            self._finish(handle, db, error); raise

    def add_assignment(self, task_id: str, node_key: str, *, requirement_bindings=None,
                       predecessors=None, candidate_ref: str | None = None,
                       assignment_id: str | None = None, idempotency_key: str | None = None,
                       status: str = "planned") -> dict:
        """Record one model-chosen node and its exact predecessor bindings.

        ``predecessors`` is a list of ``{"assignment_id": ..., "candidate_ref": ...}``.
        A candidate reference is mandatory for each predecessor, so a later
        worker cannot silently consume a newer result.
        """
        key = idempotency_key or uuid.uuid4().hex; aid = assignment_id or uuid.uuid4().hex
        bindings = {} if requirement_bindings is None else requirement_bindings
        predecessors = [] if predecessors is None else predecessors
        handle, db = self._begin()
        try:
            self._task(db, task_id)
            request = {"assignment_id": assignment_id, "node_key": node_key,
                       "requirement_bindings": bindings, "predecessors": predecessors,
                       "candidate_ref": candidate_ref, "status": status}
            replay = self._replay(db, task_id, key, "add_assignment", request)
            if replay is not None: self._finish(handle, db); return replay
            if not node_key or not isinstance(bindings, dict): raise WorkGraphError("invalid_assignment")
            if candidate_ref is not None:
                raise WorkGraphError("candidate_binding_required")
            for requirement_id, binding in bindings.items():
                if not isinstance(binding, dict) or not isinstance(binding.get("revision"), int) or binding["revision"] < 1:
                    raise WorkGraphError("inexact_requirement_ref")
                requirement = db.execute(
                    "SELECT 1 FROM work_requirement_revisions r JOIN work_requirements q "
                    "ON q.requirement_id=r.requirement_id WHERE r.requirement_id=? "
                    "AND r.revision=? AND q.task_id=?", (requirement_id, binding["revision"], task_id)
                ).fetchone()
                if requirement is None:
                    raise WorkGraphError("unknown_requirement_revision")
            normalized = []
            seen = set()
            for item in predecessors:
                if not isinstance(item, dict) or not item.get("assignment_id") or not item.get("candidate_ref"):
                    raise WorkGraphError("inexact_predecessor_ref")
                pred = item["assignment_id"]
                if pred in seen or pred == aid: raise WorkGraphError("assignment_cycle")
                seen.add(pred)
                prow = db.execute("SELECT task_id FROM work_assignments WHERE assignment_id=?", (pred,)).fetchone()
                if prow is None or prow[0] != task_id: raise WorkGraphError("unknown_predecessor")
                recorded = db.execute("SELECT candidate_ref FROM work_assignments WHERE assignment_id=?", (pred,)).fetchone()[0]
                if str(item["candidate_ref"]) != recorded: raise WorkGraphError("predecessor_candidate_mismatch")
                normalized.append({"assignment_id": pred, "candidate_ref": str(item["candidate_ref"])})
            self._check_cycle(db, aid, normalized)
            now = _now()
            db.execute("INSERT INTO work_assignments VALUES(?,?,?,?,?,?,?,?)",
                       (aid, task_id, node_key, _json(bindings), candidate_ref, status, key, now))
            for item in normalized:
                db.execute("INSERT INTO work_assignment_predecessors VALUES(?,?,?,?)",
                           (task_id, aid, item["assignment_id"], item["candidate_ref"]))
            result = {"assignment_id": aid, "task_id": task_id, "node_key": node_key,
                      "requirement_bindings": bindings, "candidate_ref": candidate_ref,
                      "predecessors": normalized, "status": status}
            self._record(db, task_id, key, "add_assignment", request, result)
            self._finish(handle, db); return result
        except BaseException as error:
            self._finish(handle, db, error); raise

    @staticmethod
    def _check_cycle(db, aid, predecessors):
        # The new node may only point to existing nodes.  Walk existing edges
        # from every predecessor and reject a path which already reaches aid.
        stack = [item["assignment_id"] for item in predecessors]; seen = set()
        while stack:
            current = stack.pop()
            if current in seen: continue
            seen.add(current)
            if current == aid: raise WorkGraphError("assignment_cycle")
            stack.extend(row[0] for row in db.execute(
                "SELECT predecessor_assignment_id FROM work_assignment_predecessors WHERE assignment_id=?", (current,)))

    def bind_candidate(self, assignment_id: str, candidate_id: str, *,
                       idempotency_key: str | None = None) -> dict:
        """Bind an assignment to a ready candidate staged for its task.

        Candidate references are deliberately absent on planned assignments.
        A predecessor can be consumed only after this method has verified the
        exact ready candidate row and recorded ``candidate:<candidate_id>``.
        """
        key = idempotency_key or uuid.uuid4().hex
        handle, db = self._begin()
        try:
            row = db.execute("SELECT task_id,candidate_ref FROM work_assignments WHERE assignment_id=?", (assignment_id,)).fetchone()
            if row is None: raise WorkGraphError("unknown_assignment")
            task_id = row[0]; request = {"assignment_id": assignment_id, "candidate_id": candidate_id}
            replay = self._replay(db, task_id, key, "bind_candidate", request)
            if replay is not None: self._finish(handle, db); return replay
            try:
                candidate = db.execute("SELECT task_id,status,content_digest FROM candidate_staging WHERE candidate_id=?", (candidate_id,)).fetchone()
            except sqlite3.OperationalError as error:
                raise WorkGraphError("candidate_unavailable") from error
            if candidate is None or candidate[0] != task_id: raise WorkGraphError("candidate_task_mismatch")
            if candidate[1] != "ready" or not candidate[2]: raise WorkGraphError("candidate_not_ready")
            ref = f"candidate:{candidate_id}"
            if row[1] is not None and row[1] != ref: raise WorkGraphError("candidate_already_bound")
            db.execute("UPDATE work_assignments SET candidate_ref=?,status=? WHERE assignment_id=?", (ref, "candidate_ready", assignment_id))
            result = {"assignment_id": assignment_id, "task_id": task_id, "candidate_id": candidate_id, "candidate_ref": ref, "status": "candidate_ready"}
            self._record(db, task_id, key, "bind_candidate", request, result)
            self._finish(handle, db); return result
        except BaseException as error:
            self._finish(handle, db, error); raise

    def create_attempt(self, assignment_id: str, *, predecessor_attempt_id: str | None = None,
                       input_candidate_ref: str | None = None,
                       idempotency_key: str | None = None) -> dict:
        """Create an executable attempt for a planned assignment.

        A continuation must explicitly name a failed or revision-requested
        predecessor attempt belonging to the same assignment.  This method
        records no semantic acceptance; completion and acceptance remain
        coordinator decisions elsewhere.
        """
        key = idempotency_key or uuid.uuid4().hex
        handle, db = self._begin()
        try:
            assignment = db.execute(
                "SELECT task_id,requirement_bindings_json,candidate_ref FROM work_assignments WHERE assignment_id=?",
                (assignment_id,)).fetchone()
            if assignment is None: raise WorkGraphError("unknown_assignment")
            task_id, bindings_json, assignment_candidate = assignment
            request = {"assignment_id": assignment_id, "predecessor_attempt_id": predecessor_attempt_id,
                       "input_candidate_ref": input_candidate_ref}
            replay = self._replay(db, task_id, key, "create_attempt", request)
            if replay is not None: self._finish(handle, db); return replay
            predecessor = None
            if predecessor_attempt_id is not None:
                predecessor = db.execute(
                    "SELECT attempt_no,status,task_id,assignment_id FROM work_assignment_attempts WHERE attempt_id=?",
                    (predecessor_attempt_id,)).fetchone()
                if predecessor is None or predecessor[2] != task_id or predecessor[3] != assignment_id:
                    raise WorkGraphError("invalid_predecessor_attempt")
                if predecessor[1] not in ("failed", "needs_revision"):
                    raise WorkGraphError("attempt_not_continuable")
            if input_candidate_ref is not None and assignment_candidate is not None and input_candidate_ref != assignment_candidate:
                raise WorkGraphError("input_candidate_mismatch")
            attempt_no = 1 if predecessor is None else predecessor[0] + 1
            attempt_id = uuid.uuid4().hex
            now = _now()
            db.execute(
                "INSERT INTO work_assignment_attempts VALUES(?,?,?,?,?,?,?,?,?,?,?)",
                (attempt_id, task_id, assignment_id, attempt_no, predecessor_attempt_id,
                 input_candidate_ref, bindings_json, "planned", None, key, now),
            )
            # The assignment row is the current-attempt projection consumed
            # by host worker lists.  A continuation after a failed attempt
            # must become planned again rather than retaining the predecessor
            # failure forever.
            db.execute(
                "UPDATE work_assignments SET status='planned' WHERE assignment_id=?",
                (assignment_id,),
            )
            result = {"attempt_id": attempt_id, "task_id": task_id, "assignment_id": assignment_id,
                      "attempt_no": attempt_no, "predecessor_attempt_id": predecessor_attempt_id,
                      "input_candidate_ref": input_candidate_ref, "status": "planned"}
            self._record(db, task_id, key, "create_attempt", request, result)
            self._finish(handle, db); return result
        except BaseException as error:
            self._finish(handle, db, error); raise

    def fail_attempt(self, attempt_id: str, reason: str, *, status: str = "failed",
                     idempotency_key: str | None = None) -> dict:
        """Durably mark an attempt failed or needing revision."""
        if status not in ("failed", "needs_revision") or not reason:
            raise WorkGraphError("invalid_attempt_failure")
        key = idempotency_key or uuid.uuid4().hex
        handle, db = self._begin()
        try:
            row = db.execute(
                "SELECT task_id,status,assignment_id FROM work_assignment_attempts WHERE attempt_id=?",
                (attempt_id,),
            ).fetchone()
            if row is None: raise WorkGraphError("unknown_attempt")
            task_id = row[0]; request = {"attempt_id": attempt_id, "reason": reason, "status": status}
            replay = self._replay(db, task_id, key, "fail_attempt", request)
            if replay is not None: self._finish(handle, db); return replay
            if row[1] not in (("planned", "running", "reported") if status == "needs_revision" else ("planned", "running")):
                raise WorkGraphError("attempt_not_active")
            db.execute("UPDATE work_assignment_attempts SET status=?,failure_reason=? WHERE attempt_id=?",
                       (status, reason, attempt_id))
            db.execute(
                "UPDATE work_assignments SET status=? WHERE assignment_id=?",
                (status, row[2]),
            )
            result = {"attempt_id": attempt_id, "task_id": task_id, "status": status, "failure_reason": reason}
            self._record(db, task_id, key, "fail_attempt", request, result)
            self._finish(handle, db); return result
        except BaseException as error:
            self._finish(handle, db, error); raise

    def transition_attempt(self, attempt_id: str, status: str, *, reason: str | None = None,
                           idempotency_key: str | None = None) -> dict:
        """Apply one durable lifecycle transition to an attempt.

        This records execution state only.  ``reported`` means that the
        worker reported a result; it does not accept the result or integrate
        it into the project.  Those remain separate coordinator decisions.
        """
        allowed = {"planned": {"running", "reported", "failed", "needs_revision"},
                   "running": {"reported", "failed", "needs_revision"},
                   "reported": {"needs_revision"}, "failed": set(), "needs_revision": set()}
        if status not in allowed or status in ("failed", "needs_revision") and not reason:
            raise WorkGraphError("invalid_attempt_transition")
        key = idempotency_key or uuid.uuid4().hex
        handle, db = self._begin()
        try:
            row = db.execute(
                "SELECT task_id,status,failure_reason,assignment_id "
                "FROM work_assignment_attempts WHERE attempt_id=?",
                (attempt_id,),
            ).fetchone()
            if row is None: raise WorkGraphError("unknown_attempt")
            task_id = row[0]; request = {"attempt_id": attempt_id, "status": status, "reason": reason}
            replay = self._replay(db, task_id, key, "transition_attempt", request)
            if replay is not None: self._finish(handle, db); return replay
            if status not in allowed[row[1]]:
                raise WorkGraphError("invalid_attempt_transition")
            failure_reason = reason if status in ("failed", "needs_revision") else None
            db.execute("UPDATE work_assignment_attempts SET status=?,failure_reason=? WHERE attempt_id=?",
                       (status, failure_reason, attempt_id))
            # Keep the assignment-level lifecycle truthful for clients which
            # list workers without joining the attempt table.  Semantic
            # acceptance remains separate; this mirrors execution only.
            db.execute(
                "UPDATE work_assignments SET status=? WHERE assignment_id=?",
                (status, row[3]),
            )
            result = {"attempt_id": attempt_id, "task_id": task_id, "status": status,
                      "failure_reason": failure_reason}
            self._record(db, task_id, key, "transition_attempt", request, result)
            self._finish(handle, db); return result
        except BaseException as error:
            self._finish(handle, db, error); raise

    def requirement_impact(self, task_id: str, requirement_id: str, *, revision: int | None = None) -> list[dict]:
        """Return assignments bound to an older revision of a requirement."""
        with self.store.connection() as db:
            self._ensure_schema(db); self._task(db, task_id)
            row = db.execute("SELECT current_revision FROM work_requirements WHERE requirement_id=? AND task_id=?",
                             (requirement_id, task_id)).fetchone()
            if row is None: raise WorkGraphError("unknown_requirement")
            target = revision or row[0]; result = []
            for item in db.execute("SELECT * FROM work_assignments WHERE task_id=? ORDER BY created_at,assignment_id", (task_id,)):
                bindings = json.loads(item["requirement_bindings_json"])
                bound = bindings.get(requirement_id)
                if isinstance(bound, dict): bound = bound.get("revision")
                if bound is not None and int(bound) < target:
                    result.append({"assignment_id": item["assignment_id"], "node_key": item["node_key"],
                                   "bound_revision": int(bound), "current_revision": target})
            return result

    def get_assignment(self, assignment_id: str) -> dict | None:
        with self.store.connection() as db:
            self._ensure_schema(db)
            row = db.execute("SELECT * FROM work_assignments WHERE assignment_id=?", (assignment_id,)).fetchone()
            if row is None: return None
            preds = db.execute("SELECT predecessor_assignment_id,predecessor_candidate_ref FROM work_assignment_predecessors WHERE assignment_id=? ORDER BY predecessor_assignment_id", (assignment_id,)).fetchall()
            return {"assignment_id": row["assignment_id"], "task_id": row["task_id"], "node_key": row["node_key"],
                    "requirement_bindings": json.loads(row["requirement_bindings_json"]), "candidate_ref": row["candidate_ref"],
                    "status": row["status"], "predecessors": [{"assignment_id": p[0], "candidate_ref": p[1]} for p in preds]}

    def snapshot(self, task_id: str, *, limit: int = 1000) -> dict:
        """Return a bounded, coordinator-readable graph snapshot."""
        if not isinstance(limit, int) or limit < 1 or limit > 1000:
            raise WorkGraphError("invalid_snapshot_limit")
        with self.store.connection() as db:
            self._ensure_schema(db); self._task(db, task_id)
            requirements = []
            for row in db.execute(
                "SELECT q.requirement_id,q.current_revision,r.statement,r.criteria_json,r.digest "
                "FROM work_requirements q JOIN work_requirement_revisions r "
                "ON r.requirement_id=q.requirement_id AND r.revision=q.current_revision "
                "WHERE q.task_id=? ORDER BY q.created_at,q.requirement_id LIMIT ?", (task_id, limit)):
                requirements.append({"requirement_id": row[0], "revision": row[1], "statement": row[2],
                                    "criteria": json.loads(row[3]), "digest": row[4]})
            assignments = []
            for row in db.execute("SELECT * FROM work_assignments WHERE task_id=? ORDER BY created_at,assignment_id LIMIT ?", (task_id, limit)):
                assignments.append(self.get_assignment(row["assignment_id"]))
            return {"task_id": task_id, "requirements": requirements, "assignments": assignments,
                    "truncated": len(requirements) >= limit or len(assignments) >= limit}
