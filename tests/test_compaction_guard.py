"""T024: recovery.compaction_guard -- bound compaction frequency, detect
immediate refill, and detect consecutive low recovery."""
from __future__ import annotations

import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.recovery.compaction_guard import (
    MAX_COMPACTIONS_PER_WINDOW,
    MIN_CONSECUTIVE_LOW_RECOVERY,
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
