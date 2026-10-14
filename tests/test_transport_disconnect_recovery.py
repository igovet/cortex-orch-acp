"""A host transport disconnect is not a task outcome.

Paseo restarts the plugin process when a plugin update is applied; every open
Cortex session then loses its ACP connection (the host shows "Provider
connection closed").  The runtime must treat that as a transport event only:
completed work stays completed, interrupted workers are reconciled from
persisted state after restart, and an action whose outcome was never retained
is never repeated automatically.
"""
from __future__ import annotations

import json
import threading
from pathlib import Path
from unittest.mock import patch

import pytest

pytestmark = pytest.mark.usefixtures('unjailed_gateway_command')

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace

SESSION = "incoming"


class _ProcessKilled(BaseException):
    """Stands in for the worker's process dying mid-call (plugin update)."""


def _plan():
    return dict(tasks=[dict(id="N0", role="backend-dev", objective="Inspect part 0", dependencies=[],
                            scope=["src/part0/**"], model="auto")],
                verification=[["python3", "-B", "-c", "print('verified')"]])


def _publish(store, channel, key):
    draft = store.call("create_draft", dict(template="general", request_key="draft-" + key),
                       channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True)
    path = Path(draft["draft_path"])
    marker = path.read_bytes().split(b"\n\n", 1)[0] + b"\n\n"
    path.write_bytes(marker + b"# Report\n\nObserved result.\n")
    return store.call("write_report", dict(draft_id=draft["draft_id"], title="Result", summary="Observed result",
                                           author="worker", request_key="report-" + key),
                      channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True)["report_id"]


def _finishing_router(coordinator_messages=None, *, ask_user=False):
    """Coordinator plans (or synthesizes); a worker publishes without changes."""
    def router(store, channel, session_ref, launch, instructions):
        def behavior(store, channel, ref, message, call_index):
            if channel.role == "coordinator":
                if coordinator_messages is not None:
                    coordinator_messages.append(message)
                context = mcp_tools.dispatch(store, channel, "task_get_context", {})
                if context["state"] == "PLANNING":
                    if ask_user:
                        mcp_tools.dispatch(store, channel, "user_question_request", dict(
                            question="How should the interrupted action be reconciled?",
                            options=["Inspect first", "Stop"], request_key="reconcile-" + str(context["revision"])))
                        return PromptOutcome(StopReason.END_TURN, "Asked the user")
                    mcp_tools.dispatch(store, channel, "plan_submit",
                                       dict(plan=_plan(), request_key="plan-" + str(context["revision"])))
                else:
                    mcp_tools.dispatch(store, channel, "synthesis_submit", dict(text="Result verified."))
                return PromptOutcome(StopReason.END_TURN, "Coordinator turn")
            report_id = _publish(store, channel, channel.attempt_id)
            mcp_tools.dispatch(store, channel, "result_submit", dict(
                report=report_id, summary="Observed result", checks=[], request_key="result"))
            return PromptOutcome(StopReason.END_TURN, "Worker done")
        return behavior
    return router


def _runtime(store, router):
    engine = FakeSessionRuntime(store, router)
    return OrchestratingRuntime(store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
                                allow_unqualified_delivery=True), engine


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="disconnect"), "Inspect part 0", "owner")
    return store, task.task_id


def _task_events(store, task_id, *types):
    marks = ",".join("?" for _ in types)
    with store.connection() as db:
        return [dict(type=row["event_type"], assignment_ref=row["assignment_ref"],
                     payload=json.loads(row["payload_json"]))
                for row in db.execute(f"SELECT * FROM task_events WHERE task_id=? AND event_type IN ({marks}) ORDER BY id",
                                      (task_id, *types))]


def _runtime_events(store, task_id, *types):
    marks = ",".join("?" for _ in types)
    with store.connection() as db:
        return [dict(type=row["type"], payload=json.loads(row["payload_json"]))
                for row in db.execute(f"SELECT type,payload_json FROM runtime_events WHERE task_id=? AND type IN ({marks}) ORDER BY id",
                                      (task_id, *types))]


def _disconnect_records(store, task_id):
    return [e["payload"] for e in _task_events(store, task_id, "progress")
            if e["payload"].get("kind") == "host_disconnected"]


def _start_with_blocking_worker(store, task_id, worker_step=None):
    """Run a first turn whose worker is still working when the host disconnects."""
    started = threading.Event()

    def router(store, channel, session_ref, launch, instructions):
        def behavior(store, channel, ref, message, call_index, cancel_event):
            if channel.role == "coordinator":
                context = mcp_tools.dispatch(store, channel, "task_get_context", {})
                mcp_tools.dispatch(store, channel, "plan_submit",
                                   dict(plan=_plan(), request_key="plan-" + str(context["revision"])))
                return PromptOutcome(StopReason.END_TURN, "Planned")
            if worker_step is not None:
                worker_step(store, channel)
            started.set()
            # The engine honours the forwarded session/cancel as a real
            # adapter would once the host connection is gone.
            cancel_event.wait(20)
            return PromptOutcome(StopReason.CANCELLED, "")
        return behavior

    runtime, engine = _runtime(store, router)
    outcome = []
    turn = threading.Thread(target=lambda: outcome.append(runtime.start_prompt(
        task_id, SESSION, "Inspect part 0", lambda _text: True, threading.Event(),
        emit_tool_call=lambda _update: True)))
    turn.start()
    assert started.wait(20), "worker never started"
    return runtime, engine, turn, outcome


def _disconnect(runtime, turn):
    """What acp.agent does on EOF: disconnect the session, then tear down."""
    runtime.disconnect_session(SESSION)
    turn.join(20)
    assert not turn.is_alive()
    runtime.shutdown()


def test_disconnect_of_a_completed_task_changes_no_outcome(tmp_path):
    store, task_id = _task(tmp_path)
    runtime, _engine = _runtime(store, _finishing_router())
    try:
        assert runtime.start_prompt(task_id, SESSION, "Inspect part 0", lambda _text: True, threading.Event(),
                                    emit_tool_call=lambda _update: True) == StopReason.END_TURN
        before = TaskMachine(store).context(task_id)
        assert before["state"] == "COMPLETED"
        runtime.disconnect_session(SESSION)
    finally:
        runtime.shutdown()
    after = TaskMachine(store).context(task_id)
    assert after["state"] == "COMPLETED"
    assert [(n["node_id"], n["status"], n["result"]) for n in after["nodes"]] == \
        [(n["node_id"], n["status"], n["result"]) for n in before["nodes"]]
    assert after["synthesis"] == before["synthesis"]
    assert not _task_events(store, task_id, "assignment_failed", "assignment_cancelled")
    assert not _runtime_events(store, task_id, "TURN_INTERRUPTED", "RETURNED_TO_COORDINATOR")
    [record] = _disconnect_records(store, task_id)
    assert record["session_id"] == SESSION and record["task_id"] == task_id
    assert record["task_state"] == "COMPLETED" and record["interrupted_workers"] == []
    assert record["unknown_outcome_actions"] == [] and record["runtime_version"]
    assert record["history"] == "complete"


def test_disconnect_with_an_active_worker_is_reconciled_after_restart_not_failed(tmp_path):
    store, task_id = _task(tmp_path)
    runtime, _engine, turn, outcome = _start_with_blocking_worker(store, task_id)
    try:
        _disconnect(runtime, turn)
    finally:
        runtime.shutdown()
    assert outcome == [StopReason.CANCELLED]
    context = TaskMachine(store).context(task_id)
    # The transport closed; the work was neither cancelled nor failed.
    assert context["state"] == "EXECUTING"
    assert [n["status"] for n in context["nodes"]] == ["running"]
    assert not _task_events(store, task_id, "assignment_failed", "assignment_cancelled")
    assert not _runtime_events(store, task_id, "TURN_INTERRUPTED")
    [record] = _disconnect_records(store, task_id)
    [worker] = record["interrupted_workers"]
    with store.connection() as db:
        attempt_id = db.execute("SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND role_id!='orchestrator'",
                                (task_id,)).fetchone()[0]
    assert worker == dict(node_id="N0", assignment_ref=context["nodes"][0]["assignment_ref"], attempt_id=attempt_id)
    assert record["task_state"] == "EXECUTING" and record["coordinator_attempt_id"]

    # Reopening the chat shows the still-working card as interrupted by the
    # connection, not as a crashed worker (display-only; nothing is written).
    from cortex_runtime.acp import replay_store
    from cortex_runtime.recovery import registry

    registry.record_process_launch(store, attempt_id=attempt_id, task_id=task_id, role="worker", pid=1, pgid=1,
                                   start_ticks="0", boot_id="gone", launch_command_digest="x")
    registry.mark_terminated(store, attempt_id, "teardown")
    card = f"worker:{worker['assignment_ref']}"
    [correction] = replay_store.terminal_worker_card_reconciliation(store, task_id, [dict(kind="update", update=dict(
        sessionUpdate="tool_call", toolCallId=card, status="in_progress"))])
    assert correction["toolCallId"] == card and correction["status"] == "failed"
    assert correction["content"][0]["text"].startswith("Interrupted when the host connection closed")
    assert correction["_meta"] == {"cortexWorkerInterrupted": {"reason": "host_disconnected"}}

    # Restart: a fresh process reads the persisted state as authority.
    runtime, engine = _runtime(store, _finishing_router())
    try:
        assert runtime.start_prompt(task_id, SESSION, "continue", lambda _text: True, threading.Event(),
                                    emit_tool_call=lambda _update: True) == StopReason.END_TURN
    finally:
        runtime.shutdown()
    context = TaskMachine(store).context(task_id)
    assert context["state"] == "COMPLETED", context
    [lost] = _task_events(store, task_id, "assignment_failed")
    assert lost["payload"]["error"] == "recovery_worker_lost"
    [retry] = _runtime_events(store, task_id, "ASSIGNMENT_RETRIED")
    assert retry["payload"]["node"] == "N0" and retry["payload"]["observed_session"] == "absent"


def test_interrupted_worker_card_survives_an_in_place_reload_with_no_confirmed_absence(tmp_path):
    """In-place plugin reload / full page reload: the host
    disconnected this exact session (``host_disconnected`` was durably
    recorded), but nothing in this fresh process/registry can positively
    confirm the old worker's OS process exited -- unlike the process-restart
    case above, no ``attempt_process_registry`` row was ever recorded absent
    for it. Real recovery (``recovery.worker_reconciliation``) marks such a
    worker ``recovery_worker_lost`` on the coordinator's next turn
    regardless of that liveness answer -- display must show "Interrupted"
    on this same durable evidence alone, never wait for a stronger signal
    recovery itself never requires, and never keep showing a stale
    "Working" pill."""
    store, task_id = _task(tmp_path)
    runtime, _engine, turn, outcome = _start_with_blocking_worker(store, task_id)
    try:
        _disconnect(runtime, turn)
    finally:
        runtime.shutdown()
    assert outcome == [StopReason.CANCELLED]
    [record] = _disconnect_records(store, task_id)
    [worker] = record["interrupted_workers"]
    context = TaskMachine(store).context(task_id)

    from cortex_runtime.acp import replay_store

    card = f"worker:{worker['assignment_ref']}"
    base_title = "Worker: backend-dev · claude · Haiku · medium — Inspect part 0"
    [correction] = replay_store.terminal_worker_card_reconciliation(store, task_id, [dict(kind="update", update=dict(
        sessionUpdate="tool_call", toolCallId=card, status="in_progress", title=base_title))])
    assert correction["toolCallId"] == card and correction["status"] == "failed"
    assert correction["title"] == base_title + " · Interrupted"
    assert correction["content"][0]["text"].startswith("Interrupted when the host connection closed")
    assert correction["_meta"] == {"cortexWorkerInterrupted": {"reason": "host_disconnected"}}
    assert context["state"] == "EXECUTING"


def test_interrupted_worker_card_carries_a_localized_title_suffix_paseo_survives(tmp_path):
    """The Paseo client (integrations/paseo) renders an interrupted worker as
    its own state, never a "Failed" pill (a review's residual): since
    the installed Paseo ACP bridge drops `_meta` on a `tool_call_update`
    before any plugin code sees it, the durable signal is the same portable
    title-suffix channel T035 already uses for "Cancelled" -- see
    worker-display.ts's `resolveWorkerCardStatus`. The notice text itself is
    localized like every other runtime system line."""
    store, task_id = _task(tmp_path)
    from cortex_runtime.acp import steering as acp_steering

    acp_steering.set_language_override(store, task_id, "ru")
    runtime, _engine, turn, outcome = _start_with_blocking_worker(store, task_id)
    try:
        _disconnect(runtime, turn)
    finally:
        runtime.shutdown()
    assert outcome == [StopReason.CANCELLED]
    context = TaskMachine(store).context(task_id)
    [record] = _disconnect_records(store, task_id)
    [worker] = record["interrupted_workers"]
    with store.connection() as db:
        attempt_id = db.execute("SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND role_id!='orchestrator'",
                                (task_id,)).fetchone()[0]

    from cortex_runtime.acp import replay_store
    from cortex_runtime.recovery import registry

    registry.record_process_launch(store, attempt_id=attempt_id, task_id=task_id, role="worker", pid=1, pgid=1,
                                   start_ticks="0", boot_id="gone", launch_command_digest="x")
    registry.mark_terminated(store, attempt_id, "teardown")
    card = f"worker:{worker['assignment_ref']}"
    base_title = "Worker: backend-dev · claude · Haiku · medium — Inspect part 0"
    [correction] = replay_store.terminal_worker_card_reconciliation(store, task_id, [dict(kind="update", update=dict(
        sessionUpdate="tool_call", toolCallId=card, status="in_progress", title=base_title))])
    assert correction["toolCallId"] == card and correction["status"] == "failed"
    # ACP's ToolCallStatus has no "interrupted" value; the title suffix is
    # the channel that survives to the client, not `_meta` (still emitted,
    # for any client build that does forward it).
    assert correction["title"] == base_title + " · Interrupted"
    assert correction["_meta"] == {"cortexWorkerInterrupted": {"reason": "host_disconnected"}}
    assert correction["content"][0]["text"] == (
        "Прервано закрытием соединения с хостом; результата ещё нет. "
        "Следующее сообщение восстановит этого исполнителя из сохранённого состояния."
    )
    assert context["state"] == "EXECUTING"


def test_unknown_outcome_command_is_never_repeated_after_restart(tmp_path):
    store, task_id = _task(tmp_path)
    marker = tmp_path / "side-effect.log"
    command = ["python3", "-B", "-c", f"open({str(marker)!r}, 'a').write('ran\\n')"]
    refs = []

    def run_then_lose_the_receipt(store, channel):
        # The command runs; the process dies before its completion is retained.
        with patch("cortex_runtime.recovery.large_output_store.persist_output", side_effect=_ProcessKilled()):
            try:
                mcp_tools.dispatch(store, channel, "command_run", dict(argv=command, request_key="side-effect"))
            except _ProcessKilled:
                pass
        with store.connection() as db:
            refs.extend(json.loads(row[0])["tool_call_ref"] for row in db.execute(
                "SELECT result_json FROM runtime_requests WHERE task_id=? AND operation='command_run'", (task_id,)))

    runtime, _engine, turn, _outcome = _start_with_blocking_worker(store, task_id, run_then_lose_the_receipt)
    try:
        _disconnect(runtime, turn)
    finally:
        runtime.shutdown()
    assert marker.read_text() == "ran\n"
    [tool_call_ref] = refs
    [record] = _disconnect_records(store, task_id)
    assert [(a["operation"], a["tool_call_ref"]) for a in record["unknown_outcome_actions"]] == [
        ("command_run", tool_call_ref)]

    messages = []
    runtime, engine = _runtime(store, _finishing_router(messages, ask_user=True))
    try:
        runtime.start_prompt(task_id, SESSION, "continue", lambda _text: True, threading.Event(),
                             emit_tool_call=lambda _update: True)
    finally:
        runtime.shutdown()
    # Not re-executed: no retry, no second run, and the receipt stays open.
    assert marker.read_text() == "ran\n"
    assert not _runtime_events(store, task_id, "ASSIGNMENT_RETRIED")
    [returned] = _runtime_events(store, task_id, "RETURNED_TO_COORDINATOR")
    assert returned["payload"]["reason"] == "lost_worker_action_outcome_unknown"
    with store.connection() as db:
        [receipt] = [json.loads(row[0]) for row in db.execute(
            "SELECT result_json FROM runtime_requests WHERE task_id=? AND operation='command_run'", (task_id,))]
    assert receipt["complete"] is False
    # The coordinator is told exactly which action needs explicit reconciliation.
    assert any(tool_call_ref in message and "outcome was never retained" in message for message in messages)
    assert TaskMachine(store).context(task_id)["state"] == "WAITING_USER"


def test_daemon_teardown_never_records_a_worker_engine_failure(tmp_path):
    """Teardown kills the worker's engine even when no turn is cancelled.

    A worker dispatched by an earlier turn keeps that turn's cancel event,
    which a later disconnect no longer reaches; its killed engine must still
    not be recorded as an engine failure (or trigger a fallback route)."""
    from cortex_runtime.acp.orchestrating_runtime import EngineTurnFailed

    store, task_id = _task(tmp_path)
    started, killed = threading.Event(), threading.Event()

    def router(store, channel, session_ref, launch, instructions):
        def behavior(store, channel, ref, message, call_index):
            if channel.role == "coordinator":
                context = mcp_tools.dispatch(store, channel, "task_get_context", {})
                mcp_tools.dispatch(store, channel, "plan_submit",
                                   dict(plan=_plan(), request_key="plan-" + str(context["revision"])))
                return PromptOutcome(StopReason.END_TURN, "Planned")
            started.set()
            killed.wait(20)
            raise EngineTurnFailed("transport_eof")
        return behavior

    runtime, engine = _runtime(store, router)
    original_terminate = engine.terminate_owned

    def terminate(attempt_id):
        killed.set()
        return original_terminate(attempt_id)

    engine.terminate_owned = terminate
    turn_cancel = threading.Event()
    turn = threading.Thread(target=lambda: runtime.start_prompt(
        task_id, SESSION, "Inspect part 0", lambda _text: True, turn_cancel, emit_tool_call=lambda _update: True))
    turn.start()
    try:
        assert started.wait(20)
        runtime.shutdown()
        turn.join(20)
        assert not turn.is_alive()
    finally:
        killed.set()
        runtime.shutdown()
    assert not turn_cancel.is_set()
    assert not _task_events(store, task_id, "assignment_failed", "assignment_cancelled")
    context = TaskMachine(store).context(task_id)
    assert context["state"] == "EXECUTING" and [n["status"] for n in context["nodes"]] == ["running"]


class _Transport:
    def __init__(self):
        self.responses, self.notifications = [], []

    def send_response(self, message):
        self.responses.append(message)

    def send_notification(self, message, timeout=None):
        self.notifications.append(message)
        return True


class _IdleRuntime:
    """A runtime with no turn for the loaded session (a restored chat)."""

    def __init__(self):
        self.disconnected = []

    def start_prompt(self, task_id, session_id, content, emit_update, cancel_event):
        emit_update("Готово.")
        return StopReason.END_TURN

    def disconnect(self, session_id):
        self.disconnected.append(session_id)
        return False


def _agent(runtime, environ):
    from cortex_runtime.acp.agent import Agent

    agent = Agent(runtime, environ=environ)
    transport = _Transport()
    agent.bind_transport(transport)
    agent.initialized = True
    return agent, transport


def _counts(store, task_id):
    with store.connection() as db:
        return [db.execute(f"SELECT COUNT(*) FROM {table} WHERE task_id=?", (task_id,)).fetchone()[0]
                for table in ("task_events", "runtime_events", "source_events")]


def test_restored_chat_shows_saved_outcome_after_a_connection_close(tmp_path):
    import tempfile

    home = Path(tempfile.mkdtemp(prefix="cortex-disconnect-home-"))
    environ = dict(HOME=str(home), XDG_CONFIG_HOME=str(home / "config"), XDG_STATE_HOME=str(home / "state"))
    project = tmp_path / "project"
    project.mkdir()
    agent, _ = _agent(_IdleRuntime(), environ)
    session_id = agent._session_new(dict(cwd=str(project), mcpServers=[]))["sessionId"]
    session = agent.sessions[session_id]
    agent._run_prompt(request_id=1, session_id=session_id, session=session, text="Проверь проект",
                      cancel_event=threading.Event(), raw_blocks=[dict(type="text", text="Проверь проект")])
    task_id = session["task_id"]
    store = agent._store_for(session["project"])
    machine = TaskMachine(store)
    machine.create(task_id, "Проверь проект")
    # The user's Russian composer selection, as the agent carries it onto the task.
    from cortex_runtime.acp import steering as acp_steering

    acp_steering.set_language_override(store, task_id, "ru")
    with store.connection() as db:
        db.execute("UPDATE runtime_tasks SET state='COMPLETED' WHERE task_id=?", (task_id,))
        db.commit()

    # A later daemon only restored the chat; then the plugin update closed it.
    restored, restored_transport = _agent(_IdleRuntime(), environ)
    restored._session_load(2, dict(sessionId=session_id, cwd=str(project), mcpServers=[]))
    assert not any(n["params"]["update"].get("messageId", "").startswith("cortex-connection:")
                   for n in restored_transport.notifications)
    restored._handle_eof()
    assert restored.runtime.disconnected == [session_id]
    [record] = _disconnect_records(store, task_id)
    assert (record["session_id"], record["task_id"], record["task_state"], record["turn"]) == (
        session_id, task_id, "COMPLETED", "no_turn_in_process")
    assert record["history"] == "complete" and record["runtime_version"]
    assert machine.context(task_id)["state"] == "COMPLETED"

    # The close interrupted nothing: the reopened chat shows no notice, and loading writes nothing.
    loader, transport = _agent(_IdleRuntime(), environ)
    before = _counts(store, task_id)
    loader._session_load(3, dict(sessionId=session_id, cwd=str(project), mcpServers=[]))
    assert _counts(store, task_id) == before
    assert not any(n["params"]["update"].get("messageId", "").startswith("cortex-connection:")
                   for n in transport.notifications)
    assert "error" not in transport.responses[-1]

    # A later close that cut off a running worker is shown with the saved state.
    from cortex_runtime.delegation import events as delegation_events

    delegation_events.append_event(store, task_id, "progress", "disconnect:interrupting", dict(
        kind="host_disconnected", session_id=session_id, task_id=task_id,
        interrupted_workers=[dict(node_id="n1", assignment_ref="a1", attempt_id="att1")],
        unknown_outcome_actions=[]), origin="runtime")
    loader, transport = _agent(_IdleRuntime(), environ)
    loader._session_load(5, dict(sessionId=session_id, cwd=str(project), mcpServers=[]))
    notice = transport.notifications[-1]["params"]["update"]
    assert notice["sessionUpdate"] == "agent_message_chunk"
    assert notice["messageId"].startswith("cortex-connection:")
    assert "COMPLETED" in notice["content"]["text"] and "Прерванных воркеров: 1" in notice["content"]["text"]
    assert notice["_meta"]["cortexConnectionNotice"]["interruptedWorkers"] == 1

    # The next user message ends the notice.

    delegation_events.append_event(store, task_id, "user_message", "user_message:after-restart",
                                   dict(content="ещё", session_id=session_id), origin=session_id)
    again, again_transport = _agent(_IdleRuntime(), environ)
    again._session_load(4, dict(sessionId=session_id, cwd=str(project), mcpServers=[]))
    assert not any(n["params"]["update"].get("messageId", "").startswith("cortex-connection:")
                   for n in again_transport.notifications)


_SIGTERM_CHILD = '''
import pathlib, sys
from cortex_runtime.acp.agent import main
out = pathlib.Path(sys.argv[1])

class Runtime:
    def start_prompt(self, task_id, session_id, content, emit_update, cancel_event):
        raise AssertionError("no prompt in this test")

    def disconnect(self, session_id):
        (out / "disconnect").write_text(session_id)
        return True

    def shutdown(self):
        (out / "shutdown").write_text("done")

main(runtime=Runtime())
'''


def test_sigterm_from_the_host_runs_the_disconnect_teardown(tmp_path):
    """Paseo stops a provider with SIGTERM (then SIGKILL after a second),
    not by closing stdin; the Paseo shim forwards the SIGTERM to Cortex."""
    import os
    import signal
    import subprocess
    import sys
    import tempfile

    home = Path(tempfile.mkdtemp(prefix="cortex-sigterm-home-"))
    project = tmp_path / "project"
    project.mkdir()
    out = tmp_path / "out"
    out.mkdir()
    env = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(home / "config"), XDG_STATE_HOME=str(home / "state"),
               PYTHONPATH=str(Path(__file__).resolve().parents[1] / "src"), PYTHONDONTWRITEBYTECODE="1")
    child = subprocess.Popen([sys.executable, "-B", "-c", _SIGTERM_CHILD, str(out)], env=env,
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    try:
        def call(request_id, method, params):
            child.stdin.write((json.dumps(dict(jsonrpc="2.0", id=request_id, method=method, params=params)) + "\n").encode())
            child.stdin.flush()
            while True:
                message = json.loads(child.stdout.readline())
                if message.get("id") == request_id:
                    assert "error" not in message, message
                    return message["result"]

        call(1, "initialize", dict(protocolVersion=1, clientCapabilities={}))
        session_id = call(2, "session/new", dict(cwd=str(project), mcpServers=[]))["sessionId"]
        child.send_signal(signal.SIGTERM)  # stdin stays open, as when the host kills the process
        assert child.wait(timeout=20) == 0
    finally:
        if child.poll() is None:
            child.kill()
    assert (out / "disconnect").read_text() == session_id
    assert (out / "shutdown").read_text() == "done"


_TRIGGER_CHILD = '''
import pathlib, sys
from cortex_runtime.acp.agent import main
out = pathlib.Path(sys.argv[1])

class Runtime:
    def start_prompt(self, task_id, session_id, content, emit_update, cancel_event):
        raise AssertionError("no prompt in this test")

    def disconnect(self, session_id, trigger=None):
        (out / "trigger").write_text(str(trigger))
        return True

main(runtime=Runtime())
'''


@pytest.mark.parametrize("how, trigger", [
    ("signal", "sigterm"), ("eof", "stdin_eof"), ("writer_closed", "writer_closed"),
])
def test_the_disconnect_record_names_what_the_process_observed(tmp_path, how, trigger):
    """Which of SIGTERM, stdin EOF or writer close ended the connection is kept on the
    durable record, so a live reload can be diagnosed from saved state."""
    _assert_disconnect_trigger(tmp_path, how, trigger)


def _assert_disconnect_trigger(tmp_path, how, trigger, child_script=_TRIGGER_CHILD):
    import os
    import signal
    import subprocess
    import sys
    import tempfile

    home = Path(tempfile.mkdtemp(prefix="cortex-trigger-home-"))
    project = tmp_path / "project"
    project.mkdir()
    out = tmp_path / "out"
    out.mkdir()
    env = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(home / "config"), XDG_STATE_HOME=str(home / "state"),
               PYTHONPATH=str(Path(__file__).resolve().parents[1] / "src"), PYTHONDONTWRITEBYTECODE="1")
    child = subprocess.Popen([sys.executable, "-B", "-c", child_script, str(out)], env=env,
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    try:
        def call(request_id, method, params):
            child.stdin.write((json.dumps(dict(jsonrpc="2.0", id=request_id, method=method, params=params)) + "\n").encode())
            child.stdin.flush()
            while True:
                message = json.loads(child.stdout.readline())
                if message.get("id") == request_id:
                    assert "error" not in message, message
                    return message["result"]

        call(1, "initialize", dict(protocolVersion=1, clientCapabilities={}))
        call(2, "session/new", dict(cwd=str(project), mcpServers=[]))
        if how == "signal":
            child.send_signal(signal.SIGTERM)
        elif how == "writer_closed":
            # Keep stdin open: only a real failed stdout write should end
            # the connection, and joining the blocked reader would wedge it.
            child.stdout.close()
            child.stdin.write((json.dumps(dict(jsonrpc="2.0", id=3, method="initialize",
                                               params=dict(protocolVersion=1, clientCapabilities={}))) + "\n").encode())
            child.stdin.flush()
        else:
            child.stdin.close()
        assert child.wait(timeout=20) == 0
    finally:
        if child.poll() is None:
            child.kill()
            child.wait(timeout=5)
    assert (out / "trigger").read_text() == trigger


# Hold the disconnect callback after opening the trigger file but before writing it.
# Joining its transport thread must finish that callback before runtime shutdown.
_ORDERED_TRIGGER_CHILD = _TRIGGER_CHILD.replace(
    "main(runtime=Runtime())",
    '''
import threading
from cortex_runtime.acp.transport import StdioTransport

entered = threading.Event()
release = threading.Event()
completed = threading.Event()
original_wait = StdioTransport.wait_for_shutdown_signal
original_join = StdioTransport.join_reader
original_stop = StdioTransport.stop_writer

def wait(self, timeout=None):
    reason = original_wait(self, timeout)
    if reason in ("reader_eof", "writer_closed"):
        assert entered.wait(5), "disconnect callback did not enter"
    return reason

def join(self, timeout=None):
    release.set()
    return original_join(self, timeout)

def stop(self):
    release.set()
    return original_stop(self)

StdioTransport.wait_for_shutdown_signal = wait
StdioTransport.join_reader = join
StdioTransport.stop_writer = stop

class OrderedRuntime(Runtime):
    def disconnect(self, session_id, trigger=None):
        with (out / "trigger").open("w") as stream:
            entered.set()
            if threading.current_thread() is not threading.main_thread():
                assert release.wait(5), "transport thread was not joined before shutdown"
            stream.write(str(trigger))
        completed.set()
        return True

    def shutdown(self):
        (out / "shutdown_order").write_text(str(completed.is_set()))
        release.set()

main(runtime=OrderedRuntime())
''',
)


@pytest.mark.parametrize("how, trigger", [
    ("signal", "sigterm"), ("eof", "stdin_eof"), ("writer_closed", "writer_closed"),
])
def test_disconnect_callback_finishes_before_runtime_shutdown(tmp_path, how, trigger):
    _assert_disconnect_trigger(tmp_path, how, trigger, _ORDERED_TRIGGER_CHILD)
    assert (tmp_path / "out" / "shutdown_order").read_text() == "True"


@pytest.mark.parametrize("trigger", ["sigterm", "stdin_eof", "writer_closed"])
def test_the_runtime_keeps_the_trigger_on_the_host_disconnected_record(tmp_path, trigger):
    store, task_id = _task(tmp_path)
    runtime, _engine, turn, _outcome = _start_with_blocking_worker(store, task_id)
    runtime.disconnect_session(SESSION, trigger=trigger)
    turn.join(20)
    runtime.shutdown()
    [record] = _disconnect_records(store, task_id)
    assert record["trigger"] == trigger


def _dead_process_identity():
    """The identity of a real process that was SIGKILLed: no handler ran."""
    import subprocess

    from cortex_runtime.engine import process_identity

    child = subprocess.Popen(["sleep", "30"], start_new_session=True)
    identity = process_identity.capture(child.pid)
    child.kill()
    child.wait()
    assert identity is not None
    return identity


def _worker_attempt(store, task_id):
    with store.connection() as db:
        row = db.execute("SELECT idempotency_key, attempt_id FROM delegation_provisioning "
                         "WHERE task_id=? AND role_id!='orchestrator'", (task_id,)).fetchone()
    return row["idempotency_key"], row["attempt_id"]


def _record_worker(store, task_id, attempt_id, *, owner, worker):
    """Durable identities as a real launch records them, with ``owner``
    standing for the Cortex process that launched the worker."""
    from cortex_runtime.recovery import registry

    with patch.object(registry, "_own_identity", return_value=owner):
        registry.record_process_launch(
            store, attempt_id=attempt_id, task_id=task_id, role="worker", pid=worker.pid, pgid=worker.pgid,
            start_ticks=worker.start_ticks, boot_id=worker.boot_id, launch_command_digest="x")


def test_worker_launch_records_the_cortex_process_that_owns_it(tmp_path):
    import os

    from cortex_runtime.recovery import registry

    store, task_id = _task(tmp_path)
    registry.record_process_launch(store, attempt_id="xa_w", task_id=task_id, role="worker", pid=1, pgid=1,
                                   start_ticks="0", boot_id="b", launch_command_digest="x")
    registry.record_process_launch(store, attempt_id="co_c", task_id=task_id, role="coordinator", pid=1, pgid=1,
                                   start_ticks="0", boot_id="b", launch_command_digest="x")
    with store.connection() as db:
        rows = {row["attempt_id"]: dict(row) for row in db.execute(
            "SELECT * FROM attempt_process_registry WHERE task_id=?", (task_id,))}
        assert set(rows) == {"xa_w", "co_c", registry.host_row_id("xa_w")}
        assert rows[registry.host_row_id("xa_w")]["role"] == registry.HOST_ROLE
        assert rows[registry.host_row_id("xa_w")]["pid"] == os.getpid()
        # This process launched it and is running: its owner is alive.
        assert registry.owner_liveness(db, "xa_w") == "alive"
        assert registry.owner_liveness(db, "co_c") is None


def test_worker_whose_host_was_replaced_without_any_signal_reads_interrupted(tmp_path):
    """A plugin reload or daemon kill that no SIGTERM/EOF reached (SIGKILL,
    OOM) writes no ``host_disconnected`` record. The launching process's
    durable identity, now provably gone, is the evidence instead: the card
    reads Interrupted, not a crash and not a stale "Working", with nothing
    written by the read."""
    store, task_id = _task(tmp_path)
    runtime, _engine, turn, _outcome = _start_with_blocking_worker(store, task_id)
    try:
        assignment_ref, attempt_id = _worker_attempt(store, task_id)
        dead = _dead_process_identity()
        _record_worker(store, task_id, attempt_id, owner=dead, worker=dead)
        assert _disconnect_records(store, task_id) == []

        from cortex_runtime.acp import replay_store
        from cortex_runtime.recovery.worker_reconciliation import interrupted_attempts

        with store.connection() as db:
            assert interrupted_attempts(db, task_id) == {attempt_id}
            before = db.execute("SELECT COUNT(*) FROM task_events WHERE task_id=?", (task_id,)).fetchone()[0]
        card = f"worker:{assignment_ref}"
        base_title = "Worker: backend-dev · claude · Haiku · medium — Inspect part 0"
        [correction] = replay_store.terminal_worker_card_reconciliation(store, task_id, [dict(kind="update", update=dict(
            sessionUpdate="tool_call", toolCallId=card, status="in_progress", title=base_title))])
        assert correction["status"] == "failed" and correction["title"] == base_title + " · Interrupted"
        assert correction["content"][0]["text"].startswith("Interrupted when the host connection closed")
        with store.connection() as db:
            assert db.execute("SELECT COUNT(*) FROM task_events WHERE task_id=?", (task_id,)).fetchone()[0] == before
        assert [n["status"] for n in TaskMachine(store).context(task_id)["nodes"]] == ["running"]
    finally:
        _disconnect(runtime, turn)


def test_worker_with_a_live_owner_still_reads_working(tmp_path):
    """No disconnect and a live launching process: the worker really runs."""
    import os

    from cortex_runtime.engine import process_identity

    store, task_id = _task(tmp_path)
    runtime, _engine, turn, _outcome = _start_with_blocking_worker(store, task_id)
    try:
        assignment_ref, attempt_id = _worker_attempt(store, task_id)
        alive = process_identity.capture(os.getpid(), os.getpgrp())
        _record_worker(store, task_id, attempt_id, owner=alive, worker=alive)

        from cortex_runtime.acp import replay_store
        from cortex_runtime.recovery.worker_reconciliation import interrupted_attempts

        with store.connection() as db:
            assert interrupted_attempts(db, task_id) == set()
        assert replay_store.terminal_worker_card_reconciliation(store, task_id, [dict(kind="update", update=dict(
            sessionUpdate="tool_call", toolCallId=f"worker:{assignment_ref}", status="in_progress"))]) == []

        # The stale-coordinator-channel write-off names only the assignment.
        from cortex_runtime.delegation import events as delegation_events

        delegation_events.append_event(store, task_id, "assignment_failed", "lost-by-assignment",
                                       dict(error="recovery_worker_lost"), origin="runtime",
                                       assignment_ref=assignment_ref)
        with store.connection() as db:
            assert interrupted_attempts(db, task_id) == {attempt_id}
    finally:
        _disconnect(runtime, turn)


def test_a_worker_lost_with_its_host_stays_interrupted_after_recovery(tmp_path):
    """Live: after a plugin reload the next message's
    recovery wrote the worker off (``recovery_worker_lost``) and the replan
    cancelled it; the card then read a generic "Failed". It stays
    Interrupted on the live update and on replay over that older history;
    the retry is a new card and nothing is repeated automatically."""
    store, task_id = _task(tmp_path)
    runtime, _engine, turn, _outcome = _start_with_blocking_worker(store, task_id)
    _disconnect(runtime, turn)
    [record] = _disconnect_records(store, task_id)
    [worker] = record["interrupted_workers"]
    card = f"worker:{worker['assignment_ref']}"

    updates = []
    runtime, _engine = _runtime(store, _finishing_router())
    try:
        assert runtime.start_prompt(task_id, SESSION, "continue", lambda _text: True, threading.Event(),
                                    emit_tool_call=lambda update: updates.append(dict(update)) or True) == StopReason.END_TURN
    finally:
        runtime.shutdown()
    [lost] = _task_events(store, task_id, "assignment_failed")
    assert lost["payload"]["error"] == "recovery_worker_lost"
    lost_card = [u for u in updates if u.get("tool_call_id") == card]
    assert lost_card, updates
    assert lost_card[-1]["status"] == "failed"
    assert lost_card[-1]["title"] == "Worker: backend-dev — Inspect part 0 · Interrupted"
    assert not any(str(u.get("title", "")).endswith("· Cancelled") for u in lost_card)
    assert "Interrupted when the host connection closed" in lost_card[-1]["text"]
    # Exactly one retry, on a new card (the other new card is the review).
    [retry] = _runtime_events(store, task_id, "ASSIGNMENT_RETRIED")
    assert retry["payload"]["node"] == "N0"
    assert {u["tool_call_id"] for u in updates if str(u.get("tool_call_id", "")).startswith("worker:")} - {card}
    assert TaskMachine(store).context(task_id)["state"] == "COMPLETED"

    # Replay over the pre-fix history, whose last update was a bare failure.
    from cortex_runtime.acp import replay_store

    base_title = "Worker: backend-dev — Inspect part 0"
    [correction] = replay_store.terminal_worker_card_reconciliation(store, task_id, [
        dict(kind="update", update=dict(sessionUpdate="tool_call", toolCallId=card, status="in_progress", title=base_title)),
        dict(kind="update", update=dict(sessionUpdate="tool_call_update", toolCallId=card, status="failed")),
    ])
    assert correction["toolCallId"] == card and correction["title"] == base_title + " · Interrupted"
    # Already Interrupted: replay adds nothing.
    assert replay_store.terminal_worker_card_reconciliation(store, task_id, [
        dict(kind="update", update=dict(sessionUpdate="tool_call", toolCallId=card, status="in_progress", title=base_title)),
        dict(kind="update", update=dict(sessionUpdate="tool_call_update", toolCallId=card, status="failed",
                                        title=base_title + " · Interrupted")),
    ]) == []


def test_recovery_adds_no_second_row_for_a_card_the_host_already_shows(tmp_path):
    """Live (this fix's own runs): the replacement process's Interrupted
    update for the lost card -- sent first as a create, then as a
    tool_call_update -- made Paseo 0.9.2 show the same worker twice. A card
    the lost process created is left to the host's own durable status and to
    the session/load correction, which renders it Interrupted."""
    from cortex_runtime.acp import replay_store
    from cortex_runtime.acp import sessions as acp_sessions

    store, task_id = _task(tmp_path)
    runtime, _engine, turn, _outcome = _start_with_blocking_worker(store, task_id)
    _disconnect(runtime, turn)
    [record] = _disconnect_records(store, task_id)
    card = f"worker:{record['interrupted_workers'][0]['assignment_ref']}"
    shown = "Worker: general · codex · 6 Luna · high — Run sleep 150"
    acp_sessions.create_session(store, SESSION, str(tmp_path), {}, {}, [])
    acp_sessions.bind_task(store, SESSION, task_id)
    replay_store.append_host_update(store, SESSION, dict(
        sessionUpdate="tool_call", toolCallId=card, status="in_progress", title=shown))

    updates = []
    runtime, _engine = _runtime(store, _finishing_router())
    try:
        runtime.start_prompt(task_id, SESSION, "continue", lambda _text: True, threading.Event(),
                             emit_tool_call=lambda update: updates.append(dict(update)) or True)
    finally:
        runtime.shutdown()
    assert [u for u in updates if u.get("tool_call_id") == card] == []
    history = replay_store.replay_session(store, SESSION)
    [correction] = [u for u in replay_store.terminal_worker_card_reconciliation(store, task_id, history)
                    if u["toolCallId"] == card]
    assert correction["title"] == shown + " · Interrupted"


def test_user_stop_says_paused_with_cut_off_actions_and_a_later_reload_adds_no_notice(tmp_path):
    """Stop pauses the task and reports, at once, the worker action it cut off;
    a host restart afterwards finds nothing in flight and adds no notice."""
    from cortex_runtime.recovery.worker_reconciliation import host_disconnect_notice, unknown_outcome_actions

    store, task_id = _task(tmp_path)
    marker = tmp_path / "side-effect.log"
    command = ["python3", "-B", "-c", f"open({str(marker)!r}, 'a').write('ran\\n')"]
    started = threading.Event()

    def router(store, channel, session_ref, launch, instructions):
        def behavior(store, channel, ref, message, call_index, cancel_event):
            if channel.role == "coordinator":
                context = mcp_tools.dispatch(store, channel, "task_get_context", {})
                mcp_tools.dispatch(store, channel, "plan_submit",
                                   dict(plan=_plan(), request_key="plan-" + str(context["revision"])))
                return PromptOutcome(StopReason.END_TURN, "Planned")
            with patch("cortex_runtime.recovery.large_output_store.persist_output", side_effect=_ProcessKilled()):
                try:
                    mcp_tools.dispatch(store, channel, "command_run", dict(argv=command, request_key="cut-off"))
                except _ProcessKilled:
                    pass
            started.set()
            cancel_event.wait(20)
            return PromptOutcome(StopReason.CANCELLED, "")
        return behavior

    runtime, _engine = _runtime(store, router)
    stop, emitted, outcome = threading.Event(), [], []
    turn = threading.Thread(target=lambda: outcome.append(runtime.start_prompt(
        task_id, SESSION, "Inspect part 0", lambda text: emitted.append(text) or True, stop,
        emit_tool_call=lambda _update: True)))
    turn.start()
    try:
        assert started.wait(20), "worker never started"
        stop.set()  # the user's Stop (session/cancel), not a transport loss
        turn.join(20)
        assert not turn.is_alive()
        text = "".join(emitted)
        assert "Paused. Send a message to continue this task" in text
        assert "The stop cut off 1 worker action(s) mid-way" in text
        assert TaskMachine(store).context(task_id)["state"] == "RECOVERING"
        with store.connection() as db:
            assert len(unknown_outcome_actions(db, task_id, revoked_only=True)) == 1
        runtime.disconnect_session(SESSION)
    finally:
        runtime.shutdown()
    [record] = _disconnect_records(store, task_id)
    assert record["unknown_outcome_actions"] == [] and record["interrupted_workers"] == []
    assert host_disconnect_notice(store, task_id, SESSION) is None


def test_read_only_external_calls_are_never_unknown_outcome_actions(tmp_path):
    from cortex_runtime.recovery.worker_reconciliation import unknown_outcome_actions

    store, task_id = _task(tmp_path)

    def open_external_calls(store, channel):
        with store.connection() as db:
            for key, read_only in (("external:s:1", True), ("external:s:2", False)):
                db.execute("INSERT INTO runtime_requests VALUES(?,?,?,?,?,?)", (
                    task_id, key, channel.channel_id, "external_call", "d" + key,
                    json.dumps(dict(complete=False, read_only=read_only))))
            db.commit()

    runtime, _engine, turn, _outcome = _start_with_blocking_worker(store, task_id, open_external_calls)
    try:
        with store.connection() as db:
            assert len(unknown_outcome_actions(db, task_id)) == 1
        _disconnect(runtime, turn)
    finally:
        runtime.shutdown()
    [record] = _disconnect_records(store, task_id)
    assert len(record["unknown_outcome_actions"]) == 1


def test_continuing_after_a_restart_does_not_announce_a_new_session_but_records_it(tmp_path):
    store, task_id = _task(tmp_path)
    runtime, _engine = _runtime(store, _finishing_router())
    try:
        runtime.start_prompt(task_id, SESSION, "Inspect part 0", lambda _text: True, threading.Event(),
                             emit_tool_call=lambda _update: True)
    finally:
        runtime.shutdown()
    emitted = []
    restarted, _engine = _runtime(store, _finishing_router())
    try:
        restarted.start_prompt(task_id, SESSION, "continue", lambda text: emitted.append(text) or True,
                               threading.Event(), emit_tool_call=lambda _update: True)
    finally:
        restarted.shutdown()
    assert not any("new coordinator session" in text or "новой сессией координатора" in text for text in emitted)
    assert any(e["payload"].get("message_key") == "coordinator_recovery_new_session"
               for e in _task_events(store, task_id, "progress"))
