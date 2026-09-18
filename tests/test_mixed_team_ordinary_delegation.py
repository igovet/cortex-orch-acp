"""EXT-01 carried item: mixed-team delegation proven through the *ordinary*
``delegation.provisioning.ProvisioningService.delegate`` path -- never only
through ``EngineSessionManagerAdapter.attempt_fallback`` -- with one
resolved route per attempt threaded through engine open, dispatch
authorization, worker-mode confirmation, and audit. See
``.cortex/acp-migration/reviews/EXT-01-independent-production-chain-review.md``,
"Смешанная команда пока требует проверки именно через обычное делегирование".

Uses the real ``ProvisioningService``, the real ``EngineSessionManagerAdapter``,
a real ``ConfigurationService`` revision, and two real fake-engine
subprocesses (never a paid/real codex-acp or claude-agent-acp process) --
the same machinery ``test_delegation_routing_dispatch.py``/
``test_delegation_worker_mode_floor.py`` already exercise. Assertions read
each connection's own ``request_log_path`` (the actual outgoing wire
requests the fake engine subprocess received), never only the
``attempt_route_audit``/``delegation_provisioning`` database rows.
"""
from __future__ import annotations

import json
import uuid

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.catalog.discovery import build_catalog, parse_observation
from cortex_runtime.configuration.policy import PolicyCeiling, RouteProfile
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
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import OwnershipEpoch, SourceEvent
from cortex_runtime.portable.roles import resolve_role
from cortex_runtime.routing import store as routing_store
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario

REVIEWER_ROLE_ID = resolve_role("code-reviewer")


class _FakeConfigAdapter:
    def configure(self, engine_session_id, revision):
        return "evidence-ref"


def _bootstrap(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Do work.", "owner-1")
    channel = resolve_channel(store, result.channel_id, result.secret)
    with store.connection() as db:
        epoch = current_owner_epoch(db, result.task_id)
    return store, result.task_id, channel, OwnershipEpoch(epoch=epoch, owner="owner-1"), project


_MODE_BY_ENGINE = {
    "codex-acp": [dict(value="read-only", name="Ask"), dict(value="agent", name="Approve")],
    "claude-agent-acp": [dict(value="plan", name="Plan"), dict(value="default", name="Manual")],
}


def _make_connection(tmp_path, environ, *, engine, model, request_log):
    config_options = [
        dict(id="model", currentValue=model, options=[dict(value=model, name=model)]),
        dict(id="mode", currentValue=_MODE_BY_ENGINE[engine][0]["value"], options=_MODE_BY_ENGINE[engine]),
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


def _setup_mixed_team(tmp_path, *, allow_role_alternate: bool):
    store, task_id, channel, epoch, project = _bootstrap(tmp_path)
    environ = isolated_environ(tmp_path)
    log_a, log_b = tmp_path / "requests-a.jsonl", tmp_path / "requests-b.jsonl"
    connection_a = _make_connection(tmp_path, environ, engine="codex-acp", model="coordinator-model", request_log=log_a)
    connection_b = _make_connection(tmp_path, environ, engine="claude-agent-acp", model="reviewer-model", request_log=log_b)
    entry_a, entry_b = _entry(connection_a, "coordinator-model"), _entry(connection_b, "reviewer-model")

    role_routes = {REVIEWER_ROLE_ID: {entry_b.route.id}} if allow_role_alternate else {REVIEWER_ROLE_ID: {entry_a.route.id}}
    ceiling = PolicyCeiling(allowed_routes={entry_a.route.id, entry_b.route.id})
    service = ConfigurationService(store)
    row = service.propose(
        task_id, entry_a, ConfigurationInputs(route=entry_a.route, parameters={}),
        expected_revision=0, current_epoch=epoch, user_layer=dict(defaults={}, ceiling=ceiling),
        route_profile=RouteProfile(role_routes=role_routes),
        route_details={entry_b.route.id: entry_b.route},
    )
    service.apply(task_id, row["id"], _FakeConfigAdapter(), "es_" + "a" * 12, epoch)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection_a, lambda t: str(project), environ=environ, call_timeout=10.0, store=store,
    )
    provisioning = ProvisioningService(
        store, SharedProjectRootWorkspaceProvisioner(), adapter, ConfigurationServiceSnapshotSource(service),
    )
    return store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b


def _read_log(path):
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


def test_ordinary_delegate_opens_allowed_role_on_a_different_engine_with_target_model(tmp_path):
    """The REQUIRED test: a coordinator on codex-acp delegates an ordinary
    (non-fallback) assignment to a role authorized (T027 stored route
    profile) for a *different* connection/engine (claude-agent-acp) --
    through ``ProvisioningService.delegate`` alone, never
    ``attempt_fallback``. The outgoing wire (each connection's own
    ``request_log_path``) shows the worker's engine session, and its
    worker-mode-floor confirm call's own model, landed on the target
    connection -- never the coordinator's."""
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b = _setup_mixed_team(
        tmp_path, allow_role_alternate=True,
    )
    result = provisioning.delegate(channel, "Review the change", "Review it.", "code-reviewer", "Approve or request changes.", "req-1")
    assert result.status == "ready"
    assert result.engine_session_ref.startswith("es_")

    # Wire evidence, not DB rows: connection B (the target) actually
    # received this attempt's session/new and its worker-mode-floor
    # session/set_config_option -- carrying B's own model, never A's.
    requests_b = _read_log(log_b)
    methods_b = [entry["method"] for entry in requests_b]
    assert "session/new" in methods_b
    model_calls_b = [
        entry for entry in requests_b
        if entry["method"] == "session/set_config_option" and (entry.get("params") or {}).get("configId") == "model"
    ]
    assert model_calls_b, "the target connection never received a model configuration call"
    assert all((call["params"].get("value")) == "reviewer-model" for call in model_calls_b)

    # Connection A (the coordinator's own route) was never touched by this
    # delegation at all -- no process of its own was ever launched for it.
    assert _read_log(log_a) == []
    assert capacity.count_active(store, connection_a.id) == 0
    assert capacity.count_active(store, connection_b.id) == 1

    audit = routing_store.list_audit_for_attempt(store, result.attempt_id)
    assert len(audit) == 1
    assert audit[0]["connection_id"] == connection_b.id
    assert audit[0]["engine"] == "claude-agent-acp"
    assert audit[0]["model"] == "reviewer-model"
    assert audit[0]["decision"] == "authorized"  # never 'fallback' -- this is the ordinary path

    adapter.terminate_owned(result.attempt_id)


def test_role_without_an_alternate_route_still_gets_the_coordinators_own(tmp_path):
    """The role profile names the reviewer only for the coordinator's own
    route (no alternate) -- the ordinary path must dispatch on the
    coordinator's connection, exactly as before this fix."""
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b = _setup_mixed_team(
        tmp_path, allow_role_alternate=False,
    )
    result = provisioning.delegate(channel, "Review the change", "Review it.", "code-reviewer", "Approve or request changes.", "req-1")
    assert result.status == "ready"
    assert "session/new" in [entry["method"] for entry in _read_log(log_a)]
    assert _read_log(log_b) == []
    audit = routing_store.list_audit_for_attempt(store, result.attempt_id)
    assert audit[0]["connection_id"] == connection_a.id
    adapter.terminate_owned(result.attempt_id)


def test_role_absent_from_mixed_team_profile_is_still_denied_by_default(tmp_path):
    """A named role absent from an explicit mixed-team profile is denied
    outright (deny-by-default), never silently inheriting the coordinator's
    route through the generic bucket -- unaffected by this fix's own route
    resolution, still enforced by ``open``'s pre-dispatch authorization."""
    store, task_id, channel, epoch, project = _bootstrap(tmp_path)
    environ = isolated_environ(tmp_path)
    connection_a = _make_connection(tmp_path, environ, engine="codex-acp", model="coordinator-model", request_log=tmp_path / "a.jsonl")
    connection_b = _make_connection(tmp_path, environ, engine="claude-agent-acp", model="reviewer-model", request_log=tmp_path / "b.jsonl")
    entry_a, entry_b = _entry(connection_a, "coordinator-model"), _entry(connection_b, "reviewer-model")

    ceiling = PolicyCeiling(allowed_routes={entry_a.route.id, entry_b.route.id})
    service = ConfigurationService(store)
    # Explicit mixed-team profile naming a DIFFERENT role ("backend-dev")
    # only -- "code-reviewer" is absent, so it must be denied outright.
    row = service.propose(
        task_id, entry_a, ConfigurationInputs(route=entry_a.route, parameters={}),
        expected_revision=0, current_epoch=epoch, user_layer=dict(defaults={}, ceiling=ceiling),
        route_profile=RouteProfile(role_routes={resolve_role("backend-dev"): {entry_b.route.id}}),
        route_details={entry_b.route.id: entry_b.route},
    )
    service.apply(task_id, row["id"], _FakeConfigAdapter(), "es_" + "a" * 12, epoch)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection_a, lambda t: str(project), environ=environ, call_timeout=10.0, store=store,
    )
    provisioning = ProvisioningService(
        store, SharedProjectRootWorkspaceProvisioner(), adapter, ConfigurationServiceSnapshotSource(service),
    )
    with pytest.raises(PortableError) as excinfo:
        provisioning.delegate(channel, "Review the change", "Review it.", "code-reviewer", "Approve or request changes.", "req-1")
    assert excinfo.value.code == "role_route_not_authorized"
    assert capacity.count_active(store, connection_a.id) == 0
    assert capacity.count_active(store, connection_b.id) == 0
    assert routing_store.list_audit_for_task(store, task_id) == ()


def test_fallback_applies_the_targets_model_not_the_sources(tmp_path):
    """EXT-01: ``attempt_fallback`` must configure the worker-mode-floor
    confirm call with the TARGET route's own model, never the source
    (coordinator's) ``config_revision``'s stored model -- reproduced first
    against the unfixed behaviour: before this fix, ``_apply_worker_mode
    _floor`` re-read ``route_engine``/model from ``config_revision`` (the
    source) instead of the connection this fallback attempt actually
    opened against."""
    store, task_id, channel, epoch, project = _bootstrap(tmp_path)
    environ = isolated_environ(tmp_path)
    log_a, log_b = tmp_path / "a.jsonl", tmp_path / "b.jsonl"
    connection_a = _make_connection(tmp_path, environ, engine="codex-acp", model="source-model", request_log=log_a)
    connection_b = _make_connection(tmp_path, environ, engine="codex-acp", model="target-model", request_log=log_b)
    entry_a, entry_b = _entry(connection_a, "source-model"), _entry(connection_b, "target-model")

    from cortex_runtime.configuration.policy import CAPACITY_UNAVAILABLE, FallbackPolicy

    ceiling = PolicyCeiling(
        allowed_routes={entry_a.route.id, entry_b.route.id},
        fallback_policy=FallbackPolicy(
            allowed_target_routes={entry_b.route.id}, allowed_failure_classes={CAPACITY_UNAVAILABLE},
            data_recipient_consent=True,
        ),
    )
    service = ConfigurationService(store)
    row = service.propose(
        task_id, entry_a, ConfigurationInputs(route=entry_a.route, parameters={}),
        expected_revision=0, current_epoch=epoch, user_layer=dict(defaults={}, ceiling=ceiling),
        route_profile=RouteProfile(role_routes={"worker": {entry_a.route.id, entry_b.route.id}}),
        route_details={entry_b.route.id: entry_b.route},
    )
    service.apply(task_id, row["id"], _FakeConfigAdapter(), "es_" + "a" * 12, epoch)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection_a, lambda t: str(project), environ=environ, call_timeout=10.0, store=store,
    )
    launch = dict(task_id=task_id, assignment_id="asg1", config_revision=row["id"])
    old_ref = adapter.open("shared_project_root", dict(launch, attempt_id="xa_w1"), "instructions")
    assert old_ref.startswith("es_")

    new_ref = adapter.attempt_fallback(
        old_attempt_id="xa_w1", new_attempt_id="xa_w1fb", task_id=task_id, role="worker",
        failure_class=CAPACITY_UNAVAILABLE, target_connection_id=connection_b.id,
        target_route_id=entry_b.route.id, workspace_binding_id="shared_project_root",
        launch=launch, instructions="instructions", target_model="target-model",
    )
    assert new_ref.startswith("es_")

    requests_b = _read_log(log_b)
    model_calls_b = [
        entry for entry in requests_b
        if entry["method"] == "session/set_config_option" and (entry.get("params") or {}).get("configId") == "model"
    ]
    assert model_calls_b, "the target connection never received a model configuration call"
    # The fix under test: every model value the fallback attempt's own
    # worker-mode-floor confirm call sent to the TARGET connection is that
    # target's own model -- never "source-model" (the source
    # config_revision's stored route_model, the pre-fix bug).
    assert all(call["params"].get("value") == "target-model" for call in model_calls_b)
    assert all(call["params"].get("value") != "source-model" for call in model_calls_b)

    fb_audit = routing_store.list_audit_for_attempt(store, "xa_w1fb")
    assert fb_audit[0]["model"] == "target-model"

    adapter.terminate_owned("xa_w1")
    adapter.terminate_owned("xa_w1fb")
