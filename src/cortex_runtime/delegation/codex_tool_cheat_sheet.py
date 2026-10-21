"""A compact, schema-derived tool cheat sheet for codex-acp sessions only.

Codex's code mode exposes Cortex MCP tools behind ``exec``: the model sees
only each tool's name and description, never its full ``inputSchema``, and
its ``ALL_TOOLS`` array returns only those two fields (observed in retained
Codex rollouts on 2026-10-22, where 10% of ``exec`` calls only queried it).
This module mechanically renders a short reference -- one signature line
with required and optional fields for every offered Cortex tool, plus one
example call in code mode's own ``tools.mcp__cortex__<name>`` shape for the
most-used tools -- derived live from the same catalogue a channel's
``tools/list`` serves (:func:`cortex_runtime.channeled_tools.channeled_base_tools`
plus :func:`cortex_runtime.delegation.mcp_tools.tool_definitions`) so it can
never silently drift out of sync with the real schema.

Only :func:`cortex_runtime.delegation.live_adapters._session_new_meta_for`'s
``codex-acp`` branch calls this; Claude and OpenCode sessions never receive
the rendered text.
"""
from __future__ import annotations

import json
from typing import Any, Mapping

from ..channeled_tools import channeled_base_tools
from . import mcp_tools

#: Hard character budget for the rendered cheat sheet text.
CHEAT_SHEET_BUDGET_CHARS = 8000

#: Preferred tool order -- the most-used tools across worker/coordinator
#: delegation operations go first; the remaining offered tools follow in
#: sorted (deterministic) name order.
_PREFERRED_TOOL_ORDER = (
    "result_submit",
    "command_run",
    "project_read",
    "project_edit",
    "plan_submit",
    "synthesis_submit",
    "access_request",
    "ask_question",
    "read_report",
    "read_skill",
)

#: How many of the first rendered tools also get an example call.
_EXAMPLE_TOOLS = 6

#: Enum values shown per field before the rest are counted, not dropped.
_MAX_ENUM_VALUES = 6

_HEADER = (
    "# Cortex tool cheat sheet (code mode)\n\n"
    "Code mode shows each tool's name and description only, and `ALL_TOOLS` returns the same two fields. "
    "This sheet is rendered from the live schemas: each Cortex tool offered to this session with its required "
    "fields, then `optional:` fields; enum values follow `=`, `?` marks an optional item field, and a field without a `: type` is a string. The most-used tools also show one example "
    "call with placeholder values; it is not code to execute literally. Call a tool with its real arguments "
    "as `await tools.mcp__cortex__<name>({...})`.\n\n"
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
    return f"await tools.mcp__cortex__{tool_name}({args_json});"


def _type_suffix(prop: Mapping[str, Any]) -> str:
    """``: string[]``-style type for anything but a plain string field. A
    live 2026-10-22 Codex worker sent ``checks[].command`` (an argv array)
    as a bare name-only field's guessed string and was refused."""
    prop_type = prop.get("type")
    if prop_type == "array":
        items = prop.get("items")
        item_type = items.get("type") if isinstance(items, Mapping) else None
        return f": {item_type or 'any'}[]"
    if isinstance(prop_type, str) and prop_type != "string":
        return f": {prop_type}"
    return ""


def _field(name: str, prop: Mapping[str, Any], *, nested: bool = True, optional: bool = False) -> str:
    """``name``, ``name=a|b`` for an enum, ``name[{x, y?}]`` for an array of
    objects (one level deep; ``?`` marks an optional item field), otherwise
    ``name`` with a ``: type`` suffix unless the field is a string."""
    mark = "?" if optional else ""
    enum = prop.get("enum")
    if isinstance(enum, list) and enum:
        shown = "|".join(str(value) for value in enum[:_MAX_ENUM_VALUES])
        more = len(enum) - _MAX_ENUM_VALUES
        return f"{name}{mark}={shown}" + (f"|(+{more} more)" if more > 0 else "")
    items = prop.get("items")
    if nested and prop.get("type") == "array" and isinstance(items, Mapping) and items.get("properties"):
        item_props = items["properties"]
        item_required = list(items.get("required") or [])
        fields = [_field(field, item_props.get(field) or {}, nested=False) for field in item_required]
        fields += [_field(field, item_props[field], nested=False, optional=True)
                   for field in item_props if field not in item_required]
        return f"{name}{mark}[{{{', '.join(fields)}}}]"
    return name + mark + _type_suffix(prop)


def _tool_entry(tool: Mapping[str, Any], *, example: bool) -> str:
    name = tool["name"]
    schema = tool.get("inputSchema") or {}
    properties = schema.get("properties") or {}
    required = list(schema.get("required") or [])
    optional = [field for field in properties if field not in required]
    parts = ", ".join(_field(field, properties.get(field) or {}) for field in required) or "none"
    if optional:
        parts += "; optional: " + ", ".join(_field(field, properties.get(field) or {}) for field in optional)
    entry = f"- {name}: required={parts}\n"
    if example:
        entry += f"  {_example_call(name, schema)}\n"
    return entry


def _selected_tools(role_kind: str, *, role_id: str | None, stage: str | None) -> list[dict]:
    tools = list(channeled_base_tools()) + mcp_tools.tool_definitions(role_kind, role_id=role_id, stage=stage)
    by_name = {tool["name"]: tool for tool in tools}
    ordered = [by_name[name] for name in _PREFERRED_TOOL_ORDER if name in by_name]
    already = {tool["name"] for tool in ordered}
    return ordered + [tool for name, tool in sorted(by_name.items()) if name not in already]


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
        entry = _tool_entry(tool, example=index < _EXAMPLE_TOOLS)
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
