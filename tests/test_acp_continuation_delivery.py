"""No-paid normal-prompt/background-continuation ownership regressions."""
import json
import threading
import uuid

import pytest

from cortex_runtime.acp import sessions as acp_sessions
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.acp.orchestrating_runtime import PromptOutcome

from test_acp_steering_and_questions import _agent_for_language_tests, _make_runtime
from runtime_scenarios import propose_inspection, publish_result


@pytest.mark.parametrize("interrupt", [None, "cancel", "stop"])
def test_native_drain_during_background_worker_does_not_interrupt(tmp_path, monkeypatch, interrupt):
    agent, transport, environ = _agent_for_language_tests(tmp_path, monkeypatch)
    environ["CORTEX_PASEO_QUESTION_UI"] = "1"
    project = tmp_path / "project"
    project.mkdir()
    session_id = agent._session_new(dict(cwd=str(project), mcpServers=[]))["sessionId"]
    store = agent._store_for(str(project))
    task = bootstrap_task(store, SourceEvent(origin="test", event_id=str(uuid.uuid4())),
                          "Inspect", "owner-1")
    session = agent.sessions[session_id]
    session["task_id"], session["owner_epoch"] = task.task_id, task.owner_epoch
    acp_sessions.bind_task(store, session_id, task.task_id)
    TaskMachine(store).create(task.task_id, "Inspect")
    entered, release = threading.Event(), threading.Event()
    planning = []
    workers = []

    def router(store, channel, *_):
        def behavior(store, channel, _ref, message, _index):
            if channel.role == "coordinator":
                context = mcp_tools.dispatch(store, channel, "task_get_context", {})
                if context["state"] == "PLANNING":
                    planning.append(message)
                    propose_inspection(store, channel)
                else:
                    mcp_tools.dispatch(store, channel, "synthesis_submit",
                                       dict(text="Reviewed and verified"))
                return PromptOutcome(StopReason.END_TURN, "Coordinator text")
            if not workers:
                workers.append(channel)
                entered.set()
                assert release.wait(10)
            return publish_result(store, channel)
        return behavior

    runtime, _ = _make_runtime(store, router)
    from cortex_runtime.acp.agent import Agent
    replacement = Agent(runtime, environ=environ)
    replacement.bind_transport(transport)
    replacement.sessions = agent.sessions
    replacement.client_capabilities = {}
    agent = replacement
    delivered = threading.Event()
    send_response = transport.send_response
    def record_response(response):
        send_response(response)
        if response.get("id") == 102:
            delivered.set()
    transport.send_response = record_response
    sender = None
    try:
        agent._queue_paseo_message(101, session_id, session, json.dumps(dict(
            clientMessageId="late-steer", content=[dict(type="text", text="Inspect title")],
        )))
        assert entered.wait(5)
        with session["lock"]:
            background = session["active"]
            assert session["background"]
        sender = threading.Thread(target=lambda: agent._session_prompt(102, dict(
            sessionId=session_id, prompt=[dict(type="text", text="Native drain")],
        )))
        sender.start()
        sender.join(5)
        assert not sender.is_alive()
        context = TaskMachine(store).context(task.task_id)
        print("drain state:", context["state"], "node:", context["nodes"][0]["status"],
              "continuation cancelled:", background.is_set())
        assert not background.is_set()
        assert context["state"] == "EXECUTING"
        assert context["nodes"][0]["status"] == "running"
        mcp_tools.dispatch(store, workers[0], "project_read", dict(path="."))
        if interrupt:
            if interrupt == "cancel":
                agent._session_cancel(dict(sessionId=session_id))
            else:
                agent._session_prompt(103, dict(
                    sessionId=session_id, prompt=[dict(type="text", text="/cortex-stop")]))
            assert delivered.wait(5)
            assert next(r for r in transport.responses if r.get("id") == 102)["result"]["stopReason"] == "cancelled"
            assert background.is_set()
            # Keep the fake worker blocked until the real controller has
            # committed interruption; a result winning that race is valid.
            agent.join_workers()
            assert TaskMachine(store).context(task.task_id)["nodes"][0]["status"] == "cancelled"
            release.set()
            agent.join_workers()
            agent.join_workers()
            context = TaskMachine(store).context(task.task_id)
            assert context["state"] == ("CANCELLED" if interrupt == "stop" else "RECOVERING")
            assert context["nodes"][0]["status"] == "cancelled"
            assert not any("Native drain" in text for text in planning[1:])
            with store.connection() as db:
                assert db.execute(
                    "SELECT COUNT(*) FROM source_events WHERE task_id=? AND origin=?",
                    (task.task_id, f"paseo-steer:{session_id}")).fetchone()[0] == 1
            return
        # A lost config acknowledgement/replay is not a new source or steer.
        agent._queue_paseo_message(104, session_id, session, json.dumps(dict(
            clientMessageId="late-steer", content=[dict(type="text", text="Inspect title")],
        )))
        release.set()
        assert delivered.wait(5)
        agent.join_workers()
        assert any("Native drain" in text for text in planning[1:])
        assert any(r.get("id") == 102 and "result" in r for r in transport.responses)
        with store.connection() as db:
            types = [r[0] for r in db.execute(
                "SELECT type FROM runtime_events WHERE task_id=?", (task.task_id,))]
        assert "TURN_INTERRUPTED" not in types
        with store.connection() as db:
            actor = f"user:paseo-steer:{session_id}:late-steer"
            assert db.execute("SELECT COUNT(*) FROM runtime_events WHERE task_id=? AND actor=? "
                              "AND type='USER_STEERING'", (task.task_id, actor)).fetchone()[0] == 1
            assert db.execute("SELECT COUNT(*) FROM runtime_events WHERE task_id=? AND actor=? "
                              "AND type='STEERING_APPLIED'", (task.task_id, actor)).fetchone()[0] == 1
            assert db.execute("SELECT COUNT(*) FROM source_events WHERE task_id=? AND origin=?",
                              (task.task_id, f"paseo-steer:{session_id}")).fetchone()[0] == 1
        assert TaskMachine(store).context(task.task_id)["state"] == "COMPLETED"
    finally:
        release.set()
        if sender is not None:
            sender.join(5)
        agent.join_workers()
        runtime.shutdown()


class _BlockedRuntime:
    """Gated engine boundary; durable capture stays in the real Agent/Store."""
    def __init__(self):
        self.entered = threading.Event()
        self.release = threading.Event()
        self.calls = []
        self.cancellations = []

    def start_prompt(self, task_id, session_id, content, emit_update, cancel_event, *,
                     source_origin=None, source_event_id=None, prompt_blocks=None):
        self.calls.append((session_id, content, source_origin, source_event_id, prompt_blocks))
        if len(self.calls) == 1:
            self.entered.set()
            assert self.release.wait(5)
        return StopReason.CANCELLED if cancel_event.is_set() else StopReason.END_TURN

    def cancel(self, session_id):
        self.cancellations.append(session_id)


def _blocked_agent(tmp_path, monkeypatch):
    from cortex_runtime.acp.agent import Agent
    from test_acp_config_options import _isolate, _RecordingTransport
    runtime = _BlockedRuntime()
    environ = _isolate(tmp_path, monkeypatch)
    agent = Agent(runtime, environ=environ)
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.client_capabilities = {}
    project = tmp_path / "project"
    project.mkdir()
    session_id = agent._session_new(dict(cwd=str(project), mcpServers=[]))["sessionId"]
    session = agent.sessions[session_id]
    store = agent._store_for(str(project))
    task = bootstrap_task(store, SourceEvent(origin="test", event_id=str(uuid.uuid4())),
                          "Inspect", "owner-1")
    session["task_id"], session["owner_epoch"] = task.task_id, task.owner_epoch
    acp_sessions.bind_task(store, session_id, task.task_id)
    TaskMachine(store).create(task.task_id, "Inspect")
    return agent, transport, runtime, store, session_id, session, task.task_id


def _responses_for(transport, *request_ids):
    received = {request_id: threading.Event() for request_id in request_ids}
    send = transport.send_response
    def response(message):
        send(message)
        if message.get("id") in received:
            received[message["id"]].set()
    transport.send_response = response
    return received


@pytest.mark.parametrize("boundary", ["before", "during", "after"])
def test_prompt_continuation_order_and_exact_content(tmp_path, monkeypatch, boundary):
    import base64
    agent, transport, runtime, store, sid, session, task_id = _blocked_agent(tmp_path, monkeypatch)
    received = _responses_for(transport, 201, 202)
    blocks = [dict(type="text", text="  native\\n"),
              dict(type="image", data=base64.b64encode(b"image bytes").decode(), mimeType="image/png"),
              dict(type="resource", resource=dict(uri="file:///fixture.txt",
                                                  mimeType="text/plain", text="file bytes"))]
    original = json.loads(json.dumps(blocks))
    meta = dict(provenance="native-drain")
    def prompt():
        agent._session_prompt(201, dict(sessionId=sid, prompt=blocks, _meta=meta))
    def queue():
        agent._queue_paseo_message(200, sid, session, json.dumps(dict(
            clientMessageId="q1", content=[dict(type="text", text="queued")])))
    try:
        if boundary == "before":
            prompt()
            assert runtime.entered.wait(5)
            queue()
        else:
            queue()
            assert runtime.entered.wait(5)
            if boundary == "after":
                runtime.release.set()
                agent.join_workers()
                assert session["active"] is None
            prompt()
        if boundary == "during":
            # Caller-owned objects cannot alter the waiting request.
            blocks[0]["text"] = "changed"
            meta["provenance"] = "changed"
            assert not received[201].is_set()
            assert not session["active"].is_set()
        runtime.release.set()
        assert received[201].wait(5)
        agent.join_workers()
        agent.join_workers()
        assert len(runtime.calls) == 2
        normal = next(call for call in runtime.calls if call[2] == f"acp:{sid}")
        assert normal[4] == original
        assert normal[1].startswith("  native\\n")
        expected = [f"acp:{sid}", f"paseo-steer:{sid}"] if boundary == "before" else [
            f"paseo-steer:{sid}", f"acp:{sid}"]
        assert [call[2] for call in runtime.calls] == expected
        with store.connection() as db:
            rows = list(db.execute("SELECT origin,meta_json FROM source_events WHERE task_id=? "
                                   "AND origin<>? ORDER BY captured_at",
                                   (task_id, "test")))
            assert [row["origin"] for row in rows] == expected
            assert json.loads(next(row["meta_json"] for row in rows if row["origin"] == f"acp:{sid}")) == dict(
                provenance="native-drain")
            captures = list(db.execute("SELECT kind,availability FROM source_blocks WHERE task_id=?",
                                       (task_id,)))
            assert ("image", "stored") in [tuple(row) for row in captures]
            assert ("resource", "stored") in [tuple(row) for row in captures]
        assert runtime.cancellations == []
        # Ordinary foreground concurrency remains refused, not a second owner.
    finally:
        runtime.release.set()
        agent.join_workers()


@pytest.mark.parametrize("ending", ["cancel", "disconnect"])
def test_waiting_prompts_cannot_dispatch_after_interruption(tmp_path, monkeypatch, ending):
    agent, transport, runtime, store, sid, session, task_id = _blocked_agent(tmp_path, monkeypatch)
    received = _responses_for(transport, 301, 302)
    try:
        agent._queue_paseo_message(300, sid, session, json.dumps(dict(
            clientMessageId="q1", content=[dict(type="text", text="queued")])))
        assert runtime.entered.wait(5)
        for request_id in (301, 302):
            agent._session_prompt(request_id, dict(sessionId=sid, prompt=[
                dict(type="text", text=f"waiting {request_id}")]))
        if ending == "cancel":
            agent._session_cancel(dict(sessionId=sid))
            assert all(event.wait(5) for event in received.values())
            assert [r["result"]["stopReason"] for r in transport.responses if r.get("id") in received] == [
                "cancelled", "cancelled"]
        else:
            agent._handle_eof(trigger="stdin_eof")
        runtime.release.set()
        agent.join_workers()
        assert len(runtime.calls) == 1
        assert not session.get("pending_prompts")
        with store.connection() as db:
            assert db.execute("SELECT COUNT(*) FROM source_events WHERE task_id=? AND origin=?",
                              (task_id, f"acp:{sid}")).fetchone()[0] == 0
    finally:
        runtime.release.set()
        agent.join_workers()


def test_background_waiter_and_other_session_keep_ownership(tmp_path, monkeypatch):
    from cortex_runtime.acp.protocol import RpcError
    agent, transport, runtime, store, sid, session, _ = _blocked_agent(tmp_path, monkeypatch)
    answers = []
    try:
        agent._queue_paseo_message(400, sid, session, json.dumps(dict(
            clientMessageId="q1", content=[dict(type="text", text="queued")])))
        assert runtime.entered.wait(5)
        session["question_waiter"] = ("question", lambda text: answers.append(text) or True)
        agent._session_prompt(401, dict(sessionId=sid, prompt=[dict(type="text", text="answer")]))
        assert answers == ["answer"]
        assert not session.get("pending_prompts")
        assert not session["active"].is_set()
        other = agent._session_new(dict(cwd=str(store.project_root), mcpServers=[]))["sessionId"]
        with pytest.raises(RpcError, match="existing task"):
            agent._queue_paseo_message(402, other, agent.sessions[other], json.dumps(dict(
                clientMessageId="q1", content=[dict(type="text", text="cross session")])))
        with pytest.raises(RpcError, match="Unknown sessionId"):
            agent._session_prompt(403, dict(sessionId="missing", prompt=[dict(type="text", text="no")]))
        assert len(runtime.calls) == 1
    finally:
        runtime.release.set()
        agent.join_workers()


def test_handoff_fifo_and_foreground_concurrency_refusal(tmp_path, monkeypatch):
    from cortex_runtime.acp.protocol import RpcError
    agent, transport, runtime, store, sid, session, task_id = _blocked_agent(tmp_path, monkeypatch)
    received = _responses_for(transport, 501, 502)
    try:
        agent._queue_paseo_message(500, sid, session, json.dumps(dict(
            clientMessageId="q1", content=[dict(type="text", text="queued")])))
        assert runtime.entered.wait(5)
        for request_id in (501, 502):
            agent._session_prompt(request_id, dict(sessionId=sid, prompt=[
                dict(type="text", text=str(request_id))]))
        runtime.release.set()
        assert all(event.wait(5) for event in received.values())
        agent.join_workers()
        assert [call[1] for call in runtime.calls] == ["queued", "501", "502"]
        assert len({call[3] for call in runtime.calls}) == 3
        # The original ACP foreground-in-flight refusal is unchanged.
        token = threading.Event()
        with session["lock"]:
            session["active"], session["background"] = token, False
        with pytest.raises(RpcError, match="already in flight"):
            agent._session_prompt(503, dict(sessionId=sid, prompt=[dict(type="text", text="no")]))
        assert not token.is_set()
        assert len(runtime.calls) == 3
        session["active"] = None
    finally:
        runtime.release.set()
        agent.join_workers()


def test_stop_between_handoff_reservation_and_launch_is_inert(tmp_path, monkeypatch):
    agent, transport, runtime, store, sid, session, task_id = _blocked_agent(tmp_path, monkeypatch)
    received = _responses_for(transport, 601)
    reserved, launch = threading.Event(), threading.Event()
    original = agent._launch_prompt
    def gated(args):
        reserved.set()
        assert launch.wait(5)
        original(args)
    monkeypatch.setattr(agent, "_launch_prompt", gated)
    try:
        agent._queue_paseo_message(600, sid, session, json.dumps(dict(
            clientMessageId="q1", content=[dict(type="text", text="queued")])))
        assert runtime.entered.wait(5)
        agent._session_prompt(601, dict(sessionId=sid, prompt=[dict(type="text", text="pending")]))
        runtime.release.set()
        assert reserved.wait(5)
        agent._session_cancel(dict(sessionId=sid))
        launch.set()
        assert received[601].wait(5)
        agent.join_workers()
        assert len(runtime.calls) == 1
        assert next(r for r in transport.responses if r.get("id") == 601)["result"]["stopReason"] == "cancelled"
        with store.connection() as db:
            assert db.execute("SELECT COUNT(*) FROM source_events WHERE task_id=? AND origin=?",
                              (task_id, f"acp:{sid}")).fetchone()[0] == 0
    finally:
        launch.set()
        runtime.release.set()
        agent.join_workers()


def test_prompt_cannot_take_session_while_runtime_cancel_is_unconfirmed(tmp_path, monkeypatch):
    from cortex_runtime.acp.protocol import RpcError
    agent, transport, runtime, store, sid, session, _ = _blocked_agent(tmp_path, monkeypatch)
    cancelling, confirm = threading.Event(), threading.Event()
    def cancel(_sid):
        cancelling.set()
        assert confirm.wait(5)
    runtime.cancel = cancel
    stopper = threading.Thread(target=lambda: agent._session_cancel(dict(sessionId=sid)))
    try:
        agent._queue_paseo_message(700, sid, session, json.dumps(dict(
            clientMessageId="q1", content=[dict(type="text", text="queued")])))
        assert runtime.entered.wait(5)
        stopper.start()
        assert cancelling.wait(5)
        runtime.release.set()
        agent.join_workers()
        with pytest.raises(RpcError, match="being cancelled"):
            agent._session_prompt(701, dict(sessionId=sid, prompt=[dict(type="text", text="no dispatch")]))
        assert len(runtime.calls) == 1
    finally:
        confirm.set()
        runtime.release.set()
        stopper.join(5)
        agent.join_workers()


def test_native_drain_answers_persisted_question_after_background_turn(tmp_path, monkeypatch):
    from cortex_runtime.acp.agent import Agent
    from cortex_runtime.acp import questions_store
    agent, transport, _, store, sid, session, task_id = _blocked_agent(tmp_path, monkeypatch)
    asked, release = threading.Event(), threading.Event()
    turns = []
    def router(store, channel, *_):
        def behavior(store, channel, _ref, message, _index):
            if channel.role != "coordinator":
                return publish_result(store, channel)
            context = mcp_tools.dispatch(store, channel, "task_get_context", {})
            turns.append(message)
            if len(turns) == 1:
                mcp_tools.dispatch(store, channel, "user_question_request", dict(
                    question="Which target?", options=["staging", "test"],
                    request_key="target-question"))
                asked.set()
                assert release.wait(5)
            elif context["state"] == "PLANNING":
                propose_inspection(store, channel)
            else:
                mcp_tools.dispatch(store, channel, "synthesis_submit", dict(text="Verified"))
            return PromptOutcome(StopReason.END_TURN, "Question/plan")
        return behavior
    runtime, _ = _make_runtime(store, router)
    replacement = Agent(runtime, environ=agent.environ)
    replacement.sessions = agent.sessions
    replacement.bind_transport(transport)
    agent = replacement
    received = _responses_for(transport, 801)
    try:
        agent._queue_paseo_message(800, sid, session, json.dumps(dict(
            clientMessageId="q1", content=[dict(type="text", text="queued")])))
        assert asked.wait(5)
        pending = questions_store.list_open(store, task_id)
        assert len(pending) == 1
        agent._session_prompt(801, dict(sessionId=sid, prompt=[dict(type="text", text="staging")]))
        assert not session["active"].is_set()
        release.set()
        assert received[801].wait(5)
        agent.join_workers()
        answered = questions_store.get(store, pending[0].id)
        assert answered.status == "answered"
        assert answered.answer_text == "staging"
        assert not questions_store.list_open(store, task_id)
        assert TaskMachine(store).context(task_id)["state"] == "COMPLETED"
        with store.connection() as db:
            assert db.execute("SELECT COUNT(*) FROM runtime_events WHERE task_id=? "
                              "AND type='USER_ANSWER_RECEIVED'", (task_id,)).fetchone()[0] == 1
    finally:
        release.set()
        agent.join_workers()
        runtime.shutdown()


def test_waiting_prompt_bound_refuses_without_capture_or_cancellation(tmp_path, monkeypatch):
    from cortex_runtime.acp.protocol import RpcError
    agent, transport, runtime, store, sid, session, task_id = _blocked_agent(tmp_path, monkeypatch)
    try:
        agent._queue_paseo_message(900, sid, session, json.dumps(dict(
            clientMessageId="q1", content=[dict(type="text", text="queued")])))
        assert runtime.entered.wait(5)
        for index in range(64):
            agent._session_prompt(901 + index, dict(sessionId=sid, prompt=[
                dict(type="text", text=str(index))]))
        with pytest.raises(RpcError, match="Too many prompts"):
            agent._session_prompt(1000, dict(sessionId=sid, prompt=[dict(type="text", text="overflow")]))
        assert len(session["pending_prompts"]) == 64
        assert not session["active"].is_set()
        with store.connection() as db:
            assert db.execute("SELECT COUNT(*) FROM source_events WHERE task_id=? AND origin=?",
                              (task_id, f"acp:{sid}")).fetchone()[0] == 0
        agent._session_cancel(dict(sessionId=sid))
        cancelled = [r for r in transport.responses if 901 <= r.get("id", 0) < 965]
        assert len(cancelled) == 64
        assert all(r["result"]["stopReason"] == "cancelled" for r in cancelled)
    finally:
        runtime.release.set()
        agent.join_workers()


def test_native_prompt_after_late_card_continuation_is_not_stranded(tmp_path, monkeypatch):
    from cortex_runtime.acp import questions_store
    agent, transport, runtime, store, sid, session, task_id = _blocked_agent(tmp_path, monkeypatch)
    entered, release = threading.Event(), threading.Event()
    question = questions_store.create_if_absent(store, task_id, "question", "Which target?")
    def answer_question(task_id, session_id, question, choice, emit_update, cancel_event):
        entered.set()
        assert release.wait(5)
        assert not cancel_event.is_set()
        assert questions_store.answer(store, question.id, choice, "elicitation")
        return StopReason.END_TURN
    runtime.answer_question = answer_question
    received = _responses_for(transport, 1101)
    answer = threading.Thread(target=agent._run_late_answer, args=(sid, session, question, "staging"))
    try:
        answer.start()
        assert entered.wait(5)
        agent._session_prompt(1101, dict(sessionId=sid, prompt=[dict(type="text", text="native")]))
        assert not session["active"].is_set()
        release.set()
        # _BlockedRuntime gates its first ordinary prompt too.
        runtime.release.set()
        assert received[1101].wait(5)
        answer.join(5)
        agent.join_workers()
        assert not answer.is_alive()
        assert [call[1] for call in runtime.calls] == ["native"]
        assert questions_store.get(store, question.id).answer_text == "staging"
        assert not session.get("pending_prompts")
    finally:
        release.set()
        runtime.release.set()
        answer.join(5)
        agent.join_workers()
