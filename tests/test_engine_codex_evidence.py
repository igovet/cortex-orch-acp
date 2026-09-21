"""engine.codex_adapter_acp evidence statuses and instruction delivery."""
from __future__ import annotations

import pytest

from cortex_runtime.engine.codex_adapter_acp import (
    EXECUTION_VERIFICATION_UNAVAILABLE,
    CodexEvidenceStatus,
    build_codex_evidence_status,
)
from cortex_runtime.portable.errors import PortableError


def test_build_codex_evidence_status_distinct_fields():
    status = build_codex_evidence_status("cr_" + "a" * 12, "eirt_x:sess-1:model=model-a")
    assert status.selected == "cr_" + "a" * 12
    assert status.configuration_applied == "eirt_x:sess-1:model=model-a"
    assert status.execution_verified == EXECUTION_VERIFICATION_UNAVAILABLE
    # Selected/applied/verified are genuinely three distinct fields, not
    # aliases of one another.
    assert status.selected != status.configuration_applied
    assert status.execution_verified not in (status.selected, status.configuration_applied)


def test_evidence_status_execution_verified_is_never_settable_to_anything_else():
    with pytest.raises(PortableError) as excinfo:
        CodexEvidenceStatus(selected="cr_x", configuration_applied="ev_x", execution_verified="confirmed")
    assert excinfo.value.code == "invalid_field"


def test_evidence_status_requires_non_empty_selected_and_applied():
    with pytest.raises(PortableError):
        build_codex_evidence_status("", "ev_x")
    with pytest.raises(PortableError):
        build_codex_evidence_status("cr_x", "")
