#!/usr/bin/env python3
"""Run a Paseo CLI command against an explicitly isolated local home."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import select
import shutil
import signal
import socket
import subprocess
import sys


def resolved(path: str) -> Path:
    candidate = Path(path).expanduser()
    if not candidate.is_absolute():
        raise ValueError("--home must be an absolute path")
    return candidate.resolve(strict=False)


def cortex_xdg_paths(home: Path) -> tuple[Path, Path]:
    """Return the Cortex-only XDG roots for one isolated Paseo home."""
    root = home / "cortex-xdg"
    return root / "config", root / "state"


def isolated_environment(home: Path) -> dict[str, str]:
    """Keep a dev Paseo process out of the user's normal Cortex XDG state."""
    config_home, state_home = cortex_xdg_paths(home)
    env = os.environ.copy()
    env["XDG_CONFIG_HOME"] = str(config_home)
    env["XDG_STATE_HOME"] = str(state_home)
    env["CORTEX_PASEO_SOURCE_ROOT"] = str(Path(__file__).resolve().parent.parent)
    return env


def _stop_xvfb(process: subprocess.Popen[str]) -> None:
    """Stop an Xvfb process started for one GUI invocation."""
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def _start_xvfb() -> tuple[subprocess.Popen[str], str]:
    """Start a private Xvfb and return its process and display name."""
    xvfb = shutil.which("Xvfb")
    if xvfb is None:
        raise RuntimeError("--xvfb requires Xvfb with -displayfd support; install Xvfb or run without --xvfb")
    process = subprocess.Popen(
        [xvfb, "-displayfd", "1", "-screen", "0", "1440x900x24", "-nolisten", "tcp"],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    assert process.stdout is not None
    ready, _, _ = select.select([process.stdout], [], [], 5)
    if not ready:
        _stop_xvfb(process)
        raise RuntimeError("Xvfb did not report a private display within 5 seconds")
    display = process.stdout.readline().strip()
    if not re.fullmatch(r"[0-9]+", display):
        _stop_xvfb(process)
        raise RuntimeError("Xvfb did not report a valid private display")
    return process, f":{display}"


def _terminate_process_group(process: subprocess.Popen, timeout: float = 5) -> None:
    """Terminate a process started with start_new_session, escalating to SIGKILL."""
    if process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=timeout)
        return
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        return
    process.wait()


def _select_debug_port(requested_port: int | None) -> int:
    """Choose a loopback port and reject explicitly requested ports already in use."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.bind(("127.0.0.1", requested_port or 0))
        return int(probe.getsockname()[1])


def _live_singleton_owner(user_data_dir: Path) -> int | None:
    """Return the live local PID holding Electron's profile lock, if any.

    Paseo calls requestSingleInstanceLock() keyed by its userData directory and
    quits with status 0 when another instance holds it, so a second launch on
    the same profile never exposes its own DevTools port.
    """
    try:
        target = os.readlink(user_data_dir / "SingletonLock")
    except OSError:
        return None
    host, _, pid_text = target.rpartition("-")
    if host != socket.gethostname() or not pid_text.isdigit():
        return None
    pid = int(pid_text)
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return None
    except PermissionError:
        pass
    return pid


def main(argv: list[str]) -> int:
    # Check if this is a gui subcommand invocation
    if argv and argv[0] == "gui":
        parser = argparse.ArgumentParser(description=__doc__, add_help=True)
        parser.add_argument("--paseo", default="/opt/Paseo/Paseo", help="Paseo Electron executable path")
        parser.add_argument("--home", default=str(Path.home() / ".paseo-dev"), help="isolated Paseo home")
        parser.add_argument(
            "--user-data-dir",
            default=str(Path.home() / ".config" / "Paseo-dev"),
            help="isolated Electron profile (default: ~/.config/Paseo-dev)",
        )
        parser.add_argument("--port", type=int, help="Chrome DevTools Protocol port (default: choose a free port)")
        parser.add_argument("--xvfb", action="store_true", help="launch a private Xvfb display for this GUI invocation")

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

    try:
        user_data_dir = resolved(args.user_data_dir)
    except ValueError:
        print("cortex_paseo_dev gui: --user-data-dir must be an absolute path", file=sys.stderr)
        return 2
    if user_data_dir == (Path.home() / ".config" / "Paseo").resolve(strict=False):
        print("cortex_paseo_dev gui: refusing production Paseo Electron profile", file=sys.stderr)
        return 2

    paseo = Path(args.paseo)
    if not paseo.is_absolute() or not paseo.is_file() or not os.access(paseo, os.X_OK):
        print(f"cortex_paseo_dev gui: --paseo must name an executable absolute path (got {args.paseo})", file=sys.stderr)
        return 2

    if args.port is not None and (args.port < 1 or args.port > 65535):
        print(f"cortex_paseo_dev gui: --port must be 1-65535 (got {args.port})", file=sys.stderr)
        return 2

    owner = _live_singleton_owner(user_data_dir)
    if owner is not None:
        print(
            f"cortex_paseo_dev gui: Paseo PID {owner} already holds {user_data_dir}; "
            "a second launch would quit at once without DevTools. Reuse or stop that instance first.",
            file=sys.stderr,
        )
        return 3

    try:
        port = _select_debug_port(args.port)
    except OSError as error:
        if args.port is None:
            print(f"cortex_paseo_dev gui: could not select a free loopback port: {error}", file=sys.stderr)
        else:
            print(f"cortex_paseo_dev gui: --port {args.port} is unavailable on 127.0.0.1: {error}", file=sys.stderr)
        return 2

    flags = ["--remote-debugging-address=127.0.0.1", f"--remote-debugging-port={port}"]
    if args.xvfb:
        flags.append("--ozone-platform=x11")

    env = isolated_environment(home)
    env["PASEO_HOME"] = str(home)
    env["PASEO_ELECTRON_USER_DATA_DIR"] = str(user_data_dir)
    env["PASEO_ELECTRON_FLAGS"] = " ".join(flags)

    # Do not set ELECTRON_RUN_AS_NODE; it interferes with isolation
    env.pop("ELECTRON_RUN_AS_NODE", None)

    xvfb_process = None
    if args.xvfb:
        try:
            xvfb_process, display = _start_xvfb()
        except RuntimeError as error:
            print(f"cortex_paseo_dev gui: {error}", file=sys.stderr)
            return 2
        env["DISPLAY"] = display

    print(f"cortex_paseo_dev gui: launching isolated Paseo at {home}")
    print(f"  Chrome DevTools Protocol: http://127.0.0.1:{port}")
    if xvfb_process is not None:
        print(f"  Private Xvfb display: {env['DISPLAY']}")

    electron_process: subprocess.Popen | None = None

    def _handle_termination(signum, _frame):
        raise SystemExit(128 + signum)

    previous_handlers = {
        sig: signal.signal(sig, _handle_termination) for sig in (signal.SIGTERM, signal.SIGINT)
    }
    try:
        electron_process = subprocess.Popen([str(paseo)], env=env, start_new_session=True)
        returncode = electron_process.wait()
        owner = _live_singleton_owner(user_data_dir)
        if owner is not None and owner != electron_process.pid:
            print(
                f"cortex_paseo_dev gui: Paseo exited because PID {owner} holds {user_data_dir}; "
                f"no DevTools endpoint is available on port {port}.",
                file=sys.stderr,
            )
            return 3
        return returncode
    finally:
        for sig, handler in previous_handlers.items():
            signal.signal(sig, handler)
        if electron_process is not None:
            _terminate_process_group(electron_process)
        if xvfb_process is not None:
            _stop_xvfb(xvfb_process)


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

    env = isolated_environment(home)
    # Paseo bundles server plugins as CJS and does not expose their source path.
    # Pass this worktree root explicitly to the isolated daemon for directory plugins.
    env["CORTEX_PASEO_SOURCE_ROOT"] = str(Path(__file__).resolve().parent.parent)
    return subprocess.run([str(paseo), "--home", str(home), *command], env=env, check=False).returncode


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
