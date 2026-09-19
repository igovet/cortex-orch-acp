"""An unexpected runtime failure must say what failed.

Found live (2026-09-19): a ``session/prompt`` died inside the runtime and
left nothing behind but "The runtime failed while processing this prompt."
-- no exception class, no source line, nothing on stderr, and an
observation reading only ``runtime_error``. There was no way to tell a
storage problem from a routing one without adding a print and reproducing
a paid run.

The class name and the failing source line are not user content and carry
no secret. The exception *message* deliberately stays out: it can quote a
path or a value.
"""
from __future__ import annotations

import threading

from cortex_runtime.acp.agent import Agent


class _ExplodingRuntime:
    def start_prompt(self, task_id, session_id, content, emit_update, cancel_event, **kwargs):
        raise RuntimeError("secret /home/somebody/private/path detail")

    def cancel(self, session_id):
        pass


class _RecordingTransport:
    def __init__(self):
        self.responses = []
        self.notifications = []

    def send_response(self, message):
        self.responses.append(message)

    def send_notification(self, message):
        self.notifications.append(message)
        return True


def test_the_error_names_the_exception_class_and_where_it_came_from(tmp_path, monkeypatch, capsys):
    environ = dict(HOME=str(tmp_path / "home"), XDG_CONFIG_HOME=str(tmp_path / "cfg"),
                   XDG_STATE_HOME=str(tmp_path / "state"))
    for value in environ.values():
        (tmp_path / value).mkdir(parents=True, exist_ok=True)
    project = tmp_path / "project"
    project.mkdir()

    agent = Agent(_ExplodingRuntime(), environ=environ)
    transport = _RecordingTransport()
    agent.bind_transport(transport)
    agent.initialized = True
    agent.client_capabilities = {}
    session_id = agent._session_new(dict(cwd=str(project), mcpServers=[]))["sessionId"]

    agent._session_prompt(7, dict(sessionId=session_id, prompt=[{"type": "text", "text": "go"}]))
    for worker in list(agent._workers):
        worker.join(timeout=10)

    reply = next(m for m in transport.responses if m.get("id") == 7)
    failure = reply["error"]["data"]["failure"]
    assert failure.startswith("RuntimeError at ")
    assert ".py:" in failure  # a concrete file:line, not just a class name
    # The message is never carried: it can quote a path or a value.
    assert "private/path" not in str(reply)
    assert "RuntimeError" in capsys.readouterr().err
