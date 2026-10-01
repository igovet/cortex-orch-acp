"""A worker's Cortex MCP server must never die while writing one reply.

The stdio server serialized each reply *outside* its per-request error
handling. A reply that could not be encoded -- a lone surrogate from a
non-UTF-8 file name (``os.listdir`` decodes those with ``surrogateescape``)
-- raised out of the read loop and ended the process. The engine's MCP client
then reports ``Transport closed`` for every later Cortex call of the session,
so the worker can publish nothing and cannot ask a question. These tests
drive the real ``src/cortex.py`` subprocess over its stdio wire.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import uuid
from pathlib import Path

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

ROOT = Path(__file__).resolve().parents[1]


def _bootstrap(project: Path):
    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Inspect files.", "owner-1")
    TaskMachine(store).create(result.task_id, "Inspect files.")
    return store, result


def _storage_only_home(home: Path) -> Path:
    cortex = home / "cortex"
    cortex.mkdir(parents=True, exist_ok=True)
    cortex.chmod(0o700)
    config = cortex / "config.toml"
    config.write_text("schema_version = 1\n[gateway]\nenabled = false\n")
    config.chmod(0o600)
    return home


def _exchange(tmp_path: Path, project: Path, channel, frames: list[bytes]):
    """Send raw request lines to one real server process; return its result."""
    env = dict(os.environ, CODEX_HOME=str(_storage_only_home(tmp_path / "host")),
               CORTEX_CHANNEL_ID=channel.channel_id, CORTEX_CHANNEL_SECRET=channel.secret,
               CORTEX_CHANNEL_PROJECT_ROOT=str(project))
    return subprocess.run(
        [sys.executable, "-B", str(ROOT / "src/cortex.py")], input=b"".join(frame + b"\n" for frame in frames),
        capture_output=True, env=env, timeout=60,
    )


def _call(request_id, name, arguments) -> bytes:
    return json.dumps(dict(jsonrpc="2.0", id=request_id, method="tools/call",
                           params=dict(name=name, arguments=arguments))).encode("ascii")


def _replies(completed) -> dict:
    # Every reply line must itself be strict UTF-8 JSON: an engine's MCP
    # client (Rust or Node) rejects anything else.
    rows = [json.loads(line.decode("utf-8")) for line in completed.stdout.splitlines()]
    return {row.get("id"): row for row in rows}


def test_non_utf8_directory_entry_keeps_the_server_and_later_calls_alive(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    store, channel = _bootstrap(project)
    (project / "logs").mkdir()
    (project / "logs" / "plain.txt").write_text("ok")
    # A real file name that is not valid UTF-8 (common in rotated/foreign logs).
    with open(os.path.join(os.fsencode(project / "logs"), b"shard-\xff.log"), "wb") as handle:
        handle.write(b"x")

    completed = _exchange(tmp_path, project, channel, [
        _call(1, "project_read", dict(path="logs")),
        _call(2, "list_reports", {}),
    ])

    assert completed.returncode == 0, completed.stderr.decode("utf-8", "replace")
    assert completed.stderr == b""
    replies = _replies(completed)
    assert set(replies) == {1, 2}
    listing = replies[1]["result"]
    assert not listing["isError"]
    entries = listing["structuredContent"]["entries"]
    assert "plain.txt" in entries
    # The undecodable byte is shown as U+FFFD, never as a raw surrogate.
    assert "shard-�.log" in entries
    assert json.loads(listing["content"][0]["text"])["entries"] == entries
    # The same process still serves the next call: the transport stayed open.
    assert not replies[2]["result"]["isError"]


def test_lone_surrogate_argument_is_answered_and_the_transport_stays_open(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    store, channel = _bootstrap(project)
    # A JSON ``\\udcff`` escape is valid JSON text; json.loads yields a lone
    # surrogate inside the model-supplied argument.
    bad = (b'{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"project_read",'
           b'"arguments":{"path":"missing-\\udcff.log"}}}')
    completed = _exchange(tmp_path, project, channel, [bad, _call(2, "list_reports", {})])

    assert completed.returncode == 0, completed.stderr.decode("utf-8", "replace")
    replies = _replies(completed)
    assert set(replies) == {1, 2}
    assert replies[1]["result"]["isError"]
    assert not replies[2]["result"]["isError"]


def test_unserializable_reply_becomes_an_uncertain_error_not_a_dead_process():
    from cortex_runtime.server import encode_reply

    frame = encode_reply(dict(jsonrpc="2.0", id=7, result=dict(value=object())))
    reply = json.loads(frame.decode("utf-8"))
    assert frame.endswith(b"\n") and frame.count(b"\n") == 1
    assert reply["id"] == 7 and reply["error"]["code"] == -32603
    # The request may have taken effect before its reply failed: the error
    # must direct an exact retry, never a changed guess.
    assert "exact original request" in reply["error"]["message"]

    surrogate = encode_reply(dict(jsonrpc="2.0", id="s", result=dict(text="a\udcffb")))
    assert json.loads(surrogate.decode("utf-8"))["result"]["text"] == "a�b"
    plain = encode_reply(dict(jsonrpc="2.0", id=1, result=dict(text="Русский — 🧪")))
    assert plain == (json.dumps(dict(jsonrpc="2.0", id=1, result=dict(text="Русский — 🧪")),
                                ensure_ascii=False) + "\n").encode("utf-8")
