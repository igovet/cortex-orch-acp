"""Regressions for coordinator handoff boundaries and dispatch failures.

These tests use the in-process engine fake only.  They exercise the runtime's
event/turn boundary without a paid engine call.
"""
from __future__ import annotations

import threading
import time
import uuid
import json

import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(
        store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())),
        "Coordinate two workers.", "owner-1",
    )
    return store, result.task_id


def _runtime(store, router, **kwargs):
    engine = FakeSessionRuntime(store, router)
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        allow_unqualified_delivery=True,
        wait_timeout_seconds=kwargs.pop("wait_timeout_seconds", 0.2),
        **kwargs,
    )
    return runtime, engine










@pytest.mark.parametrize("mode", ["streamed", "fallback", "exception"])
def test_engine_message_boundary_follows_text_before_the_next_continuation(tmp_path, mode):
    store, task_id = _task(tmp_path)
    trace = []

    def coordinator(store, channel, session_ref, message, call_index, *, on_delta):
        trace.append(("start", call_index))
        if mode != "fallback":
            on_delta("Handing off.")
        if mode == "exception":
            raise RuntimeError("fixture engine failure")
        return PromptOutcome(StopReason.END_TURN, "Handing off.")

    runtime, _ = _runtime(store, lambda *_: coordinator)
    try:
        runtime.start_prompt(
            task_id, "s1", "Do the work.", lambda text: trace.append(("text", text)),
            threading.Event(), emit_display_update=lambda update: trace.append(("display", update)),
        )
        boundaries = [index for index, (kind, value) in enumerate(trace)
                      if kind == "display" and value.get("messageId", "").startswith("__cortex_message_boundary__:")]
        starts = [index for index, (kind, _) in enumerate(trace) if kind == "start"]
        assert boundaries and len(boundaries) == len(starts)
        for number, index in enumerate(boundaries):
            assert trace[index - 1][0] == "text"
            assert trace[index][1]["content"] == {"type": "text", "text": ""}
            assert trace[index][1]["sessionUpdate"] == "agent_message_chunk"
            if number + 1 < len(starts):
                assert index < starts[number + 1]
        assert len({trace[index][1]["messageId"] for index in boundaries}) == len(boundaries)
    finally:
        runtime.shutdown()


def test_worker_loop_notice_does_not_hold_worker_cards_until_the_task_ends(tmp_path, monkeypatch):
    # An ignored-hint notice is written while workers run, with no engine turn
    # after it. Without a boundary every later worker card queued behind it.
    from cortex_runtime.orchestration import route_notices
    from cortex_runtime.orchestration.state import TaskMachine
    from runtime_scenarios import publish_fixture_report

    store, task_id = _task(tmp_path)
    trace, lock, released = [], threading.Lock(), []
    notices = [["Route notice."]]
    monkeypatch.setattr(route_notices, "ignored_hint_notices", lambda *_: notices.pop() if notices else [])

    def record(kind):
        def emit(value):
            with lock:
                trace.append((kind, value))
            return True
        return emit

    def boundary(entry):
        return entry[0] == "display" and entry[1].get("messageId", "").startswith("__cortex_message_boundary__:")

    def notice_index():
        return next((i for i, (kind, value) in enumerate(trace) if kind == "text" and "Route notice." in value), None)

    def behavior(store, channel, session_ref, message, call_index):
        if channel.role == "coordinator":
            if TaskMachine(store).context(task_id)["state"] == "PLANNING":
                mcp_tools.dispatch(store, channel, "plan_submit", dict(plan=dict(
                    tasks=[dict(id="inspect", role="backend-dev", objective="Inspect", dependencies=[],
                                scope=["."], model="auto")],
                    verification=[["python3", "-B", "-c", "pass"]]), request_key="plan"))
            else:
                mcp_tools.dispatch(store, channel, "synthesis_submit", dict(text="Done."))
            return PromptOutcome(StopReason.END_TURN, "")
        with store.connection() as db:
            stage = db.execute("SELECT n.stage FROM runtime_nodes n JOIN delegation_provisioning p "
                               "ON p.idempotency_key=n.assignment_ref WHERE p.attempt_id=?",
                               (channel.attempt_id,)).fetchone()[0]
        if stage == "execution":
            # The worker is still running: its card must not wait for it.
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline and not released:
                with lock:
                    index = notice_index()
                    if index is not None and any(boundary(entry) for entry in trace[index + 1:]):
                        released.append(True)
                time.sleep(0.02)
        extra = dict(verdict="APPROVE") if stage == "review" else {}
        mcp_tools.dispatch(store, channel, "result_submit", dict(
            report=publish_fixture_report(store, channel), summary="Observed result", checks=[],
            request_key="result-" + stage, **extra))
        return PromptOutcome(StopReason.END_TURN, "")

    engine = FakeSessionRuntime(store, lambda *_: behavior, connection_id="cn_notice_boundary", capacity_limit=4)
    runtime = OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                   allow_unqualified_delivery=True)
    try:
        runtime.start_prompt(task_id, "s1", "Inspect it.", record("text"), threading.Event(),
                             emit_tool_call=record("tool"), emit_display_update=record("display"))
    finally:
        runtime.shutdown()
    assert released == [True], "the notice kept its message open while the worker ran"
    index = notice_index()
    later_cards = [i for i, (kind, value) in enumerate(trace) if i > index and kind == "tool"
                   and value.get("tool_call_id", "").startswith("worker:")]
    assert later_cards, "a worker card follows the notice"
    # The marker after the notice is the empty, uniquely identified kind the
    # provider consumes; it releases cards queued behind the notice.
    first_boundary = next(i for i in range(index + 1, len(trace)) if boundary(trace[i]))
    assert trace[first_boundary][1]["content"] == {"type": "text", "text": ""}
    assert trace[first_boundary][1]["sessionUpdate"] == "agent_message_chunk"


def test_successful_delegate_tail_is_silent_but_later_tool_answer_is_visible(tmp_path):
    store, task_id = _task(tmp_path)
    def coordinator(store, channel, session_ref, message, call_index, *, on_tool_call, on_delta):
        on_tool_call("d1", "other", "mcp__cortex__delegate", "completed")
        on_delta("I'll wait for the worker.")
        on_delta("\n\n")
        on_tool_call("d2", "other", "mcp.cortex.delegate", "completed")
        on_delta("I'll check back shortly.")
        on_tool_call("r1", "other", "mcp.cortex.read_report", "completed")
        return PromptOutcome(StopReason.END_TURN, "The report found a concrete defect.")
    runtime, _ = _runtime(store, lambda *_: coordinator)
    output = []
    runtime.start_prompt(task_id, "s1", "Do the work.", output.append, threading.Event())
    assert "wait for" in "".join(output)
    assert "check back" in "".join(output)
    assert "concrete defect" in "".join(output)
    runtime.shutdown()
