"""Structured work classification survives plan admission, retries and fixes."""
import json
from types import SimpleNamespace

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.orchestration.contracts import NODE, validate
from cortex_runtime.orchestration.state import attempt_routing, validate_plan
from cortex_runtime.routing import auto_route, model_policy

from test_auto_route import _live_choose, LIVE_CODEX, LIVE_CLAUDE
from test_fix_node_role import _review, _fix, _provision as provision_fix
from test_runtime_task_machine import machine, plan, _provision


DESIGN = "implementation_frontend_design"


@pytest.mark.parametrize("kind", model_policy.WORK_KINDS)
def test_schema_and_runtime_accept_every_current_kind(machine, kind):
    state, channel = machine
    proposal = plan()
    proposal["tasks"][0]["work_kind"] = kind
    validate(NODE, proposal["tasks"][0], "task")
    assert validate_plan(proposal) == proposal
    result = state.submit_plan(channel, proposal, "kind")
    assert result["model_hints"][0]["work_kind"] == kind
    with state.store.connection() as db:
        saved = json.loads(db.execute("SELECT plan_json FROM runtime_tasks WHERE task_id=?",
                                      (channel.task_id,)).fetchone()[0])
    assert saved == proposal


@pytest.mark.parametrize("kind", ["implementation", "unknown", "", None, [], {}, True, 1])
def test_invalid_override_rejected_atomically(machine, kind):
    state, channel = machine
    proposal = plan()
    proposal["tasks"][0]["work_kind"] = kind
    with pytest.raises(StoreError):
        validate(NODE, proposal["tasks"][0], "task")
    with pytest.raises(StoreError):
        state.submit_plan(channel, proposal, "invalid")
    assert state.context(channel.task_id)["state"] == "PLANNING"


def test_override_reaches_attempt_dispatch_and_retry(machine, monkeypatch):
    state, channel = machine
    proposal = plan()
    proposal["tasks"][0].update(role="frontend-dev", work_kind=DESIGN)
    accepted = state.submit_plan(channel, proposal, "design")
    state.move(channel.task_id, "EXECUTING")
    state.claim_ready(channel.task_id, channel.channel_id)
    _provision(state, channel, "N0", "xa_design")
    route = attempt_routing(state.store, channel.task_id, "xa_design")
    assert route.work_kind == DESIGN and not route.escalate
    from cortex_runtime.orchestration.state import event
    with state.store.connection() as db:
        event(db, channel.task_id, "ASSIGNMENT_RETRIED", dict(node="N0", revision=accepted["revision"]))
        db.commit()
    route = attempt_routing(state.store, channel.task_id, "xa_design")
    assert route.work_kind == DESIGN and route.escalate
    seen = {}
    def choose(*args, **kwargs):
        seen.update(kwargs)
        return auto_route.Choice("cn_test", "test", "high", "auto", {}, "route")
    monkeypatch.setattr(auto_route, "choose", choose)
    adapter = SimpleNamespace(_store=state.store, _environ={}, _dispatch_configuration=lambda launch: None)
    launch = dict(assignment_id="assignment", task_id=channel.task_id,
                  attempt_id="xa_design", role_id="worker-frontend-dev")
    EngineSessionManagerAdapter._resolve_runtime_route(adapter, launch, str(state.store.project_root))
    assert seen["work_kind_override"] == DESIGN and seen["escalate"]


@pytest.mark.parametrize("override", [None, DESIGN])
def test_fix_uses_new_role_default_unless_original_explicitly_classified(tmp_path, override):
    fields = {} if override is None else {"work_kind": override}
    state, channel, review = _review(tmp_path, "architect", ["src/**"], **fields)
    _fix(state, channel, review)
    (node,) = state.claim_ready(channel.task_id, channel.channel_id)
    provision_fix(state, channel, node, "xa_fix")
    routing = attempt_routing(state.store, channel.task_id, "xa_fix")
    assert routing.escalate and routing.work_kind == override
    assert auto_route.work_kind(node["role"], routing.work_kind) == (override or "implementation_backend")


def test_override_controls_ranking_stretch_and_menu(tmp_path, monkeypatch):
    rules = {"models": {
        "claude-agent-acp": {"policy": {"opus[1m]": {"preferred_for": [DESIGN]}},
                            "tiers": {"opus[1m]": ["standard", "deep"]}},
        "codex-acp": {"policy": {"gpt-6-luna": {"also_standard_for": ["implementation_frontend"]}}}}}
    project, environ = _live_choose(monkeypatch, tmp_path, LIVE_CODEX + LIVE_CLAUDE, rules)
    choice = auto_route.choose("worker-frontend-dev", project, environ, work_kind_override=DESIGN)
    assert choice.model == "opus[1m]"
    assert choice.decision["purpose"]["work_kind"] == DESIGN
    assert choice.decision["affinity"]["engine"] == "claude-agent-acp"
    assert not choice.decision["stretch"]["candidate"]
    menu = auto_route.route_menu(project, environ)
    assert menu["role_work_kinds"]["worker-frontend-dev"] == "implementation_frontend"
    assert menu["affinity"][DESIGN] == "claude-agent-acp"
    assert menu["preferred"][DESIGN] == ["claude-agent-acp/opus[1m]"]
    assert auto_route.hint_serves_tier("custom", "light", "worker-frontend-dev", "standard",
                                     also_standard_for=[DESIGN], work_kind_override=DESIGN)
    assert not auto_route.hint_serves_tier("custom", "light", "worker-frontend-dev", "standard",
                                         also_standard_for=["implementation_frontend"], work_kind_override=DESIGN)
