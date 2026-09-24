"""The default user-global ``routing.json`` and the legacy
``route-policy.json`` cleanup, applied by explicit setup
(``scripts/cortex_acp_setup.py`` ``init-storage``/``apply-routing-defaults``
and the Paseo plugin install), never silently at session start.

- :func:`ensure_default_routing` writes ``$XDG_CONFIG_HOME/cortex/routing.json``
  as exactly :data:`DEFAULT_ROUTING_TEXT` when it is absent. An existing file keeps
  every setting; only default ``deny`` tokens it lacks are appended (Haiku is
  dropped from routing entirely, see ``routing.auto_route``). A file that is
  not valid ``routing.json`` is reported and left untouched.
- :func:`clean_route_policy` removes, from a leftover user-global
  ``route-policy.json`` (the per-role policy ``routing.auto_route`` replaced;
  the runtime no longer reads it), every route whose model the global
  ``deny`` names. Other content is preserved; a role left with no route is
  removed so it falls back to ``default``. A private backup is kept before
  any rewrite.

A project-local ``<project>/.cortex/routing.json`` or ``$CORTEX_ROUTING``
still replaces the global file wholesale (never merged); neither is touched.
"""
from __future__ import annotations

import json
import os
import tempfile
import time
from pathlib import Path
from typing import Mapping

from . import auto_route

DEFAULT_DENY = ("gpt-5.5", "haiku")
# The exact bytes of a freshly written default file.
DEFAULT_ROUTING_TEXT = '{\n  "deny": ["gpt-5.5", "haiku"]\n}\n'


def _config_root(environ: Mapping[str, str] | None) -> Path:
    from ..connections import registry

    return registry.config_root(None if environ is None else dict(environ))


def _write_private(path: Path, body: str) -> None:
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=str(path.parent))
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            fd = None
            stream.write(body)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        temporary = None
    finally:
        if fd is not None:
            os.close(fd)
        if temporary is not None:
            try:
                os.unlink(temporary)
            except FileNotFoundError:
                pass


def _regular_file(path: Path) -> bool:
    if path.is_symlink():
        raise ValueError(f"refusing symlinked configuration file {path}")
    return path.is_file()


def ensure_default_routing(environ: Mapping[str, str] | None = None) -> dict:
    """Create or complete the user-global ``routing.json``; idempotent."""
    path = _config_root(environ) / "routing.json"
    if not path.parent.is_dir():
        return dict(path=str(path), status="config_root_missing", changed=False)
    try:
        if not _regular_file(path):
            _write_private(path, DEFAULT_ROUTING_TEXT)
            return dict(path=str(path), status="created", changed=True, deny=list(DEFAULT_DENY))
        data = json.loads(path.read_text(encoding="utf-8"))
        auto_route._validate(data)
    except (OSError, ValueError) as error:
        return dict(path=str(path), status="invalid", changed=False, error=str(error))
    deny = list(data.get("deny") or [])
    added = [token for token in DEFAULT_DENY if token not in deny]
    if not added:
        return dict(path=str(path), status="current", changed=False, deny=deny)
    data = dict(data, deny=deny + added)
    try:
        _write_private(path, json.dumps(data, indent=2) + "\n")
    except OSError as error:
        return dict(path=str(path), status="write_failed", changed=False, error=str(error))
    return dict(path=str(path), status="updated", changed=True, deny=data["deny"], added=added)


def clean_route_policy(environ: Mapping[str, str] | None = None) -> dict:
    """Drop globally denied models from a leftover ``route-policy.json``."""
    root = _config_root(environ)
    path = root / "route-policy.json"
    try:
        if not _regular_file(path):
            return dict(path=str(path), status="absent", changed=False)
        routing = root / "routing.json"
        deny = []
        if _regular_file(routing):
            loaded = json.loads(routing.read_text(encoding="utf-8"))
            auto_route._validate(loaded)
            deny = list(loaded.get("deny") or [])
        data = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(data, dict):
            raise ValueError("must contain a JSON object")
    except (OSError, ValueError) as error:
        return dict(path=str(path), status="invalid", changed=False, error=str(error))
    removed = []

    def keep(route) -> bool:
        if not isinstance(route, dict):
            return True
        engine, model = route.get("engine"), route.get("model")
        if not isinstance(engine, str) or not isinstance(model, str) or model == "*":
            return True
        if auto_route._listed(deny, engine, model):
            removed.append(f"{engine}/{model}")
            return False
        return True

    cleaned = dict(data)
    roles = data.get("roles")
    emptied = []
    if isinstance(roles, dict):
        cleaned_roles = {}
        for role, routes in roles.items():
            if not isinstance(routes, list):
                cleaned_roles[role] = routes
                continue
            kept = [route for route in routes if keep(route)]
            if kept or not routes:
                cleaned_roles[role] = kept
            else:
                emptied.append(role)
        cleaned["roles"] = cleaned_roles
    if isinstance(data.get("default"), list):
        cleaned["default"] = [route for route in data["default"] if keep(route)]
    if not removed:
        return dict(path=str(path), status="current", changed=False, removed=[])
    backup = path.with_name(f"{path.name}.cortex-backup-{time.time_ns()}")
    try:
        _write_private(backup, path.read_text(encoding="utf-8"))
        _write_private(path, json.dumps(cleaned, indent=2, ensure_ascii=False) + "\n")
    except OSError as error:
        return dict(path=str(path), status="write_failed", changed=False, error=str(error))
    return dict(path=str(path), status="cleaned", changed=True, removed=sorted(set(removed)),
                removed_roles=sorted(emptied), backup=str(backup))


def apply(environ: Mapping[str, str] | None = None) -> dict:
    """Both steps, in order (the cleanup reads the completed ``deny``)."""
    return dict(routing=ensure_default_routing(environ), route_policy=clean_route_policy(environ))
