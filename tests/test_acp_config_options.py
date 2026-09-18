"""``config_options.py`` unit tests and ``session/new``/``session/set_config_option``
integration tests (T010, UI-02/UI-05/UI-06).

Fixture connections/catalog snapshots use fake secrets and placeholder
model ids only -- never a real credential, never a live engine probe.
"""
from __future__ import annotations

import json
import threading

import jsonschema
import pytest

from cortex_runtime.acp import commands as acp_commands
from cortex_runtime.acp import config_options
from cortex_runtime.acp.agent import Agent
from cortex_runtime.catalog import discovery, store as catalog_store
from cortex_runtime.catalog.evidence import EvidenceState
from cortex_runtime.catalog.model import CatalogEntry
from cortex_runtime.configuration import store as configuration_store
from cortex_runtime.configuration.service import ConfigurationService
from cortex_runtime.connections import onboarding, registry
from cortex_runtime.connections.model import AuthMode, ConnectionRecord, ConnectionState, EngineNativeDir
from cortex_runtime.portable.identity import ExecutionRoute, OwnershipEpoch
from cortex_runtime.store import Store

from pathlib import Path

SCHEMA_PATH = Path(__file__).resolve().parent / "fixtures" / "acp" / "schema-1.4.0.json"


def _schema_validator(def_name):
    document = json.loads(SCHEMA_PATH.read_text())
    resolver = jsonschema.RefResolver.from_schema(document)
    return jsonschema.Draft202012Validator({"$ref": f"#/$defs/{def_name}"}, resolver=resolver)


def _isolate(tmp_path, monkeypatch):
    config_home = tmp_path / "config"
    state_home = tmp_path / "state"
    home = tmp_path / "home"
    for path in (config_home, state_home, home):
        path.mkdir()
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    monkeypatch.setenv("XDG_STATE_HOME", str(state_home))
    monkeypatch.setenv("HOME", str(home))
    return dict(XDG_CONFIG_HOME=str(config_home), XDG_STATE_HOME=str(state_home), HOME=str(home))


def _configured_connection(name="work", engine="codex-acp", provider="openai", environ=None, key_kind=None):
    record = onboarding.create_connection(
        name=name, engine=engine, provider=provider, auth_mode=AuthMode.API_KEY,
        key_kind=key_kind or "openai_api_key", endpoint=f"cortex-test:{name}", environ=environ)
    return onboarding.set_api_key(record.id, reader=lambda: b"fake-test-secret-key", environ=environ)


def _observation_raw(model_id="model-placeholder-1", model_name="Placeholder Model 1",
                      thought_level="high", thought_levels=("high", "low")):
    options = [
        dict(id="model", name="Model", type="select", category="model", currentValue=model_id,
             options=[dict(value=model_id, name=model_name)]),
    ]
    if thought_level is not None:
        options.append(dict(id="thought_level", name="Thought level", type="select",
                             category="thought_level", currentValue=thought_level,
                             options=[dict(value=value, name=value) for value in thought_levels]))
    return options


def _build_and_save_snapshot(record, *, environ, model_id="model-placeholder-1",
                              model_name="Placeholder Model 1", thought_level="high"):
    observation = discovery.parse_observation(_observation_raw(model_id, model_name, thought_level))
    snapshot = discovery.build_catalog({record.id: record}, {record.id: observation})
    catalog_store.save_snapshot(snapshot, environ)
    return snapshot


# --- config_options.py unit tests -------------------------------------------

# --- T035: seeded model list, superseded by a real observation ------------

def test_never_observed_connection_falls_back_to_seed_with_seed_provenance(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(engine="codex-acp", environ=environ)
    # No _build_and_save_snapshot call -- this connection has never been
    # observed live yet.
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    assert entries  # the seed, not an empty result
    assert {entry.route.model for entry in entries} == {
        value for value, _name in __import__(
            "cortex_runtime.catalog.model_seed", fromlist=["SEED_MODELS"]).SEED_MODELS["codex-acp"]}
    model_option = config_options.build_model_option(entries, None, None, None)
    for option in model_option["options"]:
        assert "seeded" in option["description"]
        assert "not yet confirmed" in option["description"]  # never claimed confirmed


def test_real_observation_supersedes_the_seed(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(engine="codex-acp", environ=environ)
    seeded = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    assert {entry.route.model for entry in seeded} != {"model-placeholder-1"}

    # A real session observation is captured for this connection (see
    # delegation.live_adapters's automatic capture, T035 item 4) and
    # persisted as a real catalog snapshot -- the next read must use it,
    # never the seed, even though the seed is still a valid fallback for
    # connections that have no snapshot at all.
    _build_and_save_snapshot(record, environ=environ, model_id="model-placeholder-1")
    observed = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    assert {entry.route.model for entry in observed} == {"model-placeholder-1"}
    option = config_options.build_model_option(observed, None, None, None)
    assert "seeded" not in option["options"][0]["description"]
    assert "unverified" in option["options"][0]["description"]


def test_no_connections_means_no_model_option():
    assert config_options.build_config_options((), None) == []


def test_build_model_option_omitted_when_no_selectable_entries():
    assert config_options.build_model_option((), None, None, None) is None


def test_model_option_two_connections_same_model_distinct_values_and_labels(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record_a = _configured_connection(name="Team A", environ=environ)
    record_b = _configured_connection(name="Team B", environ=environ)
    _build_and_save_snapshot(record_a, environ=environ, model_id="shared-model", model_name="Shared Model")
    _build_and_save_snapshot(record_b, environ=environ, model_id="shared-model", model_name="Shared Model")

    entries = config_options.selectable_entries_from_snapshots(
        {record_a.id: record_a, record_b.id: record_b}, environ)
    assert len(entries) == 2
    route_ids = {entry.route.id for entry in entries}
    assert len(route_ids) == 2  # distinct connection_id -> distinct route fingerprint

    # T035: 'model' is now scoped to one provider -- the same model name
    # under two different providers/connections is one distinct model
    # option value each, seen only once each provider is selected; the
    # 'provider' option is what distinguishes the two connections.
    connections = {record_a.id: record_a, record_b.id: record_b}
    provider_option = config_options.build_provider_option(entries, connections, None, None, None)
    provider_values = {item["value"] for item in provider_option["options"]}
    assert provider_values == {record_a.id, record_b.id}
    assert len(provider_option["options"]) == 2  # disambiguated -- never silently merged

    for connection_id in (record_a.id, record_b.id):
        provider_entries = [entry for entry in entries if entry.connection_id == connection_id]
        model_option = config_options.build_model_option(provider_entries, None, None, None)
        assert {item["value"] for item in model_option["options"]} == {"shared-model"}


def _bump_mtime(connection_id, snapshot_id, environ):
    """Force ``snapshot_id``'s cached file to sort as the newest for
    ``connection_id`` (mtime-second resolution can otherwise tie two files
    saved back-to-back within the same test)."""
    import os
    import time

    path = registry.state_root(environ) / "catalog" / connection_id / f"{snapshot_id}.json"
    newest = time.time() + 5
    os.utime(path, (newest, newest))


# --- T035 FINDING-07: configuring one connection must never hide another --

def test_pin_route_style_placeholder_in_sibling_directory_never_hides_it(tmp_path, monkeypatch):
    """Reproduces the live defect directly at the storage layer: a
    ``build_catalog`` call whose ``connections`` mapping includes a
    connection it never actually observed (``observations`` has no entry
    for it) writes an entry-less placeholder into *that* connection's own
    catalog directory too (``catalog.store.save_snapshot``). Once a real
    snapshot already exists there, the newer placeholder must never win."""
    environ = _isolate(tmp_path, monkeypatch)
    claude = _configured_connection(name="Claude", engine="claude-agent-acp", environ=environ, key_kind="anthropic_api_key")
    codex = _configured_connection(name="Codex", engine="codex-acp", environ=environ)
    _build_and_save_snapshot(claude, environ=environ, model_id="claude-model")
    _build_and_save_snapshot(codex, environ=environ, model_id="codex-model")

    # Mimics the pre-fix cortex_acp_setup.pin_route bug: build_catalog is
    # called with BOTH connections but an observation only for codex, so
    # build_catalog marks claude "discovery_unavailable" even though
    # nothing about claude actually changed, and save_snapshot persists
    # that placeholder into claude's own directory, newer than its real
    # snapshot.
    from cortex_runtime.catalog.discovery import ManualDeclaration
    connections = {claude.id: claude, codex.id: codex}
    placeholder_snapshot = discovery.build_catalog(
        connections, observations={},
        manual_declarations=(ManualDeclaration(connection_id=codex.id, model="codex-model-2"),),
    )
    catalog_store.save_snapshot(placeholder_snapshot, environ)
    _bump_mtime(claude.id, placeholder_snapshot.id, environ)
    _bump_mtime(codex.id, placeholder_snapshot.id, environ)

    entries = config_options.selectable_entries_from_snapshots(connections, environ)
    connection_ids = {entry.connection_id for entry in entries}
    assert connection_ids == {claude.id, codex.id}  # both still selectable
    claude_models = {entry.route.model for entry in entries if entry.connection_id == claude.id}
    assert claude_models == {"claude-model"}  # claude's real catalog, not wiped out


def test_connection_whose_only_cached_files_are_placeholders_falls_back_to_seed(tmp_path, monkeypatch):
    """A connection that was never itself observed, but only ever received
    placeholder files as a side effect of some other connection's
    build_catalog call, must be treated exactly like one with no cached
    snapshot at all -- never silently absent, never a fabricated entry."""
    environ = _isolate(tmp_path, monkeypatch)
    claude = _configured_connection(name="Claude", engine="claude-agent-acp", environ=environ, key_kind="anthropic_api_key")
    codex = _configured_connection(name="Codex", engine="codex-acp", environ=environ)
    _build_and_save_snapshot(codex, environ=environ, model_id="codex-model")

    from cortex_runtime.catalog.discovery import ManualDeclaration
    connections = {claude.id: claude, codex.id: codex}
    placeholder_snapshot = discovery.build_catalog(
        connections, observations={},
        manual_declarations=(ManualDeclaration(connection_id=codex.id, model="codex-model"),),
    )
    catalog_store.save_snapshot(placeholder_snapshot, environ)  # claude's ONLY file is a placeholder
    _bump_mtime(claude.id, placeholder_snapshot.id, environ)

    entries = config_options.selectable_entries_from_snapshots(connections, environ)
    claude_models = {entry.route.model for entry in entries if entry.connection_id == claude.id}
    from cortex_runtime.catalog import model_seed
    assert claude_models == {value for value, _name in model_seed.SEED_MODELS["claude-agent-acp"]}


def _write_raw_snapshot_file(connection_id, snapshot_id, *, entries_payload, unavailable_reason, environ):
    """Write a ``cs_*.json`` file directly in the exact on-disk shape
    ``catalog.store.save_snapshot`` produces, bypassing
    ``catalog.discovery.build_catalog`` (which today never actually emits
    a file for a connection with an observation but zero entries -- see
    its own ``entries``/``unavailable`` bookkeeping). Used only to exercise
    ``selectable_entries_from_snapshots``'s own on-disk contract directly."""
    import json as _json

    from cortex_runtime.connections import registry as _registry

    payload = {
        "snapshot_id": snapshot_id,
        "created_at": "2026-09-18T00:00:00.000000+00:00",
        "connection_registry_fingerprint": "test-fingerprint",
        "setup_action": None,
        "unavailable_reason": unavailable_reason,
        "entries": entries_payload,
    }
    directory = _registry.state_root(environ) / "catalog" / connection_id
    _registry.ensure_private_dir(directory, root=_registry.state_root(environ), environ=environ)
    path = directory / f"{snapshot_id}.json"
    _registry.atomic_write_private(path, _json.dumps(payload).encode("utf-8"), directory=directory)


def test_genuine_zero_model_observation_stays_distinguishable_from_a_placeholder(tmp_path, monkeypatch):
    """A connection whose *own* real observation legitimately advertised no
    'model' option (entries == 0, unavailable_reason is None) is a
    genuine, engine-confirmed "nothing selectable right now" -- it must
    stay selected as the current snapshot rather than being treated as
    the "wrong connection" noise case and skipped in favour of something
    older."""
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(engine="codex-acp", environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="codex-model")

    # A later, genuine observation for the SAME connection that reports no
    # models at all -- entries=[], unavailable_reason=None: distinct from
    # the cross-connection placeholder shape (entries=[], a reason set).
    genuine_zero_id = "cs_" + "0" * 11 + "9"  # 12 lowercase hex chars, distinct from any real id
    _write_raw_snapshot_file(
        record.id, genuine_zero_id, entries_payload=[], unavailable_reason=None, environ=environ)
    _bump_mtime(record.id, genuine_zero_id, environ)
    view = catalog_store.load_connection_snapshot(record.id, genuine_zero_id, environ)
    assert view.entries == ()
    assert view.unavailable_reason is None  # genuinely observed, not a placeholder

    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    # The genuine (later, empty) observation wins -- nothing selectable for
    # this connection any more, never the older, now-stale real catalog.
    assert entries == ()


def _assert_option_names_unique(option):
    """No two entries within one ``SessionConfigOption`` may render the same
    ``name`` (FINDING-04, T035): a host that truncates two identical strings
    identically leaves the user with no way to tell them apart."""
    names = [item["name"] for item in option["options"]]
    assert len(names) == len(set(names)), names


def _model_entry(connection_id, engine, connection_name, model):
    route = ExecutionRoute(engine=engine, connection_id=connection_id, model=model, supported_parameters=())
    # T035: CatalogEntry.label/disambiguated_label are the *full* route
    # label ("Engine · Connection · Model") -- exactly what build_model_option
    # must NOT reuse as the model option's own name (see FINDING-04).
    full_label = f"{engine} · {connection_name} · {model}"
    return CatalogEntry(
        route=route, connection_id=connection_id, label=full_label, disambiguated_label=full_label,
        evidence=EvidenceState.empty(), catalog_snapshot_id="cs_" + "a" * 12)


def test_model_option_names_are_bare_model_ids_not_the_full_route_label():
    """FINDING-04: with one provider selected, every model in the dropdown
    previously repeated the identical ``Engine · Connection · Model`` prefix,
    so a host truncating the display made every entry render identically.
    The model option's own names must be the bare model ids only -- the
    provider control already conveys the engine/connection."""
    connection_id = "cn_" + "a" * 12
    entries = [
        _model_entry(connection_id, "claude-agent-acp", "claude-sonnet-live", model)
        for model in ("default", "opus[1m]", "sonnet", "haiku")
    ]
    option = config_options.build_model_option(entries, None, None, None)
    names = {item["name"] for item in option["options"]}
    assert names == {"default", "opus[1m]", "sonnet", "haiku"}
    for name in names:
        assert "claude-agent-acp" not in name
        assert "claude-sonnet-live" not in name
    _assert_option_names_unique(option)


def test_provider_option_names_never_collide_with_model_option_names():
    """FINDING-04: the provider control's own label (``Engine · Connection``)
    and the model control's own labels (bare model ids) must never coincide,
    since a Zed user reads both rows on the same composer line."""
    connection_id = "cn_" + "a" * 12
    connections = {connection_id: _fake_connection_record(connection_id, name="claude-sonnet-live")}
    entries = [
        _model_entry(connection_id, "claude-agent-acp", "claude-sonnet-live", model)
        for model in ("default", "opus[1m]", "sonnet", "haiku")
    ]
    provider_option = config_options.build_provider_option(entries, connections, None, None, None)
    model_option = config_options.build_model_option(entries, None, None, None)
    provider_names = {item["name"] for item in provider_option["options"]}
    model_names = {item["name"] for item in model_option["options"]}
    assert provider_names.isdisjoint(model_names)
    _assert_option_names_unique(provider_option)
    _assert_option_names_unique(model_option)


def test_provider_option_names_use_the_short_engine_name_not_the_adapter_package_id():
    """The user's own requirement (lead-verbatim): the first control must say
    ``codex``/``claude`` -- never the raw adapter package id. These fixture
    connections have no resolvable account (a fake credential dir that does
    not exist on disk), so the label falls back to the engine name alone --
    never a fabricated account and never the operator-chosen nickname
    (``claude-sonnet-live``/``codex-luna``, which used to be shown here and
    told the user nothing about which account is billed)."""
    claude_id, codex_id = "cn_" + "a" * 12, "cn_" + "b" * 12
    connections = {
        claude_id: _fake_connection_record(claude_id, name="claude-sonnet-live", engine="claude-agent-acp"),
        codex_id: _fake_connection_record(codex_id, name="codex-luna", engine="codex-acp"),
    }
    entries = [
        _model_entry(claude_id, "claude-agent-acp", "claude-sonnet-live", "sonnet"),
        _model_entry(codex_id, "codex-acp", "codex-luna", "gpt-5.5"),
    ]
    provider_option = config_options.build_provider_option(entries, connections, None, None, None)
    names_by_value = {item["value"]: item["name"] for item in provider_option["options"]}
    assert names_by_value[claude_id] == "claude"
    assert names_by_value[codex_id] == "codex"
    for name in names_by_value.values():
        assert "claude-agent-acp" not in name
        assert "codex-acp" not in name
        # Never the operator nickname either -- it identifies nothing about
        # which account/subscription is in use.
        assert "claude-sonnet-live" not in name
        assert "codex-luna" not in name
    # Display-only: the stored/engine-side identity is unchanged.
    assert entries[0].route.engine == "claude-agent-acp"
    assert entries[1].route.engine == "codex-acp"
    assert connections[claude_id].engine == "claude-agent-acp"
    assert connections[codex_id].engine == "codex-acp"


def test_provider_option_names_are_unique_alongside_model_option_names():
    """Extends the FINDING-04 uniqueness guard (T035) to the new short
    engine-name provider labels: two different engines/connections must
    never render an identical first-control label, and the provider labels
    must still never collide with the model option's own labels."""
    claude_id, codex_id = "cn_" + "a" * 12, "cn_" + "b" * 12
    connections = {
        claude_id: _fake_connection_record(claude_id, name="claude-sonnet-live", engine="claude-agent-acp"),
        codex_id: _fake_connection_record(codex_id, name="codex-luna", engine="codex-acp"),
    }
    entries = [
        _model_entry(claude_id, "claude-agent-acp", "claude-sonnet-live", "sonnet"),
        _model_entry(codex_id, "codex-acp", "codex-luna", "gpt-5.5"),
    ]
    provider_option = config_options.build_provider_option(entries, connections, None, None, None)
    model_option = config_options.build_model_option(
        [entry for entry in entries if entry.connection_id == claude_id], None, None, None)
    _assert_option_names_unique(provider_option)
    _assert_option_names_unique(model_option)
    provider_names = {item["name"] for item in provider_option["options"]}
    model_names = {item["name"] for item in model_option["options"]}
    assert provider_names.isdisjoint(model_names)


def test_model_option_names_disambiguated_when_sanitization_collides():
    """Two distinct model ids that sanitize to the identical displayable
    string (here: differing only by a stray combining mark stripped by
    ``catalog.labels``) must not silently render as the same option name --
    the route-id-derived suffix from ``disambiguate_labels`` must kick in."""
    connection_id = "cn_" + "a" * 12
    combining_mark = "́"  # stripped when stacked onto an ASCII base
    entries = [
        _model_entry(connection_id, "codex-acp", "work", "sonnet"),
        _model_entry(connection_id, "codex-acp", "work", "sonnet" + combining_mark),
    ]
    option = config_options.build_model_option(entries, None, None, None)
    assert len(option["options"]) == 2  # never silently merged
    _assert_option_names_unique(option)


def test_thought_level_option_only_from_currently_selected_route(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    # thought_levels defaults to ("high", "low") -- every value the engine
    # actually advertised, not only whichever was current at observation
    # time (see catalog.discovery.Observation.parameter_pairs).
    _build_and_save_snapshot(record, environ=environ, thought_level="high")
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    entry = entries[0]
    option = config_options.build_thought_level_option(entry, {}, None, None)
    assert option["id"] == "thought_level"
    assert option["currentValue"] == "high"
    assert {item["value"] for item in option["options"]} == {"high", "low"}


def test_thought_level_option_omitted_when_route_reports_none(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, thought_level=None)
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    assert config_options.build_thought_level_option(entries[0], {}, None, None) is None


def _entry_with_model_thought_levels(connection_id, model, values):
    """One catalog entry for ``model`` whose own ``thought_level`` set is
    exactly ``values`` -- mirrors the real, per-model-keyed shape
    ``catalog.discovery.Observation.parameter_pairs_for``/
    ``catalog.codex_debug_models`` produce (e.g. codex's own ``gpt-5.5``
    genuinely lacking ``max``/``ultra`` that ``gpt-5.6-luna`` has)."""
    route = ExecutionRoute(
        engine="codex-acp", connection_id=connection_id, model=model,
        supported_parameters=tuple((config_options.THOUGHT_LEVEL_OPTION_ID, value) for value in values))
    label = f"codex-acp · work · {model}"
    return CatalogEntry(
        route=route, connection_id=connection_id, label=label, disambiguated_label=label,
        evidence=EvidenceState.empty(), catalog_snapshot_id="cs_" + "c" * 12)


def test_thought_level_option_narrows_to_the_selected_models_own_set_not_an_engine_wide_union():
    """The effort control must reflect the *currently selected model's* own
    thought_level set, never the union across every model of the same
    engine/connection -- codex's own catalog reports a genuinely different
    ``supported_reasoning_levels`` per model (``gpt-5.5`` lacks
    ``max``/``ultra`` that ``gpt-5.6-luna`` has; see
    ``catalog.codex_debug_models``)."""
    connection_id = "cn_" + "d" * 12
    narrow_entry = _entry_with_model_thought_levels(
        connection_id, "gpt-5.5", ("default", "low", "medium", "high"))
    wide_entry = _entry_with_model_thought_levels(
        connection_id, "gpt-5.6-luna", ("default", "low", "medium", "high", "max", "ultra"))
    entries = [narrow_entry, wide_entry]

    narrow_option = config_options.build_thought_level_option(narrow_entry, {}, None, None)
    assert {item["value"] for item in narrow_option["options"]} == {"default", "low", "medium", "high"}
    assert "max" not in {item["value"] for item in narrow_option["options"]}

    wide_option = config_options.build_thought_level_option(wide_entry, {}, None, None)
    assert {item["value"] for item in wide_option["options"]} == {
        "default", "low", "medium", "high", "max", "ultra"}

    # End-to-end through build_config_options: switching the selected route
    # from one model to the other must narrow, never keep offering the
    # other model's wider set.
    connections = {connection_id: _fake_connection_record(connection_id, name="work")}
    options_for_narrow = config_options.build_config_options(
        entries, connections, current=dict(route_id=narrow_entry.route.id, state="active", revision=1))
    thought_option = next(o for o in options_for_narrow if o["id"] == config_options.THOUGHT_LEVEL_OPTION_ID)
    assert {item["value"] for item in thought_option["options"]} == {"default", "low", "medium", "high"}

    options_for_wide = config_options.build_config_options(
        entries, connections, current=dict(route_id=wide_entry.route.id, state="active", revision=1))
    thought_option = next(o for o in options_for_wide if o["id"] == config_options.THOUGHT_LEVEL_OPTION_ID)
    assert {item["value"] for item in thought_option["options"]} == {
        "default", "low", "medium", "high", "max", "ultra"}


def test_state_suffix_and_meta_reflect_pending_vs_active():
    entries = ()
    for state, expected_suffix in (("selected", "pending"), ("pending_next_use", "pending"), ("active", "active")):
        option = config_options.build_model_option(
            [CatalogEntry(
                route=__import__("cortex_runtime.portable.identity", fromlist=["ExecutionRoute"]).ExecutionRoute(
                    engine="codex-acp", connection_id="cn_" + "a" * 12, model="m", supported_parameters=()),
                connection_id="cn_" + "a" * 12, label="Engine · Conn · m", disambiguated_label="Engine · Conn · m",
                evidence=__import__("cortex_runtime.catalog.evidence", fromlist=["EvidenceState"]).EvidenceState.empty(),
                catalog_snapshot_id="cs_" + "b" * 12,
            )],
            None, state, 1 if state != "selected" else 1)
        assert expected_suffix in option["description"]
        assert option["_meta"]["_cortex"]["state"] == state


def test_evidence_note_marks_unverified_discovery_entries(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ)
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    option = config_options.build_model_option(entries, None, None, None)
    assert "unverified" in option["options"][0]["description"]


def test_render_boolean_as_select_without_client_capability():
    option = config_options.render_boolean_as_select("team", "Team", True, category="_extension")
    assert option["type"] == "select"
    assert option["currentValue"] == "true"
    assert {item["value"] for item in option["options"]} == {"true", "false"}


def test_render_boolean_emits_real_boolean_when_client_supports_it():
    caps = {"session": {"configOptions": {"boolean": {}}}}
    option = config_options.render_boolean_as_select("team", "Team", False, client_capabilities=caps)
    assert option["type"] == "boolean"
    assert option["currentValue"] is False


def test_most_recent_live_selection_skips_rejected_and_failed():
    history = (
        dict(revision=1, state="failed", route_id="a"),
        dict(revision=2, state="active", route_id="b"),
        dict(revision=3, state="rejected", route_id="c"),
    )
    best = config_options.most_recent_live_selection(history)
    assert best["revision"] == 2


# --- session/new integration (in-process Agent) -----------------------------

class _RecordingTransport:
    def __init__(self):
        self.responses = []
        self.notifications = []
        self._lock = threading.Lock()

    def send_response(self, message):
        with self._lock:
            self.responses.append(message)

    def send_notification(self, message, timeout=None):
        with self._lock:
            self.notifications.append(message)
        return True


class _NoopRuntime:
    def start_prompt(self, task_id, session_id, content, emit_update, cancel_event):
        from cortex_runtime.portable.engine_contract import StopReason
        return StopReason.END_TURN

    def cancel(self, session_id):
        return None


def _new_agent(tmp_path, environ):
    agent = Agent(_NoopRuntime(), environ=environ)
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    agent.client_capabilities = {}
    return agent, transport


def test_session_new_omits_model_options_with_no_connections_but_offers_language(tmp_path, monkeypatch):
    """T020: model/thought_level stay gated on selectability, but
    ``language`` is never gated on a connection/model -- it is always
    offered, even with zero connections."""
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    agent, _transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    ids = {option["id"] for option in result["configOptions"]}
    assert ids == {"language"}
    assert result["configOptions"][0]["currentValue"] == "auto"


def test_session_new_includes_model_and_thought_level_options(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ)
    project = tmp_path / "project"
    project.mkdir()
    agent, _transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    ids = {option["id"] for option in result["configOptions"]}
    assert ids == {"provider", "model", "thought_level", "language"}
    validator = _schema_validator("SessionConfigOption")
    for option in result["configOptions"]:
        validator.validate(option)


def test_available_commands_update_sent_after_session_new(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    agent._post_session_new_notifications(result["sessionId"])
    assert len(transport.notifications) == 1
    update = transport.notifications[0]["params"]["update"]
    assert update["sessionUpdate"] == "available_commands_update"
    names = {item["name"] for item in update["availableCommands"]}
    # T035 adds `harvest`: a knowledge harvest is explicit-only by the
    # coordinator's own role, and until it was a command "explicit" had
    # nothing to express it -- a model had to infer from prose whether the
    # user had really asked, which is the inference the rule exists to
    # prevent. Named as the user types it, not `cortex-`-prefixed.
    assert names == {acp_commands.CORTEX_STATUS, acp_commands.CORTEX_MODEL,
                      acp_commands.CORTEX_EFFORT, acp_commands.CORTEX_SETUP,
                      acp_commands.CORTEX_NEW_TASK, acp_commands.CORTEX_STOP,
                      acp_commands.CORTEX_ANSWER, acp_commands.CORTEX_LANGUAGE,
                      acp_commands.HARVEST}
    validator = _schema_validator("AvailableCommandsUpdate")
    validator.validate(update)


# --- session/set_config_option (mid-task): UI-05/UI-06 -----------------------

def _bootstrap_task_for_session(agent, session_id, tmp_path):
    """Drive the same first-prompt path production code uses (bootstrap +
    ownership epoch capture), without spawning the runtime asynchronously."""
    from types import SimpleNamespace
    import uuid
    from cortex_runtime.bootstrap import bootstrap_task

    session = agent.sessions[session_id]
    store = agent._store_for(session["project"])
    origin = f"acp:{session_id}"
    event = SimpleNamespace(origin=origin, event_id=str(uuid.uuid4()))
    result = bootstrap_task(store, event, "hello", owner_ref=origin)
    session["task_id"] = result.task_id
    session["owner_epoch"] = result.owner_epoch
    from cortex_runtime.acp import sessions as acp_sessions
    acp_sessions.bind_task(store, session_id, result.task_id)
    return store, result.task_id


def test_set_config_option_model_reaches_configuration_service(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a", thought_level="high")
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    store, task_id = _bootstrap_task_for_session(agent, session_id, tmp_path)

    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    route_id = entries[0].route.id

    # T035: configId='model' now takes a plain model name, scoped to the
    # already-(auto-)selected provider -- not a whole fused route id.
    agent._session_set_config_option(99, dict(sessionId=session_id, configId="model", value="model-a"))
    response = next(message for message in transport.responses if message.get("id") == 99)
    assert "result" in response, response
    options_by_id = {opt["id"]: opt for opt in response["result"]["configOptions"]}
    assert options_by_id["model"]["currentValue"] == "model-a"
    assert options_by_id["provider"]["currentValue"] == record.id

    service = ConfigurationService(store)
    history = service.history(task_id)
    assert len(history) == 1
    assert history[0]["route_id"] == route_id
    assert history[0]["state"] == "selected"

    validator = _schema_validator("SetSessionConfigOptionResponse")
    validator.validate(response["result"])
    update_validator = _schema_validator("ConfigOptionUpdate")
    update_notification = next(
        message for message in transport.notifications
        if message["params"]["update"]["sessionUpdate"] == "config_option_update")
    update_validator.validate(update_notification["params"]["update"])


def test_set_config_option_route_change_drops_incompatible_thought_level(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record_a = _configured_connection(name="A", environ=environ)
    record_b = _configured_connection(name="B", environ=environ)
    _build_and_save_snapshot(record_a, environ=environ, model_id="model-a", thought_level="high")
    _build_and_save_snapshot(record_b, environ=environ, model_id="model-b", thought_level=None)
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    store, task_id = _bootstrap_task_for_session(agent, session_id, tmp_path)

    agent._session_set_config_option(1, dict(sessionId=session_id, configId="provider", value=record_a.id))
    agent._session_set_config_option(2, dict(sessionId=session_id, configId="thought_level", value="high"))
    agent._session_set_config_option(3, dict(sessionId=session_id, configId="provider", value=record_b.id))
    response = next(message for message in transport.responses if message.get("id") == 3)
    result3 = response["result"]["configOptions"]
    ids = {opt["id"] for opt in result3}
    assert "thought_level" not in ids  # model-b reports no thought_level parameter
    options_by_id = {opt["id"]: opt for opt in result3}
    assert options_by_id["provider"]["currentValue"] == record_b.id
    assert options_by_id["model"]["currentValue"] == "model-b"

    service = ConfigurationService(store)
    history = service.history(task_id)
    assert "thought_level" not in history[-1]["parameters"]


def test_set_config_option_unknown_config_id_is_invalid_params(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ)
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    _bootstrap_task_for_session(agent, session_id, tmp_path)

    agent._session_set_config_option(7, dict(sessionId=session_id, configId="summarizer", value="x"))
    response = next(message for message in transport.responses if message.get("id") == 7)
    assert response["error"]["code"] == -32602
    restore = next(
        message for message in transport.notifications
        if message["params"]["update"]["sessionUpdate"] == "config_option_update")
    assert restore["params"]["update"]["configOptions"]  # unchanged options restored


def test_set_config_option_invalid_value_is_invalid_params(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ)
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    _bootstrap_task_for_session(agent, session_id, tmp_path)

    agent._session_set_config_option(8, dict(sessionId=session_id, configId="model", value="xr_doesnotexist000"))
    response = next(message for message in transport.responses if message.get("id") == 8)
    assert response["error"]["code"] == -32602


def test_set_config_option_boolean_rejected_without_capability(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ)
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    _bootstrap_task_for_session(agent, session_id, tmp_path)

    agent._session_set_config_option(9, dict(sessionId=session_id, configId="model", type="boolean", value=True))
    response = next(message for message in transport.responses if message.get("id") == 9)
    assert response["error"]["code"] == -32602


def test_set_config_option_pre_task_writes_user_default_and_is_pending(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a")
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]

    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    route_id = entries[0].route.id
    agent._session_set_config_option(5, dict(sessionId=session_id, configId="model", value="model-a"))
    response = next(message for message in transport.responses if message.get("id") == 5)
    assert "result" in response
    options_by_id = {opt["id"]: opt for opt in response["result"]["configOptions"]}
    option = options_by_id["model"]
    assert option["currentValue"] == "model-a"
    assert "pending" in option["description"]
    assert "pending" in options_by_id["provider"]["description"]

    defaults = configuration_store.read_user_defaults(environ)["defaults"]
    assert defaults["route_id"] == route_id
    # F2: the persisted user default is a normal T008 user-layer field, never
    # a Cortex-specific "pending" marker.
    assert "_cortex_pending_route_id" not in defaults
    assert "_cortex_pending_parameters" not in defaults
    defaults_path = configuration_store._user_defaults_path(environ)
    assert "_cortex_pending" not in defaults_path.read_text()


def test_new_session_shows_persisted_default_as_default_not_pending(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a")
    project = tmp_path / "project"
    project.mkdir()

    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    route_id = entries[0].route.id
    agent._session_set_config_option(1, dict(sessionId=session_id, configId="model", value="model-a"))

    # A brand-new session (same user) sees the persisted default -- with
    # state "default", never "pending" (F2 review finding): only the
    # session that actually made the choice shows it as pending.
    agent2, transport2 = _new_agent(tmp_path, environ)
    result2 = agent2._session_new(dict(cwd=str(project), mcpServers=[]))
    options_by_id = {opt["id"]: opt for opt in result2["configOptions"]}
    option = options_by_id["model"]
    assert option["currentValue"] == "model-a"
    assert option.get("description") is None or "pending" not in option["description"]
    assert option["_meta"]["_cortex"]["state"] == "default"
    session2 = agent2.sessions[result2["sessionId"]]
    assert session2["pending_selection"] is None
    assert session2["default_selection"]["route_id"] == route_id


# --- T035 FINDING-08: session/load must report the reloaded task's own
#     selection surface, not a bare null, and never the current global
#     default in its place ---------------------------------------------------

def test_session_load_reports_tasks_own_active_configuration(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a", thought_level="high")
    project = tmp_path / "project"
    project.mkdir()
    agent, _transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    _store, task_id = _bootstrap_task_for_session(agent, session_id, tmp_path)
    agent._session_set_config_option(1, dict(sessionId=session_id, configId="model", value="model-a"))
    agent._session_set_config_option(2, dict(sessionId=session_id, configId="thought_level", value="high"))

    # A brand-new Agent (simulating a reconnect) reloads the same durable
    # session/task.
    loader, loader_transport = _new_agent(tmp_path, environ)
    loader._session_load(42, dict(sessionId=session_id, cwd=str(project), mcpServers=[]))
    reply = next(m for m in loader_transport.responses if m.get("id") == 42)
    assert "error" not in reply, reply
    assert reply["result"]["modes"] is None  # T035: never reported by session/new either
    options_by_id = {opt["id"]: opt for opt in reply["result"]["configOptions"]}
    assert options_by_id["model"]["currentValue"] == "model-a"
    assert options_by_id["thought_level"]["currentValue"] == "high"
    # Reused vocabulary: a task's own live selection is "selected"/"active",
    # never "default" -- the mislabelling this fix must avoid.
    assert options_by_id["model"]["_meta"]["_cortex"]["state"] == "selected"
    validator = _schema_validator("LoadSessionResponse")
    validator.validate(reply["result"])


def test_session_load_reports_task_own_configuration_not_changed_global_default(tmp_path, monkeypatch):
    """The regression this finding is about: reconnect to a task after the
    *global* default changed to a different engine/model -- the reload must
    still show the task's own values, never the new default."""
    environ = _isolate(tmp_path, monkeypatch)
    record_a = _configured_connection(name="A", environ=environ)
    record_b = _configured_connection(name="B", environ=environ)
    _build_and_save_snapshot(record_a, environ=environ, model_id="model-a", thought_level="high")
    _build_and_save_snapshot(record_b, environ=environ, model_id="model-b", thought_level=None)
    project = tmp_path / "project"
    project.mkdir()

    agent, _transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    _store, task_id = _bootstrap_task_for_session(agent, session_id, tmp_path)
    agent._session_set_config_option(1, dict(sessionId=session_id, configId="provider", value=record_a.id))
    agent._session_set_config_option(2, dict(sessionId=session_id, configId="model", value="model-a"))
    agent._session_set_config_option(3, dict(sessionId=session_id, configId="thought_level", value="high"))

    # The global default changes afterward, in a different pre-task session
    # (mirrors T035's "changed global defaults" scenario).
    other_project = tmp_path / "other-project"
    other_project.mkdir()
    other_agent, _other_transport = _new_agent(tmp_path, environ)
    other_result = other_agent._session_new(dict(cwd=str(other_project), mcpServers=[]))
    other_agent._session_set_config_option(
        9, dict(sessionId=other_result["sessionId"], configId="provider", value=record_b.id))
    other_agent._session_set_config_option(
        10, dict(sessionId=other_result["sessionId"], configId="model", value="model-b"))
    defaults = configuration_store.read_user_defaults(environ)["defaults"]
    assert defaults["route_connection_id"] == record_b.id  # global default really did change

    loader, loader_transport = _new_agent(tmp_path, environ)
    loader._session_load(43, dict(sessionId=session_id, cwd=str(project), mcpServers=[]))
    reply = next(m for m in loader_transport.responses if m.get("id") == 43)
    options_by_id = {opt["id"]: opt for opt in reply["result"]["configOptions"]}
    assert options_by_id["provider"]["currentValue"] == record_a.id
    assert options_by_id["model"]["currentValue"] == "model-a"
    assert options_by_id["thought_level"]["currentValue"] == "high"
    assert options_by_id["model"]["_meta"]["_cortex"]["state"] == "selected"


def test_session_load_of_never_configured_task_is_distinguishable(tmp_path, monkeypatch):
    """A task that never got an active configuration still gets a sensible
    (auto-selected) options list, but with no ``_meta._cortex.state`` --
    distinguishable both from a real task selection ("selected"/"active")
    and from a fresh session's inherited "default"."""
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a", thought_level="high")
    project = tmp_path / "project"
    project.mkdir()
    agent, _transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    _bootstrap_task_for_session(agent, session_id, tmp_path)  # task exists, never configured

    loader, loader_transport = _new_agent(tmp_path, environ)
    loader._session_load(44, dict(sessionId=session_id, cwd=str(project), mcpServers=[]))
    reply = next(m for m in loader_transport.responses if m.get("id") == 44)
    options_by_id = {opt["id"]: opt for opt in reply["result"]["configOptions"]}
    assert options_by_id["model"]["currentValue"] == "model-a"  # sole selectable, auto-shown
    assert "_meta" not in options_by_id["model"]  # never "selected"/"active", never "default" either


def test_pending_pre_task_selection_is_proposed_on_first_prompt(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a")
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    route_id = entries[0].route.id
    agent._session_set_config_option(1, dict(sessionId=session_id, configId="model", value="model-a"))

    store, task_id = _bootstrap_task_for_session(agent, session_id, tmp_path)
    agent._propose_pending_selection_for_new_task(store, agent.sessions[session_id], task_id)

    service = ConfigurationService(store)
    history = service.history(task_id)
    assert len(history) == 1
    assert history[0]["route_id"] == route_id
    # F2: consumed -- no longer pending in the session afterward.
    session = agent.sessions[session_id]
    assert session["pending_selection"] is None
    assert session["default_selection"] is None


def test_stale_configuration_revision_is_never_silently_retried(tmp_path, monkeypatch):
    """F1 review finding: a concurrent session's change must never be
    silently clobbered by an automatic retry that re-reads/re-proposes from
    stale state. Session 1 changes model A->B (succeeds); session 2, holding
    a stale pre-change view of the options, tries to set effort for A and
    must get an explicit error with current options rebuilt fresh (still B),
    never a silent, incorrect apply."""
    environ = _isolate(tmp_path, monkeypatch)
    record_a = _configured_connection(name="A", environ=environ)
    record_b = _configured_connection(name="B", environ=environ)
    _build_and_save_snapshot(record_a, environ=environ, model_id="model-a", thought_level="high")
    _build_and_save_snapshot(record_b, environ=environ, model_id="model-b", thought_level=None)
    project = tmp_path / "project"
    project.mkdir()

    # Two sessions sharing one task/owner-epoch, to exercise a genuine
    # same-task race (see the review's "same owner epoch source").
    agent1, transport1 = _new_agent(tmp_path, environ)
    result1 = agent1._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id_1 = result1["sessionId"]
    store, task_id = _bootstrap_task_for_session(agent1, session_id_1, tmp_path)

    agent2, transport2 = _new_agent(tmp_path, environ)
    session_2 = dict(agent1.sessions[session_id_1])
    session_2["lock"] = threading.Lock()
    session_id_2 = "acps_" + "b" * 12
    agent2.sessions[session_id_2] = session_2
    from cortex_runtime.acp import sessions as acp_sessions
    acp_sessions.create_session(store, session_id_2, session_2["project"], {}, {}, [])
    acp_sessions.bind_task(store, session_id_2, task_id)

    # Session 1: select provider A, then provider B (two live revisions) --
    # this is the concurrent change session 2 will race against.
    agent1._session_set_config_option(1, dict(sessionId=session_id_1, configId="provider", value=record_a.id))
    agent1._session_set_config_option(2, dict(sessionId=session_id_1, configId="provider", value=record_b.id))
    response2 = next(message for message in transport1.responses if message.get("id") == 2)
    assert response2["result"]["configOptions"][0]["currentValue"] == record_b.id

    # Session 2 now acts on a *stale* view: its own read of "the latest
    # revision" only reflects session 1's first write (route A, revision 1),
    # not the second (route B, revision 2) that has already, genuinely,
    # happened. This deterministically reproduces the same-task race the
    # synchronous single-process reproduction above cannot otherwise create
    # (ConfigurationService.history is normally read fresh, in the same
    # call, immediately before the write -- see _apply_mid_task_selection).
    real_history = ConfigurationService.history
    call_count = {"n": 0}

    def _stale_then_real_history(self, task_id):
        call_count["n"] += 1
        if call_count["n"] <= 2:
            return real_history(self, task_id)[:1]  # only session 1's first write is "known"
        return real_history(self, task_id)  # the error-path rebuild always sees the truth

    monkeypatch.setattr(ConfigurationService, "history", _stale_then_real_history)

    # Session 2, still believing provider A is current (per its stale read),
    # tries to change the provider to A again -- a config_id whose validity
    # never depends on current state, so it clears the "unknown configId"
    # check even from a stale read, and reaches the actual revision check.
    agent2._session_set_config_option(3, dict(sessionId=session_id_2, configId="provider", value=record_a.id))
    error_response = next(message for message in transport2.responses if message.get("id") == 3)
    assert "error" in error_response, error_response
    assert error_response["error"]["code"] == -32001
    assert error_response["error"]["data"]["code"] == "stale_configuration_revision"

    # The task's configuration was never clobbered: it is still B, and a
    # config_option_update restoring the *current* (post-session-1) options
    # was emitted to session 2.
    entries = config_options.selectable_entries_from_snapshots(
        {record_a.id: record_a, record_b.id: record_b}, environ)
    route_b = next(e.route.id for e in entries if e.route.model == "model-b")
    service = ConfigurationService(store)
    history = service.history(task_id)
    assert history[-1]["route_id"] == route_b
    restore = next(
        message for message in transport2.notifications
        if message["params"]["update"]["sessionUpdate"] == "config_option_update")
    assert restore["params"]["update"]["configOptions"][0]["currentValue"] == record_b.id


# --- unavailable saved/selected route marker (review MINOR fix) -------------

def test_build_model_option_marks_unavailable_default_route(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a")
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    option = config_options.build_model_option(entries, "no-such-model", "default", None)
    assert option["currentValue"] == entries[0].route.model  # substitute, schema requires a value
    assert option["_meta"]["_cortex"]["state"] == "default_unavailable"
    assert option["description"] == "Saved route is unavailable — choose a route"


def test_build_model_option_marks_unavailable_task_selection(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a")
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    for state in ("selection_unavailable",):
        option = config_options.build_model_option(entries, "no-such-model", state, 3)
        assert option["_meta"]["_cortex"]["state"] == "selection_unavailable"
        assert option["description"] == "Saved route is unavailable — choose a route"


def test_available_route_never_marked_unavailable(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a")
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    option = config_options.build_model_option(entries, entries[0].route.model, "active", 1)
    assert option["_meta"]["_cortex"]["state"] == "active"
    assert option.get("description") == "(active)"


def test_build_provider_option_marks_unavailable_default_route(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a")
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    option = config_options.build_provider_option(entries, {record.id: record}, "cn_" + "0" * 12, "default", None)
    assert option["currentValue"] == record.id  # substitute, schema requires a value
    assert option["_meta"]["_cortex"]["state"] == "default_unavailable"
    assert option["description"] == "Saved route is unavailable — choose a route"


def test_build_provider_option_available_never_marked_unavailable(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a")
    entries = config_options.selectable_entries_from_snapshots({record.id: record}, environ)
    option = config_options.build_provider_option(entries, {record.id: record}, record.id, "active", 1)
    assert option["_meta"]["_cortex"]["state"] == "active"
    assert option.get("description") == "(active)"


def test_saved_default_route_connection_removed_marker_no_proposal_status_explains(tmp_path, monkeypatch):
    """Review MINOR: connection removed/deauthorized after the default was
    persisted -> marker present in configOptions, /cortex-status explains,
    and first-prompt consumption never proposes the substituted route."""
    environ = _isolate(tmp_path, monkeypatch)
    record_a = _configured_connection(name="A", environ=environ)
    record_b = _configured_connection(name="B", environ=environ)
    _build_and_save_snapshot(record_a, environ=environ, model_id="model-a")
    _build_and_save_snapshot(record_b, environ=environ, model_id="model-b")
    project = tmp_path / "project"
    project.mkdir()

    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    agent._session_set_config_option(1, dict(sessionId=session_id, configId="provider", value=record_a.id))

    # Connection A is removed entirely after the default was persisted.
    onboarding.remove(record_a.id, environ=environ)

    # A brand-new session (same user) now sees the marker, not a silent
    # substitution.
    agent2, transport2 = _new_agent(tmp_path, environ)
    result2 = agent2._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id2 = result2["sessionId"]
    option = result2["configOptions"][0]
    assert option["id"] == "provider"
    assert option["_meta"]["_cortex"]["state"] == "default_unavailable"
    assert "unavailable" in option["description"].lower()
    # The substitute is still a real, currently-selectable connection (B).
    assert option["currentValue"] == record_b.id

    # /cortex-status explains the same thing.
    session2 = agent2.sessions[session_id2]
    cancel_event = threading.Event()
    agent2._run_command_body(request_id=2, session_id=session_id2, session=session2,
                              recognized=("cortex-status", ""), cancel_event=cancel_event)
    chunk = next(n for n in transport2.notifications
                 if n["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
    assert "unavailable" in chunk["params"]["update"]["content"]["text"].lower()

    # First prompt: never auto-proposes the substituted route; the task is
    # left with no Cortex-proposed revision at all.
    store, task_id = _bootstrap_task_for_session(agent2, session_id2, tmp_path)
    agent2._propose_pending_selection_for_new_task(store, agent2.sessions[session_id2], task_id)
    service = ConfigurationService(store)
    assert service.history(task_id) == ()


# --- real concurrent set_config_option race (threading.Barrier) -------------

def test_concurrent_set_config_option_race_exactly_one_wins(tmp_path, monkeypatch):
    """A real two-thread race on the same task: exactly one session's
    set_config_option succeeds, the other gets a stale-revision error, and
    the task's configuration is never clobbered/left inconsistent."""
    environ = _isolate(tmp_path, monkeypatch)
    record_a = _configured_connection(name="A", environ=environ)
    record_b = _configured_connection(name="B", environ=environ)
    _build_and_save_snapshot(record_a, environ=environ, model_id="model-a", thought_level="high")
    _build_and_save_snapshot(record_b, environ=environ, model_id="model-b", thought_level=None)
    project = tmp_path / "project"
    project.mkdir()

    agent1, transport1 = _new_agent(tmp_path, environ)
    result1 = agent1._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id_1 = result1["sessionId"]
    store, task_id = _bootstrap_task_for_session(agent1, session_id_1, tmp_path)

    agent2, transport2 = _new_agent(tmp_path, environ)
    session_2 = dict(agent1.sessions[session_id_1])
    session_2["lock"] = threading.Lock()
    session_id_2 = "acps_" + "c" * 12
    agent2.sessions[session_id_2] = session_2
    from cortex_runtime.acp import sessions as acp_sessions
    acp_sessions.create_session(store, session_id_2, session_2["project"], {}, {}, [])
    acp_sessions.bind_task(store, session_id_2, task_id)

    entries = config_options.selectable_entries_from_snapshots(
        {record_a.id: record_a, record_b.id: record_b}, environ)
    route_a = next(e.route.id for e in entries if e.route.model == "model-a")
    route_b = next(e.route.id for e in entries if e.route.model == "model-b")

    # Force the genuine race window: each side's *own* internal read of
    # "the latest revision" (used to compute expected_revision inside
    # _apply_mid_task_selection) is a separate transaction from its write
    # (see ConfigurationService.propose); without synchronizing that read,
    # Python's GIL/scheduling makes one thread's whole read+write finish
    # before the other's read even starts, so no actual collision occurs.
    # A threading.Barrier on each thread's *second* ConfigurationService
    # .history() call (the expected_revision read; the first call, for
    # previous_options validation, never depends on task state for a
    # config_id="model" request) makes both threads compute
    # expected_revision from the identical pre-race state, so real SQLite
    # write-lock serialization then decides exactly one winner.
    real_history = ConfigurationService.history
    barrier = threading.Barrier(2)
    call_counts = threading.local()

    def _synchronized_history(self, task_id):
        call_counts.n = getattr(call_counts, "n", 0) + 1
        if call_counts.n == 2:
            barrier.wait(timeout=5)
        return real_history(self, task_id)

    monkeypatch.setattr(ConfigurationService, "history", _synchronized_history)

    results = {}

    def _race(agent, session_id, request_id, connection_id, key):
        agent._session_set_config_option(
            request_id, dict(sessionId=session_id, configId="provider", value=connection_id))
        results[key] = True

    t1 = threading.Thread(target=_race, args=(agent1, session_id_1, 10, record_a.id, "one"))
    t2 = threading.Thread(target=_race, args=(agent2, session_id_2, 20, record_b.id, "two"))
    t1.start()
    t2.start()
    t1.join(timeout=10)
    t2.join(timeout=10)
    assert results == {"one": True, "two": True}

    response_1 = next(m for m in transport1.responses if m.get("id") == 10)
    response_2 = next(m for m in transport2.responses if m.get("id") == 20)
    outcomes = [response_1, response_2]
    successes = [r for r in outcomes if "result" in r]
    errors = [r for r in outcomes if "error" in r]
    assert len(successes) == 1, outcomes
    assert len(errors) == 1, outcomes
    assert errors[0]["error"]["code"] == -32001
    assert errors[0]["error"]["data"]["code"] == "stale_configuration_revision"

    # The task's configuration was never clobbered: history has exactly one
    # live revision, matching whichever side actually won.
    service = ConfigurationService(store)
    history = service.history(task_id)
    live = [row for row in history if row["state"] in ("selected", "pending_next_use", "active")]
    assert len(live) == 1
    winning_route = route_a if successes[0] is response_1 else route_b
    assert live[0]["route_id"] == winning_route


# --- thought_level option must carry the model option's effective state
#     (review MINOR: config_options.py ~210-211) -----------------------------

def _entry_with_thought_levels(route_id_seed, values):
    route = ExecutionRoute(
        engine="codex-acp", connection_id="cn_" + route_id_seed * 12, model="m",
        supported_parameters=tuple((config_options.THOUGHT_LEVEL_OPTION_ID, value) for value in values))
    return CatalogEntry(
        route=route, connection_id="cn_" + route_id_seed * 12,
        label=f"Engine · Conn {route_id_seed} · m", disambiguated_label=f"Engine · Conn {route_id_seed} · m",
        evidence=EvidenceState.empty(), catalog_snapshot_id="cs_" + "b" * 12)


def _fake_connection_record(connection_id, name="Conn a", engine="codex-acp"):
    now = "2026-01-01T00:00:00+00:00"
    key_kind = "openai_api_key" if engine == "codex-acp" else "anthropic_api_key"
    return ConnectionRecord(
        id=connection_id, name=name, engine=engine, provider="acme",
        auth_mode=AuthMode.API_KEY, key_kind=key_kind, endpoint=None, launch_command=None,
        credential_ref=EngineNativeDir(path=f"/tmp/{connection_id}"), approved_fingerprint="fp",
        state=ConnectionState.CONFIGURED, qualified=True, created_at=now, updated_at=now)


def test_build_config_options_unavailable_default_marks_thought_level_too():
    """A saved default whose route is unavailable, with a saved (now stale)
    thought_level value, must never present that stale value as selected:
    the thought_level option gets the substitute route's first value and
    the identical unavailable marker as the provider/model options."""
    entry = _entry_with_thought_levels("a", ["low", "high"])  # sorted -> ["high", "low"]
    connections = {"cn_" + "a" * 12: _fake_connection_record("cn_" + "a" * 12)}
    current = dict(route_id="xr_" + "0" * 12, state="default", revision=None,
                    parameters={"thought_level": "low"})
    options = config_options.build_config_options([entry], connections, current)
    provider_option, model_option, thought_option = options
    assert provider_option["_meta"]["_cortex"]["state"] == "default_unavailable"
    assert model_option["_meta"]["_cortex"]["state"] == "default_unavailable"
    assert thought_option["_meta"]["_cortex"]["state"] == "default_unavailable"
    assert thought_option["description"] == "Depends on an unavailable route — choose a route"
    # Never the stale saved "low" -- the substituted route's first value.
    assert thought_option["currentValue"] == "high"


def test_build_config_options_unavailable_task_selection_marks_thought_level_too():
    """Same, but the saved value came from an actual task/session selection
    (state e.g. 'active'), not a pre-task default."""
    entry = _entry_with_thought_levels("a", ["low", "high"])
    connections = {"cn_" + "a" * 12: _fake_connection_record("cn_" + "a" * 12)}
    current = dict(route_id="xr_" + "1" * 12, state="active", revision=7,
                    parameters={"thought_level": "low"})
    options = config_options.build_config_options([entry], connections, current)
    provider_option, model_option, thought_option = options
    assert provider_option["_meta"]["_cortex"]["state"] == "selection_unavailable"
    assert model_option["_meta"]["_cortex"]["state"] == "selection_unavailable"
    assert thought_option["_meta"]["_cortex"]["state"] == "selection_unavailable"
    assert thought_option["description"] == "Depends on an unavailable route — choose a route"
    assert thought_option["currentValue"] == "high"
    assert thought_option["_meta"]["_cortex"]["revision"] == 7


def test_build_config_options_available_route_thought_level_unaffected():
    """Sanity: when the route *is* available, thought_level behaves exactly
    as before (no marker, saved value honored)."""
    entry = _entry_with_thought_levels("a", ["low", "high"])
    connections = {"cn_" + "a" * 12: _fake_connection_record("cn_" + "a" * 12)}
    current = dict(route_id=entry.route.id, state="active", revision=2, parameters={"thought_level": "low"})
    options = config_options.build_config_options([entry], connections, current)
    provider_option, model_option, thought_option = options
    assert provider_option["_meta"]["_cortex"]["state"] == "active"
    assert model_option["_meta"]["_cortex"]["state"] == "active"
    assert thought_option["_meta"]["_cortex"]["state"] == "active"
    assert thought_option["currentValue"] == "low"
    assert thought_option["description"] == "(active)"


def test_saved_default_route_removed_with_saved_thought_level_status_reflects_it(tmp_path, monkeypatch):
    """Integration regression: a removed connection's persisted default had
    a saved thought_level value; the new session's configOptions and
    /cortex-status must never present that stale value as selected, and
    both options carry the unavailable marker."""
    environ = _isolate(tmp_path, monkeypatch)
    record_a = _configured_connection(name="A", environ=environ)
    record_b = _configured_connection(name="B", environ=environ)
    _build_and_save_snapshot(record_a, environ=environ, model_id="model-a", thought_level="high")
    _build_and_save_snapshot(record_b, environ=environ, model_id="model-b", thought_level="high")
    project = tmp_path / "project"
    project.mkdir()

    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    agent._session_set_config_option(1, dict(sessionId=session_id, configId="provider", value=record_a.id))
    # Save a thought_level choice too, so the persisted default carries a
    # (soon to be stale) parameters value alongside the route.
    defaults = configuration_store.read_user_defaults(environ)
    defaults_dict = dict(defaults["defaults"])
    defaults_dict["parameters"] = {"thought_level": "high"}
    configuration_store.write_user_defaults(defaults_dict, defaults["ceiling"], environ)

    onboarding.remove(record_a.id, environ=environ)

    agent2, transport2 = _new_agent(tmp_path, environ)
    result2 = agent2._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id2 = result2["sessionId"]
    ids = {opt["id"]: opt for opt in result2["configOptions"]}
    assert ids["model"]["_meta"]["_cortex"]["state"] == "default_unavailable"
    if "thought_level" in ids:
        assert ids["thought_level"]["_meta"]["_cortex"]["state"] == "default_unavailable"
        assert "unavailable" in ids["thought_level"]["description"].lower()

    session2 = agent2.sessions[session_id2]
    cancel_event = threading.Event()
    agent2._run_command_body(request_id=2, session_id=session_id2, session=session2,
                              recognized=("cortex-status", ""), cancel_event=cancel_event)
    chunk = next(n for n in transport2.notifications
                 if n["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
    text = chunk["params"]["update"]["content"]["text"]
    assert text.lower().count("unavailable") >= 1


# --- T035 FINDING-10: selector reflects LIVE connection state, never a ------
#     cached/stored verdict (see config_options.selectable_entries_from_
#     snapshots's own docstring for the fix) -----------------------------

def _force_stored_state(connection_id, state, environ):
    """Directly overwrite the registry's *stored* state for one connection,
    bypassing ``onboarding.validate`` entirely -- simulates a record that
    fell out of sync with its own live credential (e.g. last validated
    while the credential was absent, or never revalidated after it came
    back), the exact scenario FINDING-10 reports."""
    with registry.transaction(environ) as reg_state:
        record = reg_state.connections[connection_id]
        stale = ConnectionRecord(
            id=record.id, name=record.name, engine=record.engine, provider=record.provider,
            auth_mode=record.auth_mode, key_kind=record.key_kind, endpoint=record.endpoint,
            launch_command=record.launch_command, credential_ref=record.credential_ref,
            approved_fingerprint=record.approved_fingerprint, state=state,
            qualified=record.qualified, created_at=record.created_at, updated_at=record.updated_at,
        )
        reg_state.connections[connection_id] = stale


def test_restored_credential_is_selectable_with_no_explicit_revalidation_call(tmp_path, monkeypatch):
    """FINDING-10 case 1: registry still reads AUTHORIZATION_REQUIRED (as it
    would right after a credential was removed) but the credential is back
    in place -- the selector must self-heal without any separate ``validate``
    call, exactly like the launch path already does."""
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a")

    # Stored state falls out of sync with the (still valid) credential --
    # never call onboarding.validate() ourselves here.
    _force_stored_state(record.id, ConnectionState.AUTHORIZATION_REQUIRED, environ)
    stale = registry.read_registry(environ).connections[record.id]
    assert stale.state == ConnectionState.AUTHORIZATION_REQUIRED

    entries = config_options.selectable_entries_from_snapshots({record.id: stale}, environ)
    assert entries  # selectable again, live state re-derived, not the stale stored one
    assert {entry.route.model for entry in entries} == {"model-a"}

    # The saved default for this exact route resolves too, not reported as
    # withdrawn.
    project = tmp_path / "project"
    project.mkdir()
    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    agent._session_set_config_option(1, dict(sessionId=session_id, configId="provider", value=record.id))

    _force_stored_state(record.id, ConnectionState.AUTHORIZATION_REQUIRED, environ)
    agent2, transport2 = _new_agent(tmp_path, environ)
    result2 = agent2._session_new(dict(cwd=str(project), mcpServers=[]))
    ids = {opt["id"]: opt for opt in result2["configOptions"]}
    assert ids["provider"]["currentValue"] == record.id
    assert "_meta" not in ids["provider"] or ids["provider"]["_meta"]["_cortex"]["state"] != "default_unavailable"


def test_disappeared_credential_stops_being_selectable_despite_stored_configured(tmp_path, monkeypatch):
    """FINDING-10 case 2 (the other direction): the registry's stored state
    still reads CONFIGURED but the credential itself is gone -- the selector
    must not keep offering it just because nothing has explicitly
    invalidated the stored record yet."""
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a")

    stored = registry.read_registry(environ).connections[record.id]
    assert stored.state == ConnectionState.CONFIGURED

    # The credential disappears on disk without any registry mutation --
    # stored state is untouched (still CONFIGURED).
    secret_path = registry.secret_path(record.id, environ)
    secret_path.unlink()
    stored_after = registry.read_registry(environ).connections[record.id]
    assert stored_after.state == ConnectionState.CONFIGURED  # still stale/optimistic

    entries = config_options.selectable_entries_from_snapshots({record.id: stored_after}, environ)
    assert entries == ()  # live state (AUTHORIZATION_REQUIRED) wins, never the stale CONFIGURED


def test_default_pointing_at_model_genuinely_absent_from_catalog_still_withdrawn(tmp_path, monkeypatch):
    """The two cases must not collapse: a saved default naming a model the
    current catalog genuinely no longer offers is still reported
    unavailable -- this must stay distinct from FINDING-10's stale-state
    case, which is now self-healed instead."""
    environ = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=environ)
    _build_and_save_snapshot(record, environ=environ, model_id="model-a")
    project = tmp_path / "project"
    project.mkdir()

    agent, transport = _new_agent(tmp_path, environ)
    result = agent._session_new(dict(cwd=str(project), mcpServers=[]))
    session_id = result["sessionId"]
    agent._session_set_config_option(1, dict(sessionId=session_id, configId="provider", value=record.id))
    agent._session_set_config_option(2, dict(sessionId=session_id, configId="model", value="model-a"))

    # The catalog is genuinely refreshed to no longer include "model-a" --
    # a real withdrawal, not a stale-state artifact; the connection's own
    # live state remains healthy (CONFIGURED) throughout.
    _build_and_save_snapshot(record, environ=environ, model_id="model-b")
    stored = registry.read_registry(environ).connections[record.id]
    assert stored.state == ConnectionState.CONFIGURED  # never touched -- this is a catalog change, not a state one

    agent2, transport2 = _new_agent(tmp_path, environ)
    result2 = agent2._session_new(dict(cwd=str(project), mcpServers=[]))
    ids = {opt["id"]: opt for opt in result2["configOptions"]}
    assert ids["model"]["_meta"]["_cortex"]["state"] == "default_unavailable"
    assert ids["model"]["description"] == "Saved route is unavailable — choose a route"
