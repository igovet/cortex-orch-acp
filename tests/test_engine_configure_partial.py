"""F3 (T013 review): a multi-option configure() call must never let a
partial engine-side mutation be reported as a clean ConfigurationRejected.

Reporting ConfigurationRejected when an earlier option in the same
configure() call already succeeded would make ConfigurationService.apply()
mark the revision 'failed' -- falsely claiming nothing was applied while
the engine is actually left in a mixed state. This file exercises
EngineSessionManager.configure() directly against the fake engine (see
engine_fakes.py/fake_engine_process.py's opt-in
"configure_behavior_by_option"/"remove_options_on_apply" scenario fields,
added for this fix) and, separately, the full ConfigurationService.apply()
integration path with a thin real-adapter shim.
"""
from __future__ import annotations

import json

import pytest

from cortex_runtime.configuration.service import ConfigurationRejected
from cortex_runtime.engine.session_manager import EngineConfigurationPartiallyApplied, EngineSessionManager
from cortex_runtime.portable.engine_contract import EngineCapability
from cortex_runtime.portable.identity import ConfigurationRevision, ExecutionRoute  # noqa: F401 (ExecutionRoute used below)

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario

REQUESTED_CAPABILITIES = (EngineCapability.SESSION_CREATION, EngineCapability.CONFIGURATION_OPTIONS)


def _make_manager(tmp_path, scenario, *, call_timeout=5.0):
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    scenario_path = write_scenario(tmp_path, scenario)
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    return EngineSessionManager(record, str(tmp_path), environ=environ, call_timeout=call_timeout)


def _two_option_revision(model="gpt-x", effort="high"):
    route = ExecutionRoute(
        engine="codex-acp", connection_id="cn_" + "0" * 12, model=model,
        supported_parameters=(("thought_level", effort),),
    )
    return ConfigurationRevision(id="cr_" + "0" * 12, task_id="t_" + "0" * 12, revision=1, route=route)


def test_first_option_rejected_is_a_clean_configuration_rejected(tmp_path):
    """Nothing was ever applied -- still a clean ConfigurationRejected, not
    the partial-application exception."""
    scenario = default_scenario(
        config_options=[
            dict(id="model", currentValue="old", options=[dict(value="gpt-x", name="GPT X")]),
            dict(id="thought_level", currentValue="low", options=[dict(value="high", name="High")]),
        ],
        configure_behavior_by_option={"model": "error"},
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        with pytest.raises(ConfigurationRejected):
            manager.configure(session_id, _two_option_revision())
    finally:
        manager.shutdown(grace=2.0)


def test_model_accepted_then_effort_rejected_raises_partial_applied_not_rejected(tmp_path):
    scenario = default_scenario(
        config_options=[
            dict(id="model", currentValue="old", options=[dict(value="gpt-x", name="GPT X")]),
            dict(id="thought_level", currentValue="low", options=[dict(value="high", name="High")]),
        ],
        configure_behavior_by_option={"model": "accept", "thought_level": "error"},
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        with pytest.raises(EngineConfigurationPartiallyApplied):
            manager.configure(session_id, _two_option_revision())
        # And never mistaken for a clean rejection.
        with pytest.raises(EngineConfigurationPartiallyApplied):
            try:
                manager.configure(session_id, _two_option_revision())
            except ConfigurationRejected:  # pragma: no cover - must never happen
                raise AssertionError("a partial application must never surface as ConfigurationRejected")
    finally:
        manager.shutdown(grace=2.0)


def test_model_accepted_then_effort_mismatch_raises_partial_applied(tmp_path):
    scenario = default_scenario(
        config_options=[
            dict(id="model", currentValue="old", options=[dict(value="gpt-x", name="GPT X")]),
            dict(id="thought_level", currentValue="low", options=[dict(value="high", name="High")]),
        ],
        configure_behavior_by_option={"model": "accept", "thought_level": "mismatch"},
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        with pytest.raises(EngineConfigurationPartiallyApplied):
            manager.configure(session_id, _two_option_revision())
    finally:
        manager.shutdown(grace=2.0)


def test_effort_unsupported_after_model_change_is_never_sent(tmp_path):
    """codex-acp-like behavior: applying "model" narrows the engine's own
    subsequently-observed configOptions so "thought_level" is no longer
    offered at all. configure() must re-check against the *latest*
    observed options and never send the now-unsupported option -- verified
    against the fake engine's own request log, not just the exception."""
    request_log = tmp_path / "requests.jsonl"
    scenario = default_scenario(
        config_options=[
            dict(id="model", currentValue="old", options=[dict(value="gpt-x", name="GPT X")]),
            dict(id="thought_level", currentValue="low", options=[dict(value="high", name="High")]),
        ],
        configure_behavior_by_option={"model": "accept"},
        remove_options_on_apply={"model": ["thought_level"]},
        request_log_path=str(request_log),
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        with pytest.raises(EngineConfigurationPartiallyApplied):
            manager.configure(session_id, _two_option_revision())
    finally:
        manager.shutdown(grace=2.0)

    logged = [
        json.loads(line) for line in request_log.read_text().splitlines() if line
    ]
    set_config_calls = [entry for entry in logged if entry["method"] == "session/set_config_option"]
    # Exactly one wire call (for "model") -- "thought_level" was recognized
    # as no-longer-offered and never sent.
    assert len(set_config_calls) == 1


def test_single_option_success_is_unaffected(tmp_path):
    scenario = default_scenario(
        config_options=[dict(id="model", currentValue="old", options=[dict(value="gpt-x", name="GPT X")])],
        configure_behavior="accept",
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        route = ExecutionRoute(engine="codex-acp", connection_id="cn_" + "0" * 12, model="gpt-x")
        revision = ConfigurationRevision(id="cr_" + "0" * 12, task_id="t_" + "0" * 12, revision=1, route=route)
        evidence_ref = manager.configure(session_id, revision)
        assert "model=gpt-x" in evidence_ref
    finally:
        manager.shutdown(grace=2.0)


class _RealEngineAdapterShim:
    """Minimal in-test shim standing in for the not-yet-built production
    wiring between ``ConfigurationService``'s dict revision rows (see
    ``configuration.store._row_dict``, which carries no ``model`` field --
    only ``route_id``/``route_engine``/``route_connection_id``/
    ``parameters``) and ``EngineSessionManager.configure()``'s
    ``ConfigurationRevision``/``ExecutionRoute`` contract. A real caller
    would resolve ``route_id`` back to a full ``ExecutionRoute`` via a
    catalog lookup (see ``scripts/cortex_codex_probe.py``'s
    ``_resolve_route`` for exactly that pattern); this shim is handed the
    already-known ``ExecutionRoute`` directly since building that wiring is
    out of this task's scope. It only proves
    ``EngineSessionManager.configure()``'s own partial-application
    exception (F3, T013 review) propagates through
    ``ConfigurationService.apply()`` to ``indeterminate`` once such wiring
    exists."""

    def __init__(self, manager, route):
        self._manager = manager
        self._route = route

    def configure(self, engine_session_id, revision_row):
        revision = ConfigurationRevision(
            id=revision_row["id"], task_id=revision_row["task_id"],
            revision=revision_row["revision"], route=self._route,
        )
        return self._manager.configure(engine_session_id, revision)


def test_configuration_service_apply_marks_indeterminate_on_partial_application(tmp_path, monkeypatch):
    """Full integration: a real EngineSessionManager (behind the shim
    above) wired as the ConfigurationService adapter -- a
    partial-application configure() must leave the revision
    'indeterminate' (fresh-evidence reconciliation required), never a
    false 'failed'."""
    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.catalog.discovery import build_catalog, parse_observation
    from cortex_runtime.configuration.resolution import ConfigurationInputs
    from cortex_runtime.configuration.service import ConfigurationService
    from cortex_runtime.connections import registry as connection_registry
    from cortex_runtime.connections.model import AuthMode, ConnectionRecord, ConnectionState, EngineNativeDir
    from cortex_runtime.ownership import current_owner_epoch
    from cortex_runtime.portable.identity import OwnershipEpoch, SourceEvent
    from cortex_runtime.store import Store

    # A sibling of tmp_path, never an ancestor of it (matches
    # test_configuration_service.py's own _isolate helper): tmp_path itself
    # becomes a project root (gains a ".cortex" directly inside it) below,
    # and connection_registry rejects XDG roots nested under any directory
    # that contains a project marker -- so this explicit environ (passed
    # all the way down to make_connection_record/EngineSessionManager,
    # never isolated_environ(tmp_path)'s tmp_path-nested one) must use this
    # sibling, not tmp_path.
    xdg_root = tmp_path.parent / (tmp_path.name + "-xdg")
    config_home = xdg_root / "config"
    state_home = xdg_root / "state"
    home = xdg_root / "home"
    for path in (config_home, state_home, home):
        path.mkdir(parents=True)
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    monkeypatch.setenv("XDG_STATE_HOME", str(state_home))
    monkeypatch.setenv("HOME", str(home))

    environ = {
        "HOME": str(home), "XDG_CONFIG_HOME": str(config_home), "XDG_STATE_HOME": str(state_home),
        "PATH": __import__("os").environ.get("PATH", "/usr/bin:/bin"),
    }
    connection_id = new_connection_id()
    scenario = default_scenario(
        config_options=[
            dict(id="model", currentValue="old", options=[dict(value="gpt-x", name="GPT X")]),
            dict(id="thought_level", currentValue="low", options=[dict(value="high", name="High")]),
        ],
        configure_behavior_by_option={"model": "accept", "thought_level": "error"},
    )
    scenario_path = write_scenario(tmp_path, scenario)
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    with connection_registry.transaction() as state:
        state.connections[record.id] = record

    manager = EngineSessionManager(record, str(tmp_path), environ=environ)
    now = "2026-01-01T00:00:00+00:00"
    model_record = ConnectionRecord(
        id=connection_id, name="fake", engine="codex-acp", provider="fake", auth_mode=AuthMode.NATIVE_ACCOUNT,
        key_kind=None, endpoint=None, launch_command=None,
        credential_ref=EngineNativeDir(path=f"/tmp/{connection_id}"), approved_fingerprint="fp",
        state=ConnectionState.CONFIGURED, qualified=True, created_at=now, updated_at=now,
    )
    options = [
        dict(id="model", name="Model", category="model", type="select", currentValue="gpt-x",
             options=[dict(value="gpt-x", name="gpt-x")]),
        dict(id="thought_level", name="Effort", category="thought_level", type="select", currentValue="high",
             options=[dict(value="high", name="high")]),
    ]
    snapshot = build_catalog({connection_id: model_record}, {connection_id: parse_observation(options)})
    entry = snapshot.entries[0]

    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id="ev-" + "0" * 12), "Do work.", "owner-1")
    with store.connection() as db:
        epoch_value = current_owner_epoch(db, result.task_id)
    epoch = OwnershipEpoch(epoch=epoch_value, owner="owner-1")

    service = ConfigurationService(store)
    inputs = ConfigurationInputs(route=entry.route, parameters={"thought_level": "high"})
    row = service.propose(result.task_id, entry, inputs, expected_revision=0, current_epoch=epoch)

    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session(result.task_id, entry.route.id)
        adapter = _RealEngineAdapterShim(manager, entry.route)
        outcome = service.apply(result.task_id, row["id"], adapter, session_id, epoch)
        assert outcome["state"] == "indeterminate"
        assert outcome["error_code"] == "apply_exception"
    finally:
        manager.shutdown(grace=2.0)
