"""Metadata-only replays of installed ACP token shapes; no paid engines."""
from __future__ import annotations

import json
import sqlite3
import threading
import time
from types import SimpleNamespace

import pytest

from cortex_runtime.acp.orchestrating_runtime import EngineTurnFailed
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.engine.session_manager import _token_values
from cortex_runtime.observability import usage
from cortex_runtime.portable.engine_contract import EngineEvent, EngineEventKind, UsageBasis

from engine_fakes import default_scenario
from test_cortex_timeline_aggregates import timeline
from test_engine_session_manager import REQUESTED_CAPABILITIES, _make_manager
from test_observability_usage import _adapter, _register, _task
from test_session_timing_signals import _UsageTurnManager


# Field names and these numeric metadata examples occur in the retained ab2
# coordinator records. Worker raw prompt responses were not retained there.
TOKENS = {
    "claude-agent-acp": dict(inputTokens=6, outputTokens=653, cachedReadTokens=77204,
                             cachedWriteTokens=39972, totalTokens=117835),
    "codex-acp": dict(inputTokens=843, outputTokens=4, cachedReadTokens=26368,
                      thoughtTokens=0, totalTokens=27215),
}


@pytest.mark.parametrize("engine", TOKENS)
@pytest.mark.parametrize("attempt", ["co_replay", "xa_replay"])
def test_real_response_shapes_persist_for_both_roles(tmp_path, engine, attempt):
    project = tmp_path / 'project'
    project.mkdir()
    store, task_id = _task(project)
    manager = _make_manager(tmp_path, default_scenario(
        prompt_updates=[dict(sessionUpdate="usage_update", used=84000, size=200000)],
        prompt_usage=TOKENS[engine]), engine=engine)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session = manager.create_session(task_id, "route")
        adapter = _adapter(store)
        _register(adapter, task_id, attempt, session, manager)
        adapter.prompt(session, "replay", threading.Event())
        rows = usage.list_usage(store, task_id)
        assert len(rows) == 1
        assert rows[0]["basis"] == "engine_reported"
        assert rows[0]["tokens"] == TOKENS[engine]
        assert manager.take_abandoned_usage(session) is None
    finally:
        manager.shutdown(grace=0.5)


class _ReleaseMarker:
    """Interrupt event consumption; the wire response settles during cleanup."""
    def __init__(self, manager):
        self.manager = manager

    def prompt_turn(self, session, message, *, prompt_blocks=None):
        turn = self.manager.prompt_turn(session, message, prompt_blocks=prompt_blocks)
        try:
            yield next(turn)
            yield EngineEvent(kind=EngineEventKind.ERROR, error_message="released_mid_turn")
        finally:
            turn.close()

    def take_abandoned_usage(self, session):
        return self.manager.take_abandoned_usage(session)

    def cancel(self, session):
        self.manager.cancel(session)


@pytest.mark.parametrize("engine", TOKENS)
@pytest.mark.parametrize("reported", [True, False])
def test_bounded_abandonment_keeps_only_a_real_final_response(tmp_path, engine, reported):
    project = tmp_path / 'project'
    project.mkdir()
    store, task_id = _task(project)
    scenario = default_scenario(
        prompt_updates=[dict(sessionUpdate="usage_update", used=84000, size=200000),
                        dict(delay=0.05, sessionUpdate="usage_update", used=84001, size=200000)],
        prompt_usage=TOKENS[engine] if reported else None)
    manager = _make_manager(tmp_path, scenario, engine=engine, abandon_wait_seconds=1)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session = manager.create_session(task_id, "route")
        delegation_events.append_event(
            store, task_id, "worker_result", "result:replay", {}, origin="test", assignment_ref="xa_replay")
        adapter = _adapter(store)
        _register(adapter, task_id, "xa_replay", session, _ReleaseMarker(manager))
        with pytest.raises(EngineTurnFailed):
            adapter.prompt(session, "replay", threading.Event())
        rows = usage.list_usage(store, task_id)
        assert len(rows) == 1
        assert rows[0]["stop_reason"] == "released_after_result"
        assert rows[0]["failed"] is False
        assert rows[0]["basis"] == ("engine_reported" if reported else "unknown")
        assert rows[0]["tokens"] == (TOKENS[engine] if reported else {})
        assert manager.take_abandoned_usage(session) is None  # consumed once
        list(manager.prompt_turn(session, "next turn"))
        assert manager.take_abandoned_usage(session) is None  # no stale replay
    finally:
        manager.shutdown(grace=0.5)


def test_abandonment_timeout_does_not_claim_late_usage(tmp_path):
    manager = _make_manager(tmp_path, default_scenario(
        prompt_updates=[dict(sessionUpdate="usage_update", used=1, size=100)],
        prompt_behavior="late_reply", late_reply_delay=0.1,
        prompt_usage=TOKENS["codex-acp"]), abandon_wait_seconds=0.001)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session = manager.create_session("t_aaaaaaaaaaaa", "route")
        turn = manager.prompt_turn(session, "replay")
        next(turn)
        turn.close()
        assert session in manager._poisoned_sessions
        assert manager.take_abandoned_usage(session) is None
    finally:
        manager.shutdown(grace=0.5)


@pytest.mark.parametrize("payload", [None, [], "invalid", {"used": 10}, {"novelCount": 5},
                                     {"inputTokens": True}, {"totalTokens": -1}])
def test_unknown_or_malformed_wire_usage_never_becomes_reported(payload):
    assert _token_values(payload) == {}


def test_wire_zero_is_real_but_metadata_and_booleans_are_not_tokens():
    assert _token_values(dict(totalTokens=0, inputTokens=True, used=7, _meta={})) == {"totalTokens": 0}


@pytest.mark.parametrize("turns", [
    [(UsageBasis.UNKNOWN, {}), (UsageBasis.ENGINE_REPORTED, {"totalTokens": 5})],
    [(UsageBasis.ENGINE_REPORTED, {"totalTokens": 5}), (UsageBasis.UNKNOWN, {}),
     (UsageBasis.ENGINE_REPORTED, {"totalTokens": 3})],
])
def test_context_only_inner_turn_cannot_be_overwritten_by_later_report(tmp_path, turns):
    project = tmp_path / 'project'
    project.mkdir()
    store, task_id = _task(project)
    adapter = _adapter(store)
    _register(adapter, task_id, "xa_partial", "session", _UsageTurnManager(turns))
    adapter.prompt("session", "replay", threading.Event())
    rows = usage.list_usage(store, task_id)
    assert rows[0]["basis"] == "unknown"
    assert rows[0]["tokens"] == {}


def test_role_coverage_keeps_released_calls_unknown_and_unattributed_routes_explicit():
    db = sqlite3.connect(":memory:")
    db.row_factory = sqlite3.Row
    db.executescript("""
        CREATE TABLE usage_records(task_id,attempt_id,basis,tokens_json,failed,stop_reason);
        CREATE TABLE attempt_route_audit(attempt_id,engine,recorded_at);
        CREATE TABLE delegation_provisioning(task_id,attempt_id,idempotency_key);
        CREATE TABLE runtime_nodes(task_id,assignment_ref,stage);
        INSERT INTO usage_records VALUES ('t','co_1','engine_reported','{"totalTokens":7}',0,'end_turn');
        INSERT INTO usage_records VALUES ('t','xa_1','unknown','{}',0,'released_after_result');
        INSERT INTO usage_records VALUES ('t','xa_2','unknown','{}',1,'released_after_result');
        INSERT INTO attempt_route_audit VALUES ('co_1','codex-acp','1');
        INSERT INTO attempt_route_audit VALUES ('xa_1','codex-acp','1');
        INSERT INTO delegation_provisioning VALUES ('t','xa_1','work');
        INSERT INTO runtime_nodes VALUES ('t','work','execution');
    """)
    rows = timeline._token_coverage(db, ["t"])
    coordinator, worker, unattributed = rows
    assert coordinator["role"] == "coordinator"
    assert coordinator["reported_calls"] == 1
    assert coordinator["unknown_token_ratio"] == 0
    assert coordinator["tokens_reported"] == {"totalTokens": 7}
    assert worker["role"] == "worker"
    assert worker["released_after_result_calls"] == 1
    assert worker["unknown_token_ratio"] == 1
    assert worker["tokens_reported"] == {}
    assert unattributed["engine"] == unattributed["role"] == "unattributed"
    assert unattributed["unknown_calls"] == 1
    assert unattributed["released_after_result_calls"] == 0
    assert timeline._token_coverage(db, ["absent"]) == []
    db.close()


def _completion_adapter(store, task_id, attempt, session, manager):
    adapter = _adapter(store)
    _register(adapter, task_id, attempt, session, manager)
    adapter._sessions = {attempt: session}
    adapter._reservations = {}
    adapter._session_meta = {}
    adapter._session_configured_revision = {}
    adapter._activity_cond = threading.Condition()
    adapter._RESULT_SETTLE_SECONDS = 1
    return adapter


@pytest.mark.parametrize("engine", TOKENS)
@pytest.mark.parametrize("attempt", ["xa_worker", "xa_review"])
@pytest.mark.parametrize("reported", [True, False])
def test_post_result_teardown_waits_for_the_real_wire_response(tmp_path, engine, attempt, reported):
    """Replay result committed before the final prompt response on both engines."""
    project = tmp_path / "project"
    project.mkdir()
    store, task_id = _task(project)
    manager = _make_manager(tmp_path, default_scenario(
        prompt_updates=[dict(sessionUpdate="usage_update", used=84000, size=200000),
                        dict(delay=0.15, sessionUpdate="agent_message_chunk",
                             content=dict(type="text", text="done"))],
        prompt_usage=TOKENS[engine] if reported else None), engine=engine)
    consumer_done = threading.Event()
    errors = []
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session = manager.create_session(task_id, "route")
        adapter = _completion_adapter(store, task_id, attempt, session, manager)
        released = []
        class WriterLock:
            def __exit__(self, *_):
                assert manager._process.wait_exited(0)
                assert len(usage.list_usage(store, task_id)) == 1
                released.append(True)
        adapter._workspace_locks = {attempt: WriterLock()}
        observed = threading.Event()
        def consume():
            try:
                adapter.prompt(session, "replay", threading.Event(),
                               on_display_update=lambda _: observed.set())
            except BaseException as exc:
                errors.append(exc)
            finally:
                consumer_done.set()
        thread = threading.Thread(target=consume)
        thread.start()
        assert observed.wait(2)
        delegation_events.append_event(
            store, task_id, "worker_result", "result:replay", {},
            origin="test", assignment_ref=attempt)
        adapter.finish_owned(attempt)
        cleanup = adapter._result_settlements[attempt]
        adapter.finish_owned(attempt)  # one owned cleanup only
        assert adapter._result_settlements[attempt] is cleanup
        assert manager._process.wait_exited(0) is False
        assert usage.list_usage(store, task_id) == []  # unknown is not prematurely banked
        assert consumer_done.wait(3)
        thread.join(1)
        cleanup.join(3)
        assert not thread.is_alive() and not cleanup.is_alive()
        assert errors == []
        rows = usage.list_usage(store, task_id)
        assert len(rows) == 1
        assert rows[0]["attempt_id"] == attempt
        assert rows[0]["engine_session_ref"] == session
        assert rows[0]["basis"] == ("engine_reported" if reported else "unknown")
        assert rows[0]["tokens"] == (TOKENS[engine] if reported else {})
        assert rows[0]["stop_reason"] == "end_turn"
        assert manager._process.wait_exited(0)
        assert not adapter._result_settlements
        assert released == [True]
    finally:
        manager.shutdown(grace=0.5)


@pytest.mark.parametrize("engine", TOKENS)
def test_post_result_settlement_timeout_preserves_unknown_and_reaps(tmp_path, engine):
    project = tmp_path / "project"
    project.mkdir()
    store, task_id = _task(project)
    manager = _make_manager(tmp_path, default_scenario(
        prompt_updates=[dict(sessionUpdate="usage_update", used=84000, size=200000)],
        prompt_behavior="one_update_then_hang"), engine=engine)
    errors = []
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session = manager.create_session(task_id, "route")
        adapter = _completion_adapter(store, task_id, "xa_timeout", session, manager)
        adapter._RESULT_SETTLE_SECONDS = 0.03
        observed = threading.Event()
        def consume():
            try:
                adapter.prompt(session, "replay", threading.Event(),
                               on_display_update=lambda _: observed.set())
            except EngineTurnFailed as exc:
                errors.append(exc)
        thread = threading.Thread(target=consume)
        thread.start()
        assert observed.wait(2)
        delegation_events.append_event(
            store, task_id, "worker_result", "result:timeout", {},
            origin="test", assignment_ref="xa_timeout")
        adapter.finish_owned("xa_timeout")
        cleanup = adapter._result_settlements["xa_timeout"]
        cleanup.join(3)
        thread.join(3)
        assert not cleanup.is_alive() and not thread.is_alive()
        assert len(errors) == 1
        row, = usage.list_usage(store, task_id)
        assert row["basis"] == "unknown" and row["tokens"] == {}
        assert row["stop_reason"] == "released_after_result"
        with store.connection() as db:
            event = db.execute("SELECT payload_json FROM task_events "
                               "WHERE dedup_key=?", (f"usage_settlement:xa_timeout:{session}",)).fetchone()
        assert json.loads(event[0])["reason"] == "post_result_prompt_settlement_timeout"
        assert manager._process.wait_exited(0)
    finally:
        manager.shutdown(grace=0.5)


def test_completed_worker_dispatch_does_not_wait_for_usage_settlement(tmp_path, monkeypatch):
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
    from cortex_runtime.orchestration import recovery
    store, task_id = _task(tmp_path)
    delegation_events.append_event(store, task_id, "worker_result", "result:dispatch", {},
                                   origin="test", assignment_ref="xa_dispatch")
    runtime = object.__new__(OrchestratingRuntime)
    runtime._store = store
    runtime._lock = threading.Lock()
    runtime._worker_turns_in_flight = {"assignment"}
    runtime._terminated_attempts = set()
    finishes = []
    runtime._engine = SimpleNamespace(finish_owned=finishes.append,
                                     terminate_owned=lambda _: pytest.fail("eager termination"))
    runtime._cleanup_isolated_workspace_if_integrated = lambda *_: None
    runtime._notify = lambda: None
    monkeypatch.setattr(recovery, "release_observed", lambda *_: False)
    runtime._terminate_completed_workers(task_id)
    assert finishes == ["xa_dispatch"]
    assert runtime._terminated_attempts == set()  # still alive, never falsely released


def test_settlement_does_not_terminate_a_replacement_or_delay_explicit_cancel(tmp_path):
    store, task_id = _task(tmp_path)
    manager = object()
    adapter = _completion_adapter(store, task_id, "xa_cancel", "session", manager)
    done = threading.Event()
    adapter._prompt_done = {"xa_cancel": done}
    calls = []
    def terminate(attempt, **kwargs):
        calls.append(attempt)
        adapter._managers.pop(attempt, None)
    adapter.terminate_owned = terminate
    adapter.finish_owned("xa_cancel")
    cleanup = adapter._result_settlements["xa_cancel"]
    adapter.terminate_owned("xa_cancel")  # cancellation does not wait on settlement
    replacement = object()
    adapter._managers["xa_cancel"] = replacement
    done.set()
    cleanup.join(2)
    assert not cleanup.is_alive()
    assert calls == ["xa_cancel"]
    assert adapter._managers["xa_cancel"] is replacement


@pytest.mark.parametrize("engine", TOKENS)
def test_eager_shutdown_reproduces_live_unknown_after_accepted_result(tmp_path, engine):
    project = tmp_path / "project"
    project.mkdir()
    store, task_id = _task(project)
    manager = _make_manager(tmp_path, default_scenario(
        prompt_updates=[dict(sessionUpdate="usage_update", used=84000, size=200000),
                        dict(delay=0.2, sessionUpdate="agent_message_chunk",
                             content=dict(type="text", text="done"))],
        prompt_usage=TOKENS[engine]), engine=engine)
    errors = []
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session = manager.create_session(task_id, "route")
        adapter = _completion_adapter(store, task_id, "xa_eager", session, manager)
        observed = threading.Event()
        def consume():
            try:
                adapter.prompt(session, "replay", threading.Event(),
                               on_display_update=lambda _: observed.set())
            except EngineTurnFailed as exc:
                errors.append(exc)
        thread = threading.Thread(target=consume)
        thread.start()
        assert observed.wait(2)
        delegation_events.append_event(store, task_id, "worker_result", "result:eager", {},
                                       origin="test", assignment_ref="xa_eager")
        adapter.terminate_owned("xa_eager")  # original live path closes the client first
        thread.join(3)
        assert not thread.is_alive() and len(errors) == 1
        row, = usage.list_usage(store, task_id)
        assert row["basis"] == "unknown" and row["tokens"] == {}
        assert row["stop_reason"] == "released_after_result"
    finally:
        manager.shutdown(grace=0.5)


def test_settlement_observation_failure_does_not_skip_owned_cleanup(tmp_path, monkeypatch):
    store, task_id = _task(tmp_path)
    manager = object()
    adapter = _completion_adapter(store, task_id, "xa_event_error", "session", manager)
    adapter._result_settlements = {"xa_event_error": object()}
    done = threading.Event()
    done.set()
    terminated = []
    adapter.terminate_owned = lambda attempt, **kwargs: terminated.append((attempt, kwargs))
    def reject_observation(*args, **kwargs):
        raise RuntimeError("observation write failed")
    monkeypatch.setattr(delegation_events, "append_event", reject_observation)
    with pytest.raises(RuntimeError, match="observation write failed"):
        adapter._finish_result_prompt("xa_event_error", manager, done)
    assert terminated == [("xa_event_error", {"expected_manager": manager})]
    assert not adapter._result_settlements


def test_expected_manager_guard_is_atomic_before_detaching_replacement(tmp_path):
    store, task_id = _task(tmp_path)
    replacement = object()
    adapter = _completion_adapter(store, task_id, "xa_guard", "session", replacement)
    adapter.terminate_owned("xa_guard", expected_manager=object())
    assert adapter._managers["xa_guard"] is replacement
    assert adapter._sessions["xa_guard"] == "session"


class _GatedManager:
    """Engine manager whose teardown the test can hold open."""
    def __init__(self, gate=None):
        self.gate = gate
        self.entered = threading.Event()
        self.shutdowns = []

    def shutdown(self, grace=None):
        self.shutdowns.append(grace)
        self.entered.set()
        if self.gate is not None:
            self.gate.wait(10)


def _settling_adapter(store, task_id, attempt, manager):
    adapter = _completion_adapter(store, task_id, attempt, "session", manager)
    adapter._shutdown_event = threading.Event()
    adapter._RESULT_SETTLE_SECONDS = 30  # far beyond every bound asserted below
    adapter._prompt_done = {attempt: threading.Event()}
    return adapter


def _settlement_events(store, attempt):
    with store.connection() as db:
        return [json.loads(row[0]) for row in db.execute(
            "SELECT payload_json FROM task_events WHERE dedup_key LIKE ?",
            (f"usage_settlement:{attempt}:%",)).fetchall()]


def test_shutdown_cancels_a_settlement_whose_usage_never_arrives(tmp_path):
    store, task_id = _task(tmp_path)
    manager = _GatedManager()
    adapter = _settling_adapter(store, task_id, "xa_never", manager)
    started = time.monotonic()
    adapter.finish_owned("xa_never")
    assert time.monotonic() - started < 1  # dispatch never waits on settlement
    cleanup = adapter._result_settlements["xa_never"]
    assert cleanup.is_alive() and manager.shutdowns == []
    started = time.monotonic()
    adapter.shutdown()
    assert time.monotonic() - started < 3
    assert not cleanup.is_alive()  # no settlement wait outlives shutdown
    assert manager.shutdowns == [2.0]  # torn down exactly once
    assert "xa_never" not in adapter._managers and not adapter._result_settlements
    assert _settlement_events(store, "xa_never") == []  # shutdown is not a timeout


def test_shutdown_waits_for_settlement_teardown_already_in_progress(tmp_path):
    store, task_id = _task(tmp_path)
    gate = threading.Event()
    manager = _GatedManager(gate)
    adapter = _settling_adapter(store, task_id, "xa_tearing", manager)
    adapter.finish_owned("xa_tearing")
    cleanup = adapter._result_settlements["xa_tearing"]
    adapter._prompt_done["xa_tearing"].set()  # the consumer recorded its usage
    assert manager.entered.wait(5)  # settlement now owns the teardown
    stopped = threading.Event()
    stopper = threading.Thread(target=lambda: (adapter.shutdown(), stopped.set()))
    stopper.start()
    try:
        assert not stopped.wait(0.3)  # shutdown does not return mid-teardown
    finally:
        gate.set()
    assert stopped.wait(5)
    stopper.join(1)
    assert not cleanup.is_alive()
    assert manager.shutdowns == [2.0]
    assert [e["outcome"] for e in _settlement_events(store, "xa_tearing")] == ["prompt_recorded"]


def test_shutdown_join_on_a_stuck_settlement_teardown_is_bounded(tmp_path):
    store, task_id = _task(tmp_path)
    gate = threading.Event()
    manager = _GatedManager(gate)
    adapter = _settling_adapter(store, task_id, "xa_stuck", manager)
    adapter._SETTLEMENT_SHUTDOWN_JOIN_SECONDS = 0.2
    adapter.finish_owned("xa_stuck")
    cleanup = adapter._result_settlements["xa_stuck"]
    adapter._prompt_done["xa_stuck"].set()
    assert manager.entered.wait(5)
    try:
        started = time.monotonic()
        adapter.shutdown()
        assert time.monotonic() - started < 2
        assert cleanup.is_alive()  # left behind as a daemon, never awaited forever
    finally:
        gate.set()
    cleanup.join(5)
    assert not cleanup.is_alive() and manager.shutdowns == [2.0]


def test_finish_after_shutdown_tears_down_without_a_settlement_wait(tmp_path):
    store, task_id = _task(tmp_path)
    manager = _GatedManager()
    adapter = _settling_adapter(store, task_id, "xa_late", manager)
    adapter._shutdown_event.set()
    started = time.monotonic()
    adapter.finish_owned("xa_late")
    assert time.monotonic() - started < 1
    assert manager.shutdowns == [2.0] and not adapter._result_settlements
