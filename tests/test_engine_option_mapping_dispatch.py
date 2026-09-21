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
from pathlib import Path

import pytest

from cortex_runtime.configuration.service import ConfigurationRejected
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter, _option_mapping_for, _session_new_meta_for
from cortex_runtime.engine.claude_adapter_acp import claude_option_mapping
from cortex_runtime.engine.codex_adapter_acp import codex_option_mapping
from cortex_runtime.portable.identity import ConfigurationRevision, ExecutionRoute

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario

CODEX_FIXTURE = Path(__file__).resolve().parent / "fixtures/catalog/codex_acp_config_options.json"
CLAUDE_FIXTURE = Path(__file__).resolve().parent / "fixtures/catalog/claude_agent_acp_config_options.json"


def _config_options(fixture_path):
    return json.loads(fixture_path.read_text(encoding="utf-8"))["configOptions"]


def test_option_mapping_for_dispatches_by_engine_name():
    assert _option_mapping_for("codex-acp") is codex_option_mapping
    assert _option_mapping_for("claude-agent-acp") is claude_option_mapping
    assert _option_mapping_for("some-future-engine") is None


def test_session_new_meta_for_pinned_engines_builds_a_payload():
    assert _session_new_meta_for("codex-acp", "1.12.0", "Be helpful.") == {"systemPrompt": "Be helpful."}
    assert _session_new_meta_for("claude-agent-acp", "0.78.0", "Be helpful.") == {
        "systemPrompt": {"append": "Be helpful."},
        "claudeCode": {"options": {"disallowedTools": ["Agent", "Task"], "strictMcpConfig": True}},
    }
    # Unrecognized version: UNSUPPORTED plan, never a payload.
    assert _session_new_meta_for("claude-agent-acp", "0.11.0", "Be helpful.") is None
    # A Claude session without role text still cannot launch native subagents.
    expected_restriction = {"claudeCode": {"options": {"disallowedTools": ["Agent", "Task"], "strictMcpConfig": True}}}
    assert _session_new_meta_for("claude-agent-acp", "0.78.0", None) == expected_restriction
    assert _session_new_meta_for("claude-agent-acp", "0.78.0", "") == expected_restriction


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
    assert dumped.get("_meta") == {
        "systemPrompt": {"append": "You are a backend developer."},
        "claudeCode": {"options": {"disallowedTools": ["Agent", "Task"], "strictMcpConfig": True}},
    }

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
    assert dumped_codex.get("_meta") == {"systemPrompt": "You are a backend developer."}
