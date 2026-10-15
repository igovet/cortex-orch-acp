"""A compact, schema-derived tool cheat sheet for codex-acp sessions only.

Codex's code mode exposes Cortex MCP tools behind ``exec``: the model sees
only each tool's name and description, never its full ``inputSchema``. This
module mechanically renders a short reference -- required fields plus one
TS-style example call, the shape code mode presents -- for the handful of
most-used tools, derived live from
:func:`cortex_runtime.delegation.mcp_tools.tool_definitions` so it can never
silently drift out of sync with the real schema.

Only :func:`cortex_runtime.delegation.live_adapters._session_new_meta_for`'s
``codex-acp`` branch calls this; Claude and OpenCode sessions never receive
the rendered text.
"""
from __future__ import annotations

import json
from typing import Any, Mapping

from . import mcp_tools

#: Hard character budget for the rendered cheat sheet text.
CHEAT_SHEET_BUDGET_CHARS = 8000

#: Preferred tool order -- the most-used tools across worker/coordinator
#: delegation operations go first; present but unlisted tools fill any
#: remaining slots in sorted (deterministic) name order.
_PREFERRED_TOOL_ORDER = (
    "result_submit",
    "command_run",
    "access_request",
    "ask_question",
    "project_edit",
    "read_report",
)

#: Maximum number of tools rendered, regardless of budget.
_MAX_TOOLS = 6

_HEADER = (
    "# Cortex tool cheat sheet (code mode)\n\n"
    "Your code-mode view only shows each tool's name and description; the full argument schema is hidden "
    "behind exec. This sheet lists each tool's required fields and one example call in the shape code mode "
    "exposes tools (a TS-style await call) -- it documents the live schema, it is not code to execute "
    "literally. Call each tool with its real arguments; consult the schema itself for optional fields.\n\n"
)


def _example_value(name: str, prop: Mapping[str, Any]) -> Any:
    """A small, deterministic placeholder value for one schema property."""
    enum = prop.get("enum")
    if isinstance(enum, list) and enum:
        return enum[0]
    prop_type = prop.get("type")
    if prop_type == "array":
        return []
    if prop_type == "object":
        return {}
    if prop_type == "boolean":
        return True
    if prop_type in ("integer", "number"):
        return 0
    return f"<{name}>"


def _example_call(tool_name: str, schema: Mapping[str, Any]) -> str:
    properties = schema.get("properties") or {}
    required = schema.get("required") or []
    args = {name: _example_value(name, properties.get(name) or {}) for name in required}
    args_json = json.dumps(args, ensure_ascii=False)
    return f"await cortex.{tool_name}({args_json});"


def _tool_entry(tool: Mapping[str, Any]) -> str:
    name = tool["name"]
    schema = tool.get("inputSchema") or {}
    required = schema.get("required") or []
    required_text = ", ".join(required) if required else "none"
    return f"- {name}: required={required_text}\n  {_example_call(name, schema)}\n\n"


def _selected_tools(role_kind: str, *, role_id: str | None, stage: str | None) -> list[dict]:
    tools = mcp_tools.tool_definitions(role_kind, role_id=role_id, stage=stage)
    by_name = {tool["name"]: tool for tool in tools}
    ordered = [by_name[name] for name in _PREFERRED_TOOL_ORDER if name in by_name]
    if len(ordered) < _MAX_TOOLS:
        already = {tool["name"] for tool in ordered}
        extra = [tool for name, tool in sorted(by_name.items()) if name not in already]
        ordered += extra[: _MAX_TOOLS - len(ordered)]
    return ordered[:_MAX_TOOLS]


def codex_tool_cheat_sheet(
    role_kind: str, *, role_id: str | None = None, stage: str | None = None,
    budget_chars: int = CHEAT_SHEET_BUDGET_CHARS,
) -> str | None:
    """Render the cheat sheet for ``role_kind`` (+ optional ``role_id``,
    ``stage``), or ``None`` if the live schema offers no tools to render.

    Deterministic truncation: tools are added to the fixed, deterministic
    order above (preferred tools first, then the remaining offered tools in
    sorted name order) until the next entry would exceed ``budget_chars``;
    the render then stops and appends an explicit truncation note naming
    how many tools were omitted, rather than silently dropping them from the
    count.
    """
    selected = _selected_tools(role_kind, role_id=role_id, stage=stage)
    if not selected:
        return None
    used = len(_HEADER)
    parts: list[str] = []
    omitted = 0
    for index, tool in enumerate(selected):
        entry = _tool_entry(tool)
        if used + len(entry) > budget_chars:
            omitted = len(selected) - index
            break
        parts.append(entry)
        used += len(entry)
    text = _HEADER + "".join(parts)
    if omitted:
        note = f"(cheat sheet truncated to stay within budget; {omitted} more tool(s) omitted)\n"
        if len(text) + len(note) <= budget_chars:
            text += note
    text = text.rstrip("\n")
    if len(text) > budget_chars:
        # Deterministic last-resort truncation: a budget tighter than even
        # the fixed header is still honored exactly, never silently ignored.
        text = text[:budget_chars].rstrip("\n")
    return text
