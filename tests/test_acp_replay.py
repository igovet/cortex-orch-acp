"""Replay/idempotency tests.

Covers T009 acceptance: "History replay does not create new task, source,
assignment, or publication mutations," exercised here through the durable
bootstrap/append-source-event path (bootstrap.bootstrap_task /
bootstrap.append_source_event), the seam session/prompt uses before ever
reaching the runtime.
"""
import json
import sqlite3
import tempfile
import threading
from pathlib import Path
from types import SimpleNamespace

from cortex_runtime.bootstrap import append_source_event, bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.acp import sessions as acp_sessions
from cortex_runtime.acp.agent import Agent, _request_id_identity
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.store import Store
from acp_fakes import spawn_production_agent


def _open_store(tmp_path):
    project = tmp_path
    (project / ".cortex").mkdir(mode=0o700, exist_ok=True)
    return Store(project / ".cortex", initialize=True, project_root=str(project))


def test_redelivered_event_creates_no_new_rows(tmp_path):
    store = _open_store(tmp_path)
    session_id = "acps_replaytest0001"
    acp_sessions.create_session(store, session_id, str(tmp_path), {}, {}, [])
    event = SimpleNamespace(origin=f"acp:{session_id}", event_id="evt-1")

    first = bootstrap_task(store, event, "Original text", owner_ref=f"acp:{session_id}")
    acp_sessions.bind_task(store, session_id, first.task_id)
    second = bootstrap_task(store, event, "Original text", owner_ref=f"acp:{session_id}")
    assert second.replayed is True
    assert second.task_id == first.task_id

    db = sqlite3.connect(tmp_path / ".cortex" / "cortex.sqlite3")
    assert db.execute("SELECT COUNT(*) FROM tasks").fetchone()[0] == 1
    assert db.execute("SELECT COUNT(*) FROM source_events").fetchone()[0] == 1
    assert db.execute("SELECT COUNT(*) FROM reports").fetchone()[0] == 1

    later = append_source_event(store, event, "Original text", first.task_id)
    assert later.replayed is True
    assert db.execute("SELECT COUNT(*) FROM source_events").fetchone()[0] == 1
    assert db.execute("SELECT COUNT(*) FROM reports").fetchone()[0] == 1


def test_identical_text_distinct_events_are_distinct_source_events_same_task(tmp_path):
    store = _open_store(tmp_path)
    session_id = "acps_replaytest0002"
    acp_sessions.create_session(store, session_id, str(tmp_path), {}, {}, [])
    origin = f"acp:{session_id}"

    first_event = SimpleNamespace(origin=origin, event_id="evt-a")
    first = bootstrap_task(store, first_event, "Same text", owner_ref=origin)
    acp_sessions.bind_task(store, session_id, first.task_id)

    second_event = SimpleNamespace(origin=origin, event_id="evt-b")
    second = append_source_event(store, second_event, "Same text", first.task_id)
    assert second.replayed is False
    assert second.task_id == first.task_id

    db = sqlite3.connect(tmp_path / ".cortex" / "cortex.sqlite3")
    assert db.execute("SELECT COUNT(*) FROM tasks").fetchone()[0] == 1
    assert db.execute("SELECT COUNT(DISTINCT event_id) FROM source_events WHERE task_id=?",
                       (first.task_id,)).fetchone()[0] == 2
    assert db.execute("SELECT COUNT(*) FROM reports WHERE task_id=?", (first.task_id,)).fetchone()[0] == 2


def test_append_source_event_never_creates_a_second_task(tmp_path):
    store = _open_store(tmp_path)
    session_id = "acps_replaytest0003"
    acp_sessions.create_session(store, session_id, str(tmp_path), {}, {}, [])
    origin = f"acp:{session_id}"
    first = bootstrap_task(store, SimpleNamespace(origin=origin, event_id="e1"), "one", owner_ref=origin)
    acp_sessions.bind_task(store, session_id, first.task_id)
    for index in range(5):
        append_source_event(store, SimpleNamespace(origin=origin, event_id=f"e{index+2}"),
                             f"message {index}", first.task_id)
    db = sqlite3.connect(tmp_path / ".cortex" / "cortex.sqlite3")
    assert db.execute("SELECT COUNT(*) FROM tasks").fetchone()[0] == 1
    assert db.execute("SELECT COUNT(*) FROM source_events").fetchone()[0] == 6


def test_append_source_event_rejects_unknown_task(tmp_path):
    store = _open_store(tmp_path)
    origin = "acp:acps_unbound"
    try:
        append_source_event(store, SimpleNamespace(origin=origin, event_id="e1"), "text", "t_doesnotexist")
        assert False, "expected StoreError"
    except StoreError as error:
        assert str(error) == "task_not_bound"


def test_acp_session_task_binding_immutable(tmp_path):
    store = _open_store(tmp_path)
    session_id = "acps_replaytest0004"
    acp_sessions.create_session(store, session_id, str(tmp_path), {}, {}, [])
    origin = f"acp:{session_id}"
    first = bootstrap_task(store, SimpleNamespace(origin=origin, event_id="e1"), "one", owner_ref=origin)
    acp_sessions.bind_task(store, session_id, first.task_id)
    acp_sessions.bind_task(store, session_id, first.task_id)  # idempotent, no error

    other = bootstrap_task(store, SimpleNamespace(origin="acp:other", event_id="e-other"),
                            "two", owner_ref="acp:other")
    try:
        acp_sessions.bind_task(store, session_id, other.task_id)
        assert False, "expected StoreError"
    except StoreError as error:
        assert str(error) == "acp_session_task_conflict"

    row = acp_sessions.get_session(store, session_id)
    assert row["task_id"] == first.task_id


# --- F2 (review fix): duplicate JSON-RPC request id on session/prompt -------

def _send(proc, message):
    proc.stdin.write(json.dumps(message) + "\n")
    proc.stdin.flush()


def _recv(proc):
    return json.loads(proc.stdout.readline())


def test_resent_request_id_is_rejected_before_capture_and_creates_no_new_event(tmp_path):
    proc = spawn_production_agent()
    try:
        _send(proc, dict(jsonrpc="2.0", id=1, method="initialize",
                          params=dict(protocolVersion=1, clientCapabilities={},
                                      clientInfo=dict(name="t", version="1"))))
        _recv(proc)
        _send(proc, dict(jsonrpc="2.0", id=2, method="session/new",
                          params=dict(cwd=str(tmp_path), mcpServers=[])))
        session_id = _recv(proc)["result"]["sessionId"]
        _recv(proc)  # available_commands_update notification (T010)

        _send(proc, dict(jsonrpc="2.0", id=3, method="session/prompt",
                          params=dict(sessionId=session_id, prompt=[dict(type="text", text="hello")])))
        first = _recv(proc)
        assert first["id"] == 3 and first["error"]["code"] == -32001  # durable capture already happened

        # Resend with the identical id=3: must be rejected before any
        # durable capture is attempted this time.
        _send(proc, dict(jsonrpc="2.0", id=3, method="session/prompt",
                          params=dict(sessionId=session_id, prompt=[dict(type="text", text="hello")])))
        second = _recv(proc)
        assert second["id"] == 3
        assert second["error"]["code"] == -32600
        assert second["error"]["message"] == "duplicate_request_id"
    finally:
        proc.stdin.close()
        proc.wait(timeout=15)
    assert proc.returncode == 0

    db = sqlite3.connect(tmp_path / ".cortex" / "cortex.sqlite3")
    assert db.execute("SELECT COUNT(*) FROM source_events").fetchone()[0] == 1
    assert db.execute("SELECT COUNT(*) FROM tasks").fetchone()[0] == 1


def test_different_request_id_identical_text_creates_a_new_source_event(tmp_path):
    proc = spawn_production_agent()
    try:
        _send(proc, dict(jsonrpc="2.0", id=1, method="initialize",
                          params=dict(protocolVersion=1, clientCapabilities={},
                                      clientInfo=dict(name="t", version="1"))))
        _recv(proc)
        _send(proc, dict(jsonrpc="2.0", id=2, method="session/new",
                          params=dict(cwd=str(tmp_path), mcpServers=[])))
        session_id = _recv(proc)["result"]["sessionId"]
        _recv(proc)  # available_commands_update notification (T010)

        _send(proc, dict(jsonrpc="2.0", id=3, method="session/prompt",
                          params=dict(sessionId=session_id, prompt=[dict(type="text", text="same text")])))
        first = _recv(proc)
        assert first["id"] == 3 and first["error"]["code"] == -32001

        # A different id with identical text is not a duplicate: it is
        # captured as a new, distinct source event on the same task.
        _send(proc, dict(jsonrpc="2.0", id=4, method="session/prompt",
                          params=dict(sessionId=session_id, prompt=[dict(type="text", text="same text")])))
        second = _recv(proc)
        assert second["id"] == 4 and second["error"]["code"] == -32001
    finally:
        proc.stdin.close()
        proc.wait(timeout=15)
    assert proc.returncode == 0

    db = sqlite3.connect(tmp_path / ".cortex" / "cortex.sqlite3")
    tasks = db.execute("SELECT id FROM tasks").fetchall()
    assert len(tasks) == 1
    events = db.execute("SELECT DISTINCT event_id FROM source_events WHERE task_id=?", (tasks[0][0],)).fetchall()
    assert len(events) == 2


# --- delta review fixes: id-identity collision, worker exception safety,
#     and a genuinely-in-flight duplicate id -----------------------------

class _RecordingTransport:
    """Test-only in-process stand-in: records every sent message."""

    def __init__(self):
        self.responses = []
        self.notifications = []
        self._lock = threading.Lock()

    def send_response(self, message):
        with self._lock:
            self.responses.append(message)

    def send_notification(self, message, timeout=None):
        with self._lock:
            self.notifications.append(message)
        return True

    def response_for(self, request_id):
        with self._lock:
            for message in self.responses:
                if message.get("id") == request_id:
                    return message
        return None


class _ReleasableRuntime:
    """Test-only runtime: start_prompt blocks until release() is called."""

    def __init__(self):
        self._release = threading.Event()

    def release(self):
        self._release.set()

    def start_prompt(self, task_id, session_id, content, emit_update, cancel_event):
        self._release.wait(10)
        emit_update("done")
        return StopReason.END_TURN

    def cancel(self, session_id):
        return None


def test_request_id_identity_distinguishes_equal_but_differently_typed_json_values():
    # Python considers 1000 == 1000.0 == True == 1; the JSON-RPC ids
    # 1000, 1000.0, "1000", true and 1 must never collide.
    assert _request_id_identity(1000) != _request_id_identity(1000.0)
    assert _request_id_identity(1000) != _request_id_identity("1000")
    assert _request_id_identity(1000.0) != _request_id_identity("1000")
    assert _request_id_identity(True) != _request_id_identity(1)
    assert _request_id_identity(False) != _request_id_identity(0)
    # Equal-and-same-type ids are still recognized as the identical id.
    assert _request_id_identity(1000) == _request_id_identity(1000)
    assert _request_id_identity("1000") == _request_id_identity("1000")


def test_track_request_id_does_not_alias_int_float_str_bool(tmp_path):
    agent = Agent(_ReleasableRuntime())
    for value in (1000, 1000.0, "1000", True, 1, False, 0):
        assert agent._track_request_id(value) is False, f"{value!r} incorrectly reported as already used"
    # Each is now recorded, so resubmitting any of them is a duplicate.
    for value in (1000, 1000.0, "1000", True, 1, False, 0):
        assert agent._track_request_id(value) is True, f"{value!r} was not remembered"


def test_run_prompt_replies_internal_error_when_store_lookup_raises(tmp_path, monkeypatch):
    """A failure before durable capture (for example in _store_for) must
    still reply -- otherwise the client hangs forever on this request id."""
    agent = Agent(_ReleasableRuntime())
    transport = _RecordingTransport()
    agent.bind_transport(transport)

    def _boom(project):
        raise RuntimeError("simulated failure before durable capture")

    monkeypatch.setattr(agent, "_store_for", _boom)
    session = dict(project=str(tmp_path), task_id=None, channel=None, active=None,
                    lock=threading.Lock())
    cancel_event = threading.Event()
    agent._run_prompt(request_id=7, session_id="acps_test", session=session,
                       text="hello", cancel_event=cancel_event)
    reply = transport.response_for(7)
    assert reply is not None, "no reply was sent at all -- the client would hang"
    assert reply["error"]["code"] == -32603
    assert "hello" not in json.dumps(reply)
    assert session["active"] is None  # cleanup still ran


def test_duplicate_id_while_first_prompt_genuinely_in_flight(tmp_path):
    runtime = _ReleasableRuntime()
    # T010's session/new now reads the connection registry/catalog (see
    # config_options.py); an isolated environ here (never the inherited
    # real os.environ, and never nested under the project workspace itself
    # -- connection storage must be user-scoped, never inside a project;
    # see registry._assert_not_nested_in_project_workspace) keeps this
    # in-process Agent from ever touching the real ~/.config/cortex or
    # ~/.local/state/cortex.
    fake_home = Path(tempfile.mkdtemp(prefix="cortex-acp-fake-home-"))
    environ = dict(HOME=str(fake_home), XDG_CONFIG_HOME=str(fake_home / "config"),
                   XDG_STATE_HOME=str(fake_home / "state"))
    agent = Agent(runtime, environ=environ)
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True

    result = agent._session_new(dict(cwd=str(tmp_path), mcpServers=[]))
    session_id = result["sessionId"]

    prompt_message = dict(jsonrpc="2.0", id=9, method="session/prompt",
                           params=dict(sessionId=session_id, prompt=[dict(type="text", text="go")]))
    agent.handle_message(prompt_message, None)
    # The worker thread is now genuinely blocked inside runtime.start_prompt
    # (on runtime._release), so this really is "still in flight", not just
    # "the id was recorded and nothing else has happened yet".
    with agent.sessions[session_id]["lock"]:
        assert agent.sessions[session_id]["active"] is not None

    agent.handle_message(prompt_message, None)  # identical id, resent
    duplicate_reply = transport.response_for(9)
    # Only one reply can carry id 9 so far: the duplicate rejection arrives
    # first since the original is still blocked.
    assert duplicate_reply is not None
    assert duplicate_reply["error"]["code"] == -32600
    assert duplicate_reply["error"]["message"] == "duplicate_request_id"

    runtime.release()
    for worker in agent._workers:
        worker.join(timeout=10)

    final_replies = [message for message in transport.responses if message.get("id") == 9]
    assert len(final_replies) == 2
    completed = [message for message in final_replies if "result" in message]
    assert len(completed) == 1
    assert completed[0]["result"]["stopReason"] == "end_turn"

    db = sqlite3.connect(tmp_path / ".cortex" / "cortex.sqlite3")
    assert db.execute("SELECT COUNT(*) FROM source_events").fetchone()[0] == 1
