"""T026 acceptance bullet 3: ``EngineSessionManagerAdapter.open`` selects
each connection's own pinned adapter ``option_mapping`` (T013's
``codex_option_mapping`` for ``codex-acp``, T026's own
``claude_option_mapping`` for ``claude-agent-acp``) -- never the generic,
unqualified ``engine.session_manager._default_option_mapping`` any real
route previously always fell back to (see
``delegation.live_adapters._option_mapping_for``'s own docstring for the
fix this backs).

Proves this two ways: (1) direct dispatch-function identity, and (2) an
end-to-end real ``EngineSessionManager`` (via the adapter, against
``tests/fake_engine_process.py``, never a real engine) for each engine,
where each manager's own ``configure()`` accepts a value shaped for its
*own* pinned fixture and rejects the *other* engine's shaped value --
proving a claude-agent-acp route never receives a codex-acp-specific
parameter/rejection rule and vice versa. Also proves neither engine's own
launched process environment ever contains the other's secret-shaped
variable names.
"""
from __future__ import annotations

import json
import sqlite3
from pathlib import Path
from types import SimpleNamespace

import pytest

from cortex_runtime.configuration.service import ConfigurationRejected
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter, _option_mapping_for, _session_new_meta_for, base_prompt_mode
from cortex_runtime.engine.claude_adapter_acp import claude_option_mapping
from cortex_runtime.engine.codex_adapter_acp import codex_option_mapping
from cortex_runtime.portable.identity import ConfigurationRevision, ExecutionRoute

from cortex_runtime.delegation.codex_tool_cheat_sheet import codex_tool_cheat_sheet
from cortex_runtime.delegation.mcp_tools import assignment_profile
from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario

CODEX_FIXTURE = Path(__file__).resolve().parent / "fixtures/catalog/codex_acp_config_options.json"
CLAUDE_FIXTURE = Path(__file__).resolve().parent / "fixtures/catalog/claude_agent_acp_config_options.json"


def _config_options(fixture_path):
    return json.loads(fixture_path.read_text(encoding="utf-8"))["configOptions"]


def _with_cheat_sheet(role, instructions, role_id=None):
    # codex-acp sessions always get the schema-derived tool cheat sheet
    # appended to their role instructions text (codex_tool_cheat_sheet.py).
    sheet = codex_tool_cheat_sheet(role, role_id=role_id)
    return instructions if not sheet else instructions + "\n\n" + sheet


@pytest.fixture
def stage_store():
    # Only the binding tables are needed to exercise the real stage query.
    db = sqlite3.connect(":memory:")
    db.row_factory = sqlite3.Row
    db.executescript("""
        CREATE TABLE delegation_provisioning (task_id, attempt_id, idempotency_key);
        CREATE TABLE runtime_nodes (task_id, assignment_ref, revision, role, stage);
        INSERT INTO delegation_provisioning VALUES ('task-a', 'attempt-a', 'assignment-a');
        INSERT INTO runtime_nodes VALUES ('task-a', 'assignment-a', 1, 'worker-backend-dev', 'execution');
        INSERT INTO runtime_nodes VALUES ('task-a', 'assignment-a', 2, 'worker-backend-dev', 'review');
        INSERT INTO runtime_nodes VALUES ('task-b', 'assignment-a', 3, 'worker-backend-dev', 'execution');
    """)
    try:
        yield SimpleNamespace(connection=lambda: db)
    finally:
        db.close()


def test_assignment_stage_matches_tools_list_binding(stage_store):
    adapter = EngineSessionManagerAdapter(lambda _: None, lambda _: ".", store=stage_store)
    channel = SimpleNamespace(role="worker", task_id="task-a", attempt_id="attempt-a")
    assert adapter._assignment_stage("task-a", "attempt-a") == assignment_profile(stage_store, channel)[1] == "review"
    assert adapter._assignment_stage("task-b", "attempt-a") is None
    assert adapter._assignment_stage("task-a", "missing-attempt") is None

    no_store = EngineSessionManagerAdapter(lambda _: None, lambda _: ".")
    assert no_store._assignment_stage("task-a", "attempt-a") is None


def test_codex_session_open_uses_bound_review_stage(tmp_path, monkeypatch, stage_store):
    environ = isolated_environ(tmp_path)
    dump = tmp_path / "session-new.json"
    scenario = write_scenario(tmp_path, default_scenario(
        config_options=_config_options(CODEX_FIXTURE),
        session_new_params_dump_path=str(dump),
    ))
    record = make_connection_record(
        new_connection_id(), launch_command=fake_launch_command(scenario),
        engine="codex-acp", environ=environ,
    )
    adapter = EngineSessionManagerAdapter(
        lambda _: record, lambda _: str(tmp_path), environ=environ, call_timeout=10.0,
    )
    # Use the real DB-backed resolver without enabling unrelated store-backed
    # capacity/configuration services in this fake-engine protocol test.
    lookup = EngineSessionManagerAdapter(lambda _: None, lambda _: ".", store=stage_store)
    monkeypatch.setattr(adapter, "_assignment_stage", lookup._assignment_stage)
    try:
        adapter.open("shared_project_root", {
            "task_id": "task-a", "attempt_id": "attempt-a", "assignment_id": "assignment-a",
            "role_id": "worker-backend-dev", "stage": "execution",
        }, "Role instructions")
        meta = json.loads(dump.read_text(encoding="utf-8"))["_meta"]
        assert meta["systemPrompt"] == "Role instructions\n\n" + codex_tool_cheat_sheet(
            "worker", role_id="worker-backend-dev", stage="review",
        )
        assert '"verdict":' in meta["systemPrompt"]
    finally:
        for manager in adapter._managers.values():
            manager.shutdown(grace=2.0)


def test_generation_metadata_fallback_uses_bound_review_stage(tmp_path, monkeypatch, stage_store):
    from cortex_runtime.delegation import generations, session_skills
    from cortex_runtime.observability import events
    from cortex_runtime.portable import instruction_bundle

    sent = []
    ledger = SimpleNamespace(
        save_intended=lambda bundle: None,
        record_sent=lambda bundle, payload, **kwargs: sent.append(json.loads(payload)),
    )
    monkeypatch.setattr(instruction_bundle, "InstructionBundleLedger", lambda store: ledger)
    monkeypatch.setattr(generations, "ensure_owner", lambda *args, **kwargs: SimpleNamespace(tier="automatic"))
    monkeypatch.setattr(generations, "open_generation", lambda *args, **kwargs: {"id": "generation-a"})
    monkeypatch.setattr(session_skills, "record_session_skills", lambda *args, **kwargs: None)
    monkeypatch.setattr(events, "append_event", lambda *args, **kwargs: None)
    adapter = EngineSessionManagerAdapter(lambda _: None, lambda _: str(tmp_path), store=stage_store)
    adapter._open_generation(
        SimpleNamespace(_workspace_root=str(tmp_path)),
        SimpleNamespace(engine="codex-acp", id="connection-a"),
        "session-a", "task-a", "attempt-a",
        {"assignment_id": "assignment-a", "role_id": "worker-backend-dev"},
        instructions="Role instructions", cause="session_open",
    )
    assert len(sent) == 1
    assert sent[0]["systemPrompt"] == "Role instructions\n\n" + codex_tool_cheat_sheet(
        "worker", role_id="worker-backend-dev", stage="review",
    )


def test_option_mapping_for_dispatches_by_engine_name():
    assert _option_mapping_for("codex-acp") is codex_option_mapping
    assert _option_mapping_for("claude-agent-acp") is claude_option_mapping
    assert _option_mapping_for("some-future-engine") is None


GATEWAY_ONLY = dict(disallowedTools=['Agent','Task','Workflow','TeamCreate','TeamDelete','SendMessage','ScheduleWakeup','Monitor','CronCreate','CronDelete','CronList','RemoteTrigger','EnterWorktree','ExitWorktree','PushNotification','EnterPlanMode','ExitPlanMode','TaskStop','Bash','Read','Edit','Write','Grep','Glob','Skill','NotebookEdit'],strictMcpConfig=True,skills=[],plugins=[],managedSettings=dict(disableAllHooks=True,autoMemoryEnabled=False))
# With the unified Cortex prompt Claude's own skill listing, attribution and
# git instructions are switched off as well.
UNIFIED = dict(GATEWAY_ONLY, skills=[], settings=dict(attribution=dict(commit='', pr=''), includeGitInstructions=False))
NATIVE_SKILLS = {
    key: value for key, value in GATEWAY_ONLY.items() if key != "skills"
}
NATIVE_SKILLS["disallowedTools"] = [
    tool for tool in GATEWAY_ONLY["disallowedTools"] if tool != "Skill"
]


def test_session_new_meta_for_pinned_engines_builds_a_payload():
    from cortex_runtime.engine.unified_system_prompt import cortex_system_prompt

    base = cortex_system_prompt('worker', None, code_mode=True)
    assert _session_new_meta_for('codex-acp','1.12.0','Be helpful.') == {
        'systemPrompt':_with_cheat_sheet('worker','Be helpful.'),'cortexRole':'worker','cortexBaseInstructions':base,'cortexNativeContext':False}
    assert _session_new_meta_for('claude-agent-acp','0.78.0','Be helpful.') == dict(
        systemPrompt=cortex_system_prompt('worker', None, native_tools=False).rstrip()+'\n\nBe helpful.',claudeCode=dict(options=UNIFIED))
    assert _session_new_meta_for('claude-agent-acp','0.11.0','Be helpful.') is None
    for text in [None,'']:
        assert _session_new_meta_for('claude-agent-acp','0.78.0',text) == dict(claudeCode=dict(options=GATEWAY_ONLY))


def test_session_new_meta_uses_native_prompts_and_skill_catalogs_when_switch_disabled(caplog):
    caplog.set_level("INFO")
    environ = {"CORTEX_UNIFIED_BASE_PROMPT": "0"}

    codex = _session_new_meta_for('codex-acp', '1.12.0', 'Be helpful.', environ=environ)
    assert codex == {'systemPrompt': _with_cheat_sheet('worker', 'Be helpful.'), 'cortexRole': 'worker'}

    claude = _session_new_meta_for('claude-agent-acp', '0.78.0', 'Be helpful.', environ=environ)
    assert claude == {
        'systemPrompt': {'append': 'Be helpful.'},
        'claudeCode': {'options': NATIVE_SKILLS},
    }
    assert "Cortex unified base prompt disabled" in caplog.text


def test_base_prompt_mode_classifies_pinned_engine_metadata():
    unified_codex = _session_new_meta_for('codex-acp', '1.12.0', 'Be helpful.')
    native_codex = _session_new_meta_for(
        'codex-acp', '1.12.0', 'Be helpful.', environ={'CORTEX_UNIFIED_BASE_PROMPT': '0'},
    )
    unified_claude = _session_new_meta_for('claude-agent-acp', '0.78.0', 'Be helpful.')
    native_claude = _session_new_meta_for(
        'claude-agent-acp', '0.78.0', 'Be helpful.', environ={'CORTEX_UNIFIED_BASE_PROMPT': '0'},
    )

    assert base_prompt_mode(unified_codex) == 'unified'
    assert base_prompt_mode(native_codex) == 'native'
    assert base_prompt_mode(unified_claude) == 'unified'
    assert base_prompt_mode(native_claude) == 'native'
    assert base_prompt_mode(None) == 'unknown'


def test_every_claude_role_disallows_native_gateway_alternatives():
    for role in ['worker','coordinator']:
        meta=_session_new_meta_for('claude-agent-acp','0.78.0','Role instructions',role=role)
        assert meta['claudeCode']['options']==UNIFIED
        assert meta['systemPrompt'].startswith('# Cortex session\n\nYou are ' + ('the Cortex coordinator' if role=='coordinator' else 'a Cortex worker'))
        assert meta['systemPrompt'].endswith('\n\nRole instructions')


def _open_one(tmp_path, engine, config_options):
    engine_dir = tmp_path / engine
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    scenario_path = write_scenario(engine_dir, default_scenario(config_options=config_options, configure_behavior="accept"))
    connection_id = new_connection_id()
    record = make_connection_record(
        connection_id, launch_command=fake_launch_command(scenario_path), engine=engine, environ=environ,
    )
    adapter = EngineSessionManagerAdapter(
        lambda task_id: record, lambda task_id: str(tmp_path), environ=environ, call_timeout=10.0,
    )
    attempt_id = "xa_" + connection_id[3:]
    session_id = adapter.open("shared_project_root", dict(task_id="t_aaaaaaaaaaaa", attempt_id=attempt_id), "instructions")
    manager = adapter._managers[attempt_id]
    return adapter, manager, session_id


def _revision(model, supported_parameters=()):
    route = ExecutionRoute(
        engine="ignored-here", connection_id="cn_" + "0" * 12, model=model, supported_parameters=supported_parameters,
    )
    return ConfigurationRevision(id="cr_" + "0" * 12, task_id="t_aaaaaaaaaaaa", revision=1, route=route)


def test_codex_acp_route_uses_codex_option_mapping_and_rejects_claude_shaped_effort(tmp_path):
    adapter, manager, session_id = _open_one(tmp_path, "codex-acp", _config_options(CODEX_FIXTURE))
    try:
        # Own fixture's real shape: accepted.
        evidence_ref = manager.configure(session_id, _revision("model-a", (("thought_level", "medium"),)))
        assert "reasoning_effort=medium" in evidence_ref
        # claude-agent-acp's own effort id is "effort", never "reasoning_effort"
        # -- and claude's mode values (e.g. "acceptEdits") are never valid
        # codex-acp mode values. Both must be rejected under codex's own
        # option_mapping, proving no claude-shaped value silently passes.
        with pytest.raises(ConfigurationRejected) as excinfo:
            manager.configure(session_id, _revision("model-a", (("mode", "acceptEdits"),)))
        assert "mode" in excinfo.value.reason
    finally:
        manager.shutdown(grace=2.0)


def test_claude_agent_acp_route_uses_claude_option_mapping_and_rejects_codex_shaped_mode(tmp_path):
    adapter, manager, session_id = _open_one(tmp_path, "claude-agent-acp", _config_options(CLAUDE_FIXTURE))
    try:
        evidence_ref = manager.configure(session_id, _revision("model-a", (("thought_level", "medium"),)))
        assert "effort=medium" in evidence_ref
        # codex-acp's own mode values (e.g. "agent") are never valid
        # claude-agent-acp mode values (default/acceptEdits/plan/auto/
        # bypassPermissions) -- must be rejected under claude's own
        # option_mapping.
        with pytest.raises(ConfigurationRejected) as excinfo:
            manager.configure(session_id, _revision("model-a", (("mode", "agent"),)))
        assert "mode" in excinfo.value.reason
    finally:
        manager.shutdown(grace=2.0)


def test_each_pinned_adapter_gets_its_own_session_new_meta(tmp_path):
    # claude-agent-acp: instructions delivered via the real session/new
    # _meta.systemPrompt channel (T026) -- never absent, never a codex-
    # style leading-prompt-block artifact.
    session_new_dump = tmp_path / "claude-session-new.json"
    engine_dir = tmp_path / "claude-dump"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    scenario_path = write_scenario(engine_dir, default_scenario(
        config_options=_config_options(CLAUDE_FIXTURE),
        session_new_params_dump_path=str(session_new_dump),
    ))
    connection_id = new_connection_id()
    record = make_connection_record(
        connection_id, launch_command=fake_launch_command(scenario_path), engine="claude-agent-acp", environ=environ,
    )
    adapter = EngineSessionManagerAdapter(
        lambda task_id: record, lambda task_id: str(tmp_path), environ=environ, call_timeout=10.0,
    )
    attempt_id = "xa_" + connection_id[3:]
    adapter.open("shared_project_root", dict(task_id="t_aaaaaaaaaaaa", attempt_id=attempt_id), "You are a backend developer.")
    for manager in list(adapter._managers.values()):
        manager.shutdown(grace=2.0)

    dumped = json.loads(session_new_dump.read_text(encoding="utf-8"))
    assert dumped['_meta']['claudeCode'] == dict(options=UNIFIED)
    assert dumped['_meta']['systemPrompt'].startswith('# Cortex session\n\nYou are the Cortex coordinator')
    assert set(dumped['_meta']) == {'systemPrompt', 'claudeCode'}

    # The verified private Codex adapter forwards this metadata to App Server.
    # Role text is not repeated in an ordinary user prompt; recovery carries
    # only the task capsule through the conversation channel.
    session_new_dump_codex = tmp_path / "codex-session-new.json"
    engine_dir_codex = tmp_path / "codex-dump"
    engine_dir_codex.mkdir()
    environ_codex = isolated_environ(engine_dir_codex)
    scenario_path_codex = write_scenario(engine_dir_codex, default_scenario(
        config_options=_config_options(CODEX_FIXTURE),
        session_new_params_dump_path=str(session_new_dump_codex),
    ))
    connection_id_codex = new_connection_id()
    record_codex = make_connection_record(
        connection_id_codex, launch_command=fake_launch_command(scenario_path_codex), engine="codex-acp", environ=environ_codex,
    )
    adapter_codex = EngineSessionManagerAdapter(
        lambda task_id: record_codex, lambda task_id: str(tmp_path), environ=environ_codex, call_timeout=10.0,
    )
    attempt_id_codex = "xa_" + connection_id_codex[3:]
    adapter_codex.open(
        "shared_project_root", dict(task_id="t_bbbbbbbbbbbb", attempt_id=attempt_id_codex), "You are a backend developer.",
    )
    for manager in list(adapter_codex._managers.values()):
        manager.shutdown(grace=2.0)

    dumped_codex = json.loads(session_new_dump_codex.read_text(encoding="utf-8"))
    meta_codex = dumped_codex.get("_meta")
    from cortex_runtime.engine.unified_system_prompt import worker_capabilities_section
    inventory = worker_capabilities_section([])
    assert meta_codex["systemPrompt"] == (
        _with_cheat_sheet("coordinator", "You are a backend developer.").rstrip() + "\n\n" + inventory)
    assert meta_codex["cortexRole"] == "coordinator"
    assert set(meta_codex) == {"systemPrompt", "cortexRole", "cortexNativeContext", "cortexBaseInstructions"}
    assert meta_codex["cortexNativeContext"] is False
    assert meta_codex["cortexBaseInstructions"].startswith("# Cortex session\n\nYou are the Cortex coordinator")
    # Both coordinators receive the same separate worker-capability guidance.
    # Role/base text stays exact; only Codex receives its schema cheat sheet.
    from cortex_runtime.engine.unified_system_prompt import cortex_system_prompt
    assert dumped['_meta']['systemPrompt'] == cortex_system_prompt(
        "coordinator", str(tmp_path), environ=environ, native_tools=False
    ).rstrip() + "\n\nYou are a backend developer.\n\n" + inventory
    assert meta_codex["cortexBaseInstructions"] == cortex_system_prompt(
        "coordinator", str(tmp_path), environ=environ_codex, native_tools=True, code_mode=True
    )


@pytest.mark.parametrize("role", ("worker", "coordinator"))
@pytest.mark.parametrize("unified", (True, False))
def test_claude_0_87_preserves_role_effort_and_native_tool_metadata(role, unified):
    environ = {} if unified else {"CORTEX_UNIFIED_BASE_PROMPT": "0"}
    meta = _session_new_meta_for("claude-agent-acp", "0.87.0", "Role instructions",
                                role=role, effort="high", environ=environ)
    expected = dict(UNIFIED if unified else NATIVE_SKILLS, effort="high")
    assert meta["claudeCode"]["options"] == expected
    assert base_prompt_mode(meta) == ("unified" if unified else "native")
    if not unified:
        assert meta["systemPrompt"] == {"append": "Role instructions"}
    empty = _session_new_meta_for("claude-agent-acp", "0.87.0", None, role=role, effort="high")
    assert empty == {"claudeCode": {"options": dict(GATEWAY_ONLY, effort="high")}}



@pytest.mark.parametrize("role", ("worker", "coordinator"))
@pytest.mark.parametrize("unified", (True, False))
def test_codex_2_1_1_preserves_production_instruction_metadata(role, unified):
    environ = {} if unified else {"CORTEX_UNIFIED_BASE_PROMPT": "0"}
    meta = _session_new_meta_for("codex-acp", "2.1.1", "Role instructions",
                                role=role, environ=environ)
    expected_system_prompt = _with_cheat_sheet(role, "Role instructions")
    assert meta["systemPrompt"] == expected_system_prompt
    assert meta["cortexRole"] == role
    assert base_prompt_mode(meta) == ("unified" if unified else "native")
    if unified:
        assert meta["cortexNativeContext"] is False
        assert meta["cortexBaseInstructions"].startswith("# Cortex session")
    else:
        assert meta == {"systemPrompt": expected_system_prompt, "cortexRole": role}


@pytest.mark.parametrize("role", ["worker", "coordinator"])
@pytest.mark.parametrize("instructions", [None, "Cortex role"])
def test_claude_0_88_dispatch_includes_native_tool_policy(role, instructions):
    meta = _session_new_meta_for("claude-agent-acp", "0.88.0", instructions, role=role, effort="high")
    options = meta["claudeCode"]["options"]
    assert options["strictMcpConfig"] is True
    assert options["disallowedTools"] == GATEWAY_ONLY["disallowedTools"]
    assert options["effort"] == "high"
    assert "tools" not in options
    assert options["plugins"] == []
    assert options["managedSettings"] == {"disableAllHooks": True, "autoMemoryEnabled": False}
    if instructions:
        assert isinstance(meta["systemPrompt"], str)
        assert instructions in meta["systemPrompt"]
    else:
        assert "systemPrompt" not in meta
    assert _session_new_meta_for("claude-agent-acp", "0.89.0", instructions, role=role) is None
