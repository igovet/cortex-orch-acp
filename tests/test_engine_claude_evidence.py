"""engine.claude_adapter_acp evidence statuses and instruction delivery.

Mirrors tests/test_engine_codex_evidence.py."""
from __future__ import annotations

import pytest
import json
from pathlib import Path

from cortex_runtime.engine.claude_adapter_acp import (
    EXECUTION_VERIFICATION_UNAVAILABLE,
    ClaudeEvidenceStatus,
    build_claude_evidence_status,
    build_claude_session_new_meta,
    build_first_prompt_blocks,
)
from cortex_runtime.portable.errors import PortableError


def test_build_claude_evidence_status_distinct_fields():
    status = build_claude_evidence_status("cr_" + "a" * 12, "eirt_x:sess-1:model=model-a")
    assert status.selected == "cr_" + "a" * 12
    assert status.configuration_applied == "eirt_x:sess-1:model=model-a"
    assert status.execution_verified == EXECUTION_VERIFICATION_UNAVAILABLE
    assert status.selected != status.configuration_applied
    assert status.execution_verified not in (status.selected, status.configuration_applied)


def test_evidence_status_execution_verified_is_never_settable_to_anything_else():
    with pytest.raises(PortableError) as excinfo:
        ClaudeEvidenceStatus(selected="cr_x", configuration_applied="ev_x", execution_verified="confirmed")
    assert excinfo.value.code == "invalid_field"


def test_evidence_status_requires_non_empty_selected_and_applied():
    with pytest.raises(PortableError):
        build_claude_evidence_status("", "ev_x")
    with pytest.raises(PortableError):
        build_claude_evidence_status("cr_x", "")


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
    monkeypatch.chdir(tmp_path)
    before = sorted(p.name for p in tmp_path.iterdir())
    blocks = build_first_prompt_blocks("instructions", [])
    after = sorted(p.name for p in tmp_path.iterdir())
    assert before == after == []
    assert blocks == [{"type": "text", "text": "instructions"}]


def test_build_claude_session_new_meta_preserves_preset_with_append():
    meta = build_claude_session_new_meta("You are operating under Cortex orchestration.")
    assert meta == {
        "systemPrompt": {"append": "You are operating under Cortex orchestration."},
        "claudeCode": {"options": {"disallowedTools": ["Agent", "Task"], "strictMcpConfig": True}},
    }


def test_build_claude_session_new_meta_rejects_empty_instructions():
    with pytest.raises(PortableError) as excinfo:
        build_claude_session_new_meta("   ")
    assert excinfo.value.code == "invalid_field"


def test_installed_claude_acp_0_78_forwards_strict_mcp_to_sdk_cli():
    """No engine call: verify this host's exact pinned adapter and SDK path."""
    root = Path.home() / ".local/share/zed/external_agents/registry/npx/claude-acp/node_modules"
    adapter = root / "@agentclientprotocol/claude-agent-acp"
    sdk = root / "@anthropic-ai/claude-agent-sdk"
    if not (adapter / "package.json").exists() or not (sdk / "sdk.mjs").exists():
        pytest.skip("Installed Claude ACP/SDK copy is unavailable on this host")
    version = json.loads((adapter / "package.json").read_text(encoding="utf-8"))["version"]
    if version != "0.78.0":
        pytest.skip(f"Installed Claude ACP is {version}, not pinned 0.78.0")
    adapter_source = (adapter / "dist/acp-agent.js").read_text(encoding="utf-8")
    sdk_source = (sdk / "sdk.mjs").read_text(encoding="utf-8")
    sdk_types = (sdk / "sdk.d.ts").read_text(encoding="utf-8")
    assert "...userProvidedOptions," in adapter_source
    assert "...(userProvidedOptions?.mcpServers || {})," in adapter_source
    assert "...mcpServers," in adapter_source
    assert "strictMcpConfig?: boolean;" in sdk_types
    assert "Only use MCP servers passed via the `mcpServers` option" in sdk_types
    assert 'if(ee)W.push("--strict-mcp-config")' in sdk_source
