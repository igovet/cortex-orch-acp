"""Cortex's own engine installs: update when newer, keep current when offline."""
from __future__ import annotations

import json
import os

from cortex_runtime.connections import discovery, engine_update

PACKAGE = "@agentclientprotocol/claude-agent-acp"
# A fake npm: `view` prints $FAKE_LATEST (or fails when unset, like offline),
# `install --prefix DIR pkg@ver` writes a minimal package there.
FAKE_NPM = """#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
if args[0] == "view":
    latest = os.environ.get("FAKE_LATEST")
    if not latest:
        sys.exit("npm ERR! network")
    print(json.dumps(latest))
elif args[0] == "install":
    prefix, spec = args[args.index("--prefix") + 1], args[-1]
    name, version = spec.rsplit("@", 1)
    pkg = os.path.join(prefix, "node_modules", name)
    os.makedirs(os.path.join(pkg, "dist"))
    open(os.path.join(pkg, "dist", "index.js"), "w").write("// adapter\\n")
    json.dump({"name": name, "version": version, "bin": {"claude-agent-acp": "dist/index.js"}},
              open(os.path.join(pkg, "package.json"), "w"))
"""


def _environ(tmp_path, latest=None):
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir(exist_ok=True)
    for name, body in (("npm", FAKE_NPM), ("node", "#!/bin/sh\n")):
        (bin_dir / name).write_text(body)
        (bin_dir / name).chmod(0o755)
    env = {"HOME": str(tmp_path), "PATH": f"{bin_dir}{os.pathsep}/usr/bin:/bin"}
    if latest:
        env["FAKE_LATEST"] = latest
    return env


def test_installs_newer_keeps_one_previous_and_survives_offline(tmp_path):
    for version in ("0.78.0", "0.80.0", "0.81.0"):
        assert engine_update.update_engine("claude-agent-acp", environ=_environ(tmp_path, version)) == version
    root = discovery.owned_root(_environ(tmp_path)) / "claude-agent-acp"
    assert sorted(p.name for p in discovery._subdirs(root)) == ["0.80.0", "0.81.0"]

    # Same version again, or offline: nothing changes.
    assert engine_update.update_engine("claude-agent-acp", environ=_environ(tmp_path, "0.81.0")) is None
    assert engine_update.update_all(environ=_environ(tmp_path)) == {}

    found = discovery.discover_engine("claude-agent-acp", environ=_environ(tmp_path))
    assert found.version == "0.81.0"
    assert found.launch_command[1].startswith(str(root / "0.81.0"))


def test_never_uses_a_client_registry_copy(tmp_path):
    zed = tmp_path / ".local/share/zed/external_agents/registry/npx/claude-acp/node_modules" / PACKAGE
    (zed / "dist").mkdir(parents=True)
    (zed / "dist/index.js").write_text("")
    (zed / "package.json").write_text(json.dumps({"version": "0.78.0", "bin": {"x": "dist/index.js"}}))
    assert discovery.discover_engine("claude-agent-acp", environ=_environ(tmp_path)) is None
