"""engine.claude_adapter_acp.build_claude_launch() and end-to-end pinned
launch through EngineProcess/EngineSessionManager against the fake engine.

Mirrors tests/test_engine_codex_launch.py, with the notable difference that
``build_claude_launch``'s ``pinned_engine_vars`` is always the empty dict
(see that function's own docstring: claude-agent-acp bundles no separate
spawned binary to pin an env var to), so no ``install=`` is ever passed to
``EngineSessionManager`` here -- ``EngineProcess`` only ever accepts an
``install`` for ``record.engine == "codex-acp"`` (see ``engine.process``);
launching claude-agent-acp works exactly like any other non-codex engine,
with ``claude_install.verify_install`` called independently by
``build_claude_launch`` before the launch command is ever used.
"""
from __future__ import annotations

import hashlib
import json
import stat
import sys
from pathlib import Path

import pytest

from cortex_runtime.engine.claude_adapter_acp import build_claude_launch, claude_option_mapping
from cortex_runtime.engine.claude_install import ClaudeAcpInstall
from cortex_runtime.engine.session_manager import EngineSessionManager
from cortex_runtime.portable.engine_contract import EngineCapability
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import ConfigurationRevision, ExecutionRoute

from engine_fakes import isolated_environ, make_connection_record, new_connection_id

REQUESTED_CAPABILITIES = (EngineCapability.SESSION_CREATION, EngineCapability.CONFIGURATION_OPTIONS)
FIXTURE_PATH = Path(__file__).resolve().parent / "fixtures/catalog/claude_agent_acp_config_options.json"
FAKE_ENGINE_TEMPLATE = (Path(__file__).resolve().parent / "fake_engine_process.py").read_text(encoding="utf-8")


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _write_executable(path: Path, content: bytes = b"#!/bin/sh\nexit 0\n") -> None:
    path.write_bytes(content)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


def _observed_config_options():
    return json.loads(FIXTURE_PATH.read_text(encoding="utf-8"))["configOptions"]


def _write_fake_engine_adapter_entry(tmp_path: Path, scenario: dict) -> Path:
    marker = "def main() -> int:"
    assert marker in FAKE_ENGINE_TEMPLATE, "fake_engine_process.py no longer defines main(); update this template split"
    preamble = FAKE_ENGINE_TEMPLATE.split(marker)[0]
    scenario_json = json.dumps(scenario)
    script = preamble + (
        "\ndef main() -> int:\n"
        f"    scenario = json.loads({scenario_json!r})\n"
        "    FakeEngine(scenario).run()\n"
        "    return 0\n"
        "\n\nif __name__ == \"__main__\":\n"
        "    raise SystemExit(main())\n"
    )
    path = tmp_path / "adapter-entry.py"
    path.write_text(script, encoding="utf-8")
    return path


def _make_install(tmp_path: Path, scenario: dict):
    install_dir = tmp_path / "install"
    install_dir.mkdir()
    adapter_entry = _write_fake_engine_adapter_entry(install_dir, scenario)
    package_json = install_dir / "package.json"
    package_json.write_text(json.dumps({"name": "@agentclientprotocol/claude-agent-acp", "version": "0.78.0"}), encoding="utf-8")
    resolved_node_binary = str(Path(sys.executable).resolve())
    return ClaudeAcpInstall(
        node_binary=resolved_node_binary,
        adapter_entry=str(adapter_entry),
        package_json=str(package_json),
        package_version="0.78.0",
        package_json_sha256=_sha256(package_json),
        adapter_entry_sha256=_sha256(adapter_entry),
    )


def _revision(model="model-a", supported_parameters=(("thought_level", "medium"),)):
    route = ExecutionRoute(
        engine="claude-agent-acp", connection_id="cn_" + "0" * 12, model=model, supported_parameters=supported_parameters,
    )
    return ConfigurationRevision(id="cr_" + "0" * 12, task_id="t_" + "0" * 12, revision=1, route=route)


def test_build_claude_launch_returns_command_and_empty_pinned_vars(tmp_path):
    scenario = dict(agent_capabilities={}, session_id="sess-1", config_options=_observed_config_options())
    install = _make_install(tmp_path, scenario)
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=None, engine="claude-agent-acp", environ=environ)
    launch_command, pinned_engine_vars = build_claude_launch(record, install, environ)
    assert launch_command == (install.node_binary, install.adapter_entry)
    assert pinned_engine_vars == {}


def test_build_claude_launch_rejects_non_claude_engine(tmp_path):
    scenario = dict(agent_capabilities={}, session_id="sess-1", config_options=[])
    install = _make_install(tmp_path, scenario)
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=None, engine="codex-acp", environ=environ)
    with pytest.raises(PortableError) as excinfo:
        build_claude_launch(record, install, environ)
    assert excinfo.value.code == "unsupported_engine"


def test_build_claude_launch_rejects_mismatched_existing_launch_command(tmp_path):
    scenario = dict(agent_capabilities={}, session_id="sess-1", config_options=[])
    install = _make_install(tmp_path, scenario)
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    record = make_connection_record(
        connection_id, launch_command=("/usr/bin/other", "/other/entry.js"), engine="claude-agent-acp", environ=environ,
    )
    with pytest.raises(PortableError) as excinfo:
        build_claude_launch(record, install, environ)
    assert excinfo.value.code == "engine_launch_command_mismatch"


def test_build_claude_launch_reverifies_and_rejects_drifted_install(tmp_path):
    scenario = dict(agent_capabilities={}, session_id="sess-1", config_options=[])
    install = _make_install(tmp_path, scenario)
    Path(install.adapter_entry).write_text("# tampered\n", encoding="utf-8")
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=None, engine="claude-agent-acp", environ=environ)
    with pytest.raises(PortableError) as excinfo:
        build_claude_launch(record, install, environ)
    assert excinfo.value.code == "engine_install_drifted"


def test_end_to_end_launch_never_pins_a_binary_and_never_leaks_secrets(tmp_path):
    env_dump_path = tmp_path / "env-dump.json"
    scenario = dict(
        agent_capabilities={},
        session_id="sess-1",
        config_options=_observed_config_options(),
        configure_behavior="accept",
        env_dump_path=str(env_dump_path),
    )
    install = _make_install(tmp_path, scenario)
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    launch_command, pinned = build_claude_launch(
        make_connection_record(connection_id, launch_command=None, engine="claude-agent-acp", environ=environ),
        install, environ,
    )
    assert pinned == {}
    record = make_connection_record(connection_id, launch_command=launch_command, engine="claude-agent-acp", environ=environ)
    # No install= passed here: EngineProcess only accepts install for
    # engine=="codex-acp" (see engine.process); the install was already
    # re-verified above by build_claude_launch itself.
    manager = EngineSessionManager(
        record, str(tmp_path), environ=environ, option_mapping=claude_option_mapping, call_timeout=5.0,
    )
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        assert tuple(manager._process.process.args) == (install.node_binary, install.adapter_entry)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        evidence_ref = manager.configure(session_id, _revision())
        assert "model=model-a" in evidence_ref
        assert "effort=medium" in evidence_ref
    finally:
        manager.shutdown(grace=2.0)

    observed_env_keys = set(json.loads(env_dump_path.read_text(encoding="utf-8")))
    assert "CLAUDE_CONFIG_DIR" in observed_env_keys  # this connection's own isolation var
    for leaked_name in (
        "ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN", "CODEX_API_KEY", "OPENAI_API_KEY",
        "CORTEX_CHANNEL_ID", "CORTEX_CHANNEL_SECRET", "CORTEX_CHANNEL_PROJECT_ROOT",
    ):
        assert leaked_name not in observed_env_keys
    # F: build_claude_launch never introduces a CODEX_PATH/NO_BROWSER-style
    # pinned var (those names are only ever valid for engine=="codex-acp").
    assert "CODEX_PATH" not in observed_env_keys
    assert "NO_BROWSER" not in observed_env_keys


def test_end_to_end_unsupported_configuration_never_reaches_the_wire(tmp_path):
    scenario = dict(
        agent_capabilities={}, session_id="sess-1", config_options=_observed_config_options(), configure_behavior="accept",
    )
    install = _make_install(tmp_path, scenario)
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    launch_command, _pinned = build_claude_launch(
        make_connection_record(connection_id, launch_command=None, engine="claude-agent-acp", environ=environ),
        install, environ,
    )
    record = make_connection_record(connection_id, launch_command=launch_command, engine="claude-agent-acp", environ=environ)
    manager = EngineSessionManager(
        record, str(tmp_path), environ=environ, option_mapping=claude_option_mapping, call_timeout=5.0,
    )
    try:
        from cortex_runtime.configuration.service import ConfigurationRejected

        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        with pytest.raises(ConfigurationRejected):
            manager.configure(session_id, _revision(model="not-a-real-model"))
    finally:
        manager.shutdown(grace=2.0)
