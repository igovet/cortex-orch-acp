"""HostRequester: tracked outbound agent-to-client requests on the incoming
ACP stdio transport (T014).

Cortex's incoming ACP agent (``acp.agent.Agent``) is ordinarily only a
JSON-RPC *server* on this transport (host -> agent requests/notifications).
This module is the one place it becomes a *client* too: an explicit,
request/response call *to* the connected host -- for example relaying a
downstream engine's ``session/request_permission``/``fs/*`` request (see
``engine.permissions.PermissionBridge``, ``engine.fs_bridge.HostFsForwarder``)
-- tracked the same way ``engine.client.EngineClient`` tracks its own
outbound calls to a downstream engine, but in a disjoint id namespace
(every id is a string prefixed ``"hr_"``) so a ``HostRequester`` id can
never collide with, or be confused for, any request id the host itself
chose for a request *to* this agent.
"""
from __future__ import annotations

import collections
import itertools
import threading
import time


class HostRequestClosed(Exception):
    """The transport reached EOF or its writer closed before a response to
    this call arrived."""


class HostRequestTimeout(Exception):
    """No response arrived within the call's timeout."""


class HostRequestError(Exception):
    """The host replied with an explicit JSON-RPC error."""

    def __init__(self, code, message):
        super().__init__(message)
        self.code = code
        self.message = message


class _Pending:
    __slots__ = ("event", "response", "timed_out")

    def __init__(self):
        self.event = threading.Event()
        self.response = None
        self.timed_out = False


class HostRequester:
    """One per incoming ACP connection (see ``acp.agent.Agent.bind_transport``)."""

    # Late replies are retained only long enough to preserve the distinction
    # between an expected late response and an unrelated/forged id.  This is
    # deliberately bounded: a dead host must not turn repeated timeouts into
    # an ever-growing in-memory request ledger.
    _MAX_TIMEOUT_TOMBSTONES = 1024

    def __init__(self, transport, *, logs=None):
        self._transport = transport
        self._logs = logs
        self._lock = threading.Lock()
        self._pending: dict[str, _Pending] = {}
        self._timed_out = collections.OrderedDict()
        self._counter = itertools.count(1)
        self._closed = threading.Event()

    def request(self, method: str, params, *, timeout: float = 30.0):
        if self._closed.is_set():
            raise HostRequestClosed(method)
        request_id = f"hr_{next(self._counter)}"
        entry = _Pending()
        with self._lock:
            self._pending[request_id] = entry
        body = dict(jsonrpc="2.0", id=request_id, method=method)
        if params is not None:
            body["params"] = params
        # A host request is a response-shaped frame on the incoming ACP
        # connection, but it still has a live future behind it.  If the
        # writer is already closed, do not leave that future pending until
        # the permission timeout: fail closed immediately and remove the
        # request from the in-flight table.
        deadline = time.monotonic() + max(0.0, float(timeout))
        remaining = max(0.0, deadline - time.monotonic())
        sent = self._transport.send_response(body, timeout=remaining)
        if sent is False:
            with self._lock:
                self._pending.pop(request_id, None)
            writer_closed = getattr(self._transport, "writer_closed", None)
            if callable(writer_closed) and writer_closed():
                raise HostRequestClosed(method)
            raise HostRequestTimeout(method)
        got = entry.event.wait(max(0.0, deadline - time.monotonic()))
        if not got:
            with self._lock:
                current = self._pending.get(request_id)
                if current is entry and not entry.event.is_set():
                    entry.timed_out = True
                    self._pending.pop(request_id, None)
                    self._timed_out[request_id] = None
                    self._timed_out.move_to_end(request_id)
                    while len(self._timed_out) > self._MAX_TIMEOUT_TOMBSTONES:
                        self._timed_out.popitem(last=False)
                    raise HostRequestTimeout(method)
                # A response won the timeout race while the caller was
                # acquiring the lock.  Consume it below instead of falsely
                # reporting a timeout.
                self._pending.pop(request_id, None)
        response = entry.response
        with self._lock:
            self._pending.pop(request_id, None)
        if response.get("__closed__"):
            raise HostRequestClosed(method)
        if "error" in response:
            error = response["error"] or {}
            raise HostRequestError(error.get("code"), error.get("message"))
        return response.get("result")

    def handle_response(self, message: dict) -> bool:
        """Call for every inbound message that has an ``id`` but no
        ``method`` (a response, not a request). Returns ``True`` if this
        requester recognized/consumed it (whether or not it matched a live
        pending call -- an unknown/late/duplicate id is still "consumed":
        observed and dropped, never surfaced as anything else) or ``False``
        if ``id`` is not in this requester's own ``"hr_"`` namespace at
        all (a plain string id happens not to be one of ours)."""
        request_id = message.get("id")
        if not isinstance(request_id, str) or not request_id.startswith("hr_"):
            return False
        with self._lock:
            entry = self._pending.get(request_id)
            already_set = entry.event.is_set() if entry is not None else None
        if entry is None:
            with self._lock:
                was_timed_out = request_id in self._timed_out
                if was_timed_out:
                    self._timed_out.pop(request_id, None)
            if was_timed_out:
                if self._logs is not None:
                    self._logs.observe("host_request_late_or_duplicate_response", request_id=request_id)
                return True
            if self._logs is not None:
                self._logs.observe("host_request_unknown_response_id", request_id=request_id)
            return True
        if already_set or entry.timed_out:
            with self._lock:
                self._pending.pop(request_id, None)
            if self._logs is not None:
                self._logs.observe("host_request_late_or_duplicate_response", request_id=request_id)
            return True
        entry.response = message
        entry.event.set()
        return True

    def close(self) -> None:
        """Resolve every still-pending call as closed; called on inbound
        EOF or a write failure (see ``acp.agent.Agent._handle_eof``/
        ``handle_writer_closed``)."""
        self._closed.set()
        with self._lock:
            pending = list(self._pending.items())
        for _request_id, entry in pending:
            if not entry.event.is_set():
                entry.response = {"__closed__": True}
                entry.event.set()
