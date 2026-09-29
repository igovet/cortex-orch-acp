"""A coordinator's text after its own accepted question never reaches the user.

Session bb142018 (investigation r_5c2679466a0e): the coordinator wrote the
details, asked with user_question_request, then wrote a full rewrite of the
same details in the same turn, and the runtime forwarded both. The prompt-only
rule against a recap did not hold, so the controller now withholds text that
follows the committed question for the rest of that engine turn.
"""
from __future__ import annotations

import threading
import uuid
from types import SimpleNamespace

import pytest

import cortex_runtime.orchestration.controller as controller_module
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.acp.questions_store import answer
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.orchestration.questions import QUESTION_RECEIPT_NEXT, pending, request
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace

DETAILS = "Details before the question with the evidence and both options."
REWRITE = "A second full version of the same details and options."
LATER = "Another tool is needed after all."
AFTER_ANSWER = "Planning the approved option now."
QUESTION = "Approve the plan above?"


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    goal = "Decide the next step with the user."
    task = bootstrap_task(store, SourceEvent(origin="test", event_id=str(uuid.uuid4())), goal, "question-owner")
    TaskMachine(store).create(task.task_id, goal)
    channel = SimpleNamespace(task_id=task.task_id, channel_id="question-coordinator", role="coordinator")
    return store, task.task_id, channel


def _ask(store, channel):
    return request(store, channel, dict(question=QUESTION, options=["Approve", "Finish"], request_key="q-approve"))


def _runtime(store, turns):
    def router(_store, _channel, _session_ref, _launch, _instructions):
        def behavior(_store, _channel, _session_ref, _message, call_index,
                     *, on_delta=None, on_tool_call=None, cancel_event=None):
            return turns[call_index](on_delta, on_tool_call, cancel_event)
        return behavior

    engine = FakeSessionRuntime(store, router, emit_outcome_text_as_delta=False)
    return OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                allow_unqualified_delivery=True)


def _run_turns(store, task_id, turns, prompts):
    runtime = _runtime(store, turns)
    shown, results = [], []
    try:
        runtime._ensure_coordinator(task_id)
        for prompt in prompts:
            emitted = []
            if callable(prompt):
                prompt()
                prompt = "Approve"
            results.append(runtime.start_prompt(task_id, "question-session", prompt, emitted.append,
                                                threading.Event()))
            shown.append("".join(emitted))
    finally:
        runtime.shutdown()
    return shown, results


def test_question_receipt_tells_the_model_to_end_the_turn(tmp_path):
    store, _task_id, channel = _task(tmp_path)
    receipt = _ask(store, channel)
    assert receipt["state"] == "WAITING_USER"
    assert receipt["next"] == QUESTION_RECEIPT_NEXT
    assert "End this turn now without writing any further text" in receipt["next"]
    # An exact retry returns the same stored receipt, hint included.
    assert _ask(store, channel) == receipt


def test_text_after_the_accepted_question_is_withheld_while_the_details_are_shown(tmp_path):
    store, task_id, channel = _task(tmp_path)

    def turn(on_delta, on_tool_call, _cancel):
        on_delta(DETAILS)
        on_tool_call("tc-question", "other", "user_question_request", "in_progress")
        assert _ask(store, channel)["next"] == QUESTION_RECEIPT_NEXT
        on_tool_call("tc-question", None, None, "completed")
        on_delta(REWRITE)
        return PromptOutcome(StopReason.END_TURN, DETAILS + REWRITE)

    (shown,), (result,) = _run_turns(store, task_id, [turn], ["Continue."])
    assert result == StopReason.END_TURN
    assert shown.count(DETAILS) == 1
    assert REWRITE not in shown
    # The runtime's own question rendering still reaches the user.
    assert QUESTION in shown
    assert TaskMachine(store).get_state(task_id) == "WAITING_USER"


def test_text_read_after_the_commit_has_no_proven_pre_question_boundary(tmp_path):
    store, task_id, channel = _task(tmp_path)

    def turn(on_delta, on_tool_call, _cancel):
        # The MCP call committed before the stream reader reached the text
        # the model wrote ahead of it; the call's own lifecycle follows.
        _ask(store, channel)
        on_delta(DETAILS)
        on_tool_call("tc-question", "other", "user_question_request", "in_progress")
        on_tool_call("tc-question", None, None, "completed")
        on_delta(REWRITE)
        return PromptOutcome(StopReason.END_TURN, DETAILS + REWRITE)

    (shown,), _ = _run_turns(store, task_id, [turn], ["Continue."])
    # These observations cannot distinguish delayed pre-question details from
    # a post-question rewrite. Do not infer ordering from a late lifecycle id.
    assert DETAILS not in shown
    assert REWRITE not in shown
    assert QUESTION in shown


@pytest.mark.parametrize("callback", [
    ("tc-question", "other", "user_question_request", "in_progress"),
    ("tc-question", None, None, "completed"),
    ("tc-unknown", "other", "Tool call", "in_progress"),
])
def test_delayed_first_lifecycle_callback_does_not_release_post_question_text(tmp_path, callback):
    store, task_id, channel = _task(tmp_path)

    def turn(on_delta, on_tool_call, _cancel):
        on_delta(DETAILS)
        _ask(store, channel)
        on_delta(REWRITE)
        on_tool_call(*callback)
        return PromptOutcome(StopReason.END_TURN, DETAILS + REWRITE)

    (shown,), (result,) = _run_turns(store, task_id, [turn], ["Continue."])
    assert result == StopReason.END_TURN
    assert shown.count(DETAILS) == 1
    assert REWRITE not in shown
    assert QUESTION in shown
    assert TaskMachine(store).get_state(task_id) == "WAITING_USER"


def test_final_only_text_after_question_is_not_reemitted(tmp_path):
    store, task_id, channel = _task(tmp_path)

    def turn(_on_delta, _on_tool_call, _cancel):
        _ask(store, channel)
        return PromptOutcome(StopReason.END_TURN, REWRITE)

    (shown,), _ = _run_turns(store, task_id, [turn], ["Continue."])
    assert REWRITE not in shown
    assert QUESTION in shown


def test_final_only_text_without_question_is_still_shown(tmp_path, monkeypatch):
    store, task_id, _channel = _task(tmp_path)
    monkeypatch.setattr(controller_module, "_stop_turn", lambda *_args: StopReason.CANCELLED)

    def turn(_on_delta, _on_tool_call, cancel_event):
        cancel_event.set()
        return PromptOutcome(StopReason.CANCELLED, DETAILS)

    (shown,), (result,) = _run_turns(store, task_id, [turn], ["Continue."])
    assert result == StopReason.CANCELLED
    assert shown.count(DETAILS) == 1


def test_a_late_update_of_the_question_call_does_not_release_the_rewrite(tmp_path):
    store, task_id, channel = _task(tmp_path)

    def turn(on_delta, on_tool_call, _cancel):
        on_delta(DETAILS)
        on_tool_call("tc-question", "other", "user_question_request", "in_progress")
        _ask(store, channel)
        on_delta(REWRITE)
        # The same call's terminal update arrives after the text.
        on_tool_call("tc-question", None, None, "completed")
        return PromptOutcome(StopReason.END_TURN, DETAILS + REWRITE)

    (shown,), _ = _run_turns(store, task_id, [turn], ["Continue."])
    assert shown.count(DETAILS) == 1
    assert REWRITE not in shown


def test_new_tool_call_after_the_question_does_not_release_text(tmp_path):
    store, task_id, channel = _task(tmp_path)

    def turn(on_delta, on_tool_call, _cancel):
        on_delta(DETAILS)
        on_tool_call("tc-question", "other", "user_question_request", "in_progress")
        _ask(store, channel)
        on_tool_call("tc-question", None, None, "completed")
        on_delta(LATER)
        on_tool_call("tc-read", "read", "task_get_context", "in_progress")
        on_tool_call("tc-read", None, None, "completed")
        on_delta(REWRITE)
        return PromptOutcome(StopReason.END_TURN, DETAILS + LATER + REWRITE)

    (shown,), _ = _run_turns(store, task_id, [turn], ["Continue."])
    assert shown.count(DETAILS) == 1
    assert LATER not in shown
    assert REWRITE not in shown


def test_text_in_the_turn_after_the_answer_is_shown(tmp_path, monkeypatch):
    store, task_id, channel = _task(tmp_path)
    monkeypatch.setattr(controller_module, "_stop_turn", lambda *_args: StopReason.CANCELLED)

    def first(on_delta, on_tool_call, _cancel):
        on_delta(DETAILS)
        on_tool_call("tc-question", "other", "user_question_request", "in_progress")
        _ask(store, channel)
        on_tool_call("tc-question", None, None, "completed")
        on_delta(REWRITE)
        return PromptOutcome(StopReason.END_TURN, DETAILS + REWRITE)

    def second(on_delta, _on_tool_call, cancel_event):
        on_delta(AFTER_ANSWER)
        cancel_event.set()
        return PromptOutcome(StopReason.CANCELLED, AFTER_ANSWER)

    def answer_question():
        assert answer(store, pending(store, task_id)["question_id"], "Approve", "text")

    (first_shown, second_shown), _ = _run_turns(store, task_id, [first, second], ["Continue.", answer_question])
    assert REWRITE not in first_shown
    assert AFTER_ANSWER in second_shown
