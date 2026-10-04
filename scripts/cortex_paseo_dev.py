#!/usr/bin/env python3
"""Run a Paseo CLI command against an explicitly isolated local home."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import select
import shutil
import signal
import socket
import stat
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
    if argv and argv[0] == 'binding':
        parser = argparse.ArgumentParser(description='Read-only dev qualification binding; never start or mutate an agent.')
        parser.add_argument('--agent', required=True, help='exact full owned UUID, not a prefix or title')
        parser.add_argument('--workspace', required=True, help='exact owned workspace id')
        parser.add_argument('--project', required=True, help='absolute disposable project outside this checkout')
        parser.add_argument('--timeout', type=int, default=15, help='total seconds (1-30)')
        return _run_binding(parser.parse_args(argv[1:]))
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
    # A wrapper invoked from an active Paseo agent must not target that agent.
    env.pop("PASEO_AGENT_ID", None)
    env.pop("PASEO_AGENT_CWD", None)
    # Paseo bundles server plugins as CJS and does not expose their source path.
    # Pass this worktree root explicitly to the isolated daemon for directory plugins.
    env["CORTEX_PASEO_SOURCE_ROOT"] = str(Path(__file__).resolve().parent.parent)
    return subprocess.run([str(paseo), "--home", str(home), *command], env=env, check=False).returncode


def _private_json(path: Path) -> dict:
    info = path.lstat()
    # Paseo writes paseo.pid under the user's umask (e.g. 0664); privacy comes from
    # the 0700 home checked by the caller, so refuse only world-writable files.
    if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
            or info.st_nlink != 1 or stat.S_IMODE(info.st_mode) & 0o002 or info.st_size > 4096):
        raise ValueError('binding_endpoint_refused')
    def identity(value):
        return tuple(getattr(value, key) for key in ('st_dev', 'st_ino', 'st_mode', 'st_uid',
                                                    'st_nlink', 'st_size', 'st_mtime_ns', 'st_ctime_ns'))
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        opened = os.fstat(fd)
        if identity(opened) != identity(info):
            raise ValueError('binding_endpoint_refused')
        raw = os.read(fd, 4097)
        if (identity(os.fstat(fd)) != identity(opened) or identity(path.lstat()) != identity(opened)
                or len(raw) > 4096):
            raise ValueError('binding_endpoint_refused')
    finally:
        os.close(fd)
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError('binding_endpoint_refused')
            result[key] = value
        return result
    result = json.loads(raw, object_pairs_hook=unique)
    if not isinstance(result, dict):
        raise ValueError('binding_endpoint_refused')
    return result


def _binding_endpoint(home: Path) -> tuple[str, str]:
    # Only endpoint/PID provenance, never credentials, config, registry or logs.
    # Require both homes' metadata so a copied production endpoint fails closed.
    production = Path.home() / '.paseo'
    for directory in (home, production):
        if str(directory.resolve(strict=True)) != str(directory) or not directory.is_dir():
            raise ValueError('binding_endpoint_refused')
        info = directory.stat()
        if info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) & 0o077:
            raise ValueError('binding_endpoint_refused')
    dev = _private_json(home / 'paseo.pid')
    prod = _private_json(production / 'paseo.pid')
    for metadata in (dev, prod):
        if (metadata.get('uid') != os.getuid() or metadata.get('hostname') != socket.gethostname()
                or type(metadata.get('pid')) is not int or metadata['pid'] < 1
                or any(not isinstance(metadata.get(key), str) or not metadata[key]
                       for key in ('listen', 'startedAt', 'serverId'))):
            raise ValueError('binding_endpoint_refused')
    listen = dev.get('listen')
    match = re.fullmatch(r'127\.0\.0\.1:([0-9]{1,5})', listen or '')
    if (match is None or not 1 <= int(match[1]) <= 65535
            or any(dev.get(k) == prod.get(k) for k in ('listen', 'pid', 'serverId'))):
        raise ValueError('binding_endpoint_refused')
    # Heartbeats may touch mtime, so fence the retained identity, not the inode.
    identity = {k: dev.get(k) for k in ('listen', 'pid', 'startedAt', 'serverId', 'uid', 'hostname')}
    return 'ws://' + listen + '/ws', hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()


def _run_binding(args) -> int:
    process = None
    diagnostic = None
    stage = 'input-validation'
    codes = {'binding_endpoint_refused', 'binding_endpoint_drift', 'binding_host_refused',
             'binding_host_drift', 'binding_storage_refused', 'binding_storage_drift',
             'binding_invalid_input', 'binding_timeout', 'binding_output_overflow',
             'binding_anonymous_connection_refused', 'binding_anonymous_host_or_storage_refused',
             'binding_host_or_storage_refused', 'binding_refused'}
    try:
        root = Path(__file__).resolve().parents[1]
        home = Path.home() / '.paseo-dev'
        project = Path(args.project)
        if (not re.fullmatch(r'[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}', args.agent)
                or not re.fullmatch(r'[A-Za-z0-9_-]{1,256}', args.workspace)
                or not project.is_absolute() or str(project.resolve(strict=True)) != str(project)
                or not project.is_dir() or not 1 <= args.timeout <= 30
                or any(project == p or project.is_relative_to(p) or p.is_relative_to(project)
                       for p in (root, home, Path.home() / '.paseo'))):
            raise ValueError('binding_invalid_input')
        stage = 'endpoint-provenance'
        url, endpoint_digest = _binding_endpoint(home)
        env = isolated_environment(home)
        for key in list(env):
            if key.startswith('PASEO_'):
                del env[key]
        env['PYTHONDONTWRITEBYTECODE'] = '1'
        argv = ['node', '--experimental-strip-types', str(root / 'integrations/paseo/server/qualification-binding.ts'),
                url, args.agent, args.workspace, str(project), str(args.timeout), sys.executable, str(root)]
        stage = 'helper-spawn'
        process = subprocess.Popen(argv, env=env, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                   start_new_session=True)
        stage = 'helper-output'
        # Bounded pipe read: do not capture an unlimited downstream body.
        import time
        deadline = time.monotonic() + args.timeout
        output = bytearray()
        assert process.stdout is not None
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise ValueError('binding_timeout')
            ready, _, _ = select.select([process.stdout], [], [], remaining)
            if not ready:
                raise ValueError('binding_timeout')
            chunk = os.read(process.stdout.fileno(), 4096)
            if not chunk:
                break
            output.extend(chunk)
            if len(output) > 8192:
                raise ValueError('binding_output_overflow')
        status = process.wait(timeout=max(0.01, deadline - time.monotonic()))
        if status != 0:
            try:
                failure = json.loads(output)
            except (ValueError, UnicodeDecodeError):
                failure = {}
            code = failure.get('error') if isinstance(failure, dict) else None
            # Local process observations remain useful even when no safe child
            # diagnostic exists (including import failures before main runs).
            diagnostic = {'stage': stage, 'reason': 'subprocess_terminated' if status < 0 else 'subprocess_failed'}
            if isinstance(code, str) and code in codes:
                # Never forward arbitrary child output, paths or exception text.
                detail = failure.get('diagnostic')
                reasons = {'inspection_invalid_binding', 'inspection_binding_mismatch', 'unsafe_storage',
                           'inspection_uncheckpointed_wal', 'legacy_storage_migration_required', 'unsupported_storage',
                           'inspection_unsafe_parameter', 'inspection_evidence_overflow', 'inspection_snapshot_changed',
                           'inspection_invalid_cursor', 'inspection_busy', 'inspection_refused'}
                child_reasons = {'spawn_failed', 'subprocess_failed', 'subprocess_terminated',
                                 'timeout', 'output_overflow', 'invalid_output'}
                if (status == 1 and code == 'binding_storage_refused' and failure.get('available') is False
                        and set(failure) == {'available', 'error', 'diagnostic'}
                        and isinstance(detail, dict) and set(detail) == {'stage', 'reason'}
                        and isinstance(detail.get('stage'), str)
                        and detail.get('stage') in {'task-discovery', 'endpoint-provenance'}
                        and isinstance(detail.get('reason'), str)
                        and (detail['reason'] in child_reasons
                             or (detail['stage'] == 'task-discovery' and detail['reason'] in reasons))
                        # Node's complete canonical output also excludes duplicate/escaped keys.
                        and output == (json.dumps(failure, separators=(',', ':')) + '\n').encode()):
                    diagnostic = detail
                elif (status == 1 and set(failure) == {'available', 'error'}
                      and failure.get('available') is False
                      and output == (json.dumps(failure, separators=(',', ':')) + '\n').encode()):
                    # A complete safe helper error is already discriminating.
                    diagnostic = None
                raise ValueError(code)
            raise ValueError('binding_host_or_storage_refused')
        # Zero exit is not evidence of a complete exact binding. Validate the
        # fixed helper envelope before adding wrapper provenance or forwarding.
        result = json.loads(output)
        if not _valid_binding_output(result, output, args):
            raise ValueError('binding_refused')
        stage = 'endpoint-recheck'
        if _binding_endpoint(home) != (url, endpoint_digest):
            raise ValueError('binding_endpoint_drift')
        result['endpoint_sha256'] = endpoint_digest
        print(json.dumps(result, sort_keys=True))
        return 0
    except Exception as error:
        if process is not None:
            _terminate_process_group(process, timeout=1)
        code = str(error) if isinstance(error, ValueError) and str(error) in codes else 'binding_refused'
        if diagnostic is None:
            reason = ('timeout' if code == 'binding_timeout' or isinstance(error, subprocess.TimeoutExpired)
                      else 'output_overflow' if code == 'binding_output_overflow'
                      else 'metadata_unavailable' if stage in {'endpoint-provenance', 'endpoint-recheck'} and isinstance(error, OSError)
                      else 'path_unavailable' if stage == 'input-validation' and isinstance(error, OSError)
                      else 'spawn_failed' if stage == 'helper-spawn'
                      else 'invalid_output' if stage == 'helper-output' and code == 'binding_refused'
                      else None)
            if reason:
                diagnostic = {'stage': stage, 'reason': reason}
        result = {'available': False, 'error': code}
        if diagnostic is not None:
            result['diagnostic'] = diagnostic
        print(json.dumps(result))
        return 1


def _valid_binding_output(result, output, args) -> bool:
    if (not isinstance(result, dict)
            or set(result) != {'available', 'host', 'binding', 'binding_sha256', 'provenance'}
            or result['available'] is not True
            or result['provenance'] != 'single-agent-api/refetch + guarded-immutable-session-task-current-owner'
            or output != (json.dumps(result, separators=(',', ':'), ensure_ascii=False) + '\n').encode()):
        return False
    host, binding = result['host'], result['binding']
    if (not isinstance(host, dict) or not isinstance(binding, dict)
            or set(host) != {'agent_id', 'workspace_id', 'project_root', 'session_id', 'wrapper_sha256'}
            or set(binding) != {'owner_epoch', 'project_root', 'session_id', 'task_id'}
            or host['agent_id'] != args.agent or host['workspace_id'] != args.workspace
            or host['project_root'] != args.project or binding['project_root'] != args.project
            or not isinstance(host['session_id'], str) or not re.fullmatch(r'acps_[0-9a-f]{12}', host['session_id'])
            or binding['session_id'] != host['session_id']
            or not isinstance(binding['task_id'], str) or not re.fullmatch(r't_[0-9a-f]{12}', binding['task_id'])
            or type(binding['owner_epoch']) is not int or not 1 <= binding['owner_epoch'] <= 9007199254740991
            or any(not isinstance(value, str) or not re.fullmatch(r'[0-9a-f]{64}', value)
                   for value in (host['wrapper_sha256'], result['binding_sha256']))):
        return False
    canonical = json.dumps(binding, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()
    return hashlib.sha256(canonical).hexdigest() == result['binding_sha256']


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
