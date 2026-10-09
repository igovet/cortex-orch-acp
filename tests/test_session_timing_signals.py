"""Stage 2 session/timing instrumentation (gaps G5, G6, G7).

G5: ``FIRST_MODEL_OUTPUT`` must fire only on a real model-produced
session/update (``agent_message_chunk``, ``agent_thought_chunk`` or
``tool_call``), never on an incidental update such as
``available_commands_update``, and its payload must record which kind was
first observed.

G6: ``COORDINATOR_TURN_ENDED`` pairs with ``COORDINATOR_TURN_STARTED`` after
the coordinator's ``prompt()`` call returns or raises.

G7: tokens for a stall-nudge/refusal continuation (more than one inner
``prompt_turn`` under the same adapter ``prompt()`` call) are summed across
every inner turn instead of only the last one winning; a call that never
sees a ``usage_update`` stays ``basis='unknown'``, never zero.
"""
from __future__ import annotations

import json
import threading
import uuid
from types import SimpleNamespace

import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.observability import usage as observability_usage
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.engine_contract import EngineEvent, EngineEventKind, StopReason, UsageBasis
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from engine_fakes import default_scenario
from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace
from test_engine_session_manager import REQUESTED_CAPABILITIES, _make_manager


# --- G5: FIRST_MODEL_OUTPUT gating and payload --------------------------------


def test_available_commands_update_first_defers_the_milestone_until_a_real_message_chunk(tmp_path):
    scenario = default_scenario(
        prompt_updates=[
            dict(delay=0.0, sessionUpdate="available_commands_update", availableCommands=[]),
            dict(delay=0.0, sessionUpdate="agent_message_chunk", content=dict(type="text", text="hi")),
        ],
    )
    manager = _make_manager(tmp_path, scenario)
    observed = []
    manager._lifecycle_observer = lambda kind, session, payload=None: observed.append((kind, payload))
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "route")
        assert [o for o in observed if o[0] == "FIRST_MODEL_OUTPUT"] == []
        list(manager.prompt_turn(session_id, "hello"))
    finally:
        manager.shutdown(grace=2.0)
    # The available_commands_update that arrived first must never have
    # started the milestone; only the later agent_message_chunk does, and
    # its own kind is what the payload records.
    first_model_output = [o for o in observed if o[0] == "FIRST_MODEL_OUTPUT"]
    assert len(first_model_output) == 1
    assert first_model_output[0][1] == {"update_kind": "agent_message_chunk"}


def test_a_tool_call_also_counts_as_the_first_model_output(tmp_path):
    scenario = default_scenario(
        prompt_updates=[
            dict(delay=0.0, sessionUpdate="tool_call", toolCallId="call-1", title="Read",
                 kind="read", status="in_progress"),
        ],
    )
    manager = _make_manager(tmp_path, scenario)
    observed = []
    manager._lifecycle_observer = lambda kind, session, payload=None: observed.append((kind, payload))
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "route")
        list(manager.prompt_turn(session_id, "hello"))
    finally:
        manager.shutdown(grace=2.0)
    first_model_output = [o for o in observed if o[0] == "FIRST_MODEL_OUTPUT"]
    assert len(first_model_output) == 1
    assert first_model_output[0][1] == {"update_kind": "tool_call"}


# --- G6: COORDINATOR_TURN_STARTED/ENDED pairing --------------------------------


def _coordinator_task(store, goal):
    task = bootstrap_task(store, SourceEvent(origin="test", event_id=str(uuid.uuid4())), goal, "timing-owner")
    TaskMachine(store).create(task.task_id, goal)
    channel = SimpleNamespace(task_id=task.task_id, channel_id="timing-coordinator", role="coordinator")
    return task.task_id, channel


def _coordinator_events(store, task_id):
    with store.connection() as db:
        started = [json.loads(r[0]) for r in db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='COORDINATOR_TURN_STARTED' "
            "ORDER BY id", (task_id,))]
        ended = [json.loads(r[0]) for r in db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='COORDINATOR_TURN_ENDED' "
            "ORDER BY id", (task_id,))]
    return started, ended


def test_a_successful_coordinator_turn_gets_a_started_ended_pair_with_stop_reason(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task_id, channel = _coordinator_task(store, "Measure a successful coordinator turn.")

    def router(_store, _channel, _session_ref, _launch, _instructions):
        def behavior(*_args, **_kwargs):
            return PromptOutcome(StopReason.END_TURN, "Nothing to plan yet.")
        return behavior

    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                   allow_unqualified_delivery=True)
    try:
        runtime._ensure_coordinator(task_id)
        runtime.start_prompt(task_id, "timing-session", "hello", lambda _text: True, threading.Event())
    finally:
        runtime.shutdown()
    started, ended = _coordinator_events(store, task_id)
    assert started and len(started) == len(ended)
    for start, end in zip(started, ended):
        assert end["engine_session_id"] == start["engine_session_id"]
        assert end["stop_reason"] == "end_turn"
        assert "failure_kind" not in end


def test_a_raising_coordinator_turn_gets_a_started_ended_pair_with_failure_kind(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task_id, channel = _coordinator_task(store, "Measure a failing coordinator turn.")

    def router(_store, _channel, _session_ref, _launch, _instructions):
        def behavior(*_args, **_kwargs):
            raise RuntimeError("boom")
        return behavior

    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                   allow_unqualified_delivery=True)
    try:
        runtime._ensure_coordinator(task_id)
        runtime.start_prompt(task_id, "timing-session", "hello", lambda _text: True, threading.Event())
    finally:
        runtime.shutdown()
    started, ended = _coordinator_events(store, task_id)
    assert len(started) == len(ended) == 1
    assert ended[0]["engine_session_id"] == started[0]["engine_session_id"]
    assert ended[0]["failure_kind"] == "RuntimeError"
    assert "stop_reason" not in ended[0]


# --- G7: usage accumulated across inner turns, never last-wins ----------------


def _usage_adapter(store, manager, task_id):
    from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter

    adapter = EngineSessionManagerAdapter.__new__(EngineSessionManagerAdapter)
    adapter._lock = threading.Lock()
    adapter._store = store
    adapter._session_to_attempt = {"sess_1": "attempt_1"}
    adapter._attempt_task = {"attempt_1": task_id}
    adapter._managers = {"attempt_1": manager}
    return adapter


class _UsageContinuationManager:
    """A refusal continuation spanning ``len(token_counts)`` inner turns,
    each optionally reporting its own (cumulative-within-that-turn) usage.
    """

    def __init__(self, token_counts):
        self.prompts = []
        self.token_counts = token_counts

    def prompt_turn(self, session_ref, message, *, prompt_blocks=None):
        index = len(self.prompts)
        self.prompts.append(message)
        tokens = self.token_counts[index]
        if tokens is not None:
            yield EngineEvent(kind=EngineEventKind.USAGE_UPDATE, usage_basis=UsageBasis.ENGINE_REPORTED,
                              usage_values={"total_tokens": tokens})
        last = index == len(self.token_counts) - 1
        yield EngineEvent(kind=EngineEventKind.TURN_END,
                          stop_reason=StopReason.END_TURN if last else StopReason.CANCELLED)

    def take_permission_refusal(self, session_ref):
        return len(self.prompts) < len(self.token_counts)

    def cancel(self, session_ref):
        pass


def test_a_refusal_continuation_sums_usage_from_every_inner_turn(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task_id, _channel = _coordinator_task(store, "Measure a continuation's accumulated usage.")
    manager = _UsageContinuationManager(token_counts=[5, 3])
    adapter = _usage_adapter(store, manager, task_id)
    outcome = adapter.prompt("sess_1", "do the thing", threading.Event())
    assert outcome.stop_reason == StopReason.END_TURN
    assert len(manager.prompts) == 2
    records = observability_usage.list_usage(store, task_id)
    assert len(records) == 1
    assert records[0]["basis"] == "engine_reported"
    assert records[0]["tokens"] == {"total_tokens": 8}


def test_a_turn_with_no_usage_update_stays_unknown_not_zero(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task_id, _channel = _coordinator_task(store, "Measure a call with no reported usage.")
    manager = _UsageContinuationManager(token_counts=[None])
    adapter = _usage_adapter(store, manager, task_id)
    outcome = adapter.prompt("sess_1", "do the thing", threading.Event())
    assert outcome.stop_reason == StopReason.END_TURN
    records = observability_usage.list_usage(store, task_id)
    assert len(records) == 1
    assert records[0]["basis"] == "unknown"
    assert records[0]["tokens"] == {}


# --- P1 fix: one inner turn with no usage report makes the whole call's
# total incomplete, even when another inner turn of the same call did
# report usage -- the combined total must never be recorded as fully
# 'engine_reported' in that mixed case. --------------------------------


class _UsageTurnManager:
    """Generalizes ``_UsageContinuationManager``: each inner turn either
    reports an explicit ``(basis, usage_values)`` pair or nothing at all
    (``None``), spanning ``len(turns)`` refusal-continuation hops."""

    def __init__(self, turns):
        self.prompts = []
        self.turns = turns

    def prompt_turn(self, session_ref, message, *, prompt_blocks=None):
        index = len(self.prompts)
        self.prompts.append(message)
        turn = self.turns[index]
        if turn is not None:
            basis, values = turn
            yield EngineEvent(kind=EngineEventKind.USAGE_UPDATE, usage_basis=basis, usage_values=values)
        last = index == len(self.turns) - 1
        yield EngineEvent(kind=EngineEventKind.TURN_END,
                          stop_reason=StopReason.END_TURN if last else StopReason.CANCELLED)

    def take_permission_refusal(self, session_ref):
        return len(self.prompts) < len(self.turns)

    def cancel(self, session_ref):
        pass


def test_a_reporting_turn_followed_by_a_missing_turn_stays_unknown(tmp_path):
    """[reported, missing]: the first inner turn reports usage, the second
    (final) inner turn reports none at all -- the combined total must be
    recorded 'unknown', not 'engine_reported', because the second turn's
    real contribution was never observed."""
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task_id, _channel = _coordinator_task(store, "Measure reported-then-missing usage.")
    manager = _UsageTurnManager(turns=[(UsageBasis.ENGINE_REPORTED, {"total_tokens": 5}), None])
    adapter = _usage_adapter(store, manager, task_id)
    outcome = adapter.prompt("sess_1", "do the thing", threading.Event())
    assert outcome.stop_reason == StopReason.END_TURN
    assert len(manager.prompts) == 2
    records = observability_usage.list_usage(store, task_id)
    assert len(records) == 1
    assert records[0]["basis"] == "unknown"
    assert records[0]["tokens"] == {}


def test_a_missing_turn_followed_by_a_reporting_turn_stays_unknown(tmp_path):
    """[missing, reported]: the first inner turn reports nothing at all,
    the second (final) inner turn reports usage -- the combined total must
    still be recorded 'unknown', never upgraded to 'engine_reported' just
    because the later turn happened to report."""
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task_id, _channel = _coordinator_task(store, "Measure missing-then-reported usage.")
    manager = _UsageTurnManager(turns=[None, (UsageBasis.ENGINE_REPORTED, {"total_tokens": 5})])
    adapter = _usage_adapter(store, manager, task_id)
    outcome = adapter.prompt("sess_1", "do the thing", threading.Event())
    assert outcome.stop_reason == StopReason.END_TURN
    assert len(manager.prompts) == 2
    records = observability_usage.list_usage(store, task_id)
    assert len(records) == 1
    assert records[0]["basis"] == "unknown"
    assert records[0]["tokens"] == {}


def test_two_reporting_turns_stay_engine_reported_and_sum(tmp_path):
    """[reported, reported]: both inner turns report usage, so the
    combined total is genuinely complete and stays 'engine_reported'."""
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task_id, _channel = _coordinator_task(store, "Measure two fully reported turns.")
    manager = _UsageTurnManager(turns=[
        (UsageBasis.ENGINE_REPORTED, {"total_tokens": 5}),
        (UsageBasis.ENGINE_REPORTED, {"total_tokens": 3}),
    ])
    adapter = _usage_adapter(store, manager, task_id)
    outcome = adapter.prompt("sess_1", "do the thing", threading.Event())
    assert outcome.stop_reason == StopReason.END_TURN
    records = observability_usage.list_usage(store, task_id)
    assert len(records) == 1
    assert records[0]["basis"] == "engine_reported"
    assert records[0]["tokens"] == {"total_tokens": 8}


def test_non_integer_usage_values_sum_without_error_when_fully_reported(tmp_path):
    """Non-integer (float) usage figures must sum without raising and keep
    the call 'engine_reported' when every inner turn actually reported."""
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task_id, _channel = _coordinator_task(store, "Measure non-integer usage figures.")
    manager = _UsageTurnManager(turns=[
        (UsageBasis.ENGINE_REPORTED, {"total_tokens": 2.5}),
        (UsageBasis.ENGINE_REPORTED, {"total_tokens": 1.5}),
    ])
    adapter = _usage_adapter(store, manager, task_id)
    outcome = adapter.prompt("sess_1", "do the thing", threading.Event())
    assert outcome.stop_reason == StopReason.END_TURN
    records = observability_usage.list_usage(store, task_id)
    assert len(records) == 1
    assert records[0]["basis"] == "engine_reported"
    assert records[0]["tokens"] == {"total_tokens": 4.0}


def test_an_explicit_unknown_basis_update_keeps_the_call_unknown_with_no_tokens(tmp_path):
    """An inner turn can itself report ``usage_basis=UsageBasis.UNKNOWN``
    (distinct from reporting nothing at all); the combined total stays
    'unknown' and its tokens are never persisted, even though an earlier
    turn in the same call did report real figures."""
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task_id, _channel = _coordinator_task(store, "Measure an explicit unknown-basis update.")
    manager = _UsageTurnManager(turns=[
        (UsageBasis.ENGINE_REPORTED, {"total_tokens": 5}),
        (UsageBasis.UNKNOWN, {"total_tokens": 999}),
    ])
    adapter = _usage_adapter(store, manager, task_id)
    outcome = adapter.prompt("sess_1", "do the thing", threading.Event())
    assert outcome.stop_reason == StopReason.END_TURN
    records = observability_usage.list_usage(store, task_id)
    assert len(records) == 1
    assert records[0]["basis"] == "unknown"
    assert records[0]["tokens"] == {}


class _ReleasedMidTurnManager:
    """A single inner turn that never ends normally -- it raises a
    transport-level ``ERROR`` event, as a post-acceptance release does."""

    def prompt_turn(self, session_ref, message, *, prompt_blocks=None):
        yield EngineEvent(kind=EngineEventKind.ERROR, error_message="released_mid_turn")

    def cancel(self, session_ref):
        pass


def test_a_final_turn_released_after_an_accepted_result_is_recorded_unknown_not_failed(tmp_path):
    """H3/P1: once this exact attempt already published its ``worker_result``,
    a later ``EngineTurnFailed`` on the same call (the runtime's own
    post-acceptance release closing the transport) is the runtime ending an
    already-successful call, not a failed one, and its basis is forced to
    'unknown' (never silently kept as some stale earlier-turn basis)."""
    from cortex_runtime.acp.orchestrating_runtime import EngineTurnFailed
    from cortex_runtime.delegation import events as delegation_events

    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task_id, _channel = _coordinator_task(store, "Measure a post-result release.")
    delegation_events.append_event(
        store, task_id, "worker_result", "result_submit:report-1",
        dict(report_ref="report-1", summary="Observed result", checks=[]),
        origin="ch_worker", assignment_ref="attempt_1",
    )
    manager = _ReleasedMidTurnManager()
    adapter = _usage_adapter(store, manager, task_id)
    with pytest.raises(EngineTurnFailed):
        adapter.prompt("sess_1", "do the thing", threading.Event())

    records = observability_usage.list_usage(store, task_id)
    assert len(records) == 1
    assert records[0]["failed"] is False
    assert records[0]["stop_reason"] == "released_after_result"
    assert records[0]["basis"] == "unknown"
    assert records[0]["tokens"] == {}
