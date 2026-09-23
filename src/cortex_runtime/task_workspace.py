"""One task's working tree, apart from the project store that records it.

A session opened in a linked Git worktree shares the main repository's one
``.cortex`` store (``project_storage.logical_project_root``): ``tasks.
project_root`` and every store key stay the logical project. The files that
task reads, writes, runs commands in and verifies are the worktree the
session was opened in. That tree is recorded here, once per task, in the
same transaction that creates the task, and every file-side consumer asks
for it through :func:`task_root`. A task without a row (created before this
existed, or opened directly in the project) works in its project root.
"""
from __future__ import annotations

from datetime import datetime, timezone
import sqlite3

from .contracts import StoreError

TASK_WORKSPACE_SCHEMA = '''
CREATE TABLE IF NOT EXISTS task_workspaces (
 task_id TEXT PRIMARY KEY REFERENCES tasks(id),
 workspace_root TEXT NOT NULL,
 recorded_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS acp_session_workspaces (
 session_id TEXT PRIMARY KEY REFERENCES acp_host_sessions(session_id),
 workspace_root TEXT NOT NULL
);
'''


def record_task(db, task_id, workspace_root):
    """Bind a task to its working tree inside the caller's transaction.

    The first binding is permanent: the same tree again is a no-op and a
    different one is refused, so a task can never move its edits to another
    checkout part-way through.
    """
    db.execute(
        "INSERT OR IGNORE INTO task_workspaces VALUES (?,?,?)",
        (task_id, workspace_root, datetime.now(timezone.utc).isoformat(timespec="microseconds")))
    row = db.execute("SELECT workspace_root FROM task_workspaces WHERE task_id=?", (task_id,)).fetchone()
    if row[0] != workspace_root:
        raise StoreError("task_workspace_conflict")


def task_root(db, task_id):
    """The directory a task's file work happens in."""
    try:
        row = db.execute("SELECT workspace_root FROM task_workspaces WHERE task_id=?", (task_id,)).fetchone()
    except sqlite3.OperationalError:
        # A store not yet opened writable since this table was added has
        # no bindings at all, so every task there works in its project.
        row = None
    if row is not None:
        return row[0]
    row = db.execute("SELECT project_root FROM tasks WHERE id=?", (task_id,)).fetchone()
    if row is None:
        raise StoreError("task_not_found")
    return row[0]


def store_task_root(store, task_id):
    with store.connection() as db:
        return task_root(db, task_id)


def record_session(db, session_id, workspace_root):
    db.execute("INSERT OR IGNORE INTO acp_session_workspaces VALUES (?,?)", (session_id, workspace_root))


def session_root(store, session_id):
    """The working tree a session was opened in, or None for a session
    recorded before this existed."""
    with store.connection() as db:
        try:
            row = db.execute(
                "SELECT workspace_root FROM acp_session_workspaces WHERE session_id=?", (session_id,)).fetchone()
        except sqlite3.OperationalError:
            row = None
    return row[0] if row is not None else None
