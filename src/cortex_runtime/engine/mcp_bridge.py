"""Builds the ``mcpServers`` array Cortex sends on ``session/new``.

The Cortex MCP server itself is always first and is launched with only the
explicit ``(channel_id, secret, project_root)`` credentials passed to
``create_session``. Additional stdio entries may come from the Cortex-owned
MCP registry or the incoming client's ``mcpServers`` list; they are forwarded
after Cortex's own entry so they cannot shadow its authenticated channel.
Secrets are kept in memory only, never written to project state or logs (see
``tests/test_engine_isolation.py`` for log-scanning coverage).
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

# .../cortex_runtime/engine/mcp_bridge.py -> parents[2] is the plugin's
# scripts/ directory (mirrors acp/agent.py's PLUGIN = parents[3], one level
# up from this deeper package).
SCRIPTS_DIR = Path(__file__).resolve().parents[2]
CORTEX_MCP_SCRIPT = SCRIPTS_DIR / "cortex.py"
MCP_RELAY_SCRIPT = Path(__file__).with_name("broker.py")

_CHANNEL_ID_PATTERN = re.compile(r"^ch_[0-9a-f]{12}$")
_CHANNEL_SECRET_PATTERN = re.compile(r"^[A-Za-z0-9_-]{16,256}$")

_ROLES = ("coordinator", "worker")
_ENGINES = ("codex-acp", "claude-agent-acp", "opencode")


def allowed_mcp_servers(role: str) -> tuple[str, ...]:
    """The only MCP server name(s) ever permitted for either Cortex role
    (T014 design decision 6): exactly ``("cortex",)`` -- the one server
    ``build_mcp_servers`` itself constructs. No host-supplied or
    engine-native additional MCP server is ever allowlisted for either
    ``"coordinator"`` or ``"worker"``; both roles get the identical,
    narrow allowlist."""
    if role not in _ROLES:
        raise ValueError(f"role must be one of {_ROLES!r}.")
    return ("cortex",)


#: Claude Code's per-tool MCP ``_meta`` key that keeps a tool's schema in the
#: prompt instead of deferring it behind ``ToolSearch`` (read by the bundled
#: CLI of the installed claude-agent-acp 0.81.x; its ACP layer drops a
#: server-level ``alwaysLoad``). Codex ignores it.
ALWAYS_LOAD_META_KEY = "anthropic/alwaysLoad"


def always_load(tool: dict) -> dict:
    """A copy of one advertised MCP tool marked to load without ToolSearch."""
    meta = tool.get("_meta")
    return {**tool, "_meta": {**(meta if isinstance(meta, dict) else {}), ALWAYS_LOAD_META_KEY: True}}


#: Argument descriptions for forwarded servers whose own ``inputSchema``
#: leaves an argument undescribed, keyed server name -> tool -> argument.
#: codebase-memory-mcp 0.10.8 (and 0.11.0) describes none of search_graph's
#: three patterns although one is a glob and two are regular expressions; a
#: model then sent a regex file_pattern (``src/.*``) and got zero results
#: with no error (live, 2026-09-30). Semantics are from its store.c
#: (``cbm_glob_to_like`` -> SQL LIKE; ``iregexp``) and were checked against
#: a live index. A description the server supplies itself always wins.
EXTERNAL_ARGUMENT_DESCRIPTIONS = {
    "codebase-memory-mcp": {
        "search_graph": {
            "file_pattern": (
                "Glob on the project-relative file path, not a regular expression: * and ** match any "
                "characters including '/', ? matches one character (e.g. src/** or **/auto_route.py). "
                "A value without * or ? matches anywhere in the path (e.g. auto_route). Regex syntax such "
                "as .*, (a|b), ^, $ or \\. is taken literally and usually matches nothing, with no error."
            ),
            "name_pattern": (
                "Regular expression searched anywhere in the symbol name, case-insensitive; anchor it "
                "(e.g. ^route_menu$) for an exact name. Ignored when query is set."
            ),
            "qn_pattern": (
                "Regular expression searched anywhere in the qualified name "
                "(<project>.<path segments>.<module>.<symbol>), case-insensitive, e.g. routing\\.auto_route\\."
            ),
        },
    },
}


def describe_arguments(server: str, tool: dict) -> dict:
    """A copy of one forwarded tool with Cortex's description for each
    argument the server left undescribed (see
    :data:`EXTERNAL_ARGUMENT_DESCRIPTIONS`); unchanged otherwise."""
    missing = EXTERNAL_ARGUMENT_DESCRIPTIONS.get(server, {}).get(tool.get("name"))
    schema = tool.get("inputSchema")
    properties = schema.get("properties") if isinstance(schema, dict) else None
    if not missing or not isinstance(properties, dict):
        return tool
    filled = {
        name: dict(value, description=missing[name])
        if name in missing and isinstance(value, dict) and not value.get("description") else value
        for name, value in properties.items()
    }
    return {**tool, "inputSchema": {**schema, "properties": filled}}


def coordinator_tools(entry) -> list[str]:
    """The tools a coordinator may call on a registry server:
    ``coordinator_tools`` when set, else ``read_only_tools``."""
    tools = entry.get("coordinator_tools")
    return list(entry.get("read_only_tools") or ()) if tools is None else list(tools)


def host_mcp_servers(servers, *, role: str | None = None, engine: str | None = None) -> list[dict[str, object]]:
    """The client's own MCP servers, as they must look on the wire to an
    engine.

    The user asked for what they configure in their editor -- a code-memory
    server, for instance -- to be available to the coordinator and to every
    worker, which means passing the client's list through instead of
    dropping it. Two things are still refused rather than forwarded: an
    entry that does not have a usable name and command (an engine cannot
    launch it, and a half-entry is worth an omission, not a crash), and any
    entry calling itself ``cortex``, which would shadow this runtime's own
    server and hand a client's process the coordinator's channel name.

    Entries are copied, not referenced, and nothing here is ever written to
    storage or a log: a client's ``env``/``headers`` values are its
    secrets, and they exist in this process only long enough to be handed
    to the engine that needs them.
    """
    forwarded: list[dict[str, object]] = []
    if role is not None and role not in _ROLES:
        raise ValueError(f"role must be one of {_ROLES!r}.")
    for entry in servers or ():
        if not isinstance(entry, dict):
            continue
        # The coordinator orients, it does not investigate: only a registry
        # server with an explicit read-only lookup set reaches it, and the
        # proxy narrows it to exactly that set.
        if role == "coordinator" and not coordinator_tools(entry):
            continue
        name = entry.get("name")
        if not isinstance(name, str) or not name or name in _ROLES or name == "cortex":
            continue
        if not isinstance(entry.get("command"), str) or not entry["command"]:
            continue
        roles = entry.get("roles")
        if roles is not None and (
            not isinstance(roles, list) or not roles
            or any(item not in _ROLES for item in roles)
        ):
            continue
        if roles is not None and role is not None and role not in roles:
            continue
        engines = entry.get("engines")
        if engines is not None and (
            not isinstance(engines, list) or not engines
            or any(item not in _ENGINES for item in engines)
        ):
            continue
        if engines is not None and engine not in engines:
            continue
        # Registry admission rules are internal, never ACP wire data.
        forwarded.append({key: value for key, value in entry.items() if key not in ("roles", "engines")})
    return forwarded


def build_mcp_servers(
    channel_id: str,
    secret: str,
    project_root: str,
    *,
    python_executable: str | None = None,
    script_path: str | Path | None = None,
    role: str | None = None,
    relay_endpoint: str | None = None,
    relay_token: str | None = None,
) -> list[dict[str, object]]:
    """``role`` (optional, ``"coordinator"``/``"worker"``) -- when given,
    every server name this builds is asserted to be in
    ``allowed_mcp_servers(role)`` (T014 design decision 6) before
    returning; a mismatch is a programming error (``AssertionError``), not
    ever silently forwarded."""
    if not isinstance(channel_id, str) or not _CHANNEL_ID_PATTERN.match(channel_id):
        raise ValueError("channel_id must be a 'ch_' + 12 hex char id.")
    if not isinstance(secret, str) or not _CHANNEL_SECRET_PATTERN.match(secret):
        raise ValueError("secret must be a URL-safe token string.")
    if not isinstance(project_root, str) or not project_root.startswith("/") or "\x00" in project_root:
        raise ValueError("project_root must be an absolute path string.")
    python = python_executable or sys.executable
    script = str(script_path or CORTEX_MCP_SCRIPT)
    if role is not None:
        assert "cortex" in allowed_mcp_servers(role)
    if (relay_endpoint is None) != (relay_token is None):
        raise ValueError("relay_endpoint and relay_token must be supplied together.")
    args = ["-B", script]
    if relay_endpoint is not None:
        if not relay_endpoint.startswith("/") or not relay_token:
            raise ValueError("relay endpoint and token must be valid.")
        args = ["-B", str(MCP_RELAY_SCRIPT), "--relay", relay_endpoint, relay_token]
    environment = [] if relay_endpoint is not None else [
        dict(name="CORTEX_CHANNEL_ID", value=channel_id),
        dict(name="CORTEX_CHANNEL_SECRET", value=secret),
        dict(name="CORTEX_CHANNEL_PROJECT_ROOT", value=project_root),
    ]
    return [
        dict(
            name="cortex",
            command=python,
            args=args,
            env=environment,
        )
    ]
