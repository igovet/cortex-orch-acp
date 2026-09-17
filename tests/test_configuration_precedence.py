"""Parametrized layer precedence: all 15 non-empty subsets of
{user, project, task, assignment} (T008 lead-required addition A)."""
from __future__ import annotations

from itertools import combinations

import pytest

from cortex_runtime.configuration.resolution import merge_layers, rederive_on_route_change
from cortex_runtime.portable.identity import ExecutionRoute

LAYER_NAMES = ("user", "project", "task", "assignment")
# most general first, most specific last -- matches merge_layers' contract.
_ORDER = {"user": 0, "project": 1, "task": 2, "assignment": 3}


def _subsets():
    subsets = []
    for size in range(1, len(LAYER_NAMES) + 1):
        for combo in combinations(LAYER_NAMES, size):
            subsets.append(tuple(sorted(combo, key=_ORDER.get)))
    return subsets


ALL_15_SUBSETS = _subsets()


def test_there_are_exactly_fifteen_non_empty_subsets():
    assert len(ALL_15_SUBSETS) == 15


@pytest.mark.parametrize("present_layers", ALL_15_SUBSETS, ids=lambda combo: "+".join(combo))
def test_most_specific_present_layer_wins_and_unset_fields_inherit(present_layers):
    # Every present layer sets BOTH its own uniquely-named field (marking
    # "this layer contributed something") AND the shared "effort" field
    # (so we can assert which layer actually won it).
    layer_values = {
        "user": {"effort": "user-value", "user_only": "u"},
        "project": {"effort": "project-value", "project_only": "p"},
        "task": {"effort": "task-value", "task_only": "t"},
        "assignment": {"effort": "assignment-value", "assignment_only": "a"},
    }
    layers = [layer_values[name] if name in present_layers else None for name in LAYER_NAMES]
    merged = merge_layers(*layers)

    # most-specific present layer wins "effort".
    most_specific = present_layers[-1]
    assert merged["effort"] == layer_values[most_specific]["effort"]

    # Every present layer's own uniquely-named field survives (inheritance
    # of fields no more specific layer overrides).
    for name in present_layers:
        key = f"{name}_only"
        assert merged[key] == layer_values[name][key]

    # A layer that is absent never contributes a field.
    for name in LAYER_NAMES:
        if name not in present_layers:
            assert f"{name}_only" not in merged


@pytest.mark.parametrize("present_layers", ALL_15_SUBSETS, ids=lambda combo: "+".join(combo))
def test_dependent_field_precedence_per_present_layer_combination(present_layers):
    # A second dependent-style field ("mode") is set by only a subset of the
    # present layers each time, to also exercise "no present layer sets it"
    # for some parametrizations (still must simply stay absent, never
    # defaulted to some other layer's unrelated value).
    contributes_mode = {"user", "project", "assignment"}
    layer_values = {
        "user": {"mode": "user-mode"},
        "project": {"mode": "project-mode"} if "project" in contributes_mode else {},
        "task": {},
        "assignment": {"mode": "assignment-mode"} if "assignment" in contributes_mode else {},
    }
    layers = [layer_values[name] if name in present_layers else None for name in LAYER_NAMES]
    merged = merge_layers(*layers)
    mode_contributors = [name for name in present_layers if name in contributes_mode]
    if mode_contributors:
        winner = mode_contributors[-1]
        assert merged["mode"] == layer_values[winner]["mode"]
    else:
        assert "mode" not in merged


def test_route_change_re_derivation_is_independent_of_which_layers_are_present():
    route_a = ExecutionRoute(engine="codex-acp", connection_id="cn_" + "a" * 12, model="model-a")
    route_b = ExecutionRoute(engine="codex-acp", connection_id="cn_" + "a" * 12, model="model-b")
    layers = {
        "user": {"effort": "high"},
        "project": {"mode": "plan"},
        "task": {"effort": "high", "mode": "plan", "custom": "kept"},
        "assignment": {},
    }
    merged = merge_layers(layers["user"], layers["project"], layers["task"], layers["assignment"])
    same_route = rederive_on_route_change(route_a.id, route_a, merged)
    assert same_route == merged  # no route change: nothing dropped
    changed_route = rederive_on_route_change(route_a.id, route_b, merged)
    assert changed_route == {"custom": "kept"}  # dependent settings re-derived, engine-agnostic field kept
