"""A worker whose Cortex MCP server was lost mid-session.

Owner-reported worker session bb142018-c650-4756-8043-81fbf02882eb: the
worker's Cortex tools started answering ``Transport closed``; it could neither
submit its result nor ask a question, ended its turn, received the runtime's
same-session completion follow-up (which asks it to publish through those same
dead tools) and was then recorded as ``worker_ended_without_result``. The
broker now reports a crashed, previously ready server through readiness, so the
running turn fails as ``cortex_mcp_connection_failed:mcp_server_crashed``
(``tests/test_engine_broker.py``, ``tests/test_engine_session_manager.py``).
These tests pin what the runtime does with that failure: a named failure with
no same-session follow-up, only the runtime's bounded fresh-attempt retry,
never a retry over an uncertain action, and the durable result whenever one
was committed before the loss.
"""
from __future__ import annotations

import json
import threading
from unittest.mock import patch

from cortex_runtime.acp.orchestrating_runtime import EngineTurnFailed, OrchestratingRuntime, PromptOutcome
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.engine_contract import StopReason

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace
from test_transport_disconnect_recovery import _plan, _publish, _runtime_events, _task, _task_events

LOST = "cortex_mcp_connection_failed:mcp_server_crashed"
SESSION = "incoming"


class _ProcessKilled(BaseException):
    """The server died before a command's completion was retained."""


def _run(store, task_id, worker):
    worker_prompts = []
    planned = []

    def router(store, channel, session_ref, launch, instructions):
        def behavior(store, channel, ref, message, call_index):
            if channel.role == "coordinator":
                context = mcp_tools.dispatch(store, channel, "task_get_context", {})
                if context["state"] == "PLANNING" and not planned:
                    planned.append(True)
                    mcp_tools.dispatch(store, channel, "plan_submit",
                                       dict(plan=_plan(), request_key="plan-" + str(context["revision"])))
                    return PromptOutcome(StopReason.END_TURN, "Planned")
                mcp_tools.dispatch(store, channel, "user_question_request", dict(
                    question="The worker lost its Cortex channel. How should we continue?",
                    options=["Delegate again", "Stop"], request_key="after-loss-" + str(context["revision"])))
                return PromptOutcome(StopReason.END_TURN, "Asked the user")
            worker_prompts.append(message)
            return worker(store, channel)
        return behavior

    runtime = OrchestratingRuntime(store, FakeSessionRuntime(store, router), FakeWorkspace(),
                                   FakeConfigSnapshotSource(), allow_unqualified_delivery=True)
    try:
        runtime.start_prompt(task_id, SESSION, "Inspect part 0", lambda _text: True, threading.Event(),
                             emit_tool_call=lambda _update: True)
    finally:
        runtime.shutdown()
    return worker_prompts


def _same_session_followups(worker_prompts):
    return [prompt for prompt in worker_prompts if "Runtime completion follow-up" in prompt]


def test_lost_cortex_channel_is_named_and_gets_no_same_session_followup(tmp_path):
    store, task_id = _task(tmp_path)

    def worker(store, channel):
        raise EngineTurnFailed(LOST)

    worker_prompts = _run(store, task_id, worker)

    failures = _task_events(store, task_id, "assignment_failed")
    assert failures
    for failure in failures:
        assert failure["payload"]["error"] == f"engine_failure:{LOST}"
        # Never the silent-end classification, never fallback-eligible.
        assert failure["payload"]["failure_class"] == "indeterminate"
    assert not _task_events(store, task_id, "worker_result")
    # One native turn per attempt: the dead tools are never asked to publish.
    assert not _same_session_followups(worker_prompts)
    assert len(worker_prompts) == len(failures)
    # Only the runtime's bounded fresh-attempt retry (a new session and a new
    # Cortex MCP server), then the coordinator is told why.
    retries = _runtime_events(store, task_id, "ASSIGNMENT_RETRIED")
    assert len(retries) == 2 and {r["payload"]["observed_session"] for r in retries} == {"absent"}
    [returned] = _runtime_events(store, task_id, "RETURNED_TO_COORDINATOR")
    assert returned["payload"]["reason"] == f"worker_retry_limit:engine_failure:{LOST}"
    assert TaskMachine(store).context(task_id)["state"] == "WAITING_USER"


def test_lost_cortex_channel_never_retries_over_an_uncertain_action(tmp_path):
    store, task_id = _task(tmp_path)
    marker = tmp_path / "side-effect.log"
    command = ["python3", "-B", "-c", f"open({str(marker)!r}, 'a').write('ran\\n')"]

    def worker(store, channel):
        with patch("cortex_runtime.recovery.large_output_store.persist_output", side_effect=_ProcessKilled()):
            try:
                mcp_tools.dispatch(store, channel, "command_run", dict(argv=command, request_key="side-effect"))
            except _ProcessKilled:
                pass
        raise EngineTurnFailed(LOST)

    worker_prompts = _run(store, task_id, worker)

    assert marker.read_text() == "ran\n"
    assert len(worker_prompts) == 1
    assert not _runtime_events(store, task_id, "ASSIGNMENT_RETRIED")
    [returned] = _runtime_events(store, task_id, "RETURNED_TO_COORDINATOR")
    assert returned["payload"]["reason"] == "lost_worker_action_outcome_unknown"
    with store.connection() as db:
        [receipt] = [json.loads(row[0]) for row in db.execute(
            "SELECT result_json FROM runtime_requests WHERE task_id=? AND operation='command_run'", (task_id,))]
    assert receipt["complete"] is False


def test_lost_cortex_channel_after_the_result_committed_keeps_the_result(tmp_path):
    store, task_id = _task(tmp_path)
    receipts = []

    def worker(store, channel):
        report_id = _publish(store, channel, channel.attempt_id)
        arguments = dict(report=dict(report_ref=report_id), summary="Observed result", checks=[], request_key="result")
        receipts.append(mcp_tools.dispatch(store, channel, "result_submit", arguments))
        # The server died while answering: the engine never saw the receipt.
        # An exact retry with the same key replays it and adds nothing.
        receipts.append(mcp_tools.dispatch(store, channel, "result_submit", arguments))
        raise EngineTurnFailed(LOST)

    worker_prompts = _run(store, task_id, worker)

    assert len(worker_prompts) == 1
    assert receipts[0] == receipts[1]
    assert not _task_events(store, task_id, "assignment_failed")
    assert not _runtime_events(store, task_id, "ASSIGNMENT_RETRIED")
    [result] = _task_events(store, task_id, "worker_result")
    assert result["payload"]["summary"] == "Observed result"
    context = TaskMachine(store).context(task_id)
    assert [n["status"] for n in context["nodes"]] != ["failed"]
    assert result["payload"]["report_ref"] in json.dumps(context["nodes"])
