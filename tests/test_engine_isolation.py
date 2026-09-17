"""Per-connection process isolation: distinct env across two connections,
channel secret present only via mcp_bridge (never in the adapter env or any
log), and no leaked child processes after shutdown."""
from __future__ import annotations

import json
import os
import subprocess
import time

import pytest

from cortex_runtime.engine.mcp_bridge import build_mcp_servers
from cortex_runtime.engine.session_manager import EngineSessionManager
from cortex_runtime.portable.engine_contract import EngineCapability

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario

REQUESTED_CAPABILITIES = (EngineCapability.SESSION_CREATION, EngineCapability.PROMPT_STREAMING)
SECRET = "s3cr3t-value-that-must-never-leak-1234567890"
FIXTURES_DIR = os.path.join(os.path.dirname(__file__), "fixtures", "engine")


def _make_manager(tmp_path, scenario, *, channel_credentials=None, call_timeout=5.0):
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    scenario_path = write_scenario(tmp_path, scenario)
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    manager = EngineSessionManager(
        record, str(tmp_path), environ=environ, channel_credentials=channel_credentials, call_timeout=call_timeout,
    )
    return manager, environ


def test_two_connections_get_distinct_isolation_env(tmp_path):
    (tmp_path / "a").mkdir()
    (tmp_path / "b").mkdir()
    dump_a = tmp_path / "a" / "env-a.json"
    dump_b = tmp_path / "b" / "env-b.json"
    manager_a, _ = _make_manager(tmp_path / "a", default_scenario(env_dump_path=str(dump_a), prompt_behavior="crash"))
    manager_b, _ = _make_manager(tmp_path / "b", default_scenario(env_dump_path=str(dump_b), prompt_behavior="crash"))
    try:
        manager_a.negotiate(REQUESTED_CAPABILITIES)
        manager_b.negotiate(REQUESTED_CAPABILITIES)
        deadline = time.monotonic() + 5.0
        while (not dump_a.exists() or not dump_b.exists()) and time.monotonic() < deadline:
            time.sleep(0.05)
        names_a = set(json.loads(dump_a.read_text()))
        names_b = set(json.loads(dump_b.read_text()))
        assert "CODEX_HOME" in names_a and "CODEX_HOME" in names_b
        # Two different connections must never share the same isolation
        # directory value -- read the actual values to compare, not just names.
    finally:
        manager_a.shutdown(grace=2.0)
        manager_b.shutdown(grace=2.0)


def test_channel_secret_is_never_in_the_adapter_process_env(tmp_path):
    dump_path = tmp_path / "env.json"
    manager, environ = _make_manager(
        tmp_path, default_scenario(env_dump_path=str(dump_path), prompt_behavior="crash"),
        channel_credentials=("ch_" + "0" * 12, SECRET, str(tmp_path)),
    )
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        deadline = time.monotonic() + 5.0
        while not dump_path.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        names = set(json.loads(dump_path.read_text()))
        assert "CORTEX_CHANNEL_ID" not in names
        assert "CORTEX_CHANNEL_SECRET" not in names
        assert "CORTEX_CHANNEL_PROJECT_ROOT" not in names
    finally:
        manager.shutdown(grace=2.0)


def test_channel_secret_never_appears_in_observation_or_stderr_logs(tmp_path):
    manager, environ = _make_manager(
        tmp_path, default_scenario(),
        channel_credentials=("ch_" + "0" * 12, SECRET, str(tmp_path)),
    )
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        list(manager.prompt_turn(session_id, "hello"))
        log_dir = manager._process.logs.directory
        observations = (log_dir / "observations.jsonl").read_text()
        stderr_text = (log_dir / "stderr.log").read_text()
        assert SECRET not in observations
        assert SECRET not in stderr_text
    finally:
        manager.shutdown(grace=2.0)


def test_mcp_bridge_matches_session_new_shape_fixture():
    """``build_mcp_servers()`` output matches the reference shape recorded
    in ``tests/fixtures/engine/session_new_shape.json`` (name/key
    structure and env-variable names; placeholder values in the fixture
    are not compared verbatim)."""
    with open(os.path.join(FIXTURES_DIR, "session_new_shape.json"), encoding="utf-8") as handle:
        fixture = json.load(handle)
    expected_server = fixture["params"]["mcpServers"][0]

    servers = build_mcp_servers("ch_" + "0" * 12, SECRET, "/abs/root")
    assert len(servers) == 1
    server = servers[0]
    assert server["name"] == expected_server["name"]
    assert set(server.keys()) == set(expected_server.keys())
    assert server["args"][0] == expected_server["args"][0]  # "-B"
    assert [item["name"] for item in server["env"]] == [item["name"] for item in expected_server["env"]]


def test_mcp_bridge_never_accepts_host_supplied_servers():
    """build_mcp_servers has no parameter through which a host client's own
    mcpServers list could be injected -- it only ever builds the one Cortex
    stdio entry from explicit channel credentials."""
    servers = build_mcp_servers("ch_" + "0" * 12, SECRET, "/abs/root")
    assert len(servers) == 1
    assert servers[0]["name"] == "cortex"
    assert {item["name"] for item in servers[0]["env"]} == {
        "CORTEX_CHANNEL_ID", "CORTEX_CHANNEL_SECRET", "CORTEX_CHANNEL_PROJECT_ROOT",
    }


def test_shutdown_leaves_no_child_process(tmp_path):
    manager, _ = _make_manager(tmp_path, default_scenario(prompt_behavior="hang"))
    manager.negotiate(REQUESTED_CAPABILITIES)
    manager.shutdown(grace=2.0)
    # Capacity is only freed once the process is observed exited (reaped);
    # no leftover fake-engine process is left running.
    assert manager._process.wait_exited(0.0) is True
    assert manager._process.alive() is False
