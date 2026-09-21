"""D-T035-04: the coordinator's own explicit (or policy-defaulted) route
choice for one delegation, end to end through the real ordinary
``delegation.provisioning.ProvisioningService.delegate`` path -- the same
fake-engine wire-evidence harness ``test_mixed_team_ordinary_delegation.py``
already proves T027's own mixed-team routing with, reused here (via direct
import, an established pattern in this suite -- see e.g.
``test_orchestrating_runtime_routing_fallback.py``) so this exercises the
real ``ProvisioningService``/``EngineSessionManagerAdapter``/route-policy
wiring, never a hand-rolled substitute.
"""
from __future__ import annotations

import json
from dataclasses import replace
from pathlib import Path

import pytest

from cortex_runtime.catalog.discovery import build_catalog, parse_observation
from cortex_runtime.configuration.policy import PolicyCeiling
from cortex_runtime.configuration.resolution import ConfigurationInputs
from cortex_runtime.configuration.service import ConfigurationService
from cortex_runtime.connections import registry as connection_registry
from cortex_runtime.delegation import capacity
from cortex_runtime.delegation.live_adapters import (
    ConfigurationServiceSnapshotSource,
    EngineSessionManagerAdapter,
    SharedProjectRootWorkspaceProvisioner,
)
from cortex_runtime.delegation.provisioning import ProvisioningService
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.routing import store as routing_store

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario
from test_mixed_team_ordinary_delegation import _bootstrap, _FakeConfigAdapter, _MODE_BY_ENGINE, _read_log


def _write_policy(project, *, roles=None, default=None):
    policy = {"roles": roles or {}, "default": default}
    (project / ".cortex").mkdir(exist_ok=True)
    (project / ".cortex" / "route-policy.json").write_text(json.dumps(policy), encoding="utf-8")


def _make_connection(tmp_path, environ, *, engine, model, request_log):
    """Like ``test_mixed_team_ordinary_delegation._make_connection``, plus
    an offered ``effort`` config option -- needed here (unlike that
    module's own tests) so ``_apply_worker_mode_floor``'s effort
    application actually has an option to apply this feature's own
    requested/policy-resolved effort onto."""
    effort_option_id = "reasoning_effort" if engine == "codex-acp" else "effort"
    config_options = [
        dict(id="model", currentValue=model, options=[dict(value=model, name=model)]),
        dict(id="mode", currentValue=_MODE_BY_ENGINE[engine][0]["value"], options=_MODE_BY_ENGINE[engine]),
        dict(id=effort_option_id, currentValue="medium", options=[
            dict(value="medium", name="Medium"), dict(value="high", name="High"),
        ]),
    ]
    scenario = default_scenario(config_options=config_options, request_log_path=str(request_log))
    scenario_path = write_scenario(tmp_path, scenario)
    connection = make_connection_record(new_connection_id(), fake_launch_command(scenario_path), engine=engine, environ=environ)
    with connection_registry.transaction(environ) as state:
        state.connections[connection.id] = connection
    return connection


def _entry(connection, model):
    options = [{"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": model,
                "options": [{"value": model, "name": model}]}]
    snapshot = build_catalog({connection.id: connection}, {connection.id: parse_observation(options)})
    return snapshot.entries[0]


def _setup(tmp_path, *, roles=None, default=None):
    store, task_id, channel, epoch, project = _bootstrap(tmp_path)
    environ = isolated_environ(tmp_path)
    log_a, log_b = tmp_path / "a.jsonl", tmp_path / "b.jsonl"
    connection_a = _make_connection(tmp_path, environ, engine="codex-acp", model="coordinator-model", request_log=log_a)
    connection_b = _make_connection(
        tmp_path, environ, engine="claude-agent-acp", model="reviewer-model", request_log=log_b,
    )
    _write_policy(project, roles=roles, default=default)

    entry_a = _entry(connection_a, "coordinator-model")
    service = ConfigurationService(store)
    row = service.propose(
        task_id, entry_a, ConfigurationInputs(route=entry_a.route, parameters={}),
        expected_revision=0, current_epoch=epoch,
        user_layer=dict(defaults={}, ceiling=PolicyCeiling(allowed_routes={entry_a.route.id})),
    )
    service.apply(task_id, row["id"], _FakeConfigAdapter(), "es_" + "a" * 12, epoch)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection_a, lambda t: str(project), environ=environ, call_timeout=10.0, store=store,
    )
    provisioning = ProvisioningService(
        store, SharedProjectRootWorkspaceProvisioner(), adapter, ConfigurationServiceSnapshotSource(service),
    )
    return store, task_id, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b


def _effort_calls(log_path):
    return [
        entry for entry in _read_log(log_path)
        if entry["method"] == "session/set_config_option"
        and (entry.get("params") or {}).get("configId") in ("effort", "thought_level", "reasoning_effort")
    ]


def test_explicit_engine_model_effort_opens_the_requested_route(tmp_path):
    """The coordinator's own explicit choice on ``delegate`` -- never the
    coordinator's own engine -- decides which connection this worker opens
    on, and the requested effort reaches the wire."""
    store, task_id, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b = _setup(
        tmp_path,
        roles={"worker-code-reviewer": [
            {"engine": "claude-agent-acp", "model": "reviewer-model", "efforts": ["medium", "high"], "default_effort": "high"},
        ]},
    )
    result = provisioning.delegate(
        channel, "Review", "Review it.", "code-reviewer", "Approve.", "req-1",
        engine="claude-agent-acp", model="reviewer-model", effort="medium",
    )
    assert result.status == "ready"
    assert "session/new" in [entry["method"] for entry in _read_log(log_b)]
    assert _read_log(log_a) == []
    calls = _effort_calls(log_b)
    assert calls and all(call["params"].get("value") == "medium" for call in calls)
    audit = routing_store.list_audit_for_attempt(store, result.attempt_id)
    assert audit[0]["connection_id"] == connection_b.id
    adapter.terminate_owned(result.attempt_id)


def test_explicit_engine_not_authorized_for_role_is_refused_naming_what_is_allowed(tmp_path):
    """A refusal never silently substitutes a different engine/model --
    and reuses ``route_policy.resolve_effort``'s own message, naming what
    IS allowed for this role."""
    store, task_id, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b = _setup(
        tmp_path,
        roles={"worker-code-reviewer": [
            {"engine": "codex-acp", "model": "coordinator-model", "efforts": ["*"], "default_effort": None},
        ]},
    )
    with pytest.raises(PortableError) as excinfo:
        provisioning.delegate(
            channel, "Review", "Review it.", "code-reviewer", "Approve.", "req-1",
            engine="claude-agent-acp", model="reviewer-model", effort="medium",
        )
    assert excinfo.value.code == "route_policy_not_authorized"
    assert "codex-acp" in excinfo.value.remediation
    # Refused before any process for either connection was ever launched.
    assert _read_log(log_a) == []
    assert _read_log(log_b) == []
    assert capacity.count_active(store, connection_a.id) == 0
    assert capacity.count_active(store, connection_b.id) == 0


def test_explicit_engine_with_no_connection_is_refused_not_substituted(tmp_path):
    store, task_id, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b = _setup(tmp_path)
    with pytest.raises(PortableError) as excinfo:
        provisioning.delegate(
            channel, "Do it", "Do the work.", "backend-dev", "Passes.", "req-1",
            engine="gpt-9000-acp",
        )
    assert excinfo.value.code == "engine_not_connected"
    assert _read_log(log_a) == []
    assert _read_log(log_b) == []


def test_omitted_engine_falls_back_to_the_roles_first_allowed_policy_entry(tmp_path):
    """No explicit choice on ``delegate`` -- the runtime still dispatches
    this role on its policy's first listed route (here, deliberately NOT
    the coordinator's own engine), never silently inheriting the
    coordinator's own connection."""
    store, task_id, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b = _setup(
        tmp_path,
        roles={"worker-code-reviewer": [
            {"engine": "claude-agent-acp", "model": "reviewer-model", "efforts": ["medium", "high"], "default_effort": "high"},
        ]},
    )
    result = provisioning.delegate(channel, "Review", "Review it.", "code-reviewer", "Approve.", "req-1")
    assert result.status == "ready"
    assert "session/new" in [entry["method"] for entry in _read_log(log_b)]
    assert _read_log(log_a) == []
    calls = _effort_calls(log_b)
    assert calls and all(call["params"].get("value") == "high" for call in calls)
    adapter.terminate_owned(result.attempt_id)


def test_omitted_engine_does_not_discard_disallowed_requested_effort(tmp_path):
    """A qualification policy blocks planner low before a paid worker launch."""
    store, task_id, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b = _setup(
        tmp_path,
        default=[
            {"engine": "codex-acp", "model": "coordinator-model", "efforts": ["medium", "high"], "default_effort": "high"},
        ],
    )
    with pytest.raises(PortableError) as excinfo:
        provisioning.delegate(
            channel, "Plan", "Plan the work.", "planner", "Plan is source-backed.", "req-low",
            effort="low",
        )
    assert excinfo.value.code == "route_policy_not_authorized"
    assert _read_log(log_a) == []
    assert _read_log(log_b) == []
    result = provisioning.delegate(
        channel, "Plan", "Plan the work.", "planner", "Plan is source-backed.", "req-high",
    )
    assert result.status == "ready"
    assert _effort_calls(log_a)[-1]["params"]["value"] == "high"
    assert _read_log(log_b) == []
    adapter.terminate_owned(result.attempt_id)


def test_explicit_engine_without_model_binds_project_policy_model(tmp_path):
    store, task_id, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b = _setup(
        tmp_path,
        default=[
            {"engine": "codex-acp", "model": "coordinator-model", "efforts": ["medium", "high"], "default_effort": "high"},
        ],
    )
    result = provisioning.delegate(
        channel, "Plan", "Plan the work.", "planner", "Plan is source-backed.", "req-engine-only",
        engine="codex-acp",
    )
    assert result.status == "ready"
    assert _effort_calls(log_a)[-1]["params"]["value"] == "high"
    audit = routing_store.list_audit_for_attempt(store, result.attempt_id)
    assert audit[0]["model"] == "coordinator-model"
    adapter.terminate_owned(result.attempt_id)


def test_disposable_qualification_policy_rejects_observed_terra_low_worker_before_launch(tmp_path):
    store, task_id, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b = _setup(tmp_path)
    fixture = Path(__file__).resolve().parents[1] / "policy" / "route-policy.qualification-luna.json"
    (tmp_path / "project" / ".cortex" / "route-policy.json").write_bytes(fixture.read_bytes())
    with pytest.raises(PortableError) as excinfo:
        provisioning.delegate(
            channel, "Plan", "Plan the work.", "planner", "Plan is source-backed.", "req-terra-low",
            engine="codex-acp", model="gpt-5.6-terra", effort="low",
        )
    assert excinfo.value.code == "route_policy_not_authorized"
    assert _read_log(log_a) == []
    assert _read_log(log_b) == []


def test_omitted_engine_with_no_policy_entry_for_role_keeps_current_behaviour(tmp_path):
    """No route policy entries at all for this role (and no 'default'
    bucket) -- unchanged: dispatches on the coordinator's own connection,
    exactly as before this feature."""
    store, task_id, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b = _setup(tmp_path)
    result = provisioning.delegate(channel, "Do it", "Do the work.", "backend-dev", "Passes.", "req-1")
    assert result.status == "ready"
    assert "session/new" in [entry["method"] for entry in _read_log(log_a)]
    assert _read_log(log_b) == []
    adapter.terminate_owned(result.attempt_id)


def test_policy_route_skips_engine_connection_without_launch_command(tmp_path):
    """A discovery placeholder must not shadow a launchable ACP connection."""
    store, task_id, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b = _setup(
        tmp_path,
        default=[
            {"engine": "codex-acp", "model": "coordinator-model", "efforts": ["*"], "default_effort": None},
        ],
    )
    placeholder_id = new_connection_id()
    placeholder = replace(
        connection_a,
        id=placeholder_id,
        name="discovery-placeholder",
        launch_command=None,
        credential_ref=replace(
            connection_a.credential_ref,
            path=str(connection_registry.connection_state_dir(placeholder_id, "codex-acp", isolated_environ(tmp_path))),
        ),
    )
    with connection_registry.transaction(isolated_environ(tmp_path)) as state:
        state.connections = {placeholder.id: placeholder, **state.connections}

    result = provisioning.delegate(channel, "Review", "Review it.", "code-reviewer", "Approve.", "req-placeholder")
    assert result.status == "ready"
    assert _read_log(log_a)
    adapter.terminate_owned(result.attempt_id)


def test_omitted_policy_route_skips_unconfigured_engine(tmp_path):
    """An allowed engine that is absent from this isolated runtime is not
    advertised or selected ahead of the connected engine."""
    store, task_id, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b = _setup(
        tmp_path,
        roles={"worker-code-reviewer": [
            {"engine": "claude-agent-acp", "model": "reviewer-model", "efforts": ["medium"], "default_effort": "medium"},
            {"engine": "codex-acp", "model": "coordinator-model", "efforts": ["*"], "default_effort": None},
        ]},
    )
    with connection_registry.transaction(isolated_environ(tmp_path)) as state:
        state.connections.pop(connection_b.id)

    result = provisioning.delegate(channel, "Review", "Review it.", "code-reviewer", "Approve.", "req-unconfigured")
    assert result.status == "ready"
    assert _read_log(log_a)
    assert _read_log(log_b) == []
    adapter.terminate_owned(result.attempt_id)


def test_policy_route_preserves_task_selected_account_for_same_engine(tmp_path):
    """An alternate configured account must not shadow the task route."""
    store, task_id, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b = _setup(
        tmp_path,
        default=[
            {"engine": "codex-acp", "model": "coordinator-model", "efforts": ["*"], "default_effort": None},
        ],
    )
    alternate_log = tmp_path / "alternate.jsonl"
    alternate = _make_connection(
        tmp_path, isolated_environ(tmp_path), engine="codex-acp", model="coordinator-model",
        request_log=alternate_log,
    )
    with connection_registry.transaction(isolated_environ(tmp_path)) as state:
        state.connections = {alternate.id: alternate, connection_a.id: connection_a, connection_b.id: connection_b}

    result = provisioning.delegate(channel, "Review", "Review it.", "code-reviewer", "Approve.", "req-account")
    assert result.status == "ready"
    assert _read_log(log_a)
    assert _read_log(alternate_log) == []
    adapter.terminate_owned(result.attempt_id)


def test_an_explicit_task_route_profile_outranks_the_coordinators_choice(tmp_path):
    """A task carrying a T027 mixed-team profile already decided which
    route this role runs on. That decision is per-task and deliberate; the
    deployment-wide policy file narrows what a role may use, it does not
    reopen it -- so a coordinator's ``engine=`` on such a task must not
    quietly move the worker somewhere else."""
    store, task_id, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b = _setup(
        tmp_path,
        roles={"worker-code-reviewer": [
            {"engine": "claude-agent-acp", "model": "reviewer-model", "efforts": ["*"], "default_effort": None},
        ]},
    )
    # What the mixed-team profile resolved for this role, as provisioning
    # hands it to open().
    launch = dict(
        assignment_id="as_1", role_id="worker-code-reviewer",
        resolved_connection_id=connection_a.id, resolved_model="coordinator-model",
        requested_engine="claude-agent-acp", requested_model="reviewer-model", requested_effort="high",
    )
    adapter._resolve_requested_route(launch, str(tmp_path / "project"))

    assert launch["resolved_connection_id"] == connection_a.id
    assert launch["resolved_model"] == "coordinator-model"
    assert "route_basis" not in launch
    assert not launch.get("parameters")


def test_the_audit_says_the_route_policy_is_what_authorized_this_route(tmp_path):
    """Absent an explicit mixed-team profile, this task's own route
    authorization knows only the coordinator's route -- a policy-chosen
    route has no id under it to check at all. The attempt's audit row
    therefore has to name the authority that did permit it, so a reader
    can tell a granted permission from a check that passed vacuously."""
    store, task_id, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b = _setup(
        tmp_path,
        roles={"worker-code-reviewer": [
            {"engine": "claude-agent-acp", "model": "reviewer-model", "efforts": ["medium"], "default_effort": "medium"},
        ]},
    )
    result = provisioning.delegate(
        channel, "Review", "Review it.", "code-reviewer", "Approve.", "req-basis",
        engine="claude-agent-acp", model="reviewer-model", effort="medium",
    )
    assert result.status == "ready"
    audit = routing_store.list_audit_for_attempt(store, result.attempt_id)
    detail = audit[0]["decision_detail"]
    assert detail["route_basis"] == "route_policy"
    # The effort this attempt ran with exists nowhere else in the store,
    # and the worker card's header is asked to name it.
    assert detail["effort"] == "medium"
    adapter.terminate_owned(result.attempt_id)
