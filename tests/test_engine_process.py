"""Process launch, env isolation, and termination (engine.process.EngineProcess)."""
from __future__ import annotations

import json
import os
import time

import pytest

from cortex_runtime.engine.process import EngineProcess
from cortex_runtime.portable.errors import PortableError

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario


def test_refuses_missing_launch_command(tmp_path):
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=None, environ=environ)
    with pytest.raises(PortableError) as excinfo:
        EngineProcess(record, str(tmp_path), environ=environ)
    assert excinfo.value.code == "engine_launch_command_missing"


def test_refuses_relative_launch_command(tmp_path):
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=("relative/path",), environ=environ)
    with pytest.raises(PortableError) as excinfo:
        EngineProcess(record, str(tmp_path), environ=environ)
    assert excinfo.value.code == "engine_launch_command_relative"


def test_refuses_relative_cwd(tmp_path):
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    scenario_path = write_scenario(tmp_path, default_scenario())
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    with pytest.raises(PortableError) as excinfo:
        EngineProcess(record, "relative/cwd", environ=environ)
    assert excinfo.value.code == "engine_cwd_required"


def test_launch_env_is_isolated_and_dumped(tmp_path):
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    dump_path = tmp_path / "env-dump.json"
    scenario = default_scenario(env_dump_path=str(dump_path), prompt_behavior="crash")
    scenario_path = write_scenario(tmp_path, scenario)
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)

    process = EngineProcess(record, str(tmp_path), environ=environ)
    try:
        # Give the fake engine a moment to write its env dump at startup.
        deadline = time.monotonic() + 5.0
        while not dump_path.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        assert dump_path.exists()
        names = json.loads(dump_path.read_text())
        assert "CODEX_HOME" in names  # this connection's isolation variable
        assert "CORTEX_CHANNEL_SECRET" not in names  # never in the adapter env
        assert "ANTHROPIC_API_KEY" not in names
    finally:
        process.release(grace=2.0)


def test_launch_env_disables_python_bytecode_writes(tmp_path):
    """Every real engine child gets the setting needed for terminal tools."""
    environ = isolated_environ(tmp_path)
    values_path = tmp_path / "env-values.json"
    scenario = default_scenario(env_dump_values_path=str(values_path), prompt_behavior="crash")
    scenario_path = write_scenario(tmp_path, scenario)
    record = make_connection_record(new_connection_id(), launch_command=fake_launch_command(scenario_path), environ=environ)

    process = EngineProcess(record, str(tmp_path), environ=environ)
    try:
        deadline = time.monotonic() + 5.0
        while not values_path.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        assert values_path.exists()
        assert json.loads(values_path.read_text())["PYTHONDONTWRITEBYTECODE"] == "1"
    finally:
        process.release(grace=2.0)


def test_brokered_engine_launch_uses_exact_command_without_linux_sandbox(tmp_path):
    environ = isolated_environ(tmp_path)
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    scenario_path = write_scenario(tmp_path, default_scenario(prompt_behavior="hang"))
    record = make_connection_record(new_connection_id(),
                                    launch_command=fake_launch_command(scenario_path), environ=environ)

    process = EngineProcess(
        record, str(project), environ=environ, broker_mcp=True,
        project_root=str(project),
        channel_credentials=("ch_0123456789ab", "secret-token-123456", str(project)),
    )
    try:
        assert process.process.args == list(record.launch_command)
        assert process.mcp_broker is not None
        assert process.alive()
    finally:
        process.release(grace=2.0)


def test_terminate_reaps_and_frees_capacity_only_after_exit(tmp_path):
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    scenario = default_scenario(prompt_behavior="hang")
    scenario_path = write_scenario(tmp_path, scenario)
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)

    process = EngineProcess(record, str(tmp_path), environ=environ)
    assert process.alive()
    process.release(grace=3.0)
    assert not process.alive()
    assert process.wait_exited(0.0) is True
    # A stderr/observation log directory was created privately (0700) under
    # the isolated XDG_STATE_HOME, never the real user's state dir.
    assert process.logs.directory.stat().st_mode & 0o777 == 0o700
    assert (process.logs.directory / "stderr.log").exists()
    assert (process.logs.directory / "observations.jsonl").exists()


# --- F4 (T012 review) coverage -------------------------------------------

def test_terminate_kills_grandchild_via_process_group(tmp_path, monkeypatch):
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    pid_file = tmp_path / "grandchild.pid"
    scenario = default_scenario(prompt_behavior="hang", spawn_grandchild_pid_file=str(pid_file))
    scenario_path = write_scenario(tmp_path, scenario)
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    process = EngineProcess(record, str(tmp_path), environ=environ)
    try:
        deadline = time.monotonic() + 5.0
        while not pid_file.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        grandchild_pid = int(pid_file.read_text())
        os.kill(grandchild_pid, 0)  # still alive
        process.release(grace=3.0)
        time.sleep(0.2)
        with pytest.raises(ProcessLookupError):
            os.kill(grandchild_pid, 0)  # killed together with the process group
    finally:
        try:
            process.release(grace=1.0)
        except Exception:
            pass


def test_sigterm_ignored_escalates_to_sigkill(tmp_path, monkeypatch):
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    scenario = default_scenario(prompt_behavior="hang", ignore_sigterm=True)
    scenario_path = write_scenario(tmp_path, scenario)
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    process = EngineProcess(record, str(tmp_path), environ=environ)
    try:
        assert process.alive()
        # Give the child interpreter time to actually start up and install
        # its SIG_IGN handler before we race it with SIGTERM.
        time.sleep(0.3)
        started = time.monotonic()
        process.terminate(grace=0.4)
        elapsed = time.monotonic() - started
        assert process.wait_exited(3.0) is True
        assert not process.alive()
        # Escalated (took at least the grace period, not instant SIGTERM death).
        assert elapsed >= 0.35
    finally:
        process.release(grace=1.0)


def test_zombie_is_reaped_after_exit(tmp_path, monkeypatch):
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    scenario = default_scenario(prompt_behavior="crash")
    scenario_path = write_scenario(tmp_path, scenario)
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    process = EngineProcess(record, str(tmp_path), environ=environ)
    try:
        process.process.stdin.write(json.dumps(dict(jsonrpc="2.0", id=1, method="initialize", params=dict(protocolVersion=1, clientCapabilities={}))).encode() + b"\n")
        process.process.stdin.flush()
        session_line = json.dumps(dict(jsonrpc="2.0", id=2, method="session/prompt", params=dict(sessionId="sess-1", prompt=[dict(type="text", text="hi")])))
        process.process.stdin.write(session_line.encode() + b"\n")
        process.process.stdin.flush()
        assert process.wait_exited(5.0) is True
        pid = process.process.pid
        with pytest.raises(ProcessLookupError):
            os.kill(pid, 0)  # fully reaped, not a lingering zombie
    finally:
        process.release(grace=1.0)


def test_stderr_flood_is_bounded_and_child_not_blocked(tmp_path, monkeypatch):
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    scenario = default_scenario(prompt_behavior="hang", stderr_flood_bytes=2_000_000)
    scenario_path = write_scenario(tmp_path, scenario)
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    process = EngineProcess(record, str(tmp_path), environ=environ)
    try:
        deadline = time.monotonic() + 10.0
        stderr_path = process.logs.directory / "stderr.log"
        # The child must not block on a full pipe: our own drain thread
        # keeps consuming, so the file eventually reaches (and stays at)
        # the bounded cap well before the child could ever finish writing
        # 2MB synchronously into an unconsumed pipe.
        while stderr_path.stat().st_size < 200_000 and time.monotonic() < deadline:
            time.sleep(0.1)
        assert stderr_path.stat().st_size > 0
        time.sleep(0.5)
        from cortex_runtime.engine.logs import MAX_LOG_BYTES
        assert stderr_path.stat().st_size <= MAX_LOG_BYTES
    finally:
        process.release(grace=2.0)
