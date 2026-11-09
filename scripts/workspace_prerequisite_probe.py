#!/usr/bin/env python3
"""Read-only, no-paid prerequisite inventory; missing items do not fail inventory.

Exit 0 means the inventory completed, NOT that a native build is qualified.
Child output is deliberately discarded: only fixed probe commands, exit codes
and present/missing states are printed, never environment or process listings.
"""
import json
import subprocess
import sys


def probe(name, command):
    reason = None
    try:
        result = subprocess.run(command, stdin=subprocess.DEVNULL,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                timeout=8, check=False)
        code = result.returncode
    except FileNotFoundError:
        code, reason = 127, "executable_not_found"
    except subprocess.TimeoutExpired:
        code, reason = 124, "timeout"
    except OSError:
        code, reason = 126, "execution_unavailable"
    item = {"name": name, "command": command, "exit_code": code,
            "status": "present" if code == 0 else "missing"}
    if reason:
        item["reason"] = reason
    return item


def main():
    commands = [
        ("node", ["node", "--version"]),
        ("npm", ["npm", "--version"]),
        ("python", [sys.executable, "--version"]),
        ("python-pytest", [sys.executable, "-B", "-c", "import pytest"]),
        ("python-pytest-xdist", [sys.executable, "-B", "-c", "import xdist"]),
        ("rustc", ["rustc", "--version"]),
        ("cargo", ["cargo", "--version"]),
        ("user-service-manager", ["systemctl", "--user", "show", "--property=Version", "--value"]),
    ]
    for library in ("webkit2gtk-4.1", "gtk+-3.0", "openssl",
                    "ayatana-appindicator3-0.1", "librsvg-2.0", "xdo"):
        commands.append((library, ["pkg-config", "--exists", library]))
    print(json.dumps({"items": [probe(name, command) for name, command in commands],
                      "qualification": "inventory_only"}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
