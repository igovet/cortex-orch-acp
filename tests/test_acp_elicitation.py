"""T020: real ``clientCapabilities.elicitation.form`` production wiring --
end to end through the real ``acp.agent.Agent``/``StdioTransport`` (a pipe,
not a subprocess -- see ``tests/test_acp_host_requests.py``'s identical
pattern) with a real ``OrchestratingRuntime`` (fakes-backed, no real
engine/network/paid model) and a scripted fake ACP client driving the
transport directly.
"""
from __future__ import annotations

import json
import os
import threading
import time
import uuid
from pathlib import Path

import pytest

from cortex_runtime.acp.agent import Agent, _PaseoQuestionElicitor
from cortex_runtime.acp import questions_store
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.acp import pipeline_bootstrap
from cortex_runtime.acp.transport import StdioTransport
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace

jsonschema = pytest.importorskip("jsonschema")


def test_paseo_question_bridge_uses_core_permission_choice_schema():
    seen = {}

    class Requester:
        def request(self, method, params, timeout):
            seen.update(method=method, params=params, timeout=timeout)
            option_id = "cortex-question-2" if len(params["options"]) > 1 else "cortex-question-1"
            return {"outcome": {"outcome": "selected", "optionId": option_id}}

    elicitor = _PaseoQuestionElicitor(Requester(), session_id="sess-1", timeout=2.0)
    assert elicitor.elicit("ignored", "Pick", ["one", "two"]) == "two"
    assert seen["method"] == "session/request_permission"
    assert seen["params"]["_meta"] == {"cortexQuestion": True}
    assert seen["params"]["toolCall"]["title"] == "Pick"
    assert [item["name"] for item in seen["params"]["options"]] == ["one", "two"]
    elicitor.set_question_id("pq-durable")
    assert elicitor.elicit("ignored", "Pick", ["one"]) == "one"
    assert seen["params"]["toolCall"]["rawInput"] == {
        "__cortex_question": True, "questionId": "pq-durable", "question": "Pick",
    }
    _schema_validator("RequestPermissionRequest").validate(seen["params"])


def test_paseo_env_enables_bridge_without_elicitation_capability():
    from acp_fakes import FakeRuntime

    agent = Agent(FakeRuntime(), environ={"CORTEX_PASEO_QUESTION_UI": "1"})
    agent.host_requester = object()
    assert agent._make_elicitor("sess-1").__class__.__name__ == "_PaseoQuestionElicitor"

SCHEMA_PATH = Path(__file__).resolve().parent / "fixtures" / "acp" / "schema-1.4.0.json"


def _schema_validator(def_name):
    document = json.loads(SCHEMA_PATH.read_text())
    resolver = jsonschema.RefResolver.from_schema(document)
    return jsonschema.Draft202012Validator({"$ref": f"#/$defs/{def_name}"}, resolver=resolver)


class _RecordingStdout:
    def __init__(self):
        self.lines = []
        self._lock = threading.Lock()

    def write(self, data):
        with self._lock:
            for line in data.splitlines():
                if line:
                    self.lines.append(json.loads(line.decode("utf-8")))

    def flush(self):
        pass


class _Harness:
    """One real Agent, driven over a real pipe-backed StdioTransport."""

    def __init__(self, tmp_path, router, *, environ=None, elicitation_timeout_seconds=None):
        self.tmp_path = tmp_path
        self.store = Store(tmp_path / ".cortex", project_root=tmp_path)
        self.engine = FakeSessionRuntime(self.store, router)
        self.runtime = OrchestratingRuntime(
            self.store, self.engine, FakeWorkspace(), FakeConfigSnapshotSource(),
            allow_unqualified_delivery=True, wait_timeout_seconds=15.0,
        )
        # A pipe-backed test transport is live enough to trigger prompt-free
        # catalog refresh. Give it an empty private connection registry so
        # tests never launch the user's configured production engines.
        if environ is None:
            home = tmp_path.parent / f"{tmp_path.name}-home"
            home.mkdir(exist_ok=True)
            environ = {"HOME": str(home), "XDG_CONFIG_HOME": str(home / ".config"),
                       "XDG_STATE_HOME": str(home / ".local" / "state")}
        self.agent = Agent(self.runtime, environ=environ, elicitation_timeout_seconds=elicitation_timeout_seconds)
        self.stdout = _RecordingStdout()
        self.read_fd, self.write_fd = os.pipe()
        self.stdin = os.fdopen(self.read_fd, "rb")
        self.transport = StdioTransport(self.agent.handle_message, raw_stdout=self.stdout, raw_stdin=self.stdin)
        self.agent.bind_transport(self.transport)
        self._request_id = 0

    def start(self):
        self.transport.start()

    def stop(self):
        self.transport.stop_writer()
        try:
            os.close(self.write_fd)
        except OSError:
            pass
        try:
            self.stdin.close()
        except OSError:
            pass

    def send(self, method, params, *, notification=False):
        self._request_id += 1
        request_id = self._request_id
        body = dict(jsonrpc="2.0", method=method, params=params)
        if not notification:
            body["id"] = request_id
        os.write(self.write_fd, (json.dumps(body) + "\n").encode("utf-8"))
        return request_id

    def send_raw(self, body):
        os.write(self.write_fd, (json.dumps(body) + "\n").encode("utf-8"))

    def wait_for(self, predicate, timeout=10.0):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            with self.stdout._lock:
                for message in self.stdout.lines:
                    if predicate(message):
                        return message
            time.sleep(0.01)
        raise AssertionError(f"timed out waiting for a matching message; got {self.stdout.lines}")

    def response_for(self, request_id, timeout=10.0):
        return self.wait_for(lambda m: m.get("id") == request_id and ("result" in m or "error" in m), timeout=timeout)


def _project(tmp_path):
    # Must match the harness's own Store's project_root exactly (see
    # _Harness.__init__: Store(tmp_path / ".cortex", project_root=tmp_path))
    # -- Agent._store_for(cwd) independently computes
    # project_store_directory(logical_project_root(canonical_project(cwd)))
    # == cwd/".cortex", so cwd must be tmp_path itself, not a subdirectory,
    # for both to resolve to the identical on-disk store.
    return tmp_path


def _initialize_and_new_session(harness, project, *, elicitation_form):
    client_capabilities = {}
    if elicitation_form is not None:
        client_capabilities["elicitation"] = dict(form=elicitation_form)
    init_id = harness.send("initialize", dict(protocolVersion=1, clientCapabilities=client_capabilities))
    harness.response_for(init_id)
    new_id = harness.send("session/new", dict(cwd=str(project), mcpServers=[]))
    new_response = harness.response_for(new_id)
    session_id = new_response["result"]["sessionId"]
    return session_id


def _ask_user_then_accept_router(question="Which environment?", options=None, observed=None):
    from runtime_scenarios import propose_inspection, publish_result
    phase = {"asked": False}
    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if not phase["asked"]:
            phase["asked"] = True
            args = dict(question=question, request_key="ask1")
            if options is not None: args["options"] = options
            mcp_tools.dispatch(store, channel, "user_question_request", args)
        else:
            context = mcp_tools.dispatch(store, channel, "task_get_context", {})
            if observed is not None: observed["continuation"] = message
            if context['state'] == 'PLANNING': propose_inspection(store,channel)
            elif context['state'] == 'RECONCILING':
                mcp_tools.dispatch(store,channel,'synthesis_submit',dict(text='Answered and verified.'))
        return PromptOutcome(StopReason.END_TURN, "")
    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel.role == "coordinator" else (
            lambda store,channel,*args: publish_result(store,channel))
    return router



def test_elicitation_form_advertised_and_accepted_resolves_same_pq_id_without_ending_turn(tmp_path):
    observed = {}
    router = _ask_user_then_accept_router(observed=observed)
    harness = _Harness(tmp_path, router)
    harness.start()
    try:
        session_id = _initialize_and_new_session(harness, _project(tmp_path), elicitation_form=True)
        prompt_id = harness.send("session/prompt", dict(
            sessionId=session_id, prompt=[dict(type="text", text="Deploy it.")]))

        elicit_request = harness.wait_for(lambda m: m.get("method") == "elicitation/create")
        assert isinstance(elicit_request.get("id"), str) and elicit_request["id"].startswith("hr_")
        params = elicit_request["params"]
        _schema_validator("CreateElicitationRequest").validate(params)
        assert params["mode"] == "form"
        assert params["sessionId"] == session_id
        assert params["requestedSchema"]["required"] == ["answer"]

        harness.send_raw(dict(jsonrpc="2.0", id=elicit_request["id"],
                               result=dict(action="accept", content=dict(answer="staging"))))

        final = harness.response_for(prompt_id)
        assert final["result"]["stopReason"] == "end_turn"

        with harness.store.connection() as db:
            row = db.execute("SELECT state FROM task_orchestration").fetchone()
        assert row["state"] == "accepted"
        task_id = next(iter(harness.agent.sessions.values()))["task_id"]
        questions = questions_store.list_open(harness.store, task_id)
        assert questions == []
        with harness.store.connection() as db:
            answered = db.execute(
                "SELECT id, status, answer_text, answer_source FROM pending_questions WHERE task_id=?", (task_id,),
            ).fetchall()
        assert len(answered) == 1
        assert answered[0]["status"] == "answered"
        assert answered[0]["answer_text"] == "staging"
        assert answered[0]["answer_source"] == "elicitation"
        assert "staging" in observed["continuation"]
    finally:
        harness.stop()


def test_elicitation_not_advertised_uses_text_fallback(tmp_path):
    observed = {}
    router = _ask_user_then_accept_router(observed=observed)
    harness = _Harness(tmp_path, router)
    harness.start()
    try:
        session_id = _initialize_and_new_session(harness, _project(tmp_path), elicitation_form=None)
        prompt_id = harness.send("session/prompt", dict(
            sessionId=session_id, prompt=[dict(type="text", text="Deploy it.")]))
        final = harness.response_for(prompt_id)
        assert final["result"]["stopReason"] == "end_turn"
        # Never an outbound elicitation/create request.
        assert not any(m.get("method") == "elicitation/create" for m in harness.stdout.lines)
        chunk = harness.wait_for(lambda m: m.get("params", {}).get("update", {}).get("content", {}).get("text") == "Which environment?")
        assert chunk is not None

        with harness.store.connection() as db:
            row = db.execute("SELECT state FROM task_orchestration").fetchone()
        assert row["state"] == "waiting_for_user"

        prompt_id2 = harness.send("session/prompt", dict(
            sessionId=session_id, prompt=[dict(type="text", text="Use staging.")]))
        final2 = harness.response_for(prompt_id2)
        assert final2["result"]["stopReason"] == "end_turn"
        with harness.store.connection() as db:
            row = db.execute("SELECT state FROM task_orchestration").fetchone()
        assert row["state"] == "accepted"
        task_id = next(iter(harness.agent.sessions.values()))["task_id"]
        with harness.store.connection() as db:
            answered = db.execute(
                "SELECT status, answer_text, answer_source FROM pending_questions WHERE task_id=?", (task_id,),
            ).fetchall()
        assert len(answered) == 1
        assert answered[0]["status"] == "answered"
        assert answered[0]["answer_text"] == "Use staging."
        assert answered[0]["answer_source"] == "text"
        assert "Use staging." in observed["continuation"]
    finally:
        harness.stop()


def test_elicitation_declined_falls_back_then_next_prompt_answers_same_pq_and_late_reply_is_inert(tmp_path):
    router = _ask_user_then_accept_router()
    harness = _Harness(tmp_path, router)
    harness.start()
    try:
        session_id = _initialize_and_new_session(harness, _project(tmp_path), elicitation_form=True)
        prompt_id = harness.send("session/prompt", dict(
            sessionId=session_id, prompt=[dict(type="text", text="Deploy it.")]))
        elicit_request = harness.wait_for(lambda m: m.get("method") == "elicitation/create")
        harness.send_raw(dict(jsonrpc="2.0", id=elicit_request["id"], result=dict(action="decline")))

        final = harness.response_for(prompt_id)
        assert final["result"]["stopReason"] == "end_turn"
        with harness.store.connection() as db:
            row = db.execute("SELECT state FROM task_orchestration").fetchone()
        assert row["state"] == "waiting_for_user"
        task_id = next(iter(harness.agent.sessions.values()))["task_id"]
        open_before = questions_store.list_open(harness.store, task_id)
        assert len(open_before) == 1
        pq_id = open_before[0].id

        # A late reply to the already-declined elicitation: inert (no
        # matching pending HostRequester call any more -- dropped/ignored,
        # never crashes and never re-resolves anything).
        harness.send_raw(dict(jsonrpc="2.0", id=elicit_request["id"],
                               result=dict(action="accept", content=dict(answer="late-and-ignored"))))
        time.sleep(0.1)

        prompt_id2 = harness.send("session/prompt", dict(
            sessionId=session_id, prompt=[dict(type="text", text="Use staging.")]))
        final2 = harness.response_for(prompt_id2)
        assert final2["result"]["stopReason"] == "end_turn"
        with harness.store.connection() as db:
            row = db.execute("SELECT state FROM task_orchestration").fetchone()
        assert row["state"] == "accepted"
        resolved = questions_store.get(harness.store, pq_id)
        assert resolved.status == "answered" and resolved.answer_text == "Use staging."
        assert resolved.answer_source == "text"  # resolved via the fallback, not the late reply
    finally:
        harness.stop()


def test_elicitation_timeout_falls_back_bounded(tmp_path):
    router = _ask_user_then_accept_router()
    harness = _Harness(tmp_path, router, elicitation_timeout_seconds=0.3)
    harness.start()
    try:
        session_id = _initialize_and_new_session(harness, _project(tmp_path), elicitation_form=True)
        prompt_id = harness.send("session/prompt", dict(
            sessionId=session_id, prompt=[dict(type="text", text="Deploy it.")]))
        harness.wait_for(lambda m: m.get("method") == "elicitation/create")
        # Never reply -- the real HostRequester's own bounded timeout (0.3s
        # here) must resolve this to None (fallback), not hang.
        began = time.monotonic()
        final = harness.response_for(prompt_id, timeout=10.0)
        elapsed = time.monotonic() - began
        assert elapsed < 5.0
        assert final["result"]["stopReason"] == "end_turn"
        with harness.store.connection() as db:
            row = db.execute("SELECT state FROM task_orchestration").fetchone()
        assert row["state"] == "waiting_for_user"
    finally:
        harness.stop()


def test_elicitation_timeout_constructor_param_is_validated_bounded(tmp_path):
    from cortex_runtime.acp.agent import Agent
    from acp_fakes import FakeRuntime

    Agent(FakeRuntime(), elicitation_timeout_seconds=1.0)  # in-range: fine
    with pytest.raises(ValueError):
        Agent(FakeRuntime(), elicitation_timeout_seconds=0.0)
    with pytest.raises(ValueError):
        Agent(FakeRuntime(), elicitation_timeout_seconds=10_000.0)
    with pytest.raises(ValueError):
        Agent(FakeRuntime(), elicitation_timeout_seconds=-1.0)
