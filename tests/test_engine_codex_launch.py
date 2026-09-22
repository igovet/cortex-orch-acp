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
import os
import select
import stat
import subprocess
import sys
import time
from pathlib import Path

import pytest

from cortex_runtime.engine.codex_adapter_acp import build_codex_launch, codex_option_mapping
from cortex_runtime.engine.process import (
    _prepare_codex_runtime_wrapper, _prepare_codex_manual_review_adapter,
    _CODEX_AGENT_REVIEWER_SOURCE, _CODEX_AGENT_REVIEWER_MANUAL,
    _CODEX_NEW_SESSION_SOURCE, _CODEX_NEW_SESSION_PATCHED,
    _CODEX_SESSION_CONFIG_SOURCE, _CODEX_SESSION_CONFIG_PATCHED,
    _CODEX_MCP_DEDUP_SOURCE, _CODEX_MCP_DEDUP_PATCHED,
    _CODEX_MCP_ENTRY_SOURCE, _CODEX_MCP_ENTRY_PATCHED,
    _CODEX_SEND_PROMPT_SOURCE, _CODEX_SEND_PROMPT_PATCHED,
    _CODEX_RAW_RESPONSE_CASE_SOURCE, _CODEX_RESPONSE_ITEM_TYPES_SOURCE,
    _CODEX_HISTORY_CASE_BLOCK_SOURCE, _CODEX_UPDATE_EVENT_SOURCE,
    _CODEX_TURN_STARTED_SOURCE, _CODEX_ITEM_EVENT_SWITCH_SOURCE,
    _CODEX_COMPLETE_ITEM_EVENT_SWITCH_SOURCE,
    _CODEX_FUNCTION_CALL_SOURCE,
    _CODEX_CUSTOM_TOOL_HELPERS_INSERT, _CODEX_RAW_RESPONSE_METHOD_INSERT,
    _CODEX_RAW_RESPONSE_CASE_PATCHED, _CODEX_RESPONSE_ITEM_TYPES_PATCHED,
    _CODEX_HISTORY_CASE_BLOCK_PATCHED, _CODEX_TURN_STARTED_PATCHED,
    _CODEX_ITEM_EVENT_SWITCH_PATCHED, _CODEX_COMPLETE_ITEM_EVENT_SWITCH_PATCHED,
)
from cortex_runtime.engine.codex_install import CodexAcpInstall
from cortex_runtime.engine.session_manager import EngineSessionManager
from cortex_runtime.portable.engine_contract import EngineCapability
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.portable.identity import ConfigurationRevision, ExecutionRoute

from engine_fakes import isolated_environ, make_connection_record, new_connection_id


@pytest.fixture(autouse=True)
def synthetic_python_adapter(monkeypatch):
    """These fake Python adapters exercise launch and wire behavior; the
    manual-review bundle transformation has separate real-layout tests."""
    from cortex_runtime.engine import process
    monkeypatch.setattr(
        process, "_prepare_codex_manual_review_adapter",
        lambda install, connection_id, environ=None: install.adapter_entry,
    )

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


def test_runtime_wrapper_disables_native_agents_without_changing_codex_home(tmp_path):
    capture = tmp_path / "wrapper-args.txt"
    binary = tmp_path / "codex"
    _write_executable(binary, ("#!/usr/bin/python3\n" +
        "import json,sys\n" +
        "a=sys.argv[1:]\n" +
        "if a[-2:]==['features','list']: print('multi_agent stable false\\nmulti_agent_v2 stable false\\nplugins stable false')\n" +
        "elif a[-3:]==['mcp','list','--json']: print(json.dumps([{'name':'native','enabled':'mcp_servers.native.enabled=false' not in a}]))\n" +
        f"else: open({str(capture)!r},'w').write('\\n'.join(a))\n").encode())
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    native_home = tmp_path / "production-codex-home"
    native_home.mkdir(mode=0o700)
    wrapper = _prepare_codex_runtime_wrapper(str(binary), connection_id, environ)
    result = subprocess.run(
        [wrapper, "app-server"], check=False,
        env={"CODEX_HOME": str(native_home), "PATH": "/usr/bin:/bin"},
    )
    assert result.returncode == 0
    actual=capture.read_text().splitlines()
    assert actual[-3:]==['-c','mcp_servers.native.enabled=false','app-server']
    for setting in ['agents.enabled=false','features.multi_agent=false','features.multi_agent_v2=false',
                    'features.plugins=false','features.hooks=false','features.apps=false']:
        assert setting in actual
    # Native action tools stay the engine's own: nothing disables or rewrites them.
    assert not any(a.startswith(('features.shell_tool','web_search','model_catalog_json')) for a in actual)
    assert native_home.exists()
    assert Path(wrapper).is_file() and not Path(wrapper).is_symlink()
    assert stat.S_IMODE(Path(wrapper).stat().st_mode) & 0o777 == 0o700


@pytest.mark.parametrize("features", [
    "multi_agent stable true\nmulti_agent_v2 stable false\n",
    "multi_agent stable false\n",
])
def test_runtime_wrapper_refuses_unverified_native_agent_switches(tmp_path, features):
    capture = tmp_path / "launched"
    binary = tmp_path / "codex"
    _write_executable(binary, (
        "#!/bin/sh\n"
        'case "$*" in *"features list")\n'
        f"  printf '%b' {features!r}\n"
        "  exit 0\n"
        "esac\n"
        f"touch {str(capture)!r}\n"
    ).encode())
    wrapper = _prepare_codex_runtime_wrapper(str(binary), new_connection_id(), isolated_environ(tmp_path))
    result = subprocess.run([wrapper, "app-server"], check=False, capture_output=True, text=True)
    assert result.returncode == 65
    assert not capture.exists()


def test_installed_codex_app_server_reads_disabled_agents_and_features(tmp_path):
    """Opt-in, no-paid check against the actual pinned Codex binary and config parser.

    Supply CORTEX_PINNED_CODEX_BINARY with the verified bundle's binary path.
    App Server initialize/config-read do not start a model turn.
    """
    binary = os.environ.get("CORTEX_PINNED_CODEX_BINARY")
    if not binary:
        pytest.skip("set CORTEX_PINNED_CODEX_BINARY to check the installed App Server")
    wrapper = _prepare_codex_runtime_wrapper(binary, new_connection_id(), isolated_environ(tmp_path))
    proc = subprocess.Popen(
        [wrapper, "app-server"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL, text=True, bufsize=1,
    )
    try:
        assert proc.stdin is not None and proc.stdout is not None
        requests = (
            (1, "initialize", {"clientInfo": {"name": "cortex-no-paid-test", "version": "1"}, "capabilities": None}),
            (2, "config/read", {"includeLayers": False, "cwd": str(tmp_path)}),
        )
        for request_id, method, params in requests:
            proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}) + "\n")
            proc.stdin.flush()
            deadline = time.monotonic() + 10
            while True:
                ready, _, _ = select.select([proc.stdout], [], [], max(0, deadline - time.monotonic()))
                assert ready, f"No App Server response to {method}"
                response = json.loads(proc.stdout.readline())
                if response.get("id") == request_id:
                    break
            assert response.get("id") == request_id and "error" not in response
        config = response["result"]["config"]
        assert config["agents"]["enabled"] is False
        assert config["features"]["multi_agent"] is False
        assert config["features"]["multi_agent_v2"] is False
        assert config["features"]["plugins"] is False
        assert all(not entry.get("enabled", True) for entry in config.get("mcp_servers", {}).values())
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=3)


def test_pinned_private_codex_adapter_uses_manual_reviewer_and_preserves_original(tmp_path):
    package = tmp_path / "install" / "node_modules" / "@agentclientprotocol" / "codex-acp"
    dist = package / "dist"
    dist.mkdir(parents=True)
    original = dist / "index.js"
    source = (b"// pinned test bundle\n" + _CODEX_AGENT_REVIEWER_SOURCE + b"\n"
              + _CODEX_NEW_SESSION_SOURCE + b"\n" + _CODEX_SESSION_CONFIG_SOURCE
              + b"\n" + _CODEX_MCP_DEDUP_SOURCE
              + b"\n" + _CODEX_MCP_ENTRY_SOURCE
              + b"\n" + _CODEX_SEND_PROMPT_SOURCE
              + b"\n" + _CODEX_RAW_RESPONSE_CASE_SOURCE
              + b"\n" + _CODEX_RESPONSE_ITEM_TYPES_SOURCE
              + b"\n" + _CODEX_HISTORY_CASE_BLOCK_SOURCE
              + b"\n" + _CODEX_UPDATE_EVENT_SOURCE
              + b"\n" + _CODEX_TURN_STARTED_SOURCE
              + b"\n" + _CODEX_ITEM_EVENT_SWITCH_SOURCE
              + b"\n" + _CODEX_COMPLETE_ITEM_EVENT_SWITCH_SOURCE
              + b"\n" + _CODEX_FUNCTION_CALL_SOURCE)
    original.write_bytes(source)
    package_json = package / "package.json"
    package_json.write_text('{"version":"1.12.0","type":"module"}')
    from types import SimpleNamespace
    install = SimpleNamespace(
        package_version="1.12.0", adapter_entry=str(original),
        adapter_entry_sha256=hashlib.sha256(source).hexdigest(), package_json=str(package_json),
    )
    environ = isolated_environ(tmp_path)
    copied = Path(_prepare_codex_manual_review_adapter(install, new_connection_id(), environ))
    assert original.read_bytes() == source
    expected = source.replace(
        _CODEX_AGENT_REVIEWER_SOURCE, _CODEX_AGENT_REVIEWER_MANUAL,
    ).replace(_CODEX_NEW_SESSION_SOURCE, _CODEX_NEW_SESSION_PATCHED).replace(
        _CODEX_SESSION_CONFIG_SOURCE, _CODEX_SESSION_CONFIG_PATCHED,
    ).replace(_CODEX_MCP_DEDUP_SOURCE, _CODEX_MCP_DEDUP_PATCHED,
    ).replace(_CODEX_MCP_ENTRY_SOURCE, _CODEX_MCP_ENTRY_PATCHED,
    ).replace(_CODEX_SEND_PROMPT_SOURCE, _CODEX_SEND_PROMPT_PATCHED).replace(
        _CODEX_FUNCTION_CALL_SOURCE, _CODEX_CUSTOM_TOOL_HELPERS_INSERT + _CODEX_FUNCTION_CALL_SOURCE,
    ).replace(_CODEX_UPDATE_EVENT_SOURCE, _CODEX_RAW_RESPONSE_METHOD_INSERT + _CODEX_UPDATE_EVENT_SOURCE).replace(
        _CODEX_RAW_RESPONSE_CASE_SOURCE, _CODEX_RAW_RESPONSE_CASE_PATCHED,
    ).replace(_CODEX_TURN_STARTED_SOURCE, _CODEX_TURN_STARTED_PATCHED).replace(
        _CODEX_ITEM_EVENT_SWITCH_SOURCE, _CODEX_ITEM_EVENT_SWITCH_PATCHED,
    ).replace(_CODEX_COMPLETE_ITEM_EVENT_SWITCH_SOURCE, _CODEX_COMPLETE_ITEM_EVENT_SWITCH_PATCHED).replace(
        _CODEX_RESPONSE_ITEM_TYPES_SOURCE, _CODEX_RESPONSE_ITEM_TYPES_PATCHED).replace(
        _CODEX_HISTORY_CASE_BLOCK_SOURCE, _CODEX_HISTORY_CASE_BLOCK_PATCHED,
    )
    assert copied.read_bytes() == expected
    assert copied.parent.parent.joinpath("node_modules").resolve() == package.parents[1].resolve()
    assert stat.S_IMODE(copied.stat().st_mode) == 0o600
    assert stat.S_IMODE(copied.parent.stat().st_mode) == 0o700


def test_private_codex_adapter_rejects_unknown_source_before_launch(tmp_path):
    package = tmp_path / "install" / "node_modules" / "@agentclientprotocol" / "codex-acp"
    dist = package / "dist"
    dist.mkdir(parents=True)
    original = dist / "index.js"
    source = b"// unexpected mode table"
    original.write_bytes(source)
    package_json = package / "package.json"
    package_json.write_text('{"version":"1.12.0","type":"module"}')
    from types import SimpleNamespace
    install = SimpleNamespace(
        package_version="1.12.0", adapter_entry=str(original),
        adapter_entry_sha256=hashlib.sha256(source).hexdigest(), package_json=str(package_json),
    )
    with pytest.raises(PortableError) as excinfo:
        _prepare_codex_manual_review_adapter(install, new_connection_id(), isolated_environ(tmp_path))
    assert excinfo.value.code == "codex_manual_review_adapter_unsupported"


def test_private_codex_adapter_rejects_missing_session_config_guard(tmp_path):
    package = tmp_path / "install" / "node_modules" / "@agentclientprotocol" / "codex-acp"
    dist = package / "dist"
    dist.mkdir(parents=True)
    original = dist / "index.js"
    source = _CODEX_AGENT_REVIEWER_SOURCE + b"\n" + _CODEX_NEW_SESSION_SOURCE
    original.write_bytes(source)
    package_json = package / "package.json"
    package_json.write_text('{"version":"1.12.0","type":"module"}')
    from types import SimpleNamespace
    install = SimpleNamespace(
        package_version="1.12.0", adapter_entry=str(original),
        adapter_entry_sha256=hashlib.sha256(source).hexdigest(), package_json=str(package_json),
    )
    with pytest.raises(PortableError) as excinfo:
        _prepare_codex_manual_review_adapter(install, new_connection_id(), isolated_environ(tmp_path))
    assert excinfo.value.code == "codex_manual_review_adapter_unsupported"


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
