"""Transport-level tests: framing, corrupt input, EOF, slow consumer.

Covers T009 acceptance: "Fragmented Unicode, malformed frames, oversized
messages, EOF, and a slow consumer have bounded, tested behavior."
"""
import ast
import json
import os
import threading
import time
from pathlib import Path

import pytest

from cortex_runtime.acp.transport import (
    FrameError, StdioTransport, parse_frame, recover_request_id, MAX_FRAME_BYTES, MAX_JSON_DEPTH,
    ID_RECOVERY_PREFIX_BYTES,
)
from acp_fakes import spawn_fake_agent, spawn_flooding_agent, spawn_production_agent


# --- F3 (T012 review): writer thread tolerates a closed pipe --------------

def test_writer_thread_tolerates_closed_pipe_and_stops_quietly(capsys):
    """Closing the read end of the pipe while writes are still queued must
    make the writer thread stop quietly (one stderr line, no unhandled
    exception escaping the thread) instead of crashing or spinning."""
    read_fd, write_fd = os.pipe()
    write_file = os.fdopen(write_fd, "wb", buffering=0)
    os.close(read_fd)  # closing the read end makes writes raise BrokenPipeError

    # A second, genuinely closeable pipe for stdin: both ends are kept as
    # real file objects so the reader thread reliably gets EOF (and this
    # test leaves no thread behind for the rest of the session -- see the
    # `_no_leaked_acp_transport_threads` autouse fixture in conftest.py).
    stdin_read_fd, stdin_write_fd = os.pipe()
    stdin_read_file = os.fdopen(stdin_read_fd, "rb")
    stdin_write_file = os.fdopen(stdin_write_fd, "wb")

    transport = StdioTransport(lambda message, error: None, raw_stdout=write_file, raw_stdin=stdin_read_file)
    transport.start()
    try:
        for _ in range(5):
            transport.send_notification(dict(jsonrpc="2.0", method="session/update", params={}), timeout=1.0)
        transport._writer_thread.join(timeout=5.0)
        assert not transport._writer_thread.is_alive()  # the loop stopped, not just blocked
        assert transport._closed.is_set()
        captured = capsys.readouterr()
        assert "acp: writer stopped after" in captured.err
    finally:
        stdin_write_file.close()  # unblocks the reader thread's readline() with EOF
        transport.join_reader(timeout=5.0)


def test_agent_exits_within_bound_when_stdout_reader_closes_but_stdin_stays_open():
    """T012 review item 1: if the client's stdout reader goes away (its
    read end closed) while the agent's stdin stays open, the agent must
    still exit within a bound instead of wedging forever in
    ``join_reader()`` waiting for a read-side EOF that will never come."""
    proc = spawn_fake_agent()
    try:
        proc.stdout.close()  # simulate the peer's stdout reader going away
        proc.stdin.write(json.dumps(dict(jsonrpc="2.0", id=1, method="initialize",
                                          params=dict(protocolVersion=1))) + "\n")
        proc.stdin.flush()
        # Deliberately never close stdin here: the reader thread would
        # otherwise get its own EOF and this test would not exercise the
        # writer-closed path at all.
        exit_code = proc.wait(timeout=10.0)
        assert exit_code == 0
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=5.0)
        for stream in (proc.stdin, proc.stderr):
            try:
                stream.close()
            except Exception:
                pass


# --- pure parse_frame unit tests -------------------------------------------

def test_duplicate_json_keys_rejected():
    raw = b'{"jsonrpc":"2.0","id":1,"method":"ping","method":"other"}'
    with pytest.raises(FrameError) as excinfo:
        parse_frame(raw)
    assert excinfo.value.code == -32600


def test_non_object_batch_array_rejected():
    raw = b'[{"jsonrpc":"2.0","id":1,"method":"ping"}]'
    with pytest.raises(FrameError) as excinfo:
        parse_frame(raw)
    assert excinfo.value.code == -32600


def test_malformed_json_is_parse_error():
    with pytest.raises(FrameError) as excinfo:
        parse_frame(b'{not json')
    assert excinfo.value.code == -32700


def test_invalid_utf8_is_parse_error():
    with pytest.raises(FrameError) as excinfo:
        parse_frame(b'\xff\xfe{"jsonrpc":"2.0"}')
    assert excinfo.value.code == -32700


def test_oversized_frame_rejected():
    raw = ('{"jsonrpc":"2.0","id":1,"method":"ping","padding":"' +
           'x' * (MAX_FRAME_BYTES + 10) + '"}').encode()
    with pytest.raises(FrameError) as excinfo:
        parse_frame(raw)
    assert excinfo.value.code == -32700
    assert excinfo.value.request_id == 1


def test_frame_limit_fits_a_prompt_with_its_full_attachment_budget():
    from cortex_runtime.acp.replay_store import MAX_PROMPT_ATTACHMENT_BYTES
    encoded = ((MAX_PROMPT_ATTACHMENT_BYTES + 2) // 3) * 4
    assert MAX_FRAME_BYTES >= encoded + 1024 * 1024
    # The Paseo relay drops the whole stream above 16 MiB; Cortex must answer first.
    assert MAX_FRAME_BYTES < 16 * 1024 * 1024


def test_frame_carrying_a_near_cap_image_is_accepted():
    """A session/prompt frame carrying a single image at the raised 10 MiB
    per-block cap still fits within the transport frame limit and parses."""
    import base64

    from cortex_runtime.acp.replay_store import MAX_BLOCK_BYTES

    image_b64 = base64.b64encode(b"x" * MAX_BLOCK_BYTES).decode("ascii")
    raw = json.dumps(dict(jsonrpc="2.0", id=1, method="session/prompt",
                           params=dict(sessionId="s1",
                                       prompt=[dict(type="image", data=image_b64, mimeType="image/png")]))).encode()
    assert len(raw) <= MAX_FRAME_BYTES
    message = parse_frame(raw)
    assert message["id"] == 1


@pytest.mark.parametrize("head, expected", [
    (b'{"jsonrpc":"2.0","id":7,"method":"session/prompt","params":{"prompt":[{"data":"AAAA', 7),
    (b'{ "jsonrpc" : "2.0" , "id" : "req-\\u00e9" , "params":{"x', "req-\u00e9"),
    (b'{"method":"session/prompt","jsonrpc":"2.0","id":3,"params":{', 3),
    (b'{"jsonrpc":"2.0","method":"session/prompt","params":{"prompt":"xxx', None),
    (b'{"params":{"id":5,"prompt":"xxx', None),
    (b'{"jsonrpc":"2.0","id":null,"method":"x","params":{', None),
    (b'{"jsonrpc":"2.0","id":true,"method":"x","params":{', None),
    (b'{"jsonrpc":"2.0","id":1.5,"method":"x","params":{', None),
    (b'{"jsonrpc":"2.0","id":"unterminated', None),
    (b'["jsonrpc","id",1]', None),
    (b'\xff{"id":1,', None),
    (b"not json at all", None),
])
def test_recover_request_id_reads_only_a_top_level_id(head, expected):
    assert recover_request_id(head) == expected


def test_recover_request_id_ignores_an_id_beyond_the_scanned_head():
    head = ('{"jsonrpc":"2.0","method":"x","pad":"' + "x" * ID_RECOVERY_PREFIX_BYTES + '","id":9}').encode()
    assert recover_request_id(head) is None


def test_missing_jsonrpc_field_rejected():
    with pytest.raises(FrameError) as excinfo:
        parse_frame(b'{"id":1,"method":"ping"}')
    assert excinfo.value.code == -32600


# --- F1: deeply-nested "bomb" frames must never crash the reader (review fix) ---

def test_deeply_nested_array_bomb_is_rejected_not_a_recursion_error():
    raw = (b"[" * 100_000) + (b"]" * 100_000)
    with pytest.raises(FrameError) as excinfo:
        parse_frame(raw)
    assert excinfo.value.code == -32700


def test_deeply_nested_object_bomb_is_rejected_not_a_recursion_error():
    raw = (b'{"a":' * 100_000) + b"1" + (b"}" * 100_000)
    with pytest.raises(FrameError) as excinfo:
        parse_frame(raw)
    assert excinfo.value.code == -32700


def test_nesting_within_the_configured_limit_still_parses():
    depth = MAX_JSON_DEPTH - 4  # leave headroom for the enclosing object/array
    nested = ("[" * depth) + "1" + ("]" * depth)
    raw = ('{"jsonrpc":"2.0","id":1,"method":"ping","params":' + nested + "}").encode()
    message = parse_frame(raw)
    assert message["id"] == 1


def test_nesting_over_the_configured_limit_is_rejected():
    depth = MAX_JSON_DEPTH + 10
    nested = ("[" * depth) + "1" + ("]" * depth)
    raw = ('{"jsonrpc":"2.0","id":1,"method":"ping","params":' + nested + "}").encode()
    with pytest.raises(FrameError) as excinfo:
        parse_frame(raw)
    assert excinfo.value.code == -32700


def test_reader_thread_survives_a_bomb_frame_and_keeps_serving(tmp_path):
    """The reader must not die on a single malformed frame: a valid
    request sent right after a nesting bomb still gets a normal reply."""
    proc = spawn_production_agent()
    try:
        bomb = ("[" * 100_000) + ("]" * 100_000)
        proc.stdin.write(bomb + "\n")
        proc.stdin.flush()
        bomb_reply = json.loads(proc.stdout.readline())
        assert bomb_reply["id"] is None and bomb_reply["error"]["code"] == -32700

        proc.stdin.write(json.dumps(dict(jsonrpc="2.0", id=2, method="session/new",
                                          params=dict(cwd=str(tmp_path), mcpServers=[]))) + "\n")
        proc.stdin.flush()
        # initialize was never called, so this is rejected for that reason
        # (-32600), not silently dropped -- proof the reader is still alive
        # and dispatching.
        reply = json.loads(proc.stdout.readline())
        assert reply["id"] == 2 and reply["error"]["code"] == -32600

        proc.stdin.write(json.dumps(dict(jsonrpc="2.0", id=3, method="initialize",
                                          params=dict(protocolVersion=1, clientCapabilities={},
                                                      clientInfo=dict(name="t", version="1")))) + "\n")
        proc.stdin.flush()
        init_reply = json.loads(proc.stdout.readline())
        assert init_reply["result"]["protocolVersion"] == 1

        proc.stdin.write(json.dumps(dict(jsonrpc="2.0", id=4, method="session/new",
                                          params=dict(cwd=str(tmp_path), mcpServers=[]))) + "\n")
        proc.stdin.flush()
        session_reply = json.loads(proc.stdout.readline())
        assert session_reply["result"]["sessionId"].startswith("acps_")
    finally:
        proc.stdin.close()
        proc.wait(timeout=15)
        if proc.poll() is None:
            proc.kill()
    assert proc.returncode == 0


# --- subprocess-level behavior ----------------------------------------------

def _initialize(proc):
    proc.stdin.write(json.dumps(dict(
        jsonrpc="2.0", id=1, method="initialize",
        params=dict(protocolVersion=1, clientCapabilities={}, clientInfo=dict(name="t", version="1")),
    )) + "\n")
    proc.stdin.flush()
    return json.loads(proc.stdout.readline())


def test_fragmented_unicode_written_byte_by_byte(tmp_path):
    proc = spawn_production_agent()
    try:
        message = json.dumps(dict(
            jsonrpc="2.0", id=1, method="initialize",
            params=dict(protocolVersion=1, clientCapabilities={},
                        clientInfo=dict(name="Русский 🧪", version="1")),
        )) + "\n"
        body = message.encode("utf-8")
        raw = proc.stdin.buffer if hasattr(proc.stdin, "buffer") else proc.stdin
        for byte in body:
            raw.write(bytes([byte]))
            raw.flush()
        reply = json.loads(proc.stdout.readline())
        assert reply["result"]["protocolVersion"] == 1
    finally:
        proc.stdin.close()
        proc.wait(timeout=15)
        assert proc.returncode == 0


def test_oversized_frame_over_subprocess_no_echo_no_hang():
    proc = spawn_production_agent()
    try:
        secret = "PRIVATE_SENTINEL"
        oversized = json.dumps(dict(
            jsonrpc="2.0", id=1, method="initialize",
            params=dict(protocolVersion=1, clientCapabilities={},
                        clientInfo=dict(name="x" * (MAX_FRAME_BYTES + 10), version=secret)),
        )) + "\n"
        good = json.dumps(dict(jsonrpc="2.0", id=2, method="initialize",
                                params=dict(protocolVersion=1, clientCapabilities={},
                                            clientInfo=dict(name="t", version="1")))) + "\n"
        proc.stdin.write(oversized)
        proc.stdin.write(good)
        proc.stdin.close()
        out = proc.stdout.read()
        proc.stderr.read()
        proc.wait(timeout=15)
    finally:
        if proc.poll() is None:
            proc.kill()
    lines = [json.loads(line) for line in out.splitlines()]
    assert len(lines) == 2
    assert lines[0]["id"] == 1 and lines[0]["error"]["code"] == -32700
    assert secret not in out
    assert lines[1]["result"]["protocolVersion"] == 1
    assert proc.returncode == 0


def test_oversized_prompt_over_subprocess_is_answered_on_its_own_id():
    """A session/prompt too large to read (e.g. several inline screenshots)
    must get an error on its own id so the client ends the turn, instead of
    an id-null reply that leaves the prompt pending forever."""
    proc = spawn_production_agent()
    try:
        init = json.dumps(dict(jsonrpc="2.0", id=1, method="initialize",
                               params=dict(protocolVersion=1, clientCapabilities={},
                                           clientInfo=dict(name="t", version="1")))) + "\n"
        image = dict(type="image", mimeType="image/png", data="A" * (MAX_FRAME_BYTES + 10))
        prompt = json.dumps(dict(jsonrpc="2.0", id=2, method="session/prompt",
                                 params=dict(sessionId="acps_missing", prompt=[image]))) + "\n"
        proc.stdin.write(init)
        proc.stdin.write(prompt)
        proc.stdin.close()
        out = proc.stdout.read()
        proc.stderr.read()
        proc.wait(timeout=15)
    finally:
        if proc.poll() is None:
            proc.kill()
    lines = [json.loads(line) for line in out.splitlines()]
    assert len(lines) == 2
    assert lines[0]["id"] == 1 and "result" in lines[0]
    assert lines[1]["id"] == 2 and lines[1]["error"]["code"] == -32700
    assert "images" in lines[1]["error"]["message"]
    assert len(out) < 4096
    assert proc.returncode == 0


def test_duplicate_keys_and_batch_over_subprocess():
    proc = spawn_production_agent()
    try:
        raw = (
            '{"jsonrpc":"2.0","id":1,"method":"ping","method":"other"}\n'
            '[{"jsonrpc":"2.0","id":2,"method":"ping"}]\n'
            + json.dumps(dict(jsonrpc="2.0", id=3, method="initialize",
                               params=dict(protocolVersion=1, clientCapabilities={},
                                           clientInfo=dict(name="t", version="1")))) + "\n"
        )
        proc.stdin.write(raw)
        proc.stdin.close()
        out = proc.stdout.read()
        proc.stderr.read()
        proc.wait(timeout=15)
    finally:
        if proc.poll() is None:
            proc.kill()
    lines = [json.loads(line) for line in out.splitlines()]
    assert lines[0]["error"]["code"] == -32600
    assert lines[1]["error"]["code"] == -32600
    assert lines[2]["result"]["protocolVersion"] == 1
    assert proc.returncode == 0


def test_eof_mid_prompt_exits_without_hanging(tmp_path):
    proc = spawn_fake_agent(chunks=("a",) * 50, delay=0.05)
    try:
        _initialize(proc)
        project = str(tmp_path)
        proc.stdin.write(json.dumps(dict(jsonrpc="2.0", id=2, method="session/new",
                                          params=dict(cwd=project, mcpServers=[]))) + "\n")
        proc.stdin.flush()
        session_id = json.loads(proc.stdout.readline())["result"]["sessionId"]
        proc.stdin.write(json.dumps(dict(jsonrpc="2.0", id=3, method="session/prompt",
                                          params=dict(sessionId=session_id,
                                                      prompt=[dict(type="text", text="go")]))) + "\n")
        proc.stdin.flush()
        # Read at most one update, then close stdin (simulate the client
        # disappearing mid-turn) without ever reading the final response.
        proc.stdout.readline()
        proc.stdin.close()
        proc.wait(timeout=15)
    finally:
        if proc.poll() is None:
            proc.kill()
    assert proc.returncode == 0


def test_slow_consumer_bounded_and_cancellable_no_deadlock(tmp_path):
    """A slow client (reads a little, then genuinely stops reading for a
    while -- no background thread keeps draining the OS pipe during that
    pause, so its kernel buffer really does fill and the agent's writer
    thread really does stall) must not deadlock, and a ``session/cancel``
    sent while the flood is in-flight must still be serviced promptly by
    the reader thread and stop the prompt -- the transport's bounded
    outbound queue and timeout-based notification-drop policy exist exactly
    to make this recoverable rather than a permanent deadlock or unbounded
    memory growth. Uses ``tmp_path`` (never the real checkout) as the
    session's project root. A watchdog kills the subprocess if this
    genuinely never recovers, so a real regression fails fast instead of
    hanging the suite."""
    proc = spawn_flooding_agent(total=5_000_000)
    watchdog = threading.Timer(60.0, proc.kill)
    watchdog.start()
    try:
        proc.stdin.write(json.dumps(dict(jsonrpc="2.0", id=1, method="initialize",
                                          params=dict(protocolVersion=1, clientCapabilities={},
                                                      clientInfo=dict(name="t", version="1")))) + "\n")
        proc.stdin.flush()
        reply = json.loads(proc.stdout.readline())
        assert reply["result"]["protocolVersion"] == 1
        proc.stdin.write(json.dumps(dict(jsonrpc="2.0", id=2, method="session/new",
                                          params=dict(cwd=str(tmp_path), mcpServers=[]))) + "\n")
        proc.stdin.flush()
        session_id = json.loads(proc.stdout.readline())["result"]["sessionId"]
        proc.stdin.write(json.dumps(dict(jsonrpc="2.0", id=3, method="session/prompt",
                                          params=dict(sessionId=session_id,
                                                      prompt=[dict(type="text", text="go")]))) + "\n")
        proc.stdin.flush()
        # Read only a handful of the flood's updates, then genuinely stop
        # reading for several seconds: this is the actual backpressure
        # condition the bounded outbound queue and send_notification
        # timeout exist to survive.
        for _ in range(3):
            proc.stdout.readline()
        time.sleep(7)
        # The reader thread must still be servicing input while the writer
        # is stalled: this notification has to reach the agent and take
        # effect.
        proc.stdin.write(json.dumps(dict(jsonrpc="2.0", method="session/cancel",
                                          params=dict(sessionId=session_id))) + "\n")
        proc.stdin.flush()
        # Resume reading (draining the stalled backlog) until the prompt's
        # final response (id 3) arrives; the watchdog bounds worst case.
        final = None
        while True:
            raw = proc.stdout.readline()
            if not raw:
                break
            item = json.loads(raw)
            if item.get("id") == 3:
                final = item
                break
        assert final is not None, "prompt never completed after cancel"
        assert final["result"]["stopReason"] == "cancelled"
    finally:
        watchdog.cancel()
        proc.stdin.close()
        proc.wait(timeout=15)
        if proc.poll() is None:
            proc.kill()
    assert proc.returncode == 0


# --- static guard: no test may pass the real checkout as a project root ---
#
# tests/conftest.py's in-process monkeypatch guard cannot reach a
# subprocess (the ACP agent is normally exercised as a real subprocess --
# see acp_fakes.spawn_*). This is a best-effort static source scan for the
# actual incident pattern: os.getcwd()/Path.cwd()/a module-level ROOT name
# used in a *project-root* position (an ACP session/new "cwd" alongside
# "mcpServers", or a "project_root"/positional argument to one of the
# functions that resolve/open project-scoped storage). It deliberately does
# NOT flag the extremely common, unrelated `subprocess.run(..., cwd=ROOT)`
# pattern (a subprocess's working directory, not a Cortex project root)
# used throughout the rest of this suite.

_ROOT_SCAN_FUNCTIONS = frozenset({
    "Store", "canonical_project", "project_store_directory",
    "migrate_legacy_project_storage", "logical_project_root",
    "bootstrap_task", "append_source_event", "create_session", "bind_task",
})
_ROOT_NAME_TOKENS = frozenset({"ROOT", "REPO_ROOT"})


def _callee_name(func):
    if isinstance(func, ast.Name):
        return func.id
    if isinstance(func, ast.Attribute):
        return func.attr
    return None


def _expression_flags_repo_root(node):
    for sub in ast.walk(node):
        if isinstance(sub, ast.Call) and _callee_name(sub.func) in ("getcwd", "cwd"):
            return True
        if isinstance(sub, ast.Name) and sub.id in _ROOT_NAME_TOKENS:
            return True
    return False


def _scan_source_for_repo_root_as_project_root(path):
    findings = []
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        name = _callee_name(node.func)
        if name == "dict":
            values_by_key = {kw.arg: kw.value for kw in node.keywords if kw.arg}
            if ("cwd" in values_by_key and "mcpServers" in values_by_key
                    and _expression_flags_repo_root(values_by_key["cwd"])):
                findings.append((node.lineno, "session/new cwd built from the repo root"))
            continue
        if name in _ROOT_SCAN_FUNCTIONS:
            for arg in node.args:
                if _expression_flags_repo_root(arg):
                    findings.append((node.lineno, f"{name}(...) positional argument built from the repo root"))
            for kw in node.keywords:
                if kw.arg in ("project_root", "cwd", "directory") and _expression_flags_repo_root(kw.value):
                    findings.append((node.lineno, f"{name}(..., {kw.arg}=...) built from the repo root"))
    return findings


def test_no_test_source_uses_repo_root_as_project_root():
    tests_dir = Path(__file__).resolve().parent
    # Top-level test modules and conftest.py only: fixtures/ holds
    # deliberately broken/synthetic source trees for other tests (not
    # actual pytest test modules) and must not be parsed as such.
    candidates = sorted(tests_dir.glob("test_*.py")) + sorted(tests_dir.glob("conftest.py"))
    all_findings = []
    for path in candidates:
        for lineno, reason in _scan_source_for_repo_root_as_project_root(path):
            all_findings.append(f"{path.relative_to(tests_dir)}:{lineno}: {reason}")
    assert not all_findings, (
        "test source uses the real repository checkout as a project root "
        "(this is exactly the T009 incident pattern -- use tmp_path or an "
        "isolated scratch clone instead):\n" + "\n".join(all_findings)
    )
