"""The Playwright setup must preserve the owner's existing MCP registry."""
from __future__ import annotations

import importlib.util
import json
from pathlib import Path


def _setup_module():
    path = Path(__file__).resolve().parents[1] / "scripts" / "cortex_install_playwright_mcp.py"
    spec = importlib.util.spec_from_file_location("cortex_install_playwright_mcp", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_preserves_existing_private_registry_and_is_idempotent(tmp_path, monkeypatch, capsys):
    module = _setup_module()
    home = tmp_path / "home"
    home.mkdir()
    config = home / ".config" / "cortex"
    config.mkdir(parents=True, mode=0o700)
    path = config / "mcp-servers.json"
    existing = {
        "version": 1,
        "servers": {"existing": {
            "command": "/bin/true", "args": [], "env": {"TOKEN": "private"}, "enabled": True,
        }},
    }
    path.write_text(json.dumps(existing), encoding="utf-8")
    path.chmod(0o600)
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.delenv("XDG_CONFIG_HOME", raising=False)
    monkeypatch.delenv("XDG_DATA_HOME", raising=False)
    monkeypatch.setattr(Path, "home", lambda: home)
    monkeypatch.setattr(module, "_install", lambda prefix: prefix / "cli.js")
    monkeypatch.setattr(module, "probe", lambda node, cli: 26)
    monkeypatch.setattr(module.shutil, "which", lambda name: "/usr/bin/node")

    assert module.main() == 0
    first = path.read_bytes()
    assert module.main() == 0
    assert path.read_bytes() == first
    registered = json.loads(first)
    assert registered["servers"]["existing"] == existing["servers"]["existing"]
    assert registered["servers"]["playwright"]["roles"] == ["worker"]
    assert registered["servers"]["playwright"]["engines"] == ["claude-agent-acp"]
    assert "private" not in capsys.readouterr().out
