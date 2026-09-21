"""Durable ACP host session metadata.

One project-scoped SQLite table, composed into ``store.py``'s per-project
database exactly like the ``identity``/``catalog``/``configuration`` optional
table groups it was modeled on (see ``split.py``'s ``_ACP_TABLES`` /
``_acp_present`` and ``migrate.py``). A row records only shape-level,
never-secret metadata about one ACP host session:

- ``task_id`` is set at most once (a trigger rejects any later change): the
  first ``session/prompt`` on a session binds it to the task that
  ``bootstrap.bootstrap_task`` (or ``bootstrap.append_source_event``)
  durably created/continued; a session with no prompt yet has ``task_id``
  NULL.
- ``mcp_servers`` is recorded as inert metadata only (see
  ``protocol.strip_mcp_servers``): server names/transport shape, never
  environment variable or header *values* -- those are never connected to
  and never persisted.
- The channel secret issued by ``bootstrap_task`` for this session's task is
  never written here (or anywhere else): it stays in the host agent
  process's memory only (see ``agent.Agent``).
"""
from __future__ import annotations

from datetime import datetime, timezone
import json

from ..contracts import StoreError

ACP_SESSION_SCHEMA = '''
CREATE TABLE IF NOT EXISTS acp_host_sessions (
 session_id TEXT PRIMARY KEY, project_root TEXT NOT NULL,
 task_id TEXT REFERENCES tasks(id), client_info TEXT NOT NULL,
 client_capabilities TEXT NOT NULL, mcp_servers TEXT NOT NULL,
 created_at TEXT NOT NULL, closed_at TEXT
);
CREATE INDEX IF NOT EXISTS acp_host_sessions_by_task ON acp_host_sessions(task_id);
CREATE TRIGGER IF NOT EXISTS acp_host_sessions_task_immutable
BEFORE UPDATE OF task_id ON acp_host_sessions
WHEN OLD.task_id IS NOT NULL
BEGIN SELECT RAISE(ABORT, 'immutable'); END;
'''

ACP_SESSION_TABLES = frozenset({"acp_host_sessions"})


def _now():
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def _encode(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"))


def create_session(store, session_id, project_root, client_info, client_capabilities, mcp_servers):
    """Insert one fresh, task-less session row."""
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "INSERT INTO acp_host_sessions VALUES (?,?,NULL,?,?,?,?,NULL)",
                (session_id, project_root, _encode(client_info), _encode(client_capabilities),
                 _encode(mcp_servers), _now()))
            db.commit()
        except BaseException:
            db.rollback(); raise


def bind_task(store, session_id, task_id):
    """Set ``task_id`` exactly once; idempotent for the identical task_id.

    A schema trigger (``acp_host_sessions_task_immutable``) rejects any
    attempt to change an already-set ``task_id``; this function additionally
    rejects a *different* task_id explicitly (``acp_session_task_conflict``)
    and skips the no-op UPDATE for the identical one so a retried bind never
    trips the trigger.
    """
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            row = db.execute("SELECT task_id FROM acp_host_sessions WHERE session_id=?", (session_id,)).fetchone()
            if row is None:
                raise StoreError("acp_session_unknown")
            if row["task_id"] is not None:
                if row["task_id"] != task_id:
                    raise StoreError("acp_session_task_conflict")
            else:
                db.execute("UPDATE acp_host_sessions SET task_id=? WHERE session_id=?", (task_id, session_id))
            db.commit()
        except BaseException:
            db.rollback(); raise


def close_session(store, session_id):
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "UPDATE acp_host_sessions SET closed_at=? WHERE session_id=? AND closed_at IS NULL",
                (_now(), session_id))
            db.commit()
        except BaseException:
            db.rollback(); raise


def close_task_sessions(store, task_id):
    """Close every ACP host session bound to a terminally stopped task.

    A task can have more than one bound host session after a reconnect. A
    task-wide stop invalidates the task for all of them, so leaving an older
    row open would make durable host-session state disagree with the terminal
    task state. The update is idempotent and scoped to the task's bound rows.
    """
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "UPDATE acp_host_sessions SET closed_at=? "
                "WHERE task_id=? AND closed_at IS NULL",
                (_now(), task_id),
            )
            db.commit()
        except BaseException:
            db.rollback(); raise


def get_session(store, session_id):
    with store.connection() as db:
        row = db.execute("SELECT * FROM acp_host_sessions WHERE session_id=?", (session_id,)).fetchone()
    if row is None:
        return None
    return dict(
        session_id=row["session_id"], project_root=row["project_root"], task_id=row["task_id"],
        client_info=json.loads(row["client_info"]), client_capabilities=json.loads(row["client_capabilities"]),
        mcp_servers=json.loads(row["mcp_servers"]), created_at=row["created_at"], closed_at=row["closed_at"])
