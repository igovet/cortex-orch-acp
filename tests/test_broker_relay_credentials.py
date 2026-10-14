"""The MCP relay credential cannot be read from argv or reused across jails.

Regression coverage for the channel-relay credential boundary: the relay's
handshake token reaches the relay only through a 0600 file in the broker's
private 0700 directory (never the relay entry's args or env, which an engine
may put on a world-readable command line), the broker compares it in
constant time, an unbound broker accepts it once, and a bound broker accepts
it only from its engine's own process tree.
"""
from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
import threading
import time

import pytest

from cortex_runtime.engine import broker as broker_module
from cortex_runtime.engine.broker import McpBroker, relay_token_path
from cortex_runtime.engine.mcp_bridge import build_mcp_servers, host_mcp_servers, relay_launch

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, \
    new_connection_id, write_scenario

CHANNEL = ("ch_0123456789ab", "fake-secret-123456")
ECHO_SERVER = "import sys\nfor line in sys.stdin:\n print(line, end='', flush=True)\n"


def _broker(tmp_path, events, **kwargs):
    script = tmp_path / "echo_server.py"
    script.write_text(ECHO_SERVER, encoding="utf-8")
    return McpBroker((*CHANNEL, str(tmp_path)), python_executable=sys.executable,
                     server_script=str(script),
                     observe=lambda event, **fields: events.append((event, fields)), **kwargs)


def _relay(endpoint, *, launcher=None):
    """Start the production relay exactly as an engine would from its entry:
    only the entry's args and env, with no other environment at all."""
    args, env = relay_launch(endpoint)
    environment = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"),
                   **{item["name"]: item["value"] for item in env}}
    argv = [sys.executable, *args]
    if launcher is not None:
        argv = [*launcher, *argv]
    return subprocess.Popen(argv, env=environment, stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)


def _forged_relay(broker, token):
    """A relay for ``broker`` whose token file holds ``token`` instead."""
    real = relay_token_path(broker.endpoint)
    with open(real, encoding="ascii") as handle:
        saved = handle.read()
    with open(real, "w", encoding="ascii") as handle:
        handle.write(token)
    try:
        proc = _relay(broker.endpoint)
        return _round_trip(proc)
    finally:
        with open(real, "w", encoding="ascii") as handle:
            handle.write(saved)


def _round_trip(proc):
    output, _ = proc.communicate(b'{"jsonrpc":"2.0","id":1}\n', timeout=10)
    return output


def _wait_for(predicate, seconds=5.0):
    deadline = time.monotonic() + seconds
    while not predicate() and time.monotonic() < deadline:
        time.sleep(0.02)
    return predicate()


def _rejections(events):
    return [fields.get("cause") for event, fields in events
            if event == "mcp_broker_failure" and fields.get("reason") == "relay_handshake_rejected"]


def _servers_started(events):
    return sum(1 for event, _ in events if event == "mcp_server_started")


def test_relay_entry_carries_no_secret_in_args_or_env():
    entry = build_mcp_servers(*CHANNEL, "/tmp", relay_endpoint="/run/cortex.sock")[0]
    assert entry["args"][-2:] == ["--relay", "/run/cortex.sock"]
    assert entry["env"] == []
    # Channel credentials never leave the broker for a relay entry.
    assert CHANNEL[1] not in json.dumps(entry)
    with pytest.raises(ValueError):
        relay_launch("relative.sock")
    with pytest.raises(TypeError):
        build_mcp_servers(*CHANNEL, "/tmp", relay_endpoint="/run/cortex.sock", relay_token="x")


def test_token_file_is_private_and_removed_with_the_broker(tmp_path):
    broker = _broker(tmp_path, [])
    path = relay_token_path(broker.endpoint)
    try:
        info = os.lstat(path)
        assert stat.S_ISREG(info.st_mode)
        assert stat.S_IMODE(info.st_mode) == 0o600
        assert info.st_uid == os.geteuid()
        assert stat.S_IMODE(os.lstat(os.path.dirname(path)).st_mode) == 0o700
        with open(path, encoding="ascii") as handle:
            assert handle.read() == broker.relay_token
    finally:
        broker.close()
    assert not os.path.exists(path)


def test_live_relay_command_line_and_environment_do_not_contain_the_token(tmp_path):
    events = []
    broker = _broker(tmp_path, events)
    try:
        proc = _relay(broker.endpoint)
        try:
            proc.stdin.write(b'{"jsonrpc":"2.0","id":1}\n')
            proc.stdin.flush()
            assert proc.stdout.readline() == b'{"jsonrpc":"2.0","id":1}\n'
            # /proc/<pid>/cmdline is readable by every process on the host.
            with open(f"/proc/{proc.pid}/cmdline", "rb") as handle:
                cmdline = handle.read()
            with open(f"/proc/{proc.pid}/environ", "rb") as handle:
                environ = handle.read()
            assert broker.relay_token.encode() not in cmdline
            assert broker.relay_token.encode() not in environ
            assert b"--relay" in cmdline
        finally:
            proc.stdin.close()
            proc.wait(timeout=5)
        assert proc.returncode == 0
    finally:
        broker.close()


def test_relay_refuses_to_start_without_its_token_file_or_with_an_argv_token(tmp_path):
    env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin")}
    missing = subprocess.run([sys.executable, "-B", broker_module.__file__, "--relay",
                              str(tmp_path / "none.sock")], env=env, capture_output=True, timeout=10)
    assert missing.returncode == 2
    assert b"cortex_mcp_relay_token_unavailable" in missing.stderr
    # A token in the environment is not a credential source.
    from_env = subprocess.run([sys.executable, "-B", broker_module.__file__, "--relay",
                               str(tmp_path / "none.sock")],
                              env={**env, "CORTEX_MCP_RELAY_TOKEN": "ctoken"}, capture_output=True, timeout=10)
    assert from_env.returncode == 2
    assert b"cortex_mcp_relay_token_unavailable" in from_env.stderr
    # The former argv form is not accepted at all.
    legacy = subprocess.run([sys.executable, "-B", broker_module.__file__, "--relay", "/run/none.sock",
                             "ctoken"], env=env, capture_output=True, timeout=10)
    assert legacy.returncode == 2
    relative = subprocess.run([sys.executable, "-B", broker_module.__file__, "--relay", "none.sock"],
                              env=env, capture_output=True, timeout=10)
    assert relative.returncode == 2
    assert b"cortex_mcp_relay_invalid_arguments" in relative.stderr


def test_relay_never_follows_a_symlinked_token_file(tmp_path):
    directory = tmp_path / "sockets"
    directory.mkdir(mode=0o700)
    target = tmp_path / "elsewhere.token"
    target.write_text("ctoken", encoding="ascii")
    os.symlink(target, directory / "relay.token")
    result = subprocess.run([sys.executable, "-B", broker_module.__file__, "--relay",
                             str(directory / "broker.sock")],
                            env={"PATH": os.environ.get("PATH", "/usr/bin:/bin")},
                            capture_output=True, timeout=10)
    assert result.returncode == 2
    assert b"cortex_mcp_relay_token_unavailable" in result.stderr


def test_unbound_broker_rejects_a_replayed_token(tmp_path):
    events = []
    broker = _broker(tmp_path, events)
    try:
        assert _round_trip(_relay(broker.endpoint)) == b'{"jsonrpc":"2.0","id":1}\n'
        replay = _relay(broker.endpoint)
        assert _round_trip(replay) == b""
        assert _wait_for(lambda: _rejections(events) == ["token_replayed"])
        # A refused handshake never starts a credentialed server.
        assert _servers_started(events) == 1
    finally:
        broker.close()


def test_wrong_token_is_rejected_without_starting_a_server(tmp_path):
    events = []
    broker = _broker(tmp_path, events)
    try:
        assert _forged_relay(broker, "c" + "x" * 43) == b""
        assert _wait_for(lambda: _rejections(events) == ["token"])
        assert _servers_started(events) == 0
        # A failed guess does not consume the real token.
        assert _round_trip(_relay(broker.endpoint)) == b'{"jsonrpc":"2.0","id":1}\n'
    finally:
        broker.close()


def test_bound_broker_accepts_its_engine_tree_repeatedly_and_refuses_others(tmp_path):
    events = []
    # Bound to this test process: relays it starts (directly or through an
    # intermediate shell) are its descendants, like an engine's MCP children.
    broker = _broker(tmp_path, events, peer_root=os.getpid())
    try:
        # Relaunching the same entry (an engine's MCP restart) keeps working:
        # the token file is not consumed by a first read.
        for _ in range(2):
            assert _round_trip(_relay(broker.endpoint)) == b'{"jsonrpc":"2.0","id":1}\n'
        # No exec: the shell stays alive between this process and the relay.
        nested = _relay(broker.endpoint, launcher=["/bin/sh", "-c", '"$@"; exit $?', "sh"])
        assert _round_trip(nested) == b'{"jsonrpc":"2.0","id":1}\n'
        assert _servers_started(events) == 3
        assert _rejections(events) == []
    finally:
        broker.close()


def test_correct_token_from_outside_the_engine_tree_is_rejected(tmp_path):
    events = []
    other = subprocess.Popen(["sleep", "30"])
    try:
        # The bound "engine" is an unrelated process; this test process (and
        # therefore its relay) is not inside that tree.
        broker = _broker(tmp_path, events, peer_root=other.pid)
        try:
            assert _round_trip(_relay(broker.endpoint)) == b""
            assert _wait_for(lambda: _rejections(events) == ["peer_process"])
            assert _servers_started(events) == 0
        finally:
            broker.close()
    finally:
        other.kill()
        other.wait(timeout=5)


def test_handshake_uses_a_constant_time_comparison(tmp_path, monkeypatch):
    calls = []
    real = broker_module.hmac.compare_digest

    def spy(left, right):
        calls.append((type(left), type(right)))
        return real(left, right)

    monkeypatch.setattr(broker_module.hmac, "compare_digest", spy)
    events = []
    broker = _broker(tmp_path, events)
    try:
        assert _forged_relay(broker, "c" + "y" * 43) == b""
        assert _round_trip(_relay(broker.endpoint)) == b'{"jsonrpc":"2.0","id":1}\n'
    finally:
        broker.close()
    assert calls == [(bytes, bytes), (bytes, bytes)]


def _memory_broker(peer_root=None):
    broker = object.__new__(McpBroker)
    broker._state_lock = threading.RLock()
    broker._token = "ctoken-value"
    broker._peer_root = peer_root
    broker._token_consumed = False
    return broker


def test_a_peer_of_another_user_is_rejected_even_with_the_token(monkeypatch):
    broker = _memory_broker()
    monkeypatch.setattr(broker_module, "_peer_credentials", lambda conn: (4242, os.geteuid() + 1, 0))
    assert broker._authenticate(None, b"ctoken-value") == "peer_user"
    assert broker._token_consumed is False
    monkeypatch.setattr(broker_module, "_peer_credentials", lambda conn: (4242, os.geteuid(), 0))
    assert broker._authenticate(None, b"ctoken-value") is None
    assert broker._authenticate(None, b"ctoken-value") == "token_replayed"


def test_bound_peer_still_needs_the_token(monkeypatch):
    broker = _memory_broker(peer_root=os.getpid())
    monkeypatch.setattr(broker_module, "_peer_credentials", lambda conn: (os.getpid(), os.geteuid(), 0))
    assert broker._authenticate(None, b"wrong") == "token"
    assert broker._authenticate(None, b"ctoken-value") is None
    assert broker._authenticate(None, b"ctoken-value") is None
    # A pid that cannot be resolved fails closed.
    monkeypatch.setattr(broker_module, "_peer_credentials", lambda conn: (0, os.geteuid(), 0))
    assert broker._authenticate(None, b"ctoken-value") == "peer_process"


@pytest.mark.parametrize("pid", [0, 1, -5, True, "123", None])
def test_peer_root_must_be_a_real_process(tmp_path, pid):
    broker = _broker(tmp_path, [])
    try:
        with pytest.raises(ValueError):
            broker.bind_peer_root(pid)
    finally:
        broker.close()


def test_each_broker_socket_lives_in_its_own_private_directory(tmp_path):
    first = _broker(tmp_path, [])
    second = _broker(tmp_path, [], socket_dir=None)
    try:
        directories = {os.path.dirname(first.endpoint), os.path.dirname(second.endpoint)}
        assert len(directories) == 2
        for directory in directories:
            info = os.lstat(directory)
            assert stat.S_ISDIR(info.st_mode)
            assert stat.S_IMODE(info.st_mode) == 0o700
            assert info.st_uid == os.geteuid()
        assert stat.S_IMODE(os.stat(first.endpoint).st_mode) == 0o600
    finally:
        first.close()
        second.close()
    assert not any(os.path.exists(directory) for directory in directories)


def test_engine_process_binds_its_brokers_to_the_launched_engine(tmp_path):
    from cortex_runtime.engine.process import EngineProcess

    environ = isolated_environ(tmp_path)
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    scenario_path = write_scenario(tmp_path, default_scenario(prompt_behavior="hang"))
    record = make_connection_record(new_connection_id(), launch_command=fake_launch_command(scenario_path),
                                    environ=environ)
    process = EngineProcess(record, str(project), environ=environ, broker_mcp=True,
                            project_root=str(project), channel_credentials=(*CHANNEL, str(project)))
    try:
        assert process.mcp_broker._peer_root == process.process.pid
        entry = process.broker_external_mcp((*CHANNEL, str(project)),
                                            dict(name="fixture", command="/usr/bin/true", allowed_tools=["a"]))
        external = process.external_mcp_brokers[-1]
        assert external._peer_root == process.process.pid
        assert external.relay_token not in json.dumps(entry)
        assert entry["env"] == []
    finally:
        process.release(grace=2.0)


@pytest.mark.parametrize("engine", ["claude-agent-acp", "codex-acp"])
def test_session_new_wire_carries_no_relay_token(tmp_path, engine):
    """The ``session/new`` the engine actually receives, through the real
    session manager with a brokered Cortex server and an external server."""
    from cortex_runtime.engine.session_manager import EngineSessionManager
    from cortex_runtime.portable.engine_contract import EngineCapability

    environ = isolated_environ(tmp_path)
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    dump_path = tmp_path / "session-new.json"
    scenario = write_scenario(tmp_path, default_scenario(session_new_params_dump_path=str(dump_path)))
    record = make_connection_record(new_connection_id(), launch_command=fake_launch_command(scenario),
                                    engine=engine, environ=environ)
    manager = EngineSessionManager(
        record, str(project), environ=environ, broker_mcp=True,
        channel_credentials=(*CHANNEL, str(project)),
        host_mcp_servers=[{"name": "code-memory", "allowed_tools": ["inspect"], "command": "/usr/bin/true",
                           "env": [{"name": "MEMORY_TOKEN", "value": "external-secret-value"}]}],
        mcp_role="worker", call_timeout=5,
    )
    try:
        manager.negotiate((EngineCapability.SESSION_CREATION, EngineCapability.PROMPT_STREAMING))
        manager.create_session("t_" + "a" * 12, "xr_" + "0" * 24)
        wire = dump_path.read_text(encoding="utf-8")
        tokens = [manager._process.mcp_broker.relay_token,
                  *(broker.relay_token for broker in manager._process.external_mcp_brokers)]
    finally:
        manager.shutdown(grace=2)
    servers = json.loads(wire)["mcpServers"]
    assert [server["name"] for server in servers] == ["cortex", "code-memory"]
    assert len(tokens) == 2 and all(tokens)
    argv = "\0".join(_claude_cli_argv(servers))
    for secret in (*tokens, CHANNEL[1], "external-secret-value"):
        assert secret not in wire
        assert secret not in argv
    assert all(server["env"] == [] for server in servers)


def _claude_cli_argv(servers):
    """The ``claude`` CLI argv the installed Claude Agent SDK builds from the
    ACP ``mcpServers`` that claude-agent-acp receives: claude-agent-acp maps
    each stdio entry to ``{type, command, args, env: fromEntries(env)}`` and
    the SDK pushes ``--mcp-config <JSON.stringify({mcpServers})>`` onto the
    world-readable command line."""
    config = {server["name"]: {"type": "stdio", "command": server["command"], "args": server["args"],
                               "env": {item["name"]: item["value"] for item in server["env"]}}
              for server in servers}
    return ["claude", "--mcp-config", json.dumps({"mcpServers": config})]


@pytest.mark.parametrize("engine", ["claude-agent-acp", "codex-acp", "opencode"])
def test_no_relay_token_reaches_any_engine_wire_entry(tmp_path, engine):
    """Every brokered ``mcpServers`` entry an engine receives (Cortex's own and
    each external server) is free of every broker token and channel secret,
    in args and env alike, for every engine; and so is the ``claude`` CLI
    command line derived from it."""
    from cortex_runtime.engine.process import EngineProcess

    environ = isolated_environ(tmp_path)
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    scenario_path = write_scenario(tmp_path, default_scenario(prompt_behavior="hang"))
    # The launch path is engine-neutral below the session manager; the
    # engine identity selects which external servers are forwarded.
    record = make_connection_record(new_connection_id(), launch_command=fake_launch_command(scenario_path),
                                    environ=environ)
    credentials = (*CHANNEL, str(project))
    process = EngineProcess(record, str(project), environ=environ, broker_mcp=True,
                            project_root=str(project), channel_credentials=credentials)
    try:
        external = host_mcp_servers(
            [{"name": "code-memory", "command": "/usr/bin/true", "allowed_tools": ["a"],
              "env": [{"name": "MEMORY_TOKEN", "value": "external-secret-value"}]}],
            role="worker", engine=engine)
        assert [entry["name"] for entry in external] == ["code-memory"]
        servers = build_mcp_servers(*credentials, relay_endpoint=process.mcp_broker.endpoint) + [
            process.broker_external_mcp(credentials, server) for server in external]
        tokens = [process.mcp_broker.relay_token,
                  *(broker.relay_token for broker in process.external_mcp_brokers)]
        assert len(tokens) == 2 and all(tokens)
        wire = json.dumps(servers)
        argv = "\0".join(_claude_cli_argv(servers))
        for secret in (*tokens, CHANNEL[1], "external-secret-value"):
            assert secret not in wire
            assert secret not in argv
        for server in servers:
            assert server["env"] == []
            assert server["args"][-2] == "--relay"
        # Each relay finds its own broker's token beside its socket.
        for server, broker in zip(servers, [process.mcp_broker, *process.external_mcp_brokers]):
            assert broker_module.read_relay_token(server["args"][-1]) == broker.relay_token
        assert [os.path.dirname(server["args"][-1]) for server in servers] == [
            os.path.dirname(process.mcp_broker.endpoint),
            *(os.path.dirname(broker.endpoint) for broker in process.external_mcp_brokers)]
    finally:
        process.release(grace=2.0)
