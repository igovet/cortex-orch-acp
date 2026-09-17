"""engine.codex_adapter_acp.build_codex_launch() and end-to-end pinned-env
launch through EngineProcess/EngineSessionManager against the fake engine.

The "fake engine" here is a *synthetic* standalone script generated from
``tests/fake_engine_process.py``'s own class/helpers (never real
codex-acp/Node) so it can be launched as an exact two-element
``(node_binary, adapter_entry)`` command -- the same shape
``codex_install.launch_command_for`` always returns -- with its scripted
scenario embedded directly in the generated source rather than passed as an
extra argv element (``launch_command_for`` never has room for one).
"""
from __future__ import annotations

import hashlib
import json
import stat
import sys
from pathlib import Path

import pytest

from cortex_runtime.engine.codex_adapter_acp import build_codex_launch, codex_option_mapping
from cortex_runtime.engine.codex_install import CodexAcpInstall
from cortex_runtime.engine.session_manager import EngineSessionManager
from cortex_runtime.portable.engine_contract import EngineCapability
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import ConfigurationRevision, ExecutionRoute

from engine_fakes import isolated_environ, make_connection_record, new_connection_id

REQUESTED_CAPABILITIES = (EngineCapability.SESSION_CREATION, EngineCapability.CONFIGURATION_OPTIONS)
FIXTURE_PATH = Path(__file__).resolve().parent / "fixtures/catalog/codex_acp_config_options.json"
FAKE_ENGINE_TEMPLATE = (Path(__file__).resolve().parent / "fake_engine_process.py").read_text(encoding="utf-8")


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _write_executable(path: Path, content: bytes = b"#!/bin/sh\nexit 0\n") -> None:
    path.write_bytes(content)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


def _observed_config_options():
    return json.loads(FIXTURE_PATH.read_text(encoding="utf-8"))["configOptions"]


def _write_fake_engine_adapter_entry(tmp_path: Path, scenario: dict) -> Path:
    """A standalone script with ``scenario`` embedded, no argv needed.

    Reuses ``FakeEngine``/its JSON-RPC helpers verbatim from
    ``fake_engine_process.py`` (everything above its ``def main()``), only
    replacing ``main()`` itself so the scenario is baked in rather than
    read from ``sys.argv[1]`` -- required because ``launch_command_for``
    always returns exactly ``(node_binary, adapter_entry)``, with no slot
    for a scenario-file argument.
    """
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
    """A synthetic ``CodexAcpInstall`` whose ``launch_command_for`` actually
    runs the generated fake engine adapter entry via this interpreter."""
    install_dir = tmp_path / "install"
    install_dir.mkdir()
    adapter_entry = _write_fake_engine_adapter_entry(install_dir, scenario)
    package_json = install_dir / "package.json"
    package_json.write_text(json.dumps({"name": "@agentclientprotocol/codex-acp", "version": "1.12.0"}), encoding="utf-8")
    bundled_codex_binary = install_dir / "codex"
    _write_executable(bundled_codex_binary)
    # codex_install.verify_install refuses any symlinked path component (see
    # its own docstring); resolve sys.executable up front rather than
    # passing e.g. a `python3` -> `python3.x` symlink straight through.
    resolved_node_binary = str(Path(sys.executable).resolve())
    return CodexAcpInstall(
        node_binary=resolved_node_binary,
        adapter_entry=str(adapter_entry),
        package_json=str(package_json),
        package_version="1.12.0",
        package_json_sha256=_sha256(package_json),
        adapter_entry_sha256=_sha256(adapter_entry),
        bundled_codex_version="0.45.0-fixture",
        bundled_codex_binary=str(bundled_codex_binary),
        bundled_codex_binary_sha256=_sha256(bundled_codex_binary),
    )


def _revision(model="model-a", supported_parameters=(("thought_level", "medium"),)):
    route = ExecutionRoute(
        engine="codex-acp", connection_id="cn_" + "0" * 12, model=model, supported_parameters=supported_parameters,
    )
    return ConfigurationRevision(id="cr_" + "0" * 12, task_id="t_" + "0" * 12, revision=1, route=route)


def test_build_codex_launch_returns_command_and_pinned_vars(tmp_path):
    scenario = dict(agent_capabilities={}, session_id="sess-1", config_options=_observed_config_options())
    install = _make_install(tmp_path, scenario)
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=None, environ=environ)
    launch_command, pinned_engine_vars = build_codex_launch(record, install, environ)
    assert launch_command == (install.node_binary, install.adapter_entry)
    assert pinned_engine_vars == {"CODEX_PATH": install.bundled_codex_binary, "NO_BROWSER": "1"}


def test_build_codex_launch_rejects_non_codex_engine(tmp_path):
    scenario = dict(agent_capabilities={}, session_id="sess-1", config_options=[])
    install = _make_install(tmp_path, scenario)
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=None, engine="claude-agent-acp", environ=environ)
    with pytest.raises(PortableError) as excinfo:
        build_codex_launch(record, install, environ)
    assert excinfo.value.code == "unsupported_engine"


def test_build_codex_launch_rejects_mismatched_existing_launch_command(tmp_path):
    scenario = dict(agent_capabilities={}, session_id="sess-1", config_options=[])
    install = _make_install(tmp_path, scenario)
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=("/usr/bin/other", "/other/entry.js"), environ=environ)
    with pytest.raises(PortableError) as excinfo:
        build_codex_launch(record, install, environ)
    assert excinfo.value.code == "engine_launch_command_mismatch"


def test_build_codex_launch_reverifies_and_rejects_drifted_install(tmp_path):
    scenario = dict(agent_capabilities={}, session_id="sess-1", config_options=[])
    install = _make_install(tmp_path, scenario)
    Path(install.adapter_entry).write_text("# tampered\n", encoding="utf-8")
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=None, environ=environ)
    with pytest.raises(PortableError) as excinfo:
        build_codex_launch(record, install, environ)
    assert excinfo.value.code == "engine_install_drifted"


def test_end_to_end_launch_pins_codex_path_and_no_browser_never_leaks_secrets(tmp_path):
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
    launch_command, _pinned = build_codex_launch(
        make_connection_record(connection_id, launch_command=None, environ=environ), install, environ,
    )
    record = make_connection_record(connection_id, launch_command=launch_command, environ=environ)
    manager = EngineSessionManager(
        record, str(tmp_path), environ=environ, option_mapping=codex_option_mapping, install=install, call_timeout=5.0,
    )
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        assert tuple(manager._process.process.args) == (install.node_binary, install.adapter_entry)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        evidence_ref = manager.configure(session_id, _revision())
        assert "model=model-a" in evidence_ref
        assert "reasoning_effort=medium" in evidence_ref
    finally:
        manager.shutdown(grace=2.0)

    observed_env_keys = set(json.loads(env_dump_path.read_text(encoding="utf-8")))
    assert "CODEX_PATH" in observed_env_keys
    assert "NO_BROWSER" in observed_env_keys
    assert "CODEX_HOME" in observed_env_keys  # this connection's own isolation var, unaffected by pinning
    for leaked_name in ("OPENAI_API_KEY", "CODEX_API_KEY", "CORTEX_CHANNEL_ID", "CORTEX_CHANNEL_SECRET", "CORTEX_CHANNEL_PROJECT_ROOT"):
        assert leaked_name not in observed_env_keys


def test_end_to_end_unsupported_configuration_never_reaches_the_wire(tmp_path):
    scenario = dict(
        agent_capabilities={}, session_id="sess-1", config_options=_observed_config_options(), configure_behavior="accept",
    )
    install = _make_install(tmp_path, scenario)
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    launch_command, _pinned = build_codex_launch(
        make_connection_record(connection_id, launch_command=None, environ=environ), install, environ,
    )
    record = make_connection_record(connection_id, launch_command=launch_command, environ=environ)
    manager = EngineSessionManager(
        record, str(tmp_path), environ=environ, option_mapping=codex_option_mapping, install=install, call_timeout=5.0,
    )
    try:
        from cortex_runtime.configuration.service import ConfigurationRejected

        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        with pytest.raises(ConfigurationRejected):
            manager.configure(session_id, _revision(model="not-a-real-model"))
    finally:
        manager.shutdown(grace=2.0)
