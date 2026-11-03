"""Bounded, prompt-free first-run setup shared by ACP and plugin startup.

Only Cortex-owned qualified adapters and existing native login markers are used.
Status is diagnostic; the connection registry and observed catalogs stay authoritative.
"""
from __future__ import annotations

import fcntl
import json
import os
import re
import stat
import tempfile

from . import discovery, engine_update, onboarding, registry

_PHASES = {"downloading", "connecting", "refreshing_models", "ready", "download_failed",
           "login_required", "opted_out", "catalog_failed", "setup_failed", "downloaded"}
_BUSY = {"downloading", "connecting", "refreshing_models"}
_LOCK = "automatic-setup.lock"
# A failure reason is a fixed code, optionally with an exception type or
# error-code suffix (see catalog.refresh.RefreshOutcome.reason); never free text.
_REASON = re.compile(r"[a-z][a-z0-9_]{0,63}(?::[A-Za-z][A-Za-z0-9_]{0,63})?")


def _safe_reason(reason):
    return reason if isinstance(reason, str) and _REASON.fullmatch(reason) else None


def _status_path(engine, environ):
    if engine not in discovery.SUPPORTED_ENGINES:
        raise ValueError("Unsupported automatic engine.")
    return registry.config_root(environ) / f"first-run-{engine}.json"


def _status(engine, phase, environ, reason=None):
    path = _status_path(engine, environ)
    registry.ensure_private_dir(path.parent, root=registry.config_root(environ), environ=environ)
    body = {"phase": phase}
    if phase == "catalog_failed" and _safe_reason(reason):
        body["reason"] = reason
    registry.atomic_write_private(path, json.dumps(body).encode(), directory=path.parent)


def read_status(engine, environ=None):
    """Passive, allowlisted status. A terminated worker never looks still busy."""
    if engine not in discovery.SUPPORTED_ENGINES:
        return None
    data = registry.read_owner_file(_status_path(engine, environ))
    if data is None:
        return None
    try:
        phase = json.loads(data).get("phase")
    except (ValueError, AttributeError):
        return "setup_failed"
    if phase not in _PHASES:
        return "setup_failed"
    if phase in _BUSY:
        try:
            fd = os.open(registry.config_root(environ) / _LOCK, os.O_RDONLY | os.O_NOFOLLOW)
        except FileNotFoundError:
            return "interrupted"
        try:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return phase
            return "interrupted"
        finally:
            os.close(fd)
    return phase


def read_status_reason(engine, environ=None):
    """The sanitized reason of a recorded model-discovery failure, else ``None``."""
    if engine not in discovery.SUPPORTED_ENGINES:
        return None
    data = registry.read_owner_file(_status_path(engine, environ))
    try:
        body = json.loads(data) if data is not None else None
    except ValueError:
        return None
    if not isinstance(body, dict) or body.get("phase") != "catalog_failed":
        return None
    return _safe_reason(body.get("reason"))


def refresh_connection(record, *, environ=None):
    """Observe models immediately after registration, never send a model prompt.

    Failure never rolls back a valid connection or replaces an older catalog.
    Return a fixed diagnostic, never adapter errors or credential bytes; the
    status file also keeps the outcome's fixed failure code.
    """
    from ..catalog.refresh import perform_refresh

    reason = None
    try:
        with tempfile.TemporaryDirectory(prefix="cortex-first-run-models-") as workspace:
            outcome = perform_refresh(record, workspace, environ=environ, timeout=20)
        phase = "ready" if outcome.success else "catalog_failed"
        reason = getattr(outcome, "reason", None)
    except Exception as error:  # noqa: BLE001 - the connection remains usable for a later retry
        phase = "catalog_failed"
        reason = f"refresh_error:{type(error).__name__}"
    record_status(record.engine, phase, environ, reason=reason)
    return phase


def record_status(engine, phase, environ=None, *, reason=None):
    """Best-effort diagnostics never turn a committed manual action into failure."""
    if engine in discovery.SUPPORTED_ENGINES:
        try:
            _status(engine, phase, environ, reason)
        except Exception:  # noqa: BLE001 - diagnostics are not authoritative state
            pass


def run(*, environ=None):
    """Contain first-run failures even when running as an ACP daemon thread."""
    try:
        _run(environ=environ)
    except Exception:  # noqa: BLE001 - never print raw setup errors to protocol/logs
        pass


def _run(*, environ=None):
    """One background pass; concurrent starts do not duplicate installs/discovery.

    npm is bounded by the existing 600-second installer, catalog RPCs by 20
    seconds each. Offline failures are isolated per engine. Retry next startup;
    no periodic downloader or model call is scheduled.
    """
    env = dict(os.environ if environ is None else environ)
    if env.get(engine_update.UPDATE_SWITCH, "").strip().lower() in {"off", "0", "false", "no"}:
        return
    root = registry.config_root(env)
    registry.ensure_private_dir(root, root=root, environ=env)
    fd = os.open(root / _LOCK, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1 or info.st_mode & 0o077:
            return
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return
        from ..routing import defaults
        try:
            defaults.apply(environ=env)
        except Exception:  # noqa: BLE001 - a malformed owner preset cannot prevent connection setup
            pass
        for engine in discovery.SUPPORTED_ENGINES:
            try:
                _setup_engine(engine, env)
            except Exception:  # noqa: BLE001 - don't expose npm/auth errors or block the other engine
                _status(engine, "setup_failed", env)
    finally:
        os.close(fd)


def _setup_engine(engine, env):
    from ..catalog.store import latest_connection_snapshot

    _status(engine, "downloading", env)
    try:
        engine_update.update_engine(engine, environ=env)
    except Exception:  # noqa: BLE001 - an offline update must not hide a usable owned copy
        pass
    found = discovery.discover_engine(engine, environ=env)
    if found is None or not discovery.is_qualified(engine, found.version):
        _status(engine, "download_failed", env)
        return
    state = registry.read_registry(env)
    existing = next((record for record in state.connections.values() if record.engine == engine), None)
    if existing is None and engine in state.auto_connect_disabled:
        _status(engine, "opted_out", env)
        return
    _status(engine, "connecting", env)
    created = onboarding.sync_discovered_connections({engine: found}, environ=env)
    if created:
        record = created[0]
    elif existing is not None:
        record = existing
    else:
        # Includes an owner removal racing this pass: registry is authoritative.
        state = registry.read_registry(env)
        _status(engine, "opted_out" if engine in state.auto_connect_disabled else "login_required", env)
        return
    if not created and latest_connection_snapshot(record.id, env) is not None:
        _status(engine, "ready", env)
        return
    _status(engine, "refreshing_models", env)
    phase = refresh_connection(record, environ=env)
    # Rewrite so an unwritable status still fails this engine's pass, keeping
    # the failure code refresh_connection recorded.
    _status(engine, phase, env, read_status_reason(engine, env))
