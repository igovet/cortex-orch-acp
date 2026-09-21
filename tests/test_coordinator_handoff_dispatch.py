"""Regressions for coordinator handoff boundaries and dispatch failures.

These tests use the in-process engine fake only.  They exercise the runtime's
event/turn boundary without a paid engine call.
"""
from __future__ import annotations

import threading
import time
import uuid
import json

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
