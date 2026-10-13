"""T024: recovery.compaction_guard -- bound compaction frequency, detect
immediate refill, and detect consecutive low recovery."""
from __future__ import annotations

from datetime import datetime, timedelta, timezone
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.recovery import compaction_guard
from cortex_runtime.recovery.compaction_guard import (
    MAX_COMPACTIONS_PER_WINDOW,
    MIN_CONSECUTIVE_LOW_RECOVERY,
    UNMEASURED_BASIS,
    WINDOW_SECONDS,
    evaluate_compaction,
)
from cortex_runtime.store import Store


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return store, result.task_id


def test_healthy_compaction_recovers_space_and_is_ok(tmp_path):
    store, task_id = _task(tmp_path)
    result = evaluate_compaction(
        store, task_id=task_id, engine_session_ref="es_1", generation_id=None,
        before_tokens=10_000, before_basis="estimated", after_tokens=2_000, after_basis="estimated",
        mandatory_tokens_after=3_000, usable_window_tokens=28_800,
    )
    assert result["status"] == "ok"
    assert result["recovered_tokens"] == 8_000


def test_compaction_frequency_exceeded_blocks(tmp_path):
    store, task_id = _task(tmp_path)
    last = None
    for _ in range(MAX_COMPACTIONS_PER_WINDOW + 1):
        last = evaluate_compaction(
            store, task_id=task_id, engine_session_ref="es_freq", generation_id=None,
            before_tokens=10_000, before_basis="estimated", after_tokens=2_000, after_basis="estimated",
            mandatory_tokens_after=3_000, usable_window_tokens=28_800,
        )
    assert last["status"] == "blocked"
    assert last["reason"] == "compaction_frequency_exceeded"
    assert "message" in last


def test_immediate_refill_blocks_without_looping(tmp_path):
    store, task_id = _task(tmp_path)
    result = evaluate_compaction(
        store, task_id=task_id, engine_session_ref="es_refill", generation_id=None,
        before_tokens=10_000, before_basis="estimated", after_tokens=2_000, after_basis="estimated",
        mandatory_tokens_after=30_000, usable_window_tokens=28_800,
    )
    assert result["status"] == "blocked"
    assert result["reason"] == "immediate_refill"


def test_consecutive_low_recovery_blocks(tmp_path):
    store, task_id = _task(tmp_path)
    last = None
    for _ in range(MIN_CONSECUTIVE_LOW_RECOVERY):
        last = evaluate_compaction(
            store, task_id=task_id, engine_session_ref="es_low", generation_id=None,
            before_tokens=10_000, before_basis="estimated", after_tokens=9_500, after_basis="estimated",
            mandatory_tokens_after=3_000, usable_window_tokens=28_800,
        )
    assert last["status"] == "blocked"
    assert last["reason"] == "consecutive_low_recovery"


def test_good_recovery_breaks_a_low_recovery_streak(tmp_path):
    store, task_id = _task(tmp_path)
    for _ in range(MIN_CONSECUTIVE_LOW_RECOVERY - 1):
        evaluate_compaction(
            store, task_id=task_id, engine_session_ref="es_mixed", generation_id=None,
            before_tokens=10_000, before_basis="estimated", after_tokens=9_500, after_basis="estimated",
            mandatory_tokens_after=3_000, usable_window_tokens=28_800,
        )
    result = evaluate_compaction(
        store, task_id=task_id, engine_session_ref="es_mixed", generation_id=None,
        before_tokens=10_000, before_basis="estimated", after_tokens=1_000, after_basis="estimated",
        mandatory_tokens_after=3_000, usable_window_tokens=28_800,
    )
    assert result["status"] == "ok"


def test_every_attempt_is_recorded_even_when_blocked(tmp_path):
    store, task_id = _task(tmp_path)
    for _ in range(MAX_COMPACTIONS_PER_WINDOW + 2):
        evaluate_compaction(
            store, task_id=task_id, engine_session_ref="es_recorded", generation_id=None,
            before_tokens=10_000, before_basis="estimated", after_tokens=2_000, after_basis="estimated",
            mandatory_tokens_after=3_000, usable_window_tokens=28_800,
        )
    with store.connection() as db:
        count = db.execute(
            "SELECT COUNT(*) FROM compaction_recovery_events WHERE engine_session_ref='es_recorded'"
        ).fetchone()[0]
    assert count == MAX_COMPACTIONS_PER_WINDOW + 2


def test_unmeasured_compactions_never_block_as_low_recovery(tmp_path):
    """No engine-reported pre-compaction size is an absent signal, not a
    zero recovery: repeated unmeasured attempts stay ok until the
    frequency bound, and only that bound stops them."""
    store, task_id = _task(tmp_path)
    results = [
        evaluate_compaction(
            store, task_id=task_id, engine_session_ref="es_unmeasured", generation_id=None,
            before_tokens=0, before_basis=UNMEASURED_BASIS, after_tokens=45_000, after_basis="estimated",
            mandatory_tokens_after=45_000, usable_window_tokens=200_000,
        )
        for _ in range(MAX_COMPACTIONS_PER_WINDOW + 1)
    ]
    assert [r["status"] for r in results[:MAX_COMPACTIONS_PER_WINDOW]] == ["ok"] * MAX_COMPACTIONS_PER_WINDOW
    assert all(r["recovered_tokens"] == 0 for r in results)
    assert results[-1]["reason"] == "compaction_frequency_exceeded"
    with store.connection() as db:
        bases = {row[0] for row in db.execute(
            "SELECT before_basis FROM compaction_recovery_events WHERE engine_session_ref='es_unmeasured'")}
    assert bases == {UNMEASURED_BASIS}


def test_unmeasured_attempts_are_not_part_of_a_low_recovery_streak(tmp_path):
    store, task_id = _task(tmp_path)

    def attempt(basis, before):
        return evaluate_compaction(
            store, task_id=task_id, engine_session_ref="es_gap", generation_id=None,
            before_tokens=before, before_basis=basis, after_tokens=9_500, after_basis="estimated",
            mandatory_tokens_after=3_000, usable_window_tokens=28_800,
        )

    attempt(UNMEASURED_BASIS, 0)
    attempt(UNMEASURED_BASIS, 0)
    assert attempt("engine_reported", 10_000)["status"] == "ok"
    assert attempt("engine_reported", 10_000)["status"] == "ok"
    blocked = attempt("engine_reported", 10_000)
    assert blocked["status"] == "blocked"
    assert blocked["reason"] == "consecutive_low_recovery"


# -- frequency window: wall-clock bounded, not lifetime --------------------

_T0 = datetime(2026, 10, 13, 8, 0, 0, tzinfo=timezone.utc)
_WINDOW = timedelta(seconds=WINDOW_SECONDS)


@pytest.fixture
def clock(monkeypatch):
    """Pin the attempt timestamp the guard both decides on and records."""
    state = {"now": _T0}
    monkeypatch.setattr(compaction_guard, "_attempt_time", lambda store: state["now"].isoformat(timespec="microseconds"))

    def set_now(moment):
        state["now"] = moment

    return set_now


def _attempt(store, task_id, session="es_clock", *, basis="engine_reported", before=10_000, after=2_000,
             mandatory=3_000, usable=28_800):
    return evaluate_compaction(
        store, task_id=task_id, engine_session_ref=session, generation_id=None,
        before_tokens=before, before_basis=basis, after_tokens=after, after_basis="estimated",
        mandatory_tokens_after=mandatory, usable_window_tokens=usable,
    )


def _insert_row(store, task_id, session, created_at, *, row_id=None, basis="engine_reported", ratio=0.8):
    with store.connection() as db:
        db.execute(
            "INSERT INTO compaction_recovery_events (id, task_id, engine_session_ref, generation_id, before_tokens, "
            "before_basis, after_tokens, after_basis, recovered_tokens, recovered_ratio, status, reason, created_at) "
            "VALUES (?,?,?,NULL,10000,?,2000,'estimated',8000,?,'ok',NULL,?)",
            (row_id or f"cre_{uuid.uuid4().hex[:12]}", task_id, session, basis, ratio, created_at),
        )
        db.commit()


def _row_count(store, session):
    with store.connection() as db:
        return db.execute(
            "SELECT COUNT(*) FROM compaction_recovery_events WHERE engine_session_ref=?", (session,),
        ).fetchone()[0]


def test_more_than_five_spaced_healthy_compactions_continue(tmp_path, clock):
    """The reported false stop: the sixth compaction of a session was
    blocked however far apart the compactions were."""
    store, task_id = _task(tmp_path)
    statuses = []
    for index in range(3 * MAX_COMPACTIONS_PER_WINDOW):
        clock(_T0 + index * _WINDOW)
        statuses.append(_attempt(store, task_id)["status"])
    assert statuses == ["ok"] * (3 * MAX_COMPACTIONS_PER_WINDOW)


def test_steady_cadence_at_the_limit_continues_and_just_faster_blocks(tmp_path, clock):
    """One compaction every WINDOW/MAX keeps at most MAX-1 prior attempts
    inside the window indefinitely; one microsecond faster puts MAX prior
    attempts inside it."""
    store, task_id = _task(tmp_path)
    cadence = _WINDOW / MAX_COMPACTIONS_PER_WINDOW
    for index in range(4 * MAX_COMPACTIONS_PER_WINDOW):
        clock(_T0 + index * cadence)
        assert _attempt(store, task_id, "es_cadence")["status"] == "ok"
    faster = cadence - timedelta(microseconds=1)
    statuses = []
    for index in range(MAX_COMPACTIONS_PER_WINDOW + 1):
        clock(_T0 + timedelta(days=1) + index * faster)
        statuses.append(_attempt(store, task_id, "es_faster")["status"])
    assert statuses == ["ok"] * MAX_COMPACTIONS_PER_WINDOW + ["blocked"]


def test_spaced_unmeasured_compactions_also_continue(tmp_path, clock):
    store, task_id = _task(tmp_path)
    for index in range(2 * MAX_COMPACTIONS_PER_WINDOW):
        clock(_T0 + index * timedelta(hours=1))
        result = _attempt(store, task_id, basis=UNMEASURED_BASIS, before=0, after=45_000, mandatory=45_000, usable=200_000)
        assert result["status"] == "ok"


def test_recent_burst_still_blocks_with_window_in_message(tmp_path, clock):
    store, task_id = _task(tmp_path)
    results = []
    for index in range(MAX_COMPACTIONS_PER_WINDOW + 1):
        clock(_T0 + index * timedelta(minutes=1))
        results.append(_attempt(store, task_id))
    assert [r["status"] for r in results] == ["ok"] * MAX_COMPACTIONS_PER_WINDOW + ["blocked"]
    assert results[-1]["reason"] == "compaction_frequency_exceeded"
    assert f"{MAX_COMPACTIONS_PER_WINDOW} compactions" in results[-1]["message"]
    assert f"last {WINDOW_SECONDS // 60} minutes" in results[-1]["message"]


@pytest.mark.parametrize(
    ("offset", "expected"),
    [
        (_WINDOW - timedelta(microseconds=1), "blocked"),  # oldest prior row still inside
        (_WINDOW, "ok"),  # oldest prior row exactly one window old has aged out
    ],
)
def test_window_edge_is_exclusive_and_deterministic(tmp_path, clock, offset, expected):
    store, task_id = _task(tmp_path)
    for index in range(MAX_COMPACTIONS_PER_WINDOW):
        clock(_T0 + index * timedelta(seconds=1))
        assert _attempt(store, task_id)["status"] == "ok"
    clock(_T0 + offset)
    result = _attempt(store, task_id)
    assert result["status"] == expected
    if expected == "blocked":
        assert result["reason"] == "compaction_frequency_exceeded"


def test_limit_minus_one_recent_attempts_is_ok(tmp_path, clock):
    store, task_id = _task(tmp_path)
    for index in range(MAX_COMPACTIONS_PER_WINDOW - 1):
        clock(_T0 + index * timedelta(seconds=1))
        _attempt(store, task_id)
    clock(_T0 + timedelta(minutes=1))
    assert _attempt(store, task_id)["status"] == "ok"


def test_old_rows_and_other_sessions_do_not_poison_the_window(tmp_path, clock):
    store, task_id = _task(tmp_path)
    for index in range(4 * MAX_COMPACTIONS_PER_WINDOW):
        _insert_row(store, task_id, "es_target", (_T0 - timedelta(days=1) + index * timedelta(seconds=1)).isoformat())
    for index in range(4 * MAX_COMPACTIONS_PER_WINDOW):
        _insert_row(store, task_id, "es_other", (_T0 - index * timedelta(seconds=1)).isoformat())
    # A row far in the future (written before a large clock correction) and
    # an unreadable timestamp are not evidence of a recent compaction.
    _insert_row(store, task_id, "es_target", (_T0 + timedelta(days=2)).isoformat())
    _insert_row(store, task_id, "es_target", "not-a-timestamp")
    clock(_T0)
    assert _attempt(store, task_id, "es_target")["status"] == "ok"
    # The other session's own burst still blocks it.
    blocked = _attempt(store, task_id, "es_other")
    assert blocked["reason"] == "compaction_frequency_exceeded"


def test_small_backward_clock_step_still_counts_recent_rows(tmp_path, clock):
    store, task_id = _task(tmp_path)
    for index in range(MAX_COMPACTIONS_PER_WINDOW):
        _insert_row(store, task_id, "es_skew", (_T0 + timedelta(seconds=1 + index)).isoformat())
    clock(_T0)
    assert _attempt(store, task_id, "es_skew")["reason"] == "compaction_frequency_exceeded"


def test_blocked_attempts_are_recorded_and_age_out(tmp_path, clock):
    store, task_id = _task(tmp_path)
    for index in range(MAX_COMPACTIONS_PER_WINDOW + 2):
        clock(_T0 + index * timedelta(seconds=10))
        _attempt(store, task_id)
    assert _row_count(store, "es_clock") == MAX_COMPACTIONS_PER_WINDOW + 2
    clock(_T0 + timedelta(seconds=10) * (MAX_COMPACTIONS_PER_WINDOW + 1) + _WINDOW)
    assert _attempt(store, task_id)["status"] == "ok"


def test_window_survives_a_store_reopen(tmp_path, clock):
    """The window comes from persisted created_at values, so a Cortex
    restart neither resets a real burst nor invents one."""
    store, task_id = _task(tmp_path)
    for index in range(MAX_COMPACTIONS_PER_WINDOW):
        clock(_T0 + index * timedelta(seconds=1))
        _attempt(store, task_id)
    reopened = Store(tmp_path / ".cortex", project_root=tmp_path)
    clock(_T0 + timedelta(minutes=1))
    assert _attempt(reopened, task_id)["reason"] == "compaction_frequency_exceeded"
    clock(_T0 + timedelta(minutes=1) + _WINDOW)
    assert _attempt(reopened, task_id)["status"] == "ok"


def test_unreadable_attempt_time_records_nothing(tmp_path, monkeypatch):
    store, task_id = _task(tmp_path)
    monkeypatch.setattr(compaction_guard, "_attempt_time", lambda store: "not-a-timestamp")
    with pytest.raises(ValueError):
        _attempt(store, task_id, "es_badclock")
    assert _row_count(store, "es_badclock") == 0


# -- adjacent guards keep their behavior with a spaced history -------------


def test_low_recovery_streak_uses_chronology_not_random_row_ids(tmp_path, clock):
    """Row ids are random; the newest measured attempt here is a good one
    whose id sorts lowest, while older low-recovery rows sort highest."""
    store, task_id = _task(tmp_path)
    _insert_row(store, task_id, "es_order", (_T0 - timedelta(hours=3)).isoformat(), row_id="cre_fffffffffff1", ratio=0.05)
    _insert_row(store, task_id, "es_order", (_T0 - timedelta(hours=2)).isoformat(), row_id="cre_fffffffffff2", ratio=0.05)
    _insert_row(store, task_id, "es_order", (_T0 - timedelta(hours=1)).isoformat(), row_id="cre_000000000000", ratio=0.8)
    clock(_T0)
    assert _attempt(store, task_id, "es_order", after=9_500)["status"] == "ok"


def test_low_recovery_streak_blocks_even_when_spaced(tmp_path, clock):
    store, task_id = _task(tmp_path)
    results = []
    for index in range(MIN_CONSECUTIVE_LOW_RECOVERY):
        clock(_T0 + index * timedelta(hours=1))
        results.append(_attempt(store, task_id, "es_slow_low", after=9_500))
    assert [r["status"] for r in results[:-1]] == ["ok"] * (MIN_CONSECUTIVE_LOW_RECOVERY - 1)
    assert results[-1]["reason"] == "consecutive_low_recovery"


def test_immediate_refill_blocks_on_a_spaced_history(tmp_path, clock):
    store, task_id = _task(tmp_path)
    for index in range(2 * MAX_COMPACTIONS_PER_WINDOW):
        clock(_T0 + index * _WINDOW)
        _attempt(store, task_id, "es_spaced_refill")
    clock(_T0 + 2 * MAX_COMPACTIONS_PER_WINDOW * _WINDOW)
    result = _attempt(store, task_id, "es_spaced_refill", mandatory=30_000)
    assert result["reason"] == "immediate_refill"
