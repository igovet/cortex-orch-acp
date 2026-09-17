"""Builds the ``mcpServers`` array Cortex sends on ``session/new``.

Exactly one stdio entry: the Cortex MCP server itself, launched with only
the explicit ``(channel_id, secret, project_root)`` credentials passed to
``create_session`` -- never a host client's own ``mcpServers`` (there is no
parameter for one; a host-supplied MCP server list is never accepted by
this API). The secret is placed only into this one dict's ``env`` list,
never logged, never included in any exception message or observation (see
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

_CHANNEL_ID_PATTERN = re.compile(r"^ch_[0-9a-f]{12}$")
_CHANNEL_SECRET_PATTERN = re.compile(r"^[A-Za-z0-9_-]{16,256}$")

_ROLES = ("coordinator", "worker")


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


def build_mcp_servers(
    channel_id: str,
    secret: str,
    project_root: str,
    *,
    python_executable: str | None = None,
    script_path: str | Path | None = None,
    role: str | None = None,
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
    return [
        dict(
            name="cortex",
            command=python,
            args=["-B", script],
            env=[
                dict(name="CORTEX_CHANNEL_ID", value=channel_id),
                dict(name="CORTEX_CHANNEL_SECRET", value=secret),
                dict(name="CORTEX_CHANNEL_PROJECT_ROOT", value=project_root),
            ],
        )
    ]
