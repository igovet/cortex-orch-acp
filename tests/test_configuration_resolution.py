"""Layer merge, route-change re-derivation, dependent-settings bundling (T008)."""
from __future__ import annotations

import dataclasses

import pytest

from cortex_runtime.catalog.discovery import build_catalog, parse_observation
from cortex_runtime.catalog.model import CatalogEntry
from cortex_runtime.configuration.resolution import (
    ConfigurationInputs,
    merge_layers,
    rederive_on_route_change,
    resolve_saved_route,
    validate_dependent_settings,
)
from cortex_runtime.connections.model import AuthMode, ConnectionRecord, ConnectionState, EngineNativeDir
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import ExecutionRoute


def _connection(connection_id="cn_" + "a" * 12):
    now = "2026-01-01T00:00:00+00:00"
    return ConnectionRecord(
        id=connection_id, name="Work", engine="codex-acp", provider="acme", auth_mode=AuthMode.NATIVE_ACCOUNT,
        key_kind=None, endpoint=None, launch_command=None,
        credential_ref=EngineNativeDir(path=f"/tmp/{connection_id}"), approved_fingerprint="fp",
        state=ConnectionState.CONFIGURED, qualified=True, created_at=now, updated_at=now,
    )


def _entry_with_effort_levels(*levels, connection=None):
    connection = connection or _connection()
    options = [
        {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "model-a",
         "options": [{"value": "model-a", "name": "Model A"}]},
        {"id": "effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": levels[0],
         "options": [{"value": level, "name": level} for level in levels]},
    ]
    snapshot = build_catalog({connection.id: connection}, {connection.id: parse_observation(options)})
    return snapshot.entries[0]


def test_merge_layers_per_field_most_specific_wins():
    user = {"effort": "low", "retries": 3}
    project = {"effort": "medium"}
    task = {"effort": "high", "extra": "kept"}
    assignment = {"retries": 9}
    merged = merge_layers(user, project, task, assignment)
    assert merged == {"effort": "high", "retries": 9, "extra": "kept"}


def test_merge_layers_skips_empty_layers():
    assert merge_layers(None, {}, {"a": 1}, None) == {"a": 1}


def test_configuration_inputs_has_no_cost_field():
    field_names = {f.name for f in dataclasses.fields(ConfigurationInputs)}
    assert not any("cost" in name or "spend" in name for name in field_names)


def test_rederive_drops_only_dependent_settings_on_route_change():
    route_a = ExecutionRoute(engine="codex-acp", connection_id="cn_" + "a" * 12, model="model-a")
    route_b = ExecutionRoute(engine="codex-acp", connection_id="cn_" + "a" * 12, model="model-b")
    parameters = {"effort": "high", "mode": "plan", "custom": "kept"}
    same_route = rederive_on_route_change(route_a.id, route_a, parameters)
    assert same_route == parameters
    changed_route = rederive_on_route_change(route_a.id, route_b, parameters)
    assert changed_route == {"custom": "kept"}
    fresh = rederive_on_route_change(None, route_b, parameters)
    assert fresh == parameters


def test_validate_dependent_settings_accepts_supported_bundle():
    # parameter_pairs() (see catalog.discovery.Observation) records every
    # advertised value per select (keyed by its shared ACP category, e.g.
    # "thought_level" -- never an adapter-specific option id like
    # "effort"), so the bundle is validated against every value the engine
    # actually offered, not only whichever was current at observation time.
    entry = _entry_with_effort_levels("high", "low")
    validate_dependent_settings(entry, {"thought_level": "high"})


def test_validate_dependent_settings_rejects_whole_bundle_atomically():
    entry = _entry_with_effort_levels("low", "high")
    with pytest.raises(PortableError) as excinfo:
        validate_dependent_settings(entry, {"thought_level": "extreme", "mode": "plan"})
    assert excinfo.value.code == "unsupported_dependent_setting"
    # No partial application signal: both offending names are reported together.
    assert "thought_level" in excinfo.value.remediation and "mode" in excinfo.value.remediation


# --- resolve_saved_route (T035 FINDING-05: a catalog refresh mints a new --
# --- route id for an unchanged (connection, model) whenever the observed --
# --- supported_parameters change) --------------------------------------

def test_resolve_saved_route_matches_by_identity_after_a_refresh_changes_parameters():
    connection = _connection()
    old_entry = _entry_with_effort_levels("none", "medium", connection=connection)
    new_entry = _entry_with_effort_levels("none", "medium", "high", "xhigh", connection=connection)
    # The refresh minted a different fingerprint id for the same model on
    # the same connection, purely because supported_parameters changed.
    assert old_entry.route.id != new_entry.route.id
    resolution = resolve_saved_route([new_entry], connection.id, "model-a")
    assert resolution.entry is new_entry
    assert resolution.withdrawn_model is None
    assert resolution.unresolved is False


def test_resolve_saved_route_fails_closed_naming_withdrawn_model():
    connection = _connection()
    resolution = resolve_saved_route([], connection.id, "model-a")
    assert resolution.entry is None
    assert resolution.withdrawn_model == "model-a"


def test_resolve_saved_route_unresolved_without_saved_identity():
    entries = [_entry_with_effort_levels("none")]
    resolution = resolve_saved_route(entries, None, None)
    assert resolution.entry is None
    assert resolution.unresolved is True
    assert resolution.withdrawn_model is None


def test_resolve_saved_route_never_substitutes_a_different_model_on_same_connection():
    """Near-miss guarantee: a sibling model still offered on the exact same
    connection must never stand in for the saved model that disappeared."""
    connection = _connection()
    options = [
        {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "model-b",
         "options": [{"value": "model-b", "name": "Model B"}]},
    ]
    snapshot = build_catalog({connection.id: connection}, {connection.id: parse_observation(options)})
    resolution = resolve_saved_route(list(snapshot.entries), connection.id, "model-a")
    assert resolution.entry is None
    assert resolution.withdrawn_model == "model-a"
    assert all(entry.route.model != "model-a" for entry in snapshot.entries)


def test_resolve_saved_route_never_substitutes_a_different_connection_with_same_model():
    """Near-miss guarantee: the identical model name on a *different*
    connection must never stand in for the saved (connection, model)."""
    connection_a = _connection("cn_" + "a" * 12)
    connection_b = _connection("cn_" + "b" * 12)
    entry_on_b = _entry_with_effort_levels("none", "medium", connection=connection_b)
    resolution = resolve_saved_route([entry_on_b], connection_a.id, entry_on_b.route.model)
    assert resolution.entry is None
    assert resolution.withdrawn_model == entry_on_b.route.model
