"""Store.immediate_write: write-lock-first transactions that retry a
contended lock instead of surfacing a raw sqlite 'database is locked' (which
previously escaped to the opaque -32603 MCP server error)."""
from __future__ import annotations

import sqlite3
import threading
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


def _store(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build.", "owner-1")
    return store


def test_commits_body_and_returns_its_value(tmp_path):
    store = _store(tmp_path)
    store.immediate_write(lambda db: db.execute(
        "CREATE TABLE IF NOT EXISTS t_iw (id INTEGER PRIMARY KEY, v TEXT)"))
    out = store.immediate_write(lambda db: db.execute(
        "INSERT INTO t_iw (v) VALUES (?)", ("kept",)))
    assert out.lastrowid == 1
    with store.connection() as db:
        assert db.execute("SELECT v FROM t_iw").fetchone()[0] == "kept"


def test_retries_a_transient_locked_error_then_succeeds(tmp_path):
    store = _store(tmp_path)
    store.immediate_write(lambda db: db.execute(
        "CREATE TABLE IF NOT EXISTS t_iw (id INTEGER PRIMARY KEY, v TEXT)"))
    calls = {"n": 0}

    def body(db):
        calls["n"] += 1
        if calls["n"] < 3:
            raise sqlite3.OperationalError("database is locked")
        db.execute("INSERT INTO t_iw (v) VALUES (?)", ("ok",))

    store.immediate_write(body, base_delay=0, max_delay=0)
    assert calls["n"] == 3  # retried past the two simulated lock errors
    with store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM t_iw").fetchone()[0] == 1  # only the committed run persisted


def test_non_lock_operational_error_is_not_retried(tmp_path):
    store = _store(tmp_path)
    calls = {"n": 0}

    def body(db):
        calls["n"] += 1
        raise sqlite3.OperationalError("no such table: nope")

    with pytest.raises(sqlite3.OperationalError):
        store.immediate_write(body, base_delay=0, max_delay=0)
    assert calls["n"] == 1  # a real error surfaces at once, never spun on


def test_concurrent_writers_all_commit_without_a_spurious_lock_error(tmp_path):
    store = _store(tmp_path)
    store.immediate_write(lambda db: db.execute(
        "CREATE TABLE IF NOT EXISTS t_iw (id INTEGER PRIMARY KEY, v TEXT)"))
    errors = []

    def worker(n):
        try:
            store.immediate_write(lambda db: db.execute(
                "INSERT INTO t_iw (v) VALUES (?)", (f"w{n}",)))
        except Exception as exc:  # noqa: BLE001
            errors.append(exc)

    threads = [threading.Thread(target=worker, args=(n,)) for n in range(12)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert errors == []
    with store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM t_iw").fetchone()[0] == 12
