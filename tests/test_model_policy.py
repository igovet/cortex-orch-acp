"""routing.model_policy: configured selection, tiers and per-model policy
over the built-in defaults."""
from __future__ import annotations

from types import SimpleNamespace

import pytest

from cortex_runtime.catalog.model_seed import SEED_MODELS
from cortex_runtime.connections.model import SUPPORTED_ENGINES
from cortex_runtime.routing import auto_route, model_policy


def _entry(connection, engine, model):
    route = SimpleNamespace(id=f"{connection}:{model}", engine=engine, model=model, supported_parameters=())
    return SimpleNamespace(connection_id=connection, route=route)


# The tiers the name tables gave before routing.json ``models`` existed.
PREVIOUS_TIERS = {
    "gpt-6-astra": "flagship", "gpt-6-sol": "deep", "gpt-6.1-sol": "deep", "gpt-6-luna": "light", "gpt-5.6-sol": "standard",
    "gpt-5.6-terra": "standard", "gpt-5.6-luna": "light", "gpt-5.5": "light", "gpt-7-luna": "light",
    "gpt-7-terra": "standard", "sonnet": "standard", "opus": "deep", "opus[1m]": "deep",
    "claude-opus-4-8": "deep", "claude-fable-5-1[1m]": "flagship", "claude-sonnet-5": "standard",
    "haiku": "light", "claude-haiku-4-5-20251001": "light", "default": None, "anthropic/claude-x": None,
}


def test_the_builtin_tables_keep_every_previous_tier():
    for engine, models in SEED_MODELS.items():
        for model, _name in models:
            assert model_policy.builtin_tier(model) == PREVIOUS_TIERS[model], (engine, model)
    for model, tier in PREVIOUS_TIERS.items():
        assert model_policy.builtin_tier(model) == tier, model
    assert auto_route.FAMILY_TIERS is model_policy.BUILTIN_FAMILY_TIERS
    assert auto_route.LUNA_WORK_KINDS == frozenset({"testing"})
    assert auto_route.DEFAULT_AUTO_DENY_ENGINES == frozenset({"opencode"})


@pytest.mark.parametrize("model,expected", [
    ("gpt-6-astra", dict(max_effort="high", request_names=("астр",))),
    ("claude-fable-5-1[1m]", dict(max_effort="high", request_names=("фейбл", "фэйбл"))),
    ("gpt-7-luna", dict(effort_floor="high", effort_ladder=("high", "xhigh", "max"),
                        also_standard_for=frozenset({"testing"}))),
    ("claude-mythos-1", dict(request_names=("мифос", "майтос", "митос"))),
    ("claude-sonnet-5", {}),
    ("anthropic/claude-sonnet-4-5", {}),
])
def test_builtin_attributes_are_the_previous_name_rules(model, expected):
    assert model_policy.builtin_attributes(model) == expected


def test_tier_precedence_and_its_source():
    entries = [_entry("codex", "codex-acp", model) for model in ("gpt-6-luna", "gpt-5.6-terra", "gpt-6-sol")]
    entries += [_entry("opencode", "opencode", model) for model in ("k/one", "k/two", "k/three")]
    overrides = {
        "tiers": {"gpt-5.6-terra": "deep", "gpt-6-sol": "light", "k/two": "standard"},
        "models": {"codex-acp": {"tiers": {"gpt-6-sol": "flagship"}}},
    }
    policies = model_policy.resolve_policies(entries, overrides)
    assert {route: (p.tier, p.tier_source) for route, p in policies.items()} == {
        "codex:gpt-6-luna": ("light", "builtin"),
        "codex:gpt-5.6-terra": ("deep", "legacy_tiers"),
        "codex:gpt-6-sol": ("flagship", "configured"),
        "opencode:k/two": ("standard", "legacy_tiers"),
        # Unplaced models keep their position among the unplaced of their list.
        "opencode:k/one": ("deep", "position"),
        "opencode:k/three": ("standard", "position"),
    }
    # A tier configured for another engine never leaks across engines.
    other = model_policy.resolve_policies(entries, {"models": {"opencode": {"tiers": {"gpt-6-sol": "light"}}}})
    assert other["codex:gpt-6-sol"].tier == "deep"


def test_configured_attributes_overlay_the_builtin_ones_key_by_key():
    overrides = {"models": {"codex-acp": {"policy": {"gpt-6-luna": {
        "effort_floor": None, "max_effort": "xhigh", "preferred_for": ["testing", "review"]}}}}}
    policy = model_policy.policy_for("codex-acp", "gpt-6-luna", overrides)
    assert policy.effort_floor is None and policy.max_effort == "xhigh"
    assert policy.effort_ladder == ("high", "xhigh", "max")
    assert policy.preferred_for == frozenset({"testing", "review"})
    assert policy.attribute_sources == dict(max_effort="configured", effort_ladder="builtin",
                                            also_standard_for="builtin", preferred_for="configured")
    assert policy.stretch and policy.tier == "light"
    assert policy.attributes()["preferred_for"] == ["review", "testing"]
    # The same model on another engine keeps its built-in policy.
    assert model_policy.policy_for("opencode", "gpt-6-luna", overrides).effort_floor == "high"


def test_selection_modes():
    overrides = {"models": {"opencode": {"auto": ["k/one"]}, "codex-acp": {"auto": []}}}
    assert model_policy.selection_mode(overrides, "opencode") == "listed"
    assert model_policy.selection_mode(overrides, "claude-agent-acp") == "builtin"
    assert model_policy.is_selected(overrides, "opencode", "k/one") is True
    assert model_policy.is_selected(overrides, "opencode", "k/two") is False
    assert model_policy.is_selected(overrides, "codex-acp", "gpt-6-luna") is False
    assert model_policy.is_selected(overrides, "claude-agent-acp", "sonnet") is None
    policy = model_policy.policy_for("opencode", "k/two", overrides)
    assert (policy.selection, policy.selected, policy.tier) == ("listed", False, None)


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
