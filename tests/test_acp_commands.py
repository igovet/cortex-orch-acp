"""``commands.py`` recognition/resolution unit tests and command<->
set_config_option parity tests (T010, UI-11).
"""
from __future__ import annotations

import threading
import uuid
import json
from types import SimpleNamespace

import pytest

from model_policy_fixtures import write_policy
from cortex_runtime.acp import commands as acp_commands
from cortex_runtime.acp import config_options
from cortex_runtime.acp.agent import Agent
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.acp import sessions as acp_sessions
from cortex_runtime.catalog import discovery, store as catalog_store
from cortex_runtime.configuration.service import ConfigurationService
from cortex_runtime.connections import onboarding
from cortex_runtime.connections.model import AuthMode
from cortex_runtime.portable.engine_contract import StopReason


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
    write_policy(environ, {record.engine: {model_id: "standard"}})
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


def test_finish_is_exactly_one_text_block_and_has_no_argument():
    assert acp_commands.recognize([dict(type="text", text="/cortex-finish")]) == (acp_commands.CORTEX_FINISH, "")
    assert acp_commands.recognize([dict(type="text", text="/cortex-finish later")]) == (acp_commands.CORTEX_FINISH, "later")
    assert acp_commands.recognize([dict(type="text", text="/cortex-finished")]) is None
    assert acp_commands.recognize([dict(type="text", text="/cortex-finish"), dict(type="text", text="more")]) is None


def test_resolve_model_argument_by_route_id_or_label(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _save_snapshot(record, environ=environ)
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    entry = entries[0]
    assert acp_commands.resolve_model_argument(entry.route.id, entries) == entry.route.id
    assert acp_commands.resolve_model_argument(entry.disambiguated_label, entries) == entry.route.id
    assert acp_commands.resolve_model_argument(entry.disambiguated_label.upper(), entries) == entry.route.id
    assert acp_commands.resolve_model_argument(entry.route.model, entries) == entry.route.id
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


class _FinishRuntime(_NoopRuntime):
    def __init__(self):
        self.stops = []

    def stop_task(self, task_id, session_id, emit_update, *, project_root,
                  stop_source, source_origin, source_event_id):
        self.stops.append((task_id, session_id, project_root, stop_source,
                           source_origin, source_event_id))
        return True


class _HarvestRuntime:
    """No-paid runtime probe that records the prompt handed to orchestration."""

    def __init__(self):
        self.prompts = []

    def start_prompt(self, task_id, session_id, content, emit_update, cancel_event):
        self.prompts.append((task_id, session_id, content))
        return StopReason.END_TURN

    def cancel(self, session_id):
        return None


def _new_agent(environ):
    agent = Agent(_NoopRuntime(), environ=environ)
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    agent.client_capabilities = {}
    return agent, transport


def test_harvest_with_goal_bootstraps_a_fresh_project_session(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    runtime = _HarvestRuntime()
    agent = Agent(runtime, environ=environ)
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    agent.client_capabilities = {}
    session_id = agent._session_new(dict(cwd=str(project), mcpServers=[]))["sessionId"]
    session = agent.sessions[session_id]

    agent._run_command_body(
        request_id=91, session_id=session_id, session=session,
        recognized=(acp_commands.HARVEST, "compose project documentation"),
        cancel_event=threading.Event(),
    )

    assert session["task_id"] is not None
    assert len(runtime.prompts) == 1
    assert "compose project documentation" in runtime.prompts[0][2]
    assert "knowledge-harvest" in runtime.prompts[0][2]
    assert next(message for message in transport.responses if message.get("id") == 91)["result"]["stopReason"] == "end_turn"


def test_bare_harvest_on_a_fresh_session_starts_full_project_documentation(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    runtime = _HarvestRuntime()
    agent = Agent(runtime, environ=environ)
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    agent.client_capabilities = {}
    session_id = agent._session_new(dict(cwd=str(project), mcpServers=[]))["sessionId"]
    session = agent.sessions[session_id]

    agent._run_command_body(
        request_id=92, session_id=session_id, session=session,
        recognized=(acp_commands.HARVEST, ""), cancel_event=threading.Event(),
    )

    assert session["task_id"] is not None
    assert len(runtime.prompts) == 1
    assert "complete source-backed documentation" in runtime.prompts[0][2]


@pytest.mark.parametrize("command_id", [acp_commands.HARVEST_REFRESH])
def test_harvest_refresh_starts_or_continues_a_project_task(tmp_path, monkeypatch, command_id):
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    runtime = _HarvestRuntime()
    agent = Agent(runtime, environ=environ)
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    agent.client_capabilities = {}
    session_id = agent._session_new(dict(cwd=str(project), mcpServers=[]))["sessionId"]
    session = agent.sessions[session_id]

    agent._run_command_body(
        request_id=94, session_id=session_id, session=session,
        recognized=(command_id, "API docs"), cancel_event=threading.Event(),
    )

    assert session["task_id"] is not None
    assert "API docs" in runtime.prompts[0][2]
    assert "documentation refresh" in runtime.prompts[0][2]


def test_bare_harvest_refresh_uses_update_goal_on_a_fresh_session(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    runtime = _HarvestRuntime()
    agent = Agent(runtime, environ=environ)
    agent.bind_transport(_RecordingTransport())
    agent.initialized = True
    agent.client_capabilities = {}
    session_id = agent._session_new(dict(cwd=str(project), mcpServers=[]))["sessionId"]

    agent._run_command_body(
        request_id=95, session_id=session_id, session=agent.sessions[session_id],
        recognized=(acp_commands.HARVEST_REFRESH, ""), cancel_event=threading.Event(),
    )

    assert "Update the project's existing source-backed documentation" in runtime.prompts[0][2]


def test_harvest_with_goal_continues_the_existing_task(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    runtime = _HarvestRuntime()
    agent = Agent(runtime, environ=environ)
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    agent.client_capabilities = {}
    session_id = agent._session_new(dict(cwd=str(project), mcpServers=[]))["sessionId"]
    _store, task_id = _bootstrap(agent, session_id)
    session = agent.sessions[session_id]

    agent._run_command_body(
        request_id=93, session_id=session_id, session=session,
        recognized=(acp_commands.HARVEST, "refresh API docs"),
        cancel_event=threading.Event(),
    )

    assert session["task_id"] == task_id
    assert runtime.prompts[0][0] == task_id
    assert "refresh API docs" in runtime.prompts[0][2]


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


def test_finish_command_captures_real_source_before_runtime_stop(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    runtime = _FinishRuntime()
    agent = Agent(runtime, environ=environ)
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    session_id = agent._session_new(dict(cwd=str(project), mcpServers=[]))["sessionId"]
    store, task_id = _bootstrap(agent, session_id)

    agent._run_command_body(41, session_id, agent.sessions[session_id],
                            (acp_commands.CORTEX_FINISH, "later"), threading.Event())
    assert not runtime.stops
    agent._run_command_body(42, session_id, agent.sessions[session_id],
                            (acp_commands.CORTEX_FINISH, ""), threading.Event())
    assert len(runtime.stops) == 1
    _, _, _, source, origin, event_id = runtime.stops[0]
    assert source == "cortex_finish_command"
    with store.connection() as db:
        assert db.execute("SELECT 1 FROM source_events WHERE task_id=? AND origin=? AND event_id=?",
                          (task_id, origin, event_id)).fetchone() is not None
    assert any("not certified" in str(n) for n in transport.notifications)


def test_paseo_queued_finish_is_not_delayed_as_steering(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    runtime = _FinishRuntime()
    agent = Agent(runtime, environ=environ)
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    session_id = agent._session_new(dict(cwd=str(project), mcpServers=[]))["sessionId"]
    store, task_id = _bootstrap(agent, session_id)
    agent._queue_paseo_message(43, session_id, agent.sessions[session_id], json.dumps(dict(
        clientMessageId="owner-finish-1", content=[dict(type="text", text="/cortex-finish")],
    )))
    assert len(runtime.stops) == 1
    assert runtime.stops[0][3:] == ("cortex_finish_command", f"paseo-steer:{session_id}", "owner-finish-1")
    with store.connection() as db:
        assert db.execute("SELECT 1 FROM source_events WHERE task_id=? AND origin=? AND event_id=?",
                          (task_id, f"paseo-steer:{session_id}", "owner-finish-1")).fetchone() is not None
        assert db.execute("SELECT COUNT(*) FROM task_events WHERE task_id=? AND event_type='user_message'",
                          (task_id,)).fetchone()[0] == 0
    assert not agent.sessions[session_id].get("queued_messages")


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
    text = transport.notifications[-1]["params"]["update"]["content"]["text"]
    assert "- execution: direct-user (no OS filesystem isolation)" in text


def test_status_execution_mode_and_command_text_replay_after_reconnect(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(environ)
    session_id = agent._session_new(dict(cwd=str(project), mcpServers=[]))["sessionId"]

    agent._session_prompt(9, dict(sessionId=session_id,
                                  prompt=[dict(type="text", text="/cortex-status")]))
    for worker in list(agent._workers):
        worker.join(timeout=10)
    assert next(message for message in transport.responses if message.get("id") == 9)["result"]["stopReason"] == "end_turn"

    loader, replay_transport = _new_agent(environ)
    loader._session_load(10, dict(sessionId=session_id, cwd=str(project), mcpServers=[]))
    updates = [item["params"]["update"] for item in replay_transport.notifications]
    assert [item["sessionUpdate"] for item in updates] == ["user_message_chunk", "agent_message_chunk"]
    assert updates[0]["content"]["text"] == "/cortex-status"
    assert "- execution: direct-user (no OS filesystem isolation)" in updates[1]["content"]["text"]


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


def test_cortex_setup_guidance_explains_pending_authorization_without_re_registration():
    text = acp_commands.build_setup_text(
        connection_states={"configured": 0, "authorization_required": 1}
    )
    assert "already registered" in text
    assert "awaiting authorization" in text
    assert "Do not register another connection" in text
    assert "isolated engine directory" in text
    assert "mark_native_login_pending" not in text
    assert "credential" not in text.lower()
    assert "/home/" not in text


def test_cortex_setup_guidance_keeps_zero_connection_onboarding():
    text = acp_commands.build_setup_text(connection_states={})
    assert "add-connection" in text
    assert "--install-record" in text
    assert "validate-connection" in text
    assert "connections.onboarding" not in text
    assert "mark_native_login_pending" not in text


@pytest.mark.parametrize("command_id,argument,expected", [
    ("cortex-model", "anything", "No model is selectable yet"),
    ("cortex-effort", "high", "No thought level is selectable"),
])
def test_route_commands_explain_missing_options_without_internal_config_id(
    tmp_path, monkeypatch, command_id, argument, expected,
):
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(environ)
    session_id = agent._session_new(dict(cwd=str(project), mcpServers=[]))["sessionId"]
    agent._run_command_body(
        request_id=99, session_id=session_id, session=agent.sessions[session_id],
        recognized=(command_id, argument), cancel_event=threading.Event(),
    )
    chunks = [n for n in transport.notifications
              if n["params"]["update"]["sessionUpdate"] == "agent_message_chunk"]
    assert chunks and expected in chunks[-1]["params"]["update"]["content"]["text"]
    assert "configId" not in chunks[-1]["params"]["update"]["content"]["text"]


@pytest.mark.parametrize(
    ("state", "expected"),
    [("unavailable", "currently unavailable"), ("invalid", "invalid configuration")],
)
def test_cortex_setup_guidance_repairs_registered_connection(state, expected):
    text = acp_commands.build_setup_text(connection_states={"configured": 0, state: 1})
    assert expected in text
    assert "add-connection" not in text
    assert "validate-connection" in text
    assert "/home/" not in text


def test_cortex_setup_guidance_prioritizes_pending_state_in_mixed_registry():
    text = acp_commands.build_setup_text(connection_states={
        "configured": 1, "authorization_required": 1, "unavailable": 1, "invalid": 1,
    })
    assert "awaiting authorization" in text
    assert "currently unavailable" not in text
    assert "invalid configuration" not in text


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


@pytest.mark.parametrize('text,days', [('0', 0), ('36500d', 36500), ('days 7', 7), ('0007', 7)])
@pytest.mark.parametrize('suffix', ['', ' confirm'])
def test_clear_days_grammar(text, days, suffix):
    parsed = acp_commands.parse_clear_argument(text + suffix)
    assert parsed['days'] == days and parsed['cutoff'] is None
    assert parsed['confirm'] == bool(suffix)
    assert parsed['confirm_command'] == f'/cortex-clear days {days} confirm'


@pytest.mark.parametrize('text', ['before 2026-10-01', '2026-10-01', '2026-10-01 confirm'])
def test_clear_absolute_grammar(text):
    from datetime import datetime, timezone
    cutoff = datetime(2026, 10, 1, tzinfo=timezone.utc)
    parsed = acp_commands.parse_clear_argument(text, now=cutoff)
    assert parsed['cutoff'] == cutoff and parsed['days'] is None
    assert parsed['confirm_command'] == '/cortex-clear before 2026-10-01 confirm'


@pytest.mark.parametrize('text', ['', 'confirm', '-1', '+1', '1.5', '36501', '9999999999999999',
                                  'days 7d', 'before 2999-01-01', '2026-02-30', '2026-1-01',
                                  'days', '7 extra', '7 confirm extra', 'confirm 7', '7 confirm confirm'])
def test_clear_invalid_grammar(text):
    assert acp_commands.parse_clear_argument(text) is None


def test_clear_command_recognition_and_advertisement():
    assert acp_commands.recognize([dict(type='text', text='/cortex-clear before 2026-10-01')]) == ('cortex-clear', 'before 2026-10-01')
    for text in ['/clear 7', '/cortex-clearer 7']:
        assert acp_commands.recognize([dict(type='text', text=text)]) is None
    assert acp_commands.recognize([dict(type='text', text='/cortex-clear 7'), dict(type='text', text='confirm')]) is None
    assert 'cortex-clear' in {item['name'] for item in acp_commands.AVAILABLE_COMMANDS}


def test_clear_chat_preview_confirm_current_task_and_no_model(tmp_path, monkeypatch):
    from datetime import datetime, timezone
    from unittest.mock import patch
    from storage_fixture import call_store
    from cortex_runtime.store import Store
    from cortex_runtime.orchestration.state import SCHEMA
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / 'project'; project.mkdir()
    agent, transport = _new_agent(environ)
    sid = agent._session_new(dict(cwd=str(project), mcpServers=[]))['sessionId']
    session = agent.sessions[sid]
    store = agent._store_for(str(project))
    ids = []
    with patch.object(Store, '_now', return_value=datetime(2026, 1, 1, tzinfo=timezone.utc).isoformat(timespec='microseconds')):
        for label in ['old-private-secret', 'current-private-secret', 'active-private-secret']:
            ids.append(call_store(store, 'create_task', dict(project_root=str(project), request=label, request_key=label))['task_id'])
    old, current, active = ids
    session['task_id'] = current
    acp_sessions.bind_task(store, sid, current)
    with store.connection() as db:
        db.executescript(SCHEMA)
        for task, state in [(current, 'COMPLETED'), (old, 'FAILED'), (active, 'EXECUTING')]:
            db.execute('INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES(?,?,?,?,?)',
                       (task, state, 'private-secret', store._now(), store._now()))
    for rid, argument in [(711, 'before 2026-10-01'), (712, 'before 2026-10-01 confirm')]:
        text = '/cortex-clear ' + argument
        # Full ACP prompt path, including recognition, command capture and dispatch.
        agent._session_prompt(rid, dict(sessionId=sid, prompt=[dict(type='text', text=text)]))
        for worker in tuple(agent._workers):
            worker.join(timeout=5)
            assert not worker.is_alive()
        with store.connection() as db:
            remaining = {r[0] for r in db.execute('SELECT id FROM tasks')}
        assert remaining == (set(ids) if rid == 711 else {current, active})
    output = json.dumps(transport.notifications)
    assert 'would_delete_tasks=1' in output and 'deleted_tasks=1' in output
    assert 'skipped_protected=2' in output
    assert '/cortex-clear before 2026-10-01 confirm' in output
    assert 'private-secret' not in output
    assert len(transport.responses) == 2


def test_clear_invalid_prompt_is_usage_only_and_taskless(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / 'project'; project.mkdir()
    agent, transport = _new_agent(environ)
    sid = agent._session_new(dict(cwd=str(project), mcpServers=[]))['sessionId']
    agent._run_prompt(713, sid, agent.sessions[sid], '/cortex-clear before 2999-01-01 confirm',
                      threading.Event(), recognized=('cortex-clear', 'before 2999-01-01 confirm'))
    assert agent.sessions[sid]['task_id'] is None
    assert 'Usage: /cortex-clear' in json.dumps(transport.notifications)
    with agent._store_for(str(project)).connection() as db:
        assert db.execute('SELECT COUNT(*) FROM tasks').fetchone()[0] == 0


def test_clear_never_becomes_an_answer_or_queued_model_prompt(tmp_path, monkeypatch):
    from cortex_runtime.acp.protocol import RpcError
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / 'project'; project.mkdir()
    agent, _ = _new_agent(environ)
    sid = agent._session_new(dict(cwd=str(project), mcpServers=[]))['sessionId']
    session = agent.sessions[sid]
    session['active'] = threading.Event()
    session['background'] = True
    session['question_waiter'] = ('q', lambda _: pytest.fail('clear must not answer a question'))
    with pytest.raises(RpcError, match='idle session'):
        agent._session_prompt(714, dict(sessionId=sid, prompt=[dict(type='text', text='/cortex-clear 0 confirm')]))
    assert not session.get('pending_prompts')


@pytest.mark.parametrize('argument', ['before 2999-01-01 confirm', '36501 confirm', '2026-02-30 confirm'])
def test_invalid_clear_preserves_existing_tasks(tmp_path, monkeypatch, argument):
    from storage_fixture import call_store
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / 'project'; project.mkdir()
    agent, transport = _new_agent(environ)
    sid = agent._session_new(dict(cwd=str(project), mcpServers=[]))['sessionId']
    store = agent._store_for(str(project))
    task = call_store(store, 'create_task', dict(project_root=str(project), request='private', request_key='k'))['task_id']
    agent._run_command_body(715, sid, agent.sessions[sid], ('cortex-clear', argument), threading.Event())
    assert 'Usage: /cortex-clear' in json.dumps(transport.notifications)
    assert (project / '.cortex' / task).exists()


def test_clear_reports_failure_without_success_or_private_content(tmp_path, monkeypatch):
    from cortex_runtime import cleanup
    from cortex_runtime.contracts import StoreError
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / 'project'; project.mkdir()
    agent, transport = _new_agent(environ)
    sid = agent._session_new(dict(cwd=str(project), mcpServers=[]))['sessionId']
    def fail(*args, **kwargs):
        raise StoreError('retention_active_conflict', 'private-secret')
    monkeypatch.setattr(cleanup, 'clear_tasks', fail)
    agent._run_command_body(716, sid, agent.sessions[sid], ('cortex-clear', '0 confirm'), threading.Event())
    output = json.dumps(transport.notifications)
    assert 'refused or incomplete' in output
    assert 'deleted_tasks=' not in output and 'private-secret' not in output
