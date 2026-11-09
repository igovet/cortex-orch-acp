"""Owner-initiated pre-save listing. No tools/call, persistence or raw diagnostics."""
import json
import os
from pathlib import Path
import re
import select
import shutil
import signal
import subprocess
import tempfile
import time

from cortex_runtime.configuration import mcp_servers
from cortex_runtime.engine.external_proxy import private_runtime_dir, write_frame
from cortex_runtime.orchestration.gateway import PRIVATE
from cortex_runtime.orchestration.sandbox import gateway_command, allow_network, compact_argv


def configuration(server):
    env = {}
    stored = mcp_servers._read_json(mcp_servers._global_path(None)) or {}
    saved = (stored.get("servers") or {}).get(server["name"], {})
    old = saved.get("env") or {}
    if isinstance(old, list):
        old = {item["name"]: item["value"] for item in old}
    for item in server.get("env") or []:
        if item.get("action") == "remove":
            continue
        env[item["name"]] = old[item["name"]] if item.get("action") == "keep" and item.get("stored") else item.get("value", "")
    # Discovery grants no tool calls; the placeholder satisfies registry validation only.
    spec = dict(command=server["command"], args=server.get("args", []), env=env,
                allowed_tools=["discovery-only"], read_paths=server.get("readPaths", []),
                write_paths=server.get("writePaths", []), network=server.get("network", False),
                runtime_dir_env=server.get("runtimeDirEnv", []))
    return mcp_servers._normalize_entries({"servers": {server["name"]: spec}}, Path("draft"))[0]


def listing(child, deadline):
    buffer = b""
    def send(method, identifier=None, params=None):
        message = dict(jsonrpc="2.0", method=method)
        if identifier is not None:
            message["id"] = identifier
        if params is not None:
            message["params"] = params
        write_frame(child.stdin, json.dumps(message).encode() + b"\n", deadline)
    def receive(identifier):
        nonlocal buffer
        for _ in range(32):
            while b"\n" not in buffer:
                left = deadline - time.monotonic()
                if left <= 0 or not select.select([child.stdout], [], [], max(0, left))[0]:
                    raise TimeoutError()
                chunk = os.read(child.stdout.fileno(), 65_536)
                if not chunk:
                    raise ValueError("closed")
                buffer += chunk
                if len(buffer) > 262_144:
                    raise ValueError("frame budget")
            line, buffer = buffer.split(b"\n", 1)
            value = json.loads(line)
            if "id" not in value:
                continue
            if value.get("id") != identifier or "error" in value:
                raise ValueError("invalid response")
            return value["result"]
        raise ValueError("notification budget")
    send("initialize", 1, dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="cortex-workspace", version="1.0.6")))
    initialized = receive(1)
    if initialized.get("protocolVersion") not in {"2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"}:
        raise ValueError("unsupported protocol")
    send("notifications/initialized")
    tools, cursor, seen = [], None, set()
    for page in range(20):
        send("tools/list", 2 + page, {"cursor": cursor} if cursor else {})
        result = receive(2 + page)
        batch = result.get("tools")
        if not isinstance(batch, list) or len(tools) + len(batch) > 1000:
            raise ValueError("tool budget")
        tools.extend(batch)
        cursor = result.get("nextCursor")
        if not cursor:
            return tools
        if not isinstance(cursor, str) or cursor in seen:
            raise ValueError("invalid cursor")
        seen.add(cursor)
    raise ValueError("page budget")


def discover(server, *, timeout=15):
    child = runtime = None
    phase = "configuration"
    try:
        config = configuration(server)
        with tempfile.TemporaryDirectory(prefix="cortex-mcp-check-") as scratch:
            root = Path(scratch) / "project"
            root.mkdir()
            phase = "sandbox"
            with gateway_command(str(root), scratch, [config["command"], *config["args"]], []) as (command, fds):
                grants = []
                # Same external-MCP grants and private-path rules as the runtime gateway.
                for key, bind in (("read_paths", "--ro-bind"), ("write_paths", "--bind")):
                    for value in config.get(key, []):
                        path = Path(value)
                        if (not path.is_absolute() or not path.exists() or PRIVATE.intersection(path.parts)
                                or any(parent.is_symlink() for parent in (path, *path.parents))
                                or path == root or path in root.parents or root in path.parents):
                            return dict(ok=False, code="path_refused", message="A sandbox grant is private, missing or a symlink. Correct it before retrying.")
                        grants += [bind, str(path), str(path)]
                home = os.environ.get("HOME", "")
                if home.startswith("/"):
                    grants += ["--setenv", "HOME", home]
                for item in config["env"]:
                    grants += ["--setenv", item["name"], item["value"]]
                runtime = private_runtime_dir(home, scratch)
                grants += ["--bind", str(runtime), str(runtime)]
                for name in ("XDG_RUNTIME_DIR", *config.get("runtime_dir_env", [])):
                    grants += ["--setenv", name, str(runtime)]
                command[command.index("--chdir"):command.index("--chdir")] = grants
                if config["network"]:
                    allow_network(command)
                command, fds = compact_argv(command, fds)
                deadline = time.monotonic() + timeout
                phase = "spawn"
                child = subprocess.Popen(command, pass_fds=fds, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                         stderr=subprocess.DEVNULL, bufsize=0, env={"PATH":"/usr/bin:/bin"}, start_new_session=True)
                try:
                    phase = "listing"
                    tools = listing(child, deadline)
                finally:
                    # Includes descendants even if a malicious server closed stdio first.
                    try:
                        os.killpg(child.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    child.wait(timeout=2)
                    child.stdin.close()
                    child.stdout.close()
        phase = "projection"
        from cortex_runtime.observability.redact import scrub_text
        secrets = [item["value"] for item in config["env"] if item["value"]]
        rows = []
        for tool in tools:
            name = tool.get("name")
            if not isinstance(name, str) or not re.fullmatch(r"[A-Za-z0-9_.:-]{1,128}", name):
                raise ValueError("invalid tool")
            description = str(tool.get("description") or "")[:1000]
            for secret in secrets:
                name, description = name.replace(secret, "[redacted]"), description.replace(secret, "[redacted]")
            annotations = tool.get("annotations") or {}
            rows.append(dict(name=name, description=scrub_text(description, max_chars=1000),
                             hint="read" if annotations.get("readOnlyHint") is True else "destructive" if annotations.get("destructiveHint") is True else None))
        return dict(ok=True, observation=dict(at=int(time.time()*1000), name=server["name"], version="reported", tools=rows))
    except TimeoutError:
        return dict(ok=False, code="timeout", message="The server did not list its tools before the deadline. Its processes were stopped. Retry or edit the connection.")
    except Exception as error:
        # Neither exception text nor server stdout/stderr is diagnostic-safe.
        return dict(ok=False, code="not_supported", message="Could not list tools in the MCP sandbox. Check the executable, grants and environment, then retry.", manual=True, diagnostic=phase + ":" + type(error).__name__)
    finally:
        if runtime is not None:
            shutil.rmtree(runtime, ignore_errors=True)
