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
    # Check if this is a gui subcommand invocation
    if argv and argv[0] == "gui":
        parser = argparse.ArgumentParser(description=__doc__, add_help=True)
        parser.add_argument("--paseo", default="/opt/Paseo/Paseo", help="Paseo Electron executable path")
        parser.add_argument("--home", default=str(Path.home() / ".paseo-dev"), help="isolated Paseo home")
        parser.add_argument("--port", type=int, default=9222, help="Chrome DevTools Protocol port (default: 9222)")
        parser.add_argument("--xvfb", action="store_true", help="Use --ozone-platform=x11 for Xvfb")

        args = parser.parse_args(argv[1:])
        return _run_gui(args)
    else:
        # Original CLI mode
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
        return _run_cli(args)


def _run_gui(args) -> int:
    """Launch the Paseo Electron desktop with isolated environment."""
    try:
        home = resolved(args.home)
    except ValueError as error:
        print(f"cortex_paseo_dev gui: {error}", file=sys.stderr)
        return 2

    production = (Path.home() / ".paseo").resolve(strict=False)
    if home == production:
        print("cortex_paseo_dev gui: refusing production Paseo home", file=sys.stderr)
        return 2

    paseo = Path(args.paseo)
    if not paseo.is_absolute() or not paseo.is_file() or not os.access(paseo, os.X_OK):
        print(f"cortex_paseo_dev gui: --paseo must name an executable absolute path (got {args.paseo})", file=sys.stderr)
        return 2

    if args.port < 1 or args.port > 65535:
        print(f"cortex_paseo_dev gui: --port must be 1-65535 (got {args.port})", file=sys.stderr)
        return 2

    flags = [f"--remote-debugging-address=127.0.0.1", f"--remote-debugging-port={args.port}"]
    if args.xvfb:
        flags.append("--ozone-platform=x11")

    env = os.environ.copy()
    env["PASEO_HOME"] = str(home)
    env["CORTEX_PASEO_SOURCE_ROOT"] = str(Path(__file__).resolve().parent.parent)
    env["PASEO_ELECTRON_USER_DATA_DIR"] = str(Path.home() / ".config" / "Paseo-dev")
    env["PASEO_ELECTRON_FLAGS"] = " ".join(flags)

    # Do not set ELECTRON_RUN_AS_NODE; it interferes with isolation
    env.pop("ELECTRON_RUN_AS_NODE", None)

    print(f"cortex_paseo_dev gui: launching isolated Paseo at {home}")
    print(f"  Chrome DevTools Protocol: http://127.0.0.1:{args.port}")
    return subprocess.run([str(paseo)], env=env, check=False).returncode


def _run_cli(args) -> int:
    """Run Paseo CLI commands against the isolated home."""
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

    env = os.environ.copy()
    # Paseo bundles server plugins as CJS and does not expose their source path.
    # Pass this worktree root explicitly to the isolated daemon for directory plugins.
    env["CORTEX_PASEO_SOURCE_ROOT"] = str(Path(__file__).resolve().parent.parent)
    return subprocess.run([str(paseo), "--home", str(home), *command], env=env, check=False).returncode


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
