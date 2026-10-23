"""The senior consultant works only from its consultation
packet and retained reports.

Its Cortex tool offer already excludes project reads and commands. These
tests cover the per-role session attachment around that offer: the
consultant's engine session is attached no external MCP server (the code
index included), its base prompt does not advertise one, and the permission
fence refuses every native action that asks. Every other role keeps its
attachments unchanged.
"""
from __future__ import annotations

import json

import pytest

from cortex_runtime.engine.mcp_bridge import host_mcp_servers, is_consultant_role, receives_external_mcp
from cortex_runtime.engine.unified_system_prompt import CODE_INDEX_SERVER, cortex_system_prompt

CONSULTANT = "worker-senior-consultant"
CODE_INDEX = {
    "name": CODE_INDEX_SERVER, "command": "/usr/bin/codebase-memory-mcp", "args": [], "env": [],
    "allowed_tools": ["list_projects", "search_graph", "trace_path", "get_code_snippet", "search_code"],
}
OTHER = {"name": "playwright", "command": "/usr/bin/playwright-mcp", "args": [], "env": [],
         "allowed_tools": ["browser_navigate"]}


@pytest.mark.parametrize("role_id", [CONSULTANT, "senior-consultant"])
def test_the_consultant_is_attached_no_external_server(role_id):
    assert is_consultant_role(role_id)
    assert not receives_external_mcp(role_id)
    for engine in ("claude-agent-acp", "codex-acp", "opencode"):
        assert host_mcp_servers([CODE_INDEX, OTHER], role="worker", engine=engine, role_id=role_id) == []


@pytest.mark.parametrize("role_id", [None, "", "worker-backend-dev", "backend-dev", "worker-code-reviewer"])
def test_every_other_worker_keeps_its_external_servers(role_id):
    assert not is_consultant_role(role_id)
    assert receives_external_mcp(role_id)
    forwarded = host_mcp_servers([CODE_INDEX, OTHER], role="worker", engine="claude-agent-acp", role_id=role_id)
    assert [entry["name"] for entry in forwarded] == [CODE_INDEX_SERVER, "playwright"]


@pytest.mark.parametrize("edition", [1, 2])
@pytest.mark.parametrize("code_mode", [False, True])
@pytest.mark.parametrize("native_tools", [False, True])
@pytest.mark.parametrize("access_class", ["write", "read_only"])
def test_consultant_base_prompt_names_packet_and_reports_not_the_code_index(
        tmp_path, edition, code_mode, native_tools, access_class):
    environ = {"HOME": str(tmp_path)}
    options = dict(environ=environ, edition=edition, code_mode=code_mode, native_tools=native_tools)
    consultant = cortex_system_prompt("worker", str(tmp_path), **options, consultant=True)
    worker = cortex_system_prompt("worker", str(tmp_path), **options, access_class=access_class)
    index_guidance = (
        "Use an attached code index first for definitions, callers, dependencies, impact and architecture, "
        "then read its exact source snippets."
    )
    assert "# Working material" in consultant and "consultation packet" in consultant
    assert "# Finding code" not in consultant
    assert CODE_INDEX_SERVER not in consultant
    assert index_guidance not in consultant
    assert 'Use only your consultation packet and retained `read_report` reports.' in consultant
    assert "No code index, project source reads or commands are offered." in consultant
    assert "access_request" not in consultant
    assert "ask through the runtime's question tool" not in consultant
    assert "@@" not in consultant
    # The ordinary worker prompt is unchanged by the variant.
    assert "# Finding code" in worker and index_guidance in worker
    assert "Follow `code-and-evidence` when included in your role." in worker
    assert "Prefer fast text search for literals, configuration, docs or code outside index coverage." in worker
    assert "# Working material" not in worker and "@@" not in worker
    with pytest.raises(ValueError):
        cortex_system_prompt("coordinator", str(tmp_path), environ=environ, consultant=True)


@pytest.mark.parametrize("engine, version, key", [
    ("claude-agent-acp", "0.78.0", "systemPrompt"),
    ("codex-acp", "1.12.0", "cortexBaseInstructions"),
])
def test_session_new_meta_selects_the_consultant_prompt_by_role_id(tmp_path, engine, version, key):
    from cortex_runtime.delegation.live_adapters import _session_new_meta_for

    environ = {"HOME": str(tmp_path)}
    consultant = _session_new_meta_for(engine, version, "role text", role="worker", project_root=str(tmp_path),
                                       environ=environ, role_id=CONSULTANT)
    worker = _session_new_meta_for(engine, version, "role text", role="worker", project_root=str(tmp_path),
                                   environ=environ, role_id="worker-backend-dev")
    assert "# Working material" in consultant[key] and CODE_INDEX_SERVER not in consultant[key]
    assert "# Finding code" in worker[key]
    if engine == "claude-agent-acp":
        # Claude's native shell, file and search tools are removed for every
        # Cortex session, the consultant's included.
        disallowed = consultant["claudeCode"]["options"]["disallowedTools"]
        assert {"Bash", "Read", "Grep", "Glob", "Edit", "Write"} <= set(disallowed)


def test_opencode_agent_prompt_selects_the_consultant_variant(tmp_path):
    from cortex_runtime.engine.opencode_adapter_acp import build_agent_prompt

    environ = {"HOME": str(tmp_path)}
    prompt = build_agent_prompt("role text", role="worker", project_root=str(tmp_path), environ=environ,
                                consultant=True)
    assert "# Working material" in prompt and "# Finding code" not in prompt


def _bridge(tmp_path, *, refused):
    from cortex_runtime.engine.correlation import CorrelationTables
    from cortex_runtime.engine.permissions import PermissionBridge
    from cortex_runtime.engine.resource_binding import BindingRegistry, ResourceBinding

    bindings = BindingRegistry()
    bindings.bind(ResourceBinding(
        engine_instance_id="eirt_a", engine_session_id="es_a", context_generation_id="gen_1",
        assignment_id="asg_1", attempt_id="att_1", workspace_binding_id="wb_1", workspace_root=str(tmp_path),
        ownership_epoch="epoch_1", role="worker"))
    host = []
    bridge = PermissionBridge(correlation=CorrelationTables(), bindings=bindings,
                              host_permission_requester=lambda *a: host.append(a) or {
                                  "outcome": {"outcome": "selected", "optionId": "allow"}},
                              native_tools_refused=refused)
    return bridge, host


OPTIONS = [dict(optionId="allow", kind="allow_once"), dict(optionId="deny", kind="reject_once")]


@pytest.mark.parametrize("tool_call", [
    dict(name="Bash", kind="execute", rawInput=dict(command="cat src/app.py")),
    dict(kind="execute", rawInput=dict(command="rg secret")),
    dict(name="Read", kind="read"),
    dict(name="mcp__codebase-memory-mcp__search_graph"),
    dict(name="exec_command", kind="delete", rawInput=dict(command="rm -rf build")),
    dict(_meta=dict(claudeCode=dict(toolName="Grep"))),
])
def test_consultant_fence_refuses_every_native_action_that_asks(tmp_path, tool_call):
    bridge, host = _bridge(tmp_path, refused=True)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1,
                           params=dict(toolCall=dict(tool_call, toolCallId="tc-1"), options=OPTIONS))
    assert result == {"outcome": {"outcome": "selected", "optionId": "deny"}}
    assert host == []  # refused locally; no deletion card reaches the user


@pytest.mark.parametrize("tool_call", [
    dict(name="mcp__cortex__read_report"),
    dict(_meta=dict(claudeCode=dict(toolName="mcp__cortex__result_submit"))),
])
def test_consultant_fence_still_allows_its_cortex_tools(tmp_path, tool_call):
    bridge, _host = _bridge(tmp_path, refused=True)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1,
                           params=dict(toolCall=dict(tool_call, toolCallId="tc-1"), options=OPTIONS))
    assert result == {"outcome": {"outcome": "selected", "optionId": "allow"}}


def test_other_workers_keep_the_ordinary_fence(tmp_path):
    bridge, _host = _bridge(tmp_path, refused=False)
    result = bridge.handle(engine_session_id="es_a", wire_session_id="w1", wire_request_id=1,
                           params=dict(toolCall=dict(name="Bash", kind="execute", toolCallId="tc-1",
                                                     rawInput=dict(command="cat src/app.py")), options=OPTIONS))
    assert result == {"outcome": {"outcome": "selected", "optionId": "allow"}}


def _manager(tmp_path, role_id):
    from cortex_runtime.engine.session_manager import EngineSessionManager
    from engine_fakes import (
        default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id,
        write_scenario,
    )

    environ = isolated_environ(tmp_path)
    dump_path = tmp_path / "session-new.json"
    scenario = write_scenario(tmp_path, default_scenario(session_new_params_dump_path=str(dump_path)))
    record = make_connection_record(new_connection_id(), launch_command=fake_launch_command(scenario),
                                    environ=environ)
    manager = EngineSessionManager(
        record, str(tmp_path), environ=environ,
        channel_credentials=("ch_" + "0" * 12, "a" * 20, str(tmp_path)),
        host_mcp_servers=[CODE_INDEX, OTHER], mcp_role="worker", mcp_role_id=role_id, call_timeout=5,
    )
    return manager, dump_path


@pytest.mark.parametrize("role_id, expected", [
    (CONSULTANT, ["cortex"]),
    ("worker-backend-dev", ["cortex", CODE_INDEX_SERVER, "playwright"]),
])
def test_engine_session_new_attaches_servers_by_worker_profile(tmp_path, role_id, expected):
    from cortex_runtime.portable.engine_contract import EngineCapability

    manager, dump_path = _manager(tmp_path, role_id)
    try:
        manager.negotiate((EngineCapability.SESSION_CREATION, EngineCapability.PROMPT_STREAMING))
        session_id = manager.create_session("t_" + "a" * 12, "xr_" + "0" * 24,
                                            session_new_meta={"systemPrompt": "role text"})
        wire = json.loads(dump_path.read_text(encoding="utf-8"))
        inventory = manager.external_mcp_inventory(session_id)
        refused = manager._permission_bridge._native_tools_refused
    finally:
        manager.shutdown(grace=2)
    assert [entry["name"] for entry in wire["mcpServers"]] == expected
    prompt = json.dumps(wire.get("_meta"))
    consultant = role_id == CONSULTANT
    assert ("Attached MCP servers" in prompt) is not consultant
    assert (CODE_INDEX_SERVER in prompt) is not consultant
    assert inventory["server_count"] == len(expected) - 1
    assert refused is consultant


@pytest.mark.parametrize("role_id", [CONSULTANT, "worker-backend-dev"])
def test_adapter_open_hands_the_worker_profile_to_the_session_manager(tmp_path, monkeypatch, role_id):
    from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
    from cortex_runtime.engine import session_manager as session_manager_module
    from engine_fakes import (
        default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id,
        write_scenario,
    )

    environ = isolated_environ(tmp_path)
    record = make_connection_record(new_connection_id(),
                                    launch_command=fake_launch_command(write_scenario(tmp_path, default_scenario())),
                                    environ=environ)
    seen = {}

    class _Stop(Exception):
        pass

    class _Recorder:
        def __init__(self, *args, **kwargs):
            seen.update(kwargs)
            raise _Stop()

    monkeypatch.setattr(session_manager_module, "EngineSessionManager", _Recorder)
    adapter = EngineSessionManagerAdapter(lambda t: record, lambda t: str(tmp_path), environ=environ,
                                          call_timeout=5.0)
    with pytest.raises(_Stop):
        adapter.open("shared_project_root", dict(task_id="t_" + "a" * 12, attempt_id="xa_d3",
                                                 assignment_id="asg_d3", role_id=role_id), "instructions")
    assert seen["mcp_role"] == "worker"
    assert seen["mcp_role_id"] == role_id
