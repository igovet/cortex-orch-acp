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

from cortex_runtime.channeled_tools import channeled_base_tools
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
    assert "await tools.mcp__cortex__result_submit(" in text
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
    entry = next(line for line in text.splitlines() if line.startswith("- result_submit: required="))
    required_part = entry.removeprefix("- result_submit: required=").split("; optional: ")[0]
    positions = [required_part.index(field) for field in schema["required"]]
    assert positions == sorted(positions)
    example_line = next(
        line.strip() for line in text.splitlines()
        if line.strip().startswith("await tools.mcp__cortex__result_submit(")
    )
    example = json.loads(example_line.removeprefix("await tools.mcp__cortex__result_submit(").removesuffix(");"))
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
    # The header alone is about 570 characters; leave room for a few entries.
    tight = codex_tool_cheat_sheet("worker", budget_chars=900)
    assert tight is not None
    assert len(tight) < len(full)
    assert len(tight) <= 900
    assert "truncated" in tight or "omitted" in tight


def test_only_codex_acp_sessions_receive_the_cheat_sheet_marker():
    codex_meta = _session_new_meta_for("codex-acp", "1.12.0", "Be helpful.")
    assert codex_meta is not None
    assert "Cortex tool cheat sheet" in codex_meta["systemPrompt"]
    assert "await tools.mcp__cortex__" in codex_meta["systemPrompt"]

    claude_meta = _session_new_meta_for("claude-agent-acp", "0.78.0", "Be helpful.")
    assert claude_meta is not None
    claude_system_prompt = claude_meta["systemPrompt"]
    assert "Cortex tool cheat sheet" not in claude_system_prompt
    assert "await tools.mcp__cortex__" not in claude_system_prompt


def test_cheat_sheet_absent_from_claude_native_switch_disabled_variant():
    environ = {"CORTEX_UNIFIED_BASE_PROMPT": "0"}
    claude_meta = _session_new_meta_for("claude-agent-acp", "0.78.0", "Be helpful.", environ=environ)
    assert claude_meta is not None
    rendered = claude_meta["systemPrompt"]
    rendered_text = rendered if isinstance(rendered, str) else str(rendered)
    assert "Cortex tool cheat sheet" not in rendered_text
    assert "await tools.mcp__cortex__" not in rendered_text


@pytest.mark.parametrize("role_kind,role_id,stage", [
    ("coordinator", None, None),
    ("worker", "worker-backend-dev", None),
    ("worker", "worker-backend-dev", "review"),
    ("worker", "worker-explorer", None),
    ("worker", "worker-senior-consultant", None),
])
def test_cheat_sheet_covers_the_whole_channel_catalogue(role_kind, role_id, stage):
    """Every tool the channel's tools/list serves gets a line, with its
    optional fields, so code mode never has to guess or query ALL_TOOLS."""
    catalogue = list(channeled_base_tools()) + mcp_tools.tool_definitions(role_kind, role_id=role_id, stage=stage)
    text = codex_tool_cheat_sheet(role_kind, role_id=role_id, stage=stage)
    assert text is not None
    assert len(text) <= CHEAT_SHEET_BUDGET_CHARS
    assert "truncated" not in text
    lines = {line.split(":", 1)[0].removeprefix("- "): line for line in text.splitlines() if line.startswith("- ")}
    assert set(lines) == {tool["name"] for tool in catalogue}
    for tool in catalogue:
        schema = tool.get("inputSchema") or {}
        optional = [field for field in schema.get("properties") or {} if field not in (schema.get("required") or [])]
        if optional:
            tail = lines[tool["name"]].split("; optional: ", 1)[1]
            assert all(field in tail for field in optional), tool["name"]
    examples = [line for line in text.splitlines() if line.startswith("  await tools.mcp__cortex__")]
    assert len(examples) == min(6, len(catalogue))


def test_cheat_sheet_renders_enums_and_array_item_fields():
    text = codex_tool_cheat_sheet("worker", role_id="worker-backend-dev")
    assert text is not None
    assert "- project_git: required=kind=status|diff, request_key" in text
    assert "edits[{old_string, new_string, replace_all?: boolean}]" in text
    # checks[].command is an argv array; a name-only line let a live Codex
    # worker send it as a string (refused, 2026-10-22).
    assert "command?: string[]" in text
    assert "- command_run: required=argv: string[], request_key; optional: cwd, timeout_seconds: integer" in text
    schema = next(tool["inputSchema"] for tool in mcp_tools.tool_definitions("worker", role_id="worker-backend-dev")
                  if tool["name"] == "result_submit")
    outcomes = schema["properties"]["checks"]["items"]["properties"]["outcome"]["enum"]
    assert "outcome=" + "|".join(outcomes) in text


@pytest.mark.parametrize("schema,expected", [
    ({"type": "integer", "minimum": 1, "maximum": 2000}, "value: integer 1..2000"),
    ({"type": "integer", "minimum": 0}, "value: integer >=0"),
    ({"type": "integer", "maximum": 2000}, "value: integer <=2000"),
    ({"type": "integer"}, "value: integer"),
    ({"type": "number", "exclusiveMinimum": -0.5}, "value: number >-0.5"),
    ({"type": "number", "exclusiveMaximum": 2.5}, "value: number <2.5"),
    ({"type": "number", "minimum": 0, "exclusiveMaximum": 1}, "value: number >=0 <1"),
    ({"type": "integer", "minimum": 0, "maximum": 10,
      "exclusiveMinimum": 1, "exclusiveMaximum": 9}, "value: integer >=0 <=10 >1 <9"),
])
def test_field_renders_numeric_bounds(schema, expected):
    assert cheat_sheet_module._field("value", schema) == expected


def test_bounds_render_for_nested_optional_and_enum_fields():
    assert cheat_sheet_module._field("values", {
        "type": "array", "items": {
            "type": "object", "properties": {
                "count": {"type": "integer", "minimum": 1},
            },
        },
    }) == "values[{count?: integer >=1}]"
    assert cheat_sheet_module._field("count", {
        "type": "integer", "enum": [1, 2], "minimum": 1,
    }) == "count=1|2 >=1"


def test_project_read_limit_bounds_follow_live_schema(monkeypatch):
    real_tool_definitions = mcp_tools.tool_definitions
    schema = next(tool["inputSchema"] for tool in real_tool_definitions("worker")
                  if tool["name"] == "project_read")
    limit = schema["properties"]["limit"]
    assert (limit["minimum"], limit["maximum"]) == (1, 2000)
    baseline = codex_tool_cheat_sheet("worker")
    entry = next(line for line in baseline.splitlines() if line.startswith("- project_read:"))
    assert f"limit: integer {limit['minimum']}..{limit['maximum']}" in entry

    def patched(role_kind, *, role_id=None, stage=None):
        tools = copy.deepcopy(real_tool_definitions(role_kind, role_id=role_id, stage=stage))
        for tool in tools:
            if tool["name"] == "project_read":
                tool["inputSchema"]["properties"]["limit"].update(minimum=3, maximum=47)
        return tools

    monkeypatch.setattr(cheat_sheet_module.mcp_tools, "tool_definitions", patched)
    changed = codex_tool_cheat_sheet("worker")
    entry = next(line for line in changed.splitlines() if line.startswith("- project_read:"))
    assert "limit: integer 3..47" in entry
    assert "limit: integer 1..2000" not in entry
    assert changed != baseline
