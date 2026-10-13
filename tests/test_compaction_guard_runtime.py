"""recovery.compaction_guard's frequency window through OrchestratingRuntime:
spaced observed compactions keep the task running, a burst records the
durable ``limit:compaction_frequency_exceeded`` blocked state."""
from __future__ import annotations

from datetime import datetime, timedelta, timezone
import json
import threading

from cortex_runtime.acp.orchestrating_runtime import PromptOutcome
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.recovery import compaction_guard

from test_context_generations import _make_runtime, _orchestration_row, _task_with_store

_T0 = datetime(2026, 10, 13, 8, 0, 0, tzinfo=timezone.utc)


def _run_compactions(tmp_path, monkeypatch, spacing, attempts):
    moments = iter(_T0 + index * spacing for index in range(attempts))
    monkeypatch.setattr(compaction_guard, "_attempt_time", lambda store: next(moments).isoformat(timespec="microseconds"))
    seen_messages = []

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        seen_messages.append(message)
        return PromptOutcome(StopReason.END_TURN, "ok")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel is not None and channel.role == "coordinator" else None

    store, task_id = _task_with_store(tmp_path)
    runtime, engine = _make_runtime(store, router)
    cancel_event = threading.Event()
    runtime.start_prompt(task_id, "s1", "Build a page.", lambda text: True, cancel_event)
    session_ref = runtime._coordinator_sessions[task_id]
    states = []
    for index in range(attempts):
        engine.trigger_compaction(session_ref, f"tc-{index}", phase="completed")
        runtime.start_prompt(task_id, "s1", f"still building {index}", lambda text: True, cancel_event)
        states.append(_orchestration_row(store, task_id))
        if states[-1]["state"] == "blocked":
            break
    with store.connection() as db:
        rows = db.execute(
            "SELECT status, reason FROM compaction_recovery_events WHERE engine_session_ref=? ORDER BY created_at",
            (session_ref,),
        ).fetchall()
    return states, [tuple(row) for row in rows], seen_messages


def test_spaced_observed_compactions_never_block_the_task(tmp_path, monkeypatch):
    attempts = 3 * compaction_guard.MAX_COMPACTIONS_PER_WINDOW
    states, rows, _ = _run_compactions(
        tmp_path, monkeypatch, timedelta(seconds=compaction_guard.WINDOW_SECONDS), attempts,
    )
    assert [state["state"] for state in states].count("blocked") == 0
    assert len(rows) == attempts
    assert {row[0] for row in rows} == {"ok"}


def test_burst_of_observed_compactions_records_the_frequency_limit(tmp_path, monkeypatch):
    limit = compaction_guard.MAX_COMPACTIONS_PER_WINDOW
    states, rows, seen_messages = _run_compactions(tmp_path, monkeypatch, timedelta(seconds=30), limit + 3)
    final = states[-1]
    assert final["state"] == "blocked"
    assert final["wait_reason"] == "limit:compaction_frequency_exceeded"
    explanation = json.loads(final["wait_detail_json"])["explanation"]
    assert "compaction_frequency_exceeded" in explanation
    assert f"last {compaction_guard.WINDOW_SECONDS // 60} minutes" in explanation
    assert len(states) == limit + 1
    assert rows == [("ok", None)] * limit + [("blocked", "compaction_frequency_exceeded")]
    # The blocked generation was never sent to the engine: one initial
    # prompt plus the allowed compactions' continuations.
    assert len(seen_messages) == 1 + limit
