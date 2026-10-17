"""Transient engine failures are retried with a capped delay and a bounded
window (engine.retry_policy.windowed_retry), each retry shown in the chat:

- a coordinator engine that fails to start (acp.orchestrating_runtime
  ._ensure_coordinator_retrying);
- a coordinator turn that failed before it streamed or called anything
  (orchestration.controller);
- a worker lost or failed for an infrastructure reason
  (orchestration.recovery.retry_lost, held by TaskMachine._ready_nodes and
  shown by orchestration.wait_notices).

In-process fakes only: no engine subprocess and no paid model call.
"""
from __future__ import annotations

import json
import threading
import time
import uuid
from types import SimpleNamespace

import pytest

from cortex_runtime.acp.orchestrating_runtime import EngineTurnFailed, OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.engine.client import EngineProtocolError
from cortex_runtime.engine.retry_policy import RetryWindow, streak_start, windowed_retry
from cortex_runtime.orchestration import wait_notices
from cortex_runtime.orchestration.recovery import retry_lost
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.errors import PortableError, PortableErrorCategory
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace


# --- the policy ----------------------------------------------------------------

def test_delays_double_from_five_seconds_and_never_exceed_a_minute():
    failures, delays = [], []
    for second in range(0, 400, 50):
        failures.append(float(second))
        decision = windowed_retry(failures, float(second))
        assert decision.may_retry
        delays.append(decision.delay_seconds)
    assert delays == [5, 10, 20, 40, 60, 60, 60, 60]
    assert [windowed_retry(failures[:n], failures[n - 1]).retry_number for n in (1, 2, 3)] == [1, 2, 3]


def test_work_stops_only_once_failures_kept_repeating_for_ten_minutes():
    failures = [0.0, 60.0, 120.0, 180.0, 240.0, 300.0, 360.0, 420.0, 480.0, 540.0]
    assert windowed_retry(failures, 540.0).may_retry
    failures.append(600.0)
    stopped = windowed_retry(failures, 600.0)
    assert not stopped.may_retry and stopped.delay_seconds == 0 and stopped.elapsed_seconds == 600


def test_a_failure_long_after_the_previous_one_starts_a_new_streak():
    failures = [0.0, 30.0, 30.0 + 601.0]
    assert streak_start(failures) == 2
    decision = windowed_retry(failures, failures[-1])
    assert decision.may_retry and decision.retry_number == 1 and decision.delay_seconds == 5


# --- coordinator launch ---------------------------------------------------------

class FlakyCoordinatorOpenEngine(FakeSessionRuntime):
    """Coordinator ``open()`` raises ``failure()`` while armed."""

    def __init__(self, store, router, failure, failures):
        super().__init__(store, router)
        self.failure, self.failures, self.coordinator_opens = failure, failures, 0

    def open(self, workspace_binding_id, launch, instructions):
        if not launch.get("assignment_id"):
            self.coordinator_opens += 1
            if self.failures > 0:
                self.failures -= 1
                raise self.failure()
        return super().open(workspace_binding_id, launch, instructions)


def _router(coordinator_behavior=None):
    def default(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "Understood.")

    def router(store, channel, session_ref, launch, instructions):
        return (coordinator_behavior or default) if channel and channel.role == "coordinator" else None

    return router


def _runtime(tmp_path, *, failure=lambda: EngineProtocolError(-32603, "Internal error", None), failures=0,
             window=RetryWindow(first_delay_seconds=0.01, max_delay_seconds=0.02), router=None):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    engine = FlakyCoordinatorOpenEngine(store, router or _router(), failure, failures)
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                   allow_unqualified_delivery=True, wait_timeout_seconds=30.0)
    runtime._engine_retry_window = window
    return runtime, engine, store, task.task_id


def _chat():
    chunks = []
    return chunks, lambda text: chunks.append(text) or True


def test_a_coordinator_engine_that_fails_to_start_is_retried_and_each_retry_is_shown(tmp_path):
    runtime, engine, _, task_id = _runtime(tmp_path, failures=2)
    chunks, emit = _chat()

    assert runtime.start_prompt(task_id, "s1", "Hello.", emit, threading.Event()) == StopReason.END_TURN

    assert engine.coordinator_opens == 3
    text = "".join(chunks)
    assert "The engine did not start (EngineProtocolError). Retry 1 in 0 s…" in text
    assert "Retry 2 in 0 s…" in text
    assert "The engine started after 2 retries." in text


def test_the_coordinator_launch_stops_once_failures_kept_repeating_for_the_window(tmp_path):
    runtime, engine, _, task_id = _runtime(
        tmp_path, failures=10_000, window=RetryWindow(first_delay_seconds=0.02, max_delay_seconds=0.05,
                                                      window_seconds=0.2))
    chunks, emit = _chat()

    with pytest.raises(EngineProtocolError) as failure:
        runtime.start_prompt(task_id, "s1", "Hello.", emit, threading.Event())

    assert failure.value.cortex_launch_stage == "engine_launch"
    assert engine.coordinator_opens >= 3
    text = "".join(chunks)
    assert "Retry 1 in" in text
    assert f"retries). Stopping." in text and f"; {engine.coordinator_opens - 1} retries" in text


def test_a_non_transient_launch_failure_is_not_retried(tmp_path):
    refusal = lambda: PortableError(category=PortableErrorCategory.VALIDATION, code="engine_launch_command_missing",
                                    remediation="No launch_command.", retry_eligible=False)
    runtime, engine, _, task_id = _runtime(tmp_path, failure=refusal, failures=5)
    chunks, emit = _chat()

    with pytest.raises(PortableError):
        runtime.start_prompt(task_id, "s1", "Hello.", emit, threading.Event())

    assert engine.coordinator_opens == 1
    assert "Retry" not in "".join(chunks)


def test_an_authentication_failure_is_retried_only_once(tmp_path):
    runtime, engine, _, task_id = _runtime(
        tmp_path, failure=lambda: EngineProtocolError(-32000, "Authentication required", None), failures=5)

    with pytest.raises(StoreError, match="engine_authentication_required.*codex login"):
        runtime.start_prompt(task_id, "s1", "Hello.", lambda _t: True, threading.Event())

    assert engine.coordinator_opens == 2


def test_a_cancel_during_the_retry_delay_ends_the_wait(tmp_path):
    runtime, engine, _, task_id = _runtime(tmp_path, failures=10_000,
                                           window=RetryWindow(first_delay_seconds=30, max_delay_seconds=30))
    cancel = threading.Event()
    threading.Timer(0.3, cancel.set).start()
    started = time.monotonic()

    with pytest.raises(EngineProtocolError):
        runtime.start_prompt(task_id, "s1", "Hello.", lambda _t: True, cancel)

    assert time.monotonic() - started < 10
    assert engine.coordinator_opens == 1


# --- coordinator turn ------------------------------------------------------------

def test_a_coordinator_turn_that_failed_before_answering_is_resent(tmp_path):
    def behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            raise EngineTurnFailed("engine_error")
        return PromptOutcome(StopReason.END_TURN, "Understood.")

    runtime, engine, store, task_id = _runtime(tmp_path, router=_router(behavior))
    chunks, emit = _chat()

    runtime.start_prompt(task_id, "s1", "Hello.", emit, threading.Event())

    assert len(engine.prompt_calls) >= 2
    assert "The coordinator's engine failed before answering (engine_error). Retry 1 in 0 s…" in "".join(chunks)
    with store.connection() as db:
        blockers = [row[0] for row in db.execute("SELECT blocker FROM runtime_tasks WHERE task_id=?", (task_id,))]
    assert not any(blocker and blocker.startswith("engine_failure") for blocker in blockers)


def test_a_rate_limited_coordinator_turn_is_not_resent(tmp_path):
    def behavior(store, channel, session_ref, message, call_index):
        raise EngineTurnFailed("route_unavailable:rate_limited")

    runtime, engine, _, task_id = _runtime(tmp_path, router=_router(behavior))
    chunks, emit = _chat()

    runtime.start_prompt(task_id, "s1", "Hello.", emit, threading.Event())

    assert len(engine.prompt_calls) == 1
    assert "Retry" not in "".join(chunks)


# --- workers ---------------------------------------------------------------------

@pytest.fixture()
def executing(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="one"), "Implement feature", "owner")
    machine = TaskMachine(store)
    machine.create(task.task_id, "Implement feature")
    channel = SimpleNamespace(task_id=task.task_id, channel_id="coordinator", role="coordinator")
    machine.submit_plan(channel, dict(
        tasks=[dict(id="N0", role="backend-dev", objective="Implement it", dependencies=[], scope=["src/**"])],
        verification=[["python3", "-B", "-m", "pytest", "-q"]]), "p")
    machine.move(task.task_id, "EXECUTING")
    runtime = SimpleNamespace(_store=store, _engine=SimpleNamespace(
        lookup_owned=lambda attempt_id: "absent", terminate_owned=lambda attempt_id: None),
        _engine_retry_window=RetryWindow(first_delay_seconds=0.4, max_delay_seconds=0.4))
    return store, machine, channel, runtime


def _engine_failed(store, task_id, node):
    delegation_events.append_event(
        store, task_id, "assignment_failed", f"assignment_failed:{node['assignment_ref']}",
        dict(error="engine_refused_session", failure_class="engine_failure"), origin="runtime",
        assignment_ref=node["assignment_ref"],
    )


def test_a_worker_engine_failure_waits_out_its_delay_and_the_chat_shows_it(executing):
    store, machine, channel, runtime = executing
    node = machine.claim_ready(channel.task_id, channel.channel_id)[0]
    _engine_failed(store, channel.task_id, node)

    retry_lost(runtime, channel.task_id, node)

    with store.connection() as db:
        retried = json.loads(db.execute(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='ASSIGNMENT_RETRIED'",
            (channel.task_id,)).fetchone()[0])
    assert retried["infrastructure_retry"] == 1 and retried["failure_class"] == "engine_failure"
    assert retried["retry_not_before"] == pytest.approx(retried["failed_at"] + 0.4)
    # Held while the delay runs, with a card naming the retry.
    assert machine.claim_ready(channel.task_id, channel.channel_id) == []
    with store.connection() as db:
        waits = wait_notices.current_waits(db, channel.task_id)
        (wait,) = waits.values()
        title, text = wait_notices.render(db, channel.task_id, wait, "en")
    assert wait["reason"] == "retry_backoff"
    assert title == "Retry 1: N0 — the worker's engine failed"
    assert "(engine_refused_session). Retry 1 starts at" in text
    time.sleep(0.45)
    assert machine.claim_ready(channel.task_id, channel.channel_id)[0]["node_id"] == "N0"


def test_worker_engine_failures_do_not_use_up_the_ordinary_retries(executing):
    store, machine, channel, runtime = executing
    runtime._engine_retry_window = RetryWindow(first_delay_seconds=0, max_delay_seconds=0)
    for _ in range(5):
        node = machine.claim_ready(channel.task_id, channel.channel_id)[0]
        _engine_failed(store, channel.task_id, node)
        retry_lost(runtime, channel.task_id, node)
    node = machine.claim_ready(channel.task_id, channel.channel_id)[0]
    delegation_events.append_event(
        store, channel.task_id, "assignment_failed", f"assignment_failed:{node['assignment_ref']}",
        dict(error="worker_ended_without_result"), origin="runtime", assignment_ref=node["assignment_ref"])
    retry_lost(runtime, channel.task_id, node)


def test_worker_engine_failures_stop_once_they_kept_repeating_for_the_window(executing):
    store, machine, channel, runtime = executing
    runtime._engine_retry_window = RetryWindow(first_delay_seconds=0, max_delay_seconds=0, window_seconds=0.3)
    with pytest.raises(StoreError, match="worker_retry_limit:engine_refused_session"):
        for _ in range(10):
            node = machine.claim_ready(channel.task_id, channel.channel_id)[0]
            _engine_failed(store, channel.task_id, node)
            retry_lost(runtime, channel.task_id, node)
            time.sleep(0.12)
