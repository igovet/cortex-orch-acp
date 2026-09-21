"""Bounded, backpressure-aware ndjson transport for the ACP stdio agent.

One line is one JSON-RPC 2.0 object (batches -- top-level JSON arrays --
are not supported). Reading happens on a dedicated thread so the agent can
keep processing an inbound ``session/cancel`` notification while a prompt
worker thread is still running a turn (see ``agent.Agent``). Writing happens
on a second dedicated thread behind a bounded queue:

- A *response* (the reply to a request) is enqueued with ``send_response``,
  which blocks until there is room. Responses are never dropped.
- A *notification* (``session/update``) is enqueued with
  ``send_notification`` under a bounded timeout. If the queue stays full for
  the whole timeout the notification is dropped and ``send_notification``
  returns ``False``; callers use that to cancel a runaway prompt against a
  slow/stalled client rather than growing memory without bound or blocking
  the writer thread forever.
"""
from __future__ import annotations

import json
import queue
import sys
import threading
import time

MAX_FRAME_BYTES = 2_000_000
MAX_JSON_DEPTH = 64
OUTBOUND_QUEUE_MAXSIZE = 1024
DEFAULT_UPDATE_TIMEOUT = 5.0

# Sentinel handed to the inbound handler once the reader thread reaches EOF.
EOF = object()


class _DuplicateKey(ValueError):
    """Raised by the ``object_pairs_hook`` when a JSON object repeats a key."""


def _reject_duplicates(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise _DuplicateKey(key)
        result[key] = value
    return result


class FrameError(Exception):
    """One malformed inbound frame; ``code``/``message`` become a JSON-RPC error.

    ``request_id`` is the id recovered from an otherwise-malformed frame
    when one could safely be identified (never guessed); it stays ``None``
    when the frame could not be parsed far enough to know an id, per the
    protocol's requirement to reply with ``id: null`` in that case.
    """

    def __init__(self, code, message, request_id=None):
        super().__init__(message)
        self.code = code
        self.message = message
        self.request_id = request_id


def _exceeds_json_depth(text, max_depth):
    """True if ``text`` contains ``{``/``[`` nesting deeper than ``max_depth``.

    A cheap, iterative, single-pass pre-scan (no recursion, so it cannot
    itself blow the interpreter stack) that only tracks bracket/brace
    nesting depth outside of JSON string literals (honoring ``\\"`` escapes
    so a quote inside a string never toggles string state incorrectly).
    Run *before* ``json.loads`` so a deeply-nested "bomb" (for example
    ``"["*100000 + "]"*100000``) is rejected without ever invoking the
    recursive decoder that would otherwise raise ``RecursionError`` and,
    left uncaught, kill the reader thread.
    """
    depth = 0
    in_string = False
    escape = False
    for ch in text:
        if in_string:
            if escape:
                escape = False
            elif ch == "\\":
                escape = True
            elif ch == '"':
                in_string = False
            continue
        if ch == '"':
            in_string = True
        elif ch == "{" or ch == "[":
            depth += 1
            if depth > max_depth:
                return True
        elif ch == "}" or ch == "]":
            depth -= 1
    return False


def parse_frame(raw_line: bytes):
    """Parse one newline-stripped inbound frame. Never echoes its content."""
    if len(raw_line) > MAX_FRAME_BYTES:
        raise FrameError(-32700, "Frame exceeds the maximum size of 2,000,000 bytes.")
    try:
        text = raw_line.decode("utf-8")
    except UnicodeDecodeError:
        raise FrameError(-32700, "Frame is not valid UTF-8.") from None
    if _exceeds_json_depth(text, MAX_JSON_DEPTH):
        raise FrameError(-32700, f"Frame exceeds the maximum nesting depth of {MAX_JSON_DEPTH}.")
    try:
        message = json.loads(text, object_pairs_hook=_reject_duplicates)
    except _DuplicateKey:
        raise FrameError(-32600, "Duplicate key in a JSON-RPC object.") from None
    # RecursionError/ValueError/MemoryError are defense-in-depth beyond the
    # depth pre-scan above (which already rejects every input that could
    # reach the decoder's recursion limit): never let a single malformed
    # frame propagate out of parsing and kill the reader thread.
    except (json.JSONDecodeError, RecursionError, ValueError, MemoryError):
        raise FrameError(-32700, "Frame is not valid JSON.") from None
    if not isinstance(message, dict):
        raise FrameError(-32600, "Batches are not supported; send one JSON-RPC object per line.")
    if message.get("jsonrpc") != "2.0":
        raise FrameError(-32600, "Missing or invalid \"jsonrpc\": \"2.0\".", message.get("id"))
    if "method" in message and not isinstance(message["method"], str):
        raise FrameError(-32600, "\"method\" must be a string.", message.get("id"))
    return message


class StdioTransport:
    """Reader/writer threads around one binary stdin/stdout pair.

    The caller is responsible for redirecting ``sys.stdout`` away from the
    handle passed as ``raw_stdout`` *before* anything else can write to it
    (see ``agent.main``): stdout is exclusively protocol traffic written by
    this class's writer thread.
    """

    def __init__(self, inbound_handler, raw_stdout, raw_stdin=None,
                 update_timeout=DEFAULT_UPDATE_TIMEOUT, on_writer_closed=None):
        self._raw_stdout = raw_stdout
        self._raw_stdin = raw_stdin if raw_stdin is not None else sys.stdin.buffer
        self._inbound_handler = inbound_handler
        self._update_timeout = update_timeout
        # Called (at most once, from the writer thread) when the writer
        # stops because a write actually failed -- never on a normal
        # `stop_writer()` shutdown. This is the "writer-closed" signal a
        # caller (see `agent.main`, `engine.client.EngineClient`) treats
        # like inbound EOF: without it, an incoming ACP agent whose stdout
        # reader has gone away (peer closed its read end) but whose stdin
        # stays open would otherwise wait forever in `join_reader()` for a
        # read-side EOF that may never come -- a wedged process able to
        # neither reply nor exit (T012 review item 1).
        self._on_writer_closed = on_writer_closed
        self._outbound: "queue.Queue[bytes | None]" = queue.Queue(maxsize=OUTBOUND_QUEUE_MAXSIZE)
        self._closed = threading.Event()
        self._writer_closed_event = threading.Event()
        self._reader_eof_event = threading.Event()
        self._reader_thread = threading.Thread(target=self._read_loop, name="acp-reader", daemon=True)
        self._writer_thread = threading.Thread(target=self._write_loop, name="acp-writer", daemon=True)

    def start(self):
        self._writer_thread.start()
        self._reader_thread.start()
        return self

    def join_reader(self, timeout=None):
        self._reader_thread.join(timeout)

    def writer_closed(self) -> bool:
        """True once the writer has stopped because a write actually
        failed (never true after a normal `stop_writer()`)."""
        return self._writer_closed_event.is_set()

    def wait_for_shutdown_signal(self, timeout=None):
        """Block until the reader hits EOF or the writer stops because a
        write failed, whichever happens first; return ``"reader_eof"``,
        ``"writer_closed"``, or ``None`` on timeout.

        Polls at a short, bounded interval rather than requiring a single
        primitive that can wait on two independent ``threading.Event``
        objects at once (the stdlib has none); the poll interval only
        bounds how promptly this returns after either event fires, not
        whether it ever does.
        """
        deadline = None if timeout is None else time.monotonic() + timeout
        poll_interval = 0.05
        while True:
            if self._reader_eof_event.is_set():
                return "reader_eof"
            if self._writer_closed_event.is_set():
                return "writer_closed"
            if deadline is not None:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return None
                poll_interval = min(poll_interval, remaining)
            self._reader_eof_event.wait(poll_interval)

    def stop_writer(self):
        """Signal the writer to flush and exit after its queue drains.

        A no-op (beyond the join, which returns immediately) if the writer
        already stopped on its own after a write failure. Also safe to call
        *from* the writer thread itself (for example from an
        ``on_writer_closed`` callback): a thread can never join itself, so
        that case skips the join rather than raising.
        """
        if not self._writer_closed_event.is_set():
            self._outbound.put(None)
        if threading.current_thread() is not self._writer_thread:
            self._writer_thread.join()

    def send_response(self, message: dict, *, timeout=None) -> bool:
        """Enqueue a response/error; blocks until there is room.

        The boolean reports whether the transport accepted the frame.  A
        caller that is relaying a request to the host must be able to fail
        immediately when the writer has already closed; waiting for its
        request timeout would only turn a known disconnect into a misleading
        slow denial.
        """
        return self._enqueue(message, timeout=timeout)

    def send_notification(self, message: dict, *, timeout=None) -> bool:
        """Enqueue a notification under a bounded timeout.

        Returns ``False`` (without raising) if the outbound queue stayed
        full for the whole timeout -- the caller (see ``agent.Agent``)
        treats that as "this prompt's updates cannot be delivered" and
        cancels the prompt rather than growing memory unboundedly or
        blocking forever.
        """
        return self._enqueue(message, timeout=self._update_timeout if timeout is None else timeout)

    def _enqueue(self, message, *, timeout) -> bool:
        if self._closed.is_set():
            return False
        body = json.dumps(message, ensure_ascii=False).encode("utf-8") + b"\n"
        try:
            self._outbound.put(body, timeout=timeout)
            return True
        except queue.Full:
            return False

    def _write_loop(self):
        while True:
            body = self._outbound.get()
            if body is None:
                break
            try:
                self._raw_stdout.write(body)
                self._raw_stdout.flush()
            except (BrokenPipeError, OSError, ValueError) as error:
                # The peer or the underlying pipe is gone (a closed pipe
                # raises BrokenPipeError/OSError; writing to an
                # already-closed file object raises ValueError). Stop the
                # writer loop quietly -- one stderr line, no unhandled
                # exception escaping this thread -- rather than looping
                # through the remaining queued items only to fail on each
                # one. `_closed` is set first so `_enqueue` (send_response/
                # send_notification) stops accepting new work immediately.
                self._closed.set()
                self._writer_closed_event.set()
                print(f"acp: writer stopped after {type(error).__name__} (peer or pipe closed)",
                      file=sys.stderr, flush=True)
                if self._on_writer_closed is not None:
                    try:
                        self._on_writer_closed()
                    except Exception as callback_error:  # noqa: BLE001 - never let this kill the writer thread
                        print(f"acp: on_writer_closed callback raised {type(callback_error).__name__}",
                              file=sys.stderr, flush=True)
                break

    def _read_loop(self):
        try:
            while True:
                raw_line = self._raw_stdin.readline(MAX_FRAME_BYTES + 1)
                if not raw_line:
                    break
                if not raw_line.endswith(b"\n"):
                    # Oversized (or final unterminated) frame: drain forward
                    # without ever accumulating the offending bytes, then
                    # reply without echoing any content, id unknown (null).
                    while raw_line and not raw_line.endswith(b"\n"):
                        raw_line = self._raw_stdin.readline(MAX_FRAME_BYTES + 1)
                    self._inbound_handler(
                        None, FrameError(-32700, "Frame exceeds the maximum size of 2,000,000 bytes."))
                    continue
                line = raw_line[:-1]
                if not line:
                    continue
                try:
                    message = parse_frame(line)
                except FrameError as error:
                    self._inbound_handler(None, error)
                    continue
                except Exception as error:
                    # Defense-in-depth: parse_frame already catches every
                    # known malformed-input case (including RecursionError
                    # from a nesting bomb -- see _exceeds_json_depth); this
                    # exists so that even an unanticipated failure never
                    # kills the reader thread on a single bad frame.
                    print(f"acp: unexpected frame-parsing error: {type(error).__name__}",
                          file=sys.stderr, flush=True)
                    self._inbound_handler(None, FrameError(-32700, "Frame could not be parsed."))
                    continue
                try:
                    self._inbound_handler(message, None)
                except Exception as error:
                    print(f"acp: unexpected error dispatching a message: {type(error).__name__}",
                          file=sys.stderr, flush=True)
        finally:
            self._reader_eof_event.set()
            self._inbound_handler(EOF, None)
