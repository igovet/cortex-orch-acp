"""T017: delegation.capacity -- reserve/release/ambiguous concurrency
accounting, and history retention across release."""
from __future__ import annotations

import uuid
import threading

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import capacity
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
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


def test_coordinator_saturation_uses_one_current_capacity_snapshot(tmp_path):
    store, task_id = _task(tmp_path)
    coordinator = capacity.reserve(store, "cn_snapshot", task_id, "co_1", "coordinator", limit=2)
    worker = capacity.reserve(store, "cn_snapshot", task_id, "xa_1", "worker", limit=2)
    assert coordinator is not None and worker is not None
    assert not capacity.saturated_by_coordinators(store, "cn_snapshot", 2)

    capacity.mark_ambiguous(store, worker)
    assert not capacity.saturated_by_coordinators(store, "cn_snapshot", 2)
    capacity.release_confirmed(store, worker)
    # A stale count of two followed by a separate kinds query used to
    # misclassify this exact post-release state as unsatisfiable.
    assert not capacity.saturated_by_coordinators(store, "cn_snapshot", 2)

    second_coordinator = capacity.reserve(store, "cn_snapshot", task_id, "co_2", "coordinator", limit=2)
    assert second_coordinator is not None
    assert capacity.saturated_by_coordinators(store, "cn_snapshot", 2)


def test_queued_worker_rechecks_capacity_without_split_saturation_reads(tmp_path, monkeypatch):
    store, task_id = _task(tmp_path)
    connection_id = "cn_release_race"
    coordinator = capacity.reserve(store, connection_id, task_id, "co_1", "coordinator", limit=2)
    worker = capacity.reserve(store, connection_id, task_id, "xa_1", "worker", limit=2)
    assert coordinator is not None and worker is not None

    adapter = EngineSessionManagerAdapter(
        lambda _: None, lambda _: str(tmp_path), store=store, capacity_limit=2,
        capacity_queue_timeout_seconds=5.0,
    )
    queued = threading.Event()
    reserve = capacity.reserve

    def observe_failed_reserve(*args, **kwargs):
        result = reserve(*args, **kwargs)
        if args[3] == "xa_2" and result is None:
            queued.set()
        return result

    def forbid_split_read(*args, **kwargs):
        raise AssertionError("saturation must use one capacity snapshot")

    monkeypatch.setattr(capacity, "reserve", observe_failed_reserve)
    monkeypatch.setattr(capacity, "count_active", forbid_split_read)
    monkeypatch.setattr(capacity, "all_active_are_kind", forbid_split_read, raising=False)
    outcome = []

    def open_queued():
        try:
            outcome.append(adapter._reserve_capacity_blocking(
                connection_id, task_id, "xa_2", {"assignment_id": "as_2"},
            ))
        except BaseException as error:
            outcome.append(error)

    thread = threading.Thread(target=open_queued)
    thread.start()
    try:
        assert queued.wait(timeout=5)
        capacity.release_confirmed(store, worker)
        thread.join(timeout=5)
        assert not thread.is_alive()
        assert len(outcome) == 1 and isinstance(outcome[0], str), outcome
    finally:
        adapter.shutdown()
        thread.join(timeout=5)
        if outcome and isinstance(outcome[0], str):
            capacity.release_confirmed(store, outcome[0])
        capacity.release_confirmed(store, coordinator)


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
