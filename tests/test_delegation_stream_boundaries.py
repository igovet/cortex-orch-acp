"""Worker prose remains continuous across telemetry, but not tool calls."""

from __future__ import annotations

import threading

from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.portable.engine_contract import EngineEvent, EngineEventKind, StopReason, UsageBasis


class _Manager:
    def __init__(self, events):
        self.events = events

    def prompt_turn(self, session_ref, message, *, prompt_blocks=None):
        yield from self.events


def _prompt(events):
    adapter = object.__new__(EngineSessionManagerAdapter)
    adapter._lock = threading.Lock()
    adapter._managers = {"attempt": _Manager(events)}
    adapter._session_to_attempt = {"session": "attempt"}
    adapter._attempt_task = {}
    adapter._store = None
    fragments = []
    outcome = adapter.prompt("session", "test", threading.Event(), on_delta=fragments.append)
    return outcome.text, "".join(fragments)


def _delta(text):
    return EngineEvent(kind=EngineEventKind.TURN_DELTA, delta_text=text)


def _end():
    return EngineEvent(kind=EngineEventKind.TURN_END, stop_reason=StopReason.END_TURN)


def test_usage_and_hidden_display_do_not_split_worker_sentence():
    events = [
        _delta("The package stamp, "),
        EngineEvent(kind=EngineEventKind.USAGE_UPDATE, usage_basis=UsageBasis.ENGINE_REPORTED,
                    usage_values={"inputTokens": 12, "outputTokens": 5},
                    display_update={"sessionUpdate": "usage_update", "used": 17, "size": 200}),
        _delta("profile check, repeated focused test, and "),
        EngineEvent(kind=EngineEventKind.DISPLAY_UPDATE,
                    display_update={"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "thinking"}}),
        _delta("package test all passed."),
        _end(),
    ]
    expected = "The package stamp, profile check, repeated focused test, and package test all passed."
    assert _prompt(events) == (expected, expected)


def test_tool_call_still_separates_visible_stream_segments():
    events = [
        _delta("Before tool."),
        EngineEvent(kind=EngineEventKind.TOOL_CALL_OBSERVED, tool_call_ref="tool-1",
                    tool_call_kind="execute", tool_call_title="Shell", tool_call_status="completed"),
        _delta("After tool."),
        _end(),
    ]
    assert _prompt(events) == ("Before tool.After tool.", "Before tool.\n\nAfter tool.")
