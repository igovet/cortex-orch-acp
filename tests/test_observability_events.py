"""T029: observability.events -- structured trace events with an explicit
basis, plus the two real append sites (store.py duplicate-publication
detection, delegation.mcp_tools schema-validation rejection).

Every Store is rooted at tmp_path (never the real checkout); no engine or
network I/O.
"""
from __future__ import annotations

from pathlib import Path
import uuid

import pytest

from storage_fixture import call_store

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.observability import events as observability_events
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return store, result.task_id


def test_append_event_rejects_unknown_basis(tmp_path):
    store, task_id = _task(tmp_path)
    with pytest.raises(ValueError):
        observability_events.append_event(store, task_id, "publication", "duplicate_attempt", "not_a_real_basis")


def test_append_and_list_events_roundtrip(tmp_path):
    store, task_id = _task(tmp_path)
    observability_events.append_event(
        store, task_id, "validation", "invalid_field", "observed",
        attempt_id="att-1", detail=dict(operation="delegate"),
    )
    observability_events.append_event(
        store, task_id, "publication", "duplicate_attempt", "observed",
        detail=dict(operation="write_report"),
    )
    all_events = observability_events.list_events(store, task_id)
    assert [row["category"] for row in all_events] == ["validation", "publication"]
    only_validation = observability_events.list_events(store, task_id, category="validation")
    assert len(only_validation) == 1
    assert only_validation[0]["basis"] == "observed"
    assert only_validation[0]["attempt_id"] == "att-1"
    assert only_validation[0]["detail"]["operation"] == "delegate"


def test_events_are_immutable(tmp_path):
    """The T029 CHECK/trigger pair mirrors attempt_route_audit's own
    immutability (T027): update/delete are refused outside a task-deletion
    sequence."""
    from cortex_runtime.contracts import StoreError

    store, task_id = _task(tmp_path)
    event_id = observability_events.append_event(store, task_id, "publication", "duplicate_attempt", "observed")
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        with pytest.raises(Exception):
            db.execute("UPDATE observability_events SET outcome='changed' WHERE id=?", (event_id,))
        db.rollback()


def test_duplicate_publication_attempt_is_recorded_on_replay(tmp_path):
    """A write_report call retried with the identical request_key/arguments
    is an idempotent replay (see store.py's own ``previous`` branch) but is
    recorded as a genuine duplicate-publication-attempt event."""
    store = Store(tmp_path / "private")
    task_id = call_store(
        store, "create_task",
        dict(project_root=str(tmp_path), request="Original request", request_key=uuid.uuid4().hex),
    )["task_id"]
    draft = call_store(store, "create_draft", dict(task_id=task_id, template="general", request_key=uuid.uuid4().hex))
    path = Path(draft["draft_path"])
    marker = path.read_bytes().split(b"\n\n", 1)[0] + b"\n\n"
    path.write_bytes(marker + b"Free **Markdown** without a template.")
    args = dict(
        task_id=task_id, title="Work", summary="A concise result.", author="worker",
        draft_id=draft["draft_id"], request_key=uuid.uuid4().hex,
    )
    first = call_store(store, "write_report", args)
    assert first["replayed"] is False
    second = call_store(store, "write_report", args)
    assert second["replayed"] is True

    duplicates = observability_events.list_events(store, task_id, category="publication")
    assert len(duplicates) == 1
    assert duplicates[0]["outcome"] == "duplicate_attempt"
    assert duplicates[0]["detail"]["operation"] == "write_report"
