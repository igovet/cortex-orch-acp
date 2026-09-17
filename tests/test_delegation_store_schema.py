"""T015: delegation_provisioning/delegation_effects as an optional schema
group, handled by split.py/migrate.py/cleanup.py exactly like the existing
IDENTITY_SCHEMA/CATALOG_SCHEMA/CONFIGURATION_SCHEMA/ACP_SESSION_SCHEMA
groups. SQLite user_version stays 11.
"""
from __future__ import annotations

import os
import sqlite3
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.cleanup import clear_tasks
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation.store import DELEGATION_SCHEMA
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.split import split_project
from cortex_runtime.store import APPLICATION_ID, Store


def event():
    return SourceEvent(origin="cli", event_id=str(uuid.uuid4()))


def test_fresh_store_has_delegation_tables_and_user_version_11(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    with store.connection() as db:
        assert db.execute("PRAGMA user_version").fetchone()[0] == 11
        assert db.execute("PRAGMA application_id").fetchone()[0] == APPLICATION_ID
        tables = {row[0] for row in db.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'delegation%'"
        )}
        assert tables == {"delegation_provisioning", "delegation_effects"}


def test_a_pre_t015_v11_store_gains_delegation_tables_on_next_open(tmp_path):
    directory = tmp_path / ".cortex"
    directory.mkdir(mode=0o700)
    path = directory / "cortex.sqlite3"
    fd = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    os.close(fd)
    db = sqlite3.connect(path)
    db.execute(f"PRAGMA application_id={APPLICATION_ID}")
    db.execute("PRAGMA user_version=11")
    from cortex_runtime.acp.sessions import ACP_SESSION_SCHEMA
    from cortex_runtime.catalog.store import CATALOG_SCHEMA
    from cortex_runtime.configuration.store import CONFIGURATION_SCHEMA
    from cortex_runtime.hook_storage import HOOK_SCHEMA
    from cortex_runtime.store import SCHEMA
    from cortex_runtime.trusted_channel import IDENTITY_SCHEMA
    db.executescript(SCHEMA + HOOK_SCHEMA + IDENTITY_SCHEMA + CATALOG_SCHEMA + CONFIGURATION_SCHEMA + ACP_SESSION_SCHEMA)
    db.commit(); db.close()

    store = Store(directory, project_root=tmp_path)
    with store.connection() as db2:
        tables = {row[0] for row in db2.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'delegation%'"
        )}
    assert tables == {"delegation_provisioning", "delegation_effects"}


def _insert_provisioning_row(store, task_id, key="k_" + "a" * 30):
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute(
            "INSERT INTO delegation_provisioning "
            "(idempotency_key, task_id, assignment_id, attempt_id, role_id, step, status, owner_epoch, created_at, updated_at) "
            "VALUES (?,?,?,?,?,?,?,?,?,?)",
            (key, task_id, "a_" + "b" * 12, "xa_" + "c" * 12, "worker-backend-dev", "ready", "ready", 1,
             "2026-01-01T00:00:00.000000Z", "2026-01-01T00:00:00.000000Z"),
        )
        db.execute(
            "INSERT INTO delegation_effects (idempotency_key, step, phase, detail_json, at) VALUES (?,?,?,?,?)",
            (key, "workspace", "intent", "{}", "2026-01-01T00:00:00.000000Z"),
        )
        db.commit()
    return key


def test_split_project_carries_delegation_rows_for_the_selected_project(tmp_path):
    from cortex_runtime.host_source import NativeSource

    source_dir = tmp_path / "legacy"
    source_dir.mkdir(mode=0o700)
    store = Store(source_dir)
    project_root = tmp_path / "project"
    project_root.mkdir()
    source = NativeSource("request-a", {"offset": "a"},
                           [dict(id="message-a", turn="turn-a", text="request-a", attachments=[])])
    store.call("create_task", dict(project_root=str(project_root), request_key="create-a"),
               "thread-a", original_request=lambda: source)
    with store.connection() as db:
        task_id = db.execute("SELECT task_id FROM thread_bindings WHERE thread_id=?", ("thread-a",)).fetchone()[0]
    key = _insert_provisioning_row(store, task_id)

    backup = tmp_path / "backup.sqlite3"
    outcome = split_project(str(source_dir), str(project_root), str(backup), access_stopped=True)
    assert outcome["schema_version"] == 11

    destination = Store(project_root / ".cortex", project_root=project_root)
    with destination.connection() as db:
        row = db.execute("SELECT * FROM delegation_provisioning WHERE idempotency_key=?", (key,)).fetchone()
        assert row is not None
        assert row["task_id"] == task_id
        effects = db.execute("SELECT * FROM delegation_effects WHERE idempotency_key=?", (key,)).fetchall()
        assert len(effects) == 1


def test_clear_tasks_removes_delegation_rows_with_the_task(tmp_path):
    # Uses store.call("create_task", ...) rather than bootstrap_task: a
    # bootstrapped task also gets a trusted_channels/task_owners/
    # source_events row (IDENTITY_SCHEMA), and cleanup.finish_deletions
    # does not delete those tables before deleting the task row -- a
    # pre-existing gap unrelated to this task's delegation schema (not
    # fixed here; see report). This test isolates the new delegation-row
    # deletion behavior from that unrelated gap.
    from cortex_runtime.host_source import NativeSource

    store = Store(tmp_path / "private")
    project = tmp_path / "project"
    project.mkdir()
    source = NativeSource("request-a", {"offset": "a"},
                           [dict(id="message-a", turn="turn-a", text="request-a", attachments=[])])
    store.call("create_task", dict(project_root=str(project), request_key="create-a"),
               "thread-a", original_request=lambda: source)
    with store.connection() as db:
        task_id = db.execute("SELECT task_id FROM thread_bindings WHERE thread_id=?", ("thread-a",)).fetchone()[0]
    key = _insert_provisioning_row(store, task_id)
    clear_tasks(store, str(project), 0)
    with store.connection() as db:
        assert db.execute("SELECT 1 FROM delegation_provisioning WHERE idempotency_key=?", (key,)).fetchone() is None
        assert db.execute("SELECT 1 FROM delegation_effects WHERE idempotency_key=?", (key,)).fetchone() is None
        assert db.execute("SELECT 1 FROM tasks WHERE id=?", (task_id,)).fetchone() is None


def test_delegation_effects_foreign_key_enforced(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        with pytest.raises(sqlite3.IntegrityError):
            db.execute(
                "INSERT INTO delegation_effects (idempotency_key, step, phase, detail_json, at) VALUES (?,?,?,?,?)",
                ("nonexistent-key", "workspace", "intent", "{}", "2026-01-01T00:00:00.000000Z"),
            )
        db.rollback()


def test_delegation_effects_phase_check_constraint(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, event(), "Build a page.", "owner-1")
    key = _insert_provisioning_row(store, result.task_id, key="k_" + "d" * 30)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        with pytest.raises(sqlite3.IntegrityError):
            db.execute(
                "INSERT INTO delegation_effects (idempotency_key, step, phase, detail_json, at) VALUES (?,?,?,?,?)",
                (key, "workspace", "not-a-real-phase", "{}", "2026-01-01T00:00:00.000000Z"),
            )
        db.rollback()
