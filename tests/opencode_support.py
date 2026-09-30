"""No-paid helpers for checks against a real, installed OpenCode binary.

``FakeProvider`` is a loopback OpenAI-compatible ``/chat/completions``
endpoint that answers from a script and records every request body, so a
check can see exactly what OpenCode sent to "the model" (system prompt,
advertised tools) without any real or paid model call. ``AcpDriver`` speaks
raw ACP JSON-RPC over the engine's stdio and answers
``session/request_permission`` through a caller-supplied policy.

Nothing here reads, prints or stores a credential.
"""
from __future__ import annotations

import http.server
import json
import queue
import subprocess
import threading
import time


class FakeProvider:
    """Script steps, consumed one per agent request (a request that carries
    tools); a request without tools (OpenCode's title/summary helpers) gets
    a short text reply and consumes nothing.

    Steps: ``("text", str)``, ``("tool", name, arguments)``,
    ``("stall", seconds)`` (answer only after ``seconds`` or ``release()``).
    """

    def __init__(self, steps=(), *, router=None):
        self.steps = list(steps)
        # ``router(body) -> step`` replaces the step list when given.
        self.router = router
        self.requests: list[dict] = []
        self.agent_requests: list[dict] = []
        # The Authorization header of each request, in order: which
        # credential OpenCode actually sent.
        self.authorizations: list[str | None] = []
        self._lock = threading.Lock()
        self._release = threading.Event()
        provider = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                self.send_response(404)
                self.end_headers()

            def do_POST(self):
                provider.authorizations.append(self.headers.get("Authorization"))
                length = int(self.headers.get("Content-Length", "0"))
                try:
                    body = json.loads(self.rfile.read(length) or b"{}")
                except ValueError:
                    body = {}
                provider._answer(self, body)

        class Server(http.server.ThreadingHTTPServer):
            def handle_error(self, request, client_address):
                pass  # a client that hung up mid-stream (a cancelled turn)

        self.server = Server(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self._thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self._thread.start()

    @property
    def base_url(self) -> str:
        return f"http://127.0.0.1:{self.server.server_port}/v1"

    def release(self) -> None:
        self._release.set()

    def close(self) -> None:
        self._release.set()
        self.server.shutdown()
        self.server.server_close()

    def _next_step(self, body):
        with self._lock:
            self.requests.append(body)
            if not body.get("tools"):
                return ("text", "Cortex check")
            self.agent_requests.append(body)
            if self.router is not None:
                step = self.router(body)
            else:
                step = self.steps.pop(0) if self.steps else ("text", "done")
        # A callable step decides from the request it answers (for example
        # the exact tool name OpenCode advertised for an MCP tool).
        return step(body) if callable(step) else step

    def _answer(self, handler, body):
        step = self._next_step(body)
        if step[0] == "stall":
            self._release.wait(step[1])
            step = ("text", "late")
        model = body.get("model") or "fake"
        chunks = []

        def chunk(delta, finish=None):
            return {"id": "chatcmpl-fake", "object": "chat.completion.chunk", "created": int(time.time()),
                    "model": model, "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}

        if step[0] == "tool":
            _, name, arguments = step
            call_id = f"call_{len(self.agent_requests)}"
            chunks.append(chunk({"role": "assistant", "tool_calls": [{
                "index": 0, "id": call_id, "type": "function",
                "function": {"name": name, "arguments": json.dumps(arguments)}}]}))
            chunks.append(chunk({}, "tool_calls"))
        else:
            chunks.append(chunk({"role": "assistant", "content": step[1]}))
            chunks.append(chunk({}, "stop"))
        usage = {"id": "chatcmpl-fake", "object": "chat.completion.chunk", "created": int(time.time()),
                 "model": model, "choices": [],
                 "usage": {"prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18}}
        if not body.get("stream"):
            message = {"role": "assistant", "content": step[1] if step[0] == "text" else None}
            payload = {"id": "chatcmpl-fake", "object": "chat.completion", "created": int(time.time()),
                       "model": model, "choices": [{"index": 0, "message": message, "finish_reason": "stop"}],
                       "usage": usage["usage"]}
            data = json.dumps(payload).encode()
            handler.send_response(200)
            handler.send_header("Content-Type", "application/json")
            handler.send_header("Content-Length", str(len(data)))
            handler.end_headers()
            handler.wfile.write(data)
            return
        stream = "".join(f"data: {json.dumps(item)}\n\n" for item in (*chunks, usage)) + "data: [DONE]\n\n"
        handler.send_response(200)
        handler.send_header("Content-Type", "text/event-stream")
        handler.send_header("Cache-Control", "no-cache")
        handler.end_headers()
        try:
            handler.wfile.write(stream.encode())
            handler.wfile.flush()
        except OSError:
            pass

    def tool_names(self, index=-1) -> list[str]:
        """Tool names advertised to the model in one agent request."""
        tools = self.agent_requests[index].get("tools") or []
        return sorted(tool.get("function", {}).get("name") for tool in tools)

    def system_text(self, index=-1) -> str:
        """Every system message of one agent request, joined."""
        parts = []
        for message in self.agent_requests[index].get("messages") or []:
            if message.get("role") != "system":
                continue
            content = message.get("content")
            if isinstance(content, str):
                parts.append(content)
            elif isinstance(content, list):
                parts.extend(item.get("text", "") for item in content if isinstance(item, dict))
        return "\n".join(parts)

    def all_text(self) -> str:
        return json.dumps(self.requests)


class AcpDriver:
    """Raw ACP client over one engine process's stdio."""

    def __init__(self, argv, *, env, cwd, pass_fds=(), permission=None):
        self.process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.PIPE, env=env, cwd=cwd, pass_fds=pass_fds)
        self.notifications: list[dict] = []
        self.permission_requests: list[dict] = []
        self.client_requests: list[str] = []
        self._permission = permission or (lambda params: allow_once(params))
        self._responses: dict = {}
        self._cond = threading.Condition()
        self._next_id = 0
        self._write_lock = threading.Lock()
        self.stderr_bytes = 0
        threading.Thread(target=self._read, daemon=True).start()
        threading.Thread(target=self._drain_stderr, daemon=True).start()

    def _drain_stderr(self):
        for line in self.process.stderr:
            self.stderr_bytes += len(line)

    def _send(self, message):
        data = (json.dumps(message) + "\n").encode()
        with self._write_lock:
            self.process.stdin.write(data)
            self.process.stdin.flush()

    def _read(self):
        for line in self.process.stdout:
            try:
                message = json.loads(line)
            except ValueError:
                continue
            if "method" in message and "id" in message:
                self._client_request(message)
            elif "method" in message:
                with self._cond:
                    self.notifications.append(message)
                    self._cond.notify_all()
            else:
                with self._cond:
                    self._responses[message.get("id")] = message
                    self._cond.notify_all()
        with self._cond:
            self._cond.notify_all()

    def _client_request(self, message):
        method = message["method"]
        self.client_requests.append(method)
        if method == "session/request_permission":
            self.permission_requests.append(message["params"])
            result = self._permission(message["params"])
            self._send({"jsonrpc": "2.0", "id": message["id"], "result": result})
            return
        self._send({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "unsupported"}})

    def start(self, method, params) -> int:
        with self._cond:
            self._next_id += 1
            request_id = self._next_id
        self._send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        return request_id

    def wait(self, request_id, timeout=60.0) -> dict:
        deadline = time.monotonic() + timeout
        with self._cond:
            while request_id not in self._responses:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or self.process.poll() is not None:
                    raise TimeoutError(f"no response to request {request_id}")
                self._cond.wait(min(remaining, 0.5))
            return self._responses.pop(request_id)

    def call(self, method, params, timeout=60.0) -> dict:
        return self.wait(self.start(method, params), timeout)

    def notify(self, method, params) -> None:
        self._send({"jsonrpc": "2.0", "method": method, "params": params})

    def updates(self, kind=None) -> list[dict]:
        with self._cond:
            found = [n["params"].get("update", {}) for n in self.notifications if n.get("method") == "session/update"]
        return [u for u in found if kind is None or u.get("sessionUpdate") == kind]

    def close(self):
        try:
            self.process.stdin.close()
        except OSError:
            pass
        self.process.terminate()
        try:
            self.process.wait(10)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(10)


def last_user_text(body) -> str:
    """The text of the newest user message in a chat-completions request."""
    for message in reversed(body.get("messages") or []):
        if message.get("role") != "user":
            continue
        content = message.get("content")
        if isinstance(content, str):
            return content
        if isinstance(content, list):
            return "\n".join(item.get("text", "") for item in content if isinstance(item, dict))
    return ""


def tool_results(body) -> list[str]:
    """Every tool-result message content in one request, as text."""
    found = []
    for message in body.get("messages") or []:
        if message.get("role") == "tool":
            content = message.get("content")
            found.append(content if isinstance(content, str) else json.dumps(content))
    return found


def allow_once(params) -> dict:
    for option in params.get("options") or []:
        if option.get("kind") == "allow_once":
            return {"outcome": {"outcome": "selected", "optionId": option["optionId"]}}
    return {"outcome": {"outcome": "cancelled"}}


def option_values(config_options, option_id) -> list:
    for option in config_options or []:
        if option.get("id") == option_id:
            return [choice.get("value") for choice in option.get("options") or []]
    return []


def bootstrap_login_database(binary, data_dir, scratch, logins, timeout=60.0):
    """The owner's OpenCode data directory as OpenCode itself creates it --
    its database schema and cached models.dev catalog, from one prompt-free
    ``session/new`` in a disposable HOME -- plus stand-in API-key logins
    (``{provider: key}``) in its ``credential`` table. No model is called."""
    import os
    import select
    import sqlite3
    from pathlib import Path

    data_dir, scratch = Path(data_dir), Path(scratch)
    home, work = scratch / "bootstrap-home", scratch / "bootstrap-work"
    for directory in (home, work, data_dir):
        directory.mkdir(parents=True, exist_ok=True)
    env = {"PATH": "/usr/bin:/bin", "HOME": str(home), "XDG_CONFIG_HOME": str(home / ".config"),
           "XDG_DATA_HOME": str(data_dir.parent), "XDG_STATE_HOME": str(home / ".state"),
           "XDG_CACHE_HOME": str(home / ".cache"), "OPENCODE_DISABLE_AUTOUPDATE": "1"}
    process = subprocess.Popen([str(binary), "acp"], cwd=work, env=env, stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    answered = False
    try:
        for index, (method, params) in enumerate((("initialize", {"protocolVersion": 1, "clientCapabilities": {}}),
                                                  ("session/new", {"cwd": str(work), "mcpServers": []}))):
            process.stdin.write((json.dumps({"jsonrpc": "2.0", "id": index, "method": method,
                                             "params": params}) + "\n").encode())
            process.stdin.flush()
            deadline = time.monotonic() + timeout
            answered = False
            while not answered and time.monotonic() < deadline:
                if not select.select([process.stdout], [], [], 0.5)[0]:
                    continue
                line = process.stdout.readline()
                if not line:
                    break
                try:
                    answered = json.loads(line).get("id") == index
                except ValueError:
                    continue
            if not answered:
                break
    finally:
        process.terminate()
        try:
            process.wait(10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(10)
    database = data_dir / "opencode.db"
    if not answered or not database.is_file():
        raise RuntimeError("OpenCode did not create its data directory")
    with sqlite3.connect(database) as db:
        for index, (provider, key) in enumerate(sorted(logins.items())):
            db.execute("INSERT INTO credential (id, integration_id, label, value, active, time_created, time_updated) "
                       "VALUES (?, ?, 'API key', ?, NULL, 1, 1)",
                       (f"cred_0standin{index:017d}", provider, json.dumps({"type": "key", "key": key})))
    db.close()
    os.chmod(data_dir, 0o700)
    return database
