"""CatalogEntry/CatalogSnapshot invariants and derived views (T007)."""
from __future__ import annotations

import pytest

from cortex_runtime.catalog.evidence import EvidenceState
from cortex_runtime.catalog.model import CatalogEntry, CatalogSnapshot
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import ExecutionRoute, new_id


def _route(connection_id="cn_" + "a" * 12, model="model-a"):
    return ExecutionRoute(engine="codex-acp", connection_id=connection_id, model=model)


def test_entry_rejects_mismatched_connection_id():
    route = _route()
    with pytest.raises(PortableError) as excinfo:
        CatalogEntry(
            route=route,
            connection_id="cn_" + "b" * 12,
            label="l",
            disambiguated_label="l",
            evidence=EvidenceState.empty(),
            catalog_snapshot_id=new_id("cs_"),
        )
    assert excinfo.value.code == "route_connection_mismatch"


def test_snapshot_entry_for_route_and_all_entries():
    route = _route()
    snapshot_id = new_id("cs_")
    entry = CatalogEntry(
        route=route, connection_id=route.connection_id, label="l", disambiguated_label="l",
        evidence=EvidenceState.empty(), catalog_snapshot_id=snapshot_id,
    )
    snapshot = CatalogSnapshot(
        id=snapshot_id, created_at="2026-01-01T00:00:00+00:00",
        connection_registry_fingerprint="fp", entries=(entry,), unavailable={},
    )
    assert snapshot.all_entries() == (entry,)
    assert snapshot.entry_for_route(route.id) is entry
    assert snapshot.entry_for_route("xr_" + "0" * 24) is None


def test_snapshot_unavailable_is_immutable_mapping():
    snapshot = CatalogSnapshot(
        id=new_id("cs_"), created_at="2026-01-01T00:00:00+00:00",
        connection_registry_fingerprint="fp", entries=(), unavailable={"cn_x": "discovery_unavailable"},
    )
    with pytest.raises(TypeError):
        snapshot.unavailable["cn_x"] = "changed"


# --- T024: optional context_window_tokens (backward-compatible) -----------


def test_context_window_tokens_defaults_to_none_for_every_existing_caller():
    route = _route()
    entry = CatalogEntry(
        route=route, connection_id=route.connection_id, label="l", disambiguated_label="l",
        evidence=EvidenceState.empty(), catalog_snapshot_id=new_id("cs_"),
    )
    assert entry.context_window_tokens is None


def test_context_window_tokens_accepts_a_positive_integer():
    route = _route()
    entry = CatalogEntry(
        route=route, connection_id=route.connection_id, label="l", disambiguated_label="l",
        evidence=EvidenceState.empty(), catalog_snapshot_id=new_id("cs_"), context_window_tokens=128_000,
    )
    assert entry.context_window_tokens == 128_000


@pytest.mark.parametrize("bad_value", [0, -1, "128000", 1.5, True])
def test_context_window_tokens_rejects_non_positive_or_wrong_type(bad_value):
    route = _route()
    with pytest.raises(PortableError) as excinfo:
        CatalogEntry(
            route=route, connection_id=route.connection_id, label="l", disambiguated_label="l",
            evidence=EvidenceState.empty(), catalog_snapshot_id=new_id("cs_"), context_window_tokens=bad_value,
        )
    assert excinfo.value.code == "invalid_field"
