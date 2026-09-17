"""stdout-purity tests.

Covers T009 acceptance: "No startup log/banner contaminates stdout protocol
messages," including with ``CORTEX_OBSERVATION_DIR`` set and noisy logging
active.
"""
import json
import sys

from acp_fakes import spawn_fake_agent, spawn_production_agent


def _send(proc, message):
    proc.stdin.write(json.dumps(message) + "\n")
    proc.stdin.flush()


def _assert_every_line_is_jsonrpc(output):
    lines = [line for line in output.splitlines() if line]
    assert lines, "expected at least one line of protocol output"
    for line in lines:
        item = json.loads(line)
        assert item.get("jsonrpc") == "2.0"
        assert ("result" in item) ^ ("error" in item) or "method" in item


def test_stdout_is_pure_jsonrpc_with_observation_dir_and_noisy_logging(tmp_path):
    observation_dir = tmp_path / "observations"
    env = {"CORTEX_OBSERVATION_DIR": str(observation_dir)}
    proc = spawn_production_agent(env=env)
    try:
        _send(proc, dict(jsonrpc="2.0", id=1, method="initialize",
                          params=dict(protocolVersion=1, clientCapabilities={},
                                      clientInfo=dict(name="t", version="1"))))
        _send(proc, dict(jsonrpc="2.0", id=2, method="session/new",
                          params=dict(cwd=str(tmp_path), mcpServers=[])))
        _send(proc, dict(jsonrpc="2.0", id=3, method="unknown/noisy", params={}))
        proc.stdin.close()
        out = proc.stdout.read()
        err = proc.stderr.read()
        proc.wait(timeout=15)
    finally:
        if proc.poll() is None:
            proc.kill()
    _assert_every_line_is_jsonrpc(out)
    assert proc.returncode == 0
    # The observation sink, if it wrote anything, must be off stdout and
    # under the configured private directory only.
    if observation_dir.exists():
        for path in observation_dir.iterdir():
            assert path.is_file()


def test_stdout_purity_with_streamed_updates(tmp_path):
    proc = spawn_fake_agent(chunks=("one", "two", "three"))
    try:
        _send(proc, dict(jsonrpc="2.0", id=1, method="initialize",
                          params=dict(protocolVersion=1, clientCapabilities={},
                                      clientInfo=dict(name="t", version="1"))))
        init_reply = json.loads(proc.stdout.readline())
        assert init_reply["result"]["protocolVersion"] == 1
        _send(proc, dict(jsonrpc="2.0", id=2, method="session/new",
                          params=dict(cwd=str(tmp_path), mcpServers=[])))
        session_id = json.loads(proc.stdout.readline())["result"]["sessionId"]
        _send(proc, dict(jsonrpc="2.0", id=3, method="session/prompt",
                          params=dict(sessionId=session_id, prompt=[dict(type="text", text="go")])))
        proc.stdin.close()
        out = proc.stdout.read()
        proc.stderr.read()
        proc.wait(timeout=15)
    finally:
        if proc.poll() is None:
            proc.kill()
    _assert_every_line_is_jsonrpc(out)
    assert proc.returncode == 0


def test_import_time_print_guard():
    """Importing the agent module tree must never write to real stdout."""
    import io
    import contextlib

    buffer = io.StringIO()
    with contextlib.redirect_stdout(buffer):
        import importlib
        import cortex_runtime.acp.agent as agent_module
        import cortex_runtime.acp.transport  # noqa: F401
        import cortex_runtime.acp.protocol  # noqa: F401
        import cortex_runtime.acp.runtime  # noqa: F401
        import cortex_runtime.acp.sessions  # noqa: F401
        importlib.reload(agent_module)
    assert buffer.getvalue() == ""
