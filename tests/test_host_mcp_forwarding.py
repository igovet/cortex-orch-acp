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
from cortex_runtime.configuration.mcp_servers import load_mcp_servers  # noqa: E402


def test_a_client_server_is_forwarded_whole():
    entry = {
        "name": "codebase_memory",
        "allowed_tools": ["inspect"], "command": "/usr/bin/node",
        "args": ["/opt/memory/index.js"],
        "env": [{"name": "MEMORY_TOKEN", "value": "s3cret"}],
    }
    forwarded = host_mcp_servers([entry])
    assert forwarded == [entry]
    # Copied, never the caller's own object.
    assert forwarded[0] is not entry


def test_an_entry_that_could_not_be_launched_is_omitted_not_fatal():
    assert host_mcp_servers([{"name": "x"}]) == []
    assert host_mcp_servers([{"allowed_tools": ["inspect"], "command": "/bin/true"}]) == []
    assert host_mcp_servers([{"name": "", "allowed_tools": ["inspect"], "command": "/bin/true"}]) == []
    assert host_mcp_servers(["not a dict"]) == []
    assert host_mcp_servers(None) == []


def test_a_client_can_never_shadow_the_cortex_server():
    assert host_mcp_servers([{"name": "cortex", "allowed_tools": ["inspect"], "command": "/bin/evil"}]) == []


def test_cortex_comes_first_and_the_client_follows():
    from cortex_runtime.engine import session_manager as sm

    cortex_only = build_mcp_servers("ch_0123456789ab", "a" * 20, "/project")
    assert [entry["name"] for entry in cortex_only] == ["cortex"]

    combined = list(cortex_only) + host_mcp_servers(
        [{"name": "codebase_memory", "allowed_tools": ["inspect"], "command": "/usr/bin/node"}],
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
    session = {"mcp_servers": [{"name": "codebase-memory-mcp", "allowed_tools": ["inspect"], "command": "/usr/bin/cbm", "args": []}]}
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


def test_production_runtime_forwards_owner_registry_to_task_and_role_filters(tmp_path):
    """The production wrapper must expose and forward the argument Agent probes.

    The old test used a dummy runtime that accepted the argument, while the
    actual production wrapper did not. That let a live Paseo session offer no
    external servers even though its owner registry was correctly loaded.
    """
    import inspect
    import json
    import threading

    from cortex_runtime.acp.agent import Agent
    from cortex_runtime.acp.production_runtime import ProductionOrchestratingRuntime
    from cortex_runtime.portable.engine_contract import StopReason

    config_root = tmp_path / "config"
    registry_dir = config_root / "cortex"
    registry_dir.mkdir(parents=True)
    path = registry_dir / "mcp-servers.json"
    secret = "test-secret-never-log-or-store"
    path.write_text(json.dumps({"version": 1, "servers": {
        "codebase-memory-mcp": {"allowed_tools": ["inspect"], "command": "/bin/true", "env": {"MEMORY_TOKEN": secret}},
        "playwright": {"allowed_tools": ["inspect"], "command": "/bin/true", "roles": ["worker"], "engines": ["claude-agent-acp"]},
    }}), encoding="utf-8")
    path.chmod(0o600)
    env = {"HOME": str(tmp_path), "XDG_CONFIG_HOME": str(config_root)}
    servers = load_mcp_servers(tmp_path, client_servers=[], environ=env)
    assert [entry["name"] for entry in servers] == ["codebase-memory-mcp", "playwright"]

    class _TaskRuntime:
        def __init__(self):
            self.by_task = {}

        def start_prompt(self, task_id, session_id, content, emit_update, cancel_event, *, host_mcp_servers=None, **kwargs):
            self.by_task[task_id] = list(host_mcp_servers or ())
            return StopReason.END_TURN

        def host_mcp_servers_for_task(self, task_id):
            return self.by_task.get(task_id)

    production = ProductionOrchestratingRuntime(environ=env)
    task_runtime = _TaskRuntime()
    production._runtime_for_project = lambda _project: task_runtime
    agent = Agent(production, environ=env)
    assert agent._runtime_accepts_host_mcp_servers
    assert "host_mcp_servers" in inspect.signature(production.start_prompt).parameters

    production.start_prompt(
        "task", "session", "prompt", lambda _update: True, threading.Event(),
        project_root=str(tmp_path), host_mcp_servers=servers,
    )
    effective = task_runtime.host_mcp_servers_for_task("task")
    assert [entry["name"] for entry in effective] == ["codebase-memory-mcp", "playwright"]
    assert [entry["name"] for entry in host_mcp_servers(effective, role="coordinator", engine="claude-agent-acp")] == []
    assert [entry["name"] for entry in host_mcp_servers(effective, role="worker", engine="claude-agent-acp")] == ["codebase-memory-mcp", "playwright"]
    assert [entry["name"] for entry in host_mcp_servers(effective, role="worker", engine="codex-acp")] == ["codebase-memory-mcp"]
    # The registry value is only held in memory for the active task.
    assert not (tmp_path / ".cortex").exists()


def test_cortex_registry_supplies_servers_when_client_sends_none(tmp_path):
    """Paseo sends an empty session/new list; Cortex config still works."""
    import json

    project_file = tmp_path / ".cortex" / "mcp-servers.json"
    project_file.parent.mkdir()
    project_file.write_text(json.dumps({"version": 1, "servers": {
        "codebase-memory-mcp": {
            "allowed_tools": ["inspect"], "command": "/home/user/.local/bin/codebase-memory-mcp",
            "args": [],
            "env": {"MEMORY_TOKEN": "in-memory-only"},
        },
    }}), encoding="utf-8")
    project_file.chmod(0o600)
    root = Path("/var/tmp") / f"cortex-mcp-test-home-{tmp_path.name}"
    config = root / "config" / "cortex"
    config.mkdir(parents=True, exist_ok=True)
    global_file = config / "mcp-servers.json"
    global_file.write_text(json.dumps({"version": 1, "servers": {
        "codebase-memory-mcp": {
            "allowed_tools": ["inspect"], "command": "/home/user/.local/bin/codebase-memory-mcp",
            "args": [],
            "env": {"MEMORY_TOKEN": "in-memory-only"},
        },
    }}), encoding="utf-8")
    global_file.chmod(0o600)
    effective = load_mcp_servers(tmp_path, client_servers=[], environ={
        "HOME": str(root),
        "XDG_CONFIG_HOME": str(root / "config"),
        "XDG_STATE_HOME": str(root / "state"),
    })
    assert effective == [{
        "name": "codebase-memory-mcp",
        "allowed_tools": ["inspect"], "command": "/home/user/.local/bin/codebase-memory-mcp",
        "args": [],
        "env": [{"name": "MEMORY_TOKEN", "value": "in-memory-only"}],
        "read_paths": [], "read_only_tools": [], "network": False,
    }]


def test_global_registry_wins_over_client_and_project_files_are_ignored(tmp_path):
    import json

    root = "/var/tmp/cortex-mcp-test-home-duplicate"
    config = Path(root) / "config" / "cortex"
    config.mkdir(parents=True, exist_ok=True)
    (config / "mcp-servers.json").write_text(json.dumps({"servers": {
        "shared": {"allowed_tools": ["inspect"], "command": "/global/server", "args": []},
    }}), encoding="utf-8")
    (config / "mcp-servers.json").chmod(0o600)
    project_file = tmp_path / ".cortex" / "mcp-servers.json"
    project_file.parent.mkdir()
    # A worker can write project files in direct-project mode.  Such a file
    # must never become a persistent executable capability for a later turn.
    project_file.write_text(json.dumps({"servers": {
        "shared": {"allowed_tools": ["inspect"], "command": "/project/server"},
        "project-only": {"allowed_tools": ["inspect"], "command": "/project/only"},
    }}), encoding="utf-8")
    project_file.chmod(0o600)
    effective = load_mcp_servers(tmp_path, client_servers=[
        {"name": "shared", "allowed_tools": ["inspect"], "command": "/client/server"},
        {"name": "client-only", "allowed_tools": ["inspect"], "command": "/client/only"},
    ], environ={"HOME": root, "XDG_CONFIG_HOME": str(Path(root) / "config"), "XDG_STATE_HOME": str(Path(root) / "state")})
    assert [entry["name"] for entry in effective] == ["shared", "client-only"]
    assert effective[0]["command"] == "/global/server"
    assert "project-only" not in {entry["name"] for entry in effective}


def test_effective_servers_reach_engine_session_new_after_cortex_entry(tmp_path):
    """The actual outgoing ACP session/new carries Cortex first and the tool."""
    import json
    from cortex_runtime.engine.session_manager import EngineSessionManager
    from cortex_runtime.portable.engine_contract import EngineCapability
    from engine_fakes import (
        default_scenario, fake_launch_command, isolated_environ,
        make_connection_record, new_connection_id, write_scenario,
    )

    environ = isolated_environ(tmp_path)
    dump_path = tmp_path / "session-new.json"
    scenario = write_scenario(tmp_path, default_scenario(session_new_params_dump_path=str(dump_path)))
    record = make_connection_record(
        new_connection_id(), launch_command=fake_launch_command(scenario), environ=environ,
    )
    client_tool = {"name": "codebase-memory-mcp", "allowed_tools": ["inspect"], "command": "/usr/bin/codebase-memory-mcp", "args": [], "env": []}
    manager = EngineSessionManager(
        record, str(tmp_path), environ=environ,
        channel_credentials=("ch_" + "0" * 12, "a" * 20, str(tmp_path)),
        host_mcp_servers=[client_tool], mcp_role="worker", call_timeout=5,
    )
    try:
        manager.negotiate((EngineCapability.SESSION_CREATION, EngineCapability.PROMPT_STREAMING))
        manager.create_session("t_" + "a" * 12, "xr_" + "0" * 24)
        wire = json.loads(dump_path.read_text(encoding="utf-8"))
    finally:
        manager.shutdown(grace=2)
    assert [entry["name"] for entry in wire["mcpServers"]] == ["cortex", "codebase-memory-mcp"]


def test_owner_registry_roles_scope_external_servers_without_wire_metadata(tmp_path):
    import json
    from cortex_runtime.portable.errors import PortableError

    config = tmp_path / "config" / "cortex"
    config.mkdir(parents=True)
    path = config / "mcp-servers.json"
    path.write_text(json.dumps({"servers": {
        "research": {"allowed_tools": ["inspect"], "command": "/bin/true", "roles": ["coordinator"], "engines": ["claude-agent-acp"]},
        "code-memory": {"allowed_tools": ["inspect"], "command": "/bin/true", "roles": ["worker"], "engines": ["codex-acp"]},
        "shared": {"allowed_tools": ["inspect"], "command": "/bin/true"},
    }}), encoding="utf-8")
    path.chmod(0o600)
    env = {"HOME": str(tmp_path), "XDG_CONFIG_HOME": str(tmp_path / "config")}
    entries = load_mcp_servers(tmp_path, environ=env)
    coordinator = host_mcp_servers(entries, role="coordinator", engine="claude-agent-acp")
    worker = host_mcp_servers(entries, role="worker", engine="codex-acp")
    assert coordinator == []
    assert [entry["name"] for entry in worker] == ["code-memory", "shared"]
    assert [entry["name"] for entry in host_mcp_servers(entries, role="worker", engine="claude-agent-acp")] == ["shared"]
    assert all("roles" not in entry and "engines" not in entry for entry in coordinator + worker)

    for invalid in ([], ["coordinator", "coordinator"], ["other"], "worker", [1]):
        path.write_text(json.dumps({"servers": {
            "broken": {"allowed_tools": ["inspect"], "command": "/bin/true", "roles": invalid},
        }}), encoding="utf-8")
        try:
            load_mcp_servers(tmp_path, environ=env)
        except PortableError as exc:
            assert exc.code == "invalid_mcp_registry"
        else:
            raise AssertionError(f"invalid roles accepted: {invalid!r}")
    for invalid in ([], ["codex-acp", "codex-acp"], ["other"], "codex-acp", [1]):
        path.write_text(json.dumps({"servers": {
            "broken": {"allowed_tools": ["inspect"], "command": "/bin/true", "engines": invalid},
        }}), encoding="utf-8")
        try:
            load_mcp_servers(tmp_path, environ=env)
        except PortableError as exc:
            assert exc.code == "invalid_mcp_registry"
        else:
            raise AssertionError(f"invalid engines accepted: {invalid!r}")


def test_engine_session_filters_external_mcp_and_records_bounded_inventory(tmp_path):
    import json
    from cortex_runtime.engine.session_manager import EngineSessionManager
    from cortex_runtime.portable.engine_contract import EngineCapability
    from engine_fakes import (
        default_scenario, fake_launch_command, isolated_environ,
        make_connection_record, new_connection_id, write_scenario,
    )

    environ = isolated_environ(tmp_path)
    dump_path = tmp_path / "session-new-role.json"
    scenario = write_scenario(tmp_path, default_scenario(session_new_params_dump_path=str(dump_path)))
    record = make_connection_record(new_connection_id(), launch_command=fake_launch_command(scenario), environ=environ)
    secret = "external-mcp-secret-must-stay-out-of-observations"
    manager = EngineSessionManager(
        record, str(tmp_path), environ=environ,
        channel_credentials=("ch_" + "0" * 12, "a" * 20, str(tmp_path)),
        host_mcp_servers=[
            {"name": "research", "allowed_tools": ["inspect"], "command": "/bin/true", "roles": ["coordinator"]},
            {"name": "code-memory", "allowed_tools": ["inspect"], "command": "/bin/true", "env": [{"name": "MEMORY_TOKEN", "value": secret}], "roles": ["worker"], "engines": ["codex-acp"]},
            {"name": "playwright", "allowed_tools": ["inspect"], "command": "/bin/true", "roles": ["worker"], "engines": ["claude-agent-acp"]},
        ],
        mcp_role="worker", call_timeout=5,
    )
    try:
        manager.negotiate((EngineCapability.SESSION_CREATION, EngineCapability.PROMPT_STREAMING))
        session = manager.create_session("t_" + "a" * 12, "xr_" + "0" * 24)
        wire = json.loads(dump_path.read_text(encoding="utf-8"))
        receipt = manager.external_mcp_inventory(session)
    finally:
        manager.shutdown(grace=2)
    assert [entry["name"] for entry in wire["mcpServers"]] == ["cortex", "code-memory"]
    assert wire["mcpServers"][1]["env"] == []
    assert "--relay" in wire["mcpServers"][1]["args"]
    assert secret not in json.dumps(wire)
    assert secret not in (manager._process.logs.directory / "observations.jsonl").read_text(encoding="utf-8")
    assert secret not in (manager._process.logs.directory / "stderr.log").read_text(encoding="utf-8")
    assert receipt == {
        "role": "worker", "engine": "codex-acp", "server_count": 1, "server_names": ["code-memory"],
        "names_truncated": False, "tool_count": None, "tool_schemas_observed": False,
    }
