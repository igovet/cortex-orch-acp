"""Every advertised Cortex argument reaches a Codex code-mode model intact.

Codex code mode shows a model each MCP tool as a TypeScript declaration
(``codex_code_mode`` ports its 0.159.1 pipeline). Two losses were observed in
live sessions: schemas over the 5,000-byte compaction default arrived with
every description stripped (plan_submit, result_submit), and a root
``oneOf`` / item ``allOf`` rendered instead of the fields
(``{ [key: string]: unknown; } | ...``, ``checks: Array<unknown>``).
"""
from __future__ import annotations

import pytest

import codex_code_mode
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.engine.process import CODEX_MCP_TOOL_SCHEMA_MAX_BYTES
from cortex_runtime.server import channeled_base_tools

PROFILES = [
    ("coordinator", None, None),
    ("worker", "worker-backend-dev", "execution"),
    ("worker", "worker-explorer", "execution"),
    ("worker", "worker-code-reviewer", "review"),
    ("worker", "worker-senior-consultant", "execution"),
]


def _tools():
    for role_kind, role_id, stage in PROFILES:
        for tool in list(channeled_base_tools()) + mcp_tools.tool_definitions(role_kind, role_id=role_id, stage=stage):
            yield pytest.param(tool, id=f"{role_id or role_kind}-{tool['name']}")


def _described(node, path="args"):
    if not isinstance(node, dict):
        return
    for name, child in (node.get("properties") or {}).items():
        if isinstance(child, dict) and child.get("description"):
            yield f"{path}.{name}", child["description"]
        yield from _described(child, f"{path}.{name}")
    yield from _described(node.get("items"), f"{path}[]")
    for key in ("anyOf", "oneOf", "allOf"):
        for index, branch in enumerate(node.get(key) or ()):
            yield from _described(branch, f"{path}<{key}{index}>")


@pytest.mark.parametrize("tool", _tools())
def test_codex_code_mode_declaration_keeps_every_field_and_description(tool):
    schema = tool["inputSchema"]
    assert codex_code_mode.normalized_size(schema) <= CODEX_MCP_TOOL_SCHEMA_MAX_BYTES
    declaration = codex_code_mode.render(schema, CODEX_MCP_TOOL_SCHEMA_MAX_BYTES)
    if tool["name"] == "read_batch":
        # Intentionally dynamic native arguments, preflighted against live
        # schemas; not an accidentally untyped root or item union.
        assert declaration.count("unknown") == 1
        assert "arguments: { [key: string]: unknown; };" in declaration
        assert "id: string;" in declaration and "tool:" in declaration
    else:
        assert "unknown" not in declaration
    missing = [path for path, text in _described(schema)
               if f"// {text.splitlines()[0].strip()}" not in declaration]
    assert not missing


def test_codex_default_compaction_is_what_hid_plan_submit_descriptions():
    """Pins the observed failure: without the configured limit the scope rule vanishes."""
    (plan,) = [tool for tool in mcp_tools.tool_definitions("coordinator") if tool["name"] == "plan_submit"]
    assert codex_code_mode.normalized_size(plan["inputSchema"]) > codex_code_mode.DEFAULT_COMPACT_BYTES
    assert "//" not in codex_code_mode.render(plan["inputSchema"])
    assert "Required: 1 to 64 items" in codex_code_mode.render(plan["inputSchema"], CODEX_MCP_TOOL_SCHEMA_MAX_BYTES)


def test_port_renders_a_root_one_of_as_codex_does():
    schema = {"type": "object", "properties": {"a": {"type": "string", "description": "A."}},
              "oneOf": [{"required": ["a"]}, {"not": {"required": ["a"]}}]}
    assert codex_code_mode.render(schema, CODEX_MCP_TOOL_SCHEMA_MAX_BYTES) == (
        "{ [key: string]: unknown; } | unknown")
