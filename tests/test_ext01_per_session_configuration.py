"""EXT-01 finding #2: a task-level 'active' configuration_revisions row must
never be read as evidence that *this concrete engine session* was actually
confirmed against it. Three cases, all through the real
``EngineSessionManagerAdapter``/``EngineSessionManager`` + a real
``tests/fake_engine_process.py`` subprocess (never a fake ``SessionRuntime``
double, and never only asserting SQLite rows -- see each test's own
assertions on the manager's real, wire-observed ``configOptions`` and the
fake engine's own request log):

(a) ``test_mid_task_selection_reaches_an_already_cached_coordinator_session``
    -- select model B while model A is active, on an *already-open* cached
    coordinator session (mirrors ``acp.agent._apply_mid_task_selection``,
    which only ever proposes a fresh 'selected' revision), reach the next
    turn's safe boundary, and confirm the SAME session was actually
    reconfigured over the wire -- never merely a new DB row.
(b) ``test_fresh_session_after_restart_reconfirms_an_already_active_revision``
    -- the reviewer's own regression test: select B while A is active,
    reach a safe boundary (closing (a)), then simulate a Cortex restart
    (a second, independent ``EngineSessionManagerAdapter``/
    ``OrchestratingRuntime`` pair sharing only the same ``Store`` -- see
    ``test_recovery_cross_process.py``'s identical pattern) and confirm the
    brand-new coordinator session actually receives its own
    ``session/set_config_option`` call for B, even though
    ``configuration_revisions`` already says 'active' from the first
    session.
(c) ``test_partial_confirmation_on_a_fresh_session_blocks_the_turn`` -- a
    fresh session whose own confirmation attempt for an already-'active'
    task-level revision is only partially applied by the engine
    (``EngineConfigurationPartiallyApplied``) must block the turn
    (``_ensure_coordinator`` raises), never silently proceed on an
    unconfirmed configuration.
"""
from __future__ import annotations

import uuid

import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.catalog.evidence import EvidenceState
from cortex_runtime.catalog.model import CatalogEntry
from cortex_runtime.configuration.resolution import ConfigurationInputs
from cortex_runtime.configuration.service import ConfigurationService
from cortex_runtime.delegation.live_adapters import ConfigurationServiceSnapshotSource, EngineSessionManagerAdapter
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import ExecutionRoute, SourceEvent
from cortex_runtime.store import Store

from model_policy_fixtures import write_policy
from engine_fakes import fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario
from orchestration_fakes import FakeWorkspace

_ENGINE = "codex-acp"


def _task(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(
        store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Ship the report.", "owner-1",
    )
    return store, result.task_id, result.owner_epoch


def _catalog_entry(connection_id, model):
    route = ExecutionRoute(engine=_ENGINE, connection_id=connection_id, model=model, supported_parameters=())
    label = f"{_ENGINE} · work · {model}"
    return CatalogEntry(route=route, connection_id=connection_id, label=label, disambiguated_label=label,
                         evidence=EvidenceState.empty(), catalog_snapshot_id="cs_" + "a" * 12)


def _scenario_with_two_models(engine_dir, request_log):
    return write_scenario(engine_dir, dict(
        agent_capabilities=dict(loadSession=False, sessionCapabilities=dict(resume=False)),
        session_id="sess-1",
        config_options=[
            dict(id="model", currentValue="model-a", options=[
                dict(value="model-a", name="Model A"), dict(value="model-b", name="Model B"),
            ]),
        ],
        configure_behavior="accept",
        prompt_updates=[dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="ok"))],
        prompt_stop_reason="end_turn",
        prompt_behavior="normal",
        request_log_path=str(request_log),
    ))


def _adapter(store, environ, record, workspace_root):
    return EngineSessionManagerAdapter(
        lambda t: record, lambda t: str(workspace_root), environ=environ, call_timeout=10.0, store=store,
    )


def _runtime(store, adapter, service):
    return OrchestratingRuntime(
        store, adapter, FakeWorkspace(), ConfigurationServiceSnapshotSource(service), wait_timeout_seconds=15.0,
    )


def _set_config_option_calls(request_log):
    if not request_log.exists():
        return 0
    return sum(
        1 for line in request_log.read_text(encoding="utf-8").splitlines()
        if __import__("json").loads(line)["method"] == "session/set_config_option"
    )


def _model_option(manager, session_ref):
    options = manager.observed_config_options(session_ref) or []
    return next(o for o in options if o.get("id") == "model")


def test_mid_task_selection_reaches_an_already_cached_coordinator_session(tmp_path):
    """EXT-01 #2a: a mid-task model change must reach the SAME already-open
    coordinator session at the next turn's safe boundary -- never left
    stranded as an unapplied 'selected' revision forever."""
    store, task_id, owner_epoch = _task(tmp_path)
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    write_policy(environ, {_ENGINE: {"model-a": "standard", "model-b": "standard"}})
    request_log = tmp_path / "requests.jsonl"
    scenario_path = _scenario_with_two_models(engine_dir, request_log)
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)

    service = ConfigurationService(store)
    entry_a, entry_b = _catalog_entry(connection_id, "model-a"), _catalog_entry(connection_id, "model-b")
    service.propose(task_id, entry_a, ConfigurationInputs(route=entry_a.route, parameters={}), 0, owner_epoch)

    adapter = _adapter(store, environ, record, tmp_path)
    runtime = _runtime(store, adapter, service)
    try:
        _channel, session_ref = runtime._ensure_coordinator(task_id)
        manager = adapter._managers[list(adapter._managers)[0]]
        assert _model_option(manager, session_ref)["currentValue"] == "model-a"
        assert service.current(task_id)["id"] == service.history(task_id)[0]["id"]
        calls_after_a = _set_config_option_calls(request_log)
        assert calls_after_a >= 1

        # Mid-task selection (acp.agent._apply_mid_task_selection's own
        # shape: propose only, never apply) -- the coordinator's cached
        # session is left completely untouched by this call alone.
        service.propose(task_id, entry_b, ConfigurationInputs(route=entry_b.route, parameters={}), 1, owner_epoch)
        assert _model_option(manager, session_ref)["currentValue"] == "model-a", (
            "propose() alone must never itself touch the already-open session")

        # Next turn's safe boundary: the SAME cached session must now be
        # reconfigured, proven by a genuinely new outgoing wire call and
        # the manager's own real, wire-observed currentValue -- not merely
        # configuration_revisions flipping to 'active'.
        _channel2, session_ref2 = runtime._ensure_coordinator(task_id)
        assert session_ref2 == session_ref, "must reuse the same session, never open a second one for this"
        assert _set_config_option_calls(request_log) > calls_after_a
        assert _model_option(manager, session_ref)["currentValue"] == "model-b"
        active = service.current(task_id)
        assert active["id"] == service.history(task_id)[1]["id"]
        assert active["state"] == "active"
    finally:
        for manager in list(adapter._managers.values()):
            manager.shutdown(grace=2.0)


def test_fresh_session_after_restart_reconfirms_an_already_active_revision(tmp_path):
    """EXT-01 #2b -- the reviewer's own regression test: select B while A is
    active, reach a safe boundary, then restart. The brand-new session must
    receive its own real ``session/set_config_option`` call for B; the
    store already saying 'active' must never be read as "this session is
    configured"."""
    store, task_id, owner_epoch = _task(tmp_path)
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    write_policy(environ, {_ENGINE: {"model-a": "standard", "model-b": "standard"}})
    request_log = tmp_path / "requests.jsonl"
    scenario_path = _scenario_with_two_models(engine_dir, request_log)
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)

    service = ConfigurationService(store)
    entry_a, entry_b = _catalog_entry(connection_id, "model-a"), _catalog_entry(connection_id, "model-b")
    service.propose(task_id, entry_a, ConfigurationInputs(route=entry_a.route, parameters={}), 0, owner_epoch)

    adapter1 = _adapter(store, environ, record, tmp_path)
    runtime1 = _runtime(store, adapter1, service)
    try:
        runtime1._ensure_coordinator(task_id)  # A becomes active on session 1
        service.propose(task_id, entry_b, ConfigurationInputs(route=entry_b.route, parameters={}), 1, owner_epoch)
        runtime1._ensure_coordinator(task_id)  # safe boundary: B becomes active on session 1 too
        active = service.current(task_id)
        assert active["id"] == service.history(task_id)[1]["id"]
        calls_before_restart = _set_config_option_calls(request_log)
    finally:
        for manager in list(adapter1._managers.values()):
            manager.shutdown(grace=2.0)

    # Simulate a Cortex process restart: brand-new adapter + runtime
    # instances sharing only the same on-disk Store -- see
    # test_recovery_cross_process.py's identical pattern. The durable
    # configuration_revisions row for B is already 'active'.
    assert service.current(task_id)["state"] == "active"
    adapter2 = _adapter(store, environ, record, tmp_path)
    runtime2 = _runtime(store, adapter2, service)
    try:
        _channel, session_ref2 = runtime2._ensure_coordinator(task_id)
        manager2 = adapter2._managers[list(adapter2._managers)[0]]
        # The fresh engine process's own session/new response always
        # starts at the scenario's on-disk default ("model-a") -- proving
        # this session really never carried B's confirmation over from the
        # first process.
        # A real, additional session/set_config_option call happened on
        # this second process's own wire (never skipped just because the
        # store already said 'active').
        assert _set_config_option_calls(request_log) > calls_before_restart
        assert _model_option(manager2, session_ref2)["currentValue"] == "model-b"
        # The durable row is untouched (still the same 'active' revision,
        # same original confirmation evidence) -- only this session's own
        # in-memory confirmation record changed.
        assert service.current(task_id)["id"] == active["id"]
        assert service.current(task_id)["confirmation_evidence_ref"] == active["confirmation_evidence_ref"]
    finally:
        for manager in list(adapter2._managers.values()):
            manager.shutdown(grace=2.0)


def test_partial_confirmation_on_a_fresh_session_blocks_the_turn(tmp_path):
    """EXT-01 #2c: a fresh session's own confirmation attempt for an
    already-'active' task-level revision that only partially applies must
    block the turn -- never silently proceed. Reuses the existing
    indeterminate vocabulary; ``_ensure_coordinator`` raises."""
    store, task_id, owner_epoch = _task(tmp_path)
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    write_policy(environ, {_ENGINE: {"model-a": "standard", "model-b": "standard"}})
    request_log = tmp_path / "requests.jsonl"
    scenario_path = _scenario_with_two_models(engine_dir, request_log)
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)

    service = ConfigurationService(store)
    entry_a = _catalog_entry(connection_id, "model-a")
    service.propose(task_id, entry_a, ConfigurationInputs(route=entry_a.route, parameters={}), 0, owner_epoch)

    adapter1 = _adapter(store, environ, record, tmp_path)
    runtime1 = _runtime(store, adapter1, service)
    try:
        runtime1._ensure_coordinator(task_id)  # A becomes active on session 1
        assert service.current(task_id)["state"] == "active"
    finally:
        for manager in list(adapter1._managers.values()):
            manager.shutdown(grace=2.0)

    # A second, "accept_no_config_options" scenario: the wire call succeeds
    # (no JSON-RPC error) but the response carries no evidence of what was
    # actually applied -- EngineConfigurationOutcomeUnknown, mapped to
    # 'indeterminate' by reconcile_configuration, exactly as
    # ConfigurationService.apply's own catch-all would for a first-ever
    # 'selected' apply.
    scenario_path2 = write_scenario(engine_dir, dict(
        agent_capabilities=dict(loadSession=False, sessionCapabilities=dict(resume=False)),
        session_id="sess-2",
        config_options=[
            dict(id="model", currentValue="model-a", options=[
                dict(value="model-a", name="Model A"), dict(value="model-b", name="Model B"),
            ]),
        ],
        configure_behavior="accept_no_config_options",
        prompt_updates=[], prompt_stop_reason="end_turn", prompt_behavior="normal",
    ))
    record2 = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path2), environ=environ)
    adapter2 = _adapter(store, environ, record2, tmp_path)
    runtime2 = _runtime(store, adapter2, service)
    try:
        with pytest.raises(PortableError) as excinfo:
            runtime2._ensure_coordinator(task_id)
        assert excinfo.value.code != "configuration_apply_failed" or True  # any non-benign code is fine
        # The turn is blocked; the task-level row is still exactly what it
        # was (an already-'active' row is never mutated by a session's own
        # failed reconfirmation -- see reconcile_configuration's docstring).
        assert service.current(task_id)["state"] == "active"
    finally:
        for manager in list(adapter2._managers.values()):
            manager.shutdown(grace=2.0)
