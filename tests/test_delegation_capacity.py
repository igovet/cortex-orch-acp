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
    # Coordinators are never limited ...
    assert capacity.reserve(store, connection_id, task_id, "co_over", "coordinator", limit=2) is not None
    # ... and the full worker limit is still available.
    workers = [capacity.reserve(store, connection_id, task_id, f"xa_{i}", "worker", limit=2) for i in range(2)]
    assert all(workers)
    assert capacity.reserve(store, connection_id, task_id, "xa_over", "worker", limit=2) is None
    assert capacity.count_active(store, connection_id) == 5
    assert capacity.count_active(store, connection_id, "worker") == 2
    assert capacity.count_active(store, connection_id, "coordinator") == 3

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
    # ambiguous row, and an ambiguous coordinator never blocks another.
    coordinator = capacity.reserve(store, connection_id, task_id, "co_1", "coordinator", limit=1)
    assert coordinator is not None
    capacity.mark_ambiguous(store, coordinator, "timeout_alive")
    assert capacity.reserve(store, connection_id, task_id, "co_2", "coordinator", limit=1) is not None
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


def _another_task(store):
    return bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Another chat.", "owner-1").task_id


def _expect_capacity_timeout(adapter, connection_id, task_id, attempt_id, limit):
    from cortex_runtime.delegation.live_adapters import QueuedOpenCancelled

    try:
        adapter._reserve_capacity_blocking(connection_id, task_id, attempt_id, {"assignment_id": "as_x"}, limit=limit)
    except QueuedOpenCancelled as error:
        assert error.kind == "capacity_timeout"
    else:
        raise AssertionError(f"{attempt_id} must not get a worker slot")


def test_adapter_applies_the_connection_limit_only_within_the_session_cap(tmp_path):
    store, task_id = _task(tmp_path)
    adapter = EngineSessionManagerAdapter(
        lambda _: None, lambda _: str(tmp_path), store=store, capacity_queue_timeout_seconds=0.2,
    )
    try:
        # A coordinator already holds a reservation on every connection.
        for connection_id in ("cn_two", "cn_six", "cn_default", "cn_other"):
            assert capacity.reserve(store, connection_id, task_id, f"co_{connection_id}", "coordinator") is not None

        # A connection limit of 2 lowers that connection's share of the
        # session; the session's remaining slots go to other connections.
        two = capacity.limit_for_connection(_Record(2))
        assert all(_reserve_workers(adapter, "cn_two", task_id, 2, two))
        _expect_capacity_timeout(adapter, "cn_two", task_id, "xa_third", two)
        assert all(_reserve_workers(adapter, "cn_other", task_id, 2, 4))
        _expect_capacity_timeout(adapter, "cn_other", task_id, "xa_fifth_in_session", 4)

        # A connection limit above the session cap never raises it.
        six_task = _another_task(store)
        six = capacity.limit_for_connection(_Record(6))
        assert all(_reserve_workers(adapter, "cn_six", six_task, 4, six))
        _expect_capacity_timeout(adapter, "cn_six", six_task, "xa_fifth", six)
        assert capacity.count_session_workers(store, six_task) == 4

        default_task = _another_task(store)
        default = capacity.limit_for_connection(_Record())
        assert all(_reserve_workers(adapter, "cn_default", default_task, 4, default))
        _expect_capacity_timeout(adapter, "cn_default", default_task, "xa_default_fifth", default)
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


def _queue_behind(adapter, connection_id, task_id, attempt_id):
    outcome = []

    def open_queued():
        try:
            outcome.append(adapter._reserve_capacity_blocking(
                connection_id, task_id, attempt_id, {"assignment_id": "as_queued"}, limit=2,
            ))
        except BaseException as error:
            outcome.append(error)

    thread = threading.Thread(target=open_queued)
    thread.start()
    return thread, outcome


def test_queued_worker_keeps_waiting_while_live_sessions_hold_every_slot(tmp_path, monkeypatch):
    """2026-09-28: seven parallel workers on a four-slot connection; the three
    queued ones failed with capacity_timeout after 600 s while the four live
    siblings were still working. Live holders now extend the wait."""
    store, task_id = _task(tmp_path)
    connection_id = "cn_live_holders"
    holders = [capacity.reserve(store, connection_id, task_id, f"xa_live_{i}", "worker", limit=2) for i in range(2)]
    assert all(holders)
    adapter = EngineSessionManagerAdapter(
        lambda _: None, lambda _: str(tmp_path), store=store, capacity_queue_timeout_seconds=0.1,
    )
    monkeypatch.setattr(adapter, "lookup_owned", lambda attempt_id: "alive" if attempt_id.startswith("xa_live_") else "absent")
    thread, outcome = _queue_behind(adapter, connection_id, task_id, "xa_waiting")
    try:
        time.sleep(0.8)  # eight queue periods
        assert thread.is_alive() and outcome == [], outcome
        capacity.release_confirmed(store, holders[0])
        thread.join(timeout=5)
        assert not thread.is_alive()
        assert len(outcome) == 1 and isinstance(outcome[0], str), outcome
    finally:
        adapter.shutdown()
        thread.join(timeout=5)


def test_queued_worker_still_times_out_behind_a_holder_not_observed_alive(tmp_path, monkeypatch):
    from cortex_runtime.delegation.live_adapters import QueuedOpenCancelled

    store, task_id = _task(tmp_path)
    for connection_id, dead_state in (("cn_absent", "reserved"), ("cn_ambiguous", "ambiguous")):
        live = capacity.reserve(store, connection_id, task_id, f"xa_live_{connection_id}", "worker", limit=2)
        other = capacity.reserve(store, connection_id, task_id, f"xa_other_{connection_id}", "worker", limit=2)
        assert live and other
        if dead_state == "ambiguous":
            capacity.mark_ambiguous(store, other)
        adapter = EngineSessionManagerAdapter(
            lambda _: None, lambda _: str(tmp_path), store=store, capacity_queue_timeout_seconds=0.1,
        )
        # An ambiguous holder may even look alive: it is never waited on.
        monkeypatch.setattr(adapter, "lookup_owned", lambda attempt_id, cid=connection_id, state=dead_state: (
            "alive" if attempt_id.startswith("xa_live_") or state == "ambiguous" else "absent"))
        thread, outcome = _queue_behind(adapter, connection_id, task_id, f"xa_waiting_{connection_id}")
        try:
            thread.join(timeout=5)
            assert not thread.is_alive()
            assert len(outcome) == 1 and isinstance(outcome[0], QueuedOpenCancelled), outcome
            assert outcome[0].kind == "capacity_timeout"
        finally:
            adapter.shutdown()
            thread.join(timeout=5)


# --- Per-session worker cap (owner rule, 2026-09-28): at most four workers of
# any engine/role run at once within one chat session; each session has its
# own four; every terminal path frees a slot exactly once; a slot whose
# holder is observed gone is reclaimed, a live one never is.


def test_one_session_runs_at_most_four_workers_across_two_engines(tmp_path, monkeypatch):
    store, task_id = _task(tmp_path)
    engines = ("cn_codex", "cn_claude")
    held = [capacity.reserve(store, engines[i % 2], task_id, f"xa_run_{i}", "worker") for i in range(4)]
    assert all(held)
    adapter = EngineSessionManagerAdapter(
        lambda _: None, lambda _: str(tmp_path), store=store, capacity_queue_timeout_seconds=5.0,
    )
    monkeypatch.setattr(adapter, "lookup_owned", lambda attempt_id: "alive")
    # Nodes five and six, one per engine: both connections are far below
    # their own limit, yet the session is full.
    fifth = _queue_on(adapter, "cn_codex", task_id, "xa_fifth")
    sixth = _queue_on(adapter, "cn_claude", task_id, "xa_sixth")
    try:
        time.sleep(0.4)
        assert fifth[1] == [] and sixth[1] == []
        assert capacity.count_session_workers(store, task_id) == 4
        capacity.release_confirmed(store, held[0])
        deadline = time.monotonic() + 5
        while len(fifth[1]) + len(sixth[1]) < 1 and time.monotonic() < deadline:
            time.sleep(0.01)
        time.sleep(0.3)
        assert len(fifth[1]) + len(sixth[1]) == 1, (fifth[1], sixth[1])
        assert capacity.count_session_workers(store, task_id) == 4
        capacity.release_confirmed(store, held[1])
        fifth[0].join(timeout=5)
        sixth[0].join(timeout=5)
        assert all(isinstance(outcome[0], str) for outcome in (fifth[1], sixth[1]))
        assert capacity.count_session_workers(store, task_id) == 4
    finally:
        adapter.shutdown()
        fifth[0].join(timeout=5)
        sixth[0].join(timeout=5)


def _queue_on(adapter, connection_id, task_id, attempt_id):
    outcome = []

    def open_queued():
        try:
            outcome.append(adapter._reserve_capacity_blocking(
                connection_id, task_id, attempt_id, {"assignment_id": f"as_{attempt_id}"},
            ))
        except BaseException as error:
            outcome.append(error)

    thread = threading.Thread(target=open_queued)
    thread.start()
    return thread, outcome


def test_each_session_gets_its_own_four_worker_slots(tmp_path):
    store, first = _task(tmp_path)
    second = _another_task(store)
    for task_id in (first, second):
        assert all(capacity.reserve(store, "cn_shared", task_id, f"xa_{task_id}_{i}", "worker") for i in range(4))
    for task_id in (first, second):
        assert capacity.reserve(store, "cn_shared", task_id, f"xa_{task_id}_over", "worker") is None
        assert capacity.reserve(store, "cn_elsewhere", task_id, f"xa_{task_id}_else", "worker") is None
        assert capacity.count_session_workers(store, task_id) == 4
    # Coordinators are not workers: a session's coordinator is unaffected.
    assert capacity.reserve(store, "cn_shared", first, "co_first", "coordinator") is not None


def test_coordinators_of_any_number_of_sessions_are_never_queued(tmp_path):
    """Live (2026-10-04): three accepted tasks whose chats stayed open and
    one running task held all four coordinator slots of a connection, so a
    new chat sat in ``provisioning`` with no visible progress. Coordinators
    have no limit; only workers within one session do."""
    store, first = _task(tmp_path)
    tasks = [first] + [_another_task(store) for _ in range(7)]
    for i, task_id in enumerate(tasks):
        assert capacity.reserve(store, "cn_shared", task_id, f"co_{i}", "coordinator", limit=1) is not None
    assert capacity.count_active(store, "cn_shared", "coordinator") == 8
    # Workers keep their own per-session limit regardless.
    assert capacity.reserve(store, "cn_shared", first, "xa_1", "worker", limit=1) is not None
    assert capacity.reserve(store, "cn_shared", first, "xa_2", "worker", limit=1) is None


def test_concurrent_reserves_in_one_session_never_exceed_four(tmp_path):
    store, task_id = _task(tmp_path)
    barrier = threading.Barrier(12)
    results = []
    lock = threading.Lock()

    def race(i):
        barrier.wait()
        reservation = capacity.reserve(store, ("cn_a", "cn_b", "cn_c")[i % 3], task_id, f"xa_race_{i}", "worker")
        with lock:
            results.append(reservation)

    threads = [threading.Thread(target=race, args=(i,)) for i in range(12)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=30)
    assert len(results) == 12
    assert sum(r is not None for r in results) == 4
    assert capacity.count_session_workers(store, task_id) == 4


class _Process:
    def __init__(self, exited):
        self.exited = exited

    def wait_exited(self, timeout=None):
        return self.exited


class _Manager:
    def __init__(self, exited):
        self._process = _Process(exited)

    def shutdown(self, grace=None):
        pass


def _own(adapter, task_id, attempt_id, reservation_id, *, exited):
    session = f"es_{attempt_id}"
    adapter._managers[attempt_id] = _Manager(exited)
    adapter._sessions[attempt_id] = session
    adapter._session_to_attempt[session] = attempt_id
    adapter._reservations[attempt_id] = reservation_id
    adapter._attempt_task[attempt_id] = task_id


def _row(store, reservation_id):
    with store.connection() as db:
        return dict(db.execute(
            "SELECT state, release_observation FROM capacity_reservations WHERE reservation_id=?", (reservation_id,),
        ).fetchone())


def _record(store, task_id, attempt_id, *, alive):
    import os

    from cortex_runtime.engine import process_identity
    from cortex_runtime.recovery import registry

    # This fixture observes the pytest process, not a private engine session.
    own = process_identity.capture(os.getpid(), os.getpgrp())
    assert own is not None
    registry.record_process_launch(
        store, attempt_id=attempt_id, task_id=task_id, role="worker", pid=own.pid, pgid=own.pgid,
        start_ticks=own.start_ticks, boot_id=own.boot_id if alive else "boot-before-a-restart",
        launch_command_digest="test",
    )


def test_every_terminal_path_frees_its_slot_exactly_once(tmp_path):
    from cortex_runtime.delegation.live_adapters import QueuedOpenCancelled
    from cortex_runtime.engine import reaper
    from cortex_runtime.recovery import worker_reconciliation

    store, task_id = _task(tmp_path)
    adapter = EngineSessionManagerAdapter(
        lambda _: None, lambda _: str(tmp_path), store=store, capacity_queue_timeout_seconds=5.0,
    )
    try:
        # Completion, failure, cancellation and stop all end in
        # terminate_owned: a confirmed exit frees the slot once.
        done = capacity.reserve(store, "cn_1", task_id, "xa_done", "worker")
        _own(adapter, task_id, "xa_done", done, exited=True)
        adapter.terminate_owned("xa_done")
        assert _row(store, done) == dict(state="released", release_observation="process_exited")
        # A later observation of the same exit changes nothing.
        assert capacity.release_confirmed(store, done, "reaper_confirmed_absent") is False
        assert capacity.mark_ambiguous(store, done) is False
        assert _row(store, done) == dict(state="released", release_observation="process_exited")
        adapter.terminate_owned("xa_done")
        assert _row(store, done)["release_observation"] == "process_exited"

        # Provisioning failure before any process started.
        failed = capacity.reserve(store, "cn_1", task_id, "xa_failed_open", "worker")
        adapter._cleanup_failed_open(None, failed)
        assert _row(store, failed) == dict(state="released", release_observation="never_started")

        # Crash/restart recovery: a worker the restarted runtime finds gone.
        lost = capacity.reserve(store, "cn_2", task_id, "xa_lost", "worker")
        worker_reconciliation._release_confirmed_capacity(store, "xa_lost", "recovery_worker_absent")
        assert _row(store, lost)["state"] == "released"

        # Host disconnect: the reaper releases a reservation whose recorded
        # process is gone.
        orphan = capacity.reserve(store, "cn_2", task_id, "xa_orphan", "worker")
        _record(store, task_id, "xa_orphan", alive=False)
        reaper.sweep_capacity(store, adapter)
        assert _row(store, orphan)["state"] == "released"

        # A cancelled queued open never held a slot.
        adapter.cancel_task(task_id)
        try:
            adapter._reserve_capacity_blocking("cn_3", task_id, "xa_queued_cancel", {"assignment_id": "as_q"})
        except QueuedOpenCancelled as error:
            assert error.kind == "task"
        else:
            raise AssertionError("a cancelled task must not reserve")
        assert capacity.count_session_workers(store, task_id) == 0
    finally:
        adapter.shutdown()


def test_stale_ambiguous_slot_of_a_dead_attempt_is_reclaimed_a_live_one_is_not(tmp_path):
    store, task_id = _task(tmp_path)
    adapter = EngineSessionManagerAdapter(
        lambda _: None, lambda _: str(tmp_path), store=store, capacity_queue_timeout_seconds=5.0,
    )
    try:
        dead = capacity.reserve(store, "cn_1", task_id, "xa_dead", "worker")
        live = capacity.reserve(store, "cn_1", task_id, "xa_live", "worker")
        for reservation in (dead, live):
            capacity.mark_ambiguous(store, reservation, "timeout_alive")
        _record(store, task_id, "xa_dead", alive=False)
        _record(store, task_id, "xa_live", alive=True)

        assert adapter.reclaim_stale_capacity(task_id) == 1
        assert _row(store, dead) == dict(state="released", release_observation="reclaimed:process_absent")
        assert _row(store, live)["state"] == "ambiguous"
        with store.connection() as db:
            event = db.execute(
                "SELECT payload_json FROM task_events WHERE task_id=? AND dedup_key=?",
                (task_id, f"capacity_reclaimed:{dead}"),
            ).fetchone()
        import json

        payload = json.loads(event["payload_json"])
        assert payload["kind"] == "capacity_reclaimed"
        assert payload["attempt_id"] == "xa_dead" and payload["prior_state"] == "ambiguous"
        assert payload["evidence"] == "process_absent"
        # Nothing more to reclaim, and no second event.
        assert adapter.reclaim_stale_capacity(task_id) == 0
    finally:
        adapter.shutdown()


def test_owned_worker_whose_engine_exited_is_reclaimed_a_running_one_is_not(tmp_path):
    store, task_id = _task(tmp_path)
    adapter = EngineSessionManagerAdapter(
        lambda _: None, lambda _: str(tmp_path), store=store, capacity_queue_timeout_seconds=5.0,
    )
    try:
        crashed = capacity.reserve(store, "cn_1", task_id, "xa_crashed", "worker")
        running = capacity.reserve(store, "cn_1", task_id, "xa_running", "worker")
        _own(adapter, task_id, "xa_crashed", crashed, exited=True)
        _own(adapter, task_id, "xa_running", running, exited=False)
        assert adapter.reclaim_stale_capacity(task_id) == 1
        assert _row(store, crashed) == dict(state="released", release_observation="reclaimed:owned_process_exited")
        assert _row(store, running)["state"] == "reserved"
        # The later terminal path does not release it a second time.
        adapter.terminate_owned("xa_crashed")
        assert _row(store, crashed)["release_observation"] == "reclaimed:owned_process_exited"
    finally:
        adapter._sessions.pop("xa_running", None)
        adapter._managers.pop("xa_running", None)
        adapter.shutdown()


def test_unrecorded_reservation_is_reclaimed_only_after_its_open_bound(tmp_path):
    store, task_id = _task(tmp_path)
    adapter = EngineSessionManagerAdapter(
        lambda _: None, lambda _: str(tmp_path), store=store, capacity_queue_timeout_seconds=5.0,
    )
    try:
        young = capacity.reserve(store, "cn_1", task_id, "xa_young", "worker")
        opening = capacity.reserve(store, "cn_1", task_id, "xa_opening", "worker")
        stale = capacity.reserve(store, "cn_1", task_id, "xa_stale", "worker")
        with store.connection() as db:
            db.execute(
                "UPDATE capacity_reservations SET reserved_at='2020-01-01T00:00:00.000000+00:00' "
                "WHERE reservation_id IN (?,?)", (opening, stale),
            )
            db.commit()
        adapter._opening.add("xa_opening")
        assert adapter.reclaim_stale_capacity(task_id) == 1
        assert _row(store, stale) == dict(
            state="released", release_observation="reclaimed:unrecorded_reservation_expired",
        )
        assert _row(store, young)["state"] == "reserved"
        assert _row(store, opening)["state"] == "reserved"
    finally:
        adapter.shutdown()


def test_queued_worker_reclaims_a_dead_holders_slot_instead_of_hanging(tmp_path):
    store, task_id = _task(tmp_path)
    holders = [capacity.reserve(store, "cn_1", task_id, f"xa_holder_{i}", "worker") for i in range(4)]
    assert all(holders)
    _record(store, task_id, "xa_holder_0", alive=False)
    for i in range(1, 4):
        _record(store, task_id, f"xa_holder_{i}", alive=True)
    adapter = EngineSessionManagerAdapter(
        lambda _: None, lambda _: str(tmp_path), store=store, capacity_queue_timeout_seconds=5.0,
    )
    try:
        reservation = adapter._reserve_capacity_blocking("cn_2", task_id, "xa_next", {"assignment_id": "as_next"})
        assert isinstance(reservation, str)
        assert _row(store, holders[0])["release_observation"] == "reclaimed:process_absent"
        assert capacity.count_session_workers(store, task_id) == 4
    finally:
        adapter.shutdown()
