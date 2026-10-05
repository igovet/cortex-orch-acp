"""JSON-RPC client for one outgoing engine connection.

Reuses ``acp.transport.StdioTransport`` unmodified (see the T012 task
card's "minimal generalization ... ONLY if needed" instruction: none was
needed -- the transport only ever calls ``.write``/``.flush`` on the object
passed as ``raw_stdout`` and ``.readline`` on ``raw_stdin``, and a launched
subprocess's ``stdin``/``stdout`` pipes already satisfy that), wired onto a
launched subprocess's own stdin (write) / stdout (read) pipes instead of
this process's own stdio.

Outbound requests are tracked in a per-call ``_Pending`` record keyed by
this engine instance's own ``CorrelationTables.next_request_id()``
sequence. A timed-out call's record is not discarded until a response
actually arrives (or the transport reaches EOF): a *late* reply after the
caller already gave up is recognized (not treated as an unknown id) and
dropped with an observation, per the T012 task card's "late replies after
timeout logged and dropped" and "duplicate responses dropped with
observation" requirements.
"""
from __future__ import annotations

import threading

from ..acp.transport import EOF, StdioTransport
from .correlation import CorrelationTables


class EngineTransportClosed(Exception):
    """The engine's stdout reached EOF. The process may or may not have
    exited yet -- see ``process.EngineProcess.wait_exited``; this alone
    never frees capacity."""


class EngineTimeout(Exception):
    """A call did not receive a response within its timeout. The process is
    not assumed dead on this alone and capacity is not released."""


class EngineProtocolError(Exception):
    """An explicit JSON-RPC error response from the engine."""

    def __init__(self, code, message, data=None):
        super().__init__(message)
        self.code = code
        self.message = message
        self.data = data


class _Pending:
    __slots__ = ("event", "response", "timed_out")

    def __init__(self):
        self.event = threading.Event()
        self.response = None
        self.timed_out = False


class EngineClient:
    """One JSON-RPC connection to one launched engine process.

    ``on_update`` is called (on the transport's reader thread) with a
    ``session/update`` notification's ``params``. ``on_client_request`` is
    called with ``(method, params, request_id)`` for every agent-to-client
    request; it is responsible for calling ``reply_error``/``reply_result``
    itself (see ``stub_handlers`` for the production handler, which always
    refuses). ``on_unknown_notification``/``logs`` are optional observation
    hooks.
    """

    def __init__(
        self,
        process,
        *,
        correlation: CorrelationTables,
        on_update,
        on_client_request,
        on_unknown_notification=None,
        logs=None,
        trace_context=None,
    ):
        self._process = process
        self.correlation = correlation
        self._on_update = on_update
        self._on_client_request = on_client_request
        self._on_unknown_notification = on_unknown_notification
        self._logs = logs
        self._pending: dict[object, _Pending] = {}
        self._lock = threading.Lock()
        self._closed = threading.Event()
        # T012 review item 1: a write failure (the engine's stdin reader
        # gone, e.g. its process already exited or closed stdin) is treated
        # identically to inbound EOF -- both mean this transport can never
        # make progress again, so every pending call must be resolved the
        # same way (never left to hang) regardless of which direction
        # actually detected the break first.
        self.transport = StdioTransport(
            self._on_message, raw_stdout=process.process.stdin, raw_stdin=process.process.stdout,
            on_writer_closed=self._handle_writer_closed,
            trace_channel="downstream_acp", trace_context=trace_context,
        )

    def start(self) -> "EngineClient":
        self.transport.start()
        return self

    # -- inbound dispatch (transport reader thread) ----------------------

    def _handle_writer_closed(self) -> None:
        """Called on the transport's writer thread when a write actually
        failed; resolves every pending call exactly like inbound EOF."""
        self._resolve_pending_as_closed()

    def _resolve_pending_as_closed(self) -> None:
        self._closed.set()
        with self._lock:
            pending = list(self._pending.items())
        for _request_id, entry in pending:
            if not entry.event.is_set():
                entry.response = {"__eof__": True}
                entry.event.set()
        if self._process is not None:
            self._process.observe_transport_eof()
        # T012 review item 2: stop cleanly on our own, rather than relying
        # on every caller remembering to call `close()` -- either
        # direction of the transport breaking (reader EOF or a write
        # failure) means the other direction can never make useful
        # progress either, so both threads must wind down together. This
        # is what makes cleanup order-independent: whichever of
        # `EngineProcess.release()`/`EngineClient.close()` a caller calls
        # (or forgets to call), a dead/closing process's client never
        # leaves its writer thread parked forever.
        self.transport.stop_writer()

    def _on_message(self, message, error):
        if message is EOF:
            self._resolve_pending_as_closed()
            return
        if error is not None:
            if self._logs is not None:
                self._logs.observe("malformed_peer", code=error.code, message=error.message)
            return
        if "id" in message and ("result" in message or "error" in message):
            self._handle_response(message)
            return
        method = message.get("method")
        if not isinstance(method, str):
            return
        if "id" in message:
            self._on_client_request(method, message.get("params"), message["id"])
            return
        if method == "session/update":
            self._on_update(message.get("params"))
        else:
            if self._logs is not None:
                self._logs.observe("unknown_notification", method=method)
            if self._on_unknown_notification is not None:
                self._on_unknown_notification(method, message.get("params"))

    def _handle_response(self, message):
        request_id = message["id"]
        with self._lock:
            entry = self._pending.get(request_id)
            if entry is None:
                already_set = None
            else:
                already_set = entry.event.is_set()
        if entry is None:
            if self._logs is not None:
                self._logs.observe("unknown_response_id", request_id=request_id)
            return
        # A reply for an id whose caller already gave up (timed out) or
        # already received one response (a genuine duplicate) is recognized
        # -- never mistaken for a fresh legitimate response -- and dropped
        # with an observation. A caller only ever waits on ``entry.event``
        # once, so ``already_set`` alone would miss the "timed out, then a
        # later reply arrives" case (nobody ever set the event on timeout);
        # ``entry.timed_out`` covers that.
        if already_set or entry.timed_out:
            with self._lock:
                self._pending.pop(request_id, None)
            if self._logs is not None:
                self._logs.observe(
                    "late_or_duplicate_response", request_id=request_id, timed_out=entry.timed_out,
                )
            return
        entry.response = message
        entry.event.set()

    # -- outbound ----------------------------------------------------

    def call(self, method, params=None, *, timeout=30.0):
        if self._closed.is_set():
            raise EngineTransportClosed(method)
        request_id = self.correlation.next_request_id()
        entry = _Pending()
        with self._lock:
            self._pending[request_id] = entry
        body = dict(jsonrpc="2.0", id=request_id, method=method)
        if params is not None:
            body["params"] = params
        # Requests must never be silently dropped, same as a response on
        # the incoming side: use the transport's non-droppable queue.
        self.transport.send_response(body)
        got = entry.event.wait(timeout)
        if not got:
            entry.timed_out = True
            if self._process is not None:
                self._process.observe_timeout_alive()
            raise EngineTimeout(method)
        response = entry.response
        with self._lock:
            self._pending.pop(request_id, None)
        if response.get("__eof__"):
            raise EngineTransportClosed(method)
        if "error" in response:
            err = response["error"]
            raise EngineProtocolError(err.get("code"), err.get("message"), err.get("data"))
        return response.get("result")

    def notify(self, method, params=None) -> None:
        body = dict(jsonrpc="2.0", method=method)
        if params is not None:
            body["params"] = params
        self.transport.send_notification(body)

    def reply_error(self, request_id, code, message, data=None) -> None:
        payload = dict(code=code, message=message)
        if data is not None:
            payload["data"] = data
        self.transport.send_response(dict(jsonrpc="2.0", id=request_id, error=payload))

    def reply_result(self, request_id, result) -> None:
        self.transport.send_response(dict(jsonrpc="2.0", id=request_id, result=result))

    def close(self) -> None:
        self.transport.stop_writer()
