#!/usr/bin/env python3
"""Run a Paseo CLI command against an explicitly isolated local home."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys


def resolved(path: str) -> Path:
    candidate = Path(path).expanduser()
    if not candidate.is_absolute():
        raise ValueError("--home must be an absolute path")
    return candidate.resolve(strict=False)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--paseo", default="/opt/Paseo/resources/bin/paseo")
    parser.add_argument(
        "--home",
        default=str(Path.home() / ".paseo-dev"),
        help="absolute isolated Paseo home (default: ~/.paseo-dev)",
    )
    parser.add_argument("--check", action="store_true", help="validate only")
    parser.add_argument("command", nargs=argparse.REMAINDER, help="Paseo command after --")
    args = parser.parse_args(argv)
    try:
        home = resolved(args.home)
    except ValueError as error:
        print(f"cortex_paseo_dev: {error}", file=sys.stderr)
        return 2
    production = (Path.home() / ".paseo").resolve(strict=False)
    if home == production:
        print("cortex_paseo_dev: refusing production Paseo home", file=sys.stderr)
        return 2
    paseo = Path(args.paseo)
    if not paseo.is_absolute() or not paseo.is_file() or not os.access(paseo, os.X_OK):
        print("cortex_paseo_dev: --paseo must name an executable absolute path", file=sys.stderr)
        return 2
    command = list(args.command)
    if command[:1] == ["--"]:
        command = command[1:]
    if any(item == "--home" or item.startswith("--home=") or item == "--host" or item.startswith("--host=") for item in command):
        print("cortex_paseo_dev: command may not override --home or select --host", file=sys.stderr)
        return 2
    if args.check:
        if command:
            print("cortex_paseo_dev: --check accepts no Paseo command", file=sys.stderr)
            return 2
        print(json.dumps({"paseo": str(paseo), "home": str(home), "production_home": str(production)}))
        return 0
    if not command:
        print("cortex_paseo_dev: provide a Paseo command after --", file=sys.stderr)
        return 2
    return subprocess.run([str(paseo), "--home", str(home), *command], check=False).returncode


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
