"""T017: delegation.capacity -- reserve/release/ambiguous concurrency
accounting, and history retention across release."""
from __future__ import annotations

import uuid

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import capacity
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return store, result.task_id


def test_reserve_up_to_limit_then_refuses(tmp_path):
    store, task_id = _task(tmp_path)
    ids = [capacity.reserve(store, "cn_1", task_id, f"xa_{i}", "worker", limit=2) for i in range(2)]
    assert all(ids)
    assert capacity.reserve(store, "cn_1", task_id, "xa_over", "worker", limit=2) is None
    assert capacity.count_active(store, "cn_1") == 2


def test_confirmed_release_frees_capacity_ambiguous_does_not(tmp_path):
    store, task_id = _task(tmp_path)
    r1 = capacity.reserve(store, "cn_2", task_id, "xa_1", "worker", limit=1)
    assert r1 is not None
    assert capacity.reserve(store, "cn_2", task_id, "xa_2", "worker", limit=1) is None

    capacity.mark_ambiguous(store, r1, "timeout_alive")
    # Ambiguous still counts against the limit -- never silently freed.
    assert capacity.count_active(store, "cn_2") == 1
    assert capacity.reserve(store, "cn_2", task_id, "xa_3", "worker", limit=1) is None

    capacity.release_confirmed(store, r1, "process_exited")
    assert capacity.count_active(store, "cn_2") == 0
    r2 = capacity.reserve(store, "cn_2", task_id, "xa_4", "worker", limit=1)
    assert r2 is not None


def test_release_never_deletes_the_reservation_row(tmp_path):
    store, task_id = _task(tmp_path)
    reservation_id = capacity.reserve(store, "cn_3", task_id, "xa_1", "coordinator", limit=4)
    capacity.release_confirmed(store, reservation_id)
    with store.connection() as db:
        row = db.execute(
            "SELECT state, release_observation FROM capacity_reservations WHERE reservation_id=?", (reservation_id,),
        ).fetchone()
    assert row["state"] == "released"
    assert row["release_observation"] == "process_exited"


def test_reserve_rejects_unknown_kind(tmp_path):
    store, task_id = _task(tmp_path)
    try:
        capacity.reserve(store, "cn_4", task_id, "xa_1", "bogus")
    except ValueError:
        return
    raise AssertionError("expected ValueError for an unknown reservation kind")
