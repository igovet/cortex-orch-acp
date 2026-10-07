"""Owner model priority and inclusive usage-switch boundary regression tests."""
import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.routing import auto_route
from cortex_runtime.store import Store
from test_settings_only_routing import fake_catalog, install, settings
from test_model_settings_template import entry


@pytest.mark.parametrize("tier", ["light", "standard", "deep", "flagship"])
@pytest.mark.parametrize("escalate", [False, True])
def test_recorded_request_overrides_pin_preferences_tier_and_usage(tmp_path, monkeypatch, tier, escalate):
    from cortex_runtime.connections import quota
    target = "codex-acp/requested"
    data = settings("requested", tier=tier)
    # Even a non-automatic member is usable when explicitly requested.
    data["models"]["codex-acp"]["auto"] = []
    data["enabled"]["claude-agent-acp"] = ["ordinary"]
    data["models"]["claude-agent-acp"] = {
        "auto": ["ordinary"], "tiers": {"ordinary": "standard"},
        "policy": {"ordinary": {"preferred_for": ["implementation"]}}}
    data["pin"] = {"*": "claude-agent-acp/ordinary"}
    env = install(tmp_path, data)
    observed = [entry("codex-acp", "requested"), entry("claude-agent-acp", "ordinary")]
    fake_catalog(monkeypatch, observed)
    monkeypatch.setattr(quota, "used_percent",
                        lambda _conn, model, *_, **_kw: 99.9 if model == "requested" else 0.0)
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="owner"), f"Use {target}", "owner")
    request = auto_route.user_model_request(store, task.task_id, target)
    assert request["source_ref"] == f"{task.report_id}:-1"
    choice = auto_route.choose("worker-backend-dev", tmp_path, env, complexity="high",
                               model_hint=target, user_request=request, escalate=escalate,
                               preference="claude-agent-acp", previous_engine="codex-acp")
    assert (choice.model, choice.basis, choice.decision["tier"]) == ("requested", "auto:user_request", tier)
    assert choice.decision["user_model_request"] == dict(request, status="honoured")


def test_request_cannot_bypass_route_authorization_ceiling(tmp_path, monkeypatch):
    env = install(tmp_path, settings("requested", tier="deep"))
    fake_catalog(monkeypatch, [entry("codex-acp", "requested")])
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="owner"),
                          "Use codex-acp/requested", "owner")
    request = auto_route.user_model_request(store, task.task_id, "codex-acp/requested")
    with pytest.raises(PortableError, match="user_requested_route_unavailable") as caught:
        auto_route.choose("worker-backend-dev", tmp_path, env, model_hint="codex-acp/requested",
                          user_request=request, eligible_route_ids=frozenset())
    assert "authorization ceiling" in caught.value.remediation


@pytest.mark.parametrize("used,expected,status", [
    (0.0, "codex-acp", "below_threshold"),
    (85.0, "codex-acp", "below_threshold"),
    (94.999, "codex-acp", "below_threshold"),
    (95.0, "claude-agent-acp", "active"),
    (99.999, "claude-agent-acp", "active"),
    (None, "codex-acp", "unavailable"),
])
def test_usage_switch_starts_only_at_observed_95_percent(used, expected, status):
    observed = [entry("codex-acp", "first"), entry("claude-agent-acp", "second")]
    policies = {"codex-acp": {"auto": ["first"], "tiers": {"first": "standard"}},
                "claude-agent-acp": {"auto": ["second"], "tiers": {"second": "standard"}}}
    picked, decision = auto_route.rank(
        "standard", observed, lambda e: used if e.route.engine == "codex-acp" else 0.0,
        role="worker-backend-dev", models=policies)
    assert picked.route.engine == expected
    assert decision["usage"]["status"] == status
    assert decision["usage"]["threshold"] == auto_route.USAGE_SWITCH_PERCENT == 95.0
    if status != "active":
        assert not decision["usage"]["active"]
        assert all(e["usage_band"] is None for e in decision["candidates"])
        assert decision["decided_by"] == "affinity"


def test_non_flagship_plan_preview_carries_recorded_source(tmp_path, monkeypatch):
    from types import SimpleNamespace
    from cortex_runtime.orchestration.state import TaskMachine
    from test_runtime_task_machine import plan

    install(tmp_path, settings("requested", tier="deep"))
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="owner"),
                          "Use codex-acp/requested", "owner")
    machine = TaskMachine(store)
    machine.create(task.task_id, "Build")
    monkeypatch.setattr(auto_route, "route_menu", lambda *_: {
        "tiers": {"deep": [{"route": "codex-acp/requested"}]}})
    proposal = plan()
    proposal["tasks"][0].update(model="codex-acp/requested", model_reason="User choice.")
    channel = SimpleNamespace(task_id=task.task_id, channel_id="coordinator", role="coordinator")
    hint = machine.submit_plan(channel, proposal, "requested-plan")["model_hints"][0]
    assert hint["preview_status"] == "pending_dispatch:user_request"
    assert hint["user_request"] == f"{task.report_id}:-1"


@pytest.mark.parametrize("ambiguous", [False, True])
def test_plan_preview_explains_bare_user_model_and_ambiguity(tmp_path,monkeypatch,ambiguous):
    from types import SimpleNamespace
    from cortex_runtime.orchestration.state import TaskMachine
    from test_runtime_task_machine import plan
    data=settings("requested",tier="deep")
    if ambiguous:
        data["enabled"]["claude-agent-acp"]=["requested"]
        data["models"]["claude-agent-acp"]={"auto":["requested"],"tiers":{"requested":"deep"}}
    install(tmp_path,data)
    store=Store(tmp_path/".cortex",project_root=tmp_path)
    task=bootstrap_task(store,SourceEvent(origin="test",event_id="bare-preview"),
                        "Use requested.", "owner")
    machine=TaskMachine(store)
    machine.create(task.task_id,"Build")
    monkeypatch.setattr(auto_route,"route_menu",lambda *_: {
        "tiers":{"deep":[{"route":"codex-acp/requested"}]}})
    proposal=plan()
    proposal["tasks"][0].update(model="codex-acp/requested",model_reason="User choice.")
    channel=SimpleNamespace(task_id=task.task_id,channel_id="coordinator",role="coordinator")
    hint=machine.submit_plan(channel,proposal,"bare-plan")["model_hints"][0]
    if ambiguous:
        assert hint["preview_status"] == "ignored:tier_mismatch"
        assert "user_request" not in hint
        assert hint["ambiguous_user_request"] == f"{task.report_id}:-1"
        assert "ambiguously without its engine" in hint["user_request_diagnostic"]
        assert "engine/model" in hint["user_request_diagnostic"]
    else:
        assert hint["preview_status"] == "pending_dispatch:user_request"
        assert hint["user_request"] == f"{task.report_id}:-1"
        assert "without its engine" in hint["user_request_diagnostic"]
        assert "uniquely matches" in hint["user_request_diagnostic"]


def test_bare_recorded_choice_still_precedes_automatic_routing(tmp_path,monkeypatch):
    data=settings("gpt-6.1-sol",tier="deep")
    data["models"]["codex-acp"]["auto"]=[]
    env=install(tmp_path,data)
    fake_catalog(monkeypatch,[entry("codex-acp","gpt-6.1-sol")])
    store=Store(tmp_path/".cortex",project_root=tmp_path)
    task=bootstrap_task(store,SourceEvent(origin="test",event_id="bare-choice"),
                        "Use gpt-6.1-sol.", "owner")
    target="codex-acp/gpt-6.1-sol"
    request=auto_route.user_model_request(store,task.task_id,target)
    choice=auto_route.choose("worker-backend-dev",tmp_path,env,model_hint=target,user_request=request)
    assert (choice.model,choice.basis) == ("gpt-6.1-sol","auto:user_request")
