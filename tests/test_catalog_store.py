"""Route selection history and user-scoped snapshot cache (T007).

Every test sets XDG_CONFIG_HOME/XDG_STATE_HOME/HOME to a tmp_path so no real
user configuration is ever touched.
"""
from __future__ import annotations

import pytest

from cortex_runtime.catalog import store as catalog_store
from cortex_runtime.catalog.discovery import build_catalog, parse_observation
from cortex_runtime.catalog.model import CatalogSnapshot
from cortex_runtime.connections.model import AuthMode, ConnectionRecord, ConnectionState, EngineNativeDir
from cortex_runtime.cleanup import clear_tasks
from cortex_runtime.contracts import StoreError
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.store import Store
from test_markdown_store import create


def _isolate(tmp_path, monkeypatch):
    config_home = tmp_path / "config"
    state_home = tmp_path / "state"
    home = tmp_path / "home"
    for path in (config_home, state_home, home):
        path.mkdir()
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    monkeypatch.setenv("XDG_STATE_HOME", str(state_home))
    monkeypatch.setenv("HOME", str(home))


def _connection(connection_id, engine="codex-acp", name="Work"):
    now = "2026-01-01T00:00:00+00:00"
    return ConnectionRecord(
        id=connection_id, name=name, engine=engine, provider="acme", auth_mode=AuthMode.NATIVE_ACCOUNT,
        key_kind=None, endpoint=None, launch_command=None,
        credential_ref=EngineNativeDir(path=f"/tmp/{connection_id}"), approved_fingerprint="fp",
        state=ConnectionState.CONFIGURED, qualified=True, created_at=now, updated_at=now,
    )


def _snapshot_with_one_entry(connection_id="cn_" + "a" * 12):
    connection = _connection(connection_id)
    options = [
        {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "model-a",
         "options": [{"value": "model-a", "name": "Model A"}]},
    ]
    observation = parse_observation(options)
    snapshot = build_catalog({connection.id: connection}, {connection.id: observation})
    return snapshot, connection


def test_record_and_current_task_route_round_trip(tmp_path):
    store = Store(tmp_path / "private")
    task = create(store, tmp_path)
    snapshot, _ = _snapshot_with_one_entry()
    entry = snapshot.entries[0]
    result = catalog_store.record_task_route(store, task, entry)
    assert result["sequence"] == 1
    current = catalog_store.current_task_route(store, task)
    assert current["route_id"] == entry.route.id
    assert current["label_at_selection"] == entry.disambiguated_label
    assert current["catalog_snapshot_id"] == entry.catalog_snapshot_id


def test_history_is_append_only_and_immutable(tmp_path):
    store = Store(tmp_path / "private")
    task = create(store, tmp_path)
    snapshot, connection = _snapshot_with_one_entry()
    entry = snapshot.entries[0]
    catalog_store.record_task_route(store, task, entry)
    catalog_store.record_task_route(store, task, entry)
    history = catalog_store.task_route_history(store, task)
    assert [row["sequence"] for row in history] == [1, 2]
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        with pytest.raises(Exception, match="immutable"):
            db.execute("UPDATE task_route_selections SET route_id='tampered' WHERE task_id=? AND sequence=1", (task,))
        db.rollback()
        db.execute("BEGIN IMMEDIATE")
        with pytest.raises(Exception, match="immutable"):
            db.execute("DELETE FROM task_route_selections WHERE task_id=? AND sequence=1", (task,))
        db.rollback()


def test_refresh_never_mutates_saved_task_route_and_flags_stale(tmp_path):
    store = Store(tmp_path / "private")
    task = create(store, tmp_path)
    first_snapshot, connection = _snapshot_with_one_entry()
    first_entry = first_snapshot.entries[0]
    catalog_store.record_task_route(store, task, first_entry)
    saved = catalog_store.current_task_route(store, task)

    # A refreshed catalog with a different discovered model set must not
    # retroactively change the already-saved row.
    options = [
        {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "model-b",
         "options": [{"value": "model-b", "name": "Model B"}]},
    ]
    refreshed_snapshot = build_catalog({connection.id: connection}, {connection.id: parse_observation(options)})
    still_saved = catalog_store.current_task_route(store, task)
    assert still_saved == saved
    assert catalog_store.is_stale(saved["route_id"], refreshed_snapshot) is True
    assert catalog_store.is_stale(first_entry.route.id, first_snapshot) is False


def test_retention_cleanup_deletes_route_history_with_task(tmp_path):
    store = Store(tmp_path / "private")
    task = create(store, tmp_path)
    snapshot, _ = _snapshot_with_one_entry()
    catalog_store.record_task_route(store, task, snapshot.entries[0])
    clear_tasks(store, tmp_path, 0)
    with store.connection() as db:
        assert db.execute("SELECT 1 FROM tasks WHERE id=?", (task,)).fetchone() is None
        assert db.execute("SELECT 1 FROM task_route_selections WHERE task_id=?", (task,)).fetchone() is None


def test_record_task_route_rejects_unknown_task(tmp_path):
    store = Store(tmp_path / "private")
    snapshot, _ = _snapshot_with_one_entry()
    with pytest.raises(PortableError) as excinfo:
        catalog_store.record_task_route(store, "t_" + "0" * 12, snapshot.entries[0])
    assert excinfo.value.code == "unknown_task"


def test_save_and_load_connection_snapshot(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    snapshot, connection = _snapshot_with_one_entry()
    catalog_store.save_snapshot(snapshot)
    view = catalog_store.load_connection_snapshot(connection.id, snapshot.id)
    assert view is not None
    assert view.snapshot_id == snapshot.id
    assert len(view.entries) == 1
    assert view.entries[0].route.id == snapshot.entries[0].route.id
    assert view.unavailable_reason is None


def test_load_missing_connection_snapshot_returns_none(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    result = catalog_store.load_connection_snapshot("cn_" + "a" * 12, "cs_" + "0" * 12)
    assert result is None


def test_save_snapshot_records_unavailable_reason_per_connection(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    connection = _connection("cn_" + "a" * 12)
    snapshot = build_catalog({connection.id: connection}, {connection.id: None})
    catalog_store.save_snapshot(snapshot)
    view = catalog_store.load_connection_snapshot(connection.id, snapshot.id)
    assert view.entries == ()
    assert view.unavailable_reason == "discovery_unavailable"


# --- T024: optional context_window_tokens round-trip -----------------------


def test_context_window_tokens_round_trips_through_saved_snapshot(tmp_path, monkeypatch):
    import dataclasses
    _isolate(tmp_path, monkeypatch)
    snapshot, connection = _snapshot_with_one_entry()
    entry = dataclasses.replace(snapshot.entries[0], context_window_tokens=200_000)
    snapshot = dataclasses.replace(snapshot, entries=(entry,))
    catalog_store.save_snapshot(snapshot)
    view = catalog_store.load_connection_snapshot(connection.id, snapshot.id)
    assert view.entries[0].context_window_tokens == 200_000


def test_pre_t024_cached_snapshot_without_the_field_deserializes_to_none(tmp_path, monkeypatch):
    _isolate(tmp_path, monkeypatch)
    snapshot, connection = _snapshot_with_one_entry()
    serialized = catalog_store._serialize_entry(snapshot.entries[0])
    del serialized["context_window_tokens"]  # simulate a pre-T024 cache file
    restored = catalog_store._deserialize_entry(serialized)
    assert restored.context_window_tokens is None
