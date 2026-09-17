"""configuration_revisions/project_configuration_defaults persistence (T008).

Every test that touches user-scoped storage sets XDG_CONFIG_HOME/
XDG_STATE_HOME/HOME to a tmp_path so no real user configuration is ever
touched.
"""
from __future__ import annotations

import pytest

from cortex_runtime.configuration import store as configuration_store
from cortex_runtime.configuration.policy import PermissionLevel, PolicyCeiling
from cortex_runtime.cleanup import clear_tasks
from cortex_runtime.portable.identity import ExecutionRoute
from cortex_runtime.store import Store
from test_markdown_store import create


def _isolate(tmp_path, monkeypatch):
    config_home = tmp_path / "config"; state_home = tmp_path / "state"; home = tmp_path / "home"
    for path in (config_home, state_home, home):
        path.mkdir()
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    monkeypatch.setenv("XDG_STATE_HOME", str(state_home))
    monkeypatch.setenv("HOME", str(home))


_ROUTE = ExecutionRoute(engine="codex-acp", connection_id="cn_" + "a" * 12, model="model-a", supported_parameters=(("effort", "high"),))


def test_project_defaults_round_trip_and_freely_updatable(tmp_path):
    store = Store(tmp_path / "private")
    create(store, tmp_path)
    ceiling = PolicyCeiling(permission_level=PermissionLevel.STANDARD)
    written = configuration_store.write_project_defaults(store, str(tmp_path), {"effort": "medium"}, ceiling)
    assert written["defaults"] == {"effort": "medium"}
    read = configuration_store.read_project_defaults(store, str(tmp_path))
    assert read["defaults"] == {"effort": "medium"}
    assert read["ceiling"] == ceiling
    # A default is a live setting, not history: it is freely updatable.
    configuration_store.write_project_defaults(store, str(tmp_path), {"effort": "low"}, ceiling)
    assert configuration_store.read_project_defaults(store, str(tmp_path))["defaults"] == {"effort": "low"}


def test_read_project_defaults_returns_none_when_unset(tmp_path):
    store = Store(tmp_path / "private")
    assert configuration_store.read_project_defaults(store, str(tmp_path)) is None


def test_user_defaults_round_trip(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    assert configuration_store.read_user_defaults() == dict(defaults={}, ceiling=PolicyCeiling())
    ceiling = PolicyCeiling(permission_level=PermissionLevel.ELEVATED)
    configuration_store.write_user_defaults({"effort": "high"}, ceiling)
    read = configuration_store.read_user_defaults()
    assert read["defaults"] == {"effort": "high"}
    assert read["ceiling"] == ceiling


def test_revision_lifecycle_identity_columns_are_immutable(tmp_path):
    store = Store(tmp_path / "private")
    task = create(store, tmp_path)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        row = configuration_store.insert_revision_tx(db, task, 1, _ROUTE, {"effort": "high"}, PolicyCeiling())
        db.commit()
    assert row["state"] == "selected" and row["revision"] == 1
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        with pytest.raises(Exception, match="immutable"):
            db.execute("UPDATE configuration_revisions SET route_id='tampered' WHERE id=?", (row["id"],))
        db.rollback()


def test_revision_state_machine_rejects_invalid_transitions(tmp_path):
    store = Store(tmp_path / "private")
    task = create(store, tmp_path)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        row = configuration_store.insert_revision_tx(db, task, 1, _ROUTE, {}, PolicyCeiling())
        db.commit()
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        with pytest.raises(Exception, match="invalid_transition"):
            db.execute("UPDATE configuration_revisions SET state='active',confirmation_evidence_ref='ev' WHERE id=?", (row["id"],))
        db.rollback()
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        with pytest.raises(Exception, match="missing_confirmation_evidence"):
            db.execute("UPDATE configuration_revisions SET state='pending_next_use' WHERE id=?", (row["id"],))
            db.execute("UPDATE configuration_revisions SET state='active' WHERE id=?", (row["id"],))
        db.rollback()
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        configuration_store.transition_tx(db, task, row["id"], "pending_next_use")
        configuration_store.transition_tx(db, task, row["id"], "active", confirmation_evidence_ref="ev_1")
        db.commit()
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        with pytest.raises(Exception, match="invalid_transition"):
            db.execute("UPDATE configuration_revisions SET state='rejected' WHERE id=?", (row["id"],))
        db.rollback()


def test_history_and_current_active_and_unique_revision_per_task(tmp_path):
    store = Store(tmp_path / "private")
    task = create(store, tmp_path)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        first = configuration_store.insert_revision_tx(db, task, 1, _ROUTE, {}, PolicyCeiling())
        configuration_store.transition_tx(db, task, first["id"], "pending_next_use")
        configuration_store.transition_tx(db, task, first["id"], "active", confirmation_evidence_ref="ev_1")
        db.commit()
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        with pytest.raises(Exception):
            configuration_store.insert_revision_tx(db, task, 1, _ROUTE, {}, PolicyCeiling())
        db.rollback()
    with store.connection() as db:
        history = configuration_store.history_tx(db, task)
        current = configuration_store.current_active_tx(db, task)
    assert [row["revision"] for row in history] == [1]
    assert current["id"] == first["id"]


def test_retention_cleanup_deletes_configuration_revisions_with_task_but_not_project_defaults(tmp_path):
    store = Store(tmp_path / "private")
    task = create(store, tmp_path)
    configuration_store.write_project_defaults(store, str(tmp_path), {"effort": "medium"}, PolicyCeiling())
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        configuration_store.insert_revision_tx(db, task, 1, _ROUTE, {}, PolicyCeiling())
        db.commit()
    clear_tasks(store, tmp_path, 0)
    with store.connection() as db:
        assert db.execute("SELECT 1 FROM tasks WHERE id=?", (task,)).fetchone() is None
        assert db.execute("SELECT 1 FROM configuration_revisions WHERE task_id=?", (task,)).fetchone() is None
    # Project defaults are a project-level setting, never deleted by
    # per-task retention (only a whole-project retirement would touch them,
    # which is out of scope for clear_tasks).
    assert configuration_store.read_project_defaults(store, str(tmp_path)) is not None
