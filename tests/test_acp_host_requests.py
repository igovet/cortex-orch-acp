"""HostRequester: tracked outbound agent->client requests on the incoming
ACP stdio transport, and its wiring into ``acp.agent.Agent`` (T014)."""
from __future__ import annotations

import io
import json
import os
import threading
import time

import pytest

from cortex_runtime.acp.agent import Agent
from cortex_runtime.acp.host_requests import HostRequestClosed, HostRequester, HostRequestTimeout
from cortex_runtime.acp.transport import StdioTransport

from acp_fakes import FakeRuntime


class _RecordingStdout:
    def __init__(self):
        self.lines = []
        self._lock = threading.Lock()

    def write(self, data):
        with self._lock:
            for line in data.splitlines():
                if line:
                    self.lines.append(json.loads(line.decode("utf-8")))

    def flush(self):
        pass


def _requests_sent(stdout):
    return [msg for msg in stdout.lines if "method" in msg]


def test_request_and_matching_reply_round_trips():
    stdout = _RecordingStdout()
    transport = StdioTransport(lambda *_: None, raw_stdout=stdout, raw_stdin=io.BytesIO(b""))
    transport.start()
    try:
        requester = HostRequester(transport)
        result_holder = {}

        def call():
            result_holder["value"] = requester.request("session/request_permission", {"x": 1}, timeout=5.0)

        worker = threading.Thread(target=call)
        worker.start()
        time.sleep(0.1)
        sent = _requests_sent(stdout)
        assert len(sent) == 1
        request_id = sent[0]["id"]
        assert isinstance(request_id, str) and request_id.startswith("hr_")
        requester.handle_response({"jsonrpc": "2.0", "id": request_id, "result": {"outcome": {"outcome": "cancelled"}}})
        worker.join(timeout=2.0)
        assert result_holder["value"] == {"outcome": {"outcome": "cancelled"}}
    finally:
        transport.stop_writer()


def test_timeout_raises_and_late_reply_is_dropped():
    stdout = _RecordingStdout()
    transport = StdioTransport(lambda *_: None, raw_stdout=stdout, raw_stdin=io.BytesIO(b""))
    transport.start()
    try:
        requester = HostRequester(transport)
        with pytest.raises(HostRequestTimeout):
            requester.request("session/request_permission", {}, timeout=0.1)
        sent = _requests_sent(stdout)
        request_id = sent[0]["id"]
        # A late reply after timeout must be dropped, not raise/crash.
        assert requester.handle_response({"jsonrpc": "2.0", "id": request_id, "result": {}}) is True
    finally:
        transport.stop_writer()


def test_duplicate_reply_is_dropped_not_double_delivered():
    stdout = _RecordingStdout()
    transport = StdioTransport(lambda *_: None, raw_stdout=stdout, raw_stdin=io.BytesIO(b""))
    transport.start()
    try:
        requester = HostRequester(transport)
        result_holder = {}

        def call():
            result_holder["value"] = requester.request("session/request_permission", {}, timeout=5.0)

        worker = threading.Thread(target=call)
        worker.start()
        time.sleep(0.1)
        request_id = _requests_sent(stdout)[0]["id"]
        assert requester.handle_response({"jsonrpc": "2.0", "id": request_id, "result": {"first": True}})
        worker.join(timeout=2.0)
        assert result_holder["value"] == {"first": True}
        # A second, duplicate response for the same id: dropped, no error.
        assert requester.handle_response({"jsonrpc": "2.0", "id": request_id, "result": {"second": True}}) is True
    finally:
        transport.stop_writer()


def test_unrelated_id_namespace_is_not_claimed():
    transport = StdioTransport(lambda *_: None, raw_stdout=_RecordingStdout(), raw_stdin=io.BytesIO(b""))
    transport.start()
    try:
        requester = HostRequester(transport)
        assert requester.handle_response({"jsonrpc": "2.0", "id": 7, "result": {}}) is False
        assert requester.handle_response({"jsonrpc": "2.0", "id": "not-ours", "result": {}}) is False
    finally:
        transport.stop_writer()


def test_close_resolves_every_pending_call_as_closed():
    stdout = _RecordingStdout()
    transport = StdioTransport(lambda *_: None, raw_stdout=stdout, raw_stdin=io.BytesIO(b""))
    transport.start()
    try:
        requester = HostRequester(transport)
        outcome = {}

        def call():
            try:
                requester.request("session/request_permission", {}, timeout=5.0)
            except HostRequestClosed:
                outcome["closed"] = True

        worker = threading.Thread(target=call)
        worker.start()
        time.sleep(0.1)
        requester.close()
        worker.join(timeout=2.0)
        assert outcome.get("closed") is True
        with pytest.raises(HostRequestClosed):
            requester.request("session/request_permission", {}, timeout=1.0)
    finally:
        transport.stop_writer()


def test_agent_routes_a_bare_response_to_its_host_requester():
    """A message with an id but no method (a response) is routed to
    Agent.host_requester, never mis-dispatched as an unknown request/
    notification (see agent.Agent.handle_message)."""
    agent = Agent(FakeRuntime(), environ={})
    stdout = _RecordingStdout()
    read_fd, write_fd = os.pipe()
    stdin = os.fdopen(read_fd, "rb")
    transport = StdioTransport(agent.handle_message, raw_stdout=stdout, raw_stdin=stdin)
    agent.bind_transport(transport)
    transport.start()
    try:
        result_holder = {}

        def call():
            result_holder["value"] = agent.host_requester.request("session/request_permission", {}, timeout=5.0)

        worker = threading.Thread(target=call)
        worker.start()
        time.sleep(0.1)
        request_id = _requests_sent(stdout)[0]["id"]
        agent.handle_message({"jsonrpc": "2.0", "id": request_id, "result": {"outcome": {"outcome": "cancelled"}}}, None)
        worker.join(timeout=2.0)
        assert result_holder["value"] == {"outcome": {"outcome": "cancelled"}}
    finally:
        transport.stop_writer()
        os.close(write_fd)
        stdin.close()
