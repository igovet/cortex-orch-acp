"""Host-side Cortex MCP broker and stdio relay."""
from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import os
import secrets
import socket
import stat
import struct
import subprocess
import sys
import tempfile
import threading

MAX_HANDSHAKE_BYTES = 128
IO_CHUNK = 64 * 1024
_BASE_REPORT_TOOLS = ("create_draft", "write_report")
_WORKER_REPORT_TOOL = "result_submit"
#: The relay reads its handshake token from this 0600 file next to the
#: broker socket, inside the broker's private 0700 directory. Neither the
#: relay's argv nor its ACP ``mcpServers[].env`` entry carries the token: an
#: engine may serialize both onto a world-readable command line (the Claude
#: Agent SDK passes the whole server map, env included, to the ``claude`` CLI
#: as ``--mcp-config <json>``), and ``/proc/<pid>/cmdline`` is readable by
#: every process on the host.
RELAY_TOKEN_FILE = "relay.token"
#: Upper bound on the token file a relay will read.
_MAX_TOKEN_FILE_BYTES = 128
#: Upper bound on the parent-chain walk that binds a relay to its engine.
_MAX_ANCESTRY_DEPTH = 64


def _peer_credentials(conn):
    """``(pid, uid, gid)`` of the connected Unix-socket peer, or ``None``
    where the platform has no ``SO_PEERCRED``. The kernel records these at
    ``connect()``; they are translated into this process's namespaces."""
    option = getattr(socket, "SO_PEERCRED", None)
    if option is None:
        return None
    size = struct.calcsize("3i")
    return struct.unpack("3i", conn.getsockopt(socket.SOL_SOCKET, option, size))


def _parent_pid(pid: int) -> int:
    with open(f"/proc/{pid}/stat", "rb") as handle:
        data = handle.read()
    # ``comm`` (field 2) may itself contain spaces or ')': fields resume
    # after its last ')' as ``state ppid ...``.
    return int(data[data.rindex(b")") + 2:].split()[1])


def _is_descendant(pid: int, root: int) -> bool:
    """Whether ``pid`` is ``root`` or one of its descendants (fail closed)."""
    current = pid
    for _ in range(_MAX_ANCESTRY_DEPTH):
        if current == root:
            return True
        if current <= 1:
            return False
        try:
            current = _parent_pid(current)
        except (OSError, ValueError, IndexError):
            return False
    return False


def relay_token_path(endpoint: str) -> str:
    """The token file a relay for ``endpoint`` reads: the broker writes it
    beside its socket, so the relay entry names only the socket path."""
    return os.path.join(os.path.dirname(endpoint), RELAY_TOKEN_FILE)


def _write_token_file(path: str, token: str) -> None:
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        os.write(fd, token.encode("ascii"))
    finally:
        os.close(fd)


def read_relay_token(endpoint: str) -> str | None:
    """The relay's handshake token, or ``None`` when its file is missing,
    not a regular file (a symlink is never followed) or malformed."""
    try:
        fd = os.open(relay_token_path(endpoint), os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    except OSError:
        return None
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            return None
        data = os.read(fd, _MAX_TOKEN_FILE_BYTES + 1)
    except OSError:
        return None
    finally:
        os.close(fd)
    if not data or len(data) > _MAX_TOKEN_FILE_BYTES or not data.isascii():
        return None
    return data.decode("ascii")


def _private_directory(parent):
    """A fresh 0700 directory owned by this user for one broker's socket."""
    directory = tempfile.TemporaryDirectory(prefix="cortex-mcp-", dir=parent)
    try:
        os.chmod(directory.name, 0o700)
        info = os.lstat(directory.name)
        if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.geteuid()
                or stat.S_IMODE(info.st_mode) & 0o077):
            raise OSError("cortex_mcp_socket_directory_not_private")
    except BaseException:
        directory.cleanup()
        raise
    return directory


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
                 observe=None, server_config=None, socket_dir=None, stderr_log=None,
                 peer_root=None):
        if not channel_credentials or len(channel_credentials) != 3:
            raise ValueError("channel_credentials are required for an MCP broker")
        self._credentials = tuple(channel_credentials)
        self._python = python_executable
        self._script = server_script
        self._observe = observe
        # The engine's private, size-capped stderr file (never the
        # observation log): the server's own tracebacks land there.
        self._stderr_log = stderr_log
        self._server_config = server_config
        self._state_lock = threading.RLock()
        self._generation = 0
        self._ready = threading.Event()
        self._closed = threading.Event()
        self._failure = None
        # This is deliberately metadata-only.  The child response is forwarded
        # to the engine, but no tool description, argument value, or schema is
        # retained by the host-side broker.
        self._tools_receipt = None
        # The relay reads this token from RELAY_TOKEN_FILE in the private
        # socket directory, never from argv or env. The literal leading
        # letter keeps a token from ever looking like a command-line option
        # to a wrapper; the random portion keeps its full entropy.
        self._token = "c" + secrets.token_urlsafe(32)
        # Authentication is the token AND the connecting peer: same user, and
        # (once bound) a process inside the engine's own process tree, so a
        # token read by any other process is useless. Until a peer root is
        # bound the token is single-use.
        self._peer_root = peer_root
        self._token_consumed = False
        self._tmp = _private_directory(socket_dir)
        self.endpoint = os.path.join(self._tmp.name, "broker.sock")
        self._listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            # Kept for the broker's lifetime (removed with the directory by
            # close()), not unlinked after a first read: an engine may
            # relaunch the same relay entry (codex-acp app-server crash
            # recovery, an MCP reconnect), and peer binding, not file
            # secrecy, is what refuses processes outside the engine tree.
            _write_token_file(relay_token_path(self.endpoint), self._token)
            self._listener.bind(self.endpoint)
            os.chmod(self.endpoint, 0o600)
        except BaseException:
            self._listener.close()
            self._tmp.cleanup()
            raise
        self._listener.listen(1)
        self._listener.settimeout(0.25)
        self._stop = threading.Event()
        self._child = None
        self._thread = threading.Thread(target=self._serve, daemon=True, name="cortex-mcp-broker")
        self._thread.start()

    @property
    def relay_token(self) -> str:
        return self._token

    def bind_peer_root(self, pid: int) -> None:
        """Accept relays only from ``pid`` (the launched engine) or its
        descendants. An engine may legitimately restart its MCP client (for
        example codex-acp's app-server crash recovery) and relaunch the relay
        with the same token; a bound root keeps that working while refusing
        every process outside the engine's tree."""
        if not isinstance(pid, int) or isinstance(pid, bool) or pid <= 1:
            raise ValueError("peer root must be a real process id")
        with self._state_lock:
            self._peer_root = pid

    def _authenticate(self, conn, line: bytes):
        """``None`` when the handshake is accepted, else a rejection cause."""
        credentials = _peer_credentials(conn)
        if credentials is not None and credentials[1] != os.geteuid():
            return "peer_user"
        # Always compare in constant time, even when the peer is refused.
        token_ok = hmac.compare_digest(line, self._token.encode("ascii"))
        with self._state_lock:
            root = self._peer_root
            if root is not None and credentials is not None:
                if not _is_descendant(credentials[0], root):
                    return "peer_process"
                return None if token_ok else "token"
            if not token_ok:
                return "token"
            if self._token_consumed:
                return "token_replayed"
            self._token_consumed = True
            return None

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
        with self._state_lock:
            if self._failure is not None:
                return self._failure
        self._ready.wait(timeout)
        with self._state_lock:
            if self._failure is not None:
                return self._failure
            if self._ready.is_set():
                return "ready"
            if self._closed.is_set():
                return "connection_closed"
            return "not_observed"

    @property
    def tools_receipt(self) -> dict:
        """Return the bounded, in-memory receipt for this broker's MCP server.

        A broker serves one trusted Cortex channel.  The receipt therefore
        applies only to the current server generation; callers attach their
        own session id when exposing it as session diagnostics.  ``None`` is
        intentionally represented as ``not_observed``: an engine may defer
        ``tools/list`` until after the first prompt.
        """
        with self._state_lock:
            receipt = self._tools_receipt
        if receipt is None:
            return {
                "state": "not_observed",
                "tool_count": None,
                "tool_names": [],
                "schema_digests": {},
                "required_tools": list(_BASE_REPORT_TOOLS),
                "missing_required_tools": list(_BASE_REPORT_TOOLS),
                "worker_result_submit_present": None,
            }
        return {
            "state": receipt["state"],
            "tool_count": receipt["tool_count"],
            "tool_names": list(receipt["tool_names"]),
            "schema_digests": dict(receipt["schema_digests"]),
            "required_tools": list(receipt["required_tools"]),
            "missing_required_tools": list(receipt["missing_required_tools"]),
            "worker_result_submit_present": receipt["worker_result_submit_present"],
        }

    def _begin_generation(self):
        with self._state_lock:
            self._generation += 1
            self._ready.clear()
            self._tools_receipt = None
            self._closed.clear()
            # Keep a known loss until the replacement proves its own tools.
            return self._generation

    def _observe_tools_list(self, reply, pid, generation):
        with self._state_lock:
            if (generation != self._generation or self._closed.is_set()
                    or self._tools_receipt is not None):
                return
            result = reply.get("result")
            tools = result.get("tools") if isinstance(result, dict) else None
            malformed = not isinstance(tools, list) or bool(result.get("nextCursor"))
            tools = tools if isinstance(tools, list) else []
            names = set()
            valid_names = set()
            schema_digests = {}
            for tool in tools:
                if not isinstance(tool, dict) or not isinstance(tool.get("name"), str):
                    malformed = True
                    continue
                name = tool["name"]
                schema = tool.get("inputSchema")
                valid = bool(name) and name not in names and isinstance(schema, dict) and schema.get("type") == "object"
                malformed |= not valid
                names.add(name)
                if valid:
                    valid_names.add(name)
                # Only digests cross the observation boundary, never schemas.
                schema_digests[name] = _schema_digest(schema)
            sorted_names = sorted(names)
            missing = [name for name in _BASE_REPORT_TOOLS if name not in valid_names]
            state = "incomplete" if missing or malformed else "complete"
            receipt = {
                "state": state,
                "tool_count": len(tools),
                "tool_names": sorted_names,
                "schema_digests": {name: schema_digests[name] for name in sorted(schema_digests)},
                "required_tools": list(_BASE_REPORT_TOOLS),
                "missing_required_tools": missing,
                "worker_result_submit_present": _WORKER_REPORT_TOOL in valid_names,
            }
            self._tools_receipt = receipt
            if state == "incomplete":
                self._failure = "mcp_tools_incomplete"
                self._event(
                    "mcp_tools_incomplete", pid=pid, tool_count=len(tools),
                    tool_names=sorted_names, schema_digests=receipt["schema_digests"],
                    missing_required_tools=missing,
                    worker_result_submit_present=receipt["worker_result_submit_present"],
                )
                return
            self._failure = None
            self._ready.set()
            self._event(
                "mcp_tools_ready", pid=pid, tool_count=len(tools),
                tool_names=sorted_names, schema_digests=receipt["schema_digests"],
                required_tools=receipt["required_tools"],
                worker_result_submit_present=receipt["worker_result_submit_present"],
            )

    def _server_exited(self, generation, pid, code):
        with self._state_lock:
            if generation != self._generation:
                return
            ready = self._ready.is_set()
            if not ready and self._tools_receipt is None:
                self._failure = "mcp_server_exited" if code else "connection_closed_before_tools"
            elif ready and code != 0 and not self._stop.is_set():
                self._failure = "mcp_server_crashed"
            # Fence even delayed replies from this exited generation.
            self._closed.set()
            self._event("mcp_server_exited", pid=pid, exit_code=code, tools_ready=ready)

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
                with self._state_lock:
                    self._failure = "broker_exception"
                    self._closed.set()
                self._event("mcp_broker_failure", reason="broker_exception",
                            exception_type=type(exc).__name__)
            finally:
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
        cause = self._authenticate(conn, line)
        if cause is not None:
            self._event("mcp_broker_failure", reason="relay_handshake_rejected", cause=cause)
            return
        self._event("mcp_relay_connected")
        generation = self._begin_generation()
        conn.settimeout(None)
        channel_id, secret, project_root = self._credentials
        env = dict(os.environ)
        env.update({"CORTEX_CHANNEL_ID": channel_id, "CORTEX_CHANNEL_SECRET": secret,
                    "CORTEX_CHANNEL_PROJECT_ROOT": project_root})
        if self._server_config is not None:
            env['CORTEX_EXTERNAL_SERVER'] = json.dumps(self._server_config)
        child = subprocess.Popen([self._python, "-B", self._script], cwd=project_root, env=env,
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE if self._stderr_log is not None else subprocess.DEVNULL,
                                 start_new_session=True)
        with self._state_lock:
            self._child = child
        self._event("mcp_server_started", pid=child.pid)
        if self._stderr_log is not None:
            def drain_stderr():
                # server._log_unexpected writes the cause of an opaque
                # storage_error here; DEVNULL used to discard every one.
                try:
                    for line in child.stderr:
                        self._stderr_log.append_line(f"[mcp-server {child.pid}] " + line.decode("utf-8", errors="replace"))
                except (OSError, ValueError):
                    pass
            threading.Thread(target=drain_stderr, name="mcp-server-stderr", daemon=True).start()
        pending_tools = set()
        pending_lock = threading.Lock()

        def frames():
            # Bounded JSON lines only; oversized frames cannot prove readiness.
            buffered = bytearray()
            dropping = False
            def feed(data):
                nonlocal dropping
                for part in data.splitlines(keepends=True):
                    if not dropping:
                        buffered.extend(part)
                        if len(buffered) > 2_000_000:
                            buffered.clear()
                            dropping = True
                    if part.endswith(b"\n"):
                        if not dropping:
                            try:
                                value = json.loads(buffered)
                                if isinstance(value, dict):
                                    yield value
                            except (ValueError, TypeError):
                                pass
                        buffered.clear()
                        dropping = False
            return feed

        request_frames = frames()
        def observe_requests(data):
            for request in request_frames(data):
                request_id = request.get("id")
                if request.get("method") == "tools/list" and type(request_id) in (int, str):
                    with pending_lock:
                        if len(pending_tools) < 64:
                            pending_tools.add((type(request_id), request_id))

        if remainder:
            observe_requests(remainder)
            child.stdin.write(remainder)
            child.stdin.flush()

        def socket_to_child():
            try:
                while True:
                    data = conn.recv(IO_CHUNK)
                    if not data:
                        break
                    observe_requests(data)
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
            response_frames = frames()
            try:
                while True:
                    data = os.read(child.stdout.fileno(), IO_CHUNK)
                    if not data:
                        break
                    for reply in response_frames(data):
                        reply_id = reply.get("id")
                        if type(reply_id) not in (int, str):
                            continue
                        key = (type(reply_id), reply_id)
                        with pending_lock:
                            observed = key in pending_tools
                            pending_tools.discard(key)
                        if observed:
                            self._observe_tools_list(reply, child.pid, generation)
                    conn.sendall(data)
            except (OSError, BrokenPipeError):
                pass

        threading.Thread(target=socket_to_child, daemon=True).start()
        response_thread = threading.Thread(target=child_to_socket, daemon=True)
        response_thread.start()
        code = child.wait()
        # Drain already-written evidence before fencing the exit. A stalled
        # relay must not block recovery indefinitely; late evidence is refused.
        response_thread.join(timeout=1.0)
        self._server_exited(generation, child.pid, code)
        try:
            conn.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        conn.close()
        with self._state_lock:
            if generation == self._generation:
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


def relay(endpoint: str, token: str | None) -> int:
    if not endpoint.startswith("/") or not token or len(token) > 100 or not token.isascii():
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
    parser.add_argument("--relay", metavar="ENDPOINT")
    args = parser.parse_args(argv)
    if args.relay:
        if not args.relay.startswith("/"):
            sys.stderr.write("cortex_mcp_relay_invalid_arguments\n")
            raise SystemExit(2)
        token = read_relay_token(args.relay)
        if token is None:
            sys.stderr.write("cortex_mcp_relay_token_unavailable\n")
            raise SystemExit(2)
        raise SystemExit(relay(args.relay, token))


if __name__ == "__main__":
    main()
