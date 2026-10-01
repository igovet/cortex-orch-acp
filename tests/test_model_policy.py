"""routing.model_policy: configured selection, tiers and per-model policy
over the built-in defaults."""
from __future__ import annotations

from types import SimpleNamespace

import pytest

from model_policy_fixtures import MODELS
from cortex_runtime.connections.model import SUPPORTED_ENGINES
from cortex_runtime.routing import auto_route, model_policy


def _entry(connection, engine, model):
    route = SimpleNamespace(id=f"{connection}:{model}", engine=engine, model=model, supported_parameters=())
    return SimpleNamespace(connection_id=connection, route=route)


def test_names_and_catalog_order_never_supply_policy():
    identities = ["gpt-6-astra", "gpt-7-luna", "opus[1m]", "default", "arbitrary/new-id"]
    entries = [_entry("test", "codex-acp", model) for model in identities]
    for ordered in (entries, entries[::-1]):
        policies = model_policy.resolve_policies(ordered, {})
        assert all(p.tier is None and p.tiers == () and not p.selected for p in policies.values())
        assert all(p.effort_floor is None and p.max_effort is None and not p.request_names for p in policies.values())


def test_only_exact_per_engine_tiers_apply():
    entries = [_entry("a", "codex-acp", "renamed-1"), _entry("b", "opencode", "renamed-1")]
    overrides = {"tiers": {"renamed-1": "flagship"},
                 "models": {"codex-acp": {"auto": ["renamed-1"], "tiers": {"renamed-1": "light"}}}}
    policies = model_policy.resolve_policies(entries, overrides)
    assert (policies["a:renamed-1"].tier, policies["a:renamed-1"].tier_source) == ("light", "configured")
    assert policies["b:renamed-1"].tier is None
    assert policies["a:renamed-1"].selected and not policies["b:renamed-1"].selected


@pytest.mark.parametrize("fields", [{}, {"effort_floor": None, "effort_ladder": None, "request_names": None}])
def test_missing_and_null_attributes_are_neutral(fields):
    config = {"models": {"codex-acp": {"policy": {"gpt-6-luna": fields}, "tiers": {"gpt-6-luna": "light"}}}}
    p = model_policy.policy_for("codex-acp", "gpt-6-luna", config)
    assert p.effort_floor is None and p.effort_ladder == () and not p.stretch
    assert p.request_names == () and p.attribute_sources == {}
    assert model_policy.policy_for("opencode", "gpt-6-luna", config).tier is None


def test_selection_is_always_exact_and_missing_lists_select_nothing():
    overrides = {"models": {"opencode": {"auto": ["k/one"]}, "codex-acp": {"auto": []}}}
    assert model_policy.selection_mode(overrides, "claude-agent-acp") == "listed"
    assert model_policy.is_selected(overrides, "opencode", "k/one")
    for engine, model in [("opencode", "k/two"), ("codex-acp", "k/one"), ("claude-agent-acp", "sonnet")]:
        assert not model_policy.is_selected(overrides, engine, model)


def test_stretch_effort_steps_along_the_ladder():
    ladder = ("high", "xhigh", "max")
    assert [model_policy.stretch_effort(ladder, tier, escalate) for tier, escalate in (
        ("light", False), ("light", True), ("standard", False), ("standard", True))] == [
        "high", "xhigh", "xhigh", "max"]
    assert model_policy.stretch_effort(("medium",), "standard", True) == "medium"
    assert model_policy.stretch_effort((), "light") is None


def test_validation_reports_every_problem_with_its_field():
    problems = model_policy.validate_models(
        {"codex-acp": {"auto": ["x", "x"], "tiers": {"y": "huge"},
                       "policy": {"z": {"effort_ladder": ["max"], "request_names": ["use", "Bad"]}}},
         "opencode": {"auto": ["plain"]},
         "nope": {}},
        engines=SUPPORTED_ENGINES, work_kinds=tuple(auto_route.WORK_KIND_ENGINES),
        reserved_word=auto_route._reserved_request_word,
        exact_model=lambda engine, model: engine != "opencode" or "/" in model)
    assert [(where, code) for where, code, _message in problems] == [
        ("models.codex-acp.auto[1]", "duplicate_model"),
        ("models.codex-acp.tiers.y", "invalid_value"),
        ("models.codex-acp.policy.z.request_names[0]", "reserved_word"),
        ("models.codex-acp.policy.z.request_names[1]", "invalid_value"),
        ("models.opencode.auto[0]", "model_not_exact"),
        ("models.nope", "unknown_engine"),
    ]
    valid = {"opencode": {"auto": ["anthropic/claude-sonnet-4-5"],
                          "tiers": {"anthropic/claude-sonnet-4-5": "standard"},
                          "policy": {"anthropic/claude-sonnet-4-5": {
                              "effort_floor": None, "max_effort": "high", "effort_ladder": ["low", "high"],
                              "also_standard_for": [], "preferred_for": ["design"],
                              "request_names": ["кими", "opencode"]}}}}
    assert model_policy.validate_models(valid, engines=SUPPORTED_ENGINES,
                                        work_kinds=tuple(auto_route.WORK_KIND_ENGINES),
                                        reserved_word=auto_route._reserved_request_word) == []


def test_preferred_model_must_be_selected_when_auto_list_is_explicit():
    rules = {"models": {"opencode": {"auto": ["zai-coding-plan/glm-5.3"],
                                        "policy": {"zai-coding-plan/glm-5.2": {
                                            "preferred_for": ["design"]}}}}}
    problems = model_policy.validate_models(
        rules["models"], engines=SUPPORTED_ENGINES, work_kinds=model_policy.WORK_KINDS,
        exact_model=lambda engine, model: engine != "opencode" or "/" in model,
        prefix="rules.models")
    assert ("rules.models.opencode.policy.zai-coding-plan/glm-5.2.preferred_for",
            "preferred_not_selected") in {(field, code) for field, code, _ in problems}
    rules["models"]["opencode"].pop("auto")
    assert model_policy.validate_models(rules["models"], engines=SUPPORTED_ENGINES,
                                        work_kinds=model_policy.WORK_KINDS) == []


def test_a_configured_tier_list_places_a_model_in_each_tier():
    overrides = {"models": {"codex-acp": {"tiers": {"gpt-6-sol": ["standard", "flagship"]}}}}
    policy = model_policy.policy_for("codex-acp", "gpt-6-sol", overrides)
    assert (policy.tier, policy.tiers, policy.tier_source) == ("flagship", ("flagship", "standard"), "configured")
    assert model_policy.policy_for("codex-acp", "gpt-6-sol").tiers == ()
    problems = model_policy.validate_models(
        {"codex-acp": {"tiers": {"a": [], "b": ["deep", "deep"], "c": ["huge"], "d": ["light", "deep"]}}},
        engines=("codex-acp",), work_kinds=(), reserved_word=lambda word: False)
    assert {where for where, _code, _message in problems} == {
        "models.codex-acp.tiers.a", "models.codex-acp.tiers.b", "models.codex-acp.tiers.c"}
