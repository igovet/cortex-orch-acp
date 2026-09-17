"""T031 acceptance bullet #3: after one-time setup, an ordinary launch
needs no per-task manual configuration/worker/MCP/skill bootstrap.

Two things are proven, both driven purely by state the setup script wrote
(no ``session/set_config_option``/``/cortex-model`` call is ever made in
either test):

1. Through the real production ``acp.agent.Agent`` (``session/new`` then a
   first ``session/prompt``), the very first prompt of a brand-new task
   automatically gets a configuration revision proposed from the stored
   setup default -- exactly the mechanism
   ``acp.agent.Agent._propose_pending_selection_for_new_task`` already
   provides for a normal in-session choice.

2. Through the real production ``acp.orchestrating_runtime.OrchestratingRuntime``
   + ``delegation.live_adapters.EngineSessionManagerAdapter`` (the same
   pair ``ProductionOrchestratingRuntime`` itself builds) with a real
   fake-engine subprocess: a coordinator session opens, its configuration
   is confirmed by the (scripted) engine and the revision becomes
   ``'active'`` with a real confirmation evidence ref, and one delegated
   assignment provisions one worker -- all resolved purely from the setup
   script's connection/route, using the real
   ``delegation.live_adapters.ConfigurationServiceSnapshotSource`` (no
   test-local snapshot-source workaround).

This closes the gap disclosed in T013's fresh-review report ("no
production glue yet between ConfigurationService task revisions and
EngineSessionManager routes"): ``ConfigurationService.apply()`` is now
actually called, from ``EngineSessionManagerAdapter.open()`` itself, right
after a coordinator's session opens -- see that method's own
``_maybe_apply_configuration`` docstring for exactly when it runs and why
a rejection/partial application never gets promoted to ``'active'``
(covered by ``test_rejected_configuration_never_becomes_active`` below).
"""
from __future__ import annotations

import sys
import threading
import uuid
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "plugins" / "cortex" / "scripts"))

from cortex_runtime.acp.agent import Agent  # noqa: E402
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime  # noqa: E402
from cortex_runtime.acp.production_runtime import ProductionOrchestratingRuntime  # noqa: E402
from cortex_runtime.configuration.resolution import ConfigurationInputs  # noqa: E402
from cortex_runtime.configuration.service import ConfigurationService  # noqa: E402
from cortex_runtime.connections import registry as connection_registry  # noqa: E402
from cortex_runtime.connections.model import AuthMode  # noqa: E402
from cortex_runtime.delegation import mcp_tools  # noqa: E402
from cortex_runtime.delegation.live_adapters import (  # noqa: E402
    ConfigurationServiceSnapshotSource, EngineSessionManagerAdapter,
)
from cortex_runtime.recovery import registry as recovery_registry  # noqa: E402
from cortex_runtime.routing import store as routing_store  # noqa: E402

import cortex_acp_setup as setup  # noqa: E402
from engine_fakes import default_scenario, fake_launch_command, write_scenario  # noqa: E402
from orchestration_fakes import FakeWorkspace  # noqa: E402


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


def _isolate(tmp_path, monkeypatch):
    config_home = tmp_path / "config"
    state_home = tmp_path / "state"
    home = tmp_path / "home"
    for path in (config_home, state_home, home):
        path.mkdir()
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    monkeypatch.setenv("XDG_STATE_HOME", str(state_home))
    monkeypatch.setenv("HOME", str(home))
    return dict(HOME=str(home), XDG_CONFIG_HOME=str(config_home), XDG_STATE_HOME=str(state_home))


_MODEL = "claude-opus-placeholder"


def _one_time_setup(tmp_path, engine_dir, *, config_options=None, configure_behavior="accept"):
    """Registers one claude-agent-acp connection (QUALIFIED instruction
    delivery -- see ``engine.instruction_delivery_registry`` -- so the
    coordinator role's non-negotiable ``critical_rules`` need no
    ``allow_unqualified_delivery`` escape hatch, matching every other
    real-adapter coordinator test in this suite) and pins its model as the
    default route, purely through ``scripts/cortex_acp_setup.py``."""
    setup.init_storage()
    scenario_path = write_scenario(engine_dir, default_scenario(
        prompt_updates=[dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="Ready."))],
        prompt_stop_reason="end_turn",
        config_options=config_options,
        configure_behavior=configure_behavior,
    ))
    secret_file = tmp_path / "secret.txt"
    secret_file.write_bytes(b"fake-api-key")
    conn = setup.add_connection(
        name="work", engine="claude-agent-acp", provider="anthropic", auth_mode=AuthMode.API_KEY,
        key_kind="anthropic_api_key", api_key_reader=lambda: secret_file.read_bytes(),
        launch_command=fake_launch_command(scenario_path),
    )
    pinned = setup.pin_route(connection_id=conn["connection_id"], model=_MODEL)
    return conn, pinned


def test_fresh_task_configuration_is_auto_proposed_from_stored_setup_state_alone(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    _conn, pinned = _one_time_setup(tmp_path, engine_dir)

    runtime = ProductionOrchestratingRuntime(environ=environ)
    agent = Agent(runtime, environ=environ)
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    agent.client_capabilities = {}

    session_result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = session_result["sessionId"]
    # The persisted setup default is already the current selection shown to
    # a brand-new session -- no explicit choice made in this session at all.
    model_option = next(o for o in session_result["configOptions"] if o["id"] == "model")
    assert model_option["currentValue"] == pinned["route_id"]

    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.portable.identity import SourceEvent

    store = agent._store_for(str(project))
    result = bootstrap_task(store, SourceEvent(origin="acp:test", event_id=str(uuid.uuid4())), "Get started.",
                             owner_ref="acp:test")
    session = agent.sessions[session_id]
    session["task_id"] = result.task_id
    session["owner_epoch"] = result.owner_epoch
    agent._propose_pending_selection_for_new_task(store, session, result.task_id)

    history = ConfigurationService(store).history(result.task_id)
    assert history, "no configuration revision was auto-proposed from the stored setup default"
    assert history[-1]["route_id"] == pinned["route_id"]
    assert history[-1]["route_connection_id"] == pinned["connection_id"]


def _bootstrap_configured_task(store, registry_state, pinned):
    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.portable.identity import SourceEvent

    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Ship the report.",
                             owner_ref="owner-1")
    task_id = result.task_id
    from cortex_runtime.acp import config_options as acp_config_options

    entries = acp_config_options.selectable_entries_from_snapshots(registry_state.connections, None)
    entry = next(e for e in entries if e.route.id == pinned["route_id"])
    service = ConfigurationService(store)
    service.propose(task_id, entry, ConfigurationInputs(route=entry.route, parameters={}), 0, result.owner_epoch)
    return task_id, service


def test_fresh_task_coordinator_applies_configuration_and_delegates_one_worker(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    conn, pinned = _one_time_setup(tmp_path, engine_dir, config_options=[
        dict(id="model", type="select", options=[dict(value=_MODEL, name="Placeholder")]),
    ])

    from cortex_runtime.store import Store

    store = Store(project / ".cortex", project_root=project)
    registry_state = connection_registry.read_registry(environ)
    task_id, service = _bootstrap_configured_task(store, registry_state, pinned)
    revision_id = service.history(task_id)[-1]["id"]
    assert service.history(task_id)[-1]["state"] == "selected"

    def _connection_factory(_task_id):
        return registry_state.connections[conn["connection_id"]]

    adapter = EngineSessionManagerAdapter(_connection_factory, lambda _t: str(project), environ=environ,
                                           store=store, call_timeout=10.0)
    runtime = OrchestratingRuntime(store, adapter, FakeWorkspace(), ConfigurationServiceSnapshotSource(service),
                                    wait_timeout_seconds=15.0)
    try:
        channel, session_ref = runtime._ensure_coordinator(task_id)
        assert session_ref.startswith("es_")

        # T008: the coordinator's own session actually confirmed this
        # revision's configuration -- it is now 'active' with a real
        # evidence ref, no manual apply() call anywhere in this test.
        active = service.current(task_id)
        assert active is not None, "ConfigurationService.apply() was never wired -- current() still resolves nothing"
        assert active["id"] == revision_id
        assert active["confirmation_evidence_ref"]

        mcp_tools.dispatch(store, channel, "delegate", dict(
            role="backend-dev", title="Investigate", brief="Look into it.", acceptance="done",
            request_key=str(uuid.uuid4()),
        ))
        cancel_event = threading.Event()
        runtime.start_prompt(task_id, "s1", "Ship the report.", lambda _c: True, cancel_event)
        with store.connection() as db:
            provisioned = db.execute(
                "SELECT COUNT(*) FROM delegation_provisioning WHERE task_id=?", (task_id,),
            ).fetchone()[0]
            attempt_id = db.execute(
                "SELECT coordinator_attempt_ref FROM task_orchestration WHERE task_id=?", (task_id,),
            ).fetchone()[0]
        assert provisioned == 1

        # T023: the coordinator's own attempt recorded this exact revision
        # as its effective configuration.
        effective = recovery_registry.get_effective_context(store, attempt_id)
        assert effective is not None
        assert effective["selected_config_revision_id"] == revision_id
        assert effective["effective_config_revision_id"] == revision_id

        # T027: the same attempt is durably route-audited.
        audit_rows = routing_store.list_audit_for_attempt(store, attempt_id)
        assert audit_rows
        assert audit_rows[0]["config_revision_id"] == revision_id
        assert audit_rows[0]["engine"] == "claude-agent-acp"
        assert audit_rows[0]["decision"] == "authorized"
    finally:
        for manager in list(adapter._managers.values()):
            manager.shutdown(grace=2.0)


def test_rejected_configuration_never_becomes_active(tmp_path, monkeypatch):
    """The engine offers no configOptions at all (the ordinary
    ``ConfigurationRejected('engine_does_not_support_configuration_options')``
    path) -- the coordinator session still opens and a worker still gets
    delegated (an unconfirmed configuration never aborts an otherwise-
    successful open), but the revision itself is never promoted to
    ``'active'``."""
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    conn, pinned = _one_time_setup(tmp_path, engine_dir, config_options=None)

    from cortex_runtime.store import Store

    store = Store(project / ".cortex", project_root=project)
    registry_state = connection_registry.read_registry(environ)
    task_id, service = _bootstrap_configured_task(store, registry_state, pinned)
    revision_id = service.history(task_id)[-1]["id"]

    def _connection_factory(_task_id):
        return registry_state.connections[conn["connection_id"]]

    adapter = EngineSessionManagerAdapter(_connection_factory, lambda _t: str(project), environ=environ,
                                           store=store, call_timeout=10.0)
    runtime = OrchestratingRuntime(store, adapter, FakeWorkspace(), ConfigurationServiceSnapshotSource(service),
                                    wait_timeout_seconds=15.0)
    try:
        channel, session_ref = runtime._ensure_coordinator(task_id)
        assert session_ref.startswith("es_")

        assert service.current(task_id) is None
        history = service.history(task_id)
        assert history[-1]["id"] == revision_id
        assert history[-1]["state"] == "failed"
        assert history[-1]["error_code"] == "engine_does_not_support_configuration_options"

        mcp_tools.dispatch(store, channel, "delegate", dict(
            role="backend-dev", title="Investigate", brief="Look into it.", acceptance="done",
            request_key=str(uuid.uuid4()),
        ))
        cancel_event = threading.Event()
        runtime.start_prompt(task_id, "s1", "Ship the report.", lambda _c: True, cancel_event)
        with store.connection() as db:
            provisioned = db.execute(
                "SELECT COUNT(*) FROM delegation_provisioning WHERE task_id=?", (task_id,),
            ).fetchone()[0]
        assert provisioned == 1
        assert service.current(task_id) is None
    finally:
        for manager in list(adapter._managers.values()):
            manager.shutdown(grace=2.0)
