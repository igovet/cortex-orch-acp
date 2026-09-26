"""T017: delegation.capacity -- reserve/release/ambiguous concurrency
accounting, and history retention across release."""
from __future__ import annotations

import threading
import time
import uuid

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


def test_coordinator_reservations_never_consume_worker_capacity(tmp_path):
    store, task_id = _task(tmp_path)
    connection_id = "cn_split"
    coordinators = [
        capacity.reserve(store, connection_id, task_id, f"co_{i}", "coordinator", limit=2) for i in range(2)
    ]
    assert all(coordinators)
    # Coordinators are at their own limit ...
    assert capacity.reserve(store, connection_id, task_id, "co_over", "coordinator", limit=2) is None
    # ... yet the full worker limit is still available.
    workers = [capacity.reserve(store, connection_id, task_id, f"xa_{i}", "worker", limit=2) for i in range(2)]
    assert all(workers)
    assert capacity.reserve(store, connection_id, task_id, "xa_over", "worker", limit=2) is None
    assert capacity.count_active(store, connection_id) == 4
    assert capacity.count_active(store, connection_id, "worker") == 2
    assert capacity.count_active(store, connection_id, "coordinator") == 2

    # Releasing a coordinator never frees a worker slot.
    capacity.release_confirmed(store, coordinators[0])
    assert capacity.reserve(store, connection_id, task_id, "xa_still_over", "worker", limit=2) is None
    capacity.release_confirmed(store, workers[0])
    assert capacity.reserve(store, connection_id, task_id, "xa_next", "worker", limit=2) is not None


def test_ambiguous_worker_still_counts_against_worker_limit(tmp_path):
    store, task_id = _task(tmp_path)
    connection_id = "cn_ambiguous"
    worker = capacity.reserve(store, connection_id, task_id, "xa_1", "worker", limit=1)
    assert worker is not None
    capacity.mark_ambiguous(store, worker, "timeout_alive")
    assert capacity.reserve(store, connection_id, task_id, "xa_2", "worker", limit=1) is None
    # A coordinator on the same connection is unaffected by the worker's
    # ambiguous row, and an ambiguous coordinator keeps its own slot.
    coordinator = capacity.reserve(store, connection_id, task_id, "co_1", "coordinator", limit=1)
    assert coordinator is not None
    capacity.mark_ambiguous(store, coordinator, "timeout_alive")
    assert capacity.reserve(store, connection_id, task_id, "co_2", "coordinator", limit=1) is None
    assert capacity.count_active(store, connection_id, "worker") == 1


class _Record:
    def __init__(self, limit=None):
        if limit is not None:
            self.max_concurrent_sessions = limit


def test_limit_for_connection_uses_the_record_default_four_and_override_only_lowers():
    assert capacity.limit_for_connection(_Record(2)) == 2
    assert capacity.limit_for_connection(_Record(6)) == 6
    assert capacity.limit_for_connection(_Record()) == capacity.DEFAULT_MAX_CONCURRENT_SESSIONS == 4
    assert capacity.limit_for_connection(None) == 4
    assert capacity.limit_for_connection(_Record(True)) == 4
    assert capacity.limit_for_connection(_Record(6), 2) == 2
    assert capacity.limit_for_connection(_Record(2), 6) == 2
    assert capacity.limit_for_connection(None, 1) == 1


def _reserve_workers(adapter, connection_id, task_id, count, limit):
    return [
        adapter._reserve_capacity_blocking(
            connection_id, task_id, f"xa_{connection_id}_{i}", {"assignment_id": f"as_{i}"}, limit=limit,
        )
        for i in range(count)
    ]


def test_adapter_enforces_each_connections_own_limit(tmp_path):
    from cortex_runtime.delegation.live_adapters import QueuedOpenCancelled

    store, task_id = _task(tmp_path)
    adapter = EngineSessionManagerAdapter(
        lambda _: None, lambda _: str(tmp_path), store=store, capacity_queue_timeout_seconds=0.2,
    )
    try:
        # A coordinator already holds a reservation on every connection.
        for connection_id in ("cn_two", "cn_six", "cn_default"):
            assert capacity.reserve(store, connection_id, task_id, f"co_{connection_id}", "coordinator") is not None

        two = capacity.limit_for_connection(_Record(2))
        assert all(_reserve_workers(adapter, "cn_two", task_id, 2, two))
        try:
            adapter._reserve_capacity_blocking("cn_two", task_id, "xa_third", {"assignment_id": "as_x"}, limit=two)
        except QueuedOpenCancelled as error:
            assert error.kind == "capacity_timeout"
        else:
            raise AssertionError("a third worker must not exceed the connection's limit of 2")

        six = capacity.limit_for_connection(_Record(6))
        assert all(_reserve_workers(adapter, "cn_six", task_id, 6, six))
        assert capacity.count_active(store, "cn_six", "worker") == 6

        default = capacity.limit_for_connection(_Record())
        assert all(_reserve_workers(adapter, "cn_default", task_id, 4, default))
        try:
            adapter._reserve_capacity_blocking("cn_default", task_id, "xa_fifth", {"assignment_id": "as_y"}, limit=default)
        except QueuedOpenCancelled as error:
            assert error.kind == "capacity_timeout"
        else:
            raise AssertionError("a record without max_concurrent_sessions keeps the default of 4")
    finally:
        adapter.shutdown()


def test_queued_worker_proceeds_once_a_worker_is_released(tmp_path, monkeypatch):
    store, task_id = _task(tmp_path)
    connection_id = "cn_release_race"
    coordinator = capacity.reserve(store, connection_id, task_id, "co_1", "coordinator", limit=2)
    workers = [capacity.reserve(store, connection_id, task_id, f"xa_{i}", "worker", limit=2) for i in range(2)]
    assert coordinator is not None and all(workers)

    adapter = EngineSessionManagerAdapter(
        lambda _: None, lambda _: str(tmp_path), store=store, capacity_limit=2,
        capacity_queue_timeout_seconds=5.0,
    )
    queued = threading.Event()
    reserve = capacity.reserve

    def observe_failed_reserve(*args, **kwargs):
        result = reserve(*args, **kwargs)
        if args[3] == "xa_queued" and result is None:
            queued.set()
        return result

    monkeypatch.setattr(capacity, "reserve", observe_failed_reserve)
    outcome = []

    def open_queued():
        try:
            outcome.append(adapter._reserve_capacity_blocking(
                connection_id, task_id, "xa_queued", {"assignment_id": "as_2"},
            ))
        except BaseException as error:
            outcome.append(error)

    thread = threading.Thread(target=open_queued)
    thread.start()
    try:
        assert queued.wait(timeout=5)
        # The queued note is appended right after the refused reserve.
        deadline = time.monotonic() + 5
        note = None
        while note is None and time.monotonic() < deadline:
            with store.connection() as db:
                note = db.execute(
                    "SELECT 1 FROM task_events WHERE task_id=? AND dedup_key='capacity_queued:xa_queued'", (task_id,),
                ).fetchone()
            if note is None:
                time.sleep(0.01)
        assert note is not None
        capacity.release_confirmed(store, workers[0])
        thread.join(timeout=5)
        assert not thread.is_alive()
        assert len(outcome) == 1 and isinstance(outcome[0], str), outcome
    finally:
        adapter.shutdown()
        thread.join(timeout=5)
        if outcome and isinstance(outcome[0], str):
            capacity.release_confirmed(store, outcome[0])
        capacity.release_confirmed(store, workers[1])
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
