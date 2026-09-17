"""``config_options.py`` unit tests and ``session/new``/``session/set_config_option``
integration tests (T010, UI-02/UI-05/UI-06).

Fixture connections/catalog snapshots use fake secrets and placeholder
model ids only -- never a real credential, never a live engine probe.
"""
from __future__ import annotations

import json
import threading

import jsonschema
import pytest

from cortex_runtime.acp import commands as acp_commands
from cortex_runtime.acp import config_options
from cortex_runtime.acp.agent import Agent
from cortex_runtime.catalog import discovery, store as catalog_store
from cortex_runtime.catalog.evidence import EvidenceState
from cortex_runtime.catalog.model import CatalogEntry
from cortex_runtime.configuration import store as configuration_store
from cortex_runtime.configuration.service import ConfigurationService
from cortex_runtime.connections import onboarding, registry
from cortex_runtime.connections.model import AuthMode
from cortex_runtime.portable.identity import ExecutionRoute, OwnershipEpoch
from cortex_runtime.store import Store

from pathlib import Path

SCHEMA_PATH = Path(__file__).resolve().parent / "fixtures" / "acp" / "schema-1.4.0.json"


def _schema_validator(def_name):
    document = json.loads(SCHEMA_PATH.read_text())
    resolver = jsonschema.RefResolver.from_schema(document)
    return jsonschema.Draft202012Validator({"$ref": f"#/$defs/{def_name}"}, resolver=resolver)


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


def _configured_connection(name="work", engine="codex-acp", provider="openai", environ=None):
    record = onboarding.create_connection(
        name=name, engine=engine, provider=provider, auth_mode=AuthMode.API_KEY,
        key_kind="openai_api_key", endpoint=f"cortex-test:{name}", environ=environ)
    return onboarding.set_api_key(record.id, reader=lambda: b"fake-test-secret-key", environ=environ)


def _observation_raw(model_id="model-placeholder-1", model_name="Placeholder Model 1",
                      thought_level="high", thought_levels=("high", "low")):
    options = [
        dict(id="model", name="Model", type="select", category="model", currentValue=model_id,
             options=[dict(value=model_id, name=model_name)]),
    ]
    if thought_level is not None:
        options.append(dict(id="thought_level", name="Thought level", type="select",
                             category="thought_level", currentValue=thought_level,
                             options=[dict(value=value, name=value) for value in thought_levels]))
    return options


def _build_and_save_snapshot(record, *, environ, model_id="model-placeholder-1",
                              model_name="Placeholder Model 1", thought_level="high"):
    observation = discovery.parse_observation(_observation_raw(model_id, model_name, thought_level))
    snapshot = discovery.build_catalog({record.id: record}, {record.id: observation})
    catalog_store.save_snapshot(snapshot, environ)
    return snapshot


# --- config_options.py unit tests -------------------------------------------

def test_no_connections_means_no_model_option():
    assert config_options.build_config_options((), None) == []


def test_build_model_option_omitted_when_no_selectable_entries():
    assert config_options.build_model_option((), None, None, None) is None


def test_model_option_two_connections_same_model_distinct_values_and_labels(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record_a = _configured_connection(name="Team A", environ=environ)
    record_b = _configured_connection(name="Team B", environ=environ)
    _build_and_save_snapshot(record_a, environ=environ, model_id="shared-model", model_name="Shared Model")
    _build_and_save_snapshot(record_b, environ=environ, model_id="shared-model", model_name="Shared Model")

    entries = config_options.selectable_entries_from_snapshots(
        {record_a.id: record_a, record_b.id: record_b}, environ)
    assert len(entries) == 2
    route_ids = {entry.route.id for entry in entries}
    assert len(route_ids) == 2  # distinct connection_id -> distinct route fingerprint

    option = config_options.build_model_option(entries, None, None, None)
    values = {item["value"] for item in option["options"]}
    names = {item["name"] for item in option["options"]}
    assert values == route_ids
    assert len(names) == 2  # disambiguated -- never silently merged


def test_thought_level_option_only_from_currently_selected_route(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, thought_level="high")
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    entry = entries[0]
    option = config_options.build_thought_level_option(entry, {}, None, None)
    assert option["id"] == "thought_level"
    assert option["currentValue"] == "high"
    assert {item["value"] for item in option["options"]} == {"high"}


def test_thought_level_option_omitted_when_route_reports_none(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, thought_level=None)
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    assert config_options.build_thought_level_option(entries[0], {}, None, None) is None


def test_state_suffix_and_meta_reflect_pending_vs_active():
    entries = ()
    for state, expected_suffix in (("selected", "pending"), ("pending_next_use", "pending"), ("active", "active")):
        option = config_options.build_model_option(
            [CatalogEntry(
                route=__import__("cortex_runtime.portable.identity", fromlist=["ExecutionRoute"]).ExecutionRoute(
                    engine="codex-acp", connection_id="cn_" + "a" * 12, model="m", supported_parameters=()),
                connection_id="cn_" + "a" * 12, label="Engine · Conn · m", disambiguated_label="Engine · Conn · m",
                evidence=__import__("cortex_runtime.catalog.evidence", fromlist=["EvidenceState"]).EvidenceState.empty(),
                catalog_snapshot_id="cs_" + "b" * 12,
            )],
            None, state, 1 if state != "selected" else 1)
        assert expected_suffix in option["description"]
        assert option["_meta"]["_cortex"]["state"] == state


def test_evidence_note_marks_unverified_discovery_entries(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ)
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    option = config_options.build_model_option(entries, None, None, None)
    assert "unverified" in option["options"][0]["description"]


def test_render_boolean_as_select_without_client_capability():
    option = config_options.render_boolean_as_select("team", "Team", True, category="_extension")
    assert option["type"] == "select"
    assert option["currentValue"] == "true"
    assert {item["value"] for item in option["options"]} == {"true", "false"}


def test_render_boolean_emits_real_boolean_when_client_supports_it():
    caps = {"session": {"configOptions": {"boolean": {}}}}
    option = config_options.render_boolean_as_select("team", "Team", False, client_capabilities=caps)
    assert option["type"] == "boolean"
    assert option["currentValue"] is False


def test_most_recent_live_selection_skips_rejected_and_failed():
    history = (
        dict(revision=1, state="failed", route_id="a"),
        dict(revision=2, state="active", route_id="b"),
        dict(revision=3, state="rejected", route_id="c"),
    )
    best = config_options.most_recent_live_selection(history)
    assert best["revision"] == 2


# --- session/new integration (in-process Agent) -----------------------------

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
        from cortex_runtime.portable.engine_contract import StopReason
        return StopReason.END_TURN

    def cancel(self, session_id):
        return None


def _new_agent(tmp_path, environ):
    agent = Agent(_NoopRuntime(), environ=environ)
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    agent.client_capabilities = {}
    return agent, transport


def test_session_new_omits_model_options_with_no_connections_but_offers_language(tmp_path, monkeypatch):
    """T020: model/thought_level stay gated on selectability, but
    ``language`` is never gated on a connection/model -- it is always
    offered, even with zero connections."""
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    agent, _transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    ids = {option["id"] for option in result["configOptions"]}
    assert ids == {"language"}
    assert result["configOptions"][0]["currentValue"] == "auto"


def test_session_new_includes_model_and_thought_level_options(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ)
    project = tmp_path / "project"
    project.mkdir()
    agent, _transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    ids = {option["id"] for option in result["configOptions"]}
    assert ids == {"model", "thought_level", "language"}
    validator = _schema_validator("SessionConfigOption")
    for option in result["configOptions"]:
        validator.validate(option)


def test_available_commands_update_sent_after_session_new(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    agent._post_session_new_notifications(result["sessionId"])
    assert len(transport.notifications) == 1
    update = transport.notifications[0]["params"]["update"]
    assert update["sessionUpdate"] == "available_commands_update"
    names = {item["name"] for item in update["availableCommands"]}
    assert names == {acp_commands.CORTEX_STATUS, acp_commands.CORTEX_MODEL,
                      acp_commands.CORTEX_EFFORT, acp_commands.CORTEX_SETUP,
                      acp_commands.CORTEX_NEW_TASK, acp_commands.CORTEX_STOP,
                      acp_commands.CORTEX_ANSWER, acp_commands.CORTEX_LANGUAGE}
    validator = _schema_validator("AvailableCommandsUpdate")
    validator.validate(update)


# --- session/set_config_option (mid-task): UI-05/UI-06 -----------------------

def _bootstrap_task_for_session(agent, session_id, tmp_path):
    """Drive the same first-prompt path production code uses (bootstrap +
    ownership epoch capture), without spawning the runtime asynchronously."""
    from types import SimpleNamespace
    import uuid
    from cortex_runtime.bootstrap import bootstrap_task

    session = agent.sessions[session_id]
    store = agent._store_for(session["project"])
    origin = f"acp:{session_id}"
    event = SimpleNamespace(origin=origin, event_id=str(uuid.uuid4()))
    result = bootstrap_task(store, event, "hello", owner_ref=origin)
    session["task_id"] = result.task_id
    session["owner_epoch"] = result.owner_epoch
    from cortex_runtime.acp import sessions as acp_sessions
    acp_sessions.bind_task(store, session_id, result.task_id)
    return store, result.task_id


def test_set_config_option_model_reaches_configuration_service(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a", thought_level="high")
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    store, task_id = _bootstrap_task_for_session(agent, session_id, tmp_path)

    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    route_id = entries[0].route.id

    agent._session_set_config_option(99, dict(sessionId=session_id, configId="model", value=route_id))
    response = next(message for message in transport.responses if message.get("id") == 99)
    assert "result" in response, response
    assert response["result"]["configOptions"][0]["currentValue"] == route_id

    service = ConfigurationService(store)
    history = service.history(task_id)
    assert len(history) == 1
    assert history[0]["route_id"] == route_id
    assert history[0]["state"] == "selected"

    validator = _schema_validator("SetSessionConfigOptionResponse")
    validator.validate(response["result"])
    update_validator = _schema_validator("ConfigOptionUpdate")
    update_notification = next(
        message for message in transport.notifications
        if message["params"]["update"]["sessionUpdate"] == "config_option_update")
    update_validator.validate(update_notification["params"]["update"])


def test_set_config_option_route_change_drops_incompatible_thought_level(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record_a = _configured_connection(name="A", environ=environ)
    record_b = _configured_connection(name="B", environ=environ)
    _build_and_save_snapshot(record_a, environ=environ, model_id="model-a", thought_level="high")
    _build_and_save_snapshot(record_b, environ=environ, model_id="model-b", thought_level=None)
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    store, task_id = _bootstrap_task_for_session(agent, session_id, tmp_path)

    entries = config_options.selectable_entries_from_snapshots(
        {record_a.id: record_a, record_b.id: record_b}, environ)
    route_a = next(e.route.id for e in entries if e.route.model == "model-a")
    route_b = next(e.route.id for e in entries if e.route.model == "model-b")

    agent._session_set_config_option(1, dict(sessionId=session_id, configId="model", value=route_a))
    agent._session_set_config_option(2, dict(sessionId=session_id, configId="thought_level", value="high"))
    agent._session_set_config_option(3, dict(sessionId=session_id, configId="model", value=route_b))
    response = next(message for message in transport.responses if message.get("id") == 3)
    ids = {opt["id"] for opt in response["result"]["configOptions"]}
    assert "thought_level" not in ids  # model-b reports no thought_level parameter

    service = ConfigurationService(store)
    history = service.history(task_id)
    assert "thought_level" not in history[-1]["parameters"]


def test_set_config_option_unknown_config_id_is_invalid_params(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ)
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    _bootstrap_task_for_session(agent, session_id, tmp_path)

    agent._session_set_config_option(7, dict(sessionId=session_id, configId="summarizer", value="x"))
    response = next(message for message in transport.responses if message.get("id") == 7)
    assert response["error"]["code"] == -32602
    restore = next(
        message for message in transport.notifications
        if message["params"]["update"]["sessionUpdate"] == "config_option_update")
    assert restore["params"]["update"]["configOptions"]  # unchanged options restored


def test_set_config_option_invalid_value_is_invalid_params(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ)
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    _bootstrap_task_for_session(agent, session_id, tmp_path)

    agent._session_set_config_option(8, dict(sessionId=session_id, configId="model", value="xr_doesnotexist000"))
    response = next(message for message in transport.responses if message.get("id") == 8)
    assert response["error"]["code"] == -32602


def test_set_config_option_boolean_rejected_without_capability(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ)
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    _bootstrap_task_for_session(agent, session_id, tmp_path)

    agent._session_set_config_option(9, dict(sessionId=session_id, configId="model", type="boolean", value=True))
    response = next(message for message in transport.responses if message.get("id") == 9)
    assert response["error"]["code"] == -32602


def test_set_config_option_pre_task_writes_user_default_and_is_pending(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a")
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]

    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    route_id = entries[0].route.id
    agent._session_set_config_option(5, dict(sessionId=session_id, configId="model", value=route_id))
    response = next(message for message in transport.responses if message.get("id") == 5)
    assert "result" in response
    option = response["result"]["configOptions"][0]
    assert option["currentValue"] == route_id
    assert "pending" in option["description"]

    defaults = configuration_store.read_user_defaults(environ)["defaults"]
    assert defaults["route_id"] == route_id
    # F2: the persisted user default is a normal T008 user-layer field, never
    # a Cortex-specific "pending" marker.
    assert "_cortex_pending_route_id" not in defaults
    assert "_cortex_pending_parameters" not in defaults
    defaults_path = configuration_store._user_defaults_path(environ)
    assert "_cortex_pending" not in defaults_path.read_text()


def test_new_session_shows_persisted_default_as_default_not_pending(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a")
    project = tmp_path / "project"
    project.mkdir()

    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    route_id = entries[0].route.id
    agent._session_set_config_option(1, dict(sessionId=session_id, configId="model", value=route_id))

    # A brand-new session (same user) sees the persisted default -- with
    # state "default", never "pending" (F2 review finding): only the
    # session that actually made the choice shows it as pending.
    agent2, transport2 = _new_agent(tmp_path, environ)
    result2 = agent2._session_new(dict(cwd=str(project), mcpServers=[]))
    option = result2["configOptions"][0]
    assert option["currentValue"] == route_id
    assert option.get("description") is None or "pending" not in option["description"]
    assert option["_meta"]["_cortex"]["state"] == "default"
    session2 = agent2.sessions[result2["sessionId"]]
    assert session2["pending_selection"] is None
    assert session2["default_selection"]["route_id"] == route_id


def test_pending_pre_task_selection_is_proposed_on_first_prompt(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a")
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    route_id = entries[0].route.id
    agent._session_set_config_option(1, dict(sessionId=session_id, configId="model", value=route_id))

    store, task_id = _bootstrap_task_for_session(agent, session_id, tmp_path)
    agent._propose_pending_selection_for_new_task(store, agent.sessions[session_id], task_id)

    service = ConfigurationService(store)
    history = service.history(task_id)
    assert len(history) == 1
    assert history[0]["route_id"] == route_id
    # F2: consumed -- no longer pending in the session afterward.
    session = agent.sessions[session_id]
    assert session["pending_selection"] is None
    assert session["default_selection"] is None


def test_stale_configuration_revision_is_never_silently_retried(tmp_path, monkeypatch):
    """F1 review finding: a concurrent session's change must never be
    silently clobbered by an automatic retry that re-reads/re-proposes from
    stale state. Session 1 changes model A->B (succeeds); session 2, holding
    a stale pre-change view of the options, tries to set effort for A and
    must get an explicit error with current options rebuilt fresh (still B),
    never a silent, incorrect apply."""
    environ = _isolate(tmp_path, monkeypatch)
    record_a = _configured_connection(name="A", environ=environ)
    record_b = _configured_connection(name="B", environ=environ)
    _build_and_save_snapshot(record_a, environ=environ, model_id="model-a", thought_level="high")
    _build_and_save_snapshot(record_b, environ=environ, model_id="model-b", thought_level=None)
    project = tmp_path / "project"
    project.mkdir()

    # Two sessions sharing one task/owner-epoch, to exercise a genuine
    # same-task race (see the review's "same owner epoch source").
    agent1, transport1 = _new_agent(tmp_path, environ)
    result1 = agent1._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id_1 = result1["sessionId"]
    store, task_id = _bootstrap_task_for_session(agent1, session_id_1, tmp_path)

    agent2, transport2 = _new_agent(tmp_path, environ)
    session_2 = dict(agent1.sessions[session_id_1])
    session_2["lock"] = threading.Lock()
    session_id_2 = "acps_" + "b" * 12
    agent2.sessions[session_id_2] = session_2
    from cortex_runtime.acp import sessions as acp_sessions
    acp_sessions.create_session(store, session_id_2, session_2["project"], {}, {}, [])
    acp_sessions.bind_task(store, session_id_2, task_id)

    entries = config_options.selectable_entries_from_snapshots(
        {record_a.id: record_a, record_b.id: record_b}, environ)
    route_a = next(e.route.id for e in entries if e.route.model == "model-a")
    route_b = next(e.route.id for e in entries if e.route.model == "model-b")

    # Session 1: select model A, then model B (two live revisions) --
    # this is the concurrent change session 2 will race against.
    agent1._session_set_config_option(1, dict(sessionId=session_id_1, configId="model", value=route_a))
    agent1._session_set_config_option(2, dict(sessionId=session_id_1, configId="model", value=route_b))
    response2 = next(message for message in transport1.responses if message.get("id") == 2)
    assert response2["result"]["configOptions"][0]["currentValue"] == route_b

    # Session 2 now acts on a *stale* view: its own read of "the latest
    # revision" only reflects session 1's first write (route A, revision 1),
    # not the second (route B, revision 2) that has already, genuinely,
    # happened. This deterministically reproduces the same-task race the
    # synchronous single-process reproduction above cannot otherwise create
    # (ConfigurationService.history is normally read fresh, in the same
    # call, immediately before the write -- see _apply_mid_task_selection).
    real_history = ConfigurationService.history
    call_count = {"n": 0}

    def _stale_then_real_history(self, task_id):
        call_count["n"] += 1
        if call_count["n"] <= 2:
            return real_history(self, task_id)[:1]  # only session 1's first write is "known"
        return real_history(self, task_id)  # the error-path rebuild always sees the truth

    monkeypatch.setattr(ConfigurationService, "history", _stale_then_real_history)

    # Session 2, still believing route A is current (per its stale read),
    # tries to change the model to A again -- a config_id whose validity
    # never depends on current state, so it clears the "unknown configId"
    # check even from a stale read, and reaches the actual revision check.
    agent2._session_set_config_option(3, dict(sessionId=session_id_2, configId="model", value=route_a))
    error_response = next(message for message in transport2.responses if message.get("id") == 3)
    assert "error" in error_response, error_response
    assert error_response["error"]["code"] == -32001
    assert error_response["error"]["data"]["code"] == "stale_configuration_revision"

    # The task's configuration was never clobbered: it is still B, and a
    # config_option_update restoring the *current* (post-session-1) options
    # was emitted to session 2.
    service = ConfigurationService(store)
    history = service.history(task_id)
    assert history[-1]["route_id"] == route_b
    restore = next(
        message for message in transport2.notifications
        if message["params"]["update"]["sessionUpdate"] == "config_option_update")
    assert restore["params"]["update"]["configOptions"][0]["currentValue"] == route_b


# --- unavailable saved/selected route marker (review MINOR fix) -------------

def test_build_model_option_marks_unavailable_default_route(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a")
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    option = config_options.build_model_option(entries, "xr_" + "0" * 12, "default", None)
    assert option["currentValue"] == entries[0].route.id  # substitute, schema requires a value
    assert option["_meta"]["_cortex"]["state"] == "default_unavailable"
    assert option["description"] == "Saved route is unavailable — choose a route"


def test_build_model_option_marks_unavailable_task_selection(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a")
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    for state in ("selected", "pending_next_use", "active"):
        option = config_options.build_model_option(entries, "xr_" + "1" * 12, state, 3)
        assert option["_meta"]["_cortex"]["state"] == "selection_unavailable"
        assert option["description"] == "Saved route is unavailable — choose a route"


def test_available_route_never_marked_unavailable(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a")
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    option = config_options.build_model_option(entries, entries[0].route.id, "active", 1)
    assert option["_meta"]["_cortex"]["state"] == "active"
    assert option.get("description") == "(active)"


def test_saved_default_route_connection_removed_marker_no_proposal_status_explains(tmp_path, monkeypatch):
    """Review MINOR: connection removed/deauthorized after the default was
    persisted -> marker present in configOptions, /cortex-status explains,
    and first-prompt consumption never proposes the substituted route."""
    environ = _isolate(tmp_path, monkeypatch)
    record_a = _configured_connection(name="A", environ=environ)
    record_b = _configured_connection(name="B", environ=environ)
    _build_and_save_snapshot(record_a, environ=environ, model_id="model-a")
    _build_and_save_snapshot(record_b, environ=environ, model_id="model-b")
    project = tmp_path / "project"
    project.mkdir()

    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    entries = config_options.selectable_entries_from_snapshots(
        {record_a.id: record_a, record_b.id: record_b}, environ)
    route_a = next(e.route.id for e in entries if e.route.model == "model-a")
    agent._session_set_config_option(1, dict(sessionId=session_id, configId="model", value=route_a))

    # Connection A is removed entirely after the default was persisted.
    onboarding.remove(record_a.id, environ=environ)

    # A brand-new session (same user) now sees the marker, not a silent
    # substitution.
    agent2, transport2 = _new_agent(tmp_path, environ)
    result2 = agent2._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id2 = result2["sessionId"]
    option = result2["configOptions"][0]
    assert option["_meta"]["_cortex"]["state"] == "default_unavailable"
    assert "unavailable" in option["description"].lower()
    # The substitute is still a real, currently-selectable route (B).
    remaining = config_options.selectable_entries_from_snapshots({record_b.id: record_b}, environ)
    assert option["currentValue"] == remaining[0].route.id

    # /cortex-status explains the same thing.
    session2 = agent2.sessions[session_id2]
    cancel_event = threading.Event()
    agent2._run_command_body(request_id=2, session_id=session_id2, session=session2,
                              recognized=("cortex-status", ""), cancel_event=cancel_event)
    chunk = next(n for n in transport2.notifications
                 if n["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
    assert "unavailable" in chunk["params"]["update"]["content"]["text"].lower()

    # First prompt: never auto-proposes the substituted route; the task is
    # left with no Cortex-proposed revision at all.
    store, task_id = _bootstrap_task_for_session(agent2, session_id2, tmp_path)
    agent2._propose_pending_selection_for_new_task(store, agent2.sessions[session_id2], task_id)
    service = ConfigurationService(store)
    assert service.history(task_id) == ()


# --- real concurrent set_config_option race (threading.Barrier) -------------

def test_concurrent_set_config_option_race_exactly_one_wins(tmp_path, monkeypatch):
    """A real two-thread race on the same task: exactly one session's
    set_config_option succeeds, the other gets a stale-revision error, and
    the task's configuration is never clobbered/left inconsistent."""
    environ = _isolate(tmp_path, monkeypatch)
    record_a = _configured_connection(name="A", environ=environ)
    record_b = _configured_connection(name="B", environ=environ)
    _build_and_save_snapshot(record_a, environ=environ, model_id="model-a", thought_level="high")
    _build_and_save_snapshot(record_b, environ=environ, model_id="model-b", thought_level=None)
    project = tmp_path / "project"
    project.mkdir()

    agent1, transport1 = _new_agent(tmp_path, environ)
    result1 = agent1._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id_1 = result1["sessionId"]
    store, task_id = _bootstrap_task_for_session(agent1, session_id_1, tmp_path)

    agent2, transport2 = _new_agent(tmp_path, environ)
    session_2 = dict(agent1.sessions[session_id_1])
    session_2["lock"] = threading.Lock()
    session_id_2 = "acps_" + "c" * 12
    agent2.sessions[session_id_2] = session_2
    from cortex_runtime.acp import sessions as acp_sessions
    acp_sessions.create_session(store, session_id_2, session_2["project"], {}, {}, [])
    acp_sessions.bind_task(store, session_id_2, task_id)

    entries = config_options.selectable_entries_from_snapshots(
        {record_a.id: record_a, record_b.id: record_b}, environ)
    route_a = next(e.route.id for e in entries if e.route.model == "model-a")
    route_b = next(e.route.id for e in entries if e.route.model == "model-b")

    # Force the genuine race window: each side's *own* internal read of
    # "the latest revision" (used to compute expected_revision inside
    # _apply_mid_task_selection) is a separate transaction from its write
    # (see ConfigurationService.propose); without synchronizing that read,
    # Python's GIL/scheduling makes one thread's whole read+write finish
    # before the other's read even starts, so no actual collision occurs.
    # A threading.Barrier on each thread's *second* ConfigurationService
    # .history() call (the expected_revision read; the first call, for
    # previous_options validation, never depends on task state for a
    # config_id="model" request) makes both threads compute
    # expected_revision from the identical pre-race state, so real SQLite
    # write-lock serialization then decides exactly one winner.
    real_history = ConfigurationService.history
    barrier = threading.Barrier(2)
    call_counts = threading.local()

    def _synchronized_history(self, task_id):
        call_counts.n = getattr(call_counts, "n", 0) + 1
        if call_counts.n == 2:
            barrier.wait(timeout=5)
        return real_history(self, task_id)

    monkeypatch.setattr(ConfigurationService, "history", _synchronized_history)

    results = {}

    def _race(agent, session_id, request_id, route_id, key):
        agent._session_set_config_option(request_id, dict(sessionId=session_id, configId="model", value=route_id))
        results[key] = True

    t1 = threading.Thread(target=_race, args=(agent1, session_id_1, 10, route_a, "one"))
    t2 = threading.Thread(target=_race, args=(agent2, session_id_2, 20, route_b, "two"))
    t1.start()
    t2.start()
    t1.join(timeout=10)
    t2.join(timeout=10)
    assert results == {"one": True, "two": True}

    response_1 = next(m for m in transport1.responses if m.get("id") == 10)
    response_2 = next(m for m in transport2.responses if m.get("id") == 20)
    outcomes = [response_1, response_2]
    successes = [r for r in outcomes if "result" in r]
    errors = [r for r in outcomes if "error" in r]
    assert len(successes) == 1, outcomes
    assert len(errors) == 1, outcomes
    assert errors[0]["error"]["code"] == -32001
    assert errors[0]["error"]["data"]["code"] == "stale_configuration_revision"

    # The task's configuration was never clobbered: history has exactly one
    # live revision, matching whichever side actually won.
    service = ConfigurationService(store)
    history = service.history(task_id)
    live = [row for row in history if row["state"] in ("selected", "pending_next_use", "active")]
    assert len(live) == 1
    winning_route = route_a if successes[0] is response_1 else route_b
    assert live[0]["route_id"] == winning_route


# --- thought_level option must carry the model option's effective state
#     (review MINOR: config_options.py ~210-211) -----------------------------

def _entry_with_thought_levels(route_id_seed, values):
    route = ExecutionRoute(
        engine="codex-acp", connection_id="cn_" + route_id_seed * 12, model="m",
        supported_parameters=tuple((config_options.THOUGHT_LEVEL_OPTION_ID, value) for value in values))
    return CatalogEntry(
        route=route, connection_id="cn_" + route_id_seed * 12,
        label=f"Engine · Conn {route_id_seed} · m", disambiguated_label=f"Engine · Conn {route_id_seed} · m",
        evidence=EvidenceState.empty(), catalog_snapshot_id="cs_" + "b" * 12)


def test_build_config_options_unavailable_default_marks_thought_level_too():
    """A saved default whose route is unavailable, with a saved (now stale)
    thought_level value, must never present that stale value as selected:
    the thought_level option gets the substitute route's first value and
    the identical unavailable marker as the model option."""
    entry = _entry_with_thought_levels("a", ["low", "high"])  # sorted -> ["high", "low"]
    current = dict(route_id="xr_" + "0" * 12, state="default", revision=None,
                    parameters={"thought_level": "low"})
    options = config_options.build_config_options([entry], current)
    model_option, thought_option = options
    assert model_option["_meta"]["_cortex"]["state"] == "default_unavailable"
    assert thought_option["_meta"]["_cortex"]["state"] == "default_unavailable"
    assert thought_option["description"] == "Depends on an unavailable route — choose a route"
    # Never the stale saved "low" -- the substituted route's first value.
    assert thought_option["currentValue"] == "high"


def test_build_config_options_unavailable_task_selection_marks_thought_level_too():
    """Same, but the saved value came from an actual task/session selection
    (state e.g. 'active'), not a pre-task default."""
    entry = _entry_with_thought_levels("a", ["low", "high"])
    current = dict(route_id="xr_" + "1" * 12, state="active", revision=7,
                    parameters={"thought_level": "low"})
    options = config_options.build_config_options([entry], current)
    model_option, thought_option = options
    assert model_option["_meta"]["_cortex"]["state"] == "selection_unavailable"
    assert thought_option["_meta"]["_cortex"]["state"] == "selection_unavailable"
    assert thought_option["description"] == "Depends on an unavailable route — choose a route"
    assert thought_option["currentValue"] == "high"
    assert thought_option["_meta"]["_cortex"]["revision"] == 7


def test_build_config_options_available_route_thought_level_unaffected():
    """Sanity: when the route *is* available, thought_level behaves exactly
    as before (no marker, saved value honored)."""
    entry = _entry_with_thought_levels("a", ["low", "high"])
    current = dict(route_id=entry.route.id, state="active", revision=2, parameters={"thought_level": "low"})
    options = config_options.build_config_options([entry], current)
    model_option, thought_option = options
    assert model_option["_meta"]["_cortex"]["state"] == "active"
    assert thought_option["_meta"]["_cortex"]["state"] == "active"
    assert thought_option["currentValue"] == "low"
    assert thought_option["description"] == "(active)"


def test_saved_default_route_removed_with_saved_thought_level_status_reflects_it(tmp_path, monkeypatch):
    """Integration regression: a removed connection's persisted default had
    a saved thought_level value; the new session's configOptions and
    /cortex-status must never present that stale value as selected, and
    both options carry the unavailable marker."""
    environ = _isolate(tmp_path, monkeypatch)
    record_a = _configured_connection(name="A", environ=environ)
    record_b = _configured_connection(name="B", environ=environ)
    _build_and_save_snapshot(record_a, environ=environ, model_id="model-a", thought_level="high")
    _build_and_save_snapshot(record_b, environ=environ, model_id="model-b", thought_level="high")
    project = tmp_path / "project"
    project.mkdir()

    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    entries = config_options.selectable_entries_from_snapshots(
        {record_a.id: record_a, record_b.id: record_b}, environ)
    route_a = next(e.route.id for e in entries if e.route.model == "model-a")
    agent._session_set_config_option(1, dict(sessionId=session_id, configId="model", value=route_a))
    # Save a thought_level choice too, so the persisted default carries a
    # (soon to be stale) parameters value alongside the route.
    defaults = configuration_store.read_user_defaults(environ)
    defaults_dict = dict(defaults["defaults"])
    defaults_dict["parameters"] = {"thought_level": "high"}
    configuration_store.write_user_defaults(defaults_dict, defaults["ceiling"], environ)

    onboarding.remove(record_a.id, environ=environ)

    agent2, transport2 = _new_agent(tmp_path, environ)
    result2 = agent2._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id2 = result2["sessionId"]
    ids = {opt["id"]: opt for opt in result2["configOptions"]}
    assert ids["model"]["_meta"]["_cortex"]["state"] == "default_unavailable"
    if "thought_level" in ids:
        assert ids["thought_level"]["_meta"]["_cortex"]["state"] == "default_unavailable"
        assert "unavailable" in ids["thought_level"]["description"].lower()

    session2 = agent2.sessions[session_id2]
    cancel_event = threading.Event()
    agent2._run_command_body(request_id=2, session_id=session_id2, session=session2,
                              recognized=("cortex-status", ""), cancel_event=cancel_event)
    chunk = next(n for n in transport2.notifications
                 if n["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
    text = chunk["params"]["update"]["content"]["text"]
    assert text.lower().count("unavailable") >= 1
