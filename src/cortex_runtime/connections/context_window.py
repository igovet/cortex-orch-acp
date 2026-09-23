"""How large a model's context window is, as the engine itself reports it --
never a number Cortex invents per model.

Both qualified adapters send the window on every ACP ``usage_update`` as
``size`` (``codex-acp``: Codex's ``modelContextWindow``;
``claude-agent-acp``: the SDK's ``contextWindow``). A size seen once for a
connection's route model is kept under
``<state_root>/context_windows/<connection_id>.json`` so a fresh engine
session -- which has not reported anything yet -- can still be budgeted
against a real, previously observed window instead of a default.

``None`` means the engine never said -- unknown, not small.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Mapping

from . import registry


def observe(connection_id: str, model: str | None, size: object,
            environ: Mapping[str, str] | None = None) -> None:
    """Record one reported window for ``(connection_id, model)``. Never raises."""
    try:
        if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
            return
        windows = _load(connection_id, environ)
        key = model or ""
        if windows.get(key) == size:
            return
        windows[key] = size
        path = _path(connection_id, environ)
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        temporary = path.with_suffix(".tmp")
        temporary.write_text(json.dumps(windows, sort_keys=True), encoding="utf-8")
        os.replace(temporary, path)
    except Exception:  # noqa: BLE001 - advisory, never fails a turn
        return


def observed(connection_id: str, model: str | None,
             environ: Mapping[str, str] | None = None) -> int | None:
    """The last window the engine reported for exactly this route model."""
    size = _load(connection_id, environ).get(model or "")
    if isinstance(size, int) and not isinstance(size, bool) and size > 0:
        return size
    return None


def _path(connection_id: str, environ) -> Path:
    return registry.state_root(dict(environ) if environ is not None else None) / "context_windows" / f"{connection_id}.json"


def _load(connection_id: str, environ) -> dict:
    try:
        data = json.loads(_path(connection_id, environ).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}
