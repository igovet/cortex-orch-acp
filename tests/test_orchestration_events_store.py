"""T016: delegation.events over the task_events/task_orchestration schema group.

Every Store is rooted at tmp_path (never the real checkout); no engine or
network I/O.
"""
from __future__ import annotations

import uuid

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return store, result.task_id


def test_append_event_is_idempotent_on_dedup_key(tmp_path):
    store, task_id = _task(tmp_path)
    first = delegation_events.append_event(store, task_id, "user_message", "dk-1", dict(content="hi"), origin="s1")
    second = delegation_events.append_event(store, task_id, "user_message", "dk-1", dict(content="hi-different-text"), origin="s1")
    assert first.id == second.id
    assert second.payload["content"] == "hi"  # the first write wins; retry never overwrites


def test_append_event_rejects_unknown_type(tmp_path):
    store, task_id = _task(tmp_path)
    try:
        delegation_events.append_event(store, task_id, "not_a_real_type", "dk", {}, origin="s1")
        assert False, "expected ValueError"
    except ValueError:
        pass


def test_list_since_paginates_by_cursor(tmp_path):
    store, task_id = _task(tmp_path)
    for i in range(5):
        delegation_events.append_event(store, task_id, "progress", f"p{i}", dict(note=str(i)), origin="s1")
    page1 = delegation_events.list_since(store, task_id, after=0, limit=2)
    assert [e.payload["note"] for e in page1] == ["0", "1"]
    page2 = delegation_events.list_since(store, task_id, after=page1[-1].id, limit=2)
    assert [e.payload["note"] for e in page2] == ["2", "3"]


def test_claim_unconsumed_is_exhausted_after_one_claim(tmp_path):
    store, task_id = _task(tmp_path)
    delegation_events.append_event(store, task_id, "user_message", "u1", dict(content="hi"), origin="s1")
    claimed = delegation_events.claim_unconsumed(store, task_id, "cont-1")
    assert len(claimed) == 1
    assert claimed[0].consumed_by == "cont-1"
    second = delegation_events.claim_unconsumed(store, task_id, "cont-2")
    assert second == []
    assert delegation_events.has_unconsumed(store, task_id) is False


def test_mark_superseded_excludes_events_from_a_future_claim(tmp_path):
    store, task_id = _task(tmp_path)
    delegation_events.append_event(
        store, task_id, "worker_result", "wr1", dict(report_ref="r_x", summary="late"),
        origin="s1", assignment_ref="a1",
    )
    delegation_events.mark_superseded(store, task_id, "a1")
    claimed = delegation_events.claim_unconsumed(store, task_id, "cont-1")
    assert claimed == []
    everything = delegation_events.list_since(store, task_id)
    assert len(everything) == 1 and everything[0].superseded is True


def test_two_concurrent_claims_never_see_the_same_event(tmp_path):
    store, task_id = _task(tmp_path)
    delegation_events.append_event(store, task_id, "user_message", "u1", dict(content="hi"), origin="s1")
    first = delegation_events.claim_unconsumed(store, task_id, "cont-a")
    second = delegation_events.claim_unconsumed(store, task_id, "cont-b")
    assert len(first) == 1 and second == []


def test_release_claim_returns_events_to_unconsumed(tmp_path):
    store, task_id = _task(tmp_path)
    delegation_events.append_event(store, task_id, "user_message", "u1", dict(content="hi"), origin="s1")
    claimed = delegation_events.claim_unconsumed(store, task_id, "cont-1")
    assert len(claimed) == 1
    released = delegation_events.release_claim(store, task_id, "cont-1")
    assert released == 1
    again = delegation_events.claim_unconsumed(store, task_id, "cont-2")
    assert len(again) == 1 and again[0].consumed_by == "cont-2"


def test_release_claim_never_touches_a_later_claimers_rows(tmp_path):
    store, task_id = _task(tmp_path)
    delegation_events.append_event(store, task_id, "user_message", "u1", dict(content="hi"), origin="s1")
    delegation_events.claim_unconsumed(store, task_id, "cont-1")
    delegation_events.release_claim(store, task_id, "cont-1")
    delegation_events.claim_unconsumed(store, task_id, "cont-2")
    # A stale release for the first (already-superseded-by-reclaim) continuation
    # id must not un-claim the second claimer's row.
    released = delegation_events.release_claim(store, task_id, "cont-1")
    assert released == 0
    assert delegation_events.has_unconsumed(store, task_id) is False


def test_existing_pre_t017_database_gains_the_additive_columns_on_open(tmp_path):
    """A store whose task_events/task_orchestration tables predate T017
    (created by d301299/T016's TASK_EVENTS_SCHEMA, with no wait_reason/
    wait_detail_json/no_progress_count/delivered_at columns) gains them via
    ALTER TABLE the next time store.Store opens it -- no data loss, no
    user_version bump."""
    import sqlite3

    from cortex_runtime.store import APPLICATION_ID, SCHEMA, private_directory

    directory = private_directory(tmp_path / ".cortex")
    path = directory / "cortex.sqlite3"
    db = sqlite3.connect(path)
    try:
        # The pre-T017 shape: task_events/task_orchestration without the new
        # columns (mirrors the original T016 TASK_EVENTS_SCHEMA exactly).
        pre_t017_task_events_schema = '''
        CREATE TABLE IF NOT EXISTS task_events (
         id INTEGER PRIMARY KEY AUTOINCREMENT,
         task_id TEXT NOT NULL REFERENCES tasks(id),
         event_type TEXT NOT NULL,
         assignment_ref TEXT, dedup_key TEXT NOT NULL,
         payload_json TEXT NOT NULL, origin TEXT NOT NULL,
         created_at TEXT NOT NULL, consumed_by TEXT,
         superseded INTEGER NOT NULL DEFAULT 0,
         UNIQUE(task_id, dedup_key)
        );
        CREATE TABLE IF NOT EXISTS task_orchestration (
         task_id TEXT PRIMARY KEY REFERENCES tasks(id),
         state TEXT NOT NULL,
         coordinator_attempt_ref TEXT,
         updated_at TEXT NOT NULL
        );
        '''
        db.executescript("BEGIN;\n" + SCHEMA + pre_t017_task_events_schema + f"PRAGMA application_id={APPLICATION_ID};\nPRAGMA user_version=12;\nCOMMIT;")
        db.execute("INSERT INTO tasks VALUES (?,?,?)", ("t_legacy0000", str(tmp_path), "2020-01-01T00:00:00.000000"))
        db.execute("INSERT INTO task_orchestration VALUES (?,?,?,?)", ("t_legacy0000", "running", "co_x", "2020-01-01T00:00:00.000000"))
        db.commit()
        before_columns = {row[1] for row in db.execute("PRAGMA table_info(task_orchestration)")}
        assert "wait_reason" not in before_columns
    finally:
        db.close()
        import os

        os.chmod(path, 0o600)

    store = Store(directory, project_root=tmp_path)
    with store.connection() as db:
        columns = {row[1] for row in db.execute("PRAGMA table_info(task_orchestration)")}
        event_columns = {row[1] for row in db.execute("PRAGMA table_info(task_events)")}
        row = db.execute("SELECT state, wait_reason, no_progress_count FROM task_orchestration WHERE task_id=?", ("t_legacy0000",)).fetchone()
    assert {"wait_reason", "wait_detail_json", "no_progress_count"} <= columns
    assert "delivered_at" in event_columns
    assert row["state"] == "running"  # pre-existing data preserved
    assert row["wait_reason"] is None
    assert row["no_progress_count"] == 0

    # Idempotent: opening again (columns already present) is a no-op, not an error.
    Store(directory, project_root=tmp_path)
