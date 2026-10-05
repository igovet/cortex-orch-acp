"""No paid engine or Paseo: exercise real stdio, fake engine and sandbox."""
from __future__ import annotations

import importlib.util
import io
import json
import multiprocessing
import os
from pathlib import Path
import stat
import subprocess
import sys
import threading
from types import SimpleNamespace

import pytest

from cortex_runtime.acp.transport import EOF, StdioTransport
from cortex_runtime.observability import wire_trace as wire
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

ROOT = Path(__file__).resolve().parents[1]


def module(path, name):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


@pytest.fixture
def trace(tmp_path, monkeypatch):
    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    directory = home / ".paseo-dev/cortex-xdg/state/cortex/wire-traces"
    tracer = wire.WireTrace(directory, {"DEMO_API_KEY": "environment-secret"})
    monkeypatch.setattr(wire, "_tracer", tracer)
    return tracer


def rows(trace):
    return [json.loads(line) for path in sorted(trace.directory.glob("*.jsonl"))
            for line in path.read_text().splitlines()]


def dev_env(home, root=ROOT):
    dev = home / ".paseo-dev"
    return {wire.OPT_IN: "1", wire.HOME_MARKER: str(dev),
            "CORTEX_PASEO_SOURCE_ROOT": str(root),
            "XDG_CONFIG_HOME": str(dev / "cortex-xdg/config"),
            "XDG_STATE_HOME": str(dev / "cortex-xdg/state")}


def test_disabled_never_checks_paths_or_creates_files(tmp_path, monkeypatch):
    monkeypatch.setattr(wire, "_gate", lambda *args: pytest.fail("disabled gate accessed filesystem"))
    assert wire.configure({}) is None
    output = io.BytesIO()
    transport = StdioTransport(lambda *args: None, output, io.BytesIO())
    transport.start()
    transport.join_reader(2)
    assert transport.send_response({"jsonrpc": "2.0", "id": 1, "result": {}})
    transport.stop_writer()
    assert list(tmp_path.iterdir()) == []


@pytest.mark.parametrize("production", ["payload", "state", "source_mismatch", "dev_symlink", "git_trace"])
def test_gate_refuses_production_and_unsafe_optin(tmp_path, monkeypatch, production):
    home = tmp_path / "home"
    home.mkdir()
    root = tmp_path / "source"
    (root / ".git").mkdir(parents=True)
    (root / "scripts").mkdir()
    (root / "scripts/cortex_paseo_dev.py").write_text("source marker")
    env = dev_env(home, root)
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: home))
    if production == "payload":
        root = home / ".local/state/cortex/paseo-runtime/1.0.0+sha256.test"
        root.mkdir(parents=True)
        env = dev_env(home, root)
    elif production == "state":
        env["XDG_STATE_HOME"] = str(home / ".local/state")
    elif production == "source_mismatch":
        env["CORTEX_PASEO_SOURCE_ROOT"] = str(tmp_path / "elsewhere")
    elif production == "dev_symlink":
        target = tmp_path / "other"
        target.mkdir()
        (home / ".paseo-dev").symlink_to(target, target_is_directory=True)
    else:
        (home / ".git").mkdir()
    with pytest.raises(ValueError, match="dev_wire_trace_refused"):
        wire._gate(env, root)
    assert not (home / ".paseo-dev/cortex-xdg/state/cortex/wire-traces").exists()


def test_guarded_wrapper_is_only_explicit_start_optin(tmp_path, monkeypatch):
    wrapper = module("scripts/cortex_paseo_dev.py", "trace_wrapper")
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    monkeypatch.setenv(wire.OPT_IN, "1")
    monkeypatch.setenv(wire.HOME_MARKER, "inherited")
    dev = tmp_path / ".paseo-dev"
    assert wire.OPT_IN not in wrapper.isolated_environment(dev)
    seen = []
    def run(argv, **kwargs):
        seen.append((argv, kwargs["env"]))
        return SimpleNamespace(returncode=0)
    monkeypatch.setattr(wrapper.subprocess, "run", run)
    args = SimpleNamespace(home=str(dev), paseo="/bin/true", check=False,
                           wire_trace=True, command=["--", "start", "--json"])
    assert wrapper._run_cli(args) == 0
    assert seen[0][1][wire.OPT_IN] == "1"
    assert seen[0][1][wire.HOME_MARKER] == str(dev)
    assert wire._gate(seen[0][1], ROOT) == dev / "cortex-xdg/state/cortex/wire-traces"
    for command, home in [(["status"], dev), (["daemon", "stop"], dev),
                          (["start"], tmp_path / ".paseo"), (["start"], tmp_path / "alternate")]:
        args.command, args.home = command, str(home)
        assert wrapper._run_cli(args) == 2
    assert len(seen) == 1


def test_upstream_stdio_captures_actual_both_directions_and_identity(trace):
    incoming = dict(jsonrpc="2.0", id=5, method="session/prompt", params={"sessionId": "acps_test"})
    output = io.BytesIO()
    transport = None
    def handler(message, error):
        if message is not EOF:
            assert error is None
            transport.send_response(dict(jsonrpc="2.0", id=message["id"], result={"ok": True}))
    transport = StdioTransport(handler, output, io.BytesIO((json.dumps(incoming) + "\n").encode()),
                               trace_context=lambda session: {"task_id": "t_test"} if session else {})
    transport.start()
    transport.join_reader(5)
    transport.stop_writer()
    events = rows(trace)
    assert [row["direction"] for row in events] == ["in", "out"]
    assert all(row["channel"] == "upstream_acp" and row["task_id"] == "t_test"
               and row["session_id"] == "acps_test" for row in events)
    assert events[1]["payload"] == json.loads(output.getvalue())


@pytest.mark.parametrize("engine", ["codex-acp", "claude-agent-acp"])
def test_downstream_real_fake_engine_frames_and_identity(trace, tmp_path, engine):
    from test_engine_session_manager import _make_manager, REQUESTED_CAPABILITIES
    from engine_fakes import default_scenario
    (tmp_path / "engine").mkdir()
    manager = _make_manager(tmp_path / "engine", default_scenario(), engine=engine)
    manager._trace_identity = {"task_id": "t_test", "assignment_ref": "assignment", "attempt_id": "attempt"}
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session = manager.create_session("t_test", "assignment")
        list(manager.prompt_turn(session, "hello"))
    finally:
        manager.shutdown(grace=2)
    events = [row for row in rows(trace) if row["channel"] == "downstream_acp"]
    assert {row["direction"] for row in events} == {"in", "out"}
    assert all(row["task_id"] == "t_test" and row["assignment_ref"] == "assignment"
               and row["attempt_id"] == "attempt" for row in events)
    assert any(row["payload"].get("method") == "session/prompt" for row in events)
    assert any(row["payload"].get("method") == "session/update" and row["engine_session_id"] == session
               for row in events)
    assert any(row["payload"].get("result", {}).get("stopReason") for row in events)


def test_mcp_tools_request_result_and_error_keep_exact_identity(trace, tmp_path):
    from cortex_runtime.server import Server, ProtocolError
    from cortex_runtime.orchestration.state import TaskMachine
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=project)
    channel = bootstrap_task(store, SourceEvent(origin="trace", event_id="mcp"), "Inspect.", "owner")
    TaskMachine(store).create(channel.task_id, "Inspect.")
    server = Server(channel_credentials=(channel.channel_id, channel.secret, str(project)))
    server.dispatch("tools/list", {}, request_id=0)
    params = {"name": "list_reports", "arguments": {"limit": 1}}
    result = server.dispatch("tools/call", params, request_id=1)
    assert result["isError"] is False
    with pytest.raises(ProtocolError):
        server.dispatch("tools/call", {"name": 1}, request_id=2)
    events = rows(trace)
    request, response = [row for row in events if row["payload"].get("id") == 1]
    assert request["payload"]["params"] == params
    assert response["payload"]["result"] == result
    assert all(row["task_id"] == channel.task_id and row["channel_id"] == channel.channel_id
               for row in (request, response))
    assert "error" in events[-1]["payload"]


def test_verification_real_sandbox_full_output_exit_and_lifecycle(trace, tmp_path):
    from cortex_runtime.delegation import revision
    from cortex_runtime.orchestration import controller
    from cortex_runtime.orchestration.state import TaskMachine, event
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=project)
    task = bootstrap_task(store, SourceEvent(origin="trace", event_id="verification"), "Verify.", "owner")
    TaskMachine(store).create(task.task_id, "Verify.")
    argv = ["python3", "-B", "-c",
            "print('first'); print('API_KEY=output-secret'); print('last'); "
            "print('-----BEGIN PRIVATE KEY-----'); print('fixture-private-material'); "
            "print('-----END PRIVATE KEY-----'); "
            "[print('row-'+str(i)+':'+ 'x'*256) for i in range(1000)]"]
    context = {"revision": 1, "plan": {"verification": [argv]}}
    runtime = SimpleNamespace(_store=store, _task_workspace_root=lambda _: project)
    before = revision.capture_revision(str(project))
    expected = before.get("dirty_digest") or before.get("digest")
    result = controller._verification_observation(runtime, task.task_id, context, expected,
                                                  None, threading.Event(), "run-test")
    assert result["checks"][0]["exit_code"] == 0
    assert result["checks"][0]["complete"]
    with store.connection() as db:
        event(db, task.task_id, "TEST_TRACE", {"assignment_ref": "assignment", "attempt_id": "attempt"})
        event(db, task.task_id, "TEST_OTHER_TRACE", ["fixture"])
    events = rows(trace)
    commands = [row for row in events if row["channel"] == "verification"]
    assert commands[0]["payload"]["argv"] == wire.Redactor({}).redact(argv)
    assert commands[0]["direction"] == "out"
    outputs = "".join(row["payload"].get("output", "") for row in commands)
    assert outputs.startswith("first\nAPI_KEY=[REDACTED]\nlast\n" +
                              "[REDACTED private-key output]\n" * 3)
    assert "fixture-private-material" not in outputs
    assert "row-0:" in outputs and outputs.endswith("row-999:" + "x" * 256 + "\n")
    assert len(outputs) > controller._VERIFICATION_OUTPUT_TAIL_BYTES
    assert commands[-1]["payload"] == {"exit_code": 0, "interrupted": False}
    assert all(row["task_id"] == task.task_id and row["command_ref"] for row in commands)
    lifecycle = [row for row in events if row["payload"].get("type") == "TEST_TRACE"][0]
    assert lifecycle["direction"] == "internal"
    assert lifecycle["assignment_ref"] == "assignment"
    assert lifecycle["payload"]["transaction"] == "pending"
    assert any(row["payload"].get("type") == "TEST_OTHER_TRACE" for row in events)


def test_redaction_structured_embedded_json_argv_env_headers_and_echo(trace):
    payload = {
        "headers": {"Authorization": "Bearer header-secret", "X-API-Key": "key-secret", "Cookie": "cookie-secret"},
        "env": [{"name": "PRIVATE_TOKEN", "value": "env-secret"}],
        "args": ["--relay", "/socket", "relay-secret", "--api-key=arg-secret", "--password", "password-secret"],
        "text": 'PRIVATE_TOKEN=text-secret\nBearer bearer-secret\nsk-abcdefghijklmnopqrstuv',
        "serialized": json.dumps({"api_key": "json-secret"}),
        "echo": "environment-secret key-secret env-secret relay-secret password-secret json-secret",
        "url": "https://username:url-password@host.example/path",
        "claim": "credential is claim-secret",
        "token_budget": 4096,
    }
    trace.learn_environment({"ENGINE_API_KEY": "child-environment-secret"})
    payload["child_echo"] = "child-environment-secret"
    trace.record("downstream_acp", "out", payload)
    trace.record("mcp", "in", {"echo": "json-secret key-secret"})
    raw = (trace.directory / "trace.jsonl").read_text()
    for value in ("header-secret", "key-secret", "cookie-secret", "env-secret", "relay-secret",
                  "arg-secret", "password-secret", "text-secret", "bearer-secret",
                  "sk-abcdefghijklmnopqrstuv", "json-secret", "environment-secret", "url-password", "claim-secret", "child-environment-secret"):
        assert value not in raw
    assert rows(trace)[0]["payload"]["token_budget"] == 4096
    assert rows(trace)[0]["payload"]["url"] == "https://[REDACTED]@host.example/path"


def _write_process(directory, index):
    tracer = wire.WireTrace(directory, {}, max_bytes=2048, file_count=4)
    for n in range(35):
        tracer.record("mcp", "in", {"process": index, "n": n, "body": "x" * 100})
    assert not tracer.failed


def test_rotation_cap_permissions_and_multiple_writers(trace):
    trace.max_bytes = 2048
    for n in range(40):
        trace.record("mcp", "in", {"n": n, "body": "x" * 100})
    processes = [multiprocessing.get_context("spawn").Process(target=_write_process,
                                                            args=(trace.directory, index)) for index in range(3)]
    for process in processes:
        process.start()
    for process in processes:
        process.join(10)
        assert process.exitcode == 0
    files = list(trace.directory.glob("*.jsonl"))
    assert len(files) == 4
    assert all(path.stat().st_size <= 2048 and stat.S_IMODE(path.stat().st_mode) == 0o600 for path in files)
    assert stat.S_IMODE(trace.directory.stat().st_mode) == 0o700
    assert rows(trace)


@pytest.mark.parametrize("unsafe", ["symlink_file", "hardlink_file", "public_file", "symlink_directory"])
def test_unsafe_trace_targets_stop_without_altering_target(trace, tmp_path, unsafe):
    trace.record("mcp", "in", {"ok": True})
    target = tmp_path / "protected"
    target.write_text("unchanged")
    target.chmod(0o600)
    path = trace.directory / "trace.jsonl"
    if unsafe == "symlink_file":
        path.unlink()
        path.symlink_to(target)
    elif unsafe == "hardlink_file":
        path.unlink()
        os.link(target, path)
    elif unsafe == "public_file":
        path.chmod(0o644)
    else:
        trace.directory.rename(trace.directory.with_name("original"))
        trace.directory.symlink_to(tmp_path, target_is_directory=True)
    trace.record("mcp", "out", {"must_not_write": True})
    assert trace.failed
    assert target.read_text() == "unchanged"


def test_viewer_is_read_only_filters_and_rejects_partial_json(trace, capsys):
    viewer = module("scripts/cortex_trace_view.py", "trace_viewer")
    trace.record("mcp", "in", {"a": 1}, task_id="t_one", assignment_ref="a_one")
    trace.record("mcp", "out", {"a": 2}, task_id="t_two", assignment_ref="a_two")
    before = {path.name: path.read_bytes() for path in trace.directory.iterdir()}
    assert viewer.main(["--directory", str(trace.directory), "--task", "t_one",
                        "--assignment", "a_one", "--direction", "in", "--pretty"]) == 0
    output = json.loads(capsys.readouterr().out)
    assert output["task_id"] == "t_one"
    assert before == {path.name: path.read_bytes() for path in trace.directory.iterdir()}
    with (trace.directory / "trace.jsonl").open("a") as stream:
        stream.write('{"incomplete":')
    assert viewer.main(["--directory", str(trace.directory)]) == 1


def test_installed_payload_import_refuses_even_with_forged_source_markers(tmp_path):
    home = tmp_path / "home"
    payload = home / ".local/state/cortex/paseo-runtime/1.0.0+sha256.test"
    directory = payload / "src/cortex_runtime/observability"
    directory.mkdir(parents=True)
    for relative in ("observability/wire_trace.py", "observability/redact.py",
                     "connections/environment.py"):
        source = ROOT / "src/cortex_runtime" / relative
        target = payload / "src/cortex_runtime" / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(source.read_text())
    # Installed-state refusal stands independently of the missing checkout markers.
    (payload / ".git").mkdir()
    (payload / "scripts").mkdir()
    (payload / "scripts/cortex_paseo_dev.py").write_text("forged marker")
    env = dict(os.environ, **dev_env(home, payload), HOME=str(home),
               PYTHONPATH=str(payload / "src"), PYTHONDONTWRITEBYTECODE="1")
    result = subprocess.run([sys.executable, "-B", "-c",
                             "from cortex_runtime.observability.wire_trace import get_trace; assert get_trace() is None"],
                            cwd=tmp_path, env=env, capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    assert "dev wire trace refused" in result.stderr
    assert not (home / ".paseo-dev").exists()


def test_real_mcp_subprocess_captures_wire_and_redacts_channel_secret(tmp_path):
    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=project)
    channel = bootstrap_task(store, SourceEvent(origin="trace", event_id="subprocess"), "Inspect.", "owner")
    env = dict(os.environ, **dev_env(home), HOME=str(home), PYTHONDONTWRITEBYTECODE="1",
               CORTEX_CHANNEL_ID=channel.channel_id, CORTEX_CHANNEL_SECRET=channel.secret,
               CORTEX_CHANNEL_PROJECT_ROOT=str(project))
    requests = [dict(jsonrpc="2.0", id=1, method="tools/list", params={}),
                dict(jsonrpc="2.0", id=2, method="tools/call",
                     params={"name": "list_reports", "arguments": {"limit": 1}}),
                dict(jsonrpc="2.0", id=3, method="tools/call", params={"name": 1})]
    result = subprocess.run([sys.executable, "-B", str(ROOT / "src/cortex.py")],
                            input="".join(json.dumps(row) + "\n" for row in requests), cwd=project,
                            env=env, capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    directory = home / ".paseo-dev/cortex-xdg/state/cortex/wire-traces"
    raw = (directory / "trace.jsonl").read_text()
    events = [json.loads(line) for line in raw.splitlines()]
    assert [row["payload"] for row in events if row["direction"] == "in"] == requests
    assert [row["payload"] for row in events if row["direction"] == "out"] == [json.loads(line) for line in result.stdout.splitlines()]
    assert channel.secret not in raw
    assert events[-1]["task_id"] == channel.task_id


def test_identity_enrichment_failure_preserves_frames_and_transport(trace):
    message = dict(jsonrpc="2.0", id=4, method="ping", params={})
    output = io.BytesIO()
    def context(_):
        raise RuntimeError("fixture failure")
    transport = StdioTransport(lambda *args: None, output,
                               io.BytesIO((json.dumps(message) + "\n").encode()),
                               trace_context=context)
    transport.start()
    transport.join_reader(3)
    assert transport.send_response(dict(jsonrpc="2.0", id=4, result={}))
    transport.stop_writer()
    events = rows(trace)
    assert [row["payload"] for row in events if row["direction"] == "in"] == [message]
    assert any(row["payload"].get("trace_error") == "identity_unavailable" for row in events)
    assert json.loads(output.getvalue())["result"] == {}


def test_location_becoming_a_git_repository_stops_capture(trace):
    trace.record("mcp", "in", {"ok": True})
    before = (trace.directory / "trace.jsonl").read_bytes()
    (trace.directory.parent / ".git").mkdir()
    trace.record("mcp", "out", {"must_not_write": True})
    assert trace.failed
    assert (trace.directory / "trace.jsonl").read_bytes() == before


def test_trace_failure_and_refusal_never_raise_even_with_closed_stderr(trace, monkeypatch):
    closed = io.StringIO()
    closed.close()
    monkeypatch.setattr(sys, "stderr", closed)
    trace.record("mcp", "in", {"ok": True})
    (trace.directory / "trace.jsonl").chmod(0o644)
    trace.record("mcp", "out", {"not_written": True})
    assert trace.failed
    def refuse(*args):
        raise ValueError("fixture refusal")
    monkeypatch.setattr(wire, "_gate", refuse)
    assert wire.configure({wire.OPT_IN: "1"}) is None


def test_oversize_record_has_explicit_gap_marker(trace):
    trace.max_bytes = 1024
    trace.record("mcp", "in", {"text": "x" * 4000})
    assert rows(trace)[0]["payload"]["trace_error"] == "record_exceeds_rotation_size"


@pytest.mark.parametrize("name", ["trace.lock", "trace.jsonl"])
@pytest.mark.parametrize("reader_peer", [False, True])
def test_fifo_trace_target_disables_capture_without_blocking(trace, name, reader_peer):
    os.close(trace._directory_fd())
    path = trace.directory / name
    os.mkfifo(path, 0o600)
    before = path.stat()
    # A peer lets O_WRONLY succeed, exercising descriptor type validation too.
    peer = os.open(path, os.O_RDONLY | os.O_NONBLOCK) if reader_peer else None
    try:
        env = dict(os.environ, PYTHONPATH=str(ROOT / "src"),
                   PYTHONDONTWRITEBYTECODE="1", CORTEX_DEV_WIRE_TRACE="0")
        result = subprocess.run(
            [sys.executable, "-B", "-c",
             "import sys; from cortex_runtime.observability.wire_trace import WireTrace; "
             "trace = WireTrace(sys.argv[1], {}); "
             "trace.record('mcp', 'in', {'must_not_write': True}); "
             "assert trace.failed; "
             "trace.record('mcp', 'out', {'still_disabled': True})",
             str(trace.directory)],
            cwd=ROOT, env=env, capture_output=True, text=True, timeout=5)
        assert result.returncode == 0, result.stderr
        assert result.stdout == ""
        assert "dev wire trace stopped" in result.stderr
        after = path.stat()
        assert stat.S_ISFIFO(after.st_mode)
        assert (after.st_ino, after.st_mode, after.st_size) == (
            before.st_ino, before.st_mode, before.st_size)
        if peer is not None:
            assert os.read(peer, 4096) == b""
        if name == "trace.lock":
            assert not (trace.directory / "trace.jsonl").exists()
    finally:
        if peer is not None:
            os.close(peer)


@pytest.mark.parametrize("name", ["trace.jsonl", "trace.1.jsonl",
                                  "trace.2.jsonl", "trace.3.jsonl"])
def test_fifo_viewer_input_refuses_without_blocking(trace, name):
    os.close(trace._directory_fd())
    path = trace.directory / name
    os.mkfifo(path, 0o600)
    before = path.stat()
    result = subprocess.run(
        [sys.executable, "-B", str(ROOT / "scripts/cortex_trace_view.py"),
         "--directory", str(trace.directory)],
        cwd=ROOT, capture_output=True, text=True, timeout=5)
    assert result.returncode == 1
    assert result.stdout == ""
    assert "cannot read complete trace (ValueError)" in result.stderr
    after = path.stat()
    assert stat.S_ISFIFO(after.st_mode)
    assert (after.st_ino, after.st_mode, after.st_size) == (
        before.st_ino, before.st_mode, before.st_size)
