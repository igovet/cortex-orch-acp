"""Prompt-free catalog refresh in its own process session.

Paseo reads a provider's models by starting a short-lived Cortex ACP process,
waiting for ``session/new`` and then stopping it (SIGTERM, then SIGKILL about
a second later). The refresh that ``acp.agent`` starts after ``session/new``
used to run on a daemon thread of that process. A Codex or OpenCode refresh
finishes within that second; a Claude refresh does not: it observes every
model's own effort selector with one ``session/set_config_option`` each
(``catalog.model_efforts``; about 12 s for eleven models, measured
2026-10-08). The thread died with the process, so Paseo's Refresh never saved
a Claude catalog, and a model the engine had just started advertising (Haiku
5.5) could not appear through it.

:func:`run` therefore performs :func:`catalog.refresh.perform_refresh` in a
child process started with ``start_new_session=True``: the child outlives the
ACP process that asked for it and saves its snapshot either way, while a
caller that is still alive receives the child's outcome. One refresh per
connection runs at a time (an advisory lock in that connection's catalog
directory); a concurrent request reports ``refresh_in_progress`` instead of
starting a second engine. No prompt is ever sent (see ``catalog.refresh``).
"""
from __future__ import annotations

from datetime import datetime, timezone
import json
import os
from pathlib import Path
import signal
import subprocess
import sys

_SCRIPT = Path(__file__).resolve()
_SRC_ROOT = _SCRIPT.parents[2]
LOCK_NAME = ".refresh.lock"
# perform_refresh bounds each engine call; this only bounds how long a caller
# waits for the child's answer.
DEFAULT_WAIT_SECONDS = 300.0


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def _failed(connection_id: str, reason: str) -> dict:
    return dict(connection_id=connection_id, success=False, reason=reason, snapshot_id=None)


def refresh_locked(connection_id: str, workspace_root: str, environ: dict[str, str]) -> dict:
    """Refresh one registered connection unless another refresh of it is
    running. Never raises; returns the outcome as a JSON-ready dict."""
    import fcntl

    from ..connections import registry
    from ..portable.errors import PortableError
    from . import refresh, store

    try:
        directory = store._snapshot_dir(connection_id, environ)
        registry.ensure_private_dir(directory, root=registry.state_root(environ), environ=environ)
        descriptor = os.open(directory / LOCK_NAME, os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0), 0o600)
    except (PortableError, OSError):
        return _failed(connection_id, "refresh_lock_unavailable")
    try:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            return _failed(connection_id, "refresh_in_progress")
        try:
            record = registry.read_registry(environ).connections.get(connection_id)
        except PortableError:
            return _failed(connection_id, "registry_unreadable")
        if record is None:
            return _failed(connection_id, "connection_not_found")
        outcome = refresh.perform_refresh(record, workspace_root, environ=environ)
        return dict(connection_id=outcome.connection_id, success=outcome.success, reason=outcome.reason,
                    snapshot_id=outcome.snapshot_id)
    finally:
        os.close(descriptor)


def main(argv=None) -> int:
    """``detached_refresh.py <connection_id> <workspace_root>``: one refresh,
    its outcome printed as one JSON line."""
    args = sys.argv[1:] if argv is None else list(argv)
    if len(args) != 2:
        sys.stderr.write("usage: detached_refresh.py <connection_id> <workspace_root>\n")
        return 2
    outcome = refresh_locked(args[0], args[1], dict(os.environ))
    try:
        sys.stdout.write(json.dumps(outcome) + "\n")
        sys.stdout.flush()
    except OSError:
        pass  # the requester is gone; the snapshot is already saved
    return 0


def run(record, workspace_root, *, environ=None, wait: float = DEFAULT_WAIT_SECONDS):
    """Refresh ``record`` in a detached child and wait for its outcome.

    Returns a :class:`catalog.refresh.RefreshOutcome`; never raises. If this
    process ends first, the child still finishes and saves its snapshot.
    """
    from .refresh import RefreshOutcome

    at = _now()
    env = dict(os.environ if environ is None else environ)
    argv = [sys.executable, "-B", str(_SCRIPT), record.id, str(workspace_root)]
    try:
        child = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                 stderr=subprocess.DEVNULL, env=env, start_new_session=True)
    except OSError as error:
        return RefreshOutcome(record.id, True, False, f"refresh_spawn_failed:{type(error).__name__}", None, at)
    try:
        output, _ = child.communicate(timeout=wait)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except OSError:
            pass
        child.wait()
        return RefreshOutcome(record.id, True, False, "refresh_timeout", None, at)
    try:
        outcome = json.loads(output.decode("utf-8").strip().splitlines()[-1])
    except (UnicodeDecodeError, ValueError, IndexError):
        return RefreshOutcome(record.id, True, False, f"refresh_child_failed:{child.returncode}", None, at)
    return RefreshOutcome(record.id, True, outcome.get("success") is True, outcome.get("reason"),
                          outcome.get("snapshot_id"), at)


if __name__ == "__main__":
    # Started by path (see ``run``). Replace this directory on sys.path (its
    # module names must not shadow anything) with the source root, and run
    # the package module itself so its relative imports resolve.
    sys.path[0] = str(_SRC_ROOT)
    from cortex_runtime.catalog import detached_refresh

    raise SystemExit(detached_refresh.main())
