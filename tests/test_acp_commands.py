"""``commands.py`` recognition/resolution unit tests and command<->
set_config_option parity tests (T010, UI-11).
"""
from __future__ import annotations

import threading
import uuid
from types import SimpleNamespace

import pytest

from cortex_runtime.acp import commands as acp_commands
from cortex_runtime.acp import config_options
from cortex_runtime.acp.agent import Agent
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.acp import sessions as acp_sessions
from cortex_runtime.catalog import discovery, store as catalog_store
from cortex_runtime.configuration.service import ConfigurationService
from cortex_runtime.connections import onboarding
from cortex_runtime.connections.model import AuthMode


def _isolate(tmp_path, monkeypatch):
    config_home = tmp_path / "config"
    state_home = tmp_path / "state"
    home = tmp_path / "home"
    for path in (config_home, state_home, home):
        path.mkdir()
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    monkeypatch.setenv("XDG_STATE_HOME", str(state_home))
    monkeypatch.setenv("HOME", str(home))
    return dict(XDG_CONFIG_HOME=str(config_home), XDG_STATE_HOME=str(state_home), HOME=str(home))


def _configured_connection(name="work", environ=None):
    record = onboarding.create_connection(
        name=name, engine="codex-acp", provider="openai", auth_mode=AuthMode.API_KEY,
        key_kind="openai_api_key", endpoint=f"cortex-test:{name}", environ=environ)
    return onboarding.set_api_key(record.id, reader=lambda: b"fake-test-secret-key", environ=environ)


def _save_snapshot(record, *, environ, model_id="model-a", thought_level="high"):
    options = [dict(id="model", name="Model", type="select", category="model", currentValue=model_id,
                     options=[dict(value=model_id, name="Model A")])]
    if thought_level is not None:
        options.append(dict(id="thought_level", name="Thought level", type="select", category="thought_level",
                             currentValue=thought_level, options=[dict(value=thought_level, name=thought_level)]))
    observation = discovery.parse_observation(options)
    snapshot = discovery.build_catalog({record.id: record}, {record.id: observation})
    catalog_store.save_snapshot(snapshot, environ)
    return snapshot


# --- recognition/resolution --------------------------------------------------

def test_recognize_requires_exactly_one_text_block():
    assert acp_commands.recognize([dict(type="text", text="/cortex-status")]) == ("cortex-status", "")
    assert acp_commands.recognize([
        dict(type="text", text="/cortex-status"), dict(type="text", text="more"),
    ]) is None
    assert acp_commands.recognize([dict(type="resource_link", name="a", uri="file:///a")]) is None


def test_recognize_requires_end_or_whitespace_after_name():
    assert acp_commands.recognize([dict(type="text", text="/cortex-modelfoo")]) is None
    assert acp_commands.recognize([dict(type="text", text="/cortex-model gpt-5")]) == ("cortex-model", "gpt-5")
    assert acp_commands.recognize([dict(type="text", text="  /cortex-effort  high  ")]) == ("cortex-effort", "high")


def test_unknown_slash_command_is_not_recognized():
    assert acp_commands.recognize([dict(type="text", text="/cortex-doesnotexist")]) is None
    assert acp_commands.recognize([dict(type="text", text="just a normal prompt")]) is None


def test_resolve_model_argument_by_route_id_or_label(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _save_snapshot(record, environ=environ)
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    entry = entries[0]
    assert acp_commands.resolve_model_argument(entry.route.id, entries) == entry.route.id
    assert acp_commands.resolve_model_argument(entry.disambiguated_label, entries) == entry.route.id
    assert acp_commands.resolve_model_argument(entry.disambiguated_label.upper(), entries) == entry.route.id
    assert acp_commands.resolve_model_argument("no-such-model", entries) is None


def test_available_commands_never_include_team_permission_spend_summarizer():
    names = {item["name"] for item in acp_commands.AVAILABLE_COMMANDS}
    for forbidden in ("team", "permission", "spend", "summarizer"):
        assert not any(forbidden in name for name in names)


# --- command <-> set_config_option parity (in-process Agent) ---------------

class _RecordingTransport:
    def __init__(self):
        self.responses = []
        self.notifications = []
        self._lock = threading.Lock()

    def send_response(self, message):
        with self._lock:
            self.responses.append(message)

    def send_notification(self, message, timeout=None):
        with self._lock:
            self.notifications.append(message)
        return True


class _NoopRuntime:
    def start_prompt(self, task_id, session_id, content, emit_update, cancel_event):
        raise AssertionError("a recognized command must never reach the runtime")

    def cancel(self, session_id):
        return None


def _new_agent(environ):
    agent = Agent(_NoopRuntime(), environ=environ)
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    agent.client_capabilities = {}
    return agent, transport


def _bootstrap(agent, session_id):
    session = agent.sessions[session_id]
    store = agent._store_for(session["project"])
    origin = f"acp:{session_id}"
    event = SimpleNamespace(origin=origin, event_id=str(uuid.uuid4()))
    result = bootstrap_task(store, event, "hello", owner_ref=origin)
    session["task_id"] = result.task_id
    session["owner_epoch"] = result.owner_epoch
    acp_sessions.bind_task(store, session_id, result.task_id)
    return store, result.task_id


def test_cortex_model_command_reaches_same_configuration_service_outcome(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _save_snapshot(record, environ=environ)
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    store, task_id = _bootstrap(agent, session_id)

    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    route_id = entries[0].route.id
    session = agent.sessions[session_id]
    cancel_event = threading.Event()
    recognized = ("cortex-model", route_id)
    agent._run_command_body(request_id=42, session_id=session_id, session=session,
                             recognized=recognized, cancel_event=cancel_event)

    final = next(message for message in transport.responses if message.get("id") == 42)
    assert final["result"]["stopReason"] == "end_turn"
    chunks = [n for n in transport.notifications
              if n["params"]["update"]["sessionUpdate"] == "agent_message_chunk"]
    assert chunks and "pending" in chunks[-1]["params"]["update"]["content"]["text"]

    service = ConfigurationService(store)
    history = service.history(task_id)
    assert len(history) == 1 and history[0]["route_id"] == route_id

    # Parity: an equivalent session/set_config_option on a fresh session
    # produces the identical durable outcome shape (a new 'selected' row).
    agent2, transport2 = _new_agent(environ)
    result2 = agent2._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id2 = result2["sessionId"]
    store2, task_id2 = _bootstrap(agent2, session_id2)
    agent2._session_set_config_option(43, dict(sessionId=session_id2, configId="model", value=entries[0].route.model))
    service2 = ConfigurationService(store2)
    history2 = service2.history(task_id2)
    assert history2[0]["route_id"] == history[0]["route_id"]
    assert history2[0]["state"] == history[0]["state"] == "selected"


def test_cortex_command_never_invokes_runtime(tmp_path, monkeypatch):
    """The _NoopRuntime.start_prompt above raises if ever called; a command
    prompt reaching it would fail this test with that AssertionError."""
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _save_snapshot(record, environ=environ)
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    _bootstrap(agent, session_id)
    session = agent.sessions[session_id]
    cancel_event = threading.Event()
    agent._run_command_body(request_id=1, session_id=session_id, session=session,
                             recognized=("cortex-status", ""), cancel_event=cancel_event)
    final = next(message for message in transport.responses if message.get("id") == 1)
    assert final["result"]["stopReason"] == "end_turn"


def test_cortex_status_never_mutates_configuration(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _save_snapshot(record, environ=environ)
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    store, task_id = _bootstrap(agent, session_id)
    session = agent.sessions[session_id]

    before = ConfigurationService(store).history(task_id)
    cancel_event = threading.Event()
    agent._run_command_body(request_id=2, session_id=session_id, session=session,
                             recognized=("cortex-status", ""), cancel_event=cancel_event)
    after = ConfigurationService(store).history(task_id)
    assert before == after == ()


def test_cortex_setup_guidance_never_contains_a_secret(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    _bootstrap(agent, session_id)
    session = agent.sessions[session_id]
    cancel_event = threading.Event()
    agent._run_command_body(request_id=3, session_id=session_id, session=session,
                             recognized=("cortex-setup", ""), cancel_event=cancel_event)
    chunk = next(n for n in transport.notifications
                 if n["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
    text = chunk["params"]["update"]["content"]["text"]
    assert "fake-test-secret-key" not in text
    assert "connection" in text.lower()


def test_pre_task_commands_never_create_a_task(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _save_snapshot(record, environ=environ)
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    session = agent.sessions[session_id]
    assert session["task_id"] is None

    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    route_id = entries[0].route.id
    cancel_event = threading.Event()
    agent._run_command_body(request_id=4, session_id=session_id, session=session,
                             recognized=("cortex-model", route_id), cancel_event=cancel_event)
    assert session["task_id"] is None  # a pre-task command never bootstraps a task
    assert session["pending_selection"]["route_id"] == route_id
