"""Paseo cancel+prompt pairing.

Paseo sends ``session/cancel`` and then, once that turn settles, a new
``session/prompt`` for a follow-up message (CLI ``send``, Desktop "Send
now"). Such a pair on the same session is steering: the turn ends, running
workers keep running and the prompt continues the task. A lone cancel still
pauses the task once the bounded grace window expires. In-process fakes
only: no real engine, network or paid model call.
"""
from __future__ import annotations

import json
import tempfile
import threading
import time
import uuid
from pathlib import Path

from cortex_runtime.acp.agent import Agent
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.workspace_isolation import IsolatedSnapshotWorkspaceProvisioner
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime


class _Scenario:
    """One task whose first worker blocks until released or cancelled."""

    def __init__(self, tmp_path, grace):
        from runtime_scenarios import propose_inspection, publish_result

        self.store = Store(tmp_path / ".cortex", project_root=tmp_path)
        self.task_id = bootstrap_task(
            self.store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1",
        ).task_id
        self.entered = threading.Event()
        self.release = threading.Event()
        self.worker_cancel = []
        self.planning = []

        def router(store, channel, *_):
            def behavior(store, channel, _ref, message, _index, cancel_event=None):
                if channel.role == "coordinator":
                    context = mcp_tools.dispatch(store, channel, "task_get_context", {})
                    if context["state"] == "PLANNING":
                        self.planning.append(message)
                        propose_inspection(store, channel)
                    else:
                        mcp_tools.dispatch(store, channel, "synthesis_submit", dict(text="Reviewed and verified"))
                    return PromptOutcome(StopReason.END_TURN, "Coordinator text")
                if not self.worker_cancel:
                    self.worker_cancel.append(cancel_event)
                    self.entered.set()
                    deadline = time.monotonic() + 20
                    while (not self.release.is_set() and not cancel_event.is_set()
                           and time.monotonic() < deadline):
                        time.sleep(0.01)
                    if not self.release.is_set():
                        return PromptOutcome(StopReason.CANCELLED, "")
                return publish_result(store, channel)
            return behavior

        self.engine = FakeSessionRuntime(self.store, router)
        self.runtime = OrchestratingRuntime(
            self.store, self.engine,
            IsolatedSnapshotWorkspaceProvisioner(self.store, str(self.store.project_root)),
            FakeConfigSnapshotSource(), allow_unqualified_delivery=True, wait_timeout_seconds=15.0,
            host_cancel_grace_seconds=grace,
        )
        self.runtime._cleanup_isolated_workspace_if_integrated = lambda task_id, attempt_id: None
        self.outcomes = []
        self.threads = []

    def prompt(self, session_id, text, cancel_event, surface="Paseo", emit=None, emit_tool_call=None):
        thread = threading.Thread(target=lambda: self.outcomes.append(self.runtime.start_prompt(
            self.task_id, session_id, text, emit or (lambda _text: True), cancel_event, client_surface=surface,
            emit_tool_call=emit_tool_call,
        )))
        thread.start()
        self.threads.append(thread)
        return thread

    def host_cancel(self, session_id, cancel_event):
        # acp.agent.Agent._session_cancel order: runtime first, then the event.
        self.runtime.cancel(session_id)
        cancel_event.set()

    def state(self):
        return TaskMachine(self.store).context(self.task_id)

    def runtime_events(self):
        with self.store.connection() as db:
            return [row[0] for row in db.execute(
                "SELECT type FROM runtime_events WHERE task_id=?", (self.task_id,))]

    def task_events(self):
        with self.store.connection() as db:
            return [(row[0], json.loads(row[1] or "{}")) for row in db.execute(
                "SELECT event_type, payload_json FROM task_events WHERE task_id=? ORDER BY id",
                (self.task_id,))]

    def progress_kinds(self):
        return [payload.get("kind") for kind, payload in self.task_events() if kind == "progress"]

    def wait_for(self, predicate, timeout=10.0):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if predicate():
                return True
            time.sleep(0.02)
        return predicate()

    def close(self):
        self.release.set()
        for thread in self.threads:
            thread.join(10)
        self.runtime.shutdown()


def _cancel_first_turn(scenario, surface="Paseo"):
    first = threading.Event()
    turn = scenario.prompt("s1", "Inspect", first, surface)
    assert scenario.entered.wait(5)
    scenario.host_cancel("s1", first)
    turn.join(10)
    assert not turn.is_alive()
    assert scenario.outcomes == [StopReason.CANCELLED]


def test_cancel_followed_by_prompt_on_same_session_is_steering(tmp_path):
    scenario = _Scenario(tmp_path, grace=5.0)
    try:
        _cancel_first_turn(scenario)
        # The turn ended without pausing the task or cancelling its worker.
        context = scenario.state()
        assert context["state"] == "EXECUTING"
        assert context["nodes"][0]["status"] == "running"
        assert not scenario.worker_cancel[0].is_set()
        assert "TURN_INTERRUPTED" not in scenario.runtime_events()

        second = scenario.prompt("s1", "Steering: also check the title", threading.Event())
        assert scenario.wait_for(lambda: "host_cancel_followed_by_prompt" in scenario.progress_kinds())
        scenario.release.set()
        second.join(20)
        assert not second.is_alive()
        assert scenario.outcomes == [StopReason.CANCELLED, StopReason.END_TURN]
        assert scenario.state()["state"] == "COMPLETED"
        event_types = [kind for kind, _payload in scenario.task_events()]
        assert "worker_result" in event_types
        assert "assignment_cancelled" not in event_types
        assert "host_cancel_paused" not in scenario.progress_kinds()
        assert "TURN_INTERRUPTED" not in scenario.runtime_events()
        # The steering reached the next planning turn.
        assert any("also check the title" in message for message in scenario.planning[1:])
    finally:
        scenario.close()


def test_steering_keeps_the_running_worker_card_open_until_its_real_outcome(tmp_path):
    """The runtime half of a steered worker card: across cancel -> prompt
    the runtime never settles the still-running worker's card, and that same
    card later carries the worker's real result. Paseo's ACP bridge settles
    open tool calls itself when a turn ends; the plugin fences that in
    integrations/paseo/server/worker-card-fence.ts."""
    scenario = _Scenario(tmp_path, grace=5.0)
    cards = []

    def emit_card(payload):
        cards.append(dict(payload))
        return True

    def worker_cards():
        return [card for card in list(cards) if str(card.get("tool_call_id", "")).startswith("worker:")]

    try:
        first = threading.Event()
        turn = scenario.prompt("s1", "Inspect", first, emit_tool_call=emit_card)
        assert scenario.entered.wait(5)
        assert scenario.wait_for(lambda: any(card.get("status") == "in_progress" for card in worker_cards()))
        scenario.host_cancel("s1", first)
        turn.join(10)
        assert scenario.outcomes == [StopReason.CANCELLED]
        [card_id] = {card["tool_call_id"] for card in worker_cards()}
        second = scenario.prompt("s1", "Steering: also check the title", threading.Event(), emit_tool_call=emit_card)
        assert scenario.wait_for(lambda: "host_cancel_followed_by_prompt" in scenario.progress_kinds())
        assert all(card.get("status") not in {"completed", "failed"} for card in worker_cards())
        assert not scenario.worker_cancel[0].is_set()
        scenario.release.set()
        second.join(20)
        assert not second.is_alive()
        assert scenario.outcomes == [StopReason.CANCELLED, StopReason.END_TURN]
        # The same card settles once with the real result; a settled card may
        # be refined (outcome label, result) but never changes its status.
        # (The applied steering's own planning turn may delegate new work,
        # which gets new cards.)
        settled = {card["status"] for card in worker_cards()
                   if card["tool_call_id"] == card_id and card.get("status") in {"completed", "failed"}}
        assert settled == {"completed"}
    finally:
        scenario.close()


def test_lone_cancel_pauses_the_task_when_the_grace_window_expires(tmp_path):
    scenario = _Scenario(tmp_path, grace=0.3)
    try:
        _cancel_first_turn(scenario)
        # The pause runs on the grace timer thread: the task turns RECOVERING
        # (policy.interrupt) before the running assignments are cancelled, so
        # wait for the progress event recorded once the whole pause is done.
        assert scenario.wait_for(lambda: "host_cancel_paused" in scenario.progress_kinds(), timeout=30.0)
        assert scenario.state()["state"] == "RECOVERING"
        assert "TURN_INTERRUPTED" in scenario.runtime_events()
        assert scenario.worker_cancel[0].is_set()
        cancelled = [payload for kind, payload in scenario.task_events() if kind == "assignment_cancelled"]
        assert cancelled and cancelled[0]["reason"] == "turn_interrupted"
        assert "host_cancel_followed_by_prompt" not in scenario.progress_kinds()
    finally:
        scenario.close()


def test_prompt_from_another_session_applies_the_held_pause_first(tmp_path):
    scenario = _Scenario(tmp_path, grace=30.0)
    try:
        _cancel_first_turn(scenario)
        assert scenario.state()["state"] == "EXECUTING"
        scenario.prompt("s2", "Something else", threading.Event())
        assert scenario.wait_for(lambda: "TURN_INTERRUPTED" in scenario.runtime_events())
        assert scenario.wait_for(lambda: scenario.worker_cancel[0].is_set())
        assert scenario.wait_for(lambda: "host_cancel_paused" in scenario.progress_kinds())
        paused = [payload for kind, payload in scenario.task_events()
                  if kind == "progress" and payload.get("kind") == "host_cancel_paused"]
        assert paused[0]["reason"] == "resolved_before_next_turn"
        assert "host_cancel_followed_by_prompt" not in scenario.progress_kinds()
    finally:
        scenario.close()


def test_a_failed_deferred_pause_leaves_a_durable_trace(tmp_path):
    scenario = _Scenario(tmp_path, grace=0.3)

    def failing_pause(task_id, hold, reason):
        raise RuntimeError("pause failed")
    scenario.runtime._apply_cancel_pause = failing_pause
    try:
        _cancel_first_turn(scenario)
        assert scenario.wait_for(lambda: "host_cancel_pause_failed" in scenario.progress_kinds())
        [failed] = [payload for kind, payload in scenario.task_events()
                    if kind == "progress" and payload.get("kind") == "host_cancel_pause_failed"]
        assert failed["error"] == "RuntimeError" and failed["session_id"] == "s1"
        assert failed["reason"] == "grace_expired"
    finally:
        scenario.close()


def test_a_deferred_pause_is_announced_once_on_the_next_turn(tmp_path):
    """The cancelled turn's host emitters are closed, so the pause a lone
    cancel applies after it ended is said at the start of the next turn."""
    scenario = _Scenario(tmp_path, grace=0.3)
    try:
        _cancel_first_turn(scenario)
        assert scenario.wait_for(lambda: "host_cancel_paused" in scenario.progress_kinds(), timeout=30.0)
        emitted = []
        scenario.prompt("s1", "Continue", threading.Event(), emit=lambda text: emitted.append(text) or True)
        assert scenario.wait_for(lambda: any("Your earlier stop paused this task" in text for text in emitted))
        notes = [payload for kind, payload in scenario.task_events()
                 if kind == "progress" and payload.get("message_key") == "turn_paused_earlier"]
        assert len(notes) == 1
        assert scenario.task_id not in scenario.runtime._pending_pause_notices
    finally:
        scenario.close()


def test_an_answer_returning_early_after_a_held_cancel_is_steering(tmp_path):
    """A ``/cortex-answer`` on the cancelled session is a prompt there, even
    when it returns before reaching the coordinator (here: a question that
    is no longer open); the held cancel must not pause the task later."""
    from cortex_runtime.acp import questions_store

    scenario = _Scenario(tmp_path, grace=0.5)
    try:
        _cancel_first_turn(scenario)
        question = questions_store.create_if_absent(scenario.store, scenario.task_id, "stale", "Which one?")
        with scenario.store.connection() as db:
            db.execute("UPDATE pending_questions SET status='superseded' WHERE id=?", (question.id,))
            db.commit()
        outcome = scenario.runtime.answer_question(
            scenario.task_id, "s1", question, "First", lambda _text: True, threading.Event())
        assert outcome == StopReason.END_TURN
        assert "host_cancel_followed_by_prompt" in scenario.progress_kinds()
        time.sleep(1.0)  # past the grace window
        assert "host_cancel_paused" not in scenario.progress_kinds()
        assert not scenario.worker_cancel[0].is_set()
        assert scenario.state()["state"] == "EXECUTING"
    finally:
        scenario.close()


def test_non_paseo_cancel_still_pauses_at_once(tmp_path):
    scenario = _Scenario(tmp_path, grace=5.0)
    try:
        _cancel_first_turn(scenario, surface="Zed")
        assert scenario.state()["state"] == "RECOVERING"
        assert "TURN_INTERRUPTED" in scenario.runtime_events()
        assert scenario.worker_cancel[0].is_set()
        assert "host_cancel_paused" not in scenario.progress_kinds()
    finally:
        scenario.close()


def test_stop_supersedes_a_held_cancel(tmp_path):
    scenario = _Scenario(tmp_path, grace=30.0)
    try:
        _cancel_first_turn(scenario)
        assert not scenario.worker_cancel[0].is_set()
        scenario.runtime.stop_task(scenario.task_id, "s1", lambda _text: True)
        assert scenario.state()["state"] == "CANCELLED"
        assert scenario.worker_cancel[0].is_set()
    finally:
        scenario.close()


class _Transport:
    def __init__(self):
        self.responses = []
        self.notifications = []

    def send_response(self, message):
        self.responses.append(message)

    def send_notification(self, message, timeout=None):
        self.notifications.append(message)
        return True


class _OrderRuntime:
    """Records whether the turn's event was already set when cancel ran."""

    def __init__(self, event):
        self.event = event
        self.seen = []

    def start_prompt(self, task_id, session_id, content, emit_update, cancel_event):
        return StopReason.END_TURN

    def cancel(self, session_id):
        self.seen.append((session_id, self.event.is_set()))

    def shutdown(self):
        pass


def test_agent_lets_the_runtime_hold_the_cancel_before_setting_the_turn_event(tmp_path):
    home = Path(tempfile.mkdtemp(prefix="cortex-pairing-home-"))
    turn_event = threading.Event()
    runtime = _OrderRuntime(turn_event)
    agent = Agent(runtime, environ=dict(
        HOME=str(home), XDG_CONFIG_HOME=str(home / "config"), XDG_STATE_HOME=str(home / "state")))
    agent.client_info = {"name": "Paseo"}
    agent.bind_transport(_Transport())
    agent.initialized = True
    session_id = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))["sessionId"]
    agent.sessions[session_id]["active"] = turn_event

    agent._session_cancel({"sessionId": session_id})

    assert runtime.seen == [(session_id, False)]
    assert turn_event.is_set()
    assert agent.sessions[session_id].get("cancelling") is False
