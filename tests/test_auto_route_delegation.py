"""routing.auto_route end to end through the real ordinary
``delegation.provisioning.ProvisioningService.delegate`` path -- the same
fake-engine wire-evidence harness ``test_mixed_team_ordinary_delegation.py``
already proves T027's own mixed-team routing with, so this exercises the
real ``ProvisioningService``/``EngineSessionManagerAdapter`` wiring.
"""
from __future__ import annotations

import json
import time
from dataclasses import replace
from pathlib import Path

import pytest

from cortex_runtime.catalog import store as catalog_store
from cortex_runtime.catalog.discovery import build_catalog, parse_observation
from cortex_runtime.configuration.policy import PolicyCeiling
from cortex_runtime.configuration.resolution import ConfigurationInputs
from cortex_runtime.configuration.service import ConfigurationService
from cortex_runtime.connections import quota
from cortex_runtime.connections import registry as connection_registry
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


def _make_connection(tmp_path, environ, *, engine, model, request_log):
    """Like ``test_mixed_team_ordinary_delegation._make_connection``, plus
    an offered effort option, so the auto-chosen effort reaches the wire."""
    effort_option_id = "reasoning_effort" if engine == "codex-acp" else "effort"
    config_options = [
        dict(id="model", currentValue=model, options=[dict(value=model, name=model)]),
        dict(id="mode", currentValue=_MODE_BY_ENGINE[engine][0]["value"], options=_MODE_BY_ENGINE[engine]),
        dict(id=effort_option_id, currentValue="medium", options=[
            dict(value=v, name=v) for v in ("low", "medium", "high")
        ]),
    ]
    scenario = default_scenario(config_options=config_options, request_log_path=str(request_log))
    scenario_path = write_scenario(tmp_path, scenario)
    connection = make_connection_record(new_connection_id(), fake_launch_command(scenario_path), engine=engine, environ=environ)
    # A signed-in native account, so the connection is actually selectable.
    home = Path(connection.credential_ref.path)
    home.mkdir(parents=True, exist_ok=True)
    home.chmod(0o700)
    (home / ("auth.json" if engine == "codex-acp" else ".credentials.json")).write_text("{}")
    with connection_registry.transaction(environ) as state:
        state.connections[connection.id] = connection
    return connection


def _snapshot(connection, model):
    options = [
        {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": model,
         "options": [{"value": model, "name": model}]},
        {"id": "thought_level", "name": "Thought level", "category": "thought_level", "type": "select",
         "currentValue": "medium", "options": [{"value": v, "name": v} for v in ("low", "medium", "high")]},
    ]
    return build_catalog({connection.id: connection}, {connection.id: parse_observation(options)})


def _setup(tmp_path, *, observed=True):
    store, task_id, channel, epoch, project = _bootstrap(tmp_path)
    environ = isolated_environ(tmp_path)
    log_a, log_b = tmp_path / "a.jsonl", tmp_path / "b.jsonl"
    connection_a = _make_connection(tmp_path, environ, engine="codex-acp", model="coordinator-model", request_log=log_a)
    connection_b = _make_connection(tmp_path, environ, engine="claude-agent-acp", model="reviewer-model", request_log=log_b)
    snapshot_a = _snapshot(connection_a, "coordinator-model")
    if observed:
        catalog_store.save_snapshot(snapshot_a, environ)
        catalog_store.save_snapshot(_snapshot(connection_b, "reviewer-model"), environ)

    entry_a = snapshot_a.entries[0]
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
    return store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ


def _codex_used(connection, percent):
    day = Path(connection.credential_ref.path) / "sessions" / "2026" / "09" / "22"
    day.mkdir(parents=True, exist_ok=True)
    limits = {"primary": {"used_percent": percent, "resets_at": time.time() + 3600}}
    (day / "rollout.jsonl").write_text(json.dumps({"payload": {"rate_limits": limits}}) + "\n")


def _routing(project, overrides):
    (project / ".cortex").mkdir(exist_ok=True)
    (project / ".cortex" / "routing.json").write_text(json.dumps(overrides), encoding="utf-8")


def _effort_calls(log_path):
    return [
        entry for entry in _read_log(log_path)
        if entry["method"] == "session/set_config_option"
        and (entry.get("params") or {}).get("configId") in ("effort", "thought_level", "reasoning_effort")
    ]


def test_deep_work_moves_off_the_account_above_the_threshold(tmp_path):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _codex_used(connection_a, 90.0)
    result = provisioning.delegate(channel, "Audit", "Audit it.", "security-auditor", "Report.", "req-1")
    assert result.status == "ready"
    assert "session/new" in [entry["method"] for entry in _read_log(log_b)]
    assert _read_log(log_a) == []
    calls = _effort_calls(log_b)
    assert calls and all(call["params"].get("value") == "high" for call in calls)
    detail = routing_store.list_audit_for_attempt(store, result.attempt_id)[0]
    assert detail["connection_id"] == connection_b.id
    assert detail["decision_detail"]["route_basis"] == "auto"
    assert detail["decision_detail"]["effort"] == "high"
    adapter.terminate_owned(result.attempt_id)


def test_light_work_spends_the_account_above_the_threshold(tmp_path):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _codex_used(connection_a, 90.0)
    result = provisioning.delegate(channel, "Look", "Find it.", "explorer", "Found.", "req-1")
    assert result.status == "ready"
    assert "session/new" in [entry["method"] for entry in _read_log(log_a)]
    assert _read_log(log_b) == []
    assert _effort_calls(log_a)[-1]["params"]["value"] == "low"
    adapter.terminate_owned(result.attempt_id)


def test_claude_usage_reported_by_the_engine_steers_the_choice(tmp_path):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _codex_used(connection_a, 10.0)
    quota.observe(connection_b.id, {"status": "allowed_warning", "rateLimitType": "seven_day",
                                    "utilization": 0.95, "resetsAt": time.time() + 3600}, environ)
    result = provisioning.delegate(channel, "Debug", "Debug it.", "debugger", "Fixed.", "req-1")
    assert result.status == "ready"
    assert _read_log(log_a) and _read_log(log_b) == []
    adapter.terminate_owned(result.attempt_id)


def test_unobserved_catalogs_keep_the_coordinators_route(tmp_path):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(
        tmp_path, observed=False)
    result = provisioning.delegate(channel, "Do it", "Do the work.", "backend-dev", "Passes.", "req-1")
    assert result.status == "ready"
    assert "session/new" in [entry["method"] for entry in _read_log(log_a)]
    assert _read_log(log_b) == []
    adapter.terminate_owned(result.attempt_id)


def test_pin_chooses_the_route_and_effort(tmp_path):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _codex_used(connection_a, 99.0)
    _routing(project, {"pin": {"planner": "codex-acp/coordinator-model:high"}})
    result = provisioning.delegate(channel, "Plan", "Plan it.", "planner", "Planned.", "req-1")
    assert result.status == "ready"
    assert _effort_calls(log_a)[-1]["params"]["value"] == "high"
    detail = routing_store.list_audit_for_attempt(store, result.attempt_id)[0]["decision_detail"]
    assert detail["route_basis"] == "auto:pin"
    adapter.terminate_owned(result.attempt_id)


def test_an_unavailable_pin_is_refused_not_substituted(tmp_path):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _routing(project, {"pin": {"*": "codex-acp/gpt-9000"}})
    with pytest.raises(PortableError) as refused:
        provisioning.delegate(channel, "Plan", "Plan it.", "planner", "Planned.", "req-1")
    assert refused.value.code == "pinned_route_unavailable"
    assert _read_log(log_a) == [] and _read_log(log_b) == []


def test_denied_models_are_never_chosen(tmp_path):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _codex_used(connection_a, 90.0)
    _routing(project, {"deny": ["reviewer-model"]})
    result = provisioning.delegate(channel, "Audit", "Audit it.", "security-auditor", "Report.", "req-1")
    assert result.status == "ready"
    assert _read_log(log_a) and _read_log(log_b) == []
    adapter.terminate_owned(result.attempt_id)


def test_connection_without_launch_command_is_skipped(tmp_path):
    """A discovery placeholder must not shadow a launchable ACP connection."""
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _codex_used(connection_a, 90.0)
    placeholder_id = new_connection_id()
    placeholder = replace(
        connection_b, id=placeholder_id, name="discovery-placeholder", launch_command=None,
        credential_ref=replace(
            connection_b.credential_ref,
            path=str(connection_registry.connection_state_dir(placeholder_id, "claude-agent-acp", environ)),
        ),
    )
    catalog_store.save_snapshot(_snapshot(placeholder, "reviewer-model"), environ)
    with connection_registry.transaction(environ) as state:
        state.connections = {placeholder.id: placeholder, connection_a.id: connection_a}
    result = provisioning.delegate(channel, "Audit", "Audit it.", "security-auditor", "Report.", "req-1")
    assert result.status == "ready"
    assert _read_log(log_a)
    adapter.terminate_owned(result.attempt_id)


def test_an_explicit_task_route_profile_outranks_auto_routing(tmp_path):
    """A task carrying a T027 mixed-team profile already decided which
    route this role runs on; auto routing must not reopen it."""
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    launch = dict(
        assignment_id="as_1", role_id="worker-code-reviewer",
        resolved_connection_id=connection_a.id, resolved_model="coordinator-model",
    )
    adapter._resolve_runtime_route(launch, str(project))
    assert launch["resolved_connection_id"] == connection_a.id
    assert launch["resolved_model"] == "coordinator-model"
    assert "route_basis" not in launch
    assert not launch.get("parameters")


def test_an_effort_above_the_models_ceiling_is_lowered_not_refused(tmp_path):
    """A saved ultra (from before the rule) must not make a thread unusable."""
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _routing(project, {"pin": {"*": "codex-acp/coordinator-model:ultra"}})
    result = provisioning.delegate(channel, "Plan", "Plan it.", "planner", "Planned.", "req-1")
    assert result.status == "ready"
    assert _effort_calls(log_a)[-1]["params"]["value"] == "high"
    adapter.terminate_owned(result.attempt_id)
    from cortex_runtime.routing import auto_route

    allowed = auto_route.allowed_efforts("codex-acp", "coordinator-model", str(project), environ)
    assert "ultra" not in allowed and auto_route.nearest("ultra", allowed) == "high"


def test_a_signed_out_engine_is_never_chosen(tmp_path):
    """Claude signed out: every role runs on Codex, even a hot Codex."""
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _codex_used(connection_a, 95.0)
    (Path(connection_b.credential_ref.path) / ".credentials.json").unlink()
    result = provisioning.delegate(channel, "Audit", "Audit it.", "security-auditor", "Report.", "req-1")
    assert result.status == "ready"
    assert _read_log(log_a) and _read_log(log_b) == []
    adapter.terminate_owned(result.attempt_id)
