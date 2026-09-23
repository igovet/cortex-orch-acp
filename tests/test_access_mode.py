"""The user's Guarded/Auto access mode (``configuration.access_mode``).

Guarded is the default everywhere. Auto is only ever set by the user's own
client, is recorded as a revision, lets the runtime answer approval prompts
itself and opens workers in the most permissive engine mode their session
offers -- while every refusal that is not the user's to give still holds.
"""
from __future__ import annotations

import json
import sqlite3
import threading
import time
import uuid
from types import SimpleNamespace

import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.configuration import access_mode
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.engine.correlation import CorrelationTables
from cortex_runtime.engine.permissions import PermissionBridge
from cortex_runtime.engine.resource_binding import BindingRegistry
from cortex_runtime.orchestration import consent, gateway
from cortex_runtime.orchestration.gateway import dispatch
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel

from test_acp_config_options import _bootstrap_task_for_session, _isolate, _new_agent
from test_delegation_worker_mode_floor import _bootstrap, _connection_and_config, _propose_and_apply
from test_engine_permissions import PARAMS, _binding, call

ALLOW = [dict(optionId="yes", kind="allow_once"), dict(optionId="no", kind="reject_once")]


def _store_and_task(tmp_path):
    project = tmp_path / "project"
    project.mkdir(exist_ok=True)
    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Do work.", "owner-1")
    return store, result.task_id


def _runtime(store, requesters=None):
    """The real ``permission_requester_for_task`` over a minimal runtime state."""
    fake = SimpleNamespace(_store=store, _lock=threading.Lock(), _task_permission_requesters=dict(requesters or {}))
    fake.permission_requester_for_task = lambda task_id: OrchestratingRuntime.permission_requester_for_task(fake, task_id)
    return fake


# -- default and recording ---------------------------------------------------

def test_default_is_guarded_and_reading_never_creates_the_table(tmp_path):
    store, task_id = _store_and_task(tmp_path)
    assert access_mode.DEFAULT == access_mode.GUARDED
    assert access_mode.current(store, task_id) == "guarded"
    assert access_mode.current(store, None) == "guarded"
    assert access_mode.current(None, task_id) == "guarded"
    with store.connection() as db:
        assert db.execute("SELECT 1 FROM sqlite_master WHERE name='access_mode_events'").fetchone() is None


def test_switch_is_an_append_only_revision_with_its_origin(tmp_path):
    store, task_id = _store_and_task(tmp_path)
    first = access_mode.switch(store, task_id, "auto", origin="acp_set_mode", session_id="acps_1")
    assert (first["revision"], first["previous"]) == (1, "guarded")
    assert access_mode.current(store, task_id) == "auto"
    second = access_mode.switch(store, task_id, "guarded", origin="acp_config_option")
    assert (second["revision"], second["previous"]) == (2, "auto")
    assert access_mode.current(store, task_id) == "guarded"
    assert [(row["mode"], row["origin"]) for row in access_mode.history(store, task_id)] == [
        ("auto", "acp_set_mode"), ("guarded", "acp_config_option")]


@pytest.mark.parametrize("mode,origin", [("yolo", "acp_set_mode"), ("auto", "model"), ("auto", "mcp"), ("auto", "runtime")])
def test_unknown_mode_or_non_user_origin_is_refused_and_records_nothing(tmp_path, mode, origin):
    store, task_id = _store_and_task(tmp_path)
    with pytest.raises(access_mode.InvalidAccessMode):
        access_mode.switch(store, task_id, mode, origin=origin)
    assert access_mode.current(store, task_id) == "guarded"


def test_unknown_stored_value_reads_as_guarded(tmp_path):
    store, task_id = _store_and_task(tmp_path)
    access_mode.switch(store, task_id, "auto", origin="acp_set_mode")
    with store.connection() as db:
        db.execute("UPDATE access_mode_events SET mode='everything' WHERE task_id=?", (task_id,))
        db.commit()
    assert access_mode.current(store, task_id) == "guarded"


def test_no_model_callable_tool_can_change_the_mode():
    """Only the ACP client's own controls reach ``switch``: no MCP tool or
    gateway operation names or writes the access mode."""
    texts = json.dumps([mcp_tools.__dict__.get(name) for name in dir(mcp_tools) if name.isupper()], default=str)
    assert "access_mode" not in texts and "access mode" not in texts.lower()
    assert not any("access" in name for name in gateway.SCHEMAS)
    assert "access_mode" not in json.dumps(gateway.SCHEMAS)


# -- the runtime's one approval choke point ------------------------------------

def test_guarded_uses_only_the_registered_host_requester(tmp_path):
    store, task_id = _store_and_task(tmp_path)
    assert _runtime(store).permission_requester_for_task(task_id) is None
    host = object()
    assert _runtime(store, {task_id: host}).permission_requester_for_task(task_id) is host


def test_auto_answers_without_any_host_and_audits_without_the_command(tmp_path):
    store, task_id = _store_and_task(tmp_path)
    asked = []
    runtime = _runtime(store, {task_id: lambda *args: asked.append(args)})
    access_mode.switch(store, task_id, "auto", origin="acp_set_mode")
    requester = runtime.permission_requester_for_task(task_id)
    params = dict(toolCall=dict(toolCallId="tc-9", kind="execute", rawInput=dict(command="rm -rf build TOKEN=s3cret")),
                  options=ALLOW)
    assert requester("es_a", params, 5) == {"outcome": {"outcome": "selected", "optionId": "yes"}}
    # Only a standing allow offered: that one; no allow at all: cancelled.
    assert requester("es_a", dict(params, options=[dict(optionId="always", kind="allow_always")]), 5)["outcome"]["optionId"] == "always"
    assert requester("es_a", dict(params, options=[dict(optionId="no", kind="reject_once")]), 5) == {"outcome": {"outcome": "cancelled"}}
    assert asked == []
    # Also with no connection live at all.
    assert _runtime(store).permission_requester_for_task(task_id) is not None
    audit = [row for row in access_mode.history(store, task_id) if row["kind"] != "switched"]
    assert [row["kind"] for row in audit] == ["auto_approved", "auto_approved", "auto_unanswerable"]
    with store.connection() as db:
        stored = " ".join(row[0] for row in db.execute("SELECT detail_json FROM access_mode_events"))
    assert "s3cret" not in stored and "rm -rf" not in stored


def test_consent_card_is_allowed_by_the_runtime_in_auto_and_refused_without_a_host_in_guarded(tmp_path):
    store, task_id = _store_and_task(tmp_path)
    runtime = _runtime(store)
    channel = SimpleNamespace(task_id=task_id, attempt_id="xa_1")

    def ask():
        outcome = {}
        thread = threading.Thread(target=lambda: outcome.setdefault(
            "allowed", consent.request(store, channel, "outside_write", "/tmp/x", timeout=10)))
        thread.start()
        deadline = time.monotonic() + 5
        while thread.is_alive() and time.monotonic() < deadline:
            consent.relay(runtime, task_id, "en")
            time.sleep(0.05)
        thread.join(5)
        return outcome["allowed"]

    assert ask() is False  # Guarded, nobody to ask: refused, as before
    access_mode.switch(store, task_id, "auto", origin="acp_config_option")
    assert ask() is True


# -- invariants that are not the user's to give --------------------------------

def _auto_bridge(store, task_id, tmp_path, role="worker"):
    bindings = BindingRegistry()
    bindings.bind(_binding(tmp_path, role=role))
    runtime = _runtime(store)
    access_mode.switch(store, task_id, "auto", origin="acp_set_mode")
    requester = runtime.permission_requester_for_task(task_id)
    return bindings, PermissionBridge(correlation=CorrelationTables(), bindings=bindings,
                                      host_permission_requester=requester)


def test_auto_confirms_a_native_deletion_without_asking(tmp_path):
    store, task_id = _store_and_task(tmp_path)
    _bindings, bridge = _auto_bridge(store, task_id, tmp_path)
    params = dict(PARAMS, toolCall=dict(toolCallId="tc-1", name="Bash", kind="execute", rawInput=dict(command="rm old.txt")))
    assert call(bridge, params) == {"outcome": {"outcome": "selected", "optionId": "allow"}}


@pytest.mark.parametrize("role", ["worker", "coordinator"])
@pytest.mark.parametrize("name", ["Agent", "Task", "spawn_agent"])
def test_auto_still_refuses_native_subagents(tmp_path, role, name):
    store, task_id = _store_and_task(tmp_path)
    _bindings, bridge = _auto_bridge(store, task_id, tmp_path, role=role)
    params = dict(PARAMS, toolCall=dict(toolCallId="tc-1", name=name, rawInput=dict(command="rm x")))
    assert call(bridge, params) == {"outcome": {"outcome": "selected", "optionId": "deny"}}
    assert all(row["kind"] == "switched" for row in access_mode.history(store, task_id))


def test_auto_never_answers_for_a_stale_binding(tmp_path):
    store, task_id = _store_and_task(tmp_path)
    bindings, bridge = _auto_bridge(store, task_id, tmp_path)
    bindings.release("es_a")
    params = dict(PARAMS, toolCall=dict(toolCallId="tc-1", name="Bash", rawInput=dict(command="rm old.txt")))
    assert call(bridge, params) == {"outcome": {"outcome": "cancelled"}}


def test_auto_leaves_gateway_scopes_role_rules_and_the_store_protected(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    boot = bootstrap_task(store, SourceEvent(origin="gateway-test", event_id="one"), "Implement", "owner")
    TaskMachine(store).create(boot.task_id, "Implement")
    channel = resolve_channel(store, boot.channel_id, boot.secret)
    access_mode.switch(store, boot.task_id, "auto", origin="acp_set_mode")
    (tmp_path / "hello").write_text("hello")
    for name, args, error in [
        ("project_write", dict(path="hello", content="bad", expected_sha256="absent", request_key="a"), "gateway_scope_denied"),
        ("project_delete", dict(path="hello", expected_sha256="x", request_key="b"), "gateway_scope_denied"),
        ("outside_write", dict(path=str(tmp_path.parent / "x"), content="bad", request_key="c"), "gateway_scope_denied"),
        ("command_run", dict(argv=["true"], request_key="d"), "gateway_command_denied"),
        ("project_read", dict(path=".cortex/cortex.sqlite3"), "gateway_path_denied"),
    ]:
        with pytest.raises(StoreError, match=error):
            dispatch(store, channel, name, args)
    assert (tmp_path / "hello").read_text() == "hello"


# -- worker engine mode ----------------------------------------------------------

@pytest.mark.parametrize("engine,offered,access,expected", [
    ("claude-agent-acp", {"default", "acceptEdits", "plan", "auto", "bypassPermissions"}, "auto", "bypassPermissions"),
    ("claude-agent-acp", {"default", "acceptEdits"}, "auto", "acceptEdits"),
    ("claude-agent-acp", {"default", "acceptEdits", "bypassPermissions"}, "guarded", "default"),
    ("codex-acp", {"read-only", "agent", "agent-full-access"}, "auto", "agent-full-access"),
    ("codex-acp", {"read-only", "agent"}, "auto", "agent"),
    ("codex-acp", {"read-only", "agent", "agent-full-access"}, "guarded", "agent"),
    ("unknown-engine", {"anything"}, "auto", None),
])
def test_worker_mode_choice(engine, offered, access, expected):
    floor = {"claude-agent-acp": "default", "codex-acp": "agent"}.get(engine)
    assert access_mode.worker_mode(engine, offered, access=access, floor=floor) == expected


def _open_worker(tmp_path, access, mode_options=None):
    store, task_id, epoch = _bootstrap(tmp_path)
    kwargs = {} if mode_options is None else dict(mode_options=mode_options)
    environ, connection = _connection_and_config(tmp_path, **kwargs)
    row = _propose_and_apply(store, task_id, epoch, connection)
    if access == "auto":
        access_mode.switch(store, task_id, "auto", origin="acp_set_mode")
    adapter = EngineSessionManagerAdapter(
        lambda t: connection, lambda t: str(tmp_path), environ=environ, call_timeout=10.0, store=store)
    session_ref = adapter.open(
        "shared_project_root",
        dict(task_id=task_id, attempt_id="xa_w1", assignment_id="asg1", config_revision=row["id"]), "instructions")
    observed = adapter._managers["xa_w1"].observed_config_options(session_ref)
    mode = next(option for option in observed if option.get("id") == "mode")["currentValue"]
    adapter.terminate_owned("xa_w1")
    return mode


def test_worker_opened_in_auto_is_confirmed_in_the_most_permissive_offered_mode(tmp_path):
    assert _open_worker(tmp_path, "auto") == "agent-full-access"


def test_worker_opened_in_guarded_keeps_the_floor(tmp_path):
    assert _open_worker(tmp_path, "guarded") == "agent"


def test_worker_in_auto_falls_back_to_the_floor_when_nothing_broader_is_offered(tmp_path):
    options = [dict(value="read-only", name="Ask"), dict(value="agent", name="Approve for me")]
    assert _open_worker(tmp_path, "auto", mode_options=options) == "agent"


# -- the client surface ------------------------------------------------------------

def test_zed_gets_an_access_option_and_a_mid_task_switch_is_recorded(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    assert "modes" not in result
    option = next(item for item in result["configOptions"] if item["id"] == "access_mode")
    assert (option["category"], option["currentValue"]) == ("mode", "guarded")
    assert [(item["value"], item["name"]) for item in option["options"]] == [
        ("guarded", "Guarded mode"), ("auto", "Auto mode")]
    store, task_id = _bootstrap_task_for_session(agent, session_id, tmp_path)

    agent._session_set_config_option(5, dict(sessionId=session_id, configId="access_mode", value="auto"))
    response = next(item for item in transport.responses if item.get("id") == 5)
    assert next(item for item in response["result"]["configOptions"] if item["id"] == "access_mode")["currentValue"] == "auto"
    assert access_mode.current(store, task_id) == "auto"
    assert [(row["origin"], row["detail"]["session_id"]) for row in access_mode.history(store, task_id)] == [
        ("acp_config_option", session_id)]

    agent._session_set_config_option(6, dict(sessionId=session_id, configId="access_mode", value="everything"))
    assert "error" in next(item for item in transport.responses if item.get("id") == 6)
    assert access_mode.current(store, task_id) == "auto"


def test_pre_task_choice_becomes_the_new_tasks_first_revision(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    agent, _transport = _new_agent(tmp_path, environ)
    agent.client_info = {"name": "Paseo"}
    session_id = agent._session_new(dict(cwd=str(project), mcpServers=[]))["sessionId"]
    agent._session_set_mode(3, dict(sessionId=session_id, modeId="auto"))
    agent._session_prompt(4, dict(sessionId=session_id, prompt=[dict(type="text", text="run")]))
    agent._workers[-1].join(timeout=10)
    task_id = agent.sessions[session_id]["task_id"]
    store = agent._store_for(agent.sessions[session_id]["project"])
    assert access_mode.current(store, task_id) == "auto"
    assert [row["origin"] for row in access_mode.history(store, task_id)] == ["acp_pre_task_selection"]
    assert agent.sessions[session_id]["pending_access_mode"] is None


def test_a_session_that_never_chose_leaves_its_task_guarded(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    agent, _transport = _new_agent(tmp_path, environ)
    session_id = agent._session_new(dict(cwd=str(project), mcpServers=[]))["sessionId"]
    agent._session_prompt(4, dict(sessionId=session_id, prompt=[dict(type="text", text="run")]))
    agent._workers[-1].join(timeout=10)
    store = agent._store_for(agent.sessions[session_id]["project"])
    task_id = agent.sessions[session_id]["task_id"]
    assert access_mode.current(store, task_id) == "guarded"
    assert access_mode.history(store, task_id) == []
