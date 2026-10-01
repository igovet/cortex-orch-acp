"""What a Codex code-mode model sees of an MCP tool's input schema.

A port of the installed Codex CLI 0.159.1 (codex-acp 2.0.1) pipeline, from
the ``rust-v0.159.1`` sources: ``tools/src/json_schema.rs`` normalizes the
MCP ``inputSchema`` into its ``JsonSchema`` struct (every other keyword --
pattern, minLength, maxItems, not, if/then/else -- is dropped),
``json_schema/compaction.rs`` compacts it when the normalized JSON exceeds the
server's ``tool_input_schema_max_bytes`` (5,000 by default; the first pass
strips every description), and ``code-mode-protocol/src/json_schema_types.rs``
renders the result as the TypeScript declaration the model reads.

Only the paths Cortex and its forwarded servers exercise are ported: local
``$ref`` expansion and the render work budget are not.
"""
from __future__ import annotations

import copy
import json

DEFAULT_COMPACT_BYTES = 5_000
DEFAULT_RENDER_BYTES = 16_000
_COMPOSITION = ("anyOf", "oneOf", "allOf")
_CHILD_KEYS = ("items", "anyOf", "oneOf", "allOf")
_DEFINITIONS = ("$defs", "definitions")
# ``JsonSchema`` struct fields, in declaration order.
_KEPT = ("$ref", "type", "description", "encrypted", "enum", "items", "minItems", "properties",
         "required", "additionalProperties", "anyOf", "oneOf", "allOf", "$defs", "definitions")


def _types(node):
    value = node.get("type")
    if isinstance(value, str):
        return [value]
    if isinstance(value, list):
        return [item for item in value if isinstance(item, str)]
    return []


def _sanitize(value):
    if isinstance(value, bool):
        return {"type": "string"}
    if isinstance(value, list):
        return [_sanitize(item) for item in value]
    if not isinstance(value, dict):
        return value
    node = dict(value)
    if isinstance(node.get("properties"), dict):
        node["properties"] = {name: _sanitize(child) for name, child in node["properties"].items()}
    if "items" in node:
        node["items"] = _sanitize(node["items"])
    if "additionalProperties" in node and not isinstance(node["additionalProperties"], bool):
        node["additionalProperties"] = _sanitize(node["additionalProperties"])
    for key in _COMPOSITION:
        if key in node:
            node[key] = _sanitize(node[key])
    for key in _DEFINITIONS:
        if isinstance(node.get(key), dict):
            node[key] = {name: _sanitize(child) for name, child in node[key].items()}
        else:
            node.pop(key, None)
    if "const" in node:
        node["enum"] = [node.pop("const")]
    types = _types(node)
    if not types and ("$ref" in node or any(key in node for key in _COMPOSITION)):
        return node
    if not types:
        if any(key in node for key in ("properties", "required", "additionalProperties")):
            types = ["object"]
        elif "items" in node or "prefixItems" in node:
            types = ["array"]
        elif "enum" in node or "format" in node:
            types = ["string"]
        elif any(key in node for key in ("minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum", "multipleOf")):
            types = ["number"]
        else:
            return {}
    node["type"] = types[0] if len(types) == 1 else types
    if "object" in types and "properties" not in node:
        node["properties"] = {}
    if "array" in types and "items" not in node:
        node["items"] = {"type": "string"}
    return node


def _struct(value):
    """Round-trip through the ``JsonSchema`` struct: unknown keywords vanish."""
    if not isinstance(value, dict):
        return value
    node = {}
    for key in _KEPT:
        if key not in value:
            continue
        child = value[key]
        if key in ("items",) or (key == "additionalProperties" and isinstance(child, dict)):
            child = _struct(child)
        elif key in ("properties", "$defs", "definitions"):
            child = {name: _struct(item) for name, item in sorted(child.items())}
        elif key in _COMPOSITION:
            child = [_struct(item) for item in child]
        node[key] = child
    return node


def _size(value):
    return len(json.dumps(_struct(value), ensure_ascii=False, separators=(",", ":")).encode("utf-8"))


def _map_children(node, visit, include_definitions):
    """Apply ``visit`` to every schema-valued child, as ``traversal.rs`` does."""
    if isinstance(node.get("properties"), dict):
        node["properties"] = {name: visit(child) for name, child in node["properties"].items()}
    for key in _CHILD_KEYS:
        if key in node:
            node[key] = visit(node[key])
    if isinstance(node.get("additionalProperties"), dict):
        node["additionalProperties"] = visit(node["additionalProperties"])
    if include_definitions:
        for key in _DEFINITIONS:
            if isinstance(node.get(key), dict):
                node[key] = {name: visit(child) for name, child in node[key].items()}
    return node


def _strip_descriptions(value):
    if isinstance(value, list):
        return [_strip_descriptions(item) for item in value]
    if isinstance(value, dict):
        value.pop("description", None)
        return _map_children(value, _strip_descriptions, True)
    return value


def _drop_definitions(value):
    if isinstance(value, dict):
        for key in _DEFINITIONS:
            value.pop(key, None)
    return value


def _collapse(value, depth=0):
    if isinstance(value, list):
        return [_collapse(item, depth) for item in value]
    if not isinstance(value, dict):
        return value
    if depth >= 3 and any(key in value for key in (*_CHILD_KEYS, "properties", "additionalProperties", "$ref")):
        return {}
    return _map_children(value, lambda child: _collapse(child, depth + 1), False)


def _prune_compositions(value):
    if isinstance(value, list):
        return [_prune_compositions(item) for item in value]
    if not isinstance(value, dict):
        return value
    if any(key in value for key in _COMPOSITION):
        return {}
    return _map_children(value, _prune_compositions, False)


def normalized(schema, max_bytes=DEFAULT_COMPACT_BYTES):
    """The ``JsonSchema`` Codex keeps for a tool, after compaction."""
    value = _sanitize(copy.deepcopy(schema))
    for compaction in (_strip_descriptions, _drop_definitions, _collapse, _prune_compositions):
        if _size(value) <= max_bytes:
            break
        value = compaction(value)
    return _struct(value)


def normalized_size(schema):
    """Normalized JSON bytes Codex compares against the compaction budget."""
    return _size(_sanitize(copy.deepcopy(schema)))


def _literal(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def _render(node):
    if node is True:
        return "unknown"
    if node is False:
        return "never"
    if not isinstance(node, dict):
        return "unknown"
    if "$ref" in node:
        return "unknown"
    if isinstance(node.get("enum"), list) and node["enum"]:
        return " | ".join(_literal(value) for value in node["enum"])
    for key in ("anyOf", "oneOf"):
        if isinstance(node.get(key), list) and node[key]:
            return " | ".join(_render(item) for item in node[key])
    if isinstance(node.get("allOf"), list) and node["allOf"]:
        parts = [_render(item) for item in node["allOf"]]
        return " & ".join(f"({part})" if " | " in part else part for part in parts)
    kind = node.get("type")
    if isinstance(kind, list) and kind:
        return " | ".join(_render_type(node, item) for item in kind)
    if isinstance(kind, str):
        return _render_type(node, kind)
    if any(key in node for key in ("properties", "additionalProperties", "required")):
        return _render_object(node)
    if "items" in node:
        return _render_array(node)
    return "unknown"


def _render_type(node, kind):
    return {"string": "string", "number": "number", "integer": "number", "boolean": "boolean",
            "null": "null"}.get(kind) or (_render_array(node) if kind == "array"
                                           else _render_object(node) if kind == "object" else "unknown")


def _render_array(node):
    return f"Array<{_render(node['items'])}>" if "items" in node else "unknown[]"


def _extra(node, properties, prefix):
    extra = node.get("additionalProperties")
    if extra is True:
        return [f"{prefix}[key: string]: unknown;"]
    if isinstance(extra, dict):
        return [f"{prefix}[key: string]: {_render(extra)};"]
    if extra is None and not properties:
        return [f"{prefix}[key: string]: unknown;"]
    return []


def _render_object(node):
    required = set(node.get("required") or ())
    properties = node.get("properties") or {}
    ordered = sorted(properties.items())

    def line(name, child):
        return f"{name}{'' if name in required else '?'}: {_render(child)};"

    if any(isinstance(child, dict) and child.get("description") for _, child in ordered):
        lines = ["{"]
        for name, child in ordered:
            for text in str(child.get("description") or "").splitlines():
                if text.strip():
                    lines.append(f"  // {text.strip()}")
            lines.append(f"  {line(name, child)}")
        return "\n".join(lines + _extra(node, properties, "  ") + ["}"])
    lines = [line(name, child) for name, child in ordered] + _extra(node, properties, "")
    return "{ " + " ".join(lines) + " }" if lines else "{}"


def render(schema, max_bytes=None):
    """TypeScript argument type a code-mode model reads for ``schema``.

    ``max_bytes`` is the server's configured ``tool_input_schema_max_bytes``;
    None is Codex's default (5,000 compaction, 16,000 rendering).
    """
    kept = normalized(schema, DEFAULT_COMPACT_BYTES if max_bytes is None else max_bytes)
    rendered = _render(kept)
    budget = max(DEFAULT_RENDER_BYTES, max_bytes or 0)
    return "unknown" if len(rendered.encode("utf-8")) > budget else rendered
