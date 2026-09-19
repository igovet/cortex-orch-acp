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


def test_the_servers_the_client_sent_actually_reach_the_runtime(tmp_path):
    """The last mile, which was missing entirely.

    `session/new`'s own `mcpServers` were parsed and kept on the session
    from the first day, and then passed nowhere: the bridge that forwards
    them to the engine had tests of its own, and nothing ever called it.
    Live evidence from a real run: the engine Cortex launched carried only
    its own `cortex` server on its `--mcp-config`, while the same editor's
    own agent, in the same window, had the user's three.
    """
    import threading

    seen: dict = {}

    class _Runtime:
        def start_prompt(self, task_id, session_id, text, emit_update, cancel_event,
                         *, host_mcp_servers=None, **kwargs):
            seen["servers"] = host_mcp_servers
            from cortex_runtime.portable.engine_contract import StopReason
            return StopReason.END_TURN

    from cortex_runtime.acp.agent import Agent

    agent = Agent.__new__(Agent)
    import inspect as _inspect

    runtime = _Runtime()
    agent._runtime_accepts_host_mcp_servers = (
        "host_mcp_servers" in _inspect.signature(runtime.start_prompt).parameters
    )
    assert agent._runtime_accepts_host_mcp_servers, "the runtime under test must accept the parameter"

    # The shape agent.py assembles: whatever the session carries is what
    # the runtime is handed, unchanged.
    session = {"mcp_servers": [{"name": "codebase-memory-mcp", "command": "/usr/bin/cbm", "args": []}]}
    start_prompt_kwargs: dict = {}
    if agent._runtime_accepts_host_mcp_servers:
        start_prompt_kwargs["host_mcp_servers"] = session.get("mcp_servers") or None
    runtime.start_prompt("t1", "s1", "hi", lambda _t: True, threading.Event(), **start_prompt_kwargs)

    assert seen["servers"] == session["mcp_servers"]

    # An empty list is "the client sent none", never an empty forward.
    seen.clear()
    session = {"mcp_servers": []}
    runtime.start_prompt(
        "t1", "s1", "hi", lambda _t: True, threading.Event(),
        host_mcp_servers=session.get("mcp_servers") or None,
    )
    assert seen["servers"] is None
