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
from model_policy_fixtures import routing_fixture

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario
from test_mixed_team_ordinary_delegation import _bootstrap, _FakeConfigAdapter, _MODE_BY_ENGINE, _read_log


@pytest.fixture(autouse=True)
def _claude_usage_cli_unavailable(monkeypatch):
    """This module's fake claude-agent-acp connections have a real,
    on-disk credential_ref directory (see ``_make_connection``), so
    ``connections.quota``'s lazy ``/usage`` CLI refresh would otherwise try
    to spawn one; these tests exercise routing purely through
    ``quota.observe``/``rate_limit_event``, so the CLI itself is simulated
    as not installed for this connection (a realistic outcome, and the
    same one production sees when the CLI is absent)."""
    def not_installed(argv, env, cwd, timeout):
        raise FileNotFoundError("claude CLI not installed for this fake connection")
    monkeypatch.setattr(quota, "_run_claude_usage_subprocess", not_installed)


def _make_connection(tmp_path, environ, *, engine, model, request_log, mode=True, efforts=("low", "medium", "high"), **scenario):
    """Like ``test_mixed_team_ordinary_delegation._make_connection``, plus
    an offered effort option, so the auto-chosen effort reaches the wire.
    ``mode=False`` offers no mode option (the worker's mode can never be
    confirmed); ``scenario`` overrides the fake engine's own behaviour."""
    effort_option_id = "reasoning_effort" if engine == "codex-acp" else "effort"
    config_options = [
        dict(id="model", currentValue=model, options=[dict(value=model, name=model)]),
        *([dict(id="mode", currentValue=_MODE_BY_ENGINE[engine][0]["value"], options=_MODE_BY_ENGINE[engine])]
          if mode else []),
        dict(id=effort_option_id, currentValue="medium", options=[
            dict(value=v, name=v) for v in efforts
        ]),
    ]
    scenario = default_scenario(config_options=config_options, request_log_path=str(request_log), **scenario)
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


def _snapshot(connection, model, efforts=("low", "medium", "high")):
    options = [
        {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": model,
         "options": [{"value": model, "name": model}]},
        {"id": "thought_level", "name": "Thought level", "category": "thought_level", "type": "select",
         "currentValue": "medium", "options": [{"value": v, "name": v} for v in efforts]},
    ]
    return build_catalog({connection.id: connection}, {connection.id: parse_observation(options)})


def _setup(tmp_path, *, observed=True, a=None, b=None, model_a="coordinator-model", model_b="reviewer-model",
           efforts=("low", "medium", "high")):
    store, task_id, channel, epoch, project = _bootstrap(tmp_path)
    environ = isolated_environ(tmp_path)
    log_a, log_b = tmp_path / "a.jsonl", tmp_path / "b.jsonl"
    connection_a = _make_connection(tmp_path, environ, engine="codex-acp", model=model_a, request_log=log_a, efforts=efforts,
                                    **(a or {}))
    connection_b = _make_connection(tmp_path, environ, engine="claude-agent-acp", model=model_b, request_log=log_b, efforts=efforts,
                                    **(b or {}))
    snapshot_a = _snapshot(connection_a, model_a, efforts)
    if observed:
        catalog_store.save_snapshot(snapshot_a, environ)
        catalog_store.save_snapshot(_snapshot(connection_b, model_b, efforts), environ)

    entry_a = snapshot_a.entries[0]
    service = ConfigurationService(store)
    row = service.propose(
        task_id, entry_a, ConfigurationInputs(route=entry_a.route, parameters={}),
        expected_revision=0, current_epoch=epoch,
        # These mixed-engine controls intentionally permit both observed
        # routes. A coordinator-only ceiling used to escape enforcement.
        user_layer=dict(defaults={}, ceiling=PolicyCeiling()),
    )
    service.apply(task_id, row["id"], _FakeConfigAdapter(), "es_" + "a" * 12, epoch)

    adapter = EngineSessionManagerAdapter(
        lambda t: connection_a, lambda t: str(project), environ=environ, call_timeout=10.0, store=store,
    )
    provisioning = ProvisioningService(
        store, SharedProjectRootWorkspaceProvisioner(), adapter, ConfigurationServiceSnapshotSource(service),
    )
    config = routing_fixture()
    # These fake catalog routes are intentionally automatic, even when they
    # are historical IDs: executable code must never supersede owner choices.
    for engine, model in (("codex-acp", model_a), ("claude-agent-acp", model_b)):
        if model != "claude-haiku-4-5-20251001":
            config["models"][engine]["auto"] = [model]
    _routing(project, config)
    return store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ


def _codex_used(connection, percent):
    day = Path(connection.credential_ref.path) / "sessions" / "2026" / "09" / "22"
    day.mkdir(parents=True, exist_ok=True)
    limits = {"primary": {"used_percent": percent, "resets_at": time.time() + 3600}}
    (day / "rollout.jsonl").write_text(json.dumps({"payload": {"rate_limits": limits}}) + "\n")


def _claude_used(connection, percent, environ):
    quota.observe(connection.id, dict(rateLimitType="unscoped", utilization=percent / 100.0,
                                      resetsAt=time.time() + 3600), environ)


def _routing(project, overrides):
    (project / ".cortex").mkdir(exist_ok=True)
    (project / ".cortex" / "routing.json").write_text(json.dumps(routing_fixture(overrides)), encoding="utf-8")


def _effort_calls(log_path):
    return [
        entry for entry in _read_log(log_path)
        if entry["method"] == "session/set_config_option"
        and (entry.get("params") or {}).get("configId") in ("effort", "thought_level", "reasoning_effort")
    ]


def test_deep_work_moves_off_the_account_above_the_threshold(tmp_path):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _codex_used(connection_a, 95.0)
    result = provisioning.delegate(channel, "Audit", "Audit it.", "security-auditor", "Report.", "req-1")
    assert result.status == "ready"
    assert "session/new" in [entry["method"] for entry in _read_log(log_b)]
    assert _read_log(log_a) == []
    calls = _effort_calls(log_b)
    assert calls and all(call["params"].get("value") == "high" for call in calls)
    detail = routing_store.list_audit_for_attempt(store, result.attempt_id)[0]
    from cortex_runtime.delegation.events import list_since
    confirmed = [e.payload for e in list_since(store, channel.task_id)
                 if e.payload.get('kind') == 'worker_configuration_confirmed']
    assert len(confirmed) == 1
    assert confirmed[0]['attempt_id'] == result.attempt_id
    assert confirmed[0]['connection_id'] == connection_b.id
    assert confirmed[0]['parameters']['effort'] == 'high'
    assert confirmed[0]['confirmation_evidence_ref']
    assert detail["connection_id"] == connection_b.id
    assert detail["decision_detail"]["route_basis"] == "auto"
    assert detail["decision_detail"]["effort"] == "high"
    decision = detail["decision_detail"]["route_decision"]
    assert decision["rank_version"] == 14
    assert "balance" not in decision
    assert decision["used_percent_compared"] is False
    assert decision["usage"] == dict(compared=False, active=False, status="unavailable", threshold=95.0, margin=10.0, least_used_percent=None, scope="all")
    assert {c["quota_state"] for c in decision["candidates"]} == {"known", "unknown"}
    assert not hasattr(adapter, "_route_inflight")
    adapter.terminate_owned(result.attempt_id)


def test_worker_configuration_confirmed_is_findable_by_its_own_recovery_capsule(tmp_path):
    """The event used to be stored with assignment_ref=None (``launch``
    never had an "assignment_ref" key -- only the unrelated work-graph
    "assignment_id" -- so ``launch.get("assignment_ref")`` was always
    ``None``), and recovery.capsule's own lookup is an exact match on the
    real assignment_ref (the provisioning idempotency_key). Confirm the real
    end-to-end delegate() -> build_capsule path now finds it."""
    from cortex_runtime.recovery.capsule import build_capsule

    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    result = provisioning.delegate(channel, "Audit", "Audit it.", "security-auditor", "Report.", "req-confirm-1")
    assert result.status == "ready"
    capsule = build_capsule(store, channel.task_id,
                            dict(role="worker", assignment_ref=result.assignment_ref, attempt_id=result.attempt_id))
    confirmed = capsule["sections"]["route_and_config"]["own_worker_route"]["worker_configuration_confirmed"]
    assert confirmed.get("status") != "unavailable", confirmed
    assert confirmed["attempt_id"] == result.attempt_id
    assert confirmed["confirmation_evidence_ref"]
    adapter.terminate_owned(result.attempt_id)


def test_a_known_hot_account_now_loses_to_an_unknown_alternative_despite_affinity(tmp_path):
    """Rank version 11: a known >=95%-used candidate
    always loses to a non-hot one now, even an *unknown*-usage
    alternative and even when engine affinity would otherwise keep the
    work on the hot account -- rank version 6's 'no known cheaper
    alternative keeps it' premise no longer holds."""
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _routing(project, {"engine_affinity": {"*": "codex-acp"}})
    _codex_used(connection_a, 95.0)
    result = provisioning.delegate(channel, "Look", "Find it.", "explorer", "Found.", "req-1")
    assert result.status == "ready"
    assert "session/new" in [entry["method"] for entry in _read_log(log_b)]
    assert _read_log(log_a) == []
    assert _effort_calls(log_b)[-1]["params"]["value"] == "low"
    decision = routing_store.list_audit_for_attempt(store, result.attempt_id)[0]["decision_detail"]["route_decision"]
    assert decision["decided_by"] == "quota"
    adapter.terminate_owned(result.attempt_id)


def test_light_work_still_moves_off_the_account_above_the_threshold_on_a_meaningful_gap(tmp_path):
    """A meaningfully cheaper known alternative still moves light work off
    the hot account; the hot/non-hot quota flag decides it directly now
    (rank version 11), ahead of the usage-gap signal it used to
    reach through."""
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _routing(project, {"engine_affinity": {"*": "codex-acp"}})
    _codex_used(connection_a, 95.0)
    _claude_used(connection_b, 10.0, environ)
    result = provisioning.delegate(channel, "Look", "Find it.", "explorer", "Found.", "req-1")
    assert result.status == "ready"
    assert "session/new" in [entry["method"] for entry in _read_log(log_b)]
    assert _read_log(log_a) == []
    assert _effort_calls(log_b)[-1]["params"]["value"] == "low"
    decision = routing_store.list_audit_for_attempt(store, result.attempt_id)[0]["decision_detail"]["route_decision"]
    assert decision["decided_by"] == "quota"
    adapter.terminate_owned(result.attempt_id)


def test_a_late_quota_tie_breaker_still_moves_work_when_affinity_ties(tmp_path):
    """Debugging has no engine affinity lean (neutral); with no meaningful
    usage gap, the quota band remains a real, if late, tie breaker."""
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _codex_used(connection_a, 95.0)
    _claude_used(connection_b, 94.0, environ)
    result = provisioning.delegate(channel, "Debug", "Debug it.", "debugger", "Fixed.", "req-1")
    assert result.status == "ready"
    assert "session/new" in [entry["method"] for entry in _read_log(log_b)]
    assert _read_log(log_a) == []
    decision = routing_store.list_audit_for_attempt(store, result.attempt_id)[0]["decision_detail"]["route_decision"]
    assert decision["decided_by"] == "quota"
    adapter.terminate_owned(result.attempt_id)


def test_earlier_attempt_counts_never_move_the_choice(tmp_path, monkeypatch):
    """Many recorded Codex attempts in the tier do not hand implementation
    work to Claude: only the work and plan usage decide."""
    from cortex_runtime.orchestration import state

    store, channel, provisioning, adapter, *_rest = _setup(tmp_path)
    project = _rest[-2]
    monkeypatch.setattr(state, "attempt_routing", lambda *_: state.AttemptRouting(False, "normal", "auto", None, None))
    decisions = []
    for index in range(6):
        launch = {"task_id": channel.task_id, "attempt_id": f"repeat-{index}",
                  "assignment_id": f"repeat-assignment-{index}", "role_id": "worker-backend-dev"}
        adapter._resolve_runtime_route(launch, str(project))
        decision = launch["route_decision"]
        decisions.append(decision)
        routing_store.record_attempt_audit(
            store, attempt_id=launch["attempt_id"], task_id=channel.task_id, role="worker-backend-dev",
            engine=decision["chosen"].split("/", 1)[0], connection_id=launch["resolved_connection_id"],
            model=launch["resolved_model"], params_fingerprint="fp", config_revision_id=None,
            decision="authorized", detail={"route_decision": decision},
        )
    assert {d["chosen"].split("/", 1)[0] for d in decisions} == {"codex-acp"}
    assert {d["decided_by"] for d in decisions} == {"affinity"}


def test_claude_usage_reported_by_the_engine_steers_the_choice(tmp_path):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _codex_used(connection_a, 10.0)
    quota.observe(connection_b.id, {"status": "allowed_warning", "rateLimitType": "seven_day",
                                    "utilization": 0.95, "resetsAt": time.time() + 3600}, environ)
    result = provisioning.delegate(channel, "Debug", "Debug it.", "debugger", "Fixed.", "req-1")
    assert result.status == "ready"
    assert _read_log(log_a) and _read_log(log_b) == []
    adapter.terminate_owned(result.attempt_id)


def test_unobserved_catalogs_refuse_automatic_dispatch(tmp_path):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(
        tmp_path, observed=False)
    with pytest.raises(PortableError) as refused:
        provisioning.delegate(channel, "Do it", "Do the work.", "backend-dev", "Passes.", "req-1")
    assert refused.value.code == "automatic_route_identity_unavailable"
    assert _read_log(log_a) == [] and _read_log(log_b) == []


def test_default_haiku_exclusion_dispatches_light_writing_to_stretch(tmp_path):
    """A real delegated light writing role reaches Luna even when Claude offers Haiku."""
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(
        tmp_path, model_a="gpt-5.6-luna", model_b="claude-haiku-4-5-20251001", efforts=("low", "medium", "high", "xhigh", "max"))
    result = provisioning.delegate(channel, "Write", "Write it.", "technical-writer", "Written.", "req-1")
    assert result.status == "ready"
    assert "session/new" in [entry["method"] for entry in _read_log(log_a)]
    assert _read_log(log_b) == []
    audit = routing_store.list_audit_for_attempt(store, result.attempt_id)[0]
    assert audit["model"] == "gpt-5.6-luna"
    assert audit["decision_detail"]["route_basis"] == "auto"
    assert audit["decision_detail"]["route_decision"]["chosen"] == "codex-acp/gpt-5.6-luna"
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
    _codex_used(connection_a, 95.0)
    _routing(project, {"deny": ["reviewer-model"]})
    result = provisioning.delegate(channel, "Audit", "Audit it.", "security-auditor", "Report.", "req-1")
    assert result.status == "ready"
    assert _read_log(log_a) and _read_log(log_b) == []
    adapter.terminate_owned(result.attempt_id)


def test_providers_restrict_auto_routing_to_the_listed_engine(tmp_path):
    """Codex is hot and would normally lose the deep-work pick to Claude,
    but 'providers' excludes Claude, so the work stays on Codex instead."""
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _codex_used(connection_a, 95.0)
    _routing(project, {"providers": ["codex-acp"]})
    result = provisioning.delegate(channel, "Audit", "Audit it.", "security-auditor", "Report.", "req-1")
    assert result.status == "ready"
    assert "session/new" in [entry["method"] for entry in _read_log(log_a)]
    assert _read_log(log_b) == []
    adapter.terminate_owned(result.attempt_id)


def test_a_pin_to_an_excluded_provider_is_refused(tmp_path):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _routing(project, {"providers": ["codex-acp"], "pin": {"planner": "claude-agent-acp/reviewer-model"}})
    with pytest.raises(PortableError) as refused:
        provisioning.delegate(channel, "Plan", "Plan it.", "planner", "Planned.", "req-1")
    assert refused.value.code == "pinned_route_unavailable"
    assert "providers" in refused.value.remediation
    assert _read_log(log_a) == [] and _read_log(log_b) == []


def test_providers_excluding_every_route_fails_closed(tmp_path):
    """Both connections are engine-confirmed but neither engine is allowed:
    refused, never silently falls back to the coordinator's (excluded) route."""
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _routing(project, {"providers": ["claude-agent-acp"]})
    with connection_registry.transaction(environ) as state:
        state.connections = {connection_a.id: connection_a}
    with pytest.raises(PortableError) as refused:
        provisioning.delegate(channel, "Audit", "Audit it.", "security-auditor", "Report.", "req-1")
    assert refused.value.code == "no_available_route"
    assert "providers" in refused.value.remediation
    assert _read_log(log_a) == [] and _read_log(log_b) == []


def test_connection_without_launch_command_is_skipped(tmp_path):
    """A discovery placeholder must not shadow a launchable ACP connection."""
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _codex_used(connection_a, 95.0)
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


# --- choosing among one tier's candidates (auto_route.rank) -----------------

def _planned_provider(monkeypatch, provider):
    """The node's planned ``provider``, as ``attempt_routing`` reads it from the plan."""
    from cortex_runtime.orchestration import state

    monkeypatch.setattr(state, "attempt_routing", lambda store, task_id, attempt_id:
                        state.AttemptRouting(False, "normal", provider, "This work suits the engine.", None))


def _decision(store, result):
    return routing_store.list_audit_for_attempt(store, result.attempt_id)[0]["decision_detail"]["route_decision"]


@pytest.mark.parametrize("role, engine", [("backend-dev", "codex-acp"), ("security-auditor", "claude-agent-acp")])
def test_idle_accounts_are_split_by_the_kind_of_work_not_the_listing_order(tmp_path, role, engine):
    """Neither account reports usage: implementation goes to Codex, review to
    Claude, whichever connection is listed first."""
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    result = provisioning.delegate(channel, "Work", "Do it.", role, "Done.", "req-1")
    assert result.status == "ready"
    chosen, other = (log_a, log_b) if engine == "codex-acp" else (log_b, log_a)
    assert "session/new" in [entry["method"] for entry in _read_log(chosen)] and _read_log(other) == []
    decision = _decision(store, result)
    assert decision["chosen"].startswith(engine + "/") and decision["preference_status"] == "none"
    assert {c["route"].split("/")[0] for c in decision["candidates"]} == {"codex-acp", "claude-agent-acp"}
    adapter.terminate_owned(result.attempt_id)


def test_the_coordinators_provider_preference_is_honoured(tmp_path, monkeypatch):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _planned_provider(monkeypatch, "codex-acp")
    result = provisioning.delegate(channel, "Audit", "Audit it.", "security-auditor", "Report.", "req-1")
    assert result.status == "ready"
    assert _read_log(log_a) and _read_log(log_b) == []
    decision = _decision(store, result)
    assert decision["model_hint"] == "codex-acp" and decision["model_hint_status"] == "honoured"
    adapter.terminate_owned(result.attempt_id)


def test_a_preference_outside_providers_is_ignored_and_recorded(tmp_path, monkeypatch):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _routing(project, {"providers": ["codex-acp"]})
    _planned_provider(monkeypatch, "claude-agent-acp")
    result = provisioning.delegate(channel, "Audit", "Audit it.", "security-auditor", "Report.", "req-1")
    assert result.status == "ready"
    assert _read_log(log_a) and _read_log(log_b) == []
    assert _decision(store, result)["model_hint_status"] == "ignored:provider_not_allowed"
    adapter.terminate_owned(result.attempt_id)


def test_a_preference_never_overrides_a_pin(tmp_path, monkeypatch):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _routing(project, {"pin": {"planner": "codex-acp/coordinator-model"}})
    _planned_provider(monkeypatch, "claude-agent-acp")
    result = provisioning.delegate(channel, "Plan", "Plan it.", "planner", "Planned.", "req-1")
    assert result.status == "ready"
    assert _read_log(log_a) and _read_log(log_b) == []
    decision = _decision(store, result)
    assert decision["basis"] == "pin" and decision["model_hint_status"] == "ignored:pinned"
    adapter.terminate_owned(result.attempt_id)


def test_a_preference_never_takes_an_exhausted_provider(tmp_path, monkeypatch):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _unavailable(store, channel, connection_a)
    _planned_provider(monkeypatch, "codex-acp")
    result = provisioning.delegate(channel, "Build", "Build it.", "backend-dev", "Built.", "req-1")
    assert result.status == "ready"
    assert _read_log(log_a) == [] and _read_log(log_b)
    assert _decision(store, result)["model_hint_status"] == "ignored:unavailable"
    adapter.terminate_owned(result.attempt_id)


# --- routes that just failed are skipped (routing.availability) ------------

def _runtime_task(store, channel):
    """Unavailability is recorded on the failed attempt's runtime task."""
    from cortex_runtime.orchestration.state import TaskMachine

    TaskMachine(store).create(channel.task_id, "Do work.")


def _unavailable(store, channel, connection, *, reason="quota_exhausted", scope="connection", model=None, **kwargs):
    from cortex_runtime.routing import availability

    _runtime_task(store, channel)
    return availability.record(
        store, channel.task_id, engine=connection.engine, connection_id=connection.id, model=model,
        reason=reason, scope=scope, evidence="test", **kwargs,
    )


def test_an_exhausted_provider_is_skipped(tmp_path):
    """Implementation work leans to the Codex account with room; its
    recorded exhaustion sends the work to the other provider instead."""
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _codex_used(connection_a, 10.0)
    _unavailable(store, channel, connection_a)
    result = provisioning.delegate(channel, "Build", "Build it.", "backend-dev", "Built.", "req-1")
    assert result.status == "ready"
    assert _read_log(log_a) == []
    assert "session/new" in [entry["method"] for entry in _read_log(log_b)]
    adapter.terminate_owned(result.attempt_id)


def test_a_model_scoped_record_leaves_the_rest_of_the_account_usable(tmp_path):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _codex_used(connection_a, 10.0)
    _unavailable(store, channel, connection_a, reason="model_unavailable", scope="model", model="another-model")
    result = provisioning.delegate(channel, "Build", "Build it.", "backend-dev", "Built.", "req-1")
    assert result.status == "ready"
    assert "session/new" in [entry["method"] for entry in _read_log(log_a)]
    adapter.terminate_owned(result.attempt_id)


def test_an_expired_record_no_longer_blocks(tmp_path):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _codex_used(connection_a, 10.0)
    # A rate limit with no reported reset keeps a route out for 15 minutes.
    _unavailable(store, channel, connection_a, reason="rate_limited", now=time.time() - 3600)
    result = provisioning.delegate(channel, "Build", "Build it.", "backend-dev", "Built.", "req-1")
    assert result.status == "ready"
    assert "session/new" in [entry["method"] for entry in _read_log(log_a)]
    adapter.terminate_owned(result.attempt_id)


def test_nothing_available_is_refused_naming_every_route(tmp_path):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _unavailable(store, channel, connection_a, resets_at=time.time() + 7200)
    _unavailable(store, channel, connection_b, reason="rate_limited")
    with pytest.raises(PortableError) as refused:
        provisioning.delegate(channel, "Look", "Find it.", "explorer", "Found.", "req-1")
    assert refused.value.code == "no_available_route"
    assert refused.value.retry_eligible is False
    assert connection_a.id in refused.value.remediation and connection_b.id in refused.value.remediation
    assert "quota_exhausted" in refused.value.remediation and "rate_limited" in refused.value.remediation
    # Refused before any engine was started -- never the coordinator's route.
    assert _read_log(log_a) == [] and _read_log(log_b) == []


def test_deny_is_never_lifted_to_make_room(tmp_path):
    """Codex exhausted and the only other model denied: refused, not the denied model."""
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _routing(project, {"deny": ["reviewer-model"]})
    _unavailable(store, channel, connection_a)
    with pytest.raises(PortableError) as refused:
        provisioning.delegate(channel, "Audit", "Audit it.", "security-auditor", "Report.", "req-1")
    assert refused.value.code == "no_available_route"
    assert _read_log(log_a) == [] and _read_log(log_b) == []


def test_an_unavailable_pin_is_never_substituted(tmp_path):
    """The pinned account is exhausted and the other one is free: still refused."""
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _routing(project, {"pin": {"planner": "codex-acp/coordinator-model"}})
    _unavailable(store, channel, connection_a)
    with pytest.raises(PortableError) as refused:
        provisioning.delegate(channel, "Plan", "Plan it.", "planner", "Planned.", "req-1")
    assert refused.value.code == "pinned_route_unavailable"
    assert "quota_exhausted" in refused.value.remediation
    assert _read_log(log_a) == [] and _read_log(log_b) == []


def test_the_coordinators_route_is_refused_when_it_is_unavailable(tmp_path):
    """Without a catalog, even an exhausted inherited route cannot be
    mistaken for a confirmed automatic identity: refused before opening."""
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(
        tmp_path, observed=False)
    _unavailable(store, channel, connection_a)
    with pytest.raises(PortableError) as refused:
        provisioning.delegate(channel, "Do it", "Do the work.", "backend-dev", "Passes.", "req-1")
    assert refused.value.code == "automatic_route_identity_unavailable"
    assert _read_log(log_a) == []


def test_a_dispatch_failure_records_the_route_and_the_retry_uses_another_provider(tmp_path):
    """The Codex engine never offers a worker mode: WorkerModeNotConfirmed.
    That route is recorded and the next delegation opens on Claude."""
    from cortex_runtime.delegation.live_adapters import WorkerModeNotConfirmed
    from cortex_runtime.routing import availability

    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(
        tmp_path, a=dict(mode=False))
    _runtime_task(store, channel)
    _codex_used(connection_a, 10.0)
    with pytest.raises(WorkerModeNotConfirmed):
        provisioning.delegate(channel, "Build", "Build it.", "backend-dev", "Built.", "req-1")
    [record] = availability.active(store)
    assert record["connection_id"] == connection_a.id and record["model"] == "coordinator-model"
    assert (record["reason"], record["scope"], record["expiry_basis"]) == ("configuration_not_confirmed", "model", "default")
    with store.connection() as db:
        attempt_id = db.execute("SELECT attempt_id FROM delegation_provisioning WHERE task_id=?",
                                (channel.task_id,)).fetchone()[0]
        assert availability.for_attempt(db, channel.task_id, attempt_id)["reason"] == "configuration_not_confirmed"
    result = provisioning.delegate(channel, "Build", "Build it.", "backend-dev", "Built.", "req-2")
    assert result.status == "ready"
    assert "session/new" in [entry["method"] for entry in _read_log(log_b)]
    adapter.terminate_owned(result.attempt_id)


def test_an_exhausted_plan_during_a_turn_records_the_account_until_its_reset(tmp_path):
    """codex-acp 1.13.0 refuses a turn on an exhausted plan with -32603 and
    data.codexErrorInfo="usageLimitExceeded"; the reset comes from the
    engine's own rate_limits."""
    import threading

    from cortex_runtime.acp.orchestrating_runtime import EngineTurnFailed
    from cortex_runtime.routing import availability

    refusal = dict(code=-32603, message="Internal error",
                   data=dict(message="You've hit your usage limit.", codexErrorInfo="usageLimitExceeded"))
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(
        tmp_path, a=dict(prompt_behavior="error", prompt_error=refusal))
    _runtime_task(store, channel)
    _codex_used(connection_a, 10.0)
    result = provisioning.delegate(channel, "Build", "Build it.", "backend-dev", "Built.", "req-1")
    assert "session/new" in [entry["method"] for entry in _read_log(log_a)]
    with pytest.raises(EngineTurnFailed) as failed:
        adapter.prompt(result.engine_session_ref, "Build it.", threading.Event())
    assert failed.value.kind == "route_unavailable:quota_exhausted"
    [record] = availability.active(store)
    assert (record["connection_id"], record["scope"], record["model"]) == (connection_a.id, "connection", None)
    assert record["evidence"] == "codexErrorInfo=usageLimitExceeded"
    assert record["expiry_basis"] == "engine_reported" and record["attempt_id"] == result.attempt_id
    # The recorded account now stays out; nothing in it is credential-like.
    assert set(record) == {"engine", "connection_id", "model", "scope", "reason", "evidence", "attempt_id",
                           "expires_at", "expiry_basis"}
    adapter.terminate_owned(result.attempt_id)
    retry = provisioning.delegate(channel, "Build", "Build it.", "backend-dev", "Built.", "req-2")
    assert retry.status == "ready"
    assert "session/new" in [entry["method"] for entry in _read_log(log_b)]
    adapter.terminate_owned(retry.attempt_id)


def test_an_unrelated_turn_error_records_nothing(tmp_path):
    import threading

    from cortex_runtime.engine.client import EngineProtocolError
    from cortex_runtime.routing import availability

    refusal = dict(code=-32603, message="Internal error: usage limit exceeded", data=dict(codexErrorInfo="other"))
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(
        tmp_path, a=dict(prompt_behavior="error", prompt_error=refusal))
    _runtime_task(store, channel)
    _codex_used(connection_a, 10.0)
    result = provisioning.delegate(channel, "Build", "Build it.", "backend-dev", "Built.", "req-1")
    with pytest.raises(EngineProtocolError):
        adapter.prompt(result.engine_session_ref, "Build it.", threading.Event())
    assert availability.active(store) == []
    adapter.terminate_owned(result.attempt_id)



def test_exact_model_hint_and_reason_are_audited(tmp_path, monkeypatch):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _planned_provider(monkeypatch, "codex-acp/coordinator-model")
    result = provisioning.delegate(channel, "Audit", "Audit it.", "security-auditor", "Report.", "req-hint")
    assert result.status == "ready"
    detail = routing_store.list_audit_for_attempt(store, result.attempt_id)[0]["decision_detail"]
    decision = detail["route_decision"]
    assert decision["model_hint"] == "codex-acp/coordinator-model"
    assert decision["model_hint_status"] == "honoured"
    assert decision["model_hint_reason"] == "This work suits the engine."
    assert detail["route_decision"]["chosen"] == "codex-acp/coordinator-model"
    adapter.terminate_owned(result.attempt_id)


@pytest.mark.parametrize("requested", [True, False])
def test_a_user_requested_flagship_is_dispatched_and_audited(tmp_path, monkeypatch, requested):
    """Standard implementation work runs on the flagship route only when the
    user asked for its family; the audit names the user's source."""
    from cortex_runtime.orchestration import state

    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(
        tmp_path, model_a="gpt-6-astra", model_b="claude-sonnet-5")
    user_request = dict(source_ref="r_0123456789ab:-1", family="astra", matched="Astra") if requested else None
    monkeypatch.setattr(state, "attempt_routing", lambda store, task_id, attempt_id: state.AttemptRouting(
        False, "normal", "codex-acp/gpt-6-astra", "The user asked for Astra.", None, user_request))
    result = provisioning.delegate(channel, "Build", "Build it.", "backend-dev", "Done.", "req-user-flagship")
    assert result.status == "ready"
    audit = routing_store.list_audit_for_attempt(store, result.attempt_id)[0]
    detail = audit["decision_detail"]
    decision = detail["route_decision"]
    if requested:
        assert (audit["engine"], audit["model"], detail["route_basis"]) == ("codex-acp", "gpt-6-astra", "auto:user_request")
        assert (decision["tier"], decision["decided_by"]) == ("flagship", "user_request")
        assert decision["user_model_request"] == dict(user_request, status="honoured")
    else:
        assert (audit["engine"], audit["model"], detail["route_basis"]) == ("claude-agent-acp", "claude-sonnet-5", "auto")
        assert decision["model_hint_status"] == "ignored:tier_mismatch"
        assert decision["user_model_request"] == dict(status="not_requested_by_user")
    adapter.terminate_owned(result.attempt_id)


def test_a_user_revocation_reaches_the_live_route_audit(tmp_path, monkeypatch):
    """``orchestration.state.attempt_routing`` and
    ``delegation.live_adapters._resolve_runtime_route`` carry the user's
    revocation (``routing.auto_route.user_model_revocation``) through to
    the real dispatch, not merely at the ``auto_route.choose()`` level --
    the gap an earlier fix left open."""
    from cortex_runtime.orchestration import state

    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(
        tmp_path, model_a="gpt-6-astra", model_b="claude-sonnet-5")
    user_revocation = dict(source_ref="r_0123456789ab:-1", family="astra", matched="Astra",
                           reason="revoked_by_user", revoked_source_ref="r_fedcba987654:-1")
    monkeypatch.setattr(state, "attempt_routing", lambda store, task_id, attempt_id: state.AttemptRouting(
        False, "normal", "codex-acp/gpt-6-astra", "The user asked for Astra.", None, None, user_revocation))
    result = provisioning.delegate(channel, "Build", "Build it.", "backend-dev", "Done.", "req-user-revocation")
    assert result.status == "ready"
    audit = routing_store.list_audit_for_attempt(store, result.attempt_id)[0]
    decision = audit["decision_detail"]["route_decision"]
    # The revoked family is never substituted in: the ordinary non-flagship
    # route is dispatched, exactly as an absent/unrequested hint would be.
    assert (audit["engine"], audit["model"]) == ("claude-agent-acp", "claude-sonnet-5")
    assert decision["user_model_request"] == dict(user_revocation, status="revoked_by_user")
    adapter.terminate_owned(result.attempt_id)


@pytest.mark.parametrize("trigger", ["retry", "review_fix"])
def test_normal_testing_retry_and_review_fix_dispatch_stretch_max(tmp_path, trigger):
    from cortex_runtime.orchestration.state import TaskMachine, event
    from test_runtime_task_machine import plan

    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(
        tmp_path, model_a="gpt-6-luna", model_b="claude-sonnet-5",
        efforts=("low", "medium", "high", "xhigh", "max"))
    # Testing has no default engine lean; name one so the engine-order
    # tie between the two connections cannot decide.
    (project / ".cortex").mkdir(exist_ok=True)
    (project / ".cortex" / "routing.json").write_text(json.dumps(routing_fixture({"engine_affinity": {"testing": "codex-acp"}})))
    machine = TaskMachine(store)
    machine.create(channel.task_id, "Test ordinary backend work")
    testing_plan = plan()
    testing_plan["tasks"][0]["role"] = "qa-engineer"
    machine.submit_plan(channel, testing_plan, "normal-plan")
    machine.move(channel.task_id, "EXECUTING")
    node = machine.claim_ready(channel.task_id, channel.channel_id)[0]
    with store.connection() as db:
        if trigger == "retry":
            event(db, channel.task_id, "ASSIGNMENT_RETRIED",
                  dict(node=node["node_id"], revision=node["revision"], retry=1))
        else:
            db.execute("UPDATE runtime_nodes SET node_id=? WHERE task_id=? AND assignment_ref=?",
                       ("_fix_1_" + node["node_id"], channel.task_id, node["assignment_ref"]))
        db.commit()

    result = provisioning.delegate(channel, "Fix", "Fix the ordinary backend tests.", "qa-engineer",
                                   "Tests pass.", node["request_key"])
    assert result.status == "ready"
    try:
        assert _read_log(log_b) == []
        assert _effort_calls(log_a)[-1]["params"]["value"] == "max"
        audit = routing_store.list_audit_for_attempt(store, result.attempt_id)[0]
        assert audit["model"] == "gpt-6-luna"
        decision = audit["decision_detail"]["route_decision"]
        assert decision["base_role_tier"] == decision["role_tier"] == "standard"
        assert decision["stretch"]["status"] == "standard_candidate"
        assert decision["stretch"]["chosen"] is True
        assert decision["effort_basis"] == "stretch:standard:escalated"
    finally:
        adapter.terminate_owned(result.attempt_id)


def test_normal_development_retry_never_dispatches_stretch(tmp_path):
    from cortex_runtime.orchestration.state import TaskMachine, event
    from test_runtime_task_machine import plan

    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(
        tmp_path, model_a="gpt-6-luna", model_b="claude-sonnet-5",
        efforts=("low", "medium", "high", "xhigh", "max"))
    machine = TaskMachine(store)
    machine.create(channel.task_id, "Implement ordinary backend work")
    machine.submit_plan(channel, plan(), "normal-plan")
    machine.move(channel.task_id, "EXECUTING")
    node = machine.claim_ready(channel.task_id, channel.channel_id)[0]
    with store.connection() as db:
        event(db, channel.task_id, "ASSIGNMENT_RETRIED",
              dict(node=node["node_id"], revision=node["revision"], retry=1))
        db.commit()

    result = provisioning.delegate(channel, "Fix", "Fix ordinary backend work.", "backend-dev",
                                   "Tests pass.", node["request_key"])
    assert result.status == "ready"
    try:
        assert _read_log(log_a) == []
        audit = routing_store.list_audit_for_attempt(store, result.attempt_id)[0]
        assert audit["model"] == "claude-sonnet-5"
        decision = audit["decision_detail"]["route_decision"]
        assert decision["base_role_tier"] == "standard" and decision["role_tier"] == "deep"
        assert decision["stretch"]["status"] == "excluded:deep_work"
    finally:
        adapter.terminate_owned(result.attempt_id)


@pytest.mark.parametrize("role", ["worker-backend-dev", "worker-code-reviewer"])
@pytest.mark.parametrize("manual_fallback", [False, True])
def test_saved_manual_worker_and_reviewer_routes_cannot_bypass_current_provider_policy(tmp_path, role, manual_fallback):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    with store.connection() as db:
        row = db.execute("SELECT id,task_id,route_id FROM configuration_revisions").fetchone()
    _routing(project, {"providers": ["claude-agent-acp"]})
    launch = dict(config_revision=row["id"], assignment_id="saved-assignment", role_id=role,
                  resolved_route_id=row["route_id"], resolved_model="coordinator-model")
    if manual_fallback: launch["route_override"] = dict(model="coordinator-model")
    with pytest.raises(PortableError) as error:
        adapter._authorize_dispatch(connection_a, row["task_id"], launch, str(project))
    assert error.value.code == "route_provider_excluded"
    assert _read_log(log_a) == [] and _read_log(log_b) == []


@pytest.mark.parametrize("launch_kind", ["coordinator-recovery", "worker-manual", "reviewer"])
def test_legacy_settings_diagnostic_is_not_replaced_by_saved_route_selection(tmp_path, launch_kind):
    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    with store.connection() as db:
        row = db.execute("SELECT id,task_id,route_id FROM configuration_revisions").fetchone()
    (project / ".cortex" / "routing.json").write_text('{}')
    launch = dict(config_revision=row["id"], resolved_route_id=row["route_id"], resolved_model="coordinator-model")
    if launch_kind != "coordinator-recovery": launch.update(assignment_id="saved-assignment", role_id="worker-code-reviewer" if launch_kind == "reviewer" else "worker-backend-dev")
    with pytest.raises(PortableError) as error:
        adapter._authorize_dispatch(connection_a, row["task_id"], launch, str(project))
    assert error.value.code == "model_policy_migration_required"
    assert _read_log(log_a) == [] and _read_log(log_b) == []


@pytest.mark.parametrize("denied", [False, True])
def test_recorded_nonflagship_request_dispatches_or_refuses_without_substitution(tmp_path, monkeypatch, denied):
    from cortex_runtime.bootstrap import append_source_event
    from cortex_runtime.orchestration import state
    from cortex_runtime.portable.identity import SourceEvent
    from cortex_runtime.routing import auto_route

    store, channel, provisioning, adapter, a, b, log_a, log_b, project, env = _setup(
        tmp_path, model_a="gpt-6-sol", model_b="claude-sonnet-5")
    target = "codex-acp/gpt-6-sol"
    _routing(project, {"pin": {"*": "claude-agent-acp/claude-sonnet-5"},
                       "deny": ["gpt-6-sol"] if denied else []})
    _codex_used(a, 99.0)
    _claude_used(b, 0.0, env)
    source = append_source_event(store, SourceEvent(origin="test", event_id="exact-owner"),
                                 f"Use {target}", channel.task_id)
    grant = auto_route.user_model_request(store, channel.task_id, target)
    monkeypatch.setattr(state, "attempt_routing", lambda *_: state.AttemptRouting(
        True, "normal", target, "User choice.", "codex-acp", grant))
    if denied:
        with pytest.raises(PortableError, match="user_requested_route_unavailable"):
            provisioning.delegate(channel, "Build", "Build it.", "backend-dev", "Done.", "req-strict")
        assert _read_log(log_a) == [] and _read_log(log_b) == []
        assert not routing_store.list_audit_for_task(store, channel.task_id)
    else:
        result = provisioning.delegate(channel, "Build", "Build it.", "backend-dev", "Done.", "req-strict")
        assert result.status == "ready"
        audit = routing_store.list_audit_for_attempt(store, result.attempt_id)[0]
        assert (audit["engine"], audit["model"]) == ("codex-acp", "gpt-6-sol")
        assert audit["decision_detail"]["route_basis"] == "auto:user_request"
        decision = audit["decision_detail"]["route_decision"]
        assert decision["user_model_request"]["source_ref"] == f"{source.report_id}:-1"
        assert _read_log(log_b) == [] and _effort_calls(log_a)[-1]["params"]["value"] == "high"
        adapter.terminate_owned(result.attempt_id)


def test_coordinator_only_profile_cannot_silently_replace_user_model(tmp_path, monkeypatch):
    from cortex_runtime.orchestration import state
    from cortex_runtime.configuration.policy import RouteProfile

    store, channel, provisioning, adapter, a, b, log_a, log_b, project, env = _setup(tmp_path)
    revision = ConfigurationService(store).current(channel.task_id)
    profile = RouteProfile(role_routes={"worker-backend-dev": {revision["route_id"]}})
    monkeypatch.setattr(adapter, "_dispatch_configuration", lambda _: (revision, profile))
    grant = dict(source_ref="fixture-owner", family="claude-agent-acp/reviewer-model", matched="reviewer-model")
    monkeypatch.setattr(state, "attempt_routing", lambda *_: state.AttemptRouting(
        False, "normal", "claude-agent-acp/reviewer-model", "User choice.", None, grant))
    launch = dict(task_id=channel.task_id, attempt_id="requested-attempt",
                  assignment_id="requested-assignment", role_id="worker-backend-dev")
    with pytest.raises(PortableError, match="user_requested_route_unavailable"):
        adapter._resolve_runtime_route(launch, str(project))
    assert not launch.get("resolved_connection_id")
    assert _read_log(log_a) == [] and _read_log(log_b) == []


@pytest.mark.parametrize("used,expected_engine", [(9.999, "codex-acp"), (10.0, "claude-agent-acp"),
                                                  (95.0, "claude-agent-acp")])
def test_usage_boundary_reaches_real_dispatch(tmp_path, used, expected_engine):
    store, channel, provisioning, adapter, a, b, log_a, log_b, project, env = _setup(
        tmp_path, model_a="gpt-5.6-terra", model_b="claude-sonnet-5")
    _codex_used(a, used)
    _claude_used(b, 0.0, env)
    result = provisioning.delegate(channel, "Build", "Build it.", "backend-dev", "Done.", "boundary")
    try:
        audit = routing_store.list_audit_for_attempt(store, result.attempt_id)[0]
        assert audit["engine"] == expected_engine
        usage = audit["decision_detail"]["route_decision"]["usage"]
        assert usage["active"] is True
        assert _read_log(log_a if expected_engine == "claude-agent-acp" else log_b) == []
    finally:
        adapter.terminate_owned(result.attempt_id)


def _delivery_refusal():
    from cortex_runtime.portable.errors import PortableErrorCategory
    return PortableError(
        category=PortableErrorCategory.VALIDATION, code="instruction_delivery_unqualified",
        remediation="Exact engine version is not qualified.", retry_eligible=False,
    )


def test_delivery_refusal_blocks_every_model_on_same_engine_version_for_dispatch(tmp_path, monkeypatch):
    from cortex_runtime.delegation import live_adapters
    from cortex_runtime.routing import availability

    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    _runtime_task(store, channel)
    _routing(project, {"engine_affinity": {"*": "claude-agent-acp"}})
    first = provisioning.delegate(channel, "Build", "Build it.", "backend-dev", "Built.", "req-1")
    assert _read_log(log_b)
    # This is the hook used by the runtime's unexpected generation-error path.
    adapter.record_instruction_delivery_failure(first.engine_session_ref, _delivery_refusal())
    [entry] = availability.active(store)
    assert entry["reason"] == "instruction_delivery_unqualified"
    assert entry["model"] is None and entry["scope"] == "connection"
    assert entry["evidence"] == "claude-agent-acp@0.88.0"
    assert entry["attempt_id"] == first.attempt_id
    adapter.terminate_owned(first.attempt_id)
    before_b = list(_read_log(log_b))

    # Other connections and models on that engine+version cannot evade it.
    (tmp_path / "other").mkdir()
    second_claude = _make_connection(tmp_path / "other", environ, engine="claude-agent-acp",
                                    model="other-model", request_log=tmp_path / "other.jsonl")
    entries = adapter._worker_unavailable_routes(dict(task_id=channel.task_id, role_id="worker-backend-dev"))
    assert availability.blocking(entries, second_claude.id, "other-model")
    assert availability.blocking(entries, connection_b.id, "sonnet")
    with monkeypatch.context() as expired:
        expired.setattr(availability, "active", lambda store: [])
        entries = adapter._worker_unavailable_routes(dict(task_id=channel.task_id, role_id="worker-backend-dev"))
        assert availability.blocking(entries, connection_b.id, "opus")
        assert availability.blocking(entries, second_claude.id, "other-model")
    result = provisioning.delegate(channel, "Build", "Build it.", "backend-dev", "Built.", "req-2")
    try:
        assert _read_log(log_a) and _read_log(log_b) == before_b
        assert routing_store.list_audit_for_attempt(store, result.attempt_id)[0]["engine"] == "codex-acp"
    finally:
        adapter.terminate_owned(result.attempt_id)

    # An updated qualified install and roles with no critical rules are not
    # poisoned by a connection-level record of another version's refusal.
    original = live_adapters._pinned_version_for
    monkeypatch.setattr(live_adapters, "_pinned_version_for",
                        lambda engine, record=None: "0.87.0" if engine == "claude-agent-acp"
                        else original(engine, record))
    entries = adapter._worker_unavailable_routes(dict(task_id=channel.task_id, role_id="worker-backend-dev"))
    assert availability.blocking(entries, connection_b.id, "sonnet") is None
    entries = adapter._worker_unavailable_routes(dict(task_id=channel.task_id, role_id="content-safety"))
    assert availability.blocking(entries, connection_b.id, "sonnet") is None


def test_unregistered_claude_dispatch_skips_provider_before_start_and_pin_refuses(tmp_path, monkeypatch):
    from cortex_runtime.delegation import live_adapters

    store, channel, provisioning, adapter, connection_a, connection_b, log_a, log_b, project, environ = _setup(tmp_path)
    original = live_adapters._pinned_version_for
    monkeypatch.setattr(live_adapters, "_pinned_version_for",
                        lambda engine, record=None: "0.89.0" if engine == "claude-agent-acp"
                        else original(engine, record))
    _routing(project, {"engine_affinity": {"*": "claude-agent-acp"}})
    result = provisioning.delegate(channel, "Build", "Build it.", "backend-dev", "Built.", "req-1")
    adapter.terminate_owned(result.attempt_id)
    assert _read_log(log_a) and _read_log(log_b) == []
    _routing(project, {"pin": {"backend-dev": "claude-agent-acp/reviewer-model"}})
    with pytest.raises(PortableError) as refused:
        provisioning.delegate(channel, "Build", "Build it.", "backend-dev", "Built.", "req-2")
    assert refused.value.code == "pinned_route_unavailable"
    assert "instruction_delivery_unqualified" in refused.value.remediation
    assert _read_log(log_b) == []


def test_unrelated_or_retryable_portable_failure_does_not_poison_engine_route(tmp_path):
    from cortex_runtime.portable.errors import PortableErrorCategory
    from cortex_runtime.routing import availability

    store, channel, _, adapter, _, connection_b, *_ = _setup(tmp_path)
    _runtime_task(store, channel)
    for code, retry in [("invalid_field", False), ("instruction_delivery_unqualified", True)]:
        error = PortableError(category=PortableErrorCategory.VALIDATION, code=code,
                              remediation="Test error.", retry_eligible=retry)
        adapter._record_dispatch_route_failure(error, connection_b, "reviewer-model", channel.task_id, "xa_test")
    assert adapter._unqualified_instruction_routes == set()
    assert availability.active(store) == []


def test_unknown_role_is_fenced_not_refused_by_delivery_qualification(tmp_path, monkeypatch):
    """Role validity belongs to route authorization: the delivery fence must
    neither raise for an unloadable role nor let one evade the fence."""
    from cortex_runtime.delegation import live_adapters
    from cortex_runtime.routing import availability

    store, channel, _, adapter, _, connection_b, *_ = _setup(tmp_path)
    original = live_adapters._pinned_version_for
    monkeypatch.setattr(live_adapters, "_pinned_version_for",
                        lambda engine, record=None: "0.89.0" if engine == "claude-agent-acp"
                        else original(engine, record))
    entries = adapter._worker_unavailable_routes(dict(task_id=channel.task_id, role_id="implementer"))
    entry = availability.blocking(entries, connection_b.id, "sonnet")
    assert entry is not None and entry["reason"] == "instruction_delivery_unqualified"
