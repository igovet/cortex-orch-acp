"""build_catalog scenarios: duplicates, missing discovery, manual declarations (T007)."""
from __future__ import annotations

import json
from pathlib import Path

import pytest

from cortex_runtime.catalog.discovery import ManualDeclaration, build_catalog, parse_observation
from cortex_runtime.catalog.evidence import Fact
from cortex_runtime.connections.model import AuthMode, ConnectionRecord, ConnectionState, EngineNativeDir
from cortex_runtime.portable.errors import PortableError

FIXTURES = Path(__file__).resolve().parent / "fixtures" / "catalog"


def _load_options(name):
    payload = json.loads((FIXTURES / name).read_text())
    return payload["configOptions"]


def _connection(connection_id, engine, name, *, auth_mode=AuthMode.NATIVE_ACCOUNT, state=ConnectionState.CONFIGURED):
    now = "2026-01-01T00:00:00+00:00"
    return ConnectionRecord(
        id=connection_id,
        name=name,
        engine=engine,
        provider="acme",
        auth_mode=auth_mode,
        key_kind=None,
        endpoint=None,
        launch_command=None,
        credential_ref=EngineNativeDir(path=f"/tmp/{connection_id}"),
        approved_fingerprint="fp",
        state=state,
        qualified=True,
        created_at=now,
        updated_at=now,
    )


def test_no_connections_gives_empty_snapshot_with_setup_action():
    snapshot = build_catalog({}, {})
    assert snapshot.entries == ()
    assert snapshot.unavailable == {}
    assert snapshot.setup_action == "add_connection"


def test_duplicate_model_names_across_two_connections_are_isolated():
    claude = _connection("cn_" + "a" * 12, "claude-agent-acp", "Work")
    codex = _connection("cn_" + "b" * 12, "codex-acp", "Personal")
    connections = {claude.id: claude, codex.id: codex}
    observations = {
        claude.id: parse_observation(_load_options("claude_agent_acp_config_options.json")),
        codex.id: parse_observation(_load_options("codex_acp_config_options.json")),
    }
    snapshot = build_catalog(connections, observations, adapter_versions={claude.id: "1.0", codex.id: "2.0"})
    model_a_entries = [entry for entry in snapshot.entries if entry.route.model == "model-a"]
    assert len(model_a_entries) == 2
    first, second = model_a_entries
    assert first.route.id != second.route.id
    assert first.disambiguated_label != second.disambiguated_label
    assert first.route.connection_id != second.route.connection_id
    # Parameters stay isolated per connection/adapter.
    assert dict(first.route.supported_parameters) != {} or dict(second.route.supported_parameters) != {}


def test_discovery_failure_for_one_connection_does_not_affect_another():
    good = _connection("cn_" + "a" * 12, "codex-acp", "Good")
    bad = _connection("cn_" + "b" * 12, "codex-acp", "Bad")
    connections = {good.id: good, bad.id: bad}
    observations = {good.id: parse_observation(_load_options("codex_acp_config_options.json")), bad.id: None}
    snapshot = build_catalog(connections, observations)
    assert bad.id in snapshot.unavailable
    assert all(entry.connection_id != bad.id for entry in snapshot.entries)
    assert any(entry.connection_id == good.id for entry in snapshot.entries)


def test_empty_observation_yields_zero_routes_and_retains_unavailability():
    connection = _connection("cn_" + "a" * 12, "codex-acp", "Empty")
    observations = {connection.id: parse_observation(_load_options("empty_config_options.json"))}
    snapshot = build_catalog({connection.id: connection}, observations)
    assert snapshot.unavailable[connection.id] == "no_models_observed"
    assert snapshot.entries == ()


def test_gateway_connections_produce_entries_marked_not_selectable():
    connection = _connection("cn_" + "a" * 12, "codex-acp", "Gateway", auth_mode=AuthMode.GATEWAY)
    observations = {connection.id: parse_observation(_load_options("codex_acp_config_options.json"))}
    connections = {connection.id: connection}
    snapshot = build_catalog(connections, observations)
    entry = snapshot.entries[0]
    assert entry.is_selectable(ConnectionState.CONFIGURED, "gateway") is False
    assert snapshot.selectable_for_new_execution(connections) == ()
    assert entry.is_selectable(ConnectionState.CONFIGURED, "api_key") is True


def test_revoked_authorization_excluded_from_selectable():
    connection = _connection("cn_" + "a" * 12, "codex-acp", "Revoked", state=ConnectionState.AUTHORIZATION_REQUIRED)
    connections = {connection.id: connection}
    # Authorization-required connections have no successful observation.
    snapshot = build_catalog(connections, {connection.id: None})
    assert connection.id in snapshot.unavailable
    assert snapshot.unavailable[connection.id] == "authorization_required"
    assert snapshot.selectable_for_new_execution(connections) == ()


def test_collaboration_mode_category_is_never_folded_into_route_parameters():
    # codex-acp's real "collaboration_mode" category (index.js:27883-27891,
    # values "default"/"plan") is a distinct, adapter-specific category from
    # "mode"; it must never appear in a route's supported_parameters, only
    # mode/model_config/thought_level select categories are captured there.
    codex = _connection("cn_" + "a" * 12, "codex-acp", "Work")
    observation = parse_observation(_load_options("codex_acp_config_options.json"))
    assert any(option.category == "collaboration_mode" for option in observation.options)
    snapshot = build_catalog({codex.id: codex}, {codex.id: observation})
    entry = snapshot.entries[0]
    params = dict(entry.route.supported_parameters)
    assert "collaboration_mode" not in params
    assert "mode" in params  # the real "mode" category is still captured


def _values_for(supported_parameters, name):
    return {value for pname, value in supported_parameters if pname == name}


def test_boolean_parameters_included_only_when_present():
    claude = _connection("cn_" + "a" * 12, "claude-agent-acp", "Work")
    observation = parse_observation(_load_options("claude_agent_acp_config_options.json"))
    snapshot = build_catalog({claude.id: claude}, {claude.id: observation})
    entry = snapshot.entries[0]
    names = {name for name, _value in entry.route.supported_parameters}
    assert _values_for(entry.route.supported_parameters, "fast") == {"false"}
    assert "mode" in names
    # Keyed by the shared ACP category ("thought_level"), never
    # claude-agent-acp's own adapter-specific option id ("effort") --
    # codex-acp's own "reasoning_effort" id shares this same category, and
    # only the shared name lets acp.config_options.build_thought_level_option
    # find either engine's effort choice by one name.
    assert "effort" not in names
    assert _values_for(entry.route.supported_parameters, "thought_level") == {"default", "medium"}


def test_thought_level_parameter_is_keyed_by_category_not_adapter_id():
    codex = _connection("cn_" + "a" * 12, "codex-acp", "Work")
    observation = parse_observation(_load_options("codex_acp_config_options.json"))
    snapshot = build_catalog({codex.id: codex}, {codex.id: observation})
    entry = snapshot.entries[0]
    names = {name for name, _value in entry.route.supported_parameters}
    assert "reasoning_effort" not in names
    # Every advertised effort value is captured, not only the momentarily
    # current one -- otherwise "high"/"low" could never be selected.
    assert _values_for(entry.route.supported_parameters, "thought_level") == {"none", "medium"}


def test_manual_declaration_is_unverified_and_requires_no_parameters():
    codex = _connection("cn_" + "a" * 12, "codex-acp", "Manual")
    declaration = ManualDeclaration(connection_id=codex.id, model="model-z")
    snapshot = build_catalog({codex.id: codex}, {codex.id: None}, manual_declarations=[declaration])
    entry = next(entry for entry in snapshot.entries if entry.route.model == "model-z")
    assert not entry.evidence.has(Fact.CATALOG_OBTAINED)
    assert not entry.evidence.has(Fact.CONFIGURATION_APPLIED)


def test_manual_declaration_rejects_unknown_parameters():
    codex = _connection("cn_" + "a" * 12, "codex-acp", "Manual")
    declaration = ManualDeclaration(connection_id=codex.id, model="model-z", parameters=(("mode", "agent"),))
    with pytest.raises(PortableError) as excinfo:
        build_catalog({codex.id: codex}, {codex.id: None}, manual_declarations=[declaration])
    assert excinfo.value.code == "unsupported_manual_parameters"


def test_manual_declaration_requires_known_connection():
    declaration = ManualDeclaration(connection_id="cn_" + "z" * 12, model="model-z")
    with pytest.raises(PortableError) as excinfo:
        build_catalog({}, {}, manual_declarations=[declaration])
    assert excinfo.value.code in ("unknown_connection",)


def test_parse_observation_rejects_malformed_payload():
    with pytest.raises(PortableError) as excinfo:
        parse_observation(_load_options("malformed_config_options.json"))
    assert excinfo.value.code == "invalid_observation"


def test_build_catalog_never_crosses_connections_for_labels():
    first = _connection("cn_" + "a" * 12, "codex-acp", "First")
    second = _connection("cn_" + "b" * 12, "codex-acp", "Second")
    observations = {
        first.id: parse_observation(_load_options("codex_acp_config_options.json")),
        second.id: parse_observation(_load_options("codex_acp_config_options.json")),
    }
    snapshot = build_catalog({first.id: first, second.id: second}, observations)
    for entry in snapshot.entries:
        assert entry.connection_id == entry.route.connection_id


# --- T035: static seed provenance is distinct from a real observation ------

def test_seeded_connection_promotes_seed_provenance_not_discovery():
    from cortex_runtime.catalog import discovery
    from cortex_runtime.catalog.evidence import ProvenanceSource

    record = _connection("cn_" + "a" * 12, "codex-acp", "Work")
    assert discovery.seed_observation("codex-acp") is None
    observation = parse_observation(_load_options("codex_acp_config_options.json"))
    snapshot = build_catalog(
        {record.id: record}, {record.id: observation}, seeded_connection_ids=frozenset({record.id}))
    assert snapshot.entries
    for entry in snapshot.entries:
        provenance = entry.evidence.get(Fact.CATALOG_OBTAINED)
        assert provenance.source == ProvenanceSource.SEED
        assert provenance.source != ProvenanceSource.DISCOVERY


def test_unlisted_engine_has_no_seed():
    from cortex_runtime.catalog import discovery
    assert discovery.seed_observation("no-such-engine") is None


def test_real_observation_still_uses_discovery_not_seed_provenance():
    from cortex_runtime.catalog.evidence import ProvenanceSource

    record = _connection("cn_" + "b" * 12, "codex-acp", "Work")
    observation = parse_observation(_load_options("codex_acp_config_options.json"))
    snapshot = build_catalog({record.id: record}, {record.id: observation})
    for entry in snapshot.entries:
        provenance = entry.evidence.get(Fact.CATALOG_OBTAINED)
        assert provenance.source == ProvenanceSource.DISCOVERY
