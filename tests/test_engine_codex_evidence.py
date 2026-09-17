"""engine.codex_adapter_acp evidence statuses and instruction delivery."""
from __future__ import annotations

import pytest

from cortex_runtime.engine.codex_adapter_acp import (
    EXECUTION_VERIFICATION_UNAVAILABLE,
    CodexEvidenceStatus,
    build_codex_evidence_status,
    build_first_prompt_blocks,
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


def test_build_first_prompt_blocks_prepends_leading_text_block():
    blocks = build_first_prompt_blocks("You are operating under Cortex orchestration.", [{"type": "text", "text": "do the thing"}])
    assert blocks[0] == {"type": "text", "text": "You are operating under Cortex orchestration."}
    assert blocks[1] == {"type": "text", "text": "do the thing"}
    assert len(blocks) == 2


def test_build_first_prompt_blocks_rejects_empty_instructions():
    with pytest.raises(PortableError) as excinfo:
        build_first_prompt_blocks("   ", [])
    assert excinfo.value.code == "invalid_field"


def test_build_first_prompt_blocks_rejects_non_list_user_blocks():
    with pytest.raises(PortableError) as excinfo:
        build_first_prompt_blocks("instructions", "not-a-list")
    assert excinfo.value.code == "invalid_field"


def test_build_first_prompt_blocks_never_writes_a_file_or_returns_env_vars(tmp_path, monkeypatch):
    # Documented as unqualified: the only effect is the returned leading
    # block -- no filesystem write (e.g. an AGENTS.md into the workspace)
    # and no environment mutation of any kind.
    monkeypatch.chdir(tmp_path)
    before = sorted(p.name for p in tmp_path.iterdir())
    blocks = build_first_prompt_blocks("instructions", [])
    after = sorted(p.name for p in tmp_path.iterdir())
    assert before == after == []
    assert blocks == [{"type": "text", "text": "instructions"}]
