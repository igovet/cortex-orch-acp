"""TerminalBridge: local ``terminal/*`` agent-to-client handling (T014),
opt-in (only reachable when ``resource_policy.ResourcePolicy.terminal`` is
true -- see ``session_manager.EngineSessionManager``'s dispatch).

Least privilege: a launched terminal process never inherits this parent's
own environment. Only the explicit ``env`` entries from the request are
applied, layered over a minimal fixed base (``PATH``/``HOME``/``LANG``) --
mirroring ``connections.environment.build_process_env``'s "no ambient
environ passthrough" discipline, applied here to the *terminal* subprocess
rather than the engine subprocess itself. ``cwd`` defaults to (and is
always required to be contained in) the bound ``ResourceBinding``'s
``workspace_root``, the same containment ``fs_bridge`` applies.

Output is bounded per terminal: ``min(request outputByteLimit, policy
output_byte_limit_max)`` bytes, truncated from the *beginning* of the
buffer at a UTF-8 character boundary once exceeded, with a ``truncated``
flag. ``exitStatus`` (``{exitCode, signal}``) appears once the process has
actually exited. ``kill`` sends SIGTERM to the process group, then SIGKILL
after a grace period if it has not exited. ``release`` kills (if still
running) and frees the terminal id; every terminal for a session is
cleaned up on session release/engine exit/invalidate_session (see
``session_manager.EngineSessionManager.release``).
"""
from __future__ import annotations

import os
import signal
import subprocess
import threading
import uuid

MAX_TERMINALS_PER_SESSION = 8
KILL_GRACE_SECONDS = 5.0
_MINIMAL_BASE_ENV = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8"}


class TerminalBridgeError(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code
        self.message = message


def _truncate_from_start(buffer: bytes, limit: int) -> tuple[bytes, bool]:
    if len(buffer) <= limit:
        return buffer, False
    cut = len(buffer) - limit
    # Never cut in the middle of a UTF-8 multi-byte sequence: advance past
    # any continuation byte (10xxxxxx) at the cut point.
    while cut < len(buffer) and (buffer[cut] & 0xC0) == 0x80:
        cut += 1
    return buffer[cut:], True


class LocalTerminal:
    def __init__(self, terminal_id: str, process: subprocess.Popen, *, output_byte_limit: int):
        self.terminal_id = terminal_id
        self.process = process
        self._output_byte_limit = output_byte_limit
        self._lock = threading.Lock()
        self._buffer = b""
        self._truncated = False
        self._exit_status: dict | None = None
        self._exited_event = threading.Event()
        threading.Thread(target=self._drain, daemon=True, name="terminal-drain").start()
        threading.Thread(target=self._wait, daemon=True, name="terminal-wait").start()

    def _drain(self) -> None:
        stream = self.process.stdout
        try:
            while True:
                chunk = stream.read(4096)
                if not chunk:
                    break
                with self._lock:
                    self._buffer, was_truncated = _truncate_from_start(self._buffer + chunk, self._output_byte_limit)
                    self._truncated = self._truncated or was_truncated
        except (OSError, ValueError):
            pass

    def _wait(self) -> None:
        code = self.process.wait()
        signal_name = None
        if code is not None and code < 0:
            try:
                signal_name = signal.Signals(-code).name
            except ValueError:
                signal_name = str(-code)
            code = None
        with self._lock:
            self._exit_status = dict(exitCode=code, signal=signal_name)
        self._exited_event.set()

    def output(self) -> dict:
        with self._lock:
            result = dict(output=self._buffer.decode("utf-8", errors="replace"), truncated=self._truncated)
            if self._exit_status is not None:
                result["exitStatus"] = dict(self._exit_status)
        return result

    def wait_for_exit(self, timeout=None) -> bool:
        return self._exited_event.wait(timeout)

    def kill(self, grace: float = KILL_GRACE_SECONDS) -> None:
        if self._exited_event.is_set():
            return
        try:
            os.killpg(os.getpgid(self.process.pid), signal.SIGTERM)
        except (ProcessLookupError, PermissionError, OSError):
            pass
        if self._exited_event.wait(grace):
            return
        try:
            os.killpg(os.getpgid(self.process.pid), signal.SIGKILL)
        except (ProcessLookupError, PermissionError, OSError):
            pass
        self._exited_event.wait(grace)

    def release(self) -> None:
        self.kill()


class TerminalBridge:
    """Per-``EngineSessionManager`` terminal registry; every terminal is
    scoped to the ``engine_session_id`` that created it -- ``output``/
    ``wait_for_exit``/``kill``/``release`` for a ``terminalId`` created by a
    *different* session (even on the same engine instance) is refused as
    unknown, never resolved."""

    def __init__(self, *, max_per_session: int = MAX_TERMINALS_PER_SESSION):
        self._lock = threading.Lock()
        self._terminals: dict[str, dict[str, LocalTerminal]] = {}
        self._max_per_session = max_per_session

    def create(self, binding, params, *, output_byte_limit_max: int) -> str:
        command = (params or {}).get("command")
        if not isinstance(command, str) or not command:
            raise TerminalBridgeError(-32602, "command must be a non-empty string.")
        args = (params or {}).get("args") or []
        if not isinstance(args, list) or not all(isinstance(item, str) for item in args):
            raise TerminalBridgeError(-32602, "args must be a list of strings.")
        cwd = (params or {}).get("cwd") or binding.workspace_root
        if not isinstance(cwd, str) or not cwd.startswith("/"):
            raise TerminalBridgeError(-32602, "cwd must be an absolute path.")
        try:
            resolved_cwd = os.path.realpath(cwd)
        except OSError as exc:
            raise TerminalBridgeError(-32602, f"cwd is not resolvable: {exc}") from exc
        try:
            contained = os.path.commonpath([resolved_cwd, binding.workspace_root]) == binding.workspace_root
        except ValueError:
            contained = False
        if not contained:
            raise TerminalBridgeError(-32602, "cwd is outside the bound workspace root.")
        env_entries = (params or {}).get("env") or []
        if not isinstance(env_entries, list):
            raise TerminalBridgeError(-32602, "env must be a list of {name, value} entries.")
        env = dict(_MINIMAL_BASE_ENV)
        env["HOME"] = binding.workspace_root
        for item in env_entries:
            if not (isinstance(item, dict) and isinstance(item.get("name"), str) and isinstance(item.get("value"), str)):
                raise TerminalBridgeError(-32602, "each env entry must have string name/value.")
            env[item["name"]] = item["value"]
        requested_limit = (params or {}).get("outputByteLimit")
        limit = output_byte_limit_max
        if isinstance(requested_limit, int) and requested_limit > 0:
            limit = min(requested_limit, output_byte_limit_max)
        with self._lock:
            session_terminals = self._terminals.setdefault(binding.engine_session_id, {})
            if len(session_terminals) >= self._max_per_session:
                raise TerminalBridgeError(-32000, "too many concurrent terminals for this session.")
            terminal_id = "term_" + uuid.uuid4().hex[:12]
            try:
                process = subprocess.Popen(
                    [command, *args], cwd=resolved_cwd, env=env,
                    stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                    start_new_session=True,
                )
            except OSError as exc:
                raise TerminalBridgeError(-32602, f"could not start command: {exc}") from exc
            session_terminals[terminal_id] = LocalTerminal(terminal_id, process, output_byte_limit=limit)
        return terminal_id

    def _get(self, engine_session_id: str, terminal_id) -> LocalTerminal:
        with self._lock:
            terminal = (self._terminals.get(engine_session_id) or {}).get(terminal_id)
        if terminal is None:
            raise TerminalBridgeError(-32602, "unknown terminalId for this session.")
        return terminal

    def output(self, engine_session_id: str, terminal_id) -> dict:
        return self._get(engine_session_id, terminal_id).output()

    def wait_for_exit(self, engine_session_id: str, terminal_id, timeout=None) -> dict:
        terminal = self._get(engine_session_id, terminal_id)
        terminal.wait_for_exit(timeout)
        return terminal.output().get("exitStatus") or dict(exitCode=None, signal=None)

    def kill(self, engine_session_id: str, terminal_id) -> dict:
        self._get(engine_session_id, terminal_id).kill()
        return {}

    def release(self, engine_session_id: str, terminal_id) -> dict:
        terminal = self._get(engine_session_id, terminal_id)
        terminal.release()
        with self._lock:
            session_terminals = self._terminals.get(engine_session_id)
            if session_terminals is not None:
                session_terminals.pop(terminal_id, None)
        return {}

    def release_session(self, engine_session_id: str) -> None:
        with self._lock:
            session_terminals = self._terminals.pop(engine_session_id, {})
        for terminal in session_terminals.values():
            terminal.release()
