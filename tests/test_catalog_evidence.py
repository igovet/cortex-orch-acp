"""Fact/Provenance/EvidenceState promotion rules (T007)."""
from __future__ import annotations

import pytest

from cortex_runtime.catalog.evidence import EvidenceState, Fact, Provenance, ProvenanceSource
from cortex_runtime.portable.errors import PortableError


def _provenance(source, evidence_ref=None):
    return Provenance(source=source, adapter_version="1.0.0", observed_at="2026-01-01T00:00:00.000000+00:00", evidence_ref=evidence_ref)


def test_empty_state_has_no_facts():
    state = EvidenceState.empty()
    assert not state.has(Fact.CONFIGURED)
    assert state.get(Fact.CONFIGURED) is None


def test_promote_returns_new_immutable_state_without_mutating_original():
    empty = EvidenceState.empty()
    updated = empty.promote(Fact.CONFIGURED, _provenance(ProvenanceSource.DISCOVERY))
    assert not empty.has(Fact.CONFIGURED)
    assert updated.has(Fact.CONFIGURED)


def test_manual_never_sets_catalog_obtained():
    state = EvidenceState.empty()
    with pytest.raises(PortableError) as excinfo:
        state.promote(Fact.CATALOG_OBTAINED, _provenance(ProvenanceSource.MANUAL))
    assert excinfo.value.code == "unverified_promotion"


def test_discovery_may_set_catalog_obtained():
    state = EvidenceState.empty().promote(Fact.CATALOG_OBTAINED, _provenance(ProvenanceSource.DISCOVERY))
    assert state.has(Fact.CATALOG_OBTAINED)


@pytest.mark.parametrize("fact", [Fact.CONFIGURATION_APPLIED, Fact.EXECUTION_VERIFIED])
def test_confirmation_facts_refuse_non_engine_sources(fact):
    state = EvidenceState.empty()
    with pytest.raises(PortableError) as excinfo:
        state.promote(fact, _provenance(ProvenanceSource.MANUAL, evidence_ref="ev-1"))
    assert excinfo.value.code == "unverified_promotion"
    with pytest.raises(PortableError):
        state.promote(fact, _provenance(ProvenanceSource.DISCOVERY, evidence_ref="ev-1"))


@pytest.mark.parametrize("fact", [Fact.CONFIGURATION_APPLIED, Fact.EXECUTION_VERIFIED])
def test_confirmation_facts_require_non_empty_evidence_ref(fact):
    state = EvidenceState.empty()
    with pytest.raises(PortableError) as excinfo:
        state.promote(fact, _provenance(ProvenanceSource.ENGINE_REPORT, evidence_ref=None))
    assert excinfo.value.code == "missing_confirmation_evidence"


@pytest.mark.parametrize("fact,source", [
    (Fact.CONFIGURATION_APPLIED, ProvenanceSource.ENGINE_REPORT),
    (Fact.EXECUTION_VERIFIED, ProvenanceSource.OBSERVATION),
])
def test_confirmation_facts_succeed_with_qualifying_evidence(fact, source):
    state = EvidenceState.empty().promote(fact, _provenance(source, evidence_ref="ev-1"))
    assert state.has(fact)
    assert state.get(fact).evidence_ref == "ev-1"


def test_selected_fact_has_no_source_restriction():
    state = EvidenceState.empty().promote(Fact.SELECTED, _provenance(ProvenanceSource.MANUAL))
    assert state.has(Fact.SELECTED)


def test_direct_construction_bypassing_promote_is_rejected():
    # Directly constructing an EvidenceState with execution_verified from
    # manual provenance must fail, even though it would never pass promote().
    with pytest.raises(PortableError) as excinfo:
        EvidenceState({Fact.EXECUTION_VERIFIED: _provenance(ProvenanceSource.MANUAL)})
    assert excinfo.value.code == "invalid_evidence_construction"


def test_direct_construction_with_correct_looking_token_guess_is_rejected():
    with pytest.raises(PortableError) as excinfo:
        EvidenceState({}, object())
    assert excinfo.value.code == "invalid_evidence_construction"


def test_setattr_after_construction_is_rejected():
    state = EvidenceState.empty()
    with pytest.raises(PortableError) as excinfo:
        state._facts = {}
    assert excinfo.value.code == "immutable_evidence_state"


def test_pickle_and_deepcopy_reconstruction_are_rejected():
    # pickle/copy.deepcopy would otherwise call cls.__new__ directly and
    # restore state without ever going through __init__'s token check; both
    # must fail closed rather than silently forging a state.
    import copy
    import pickle

    state = EvidenceState.empty().promote(Fact.CONFIGURED, _provenance(ProvenanceSource.DISCOVERY))
    with pytest.raises(PortableError) as excinfo:
        pickle.dumps(state)
    assert excinfo.value.code == "unpicklable_evidence_state"
    with pytest.raises(PortableError) as excinfo:
        copy.deepcopy(state)
    assert excinfo.value.code == "unpicklable_evidence_state"


def test_deserialization_boundary_reconstructs_only_through_promote(tmp_path, monkeypatch):
    # The catalog.store deserialization path (the "deserialization boundary"
    # referenced by the review) only ever calls EvidenceState.empty()/promote()
    # when reading a persisted snapshot back; a tampered evidence blob that
    # would violate promote()'s rules must fail closed, not silently load.
    import json

    from cortex_runtime.catalog import store as catalog_store

    tampered = {
        Fact.EXECUTION_VERIFIED.value: {
            "source": ProvenanceSource.MANUAL.value,
            "adapter_version": "1.0",
            "observed_at": "2026-01-01T00:00:00+00:00",
            "evidence_ref": None,
        }
    }
    with pytest.raises(PortableError) as excinfo:
        catalog_store._deserialize_evidence(tampered)
    assert excinfo.value.code in ("unverified_promotion", "missing_confirmation_evidence")
