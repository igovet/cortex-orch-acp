#!/usr/bin/env python3
"""Install and register the pinned official Playwright MCP for Cortex workers.

The browser binary is deliberately outside this installer: an MCP handshake
verifies server discovery, while browser navigation needs its own live check.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import sys
import time
import fcntl

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "src"))

from cortex_runtime.configuration.mcp_servers import load_mcp_servers  # noqa: E402
from cortex_runtime.connections import registry  # noqa: E402

PACKAGE = "@playwright/mcp"
VERSION = "0.0.81"
NAME = "playwright"


def _private_dir(path: Path) -> None:
    if path.is_symlink():
        raise RuntimeError(f"unsafe directory: {path}")
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    info = path.stat()
    if not path.is_dir() or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise RuntimeError(f"directory must be owner-only: {path}")


def _installed_version(prefix: Path) -> str | None:
    manifest = prefix / "node_modules" / "@playwright" / "mcp" / "package.json"
    if not manifest.exists():
        return None
    return json.loads(manifest.read_text(encoding="utf-8")).get("version")


def _install(prefix: Path) -> Path:
    _private_dir(prefix)
    installed = _installed_version(prefix)
    if installed != VERSION:
        npm = shutil.which("npm")
        if not npm:
            raise RuntimeError("npm was not found")
        completed = subprocess.run(
            [npm, "install", "--prefix", str(prefix), "--save-exact", "--omit=dev", "--no-audit", "--no-fund", f"{PACKAGE}@{VERSION}"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=180,
        )
        if completed.returncode:
            raise RuntimeError("npm installation failed")
    if _installed_version(prefix) != VERSION:
        raise RuntimeError("installed package version does not match pin")
    cli = prefix / "node_modules" / "@playwright" / "mcp" / "cli.js"
    if not cli.is_file():
        raise RuntimeError("Playwright MCP CLI is missing")
    return cli


def _read_response(process: subprocess.Popen, request_id: int, timeout: float = 20) -> dict:
    deadline = time.monotonic() + timeout
    buffer = b""
    fd = process.stdout.fileno()
    while time.monotonic() < deadline:
        readable, _, _ = select.select([fd], [], [], min(0.5, max(0, deadline - time.monotonic())))
        if not readable:
            if process.poll() is not None:
                break
            continue
        chunk = os.read(fd, 65536)
        if not chunk:
            break
        buffer += chunk
        while b"\n" in buffer:
            line, buffer = buffer.split(b"\n", 1)
            try:
                response = json.loads(line)
            except (ValueError, UnicodeDecodeError):
                continue
            if response.get("id") == request_id:
                if "error" in response:
                    raise RuntimeError(f"MCP request {request_id} returned an error")
                return response
    raise RuntimeError(f"MCP request {request_id} did not complete")


def _send(process: subprocess.Popen, body: dict) -> None:
    process.stdin.write((json.dumps(body) + "\n").encode())
    process.stdin.flush()


def probe(command: str, cli: Path) -> int:
    with subprocess.Popen(
        [command, str(cli), "--headless", "--isolated"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
    ) as process:
        try:
            _send(process, {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-03-26", "capabilities": {},
                "clientInfo": {"name": "cortex-playwright-setup", "version": "1.0.0"},
            }})
            initial = _read_response(process, 1)
            if not isinstance(initial.get("result"), dict):
                raise RuntimeError("MCP initialize returned no result")
            _send(process, {"jsonrpc": "2.0", "method": "notifications/initialized"})
            _send(process, {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
            listed = _read_response(process, 2)
            tools = listed.get("result", {}).get("tools")
            if not isinstance(tools, list) or not tools or any(not isinstance(t, dict) or not isinstance(t.get("name"), str) for t in tools):
                raise RuntimeError("MCP tools/list returned no valid tools")
            return len(tools)
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


def main() -> int:
    home = Path.home()
    data_base = Path(os.environ.get("XDG_DATA_HOME", home / ".local" / "share"))
    if not data_base.is_absolute():
        raise RuntimeError("XDG_DATA_HOME must be absolute")
    prefix = data_base / "cortex" / "mcp" / "playwright"
    cli = _install(prefix)
    node = shutil.which("node")
    if not node:
        raise RuntimeError("node was not found")
    node = str(Path(node).resolve())
    count = probe(node, cli)

    config_dir = registry.config_root()
    _private_dir(config_dir)
    path = config_dir / "mcp-servers.json"
    lock_path = config_dir / "mcp-servers.lock"
    with open(lock_path, "a+b") as lock:
        os.fchmod(lock.fileno(), 0o600)
        fcntl.flock(lock, fcntl.LOCK_EX)
        # Use the product loader to validate all existing entries before editing.
        load_mcp_servers(str(ROOT), environ=os.environ)
        raw = registry.read_owner_file(path)
        document = json.loads(raw) if raw is not None else {"version": 1, "servers": {}}
        servers = document.get("servers")
        if document.get("version") != 1 or not isinstance(servers, dict):
            raise RuntimeError("MCP registry has an unsupported shape")
        desired = {"command": node, "args": [str(cli), "--headless", "--isolated"], "roles": ["worker"], "engines": ["claude-agent-acp"], "enabled": True}
        if servers.get(NAME) != desired:
            servers[NAME] = desired
            registry.atomic_write_private(path, (json.dumps(document, indent=2, ensure_ascii=False) + "\n").encode(), directory=config_dir)
        effective = load_mcp_servers(str(ROOT), environ=os.environ)
        actual = next((entry for entry in effective if entry["name"] == NAME), None)
        if actual is None or actual.get("roles") != ["worker"] or actual.get("engines") != ["claude-agent-acp"] or actual.get("args") != desired["args"]:
            raise RuntimeError("effective registry did not contain the Claude-worker Playwright entry")
    print(f"Registered {PACKAGE}@{VERSION} for Claude workers; MCP tools/list: {count} tools.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, RuntimeError) as error:
        print(f"Playwright MCP setup failed: {error}", file=sys.stderr)
        raise SystemExit(1) from None
