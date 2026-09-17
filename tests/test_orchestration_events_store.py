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
