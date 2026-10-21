"""Explicit owner effort is source-bound, exact, and never an automatic target."""
import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.routing import auto_route
from cortex_runtime.store import Store
from test_settings_only_routing import fake_catalog, install, settings
from test_model_settings_template import entry


TARGET = "codex-acp/gpt-6.1-sol"


@pytest.mark.parametrize("text", [
    f"Use {TARGET}:medium throughout.",
    f"Use {TARGET}/medium for coordinator, specialists, review, retries and fixes.",
    f"Use {TARGET} at medium effort throughout.",
    f'Use "{TARGET}" at medium effort throughout.',
    f'Use "{TARGET}:medium" throughout.',
])
@pytest.mark.parametrize("role", ["worker-explorer", "worker-code-reviewer", "worker-debugger"])
@pytest.mark.parametrize("escalate", [False, True])
def test_explicit_effort_is_not_replaced_by_deep_model_default(tmp_path, monkeypatch, text, role, escalate):
    data = settings("gpt-6.1-sol", tier="deep", attributes={"effort_floor": "medium", "max_effort": "xhigh"})
    env = install(tmp_path, data)
    fake_catalog(monkeypatch, [entry("codex-acp", "gpt-6.1-sol")])
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="owner"), text, "owner")
    request = auto_route.user_model_request(store, task.task_id, TARGET)
    choice = auto_route.choose(role, tmp_path, env, model_hint=TARGET,
                               user_request=request, escalate=escalate)
    assert choice.effort == "medium"
    assert request["effort"] == "medium"
    assert request["source_ref"] == f"{task.report_id}:-1"
    assert choice.decision["effort_basis"] == "user_request:explicit_effort"


def test_explicit_effort_conflict_refuses_instead_of_clamping(tmp_path, monkeypatch):
    env = install(tmp_path, settings("gpt-6.1-sol", tier="deep", attributes={"effort_floor": "high"}))
    fake_catalog(monkeypatch, [entry("codex-acp", "gpt-6.1-sol")])
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="owner"),
                          f"Use {TARGET}:medium.", "owner")
    request = auto_route.user_model_request(store, task.task_id, TARGET)
    with pytest.raises(PortableError, match="user_requested_effort_unavailable"):
        auto_route.choose("worker-explorer", tmp_path, env, model_hint=TARGET, user_request=request)


@pytest.mark.parametrize("text,effort", [
    (f"Use {TARGET}.", None),
    (f"Use {TARGET} high priority.", None),
    (f"Use {TARGET}, medium throughout.", "medium"),
    (f"Use {TARGET} / medium throughout.", "medium"),
    (f"Use {TARGET}. The report mentions high effort.", None),
    (f"Use {TARGET}:medium. Compare medium results with high results.", "medium"),
    (f"Use {TARGET} на medium.", "medium"),
])
def test_only_attached_effort_from_the_grant_source_is_selected(tmp_path, text, effort):
    install(tmp_path, settings("gpt-6.1-sol", tier="deep"))
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="owner"), text, "owner")
    request = auto_route.user_model_request(store, task.task_id, TARGET)
    assert request.get("effort") == effort


def test_new_source_replaces_effort_and_refusal_revokes_it(tmp_path):
    from cortex_runtime.bootstrap import append_source_event
    install(tmp_path, settings("gpt-6.1-sol", tier="deep"))
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="owner"), f"Use {TARGET}:high.", "owner")
    updated = append_source_event(store, SourceEvent(origin="test", event_id="new"), f"Use {TARGET}:medium.", task.task_id)
    request = auto_route.user_model_request(store, task.task_id, TARGET)
    assert (request["effort"], request["source_ref"]) == ("medium", f"{updated.report_id}:-1")
    append_source_event(store, SourceEvent(origin="test", event_id="refusal"), f"Do not use {TARGET}:medium.", task.task_id)
    assert auto_route.user_model_request(store, task.task_id, TARGET) is None


def test_conflicting_efforts_in_one_user_source_refuse(tmp_path):
    install(tmp_path, settings("gpt-6.1-sol", tier="deep"))
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="owner"),
                          f"Use {TARGET}:medium. Use {TARGET}:high.", "owner")
    with pytest.raises(PortableError, match="user_requested_effort_ambiguous"):
        auto_route.user_model_request(store, task.task_id, TARGET)


def test_answer_effort_binds_to_the_answer_not_the_question_or_another_task(tmp_path):
    from cortex_runtime.acp import questions_store
    install(tmp_path, settings("gpt-6.1-sol", tier="deep"))
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="first"), "Inspect safely.", "owner")
    question = questions_store.create_if_absent(
        store, task.task_id, "effort-choice", f"Use {TARGET}:high?", options=["Continue"])
    assert questions_store.answer(store, question.id, f"Use {TARGET}:medium.", "text")
    request = auto_route.user_model_request(store, task.task_id, TARGET)
    assert request["source_ref"] == f"{question.id}:answer"
    assert request["effort"] == "medium"
    other = bootstrap_task(store, SourceEvent(origin="test", event_id="other"), "Inspect safely.", "owner")
    assert auto_route.user_model_request(store, other.task_id, TARGET) is None


def test_configured_longer_identity_never_becomes_base_effort_consent(tmp_path):
    data = settings("gpt-6.1-sol", tier="deep")
    data["enabled"]["codex-acp"].append("gpt-6.1-sol/medium")
    data["models"]["codex-acp"]["tiers"]["gpt-6.1-sol/medium"] = "deep"
    install(tmp_path, data)
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="longer"),
                          f"Use {TARGET}/medium.", "owner")
    assert auto_route.user_model_request(store, task.task_id, TARGET) is None


def test_model_only_request_keeps_the_automatic_deep_effort(tmp_path, monkeypatch):
    env = install(tmp_path, settings("gpt-6.1-sol", tier="deep"))
    fake_catalog(monkeypatch, [entry("codex-acp", "gpt-6.1-sol")])
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="owner"), f"Use {TARGET}.", "owner")
    request = auto_route.user_model_request(store, task.task_id, TARGET)
    choice = auto_route.choose("worker-explorer", tmp_path, env, model_hint=TARGET, user_request=request)
    assert choice.effort == "high"


# Real plan -> provisioning -> wire -> confirmation/capsule, with fake engines only.
from test_auto_route_delegation import (
    _setup, _routing, _read_log, _effort_calls, _claude_usage_cli_unavailable,
)


@pytest.mark.parametrize("trigger", ["initial", "retry", "review_fix"])
def test_planned_effort_reaches_native_confirmation_and_exact_recovery_binding(tmp_path, monkeypatch, trigger):
    from cortex_runtime.bootstrap import append_source_event
    from cortex_runtime.orchestration.state import TaskMachine, event
    from cortex_runtime.routing import store as routing_store
    from cortex_runtime.recovery.capsule import build_capsule
    from test_runtime_task_machine import plan

    store, channel, provisioning, adapter, a, b, log_a, log_b, project, env = _setup(
        tmp_path, model_a="gpt-6.1-sol")
    source = append_source_event(store, SourceEvent(origin="test", event_id="medium"),
                                 f"Use {TARGET} at medium effort throughout.", channel.task_id)
    monkeypatch.setattr(auto_route, "route_menu", lambda *_: {"tiers": {"deep": [{"route": TARGET}]}})
    fixture = project / "fixture.txt"
    fixture.write_text("safe fixture")
    proposal = plan()
    proposal["tasks"][0].update(role="explorer", model=TARGET, model_reason="Owner request.", scope=["fixture.txt"])
    machine = TaskMachine(store)
    machine.create(channel.task_id, "Inspect fixture")
    preview = machine.submit_plan(channel, proposal, "plan")["model_hints"][0]
    assert preview["user_request"] == f"{source.report_id}:-1"
    machine.move(channel.task_id, "EXECUTING")
    node = machine.claim_ready(channel.task_id, channel.channel_id)[0]
    with store.connection() as db:
        if trigger == "retry":
            event(db, channel.task_id, "ASSIGNMENT_RETRIED", dict(node=node["node_id"], revision=1, retry=1))
        elif trigger == "review_fix":
            db.execute("UPDATE runtime_nodes SET node_id=? WHERE task_id=? AND assignment_ref=?",
                       ("_fix_1_" + node["node_id"], channel.task_id, node["assignment_ref"]))
        db.commit()
    result = provisioning.delegate(channel, "Inspect", "Read fixture.", "explorer", "Report.", node["request_key"])
    assert result.status == "ready"
    try:
        assert _read_log(log_b) == []
        assert _effort_calls(log_a)[-1]["params"]["value"] == "medium"
        audit = routing_store.list_audit_for_attempt(store, result.attempt_id)[0]
        detail = audit["decision_detail"]
        assert (detail["effort"], detail["effort_source"]) == ("medium", "user_request")
        assert detail["route_decision"]["user_model_request"]["source_ref"] == f"{source.report_id}:-1"
        capsule = build_capsule(store, channel.task_id,
                                dict(role="worker", assignment_ref=result.assignment_ref, attempt_id=result.attempt_id))
        own = capsule["sections"]["route_and_config"]["own_worker_route"]
        confirmed = own["worker_configuration_confirmed"]
        assert confirmed["attempt_id"] == result.attempt_id
        assert confirmed["engine_session_ref"] == result.engine_session_ref
        assert confirmed["parameters"].get("effort", confirmed["parameters"].get("thought_level")) == "medium"
        assert "medium" in confirmed["confirmation_evidence_ref"]
        wrong = build_capsule(store, channel.task_id,
                              dict(role="worker", assignment_ref="unrelated-assignment", attempt_id="unrelated-attempt"))
        assert wrong["sections"]["route_and_config"]["own_worker_route"]["worker_configuration_confirmed"]["status"] == "unavailable"
    finally:
        adapter.terminate_owned(result.attempt_id)


def test_mandatory_reviewer_keeps_its_owner_medium_pin(tmp_path):
    from cortex_runtime.routing import store as routing_store
    store, channel, provisioning, adapter, a, b, log_a, log_b, project, env = _setup(
        tmp_path, model_a="gpt-6.1-sol")
    _routing(project, {"pin": {"worker-code-reviewer": TARGET + ":medium"}})
    result = provisioning.delegate(channel, "Review", "Review it.", "code-reviewer", "Verdict.", "review-medium")
    assert result.status == "ready"
    try:
        assert _effort_calls(log_a)[-1]["params"]["value"] == "medium"
        audit = routing_store.list_audit_for_attempt(store, result.attempt_id)[0]
        assert audit["decision_detail"]["route_basis"] == "auto:pin"
    finally:
        adapter.terminate_owned(result.attempt_id)


@pytest.mark.parametrize("failure", ["policy", "catalog_support", "native_support", "native_mismatch"])
def test_explicit_effort_refusal_never_sends_a_worker_prompt(tmp_path, monkeypatch, failure):
    import json
    from pathlib import Path
    from cortex_runtime.bootstrap import append_source_event
    from cortex_runtime.orchestration import state
    from cortex_runtime.delegation.live_adapters import WorkerModeNotConfirmed
    from cortex_runtime.delegation.events import list_since

    store, channel, provisioning, adapter, a, b, log_a, log_b, project, env = _setup(
        tmp_path, model_a="gpt-6.1-sol",
        a={"configure_behavior_by_option": {"reasoning_effort": "mismatch"}} if failure == "native_mismatch" else None)
    append_source_event(store, SourceEvent(origin="test", event_id="medium"),
                        f"Use {TARGET}:medium.", channel.task_id)
    request = auto_route.user_model_request(store, channel.task_id, TARGET)
    monkeypatch.setattr(state, "attempt_routing", lambda *_: state.AttemptRouting(
        False, "normal", TARGET, "Owner request.", None, request))
    if failure == "policy":
        _routing(project, {"models": {"codex-acp": {"policy": {"gpt-6.1-sol": {"effort_floor": "high"}}}}})
    elif failure == "catalog_support":
        from cortex_runtime.catalog import store as catalog_store
        from cortex_runtime.catalog.discovery import build_catalog, parse_observation
        options = [
            dict(id="model", name="Model", category="model", type="select", currentValue="gpt-6.1-sol",
                 options=[dict(value="gpt-6.1-sol", name="Model")]),
            dict(id="thought_level", name="Effort", category="thought_level", type="select", currentValue="high",
                 options=[dict(value="high", name="High")]),
        ]
        catalog_store.save_snapshot(build_catalog({a.id: a}, {a.id: parse_observation(options)}), env)
    elif failure == "native_support":
        scenario_file = Path(a.launch_command[-1])
        scenario = json.loads(scenario_file.read_text())
        for option in scenario["config_options"]:
            if option["id"] == "reasoning_effort":
                option["options"] = [{"value": "high", "name": "High"}]
        scenario_file.write_text(json.dumps(scenario))
    error = PortableError if failure in {"policy", "catalog_support"} else WorkerModeNotConfirmed
    with pytest.raises(error):
        provisioning.delegate(channel, "Inspect", "Read fixture.", "explorer", "Report.", "strict-refusal")
    assert not any(call["method"] == "session/prompt" for call in _read_log(log_a) + _read_log(log_b))
    assert not any(e.payload.get("kind") == "worker_configuration_confirmed"
                   for e in list_since(store, channel.task_id))
    if failure in {"policy", "catalog_support"}:
        assert _read_log(log_a) == [] and _read_log(log_b) == []


def test_saved_explicit_effort_cannot_be_clamped_after_policy_change(tmp_path):
    from cortex_runtime.configuration.service import ConfigurationService
    store, channel, provisioning, adapter, a, b, log_a, log_b, project, env = _setup(
        tmp_path, model_a="gpt-6.1-sol")
    row = ConfigurationService(store).current(channel.task_id)
    _routing(project, {"models": {"codex-acp": {"policy": {"gpt-6.1-sol": {"effort_floor": "high"}}}}})
    launch = dict(config_revision=row["id"], task_id=channel.task_id, assignment_id="saved",
                  resolved_route_id=row["route_id"], resolved_model="gpt-6.1-sol",
                  parameters={"effort": "medium"}, effort_source="user_request")
    with pytest.raises(PortableError, match="user_requested_effort_unavailable"):
        adapter._authorize_dispatch(a, channel.task_id, launch, str(project))
    assert launch["parameters"]["effort"] == "medium"
    assert _read_log(log_a) == [] and _read_log(log_b) == []


def test_auth_replacement_retains_explicit_effort_strictness(tmp_path, monkeypatch):
    import json
    from cortex_runtime.orchestration import auth_recovery
    from cortex_runtime.configuration.service import ConfigurationService
    store, channel, provisioning, adapter, a, b, log_a, log_b, project, env = _setup(
        tmp_path, model_a="gpt-6.1-sol")
    row = ConfigurationService(store).current(channel.task_id)
    prior = dict(engine="codex-acp", connection_id=a.id, model="gpt-6.1-sol",
                 decision_detail_json=json.dumps(dict(effort="medium", effort_source="user_request",
                     route_decision={"user_model_request": {"source_ref": "retained-owner-source", "effort": "medium"}})))
    monkeypatch.setattr(auth_recovery, "worker_route", lambda *_: prior)
    launch = dict(config_revision=row["id"], task_id=channel.task_id, attempt_id="replacement",
                  assignment_id="saved", role_id="worker-explorer", parameters={"effort": "high"})
    adapter._resolve_runtime_route(launch, str(project))
    assert launch["parameters"]["effort"] == "medium"
    assert launch["effort_source"] == "user_request"
    assert launch["route_decision"]["user_model_request"]["source_ref"] == "retained-owner-source"
    _routing(project, {"models": {"codex-acp": {"policy": {"gpt-6.1-sol": {"effort_floor": "high"}}}}})
    with pytest.raises(PortableError, match="user_requested_effort_unavailable"):
        adapter._authorize_dispatch(a, channel.task_id, launch, str(project))
    assert _read_log(log_a) == [] and _read_log(log_b) == []


def test_reviewer_pin_native_effort_mismatch_never_becomes_confirmation(tmp_path):
    from cortex_runtime.delegation.live_adapters import WorkerModeNotConfirmed
    from cortex_runtime.delegation.events import list_since
    store, channel, provisioning, adapter, a, b, log_a, log_b, project, env = _setup(
        tmp_path, model_a="gpt-6.1-sol",
        a={"configure_behavior_by_option": {"reasoning_effort": "mismatch"}})
    _routing(project, {"pin": {"worker-code-reviewer": TARGET + ":medium"}})
    with pytest.raises(WorkerModeNotConfirmed):
        provisioning.delegate(channel, "Review", "Review it.", "code-reviewer", "Verdict.", "pin-native-mismatch")
    assert not any(call["method"] == "session/prompt" for call in _read_log(log_a) + _read_log(log_b))
    assert not any(e.payload.get("kind") == "worker_configuration_confirmed"
                   for e in list_since(store, channel.task_id))


def test_coordinator_editor_effort_is_confirmed_without_automatic_worker_routing(tmp_path):
    from cortex_runtime.configuration.service import ConfigurationService
    from cortex_runtime.configuration.resolution import ConfigurationInputs
    from cortex_runtime.portable.identity import OwnershipEpoch
    from test_auto_route_delegation import _snapshot

    store, channel, provisioning, adapter, a, b, log_a, log_b, project, env = _setup(
        tmp_path, model_a="gpt-6.1-sol")
    service = ConfigurationService(store)
    current = service.current(channel.task_id)
    model_entry = _snapshot(a, "gpt-6.1-sol").entries[0]
    row = service.propose(channel.task_id, model_entry,
                          ConfigurationInputs(route=model_entry.route, parameters={"thought_level": "medium"}),
                          current["revision"], OwnershipEpoch(epoch=channel.owner_epoch, owner="owner-1"))
    launch = dict(task_id=channel.task_id, attempt_id="co_medium", config_revision=row["id"])
    session = adapter.open("shared_project_root", launch, "Coordinate only.")
    try:
        assert _effort_calls(log_a)[-1]["params"]["value"] == "medium"
        active = service.current(channel.task_id)
        assert active["state"] == "active"
        assert active["parameters"]["thought_level"] == "medium"
        assert "medium" in active["confirmation_evidence_ref"]
        assert launch["effort_source"] == "configuration_or_request"
        assert _read_log(log_b) == []
    finally:
        adapter.terminate_owned("co_medium")
