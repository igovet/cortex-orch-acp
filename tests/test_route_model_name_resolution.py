"""T035: ``live_adapters._resolve_route_model_name`` resolves a route's real,
engine-observed model display name from this connection's latest cached
catalog snapshot -- never a guessed version. Covers realistic Claude and
Codex ``configOptions`` shapes (aliases ``default``/``opus[1m]``/``haiku``,
already-concrete ids ``claude-sonnet-5``/``gpt-5.6-luna``)."""
from __future__ import annotations

from cortex_runtime.catalog import store as catalog_store
from cortex_runtime.catalog.evidence import EvidenceState
from cortex_runtime.catalog.model import CatalogEntry, CatalogSnapshot
from cortex_runtime.delegation.live_adapters import _resolve_route_model_name
from cortex_runtime.portable.identity import ExecutionRoute, new_id

from engine_fakes import isolated_environ

_CONNECTION_ID = "cn_" + "a" * 12


def _entry(model, *, model_name=None, model_description=None, connection_id=_CONNECTION_ID, engine="claude-agent-acp"):
    route = ExecutionRoute(engine=engine, connection_id=connection_id, model=model)
    return CatalogEntry(
        route=route, connection_id=connection_id, label="l", disambiguated_label="l",
        evidence=EvidenceState.empty(), catalog_snapshot_id=new_id("cs_"),
        model_name=model_name, model_description=model_description,
    )


def _save(tmp_path, entries):
    environ = isolated_environ(tmp_path)
    snapshot = CatalogSnapshot(
        id=new_id("cs_"), created_at="2026-01-01T00:00:00+00:00",
        connection_registry_fingerprint="fp", entries=tuple(entries), unavailable={},
    )
    catalog_store.save_snapshot(snapshot, environ)
    return environ


def test_no_cached_snapshot_yields_none(tmp_path):
    environ = isolated_environ(tmp_path)
    assert _resolve_route_model_name(_CONNECTION_ID, "opus[1m]", environ) is None


def test_no_matching_entry_yields_none(tmp_path):
    environ = _save(tmp_path, [_entry("haiku")])
    assert _resolve_route_model_name(_CONNECTION_ID, "opus[1m]", environ) is None


def test_alias_with_no_engine_evidence_yields_none(tmp_path):
    """``haiku`` names no version and the engine gave no ``model_name``/
    ``model_description`` for it -- callers must show it as a bare alias,
    never a fabricated name."""
    environ = _save(tmp_path, [_entry("haiku")])
    assert _resolve_route_model_name(_CONNECTION_ID, "haiku", environ) is None


def test_engine_named_alias_resolves_to_its_own_name(tmp_path):
    """A ``configOptions`` entry the engine itself named (e.g. a Zed/Paseo
    entry's own ``name``) is used verbatim."""
    environ = _save(tmp_path, [_entry("opus[1m]", model_name="Opus 5.5 (1M)")])
    assert _resolve_route_model_name(_CONNECTION_ID, "opus[1m]", environ) == "Opus 5.5 (1M)"


def test_default_alias_resolves_via_description_when_not_also_listed(tmp_path):
    """claude-agent-acp's own convention: ``default`` is described by the
    concrete model id it resolves to."""
    environ = _save(tmp_path, [_entry("default", model_description="claude-opus-5-5[1m]")])
    assert _resolve_route_model_name(_CONNECTION_ID, "default", environ) == "Opus 5.5 (1M)"


def test_already_concrete_versioned_id_decodes_without_engine_name(tmp_path):
    environ = _save(tmp_path, [_entry("claude-sonnet-5")])
    assert _resolve_route_model_name(_CONNECTION_ID, "claude-sonnet-5", environ) == "Sonnet 5"


def test_codex_engine_named_model_resolves_to_its_own_name(tmp_path):
    environ = _save(tmp_path, [_entry("gpt-5.6-luna", model_name="Luna", engine="codex-acp")])
    assert _resolve_route_model_name(_CONNECTION_ID, "gpt-5.6-luna", environ) == "Luna"


def test_codex_bare_model_id_with_no_evidence_yields_none(tmp_path):
    """A codex id like ``gpt-5.6-luna`` does not match the Claude-specific
    versioned-id decoder (it uses a dotted minor version), so with no
    engine-given name it stays unresolved rather than guessed."""
    environ = _save(tmp_path, [_entry("gpt-5.6-luna", engine="codex-acp")])
    assert _resolve_route_model_name(_CONNECTION_ID, "gpt-5.6-luna", environ) is None


def test_unknown_placeholder_model_yields_none(tmp_path):
    environ = _save(tmp_path, [_entry("haiku")])
    assert _resolve_route_model_name(_CONNECTION_ID, "unknown", environ) is None
