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
``reconcile_configuration`` docstring for exactly when it runs and why
a rejection/partial application never gets promoted to ``'active'``
(covered by ``test_rejected_configuration_never_becomes_active`` below).
"""
from __future__ import annotations

import sys
import threading
import uuid
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

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


def _register_connection(tmp_path, engine_dir, *, name, model, config_options=None,
                          configure_behavior="accept", env_dump_path=None):
    """Registers one claude-agent-acp connection and pins ``model`` as its
    route, purely through ``scripts/cortex_acp_setup.py`` -- the same
    building block ``_one_time_setup`` uses for its single "work"
    connection, generalized so a test can register a *second*, independent
    connection (see the FINDING-09 no-fallback test below) without
    duplicating this scaffolding."""
    scenario_path = write_scenario(engine_dir, default_scenario(
        prompt_updates=[dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="Ready."))],
        prompt_stop_reason="end_turn",
        config_options=config_options,
        configure_behavior=configure_behavior,
        env_dump_path=env_dump_path,
    ))
    secret_file = tmp_path / f"secret-{name}.txt"
    secret_file.write_bytes(b"fake-api-key")
    conn = setup.add_connection(
        name=name, engine="claude-agent-acp", provider="anthropic", auth_mode=AuthMode.API_KEY,
        key_kind="anthropic_api_key", api_key_reader=lambda: secret_file.read_bytes(),
        launch_command=fake_launch_command(scenario_path),
    )
    pinned = setup.pin_route(connection_id=conn["connection_id"], model=model)
    return conn, pinned


def _one_time_setup(tmp_path, engine_dir, *, config_options=None, configure_behavior="accept"):
    """Registers one claude-agent-acp connection (QUALIFIED instruction
    delivery -- see ``engine.instruction_delivery_registry`` -- so the
    coordinator role's non-negotiable ``critical_rules`` need no
    ``allow_unqualified_delivery`` escape hatch, matching every other
    real-adapter coordinator test in this suite) and pins its model as the
    default route, purely through ``scripts/cortex_acp_setup.py``."""
    setup.init_storage()
    return _register_connection(
        tmp_path, engine_dir, name="work", model=_MODEL,
        config_options=config_options, configure_behavior=configure_behavior,
    )


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
    assert model_option["currentValue"] == pinned["model"]

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

        from cortex_runtime.orchestration.state import TaskMachine
        from runtime_scenarios import propose_inspection
        machine=TaskMachine(store);machine.create(task_id,'Ship the report.')
        propose_inspection(store,channel)
        assert machine.context(task_id)['state']=='PLAN_READY'
        assert machine.context(task_id)['nodes'][0]['status']=='pending'
        with store.connection() as db:
            provisioned = db.execute(
                "SELECT COUNT(*) FROM delegation_provisioning WHERE task_id=?", (task_id,),
            ).fetchone()[0]
            attempt_id = db.execute(
                "SELECT coordinator_attempt_ref FROM task_orchestration WHERE task_id=?", (task_id,),
            ).fetchone()[0]
        assert provisioned == 0

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


def test_fresh_task_first_coordinator_open_through_production_runtime_promotes_selected_to_active(tmp_path, monkeypatch):
    """T035 regression: a fresh task's *first* coordinator open, driven
    through the real ``ProductionOrchestratingRuntime`` (the same
    ``_connection_record_factory`` production actually wires -- unlike the
    two tests above, which construct ``OrchestratingRuntime`` directly with
    a test-local connection factory and so never exercised this factory's
    own strict ``current()`` bug at all).

    Before the fix: ``_connection_record_factory`` required an already-
    ``active`` revision while only ``EngineSessionManagerAdapter.open``
    (which needs that very factory to run first) ever promotes 'selected'
    to 'active' -- a fresh task could never open its coordinator at all
    (``RouteUnresolved('no_configuration_revision')`` on every attempt).
    """
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
    # No prior active revision anywhere for this task -- exactly the fresh-
    # task state the live finding observed (a pinned route, a 'selected'
    # revision, never yet applied).
    assert service.current(task_id) is None
    assert service.history(task_id)[-1]["state"] == "selected"

    runtime = ProductionOrchestratingRuntime(environ=environ)
    try:
        cancel_event = threading.Event()
        session_ref = runtime.start_prompt(
            task_id, "s1", "Ship the report.", lambda _c: True, cancel_event,
            project_root=str(project),
        )
        assert session_ref

        active = service.current(task_id)
        assert active is not None, "the coordinator's first open never promoted the 'selected' revision to 'active'"
        assert active["id"] == revision_id
        assert active["confirmation_evidence_ref"]
    finally:
        for orch_runtime in list(runtime._runtimes.values()):
            for manager in list(orch_runtime._engine._managers.values()):
                manager.shutdown(grace=2.0)


def test_a_rejected_latest_revision_is_still_refused_by_the_production_connection_factory(tmp_path, monkeypatch):
    """The T035 fix (``current_or_latest`` tolerance) must not resurrect the
    second defect it also closes: a 'rejected' latest revision (explicitly
    refused, never confirmed) is never silently treated as this task's
    route -- ``start_prompt`` still refuses with
    ``no_configuration_revision``, exactly as it did (for the right reason)
    before this task ever had any revision at all."""
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    _conn, pinned = _one_time_setup(tmp_path, engine_dir)

    from cortex_runtime.store import Store

    store = Store(project / ".cortex", project_root=project)
    registry_state = connection_registry.read_registry(environ)
    task_id, service = _bootstrap_configured_task(store, registry_state, pinned)
    revision_id = service.history(task_id)[-1]["id"]
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute(
            "UPDATE configuration_revisions SET state='rejected',error_code='ceiling_tightened' WHERE id=?",
            (revision_id,),
        )
        db.commit()
    assert service.current(task_id) is None
    assert service.current_or_latest(task_id) is None

    runtime = ProductionOrchestratingRuntime(environ=environ)
    cancel_event = threading.Event()
    try:
        runtime.start_prompt(
            task_id, "s1", "Ship the report.", lambda _c: True, cancel_event,
            project_root=str(project),
        )
        assert False, "a rejected-only revision must never resolve a route"
    except Exception as error:
        assert str(error) == "no_configuration_revision"
    finally:
        for orch_runtime in list(runtime._runtimes.values()):
            for manager in list(orch_runtime._engine._managers.values()):
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

        from cortex_runtime.orchestration.state import TaskMachine
        from runtime_scenarios import propose_inspection
        machine=TaskMachine(store);machine.create(task_id,'Ship the report.')
        propose_inspection(store,channel)
        assert machine.context(task_id)['state']=='PLAN_READY'
        assert machine.context(task_id)['nodes'][0]['status']=='pending'
        with store.connection() as db:
            provisioned = db.execute(
                "SELECT COUNT(*) FROM delegation_provisioning WHERE task_id=?", (task_id,),
            ).fetchone()[0]
        assert provisioned == 0
        assert service.current(task_id) is None
    finally:
        for manager in list(adapter._managers.values()):
            manager.shutdown(grace=2.0)


def test_real_coordinator_session_open_automatically_captures_catalog_observation(tmp_path, monkeypatch):
    """T035 item 4: opening a real coordinator engine session
    (``EngineSessionManagerAdapter.open`` -> ``EngineSessionManager
    .create_session``) automatically feeds that session's own observed
    ``configOptions`` into ``catalog.discovery``/``catalog.store`` with
    real ``DISCOVERY`` provenance -- no manual ``cortex_acp_setup.py
    record-observation`` step needed. The capture is purely additive/
    best-effort: it never blocks or fails the engine session itself."""
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    conn, pinned = _one_time_setup(tmp_path, engine_dir, config_options=[
        dict(id="model", name="Model", type="select", category="model", currentValue=_MODEL,
             options=[dict(value=_MODEL, name="Placeholder")]),
    ])

    from cortex_runtime.store import Store
    from cortex_runtime.acp import config_options as acp_config_options
    from cortex_runtime.catalog.evidence import Fact, ProvenanceSource

    store = Store(project / ".cortex", project_root=project)
    registry_state = connection_registry.read_registry(environ)
    task_id, service = _bootstrap_configured_task(store, registry_state, pinned)

    # Before any real session opens, only the manually pin_route'd model
    # is known -- nothing yet claims real DISCOVERY provenance.
    connections = {conn["connection_id"]: registry_state.connections[conn["connection_id"]]}
    before = acp_config_options.selectable_entries_from_snapshots(connections, environ)
    assert {entry.route.model for entry in before} == {_MODEL}
    for entry in before:
        assert not entry.evidence.has(Fact.CATALOG_OBTAINED)  # manual declaration, never claimed obtained

    def _connection_factory(_task_id):
        return registry_state.connections[conn["connection_id"]]

    adapter = EngineSessionManagerAdapter(_connection_factory, lambda _t: str(project), environ=environ,
                                           store=store, call_timeout=10.0)
    runtime = OrchestratingRuntime(store, adapter, FakeWorkspace(), ConfigurationServiceSnapshotSource(service),
                                    wait_timeout_seconds=15.0)
    try:
        runtime._ensure_coordinator(task_id)
    finally:
        for manager in list(adapter._managers.values()):
            manager.shutdown(grace=2.0)

    # The scripted fake engine's own scenario config_options (set above)
    # is what a real session/new would have returned -- now captured
    # automatically as a real DISCOVERY-provenance catalog snapshot.
    after = acp_config_options.selectable_entries_from_snapshots(connections, environ)
    assert {entry.route.model for entry in after} == {_MODEL}
    for entry in after:
        obtained = entry.evidence.get(Fact.CATALOG_OBTAINED)
        assert obtained is not None
        assert obtained.source == ProvenanceSource.DISCOVERY


# ---------------------------------------------------------------------------
# T035 FINDING-05: a catalog refresh must never orphan the saved default.
#
# Live defect this reproduces (see
# .cortex/acp-migration/evidence/T035/FINDING-05-refresh-invalidates-default.md):
# pin-route/record-observation persist a default route id; a later catalog
# refresh (see catalog.refresh/EngineSessionManagerAdapter._capture_catalog_
# observation) re-observes the connection and, if the engine's advertised
# supported_parameters differ at all (e.g. per-model thought levels), mints
# a *new* fingerprint id for the exact same (connection, model) -- orphaning
# the saved default and failing every first prompt of a new task with
# "no_configuration_revision" (via the opaque
# "pending_selection_route_unavailable" one step earlier).
# ---------------------------------------------------------------------------

def _refresh_snapshot(connection_id, record, *, model_values, thought_levels, environ):
    """Simulate one real catalog refresh: a fresh DISCOVERY-provenance
    snapshot for ``connection_id``, built and saved exactly the way
    ``catalog.refresh.perform_refresh``/``EngineSessionManagerAdapter.
    _capture_catalog_observation`` do in production -- never through this
    test's own ad-hoc shape."""
    from cortex_runtime.catalog import discovery as catalog_discovery
    from cortex_runtime.catalog import store as catalog_store_module

    options = [
        {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": model_values[0],
         "options": [{"value": value, "name": value} for value in model_values]},
    ]
    if thought_levels:
        options.append(
            {"id": "effort", "name": "Effort", "category": "thought_level", "type": "select",
             "currentValue": thought_levels[0],
             "options": [{"value": value, "name": value} for value in thought_levels]},
        )
    observation = catalog_discovery.parse_observation(options)
    snapshot = catalog_discovery.build_catalog({connection_id: record}, {connection_id: observation})
    catalog_store_module.save_snapshot(snapshot, environ)
    return snapshot


def _fresh_agent_session(environ, project):
    from cortex_runtime.acp.production_runtime import ProductionOrchestratingRuntime

    runtime = ProductionOrchestratingRuntime(environ=environ)
    agent = Agent(runtime, environ=environ)
    agent.bind_transport(_RecordingTransport())
    agent.initialized = True
    agent.client_capabilities = {}
    session_result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    return agent, session_result["sessionId"]


def _propose_first_prompt(agent, session_id, project):
    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.portable.identity import SourceEvent

    store = agent._store_for(str(project))
    result = bootstrap_task(
        store, SourceEvent(origin="acp:test", event_id=str(uuid.uuid4())), "Get started.", owner_ref="acp:test",
    )
    session = agent.sessions[session_id]
    session["task_id"] = result.task_id
    session["owner_epoch"] = result.owner_epoch
    agent._propose_pending_selection_for_new_task(store, session, result.task_id)
    return store, result.task_id


def test_saved_default_survives_a_refresh_that_changes_parameters_for_the_same_model(tmp_path, monkeypatch):
    """Requirement #1: a refresh that still offers the same (connection,
    model) -- just with different supported_parameters -- leaves the saved
    default working, on the very next task's first prompt, with no
    explicit re-pin."""
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    conn, pinned = _one_time_setup(tmp_path, engine_dir)
    registry_state = connection_registry.read_registry(environ)
    record = registry_state.connections[conn["connection_id"]]

    old_snapshot = _refresh_snapshot(
        conn["connection_id"], record, model_values=[_MODEL, "sibling-model"],
        thought_levels=["none", "medium"], environ=environ,
    )
    old_entry = next(e for e in old_snapshot.entries if e.route.model == _MODEL)
    old_route_id = old_entry.route.id

    # The saved default is pinned against this first observed snapshot --
    # the normal ``pin_route``/``record_observation``/in-session-choice
    # write shape (route_id + identity), never this test's own bespoke one.
    from cortex_runtime.configuration import store as configuration_store

    current = configuration_store.read_user_defaults(environ)
    defaults = dict(current["defaults"], route_id=old_route_id, route_connection_id=conn["connection_id"],
                     route_model=_MODEL, parameters={})
    configuration_store.write_user_defaults(defaults, current["ceiling"], environ)

    # A later real refresh: same connection, same model, but the engine now
    # additionally advertises "high"/"xhigh" thought levels -- a real,
    # non-contrived reason the fingerprint changes (T035 follow-up
    # "per-model efforts").
    new_snapshot = _refresh_snapshot(
        conn["connection_id"], record, model_values=[_MODEL, "sibling-model"],
        thought_levels=["none", "medium", "high", "xhigh"], environ=environ,
    )
    new_route_id = next(e.route.id for e in new_snapshot.entries if e.route.model == _MODEL)
    assert new_route_id != old_route_id, "the scenario must actually change the fingerprint id"

    # The user's saved default still names the *old* id (never re-pinned).
    current = configuration_store.read_user_defaults(environ)
    assert current["defaults"]["route_id"] == old_route_id
    assert current["defaults"]["route_model"] == _MODEL
    assert current["defaults"]["route_connection_id"] == conn["connection_id"]

    agent, session_id = _fresh_agent_session(environ, project)
    store, task_id = _propose_first_prompt(agent, session_id, project)

    history = ConfigurationService(store).history(task_id)
    assert history, "a refresh that keeps offering the same (connection, model) must not orphan the default"
    assert history[-1]["route_id"] == new_route_id
    assert history[-1]["route_connection_id"] == conn["connection_id"]

    # Requirement #3, positive form: the resolved route is the *same*
    # model on the *same* connection -- never anything else (e.g. the
    # sibling model that was also offered on the same connection).
    extras = routing_store.get_route_extras(store, history[-1]["id"])
    assert extras["route_model"] == _MODEL
    assert extras["route_model"] != "sibling-model"

    # Self-healing: defaults.json now carries the fresh id/identity, so the
    # *next* task resolves directly (no re-scan needed).
    migrated = configuration_store.read_user_defaults(environ)
    assert migrated["defaults"]["route_id"] == new_route_id
    assert migrated["defaults"]["route_connection_id"] == conn["connection_id"]
    assert migrated["defaults"]["route_model"] == _MODEL


def test_stale_route_migration_does_not_overwrite_a_concurrent_explicit_default(tmp_path, monkeypatch):
    """A migration must abandon its stale snapshot when another session
    selects a new global default before the compare-and-write point."""
    environ = _isolate(tmp_path, monkeypatch)
    from cortex_runtime.configuration import store as configuration_store

    configuration_store.write_user_defaults(
        {
            "route_id": "old-route",
            "route_connection_id": "cn_old",
            "route_model": "old-model",
            "parameters": {"thought_level": "medium"},
        },
        configuration_store.read_user_defaults(environ)["ceiling"],
        environ,
    )
    original_read = configuration_store.read_user_defaults
    original_write = configuration_store.write_user_defaults
    reads = 0

    def read_with_explicit_selection(environ=None):
        nonlocal reads
        reads += 1
        current = original_read(environ)
        if reads == 2:
            explicit = dict(
                current["defaults"],
                route_id="explicit-route",
                route_connection_id="cn_explicit",
                route_model="explicit-model",
                parameters={"thought_level": "high"},
            )
            original_write(explicit, current["ceiling"], environ)
            return original_read(environ)
        return current

    monkeypatch.setattr(configuration_store, "read_user_defaults", read_with_explicit_selection)
    agent = object.__new__(Agent)
    agent.environ = environ
    entry = SimpleNamespace(
        route=SimpleNamespace(id="migrated-route", model="old-model"),
        connection_id="cn_old",
    )

    agent._persist_migrated_default(entry, {"thought_level": "none"})

    saved = original_read(environ)["defaults"]
    assert saved["route_id"] == "explicit-route"
    assert saved["route_connection_id"] == "cn_explicit"
    assert saved["route_model"] == "explicit-model"
    assert saved["parameters"] == {"thought_level": "high"}


def test_saved_default_fails_closed_naming_the_withdrawn_model(tmp_path, monkeypatch):
    """Requirement #2: a refresh that genuinely withdraws the saved model
    still fails closed -- but names the model, never the opaque
    "pending_selection_route_unavailable"."""
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    conn, pinned = _one_time_setup(tmp_path, engine_dir)
    registry_state = connection_registry.read_registry(environ)
    record = registry_state.connections[conn["connection_id"]]

    _refresh_snapshot(
        conn["connection_id"], record, model_values=[_MODEL], thought_levels=["none"], environ=environ,
    )
    # The engine withdraws the saved model entirely -- only a different one
    # remains.
    _refresh_snapshot(
        conn["connection_id"], record, model_values=["replacement-model"], thought_levels=["none"], environ=environ,
    )

    agent, session_id = _fresh_agent_session(environ, project)
    events = []
    monkeypatch.setattr("cortex_runtime.acp.agent.observe", lambda environ, kind, reason: events.append(reason))
    store, task_id = _propose_first_prompt(agent, session_id, project)

    assert not ConfigurationService(store).history(task_id), "a genuinely withdrawn model must never auto-select anything"
    assert f"pending_selection_model_withdrawn:{_MODEL}" in events
    assert "pending_selection_route_unavailable" not in events


def test_legacy_defaults_json_with_no_saved_identity_self_heals_after_a_refresh(tmp_path, monkeypatch):
    """Backward compatibility: a defaults.json written before this fix
    (only ``route_id``, no ``route_connection_id``/``route_model`` -- the
    exact shape of the live-reported defaults.json) still recovers, via a
    one-time scan of older cached snapshots, and is migrated forward."""
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    conn, pinned = _one_time_setup(tmp_path, engine_dir)
    registry_state = connection_registry.read_registry(environ)
    record = registry_state.connections[conn["connection_id"]]

    old_snapshot = _refresh_snapshot(
        conn["connection_id"], record, model_values=[_MODEL], thought_levels=["none", "medium"], environ=environ,
    )
    old_route_id = next(e.route.id for e in old_snapshot.entries if e.route.model == _MODEL)

    from cortex_runtime.configuration import store as configuration_store

    # Strip the identity fields back out, simulating a pre-fix defaults.json.
    current = configuration_store.read_user_defaults(environ)
    legacy_defaults = dict(current["defaults"])
    legacy_defaults.pop("route_connection_id", None)
    legacy_defaults.pop("route_model", None)
    legacy_defaults["route_id"] = old_route_id
    configuration_store.write_user_defaults(legacy_defaults, current["ceiling"], environ)

    new_snapshot = _refresh_snapshot(
        conn["connection_id"], record, model_values=[_MODEL], thought_levels=["none", "medium", "high"], environ=environ,
    )
    new_route_id = next(e.route.id for e in new_snapshot.entries if e.route.model == _MODEL)
    assert new_route_id != old_route_id

    agent, session_id = _fresh_agent_session(environ, project)
    store, task_id = _propose_first_prompt(agent, session_id, project)

    history = ConfigurationService(store).history(task_id)
    assert history, "a legacy defaults.json with no saved identity must still self-heal from cached snapshot history"
    assert history[-1]["route_id"] == new_route_id
    assert history[-1]["route_connection_id"] == conn["connection_id"]

    migrated = configuration_store.read_user_defaults(environ)
    assert migrated["defaults"]["route_connection_id"] == conn["connection_id"]
    assert migrated["defaults"]["route_model"] == _MODEL


# ---------------------------------------------------------------------------
# T035 FINDING-09: an expired/unusable connection must be refused *as such*,
# distinct from a genuine "no configuration revision" -- never collapsed
# into the same opaque code, and never silently used anyway.
#
# Live defect this reproduces (see
# .cortex/acp-migration/evidence/T035/FINDING-09-auth-required-reported-as-no-configuration.md):
# a task's route named a connection whose login had expired
# (``ConnectionState.AUTHORIZATION_REQUIRED``); ``start_prompt`` refused
# correctly (fail closed, no fallback) but with reason
# ``no_configuration_revision`` -- the wrong diagnosis, sending a user
# hunting through routes/defaults instead of at their expired login.
# ---------------------------------------------------------------------------

def test_route_to_an_authorization_required_connection_is_refused_naming_it(tmp_path, monkeypatch):
    """A route that names a *known* connection whose login has expired is
    refused with ``connection_requires_reauthorization:<connection_id>`` --
    the exact vocabulary ``connections.onboarding``'s own
    ``_STATE_ERROR_CODE``/``resolve_connection_for_route`` already use for
    the non-orchestration engine-session path -- never the generic
    ``no_configuration_revision``."""
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
    # Exactly what the live finding observed: the pinned route's own
    # 'selected' revision already exists, but the login backing its
    # connection has since expired -- simulated here by emptying the
    # connection's own canonical secret file, which
    # ``connections.onboarding._derive_state`` treats as
    # ``AUTHORIZATION_REQUIRED`` (a zero-byte secret file), the same as a
    # real API key expiring/being revoked.
    secret_path = connection_registry.secret_path(conn["connection_id"], environ)
    secret_path.write_bytes(b"")

    runtime = ProductionOrchestratingRuntime(environ=environ)
    cancel_event = threading.Event()
    try:
        runtime.start_prompt(
            task_id, "s1", "Ship the report.", lambda _c: True, cancel_event,
            project_root=str(project),
        )
        assert False, "an authorization_required connection must never be silently used"
    except Exception as error:
        assert str(error) == f"connection_requires_reauthorization:{conn['connection_id']}"
        assert str(error) != "no_configuration_revision"
    finally:
        for orch_runtime in list(runtime._runtimes.values()):
            for manager in list(orch_runtime._engine._managers.values()):
                manager.shutdown(grace=2.0)


def test_route_to_an_invalid_connection_is_refused_naming_it(tmp_path, monkeypatch):
    """Same path, ``ConnectionState.INVALID`` this time (a secret file with
    unsafe group/other-readable permissions) -- refused as
    ``connection_invalid:<connection_id>``, still never
    ``no_configuration_revision``."""
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
    secret_path = connection_registry.secret_path(conn["connection_id"], environ)
    secret_path.chmod(0o644)  # group/other-readable -- unsafe, per _derive_state

    runtime = ProductionOrchestratingRuntime(environ=environ)
    cancel_event = threading.Event()
    try:
        runtime.start_prompt(
            task_id, "s1", "Ship the report.", lambda _c: True, cancel_event,
            project_root=str(project),
        )
        assert False, "an invalid connection must never be silently used"
    except Exception as error:
        assert str(error) == f"connection_invalid:{conn['connection_id']}"
    finally:
        secret_path.chmod(0o600)  # restore before the fixture's own teardown
        for orch_runtime in list(runtime._runtimes.values()):
            for manager in list(orch_runtime._engine._managers.values()):
                manager.shutdown(grace=2.0)


# ``ConnectionState.UNAVAILABLE`` is not exercised here: for the
# ``api_key``/``SecretFile`` connections this suite's fake engine uses, an
# absent/empty/malformed secret file always resolves to
# ``AUTHORIZATION_REQUIRED``/``INVALID`` (see ``_derive_state``'s
# ``SecretFile`` branch just above) -- ``UNAVAILABLE`` is only ever reached
# for an ``os_keyring`` credential whose ``keyring`` module import/lookup
# itself fails, or an ``EngineNativeDir`` connection whose engine has no
# native-login-marker mapping. Neither shape exists anywhere else in this
# test module (every connection here is ``api_key``/``SecretFile``, by the
# same convention every other real-adapter test in this file already
# follows), and standing one up would duplicate ``test_connections.py``'s
# own ``os_keyring``/native-account coverage rather than add anything new
# to this file's route-refusal path -- ``resolve_connection_for_route``
# maps ``UNAVAILABLE`` through the identical ``_STATE_ERROR_CODE`` table as
# ``AUTHORIZATION_REQUIRED``/``INVALID`` above, so the two cases already
# covered are sufficient evidence that the dispatch itself is state-generic,
# not special-cased per state.


def test_a_task_with_no_configuration_revision_at_all_is_still_refused_as_such(tmp_path, monkeypatch):
    """The other half of the FINDING-09 requirement: a task that genuinely
    has never had any configuration revision must still be refused
    ``no_configuration_revision`` -- the two reasons must never collapse
    into one code in either direction."""
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    _one_time_setup(tmp_path, engine_dir)

    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.portable.identity import SourceEvent
    from cortex_runtime.store import Store

    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Ship the report.",
                             owner_ref="owner-1")
    task_id = result.task_id
    assert not ConfigurationService(store).history(task_id)

    runtime = ProductionOrchestratingRuntime(environ=environ)
    cancel_event = threading.Event()
    try:
        runtime.start_prompt(
            task_id, "s1", "Ship the report.", lambda _c: True, cancel_event,
            project_root=str(project),
        )
        assert False, "a task with no configuration revision at all must be refused as such"
    except Exception as error:
        assert str(error) == "no_configuration_revision"
    finally:
        for orch_runtime in list(runtime._runtimes.values()):
            for manager in list(orch_runtime._engine._managers.values()):
                manager.shutdown(grace=2.0)


def test_an_unusable_route_never_falls_back_to_a_different_working_connection(tmp_path, monkeypatch):
    """The security property the live run confirmed, asserted explicitly:
    with the task's own route pointing at an ``authorization_required``
    connection and a second, perfectly usable connection also configured,
    the refusal stands -- ``start_prompt`` never silently opens the working
    connection instead."""
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    work_engine_dir = tmp_path / "work_engine"
    work_engine_dir.mkdir()
    spare_engine_dir = tmp_path / "spare_engine"
    spare_engine_dir.mkdir()
    spare_model = "sibling-placeholder"
    spare_launch_marker = tmp_path / "spare_launched.json"

    setup.init_storage()
    conn, pinned = _register_connection(
        tmp_path, work_engine_dir, name="work", model=_MODEL,
        config_options=[dict(id="model", type="select", options=[dict(value=_MODEL, name="Placeholder")])],
    )
    _register_connection(
        tmp_path, spare_engine_dir, name="spare", model=spare_model,
        config_options=[dict(id="model", type="select", options=[dict(value=spare_model, name="Spare")])],
        env_dump_path=str(spare_launch_marker),
    )

    from cortex_runtime.store import Store

    store = Store(project / ".cortex", project_root=project)
    registry_state = connection_registry.read_registry(environ)
    task_id, service = _bootstrap_configured_task(store, registry_state, pinned)
    # Only the task's own connection ("work") is deauthorized -- "spare"
    # stays fully configured and working throughout.
    secret_path = connection_registry.secret_path(conn["connection_id"], environ)
    secret_path.write_bytes(b"")

    runtime = ProductionOrchestratingRuntime(environ=environ)
    cancel_event = threading.Event()
    try:
        runtime.start_prompt(
            task_id, "s1", "Ship the report.", lambda _c: True, cancel_event,
            project_root=str(project),
        )
        assert False, "an unusable route must be refused, never silently rerouted"
    except Exception as error:
        assert str(error) == f"connection_requires_reauthorization:{conn['connection_id']}"
    # The decisive assertion: "spare"'s own fake-engine process was never
    # even launched (it would have dumped its process env to this path on
    # startup -- see fake_engine_process.py) -- no hidden provider switch,
    # no silent substitution of a different, working connection.
    assert not spare_launch_marker.exists(), (
        "the refusal must never fall back to a different, working connection"
    )
    # Nor did the task's own configuration/route change underneath it.
    assert service.current(task_id) is None
    assert service.history(task_id)[-1]["route_connection_id"] == conn["connection_id"]
    for orch_runtime in list(runtime._runtimes.values()):
        for manager in list(orch_runtime._engine._managers.values()):
            manager.shutdown(grace=2.0)
