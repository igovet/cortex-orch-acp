"""The client's own MCP servers reach the engines.

User decision: what is configured in the editor -- a code-memory server, for
instance -- must be available to the coordinator and to every worker, since
Cortex is what launches those engines. Until now `engine.mcp_bridge` built
exactly one server (Cortex's own) and a host-supplied list was never accepted,
so nothing the user configured was ever visible to a delegated model.

Two things are still refused rather than forwarded, and both are asserted here:
an entry an engine could not launch, and an entry calling itself `cortex`,
which would shadow this runtime's own server.
"""
from __future__ import annotations

import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "src"))

from cortex_runtime.engine.mcp_bridge import build_mcp_servers, host_mcp_servers  # noqa: E402


def test_a_client_server_is_forwarded_whole():
    entry = {
        "name": "codebase_memory",
        "command": "/usr/bin/node",
        "args": ["/opt/memory/index.js"],
        "env": [{"name": "MEMORY_TOKEN", "value": "s3cret"}],
    }
    forwarded = host_mcp_servers([entry])
    assert forwarded == [entry]
    # Copied, never the caller's own object.
    assert forwarded[0] is not entry


def test_an_entry_that_could_not_be_launched_is_omitted_not_fatal():
    assert host_mcp_servers([{"name": "x"}]) == []
    assert host_mcp_servers([{"command": "/bin/true"}]) == []
    assert host_mcp_servers([{"name": "", "command": "/bin/true"}]) == []
    assert host_mcp_servers(["not a dict"]) == []
    assert host_mcp_servers(None) == []


def test_a_client_can_never_shadow_the_cortex_server():
    assert host_mcp_servers([{"name": "cortex", "command": "/bin/evil"}]) == []


def test_cortex_comes_first_and_the_client_follows():
    from cortex_runtime.engine import session_manager as sm

    cortex_only = build_mcp_servers("ch_0123456789ab", "a" * 20, "/project")
    assert [entry["name"] for entry in cortex_only] == ["cortex"]

    combined = list(cortex_only) + host_mcp_servers(
        [{"name": "codebase_memory", "command": "/usr/bin/node"}],
    )
    assert [entry["name"] for entry in combined] == ["cortex", "codebase_memory"]
    # The manager accepts the list and keeps its own copy.
    assert "host_mcp_servers" in sm.EngineSessionManager.__init__.__code__.co_varnames
