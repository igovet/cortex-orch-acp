"""Host-side Cortex MCP broker and stdio relay."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import secrets
import socket
import subprocess
import sys
import tempfile
import threading

MAX_HANDSHAKE_BYTES = 128
IO_CHUNK = 64 * 1024
_BASE_REPORT_TOOLS = ("create_draft", "write_report")
_WORKER_REPORT_TOOL = "publish_result"


def _schema_digest(schema) -> str:
    """Return a stable digest without retaining or logging the schema."""
    try:
        encoded = json.dumps(
            schema, sort_keys=True, ensure_ascii=False, separators=(",", ":"),
        ).encode("utf-8")
    except (TypeError, ValueError):
        encoded = repr(type(schema).__name__).encode("ascii")
    return hashlib.sha256(encoded).hexdigest()


class McpBroker:
    """Keep task-channel credentials in the runtime's MCP child process."""

    def __init__(self, channel_credentials, *, python_executable: str, server_script: str,
                 observe=None):
        if not channel_credentials or len(channel_credentials) != 3:
            raise ValueError("channel_credentials are required for an MCP broker")
        self._credentials = tuple(channel_credentials)
        self._python = python_executable
        self._script = server_script
        self._observe = observe
        self._ready = threading.Event()
        self._closed = threading.Event()
        self._failure = None
        # This is deliberately metadata-only.  The child response is forwarded
        # to the engine, but no tool description, argument value, or schema is
        # retained by the host-side broker.
        self._tools_receipt = None
        # This token is passed as an argparse value after ``--relay``. A raw
        # URL-safe token can begin with '-', which argparse treats as another
        # option: the relay exits 2 before it can connect. Prefix a literal
        # letter without reducing the random portion's entropy.
        self._token = "c" + secrets.token_urlsafe(32)
        self._tmp = tempfile.TemporaryDirectory(prefix="cortex-mcp-")
        self.endpoint = os.path.join(self._tmp.name, "broker.sock")
        self._listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._listener.bind(self.endpoint)
        os.chmod(self.endpoint, 0o600)
        self._listener.listen(1)
        self._listener.settimeout(0.25)
        self._stop = threading.Event()
        self._child = None
        self._thread = threading.Thread(target=self._serve, daemon=True, name="cortex-mcp-broker")
        self._thread.start()

    @property
    def relay_token(self) -> str:
        return self._token

    def _event(self, event, **fields):
        # Only locally generated enum/status fields reach persistent logs. Never
        # copy MCP frames, exception messages, process environment or stderr.
        if self._observe is not None:
            self._observe(event, **fields)

    def readiness(self, timeout: float) -> str:
        """Observe the *engine's* relay reaching a successful tools/list.

        No separate synthetic connection can satisfy this signal. Some engines
        defer MCP startup until the first prompt, so a timeout is explicitly
        inconclusive rather than evidence that the server failed.
        """
        if self._failure is not None:
            return self._failure
        if self._ready.wait(timeout):
            return "ready"
        if self._closed.is_set():
            return self._failure or "connection_closed"
        return "not_observed"

    @property
    def tools_receipt(self) -> dict:
        """Return the bounded, in-memory receipt for this broker's MCP server.

        A broker serves one trusted Cortex channel.  The receipt therefore
        applies to every session using this broker, while callers attach their
        own session id when exposing it as session diagnostics.  ``None`` is
        intentionally represented as ``not_observed``: an engine may defer
        ``tools/list`` until after the first prompt.
        """
        receipt = self._tools_receipt
        if receipt is None:
            return {
                "state": "not_observed",
                "tool_count": None,
                "tool_names": [],
                "schema_digests": {},
                "required_tools": list(_BASE_REPORT_TOOLS),
                "missing_required_tools": list(_BASE_REPORT_TOOLS),
                "worker_publish_result_present": None,
            }
        return {
            "state": receipt["state"],
            "tool_count": receipt["tool_count"],
            "tool_names": list(receipt["tool_names"]),
            "schema_digests": dict(receipt["schema_digests"]),
            "required_tools": list(receipt["required_tools"]),
            "missing_required_tools": list(receipt["missing_required_tools"]),
            "worker_publish_result_present": receipt["worker_publish_result_present"],
        }

    def _observe_tools_list(self, reply, pid):
        result = reply.get("result")
        if not isinstance(result, dict) or not isinstance(result.get("tools"), list):
            return
        tools = result["tools"]
        names = set()
        schema_digests = {}
        for tool in tools:
            if not isinstance(tool, dict) or not isinstance(tool.get("name"), str):
                continue
            name = tool["name"]
            names.add(name)
            # Only the digest crosses the observation boundary.  In particular,
            # descriptions and inputSchema values never reach persistent logs.
            schema_digests[name] = _schema_digest(tool.get("inputSchema"))
        sorted_names = sorted(names)
        missing = [name for name in _BASE_REPORT_TOOLS if name not in names]
        state = "complete" if not missing else "incomplete"
        receipt = {
            "state": state,
            "tool_count": len(tools),
            "tool_names": sorted_names,
            "schema_digests": {name: schema_digests[name] for name in sorted(schema_digests)},
            "required_tools": list(_BASE_REPORT_TOOLS),
            "missing_required_tools": missing,
            "worker_publish_result_present": _WORKER_REPORT_TOOL in names,
        }
        self._tools_receipt = receipt
        if missing:
            self._failure = "mcp_tools_incomplete"
            self._event(
                "mcp_tools_incomplete", pid=pid, tool_count=len(tools),
                tool_names=sorted_names, schema_digests=receipt["schema_digests"],
                missing_required_tools=missing,
                worker_publish_result_present=receipt["worker_publish_result_present"],
            )
            return
        self._ready.set()
        self._event(
            "mcp_tools_ready", pid=pid, tool_count=len(tools),
            tool_names=sorted_names, schema_digests=receipt["schema_digests"],
            required_tools=receipt["required_tools"],
            worker_publish_result_present=receipt["worker_publish_result_present"],
        )

    def _serve(self):
        while not self._stop.is_set():
            try:
                conn, _ = self._listener.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            try:
                self._serve_connection(conn)
            except Exception as exc:
                self._failure = "broker_exception"
                self._closed.set()
                self._event("mcp_broker_failure", reason="broker_exception",
                            exception_type=type(exc).__name__)
                try:
                    conn.close()
                except OSError:
                    pass

    def _serve_connection(self, conn):
        conn.settimeout(5.0)
        received = bytearray()
        while len(received) < MAX_HANDSHAKE_BYTES:
            part = conn.recv(64)
            if not part:
                return
            received.extend(part)
            if b"\n" in received:
                break
        line, _, remainder = bytes(received).partition(b"\n")
        if line.decode("ascii", errors="ignore") != self._token:
            self._event("mcp_broker_failure", reason="relay_handshake_rejected")
            return
        self._event("mcp_relay_connected")
        conn.settimeout(None)
        channel_id, secret, project_root = self._credentials
        env = dict(os.environ)
        env.update({"CORTEX_CHANNEL_ID": channel_id, "CORTEX_CHANNEL_SECRET": secret,
                    "CORTEX_CHANNEL_PROJECT_ROOT": project_root})
        child = subprocess.Popen([self._python, "-B", self._script], cwd=project_root, env=env,
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                 stderr=subprocess.DEVNULL, start_new_session=True)
        self._child = child
        self._event("mcp_server_started", pid=child.pid)
        if remainder:
            child.stdin.write(remainder)
            child.stdin.flush()

        def socket_to_child():
            try:
                while True:
                    data = conn.recv(IO_CHUNK)
                    if not data:
                        break
                    child.stdin.write(data)
                    child.stdin.flush()
            except (OSError, BrokenPipeError):
                pass
            finally:
                try:
                    child.stdin.close()
                except (OSError, ValueError):
                    pass

        def child_to_socket():
            buffered = bytearray()
            try:
                while True:
                    data = os.read(child.stdout.fileno(), IO_CHUNK)
                    if not data:
                        break
                    # Inspect only bounded JSON-RPC response structure; never
                    # retain or persist tool schemas, content, or error text.
                    if self._tools_receipt is None and len(buffered) <= 2_000_000:
                        buffered.extend(data)
                        while b"\n" in buffered:
                            line, _, rest = buffered.partition(b"\n")
                            buffered = bytearray(rest)
                            try:
                                reply = json.loads(line)
                                if isinstance(reply, dict):
                                    self._observe_tools_list(reply, child.pid)
                            except (ValueError, TypeError, AttributeError):
                                pass
                    conn.sendall(data)
            except (OSError, BrokenPipeError):
                pass

        threading.Thread(target=socket_to_child, daemon=True).start()
        threading.Thread(target=child_to_socket, daemon=True).start()
        code = child.wait()
        if not self._ready.is_set() and self._tools_receipt is None:
            self._failure = "mcp_server_exited" if code else "connection_closed_before_tools"
            self._closed.set()
        elif not self._ready.is_set():
            # An observed incomplete tools/list is a stronger, fail-closed
            # diagnostic than the transport's later ordinary EOF.
            self._closed.set()
        self._event("mcp_server_exited", pid=child.pid, exit_code=code,
                    tools_ready=self._ready.is_set())
        try:
            conn.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        conn.close()
        self._child = None

    def close(self):
        self._stop.set()
        try:
            self._listener.close()
        except OSError:
            pass
        child = self._child
        if child is not None and child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=2.0)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=2.0)
        self._thread.join(timeout=2.0)
        self._tmp.cleanup()


def relay(endpoint: str, token: str) -> int:
    if not endpoint.startswith("/") or not token or len(token) > 100:
        sys.stderr.write("cortex_mcp_relay_invalid_arguments\n")
        return 2
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    phase = "connect"
    try:
        sock.settimeout(5.0)
        sock.connect(endpoint)
        sock.sendall(token.encode("ascii") + b"\n")
        sock.settimeout(None)
        phase = "forward"
        def stdin_to_socket():
            try:
                while True:
                    data = os.read(0, IO_CHUNK)
                    if not data:
                        break
                    sock.sendall(data)
            except OSError:
                pass
            finally:
                try:
                    sock.shutdown(socket.SHUT_WR)
                except OSError:
                    pass

        threading.Thread(target=stdin_to_socket, daemon=True).start()
        # EOF on stdin only half-closes the request direction. Keep reading
        # until the broker closes its response stream, otherwise a short MCP
        # exchange loses the reply as soon as the caller finishes writing.
        while True:
            data = sock.recv(IO_CHUNK)
            if not data:
                break
            os.write(1, data)
        return 0
    except OSError:
        sys.stderr.write("cortex_mcp_relay_" + phase + "_failed\n")
        return 1
    finally:
        sock.close()


def main(argv=None):
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--relay", nargs=2)
    args = parser.parse_args(argv)
    if args.relay:
        raise SystemExit(relay(*args.relay))


if __name__ == "__main__":
    main()
