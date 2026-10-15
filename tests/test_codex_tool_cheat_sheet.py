"""Tests for the codex-acp-only, schema-derived tool cheat sheet.

Covers: the rendered text is derived from the live tool schema (a schema
change changes the cheat sheet, so it cannot silently drift out of sync),
it stays within its character budget, and non-codex engines never receive
it via ``_session_new_meta_for``.
"""
from __future__ import annotations

import copy
import json

import pytest

from cortex_runtime.delegation import codex_tool_cheat_sheet as cheat_sheet_module
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.codex_tool_cheat_sheet import (
    CHEAT_SHEET_BUDGET_CHARS,
    codex_tool_cheat_sheet,
)
from cortex_runtime.delegation.live_adapters import _session_new_meta_for


def test_cheat_sheet_renders_required_fields_and_example_call():
    text = codex_tool_cheat_sheet("worker")
    assert text is not None
    assert "result_submit" in text
    assert "await cortex.result_submit(" in text
    # result_submit's required fields (report, summary, checks, request_key)
    # must all appear in its rendered entry.
    for field in ("report", "summary", "checks", "request_key"):
        assert field in text


@pytest.mark.parametrize("stage", [None, "execution", "review"])
@pytest.mark.parametrize("unified", [True, False])
def test_cheat_sheet_matches_stage_scoped_result_schema(stage, unified):
    role_id = "worker-backend-dev"
    schema = next(
        tool["inputSchema"] for tool in mcp_tools.tool_definitions(
            "worker", role_id=role_id, stage=stage,
        ) if tool["name"] == "result_submit"
    )
    text = codex_tool_cheat_sheet("worker", role_id=role_id, stage=stage)
    assert text is not None
    assert len(text) <= CHEAT_SHEET_BUDGET_CHARS
    assert "- result_submit: required=" + ", ".join(schema["required"]) in text
    example_line = next(
        line.strip() for line in text.splitlines()
        if line.strip().startswith("await cortex.result_submit(")
    )
    example = json.loads(example_line.removeprefix("await cortex.result_submit(").removesuffix(");"))
    assert set(example) == set(schema["required"])
    assert ("verdict" in example) == (stage == "review")
    if stage == "review":
        assert example["verdict"] in schema["properties"]["verdict"]["enum"]
        assert "project_edit" not in text

    meta = _session_new_meta_for(
        "codex-acp", "2.1.1", "Role instructions", role_id=role_id, stage=stage,
        environ={} if unified else {"CORTEX_UNIFIED_BASE_PROMPT": "0"},
    )
    assert meta["systemPrompt"] == "Role instructions\n\n" + text


def test_cheat_sheet_stays_within_character_budget():
    text = codex_tool_cheat_sheet("worker")
    assert text is not None
    assert len(text) <= CHEAT_SHEET_BUDGET_CHARS

    narrow = codex_tool_cheat_sheet("worker", budget_chars=400)
    assert narrow is not None
    assert len(narrow) <= 400


def test_cheat_sheet_is_derived_from_the_live_schema(monkeypatch):
    """Changing a tool's live schema changes the rendered cheat sheet --
    proof the text is mechanically derived, not a hand-written copy."""
    baseline = codex_tool_cheat_sheet("worker")
    assert baseline is not None

    real_tool_definitions = mcp_tools.tool_definitions

    def patched(role_kind, *, role_id=None, stage=None):
        tools = copy.deepcopy(real_tool_definitions(role_kind, role_id=role_id, stage=stage))
        for tool in tools:
            if tool["name"] == "result_submit":
                tool["inputSchema"] = dict(tool["inputSchema"])
                tool["inputSchema"]["required"] = list(tool["inputSchema"]["required"]) + ["zzz_marker_field"]
        return tools

    monkeypatch.setattr(cheat_sheet_module.mcp_tools, "tool_definitions", patched)
    changed = codex_tool_cheat_sheet("worker")
    assert changed is not None
    assert changed != baseline
    assert "zzz_marker_field" in changed


def test_cheat_sheet_records_an_explicit_omission_note_when_truncated():
    # A tight budget still records an explicit omission count rather than
    # silently dropping tools out of the render.
    full = codex_tool_cheat_sheet("worker")
    tight = codex_tool_cheat_sheet("worker", budget_chars=600)
    assert tight is not None
    assert len(tight) < len(full)
    assert len(tight) <= 600
    assert "truncated" in tight or "omitted" in tight


def test_only_codex_acp_sessions_receive_the_cheat_sheet_marker():
    codex_meta = _session_new_meta_for("codex-acp", "1.12.0", "Be helpful.")
    assert codex_meta is not None
    assert "Cortex tool cheat sheet" in codex_meta["systemPrompt"]
    assert "await cortex." in codex_meta["systemPrompt"]

    claude_meta = _session_new_meta_for("claude-agent-acp", "0.78.0", "Be helpful.")
    assert claude_meta is not None
    claude_system_prompt = claude_meta["systemPrompt"]
    assert "Cortex tool cheat sheet" not in claude_system_prompt
    assert "await cortex." not in claude_system_prompt


def test_cheat_sheet_absent_from_claude_native_switch_disabled_variant():
    environ = {"CORTEX_UNIFIED_BASE_PROMPT": "0"}
    claude_meta = _session_new_meta_for("claude-agent-acp", "0.78.0", "Be helpful.", environ=environ)
    assert claude_meta is not None
    rendered = claude_meta["systemPrompt"]
    rendered_text = rendered if isinstance(rendered, str) else str(rendered)
    assert "Cortex tool cheat sheet" not in rendered_text
    assert "await cortex." not in rendered_text
