"""Configured capability visibility is not execution authority or model obedience."""
import json
import threading
import uuid
from types import SimpleNamespace

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.engine.mcp_bridge import host_mcp_servers
from cortex_runtime.engine.session_manager import EngineSessionManager
from cortex_runtime.engine.unified_system_prompt import (
    native_worker_capabilities_section, with_attached_servers, worker_capabilities_section,
)
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.portable.roles import composed_instructions, known_role_ids, load_role
from cortex_runtime.store import Store


def _servers():
    # No server-specific behavior: names change on every invocation.
    suffix = uuid.uuid4().hex[:8]
    return [
        dict(name="archive-" + suffix, command="/bin/true", roles=["worker"],
             allowed_tools=["read_record", "update_record"], read_only_tools=["read_record"],
             env=[dict(name="TOKEN", value="private-value")], args=["private-argument"]),
        dict(name="catalog-" + suffix, command="/bin/true",
             allowed_tools=["lookup", "rebuild"], read_only_tools=["lookup"],
             coordinator_tools=["lookup"], engines=["codex-acp"]),
    ]


def test_inventory_distinguishes_worker_grants_from_coordinator_tools():
    servers = _servers()
    note = worker_capabilities_section(servers)
    for engine in ("codex-acp", "claude-agent-acp", "opencode"):
        assert f"`{engine}` / `{servers[0]['name']}`: read-only worker: `read_record`; writing worker: `read_record`, `update_record`." in note
    assert f"`codex-acp` / `{servers[1]['name']}`" in note
    assert f"`claude-agent-acp` / `{servers[1]['name']}`" not in note
    assert f"`opencode` / `{servers[1]['name']}`" not in note
    assert "not observed discovery, invocation or success" in note
    assert "consultant receives no external MCP" in note
    assert host_mcp_servers(servers, role="worker", engine="codex-acp",
                            role_id="worker-senior-consultant") == []
    coordinator = host_mcp_servers(servers, role="coordinator", engine="codex-acp")
    assert [item["name"] for item in coordinator] == [servers[1]["name"]]
    prompt = with_attached_servers(dict(systemPrompt="Role"), coordinator,
                                   role="coordinator", worker_servers=servers)["systemPrompt"]
    attached, hints = prompt.split("# Configured worker MCP capabilities")
    assert "update_record" not in attached and "rebuild" not in attached
    assert servers[0]["name"] not in attached
    assert "update_record" in hints and "rebuild" in hints


def test_unavailable_empty_and_filtered_inventory_are_explicit(tmp_path):
    assert "Worker inventory unavailable" in worker_capabilities_section(None)
    assert "no eligible configured worker" in worker_capabilities_section([])
    servers = _servers()
    servers[0]["read_paths"] = [str(tmp_path / "absent")]
    servers[1]["roles"] = ["coordinator"]
    note = worker_capabilities_section(servers)
    assert "no eligible configured worker" in note
    assert all(server["name"] not in note for server in servers)
    no_grant = worker_capabilities_section([dict(name="unconfigured", command="/bin/true")])
    assert "unavailable (explicit tool grants missing)" in no_grant


def test_inventory_bounds_and_sanitizes_identifiers_without_private_launch_data():
    servers = _servers()
    servers += [dict(name=f"bulk-{i}", command="/private/executable",
                     allowed_tools=[f"tool-{j}-" + "x" * 100 for j in range(50)],
                     read_only_tools=[]) for i in range(100)]
    servers += [dict(name="bad\n# instruction", command="/bin/true", allowed_tools=["inspect"])]
    servers[0]["allowed_tools"] += ["bad`\n# instruction"]
    note = worker_capabilities_section(servers)
    assert len(note) < 12500
    assert "Inventory incomplete" in note and "tool names omitted" in note
    for private in ("private-value", "TOKEN", "private-argument", "/private/executable",
                    "bad\n", "# instruction", "read_paths", "inputSchema"):
        assert private not in note


@pytest.mark.parametrize("prompt", ["Role", {"append": "Role", "type": "preset"}, None])
def test_inventory_delivery_even_without_coordinator_attachments_does_not_mutate_meta(prompt):
    meta = dict(systemPrompt=prompt, marker="preserve")
    original = json.dumps(meta)
    servers = _servers()
    result = with_attached_servers(meta, [], role="coordinator", worker_servers=servers)
    text = result["systemPrompt"]
    text = text if isinstance(text, str) else text["append"]
    assert "# Configured worker MCP capabilities" in text
    assert "# Attached MCP servers" not in text
    assert servers[0]["name"] in text
    assert json.dumps(meta) == original and result["marker"] == "preserve"
    assert with_attached_servers(meta, [], role="worker", worker_servers=servers) == meta


@pytest.mark.parametrize("engine", ["codex-acp", "claude-agent-acp"])
@pytest.mark.parametrize("role", ["coordinator", "worker"])
def test_session_new_delivers_worker_only_inventory_without_attaching_it_to_coordinator(tmp_path, engine, role):
    from cortex_runtime.portable.engine_contract import EngineCapability
    from engine_fakes import (
        default_scenario, fake_launch_command, isolated_environ,
        make_connection_record, new_connection_id, write_scenario,
    )
    environ = isolated_environ(tmp_path)
    dump = tmp_path / "session.json"
    scenario = write_scenario(tmp_path, default_scenario(session_new_params_dump_path=str(dump)))
    record = make_connection_record(new_connection_id(), fake_launch_command(scenario),
                                    engine=engine, environ=environ)
    servers = _servers()[:1]
    manager = EngineSessionManager(
        record, str(tmp_path), environ=environ, call_timeout=5,
        channel_credentials=("ch_" + "0" * 12, "a" * 20, str(tmp_path)),
        host_mcp_servers=servers, mcp_role=role,
        mcp_role_id="worker-explorer" if role == "worker" else None,
    )
    try:
        manager.negotiate((EngineCapability.SESSION_CREATION, EngineCapability.PROMPT_STREAMING))
        session = manager.create_session("t_" + "a" * 12, "xr_" + "0" * 24,
                                         session_new_meta=dict(systemPrompt="Role"))
        wire = json.loads(dump.read_text())
        inventory = manager.external_mcp_inventory(session)
    finally:
        manager.shutdown(grace=2)
    assert wire["mcpServers"][0]["name"] == "cortex"
    assert len(wire["mcpServers"]) == (1 if role == "coordinator" else 2)
    prompt = wire["_meta"]["systemPrompt"]
    assert servers[0]["name"] in prompt and "private-value" not in json.dumps(wire)
    if role == "coordinator":
        assert "# Configured worker MCP capabilities" in prompt
        assert "# Attached MCP servers" not in prompt
        assert inventory["server_count"] == 0
    else:
        assert "# Configured worker MCP capabilities" not in prompt
        assert "# Attached MCP servers" in prompt
        assert "update_record" not in prompt
        assert inventory["server_count"] == 1
    assert inventory["tool_schemas_observed"] is False


def test_opencode_launch_prompt_carries_same_separate_inventory():
    manager = EngineSessionManager.__new__(EngineSessionManager)
    manager._engine_instructions = "Role"
    manager._host_mcp_servers = _servers()[:1]
    manager._mcp_role = "coordinator"
    manager._mcp_role_id = None
    manager._record = SimpleNamespace(engine="opencode")
    manager._workspace_root = "/fixture"
    prompt = manager._opencode_prompt()
    assert "# Configured worker MCP capabilities" in prompt
    assert "# Attached MCP servers" not in prompt
    assert manager._host_mcp_servers[0]["name"] in prompt


def test_composed_guidance_preserves_direct_reads_and_denial_boundary_for_all_profiles():
    coordinator = " ".join(composed_instructions(load_role("orchestrator")).split())
    for phrase in ("granted informational reads", "submit a direct answer",
                   "outside your coordinator role", "suitable specialist objective",
                   "never retry a denied action through another tool, engine or worker",
                   "unconfigured or unknown", "runtime validates and schedules"):
        assert phrase in coordinator
    profiles = [role for role in known_role_ids() if role.startswith("worker-")]
    assert len(profiles) == 23
    for role in profiles:
        instructions = composed_instructions(load_role(role))
        assert "refusal is a decision, not a glitch" in " ".join(instructions.split())
        assert "never reach the same effect another way" in instructions
    names = {tool["name"] for tool in mcp_tools.tool_definitions("coordinator")}
    assert {"plan_submit", "answer_submit", "project_read"} <= names
    assert not {"command_run", "outside_command", "project_write", "result_submit"} & names
    worker = {tool["name"] for tool in mcp_tools.tool_definitions("worker", role_id="worker-explorer")}
    assert "plan_submit" not in worker and "result_submit" in worker


def test_coordinator_command_boundary_is_enforced_without_a_fallback(tmp_path):
    from cortex_runtime.orchestration.state import TaskMachine
    from cortex_runtime.trusted_channel import resolve_channel

    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="denial"), "Question", "owner")
    channel = resolve_channel(store, task.channel_id, task.secret)
    TaskMachine(store).create(task.task_id, "Question")
    with pytest.raises(StoreError, match="gateway_command_denied"):
        mcp_tools.dispatch(store, channel, "command_run", dict(argv=["true"], request_key="forbidden"))
    assert TaskMachine(store).context(task.task_id)["nodes"] == []


@pytest.mark.parametrize("objective", [
    "Investigate the owner-selected outside folder with authorized bounded read/search commands; no writes.",
    "Locate the unknown external source through supported read-only discovery; do not demand pasted data.",
    "Investigate Paseo session 615aaab2-eb98-4df7-acee-8959a3f956e1, not a local Cortex report. "
    "Discover a safe installed persisted-history/export reader; exact-session bounded reads only, "
    "no resume/create or production mutation. Report identity, completeness or unsupported access.",
])
def test_scripted_specialist_objective_reaches_runtime_dispatched_worker(tmp_path, objective):
    """Real plan/dispatch handlers, scripted reasoning; no external MCP invocation."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
    from cortex_runtime.orchestration.state import TaskMachine
    from cortex_runtime.portable.engine_contract import StopReason
    from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace
    from runtime_scenarios import publish_result

    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(store, SourceEvent(origin="test", event_id="delegation"),
                          "Inspect an external record through an authorized specialist.", "owner")
    servers = _servers()[:1]
    assert "read_record" in worker_capabilities_section(servers)
    observed = []

    def router(store, channel, ref, launch, instructions):
        if channel.role == "coordinator":
            assert "outside your coordinator role" in instructions
            def coordinator(store, channel, ref, message, index):
                context = mcp_tools.dispatch(store, channel, "task_get_context", {})
                if context["state"] == "PLANNING":
                    accepted = mcp_tools.dispatch(store, channel, "plan_submit", dict(
                        plan=dict(tasks=[dict(id="inspect", role="explorer", objective=objective,
                                              dependencies=[], scope=["."], model="auto")],
                                  verification=[["python3", "-B", "-c", "print('fixture verification')"]]),
                        request_key="specialist-plan"))
                    assert accepted["state"] == "PLAN_READY"
                elif context["state"] == "RECONCILING":
                    mcp_tools.dispatch(store, channel, "synthesis_submit", dict(text="Fixture handoff checked."))
                return PromptOutcome(StopReason.END_TURN, "")
            return coordinator
        assert "Do not create subagents" in instructions
        def worker(store, channel, ref, message, index):
            observed.append(str(message))
            return publish_result(store, channel)
        return worker

    runtime = OrchestratingRuntime(store, FakeSessionRuntime(store, router), FakeWorkspace(),
                                   FakeConfigSnapshotSource(), allow_unqualified_delivery=True,
                                   wait_timeout_seconds=20)
    try:
        runtime.start_prompt(task.task_id, "incoming", "Inspect the record.", lambda _: None, threading.Event())
    finally:
        runtime.shutdown()
    assert observed and any(objective in message for message in observed)
    context = TaskMachine(store).context(task.task_id)
    assert context["nodes"][0]["role"] == "worker-explorer"
    assert context["nodes"][0]["status"] == "completed"


def test_native_hints_work_without_external_attachments_and_follow_current_offers(monkeypatch):
    note = native_worker_capabilities_section()
    for label, role_id in (("read-only investigator", "worker-explorer"),
                           ("writing specialist", "worker-backend-dev"),
                           ("consultant", "worker-senior-consultant")):
        row = next(line for line in note.splitlines() if line.startswith(f"- {label}:"))
        offered = {tool["name"] for tool in mcp_tools.tool_definitions("worker", role_id=role_id)}
        for name in ("command_run", "access_request", "outside_command", "outside_write", "read_large_output"):
            assert (f"`{name}`" in row) == (name in offered)
    assert len(note) < 1800
    for inventory in (None, []):
        prompt = with_attached_servers({}, [], role="coordinator", worker_servers=inventory)["systemPrompt"]["append"]
        assert note in prompt
        assert "# Attached MCP servers" not in prompt
        assert "not external grants" in prompt
    monkeypatch.setattr(mcp_tools, "tool_definitions", lambda *a, **k: [])
    unavailable = native_worker_capabilities_section()
    assert unavailable.count("none of these operations") == 3
    assert "`command_run`" not in unavailable


def test_external_investigation_guidance_is_delivered_to_ordinary_workers_not_consultant():
    coordinator = " ".join(composed_instructions(load_role("orchestrator")).split())
    for phrase in ("unknown source location is the worker's discovery objective",
                   "Do not demand pasted data", "host session UUID is a host identity",
                   "not a current-project Cortex report reference", "no-resume/no-mutation"):
        assert phrase in coordinator
    for role in known_role_ids():
        if not role.startswith("worker-"):
            continue
        text = " ".join(composed_instructions(load_role(role)).split())
        if role == "worker-senior-consultant":
            assert "## External investigation" not in text
            continue
        for phrase in ("## External investigation", "no assumed Cortex outside-read/search API",
                       "not arbitrary external access", "installed help, storage or export",
                       "may load/resume/create", "page/byte/event coverage and truncation",
                       "If no safe mechanism is observable"):
            assert phrase in text, (role, phrase)


def test_paseo_uuid_is_not_a_report_identifier_or_project_read_path(tmp_path):
    from cortex_runtime.contracts import validate
    from cortex_runtime.orchestration.gateway import _read

    session_id = "615aaab2-eb98-4df7-acee-8959a3f956e1"
    with pytest.raises(StoreError, match="invalid_arguments"):
        validate("read_report", dict(report_id=session_id))
    for path in ("/outside/" + session_id, "../" + session_id):
        with pytest.raises(StoreError):
            _read(tmp_path, dict(path=path))


@pytest.mark.parametrize("development", [True, False])
def test_native_outside_read_discovery_uses_real_cwd_validation(tmp_path, development):
    """Real cwd validator + bounded read; not a sandbox/process qualification."""
    from cortex_runtime.orchestration.gateway import _command_cwd

    root = tmp_path / "project"
    root.mkdir()
    outside = tmp_path / ("archive-" + uuid.uuid4().hex[:8])
    outside.mkdir()
    data = outside / "record.txt"
    data.write_bytes(b"exact record")
    if not development:
        with pytest.raises(StoreError, match="gateway_command_cwd_denied"):
            with _command_cwd(root, dict(cwd=str(outside)), development):
                pytest.fail("Guarded cwd must not be admitted")
        return
    with _command_cwd(root, dict(cwd=str(outside)), development) as (cwd, fd):
        assert cwd == str(outside) and fd >= 0
        with data.open("rb") as source:
            chunk = source.read(64)
            assert chunk == b"exact record" and source.read(1) == b""
    assert data.read_bytes() == b"exact record"


def _fixture_read_command(selected, identity):
    script = (
        "import json,sys; from pathlib import Path; "
        "source=Path(sys.argv[1]); "
        "f=source.open('rb'); data=f.read(1024); remainder=f.read(1); f.close(); "
        "assert not remainder; record=json.loads(data); "
        "assert record['id']==sys.argv[2]; "
        "assert [e['id'] for e in record['events']]==['event-1']; "
        "print('exact binding; 1 event; complete; no truncation')"
    )
    return ["python3", "-B", "-c", script, str(selected), identity]


@pytest.mark.parametrize("case", ["exact", "mismatch", "oversize"])
def test_outside_fixture_reader_rejects_mismatch_and_truncation(tmp_path, case):
    """Qualifies the fixture command only, not gateway or host containment."""
    import subprocess

    selected = tmp_path / "selected.json"
    selected.write_text(json.dumps(dict(id="exact-id", events=[dict(id="event-1")])))
    if case == "oversize":
        selected.write_text(selected.read_text() + " " * 1024)
    result = subprocess.run(_fixture_read_command(selected, "wrong-id" if case == "mismatch" else "exact-id"),
                            capture_output=True, text=True, timeout=5)
    if case == "exact":
        assert result.returncode == 0, result.stderr
        assert result.stdout.splitlines() == ["exact binding; 1 event; complete; no truncation"]
    else:
        assert result.returncode != 0 and not result.stdout


@pytest.mark.requires_bwrap
@pytest.mark.parametrize("role", ["explorer", "backend-dev"])
@pytest.mark.parametrize("mode", ["guarded", "auto"])
def test_real_worker_command_reports_host_var_tmp_visibility_truthfully(tmp_path, role, mode):
    """Real namespace observation, not a general host-read capability claim."""
    from cortex_runtime.configuration import access_mode
    from test_runtime_gateway import _role_worker

    project = tmp_path / "project"
    project.mkdir()
    import tempfile
    from pathlib import Path
    with tempfile.TemporaryDirectory(prefix="cortex-host-visibility-", dir="/var/tmp") as directory:
        selected = Path(directory) / "selected.json"
        identity = uuid.uuid4().hex
        data = json.dumps(dict(id=identity, events=[dict(id="event-1")]))
        selected.write_text(data)
        store, worker, _ = _role_worker(project, role)
        access_mode.switch(store, worker.task_id, mode, origin="acp_set_mode")
        result = mcp_tools.dispatch(store, worker, "command_run", dict(
            argv=_fixture_read_command(selected, identity), request_key="outside-fixture-read"))
        assert result["complete"] and not result["output_truncated"]
        if role == "backend-dev" and mode == "auto":
            assert result["exit_code"] == 0
            assert result["output"].splitlines() == ["exact binding; 1 event; complete; no truncation"]
        else:
            assert result["exit_code"] != 0 and "FileNotFoundError" in result["output"]
        assert result["profile"] == ("read_only" if role == "explorer" else
                                      "development" if mode == "auto" else "restricted")
        assert selected.read_text() == data
