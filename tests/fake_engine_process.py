#!/usr/bin/env python3
"""Standalone fake downstream ACP engine (test-only; see engine_fakes.py).

No import of ``cortex_runtime`` or ``tests`` -- this runs as a real
subprocess launched from a ``ConnectionRecord.launch_command``. It never
claims to emulate a real engine (codex-acp/claude-agent-acp); it is a
scripted collaborator for T012's process/session/client/correlation tests.

Usage: ``python3 -B fake_engine_process.py <scenario.json>``
"""
from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import threading
import time

STDOUT = sys.stdout.buffer
STDIN = sys.stdin.buffer
_WRITE_LOCK = threading.Lock()


def _write_raw(data: bytes) -> None:
    with _WRITE_LOCK:
        try:
            STDOUT.write(data)
            STDOUT.flush()
        except (BrokenPipeError, OSError):
            pass


def _write_json(obj) -> None:
    _write_raw((json.dumps(obj, ensure_ascii=False) + "\n").encode("utf-8"))


def _write_malformed(mode: str) -> None:
    if mode == "non_json":
        _write_raw(b"not valid json at all\n")
    elif mode == "batch":
        _write_raw(b"[]\n")
    elif mode == "oversize":
        _write_raw(b"{" + b"\"x\":\"" + (b"a" * 3_000_000) + b"\"}\n")
    elif mode == "nesting_bomb":
        _write_raw((b"[" * 100_000) + (b"]" * 100_000) + b"\n")
    elif mode == "invalid_utf8":
        _write_raw(b"\xff\xfe not utf8\n")


def _notify(method, params) -> None:
    _write_json(dict(jsonrpc="2.0", method=method, params=params))


def _result(request_id, result) -> None:
    _write_json(dict(jsonrpc="2.0", id=request_id, result=result))


def _error(request_id, code, message) -> None:
    _write_json(dict(jsonrpc="2.0", id=request_id, error=dict(code=code, message=message)))


class FakeEngine:
    def __init__(self, scenario: dict):
        self.scenario = scenario
        self._cancel_events: dict[str, threading.Event] = {}
        self._client_request_id = 1_000_000

    def run(self) -> None:
        env_dump_path = self.scenario.get("env_dump_path")
        if env_dump_path:
            try:
                with open(env_dump_path, "w", encoding="utf-8") as handle:
                    json.dump(sorted(os.environ.keys()), handle)
            except OSError:
                pass
        if self.scenario.get("ignore_sigterm"):
            # Only SIGTERM is ignored; SIGKILL (untrappable) still works,
            # exercising EngineProcess.terminate()'s SIGTERM -> grace ->
            # SIGKILL escalation.
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
        grandchild_pid_file = self.scenario.get("spawn_grandchild_pid_file")
        if grandchild_pid_file:
            # A grandchild left in the *same* process group as this fake
            # engine (no start_new_session here) -- EngineProcess.terminate()
            # signals the whole group, so it must die too.
            grandchild = subprocess.Popen(["sleep", "300"])
            with open(grandchild_pid_file, "w", encoding="utf-8") as handle:
                handle.write(str(grandchild.pid))
        stderr_flood_bytes = self.scenario.get("stderr_flood_bytes")
        if stderr_flood_bytes:
            threading.Thread(target=self._flood_stderr, args=(stderr_flood_bytes,), daemon=True).start()
        for raw_line in iter(STDIN.readline, b""):
            line = raw_line.strip(b"\n")
            if not line:
                continue
            try:
                message = json.loads(line.decode("utf-8"))
            except (UnicodeDecodeError, ValueError):
                continue
            if not isinstance(message, dict):
                continue
            method = message.get("method")
            request_id = message.get("id")
            request_log_path = self.scenario.get("request_log_path")
            if request_log_path and isinstance(method, str):
                try:
                    with open(request_log_path, "a", encoding="utf-8") as handle:
                        handle.write(json.dumps(dict(method=method, has_id="id" in message)) + "\n")
                except OSError:
                    pass
            if method == "initialize":
                self._handle_initialize(request_id)
            elif method == "session/new":
                self._handle_session_new(request_id)
            elif method == "session/set_config_option":
                self._handle_set_config_option(request_id, message.get("params") or {})
            elif method == "session/prompt":
                threading.Thread(
                    target=self._handle_prompt, args=(request_id, message.get("params") or {}), daemon=True,
                ).start()
            elif method == "session/cancel":
                self._handle_cancel(message.get("params") or {})
            # Unknown methods/notifications are silently ignored (mirrors a
            # real engine's own "ignore what it doesn't recognize" duty).
        if self.scenario.get("ignore_sigterm"):
            # stdin EOF (the parent closed its write end as the first step
            # of EngineProcess.terminate()) would otherwise let this loop
            # -- and the process -- exit gracefully on its own; a fake
            # engine truly modeling "ignores SIGTERM" must also outlive
            # that, so the only way the parent can actually end it is
            # SIGKILL.
            time.sleep(300)

    def _handle_initialize(self, request_id) -> None:
        malformed = self.scenario.get("malformed_mode")
        if malformed:
            _write_malformed(malformed)
        if self.scenario.get("send_unsolicited_response_first"):
            # A response whose id was never sent by the client -- must be
            # recognized as unknown and dropped, never matched to any
            # pending call.
            _result(999_999_999, dict(configOptions=[]))
        caps = self.scenario.get("agent_capabilities") or {}
        _result(request_id, dict(agentCapabilities=caps))

    def _handle_session_new(self, request_id) -> None:
        for update in self.scenario.get("replay_updates") or []:
            _notify("session/update", dict(sessionId=self.scenario["session_id"], update=update))
        payload = dict(sessionId=self.scenario["session_id"])
        options = self.scenario.get("config_options")
        if options:
            payload["configOptions"] = options
        _result(request_id, payload)

    def _handle_set_config_option(self, request_id, params) -> None:
        behavior = self.scenario.get("configure_behavior", "accept")
        if behavior == "hang":
            return
        if behavior == "error":
            _error(request_id, -32001, "engine_rejected_configuration")
            return
        if behavior == "accept_no_config_options":
            # A "successful" (no JSON-RPC error) reply that omits
            # configOptions entirely -- no evidence either way of what was
            # actually applied.
            _result(request_id, {})
            return
        options = list(self.scenario.get("config_options") or [])
        config_id = params.get("configId")
        value = params.get("value")
        updated = []
        for option in options:
            option = dict(option)
            if option.get("id") == config_id:
                option["currentValue"] = value if behavior == "accept" else "unchanged-mismatch-value"
            updated.append(option)
        self.scenario["config_options"] = updated
        _result(request_id, dict(configOptions=updated))

    def _handle_cancel(self, params) -> None:
        session_id = params.get("sessionId")
        event = self._cancel_events.setdefault(session_id, threading.Event())
        event.set()

    def _flood_stderr(self, total_bytes: int) -> None:
        chunk = (b"x" * 4096) + b"\n"
        written = 0
        while written < total_bytes:
            try:
                sys.stderr.buffer.write(chunk)
                sys.stderr.buffer.flush()
            except (BrokenPipeError, OSError, ValueError):
                return
            written += len(chunk)

    def _handle_prompt(self, request_id, params) -> None:
        session_id = params.get("sessionId")
        # Cancellation is scoped to one turn, never sticky across a later
        # turn on the same session -- reset at the start of every prompt
        # (matches real ACP semantics: a fresh session/prompt call starts a
        # fresh cancellable turn).
        cancel_event = self._cancel_events.setdefault(session_id, threading.Event())
        cancel_event.clear()
        behavior = self.scenario.get("prompt_behavior", "normal")

        if behavior == "hang":
            return  # never reply, never emit -- caller times out
        if behavior == "one_update_then_hang":
            # Emit exactly the first scripted update, then behave exactly
            # like "hang": never reply, never emit again -- lets a test
            # drive the host-side generator to a real suspended yield
            # point before abandoning it (unlike a bare "hang", where the
            # generator would never have started at all).
            updates = self.scenario.get("prompt_updates") or []
            if updates:
                _notify("session/update", dict(sessionId=session_id, update=updates[0]))
            return
        if behavior == "crash":
            for update in self.scenario.get("prompt_updates") or []:
                _notify("session/update", dict(sessionId=session_id, update=update))
            STDOUT.flush()
            os._exit(7)
        if behavior == "eof_no_exit":
            for update in self.scenario.get("prompt_updates") or []:
                _notify("session/update", dict(sessionId=session_id, update=update))
            try:
                STDOUT.close()
            except OSError:
                pass
            time.sleep(30)
            return

        honor_cancel = behavior != "ignore_cancel"
        for update in self.scenario.get("prompt_updates") or []:
            if honor_cancel and cancel_event.is_set():
                break
            delay = update.get("delay", 0.0)
            if delay:
                time.sleep(delay)
            _notify("session/update", dict(sessionId=session_id, update=update))
            if behavior == "malformed_then_normal":
                _write_malformed(self.scenario.get("malformed_mode") or "non_json")

        if behavior == "client_request":
            request = self.scenario.get("client_request") or dict(method="request_permission", params={})
            self._client_request_id += 1
            _write_json(dict(jsonrpc="2.0", id=self._client_request_id, method=request["method"], params=request.get("params") or {}))

        stop_reason = "cancelled" if (honor_cancel and cancel_event.is_set()) else self.scenario.get("prompt_stop_reason", "end_turn")

        if behavior == "late_reply":
            time.sleep(self.scenario.get("late_reply_delay", 1.5))

        _result(request_id, dict(stopReason=stop_reason))


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: fake_engine_process.py <scenario.json>", file=sys.stderr)
        return 2
    with open(sys.argv[1], "r", encoding="utf-8") as handle:
        scenario = json.load(handle)
    FakeEngine(scenario).run()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
