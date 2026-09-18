"""T029: usage/cost attribution -- observability.usage and the
delegation.live_adapters.EngineSessionManagerAdapter.prompt() wiring that
feeds it.

Every ``Store`` is rooted at ``tmp_path`` (never the real checkout); no
engine process or network I/O -- ``prompt()`` is exercised against a fake
in-process ``manager`` whose ``prompt_turn`` yields portable
``EngineEvent``s directly.
"""
from __future__ import annotations

import threading
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.observability import usage as observability_usage
from cortex_runtime.portable.engine_contract import EngineEvent, EngineEventKind, StopReason, UsageBasis
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return store, result.task_id


class _FakeManager:
    def __init__(self, events):
        self._events = list(events)

    def prompt_turn(self, session_ref, message):
        for event in self._events:
            yield event


def _adapter(store):
    """A bare ``EngineSessionManagerAdapter`` with only the private state
    ``prompt()`` actually reads -- avoids the heavy real constructor
    (connection factories, workspace roots, ...) this unit test needs
    none of."""
    adapter = object.__new__(EngineSessionManagerAdapter)
    adapter._lock = threading.Lock()
    adapter._managers = {}
    adapter._session_to_attempt = {}
    adapter._attempt_task = {}
    adapter._store = store
    adapter._pending_generation = {}
    return adapter


def _register(adapter, task_id, attempt_id, session_ref, manager):
    adapter._managers[attempt_id] = manager
    adapter._session_to_attempt[session_ref] = attempt_id
    adapter._attempt_task[attempt_id] = task_id


def test_prompt_records_engine_reported_usage(tmp_path):
    store, task_id = _task(tmp_path)
    adapter = _adapter(store)
    manager = _FakeManager([
        EngineEvent(kind=EngineEventKind.TURN_DELTA, delta_text="hi"),
        EngineEvent(
            kind=EngineEventKind.USAGE_UPDATE, usage_basis=UsageBasis.ENGINE_REPORTED,
            usage_values=dict(input_tokens=10, output_tokens=5),
        ),
        EngineEvent(kind=EngineEventKind.TURN_END, stop_reason=StopReason.END_TURN),
    ])
    _register(adapter, task_id, "att-1", "sess-1", manager)
    outcome = adapter.prompt("sess-1", "hello", threading.Event())
    assert outcome.text == "hi"

    records = observability_usage.list_usage(store, task_id)
    assert len(records) == 1
    assert records[0]["basis"] == "engine_reported"
    assert records[0]["tokens"] == dict(input_tokens=10, output_tokens=5)
    assert records[0]["failed"] is False
    assert records[0]["call_sequence"] == 1


def test_missing_telemetry_is_unknown_never_zero_and_never_gates_execution(tmp_path):
    """An engine that reports no usage_update at all still lets the turn
    complete, and the recorded basis is 'unknown', never '0'/free (T029
    acceptance: absent optional telemetry never gates execution)."""
    store, task_id = _task(tmp_path)
    adapter = _adapter(store)
    manager = _FakeManager([
        EngineEvent(kind=EngineEventKind.TURN_DELTA, delta_text="ok"),
        EngineEvent(kind=EngineEventKind.TURN_END, stop_reason=StopReason.END_TURN),
    ])
    _register(adapter, task_id, "att-2", "sess-2", manager)
    outcome = adapter.prompt("sess-2", "hello", threading.Event())
    assert outcome.text == "ok"  # work proceeded despite no usage telemetry

    records = observability_usage.list_usage(store, task_id)
    assert records[0]["basis"] == "unknown"
    assert records[0]["tokens"] == {}
    summary = observability_usage.usage_summary(store, task_id)
    assert summary["unknown_basis_calls"] == 1
    assert summary["total_calls"] == 1
    # unknown is never folded into a reported (zero) token total
    for bucket in summary["by_connection"].values():
        assert bucket["tokens_reported"] == {}


def test_mixed_known_and_unknown_usage_never_produces_a_falsely_cheap_total(tmp_path):
    """PR-07: when some calls in a task report tokens and others omit
    telemetry entirely, the aggregate must not silently treat the missing
    calls as zero-cost -- ``unknown_basis_calls`` stays a distinct,
    non-zero count alongside ``tokens_reported`` (which only ever reflects
    the calls that actually reported), so a caller reading the summary
    cannot mistake ``tokens_reported`` for the whole task's true usage."""
    store, task_id = _task(tmp_path)

    observability_usage.record_call_usage(
        store, task_id=task_id, attempt_id="att-5", engine_session_ref="sess-5a",
        usage_basis=UsageBasis.ENGINE_REPORTED, usage_values=dict(input_tokens=10, output_tokens=5),
        stop_reason="end_turn", failed=False,
    )
    # An engine that omits telemetry entirely for this call (usage_basis=None).
    observability_usage.record_call_usage(
        store, task_id=task_id, attempt_id="att-5", engine_session_ref="sess-5b",
        usage_basis=None, usage_values=None, stop_reason="end_turn", failed=False,
    )
    # An engine that explicitly reports UNKNOWN.
    observability_usage.record_call_usage(
        store, task_id=task_id, attempt_id="att-5", engine_session_ref="sess-5c",
        usage_basis=UsageBasis.UNKNOWN, usage_values=None, stop_reason="end_turn", failed=False,
    )

    summary = observability_usage.usage_summary(store, task_id)
    assert summary["total_calls"] == 3
    assert summary["unknown_basis_calls"] == 2
    for bucket in summary["by_connection"].values():
        # Only the one reporting call's tokens are summed -- the two
        # telemetry-omitted calls are never folded in as zero.
        assert bucket["tokens_reported"] == dict(input_tokens=10, output_tokens=5)
        assert bucket["unknown_basis_calls"] == 2
        assert bucket["calls"] == 3
    # A caller that reads only tokens_reported without unknown_basis_calls
    # would undercount this task's true call volume by 2/3 -- the summary
    # keeps that count directly alongside the (necessarily partial) totals
    # rather than omitting it, so the undercount can never be silent.
    assert summary["unknown_basis_calls"] > 0


def test_omitted_basis_and_none_values_are_both_stored_as_unknown_not_absent(tmp_path):
    """A ``None`` usage_basis (an adapter code path that never even calls
    record_call_usage's usage arguments meaningfully) is normalized to the
    same 'unknown' basis as an explicit UsageBasis.UNKNOWN -- there is no
    third, un-attributed state that a total could skip entirely."""
    store, task_id = _task(tmp_path)
    record_id = observability_usage.record_call_usage(
        store, task_id=task_id, attempt_id=None, engine_session_ref="sess-6",
        usage_basis=None, usage_values=None, stop_reason="end_turn", failed=False,
    )
    assert record_id
    records = observability_usage.list_usage(store, task_id)
    assert len(records) == 1
    assert records[0]["basis"] == "unknown"
    assert records[0]["tokens"] == {}


def test_retry_calls_are_each_attributed_separately(tmp_path):
    """A failed call followed by a retry on the same engine session
    produces two distinct usage_records rows -- the retry is never
    dropped or collapsed into only the final successful attempt."""
    from cortex_runtime.acp.orchestrating_runtime import EngineTurnFailed

    store, task_id = _task(tmp_path)
    adapter = _adapter(store)
    failing = _FakeManager([EngineEvent(kind=EngineEventKind.ERROR, error_message="boom")])
    _register(adapter, task_id, "att-3", "sess-3", failing)
    with pytest.raises(EngineTurnFailed):
        adapter.prompt("sess-3", "hello", threading.Event())

    succeeding = _FakeManager([
        EngineEvent(
            kind=EngineEventKind.USAGE_UPDATE, usage_basis=UsageBasis.ENGINE_REPORTED,
            usage_values=dict(input_tokens=1),
        ),
        EngineEvent(kind=EngineEventKind.TURN_END, stop_reason=StopReason.END_TURN),
    ])
    adapter._managers["att-3"] = succeeding
    adapter.prompt("sess-3", "hello", threading.Event())

    records = observability_usage.list_usage(store, task_id)
    assert [row["call_sequence"] for row in records] == [1, 2]
    assert records[0]["failed"] is True and records[0]["basis"] == "unknown"
    assert records[1]["failed"] is False and records[1]["basis"] == "engine_reported"


def test_route_is_derived_from_attempt_route_audit_join(tmp_path):
    store, task_id = _task(tmp_path)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute(
            "INSERT INTO attempt_route_audit "
            "(id, attempt_id, task_id, role, engine, connection_id, model, params_fingerprint, "
            " config_revision_id, decision, decision_detail_json, predecessor_attempt_id, recorded_at) "
            "VALUES ('ara_1','att-4',?,'worker','codex-acp','conn-1','gpt-x','fp','rev-1','authorized','{}',NULL,?)",
            (task_id, store._now()),
        )
        db.commit()
    observability_usage.record_call_usage(
        store, task_id=task_id, attempt_id="att-4", engine_session_ref="sess-4",
        usage_basis=UsageBasis.ENGINE_REPORTED, usage_values=dict(input_tokens=3),
        stop_reason="end_turn", failed=False,
    )
    records = observability_usage.list_usage(store, task_id)
    assert records[0]["connection_id"] == "conn-1"
    assert records[0]["engine"] == "codex-acp"
    assert records[0]["route_basis"] == "derived"
