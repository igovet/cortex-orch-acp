"""How much of a connection's subscription plan is already used, as the
engine itself reports it -- never estimated from Cortex's own token counts.

- ``codex-acp``: the Codex CLI writes its account-wide ``rate_limits``
  (``primary``/``secondary`` windows, ``used_percent``, ``resets_at``) into
  every session rollout under ``<CODEX_HOME>/sessions/YYYY/MM/DD/*.jsonl``.
  codex-acp does not forward them over ACP, so the newest rollout in the
  connection's own native directory is read directly.
- ``claude-agent-acp``: the adapter forwards the SDK's ``rate_limit_event``
  as ``usage_update._meta["_claude/rateLimit"]`` (one window per event:
  ``five_hour``, ``seven_day``, ``seven_day_opus``, ...). Nothing on disk
  holds it, so ``engine.session_manager`` hands each one to :func:`observe`
  and the latest value per window is kept under
  ``<state_root>/quota/<connection_id>.json``.

A window whose ``resets_at`` has passed no longer counts. ``None`` means
the engine never said -- unknown, not zero.
"""
from __future__ import annotations

import json
import os
import time
from pathlib import Path
from typing import Mapping

from . import registry

_ROLLOUT_TAIL_BYTES = 256 * 1024
_ROLLOUT_FILES_SCANNED = 10
# Paid extra usage is not part of the plan and must not look like headroom.
_IGNORED_WINDOWS = ("overage",)
_MODEL_SCOPED = ("opus", "sonnet", "haiku", "fable")


def used_percent(record, model: str | None, environ: Mapping[str, str] | None = None,
                 now: float | None = None) -> float | None:
    """The highest still-open window's used percentage (0-100) that applies
    to ``model`` on this connection, or ``None`` when nothing is known."""
    now = time.time() if now is None else now
    windows = _codex_windows(record) if record.engine == "codex-acp" else _load(record.id, environ)
    model = (model or "").casefold()
    live = [
        float(window["used"]) for name, window in windows.items()
        if _applies(name, model) and isinstance(window.get("used"), (int, float))
        and not (isinstance(window.get("resets_at"), (int, float)) and window["resets_at"] <= now)
    ]
    return max(live) if live else None


def resets_at(record, model: str | None, environ: Mapping[str, str] | None = None,
              now: float | None = None) -> float | None:
    """When the most-used still-open window that applies to ``model``
    resets (epoch seconds), as the engine reported it; ``None`` when the
    engine never said."""
    now = time.time() if now is None else now
    windows = _codex_windows(record) if record.engine == "codex-acp" else _load(record.id, environ)
    model = (model or "").casefold()
    live = [
        (float(window["used"]), float(window["resets_at"])) for name, window in windows.items()
        if _applies(name, model) and isinstance(window.get("used"), (int, float))
        and isinstance(window.get("resets_at"), (int, float)) and window["resets_at"] > now
    ]
    return max(live)[1] if live else None


def observe(connection_id: str, info: object, environ: Mapping[str, str] | None = None) -> None:
    """Record one Claude ``rate_limit_info``. Never raises."""
    try:
        if not isinstance(info, Mapping):
            return
        name = info.get("rateLimitType") or "unscoped"
        utilization = info.get("utilization")
        if info.get("status") == "rejected":
            used = 100.0
        elif isinstance(utilization, (int, float)):
            # The SDK reports a fraction of the window (0..1).
            used = float(utilization) * 100.0
        else:
            return
        windows = _load(connection_id, environ)
        windows[str(name)] = dict(used=used, resets_at=info.get("resetsAt"))
        path = _quota_path(connection_id, environ)
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        temporary = path.with_suffix(".tmp")
        temporary.write_text(json.dumps(windows), encoding="utf-8")
        os.replace(temporary, path)
    except Exception:  # noqa: BLE001 - quota is advisory, never fails a turn
        return


def _applies(window: str, model: str) -> bool:
    window = window.casefold()
    if any(marker in window for marker in _IGNORED_WINDOWS):
        return False
    scoped = next((family for family in _MODEL_SCOPED if family in window), None)
    return scoped is None or scoped in model


def _quota_path(connection_id: str, environ) -> Path:
    return registry.state_root(dict(environ) if environ is not None else None) / "quota" / f"{connection_id}.json"


def _load(connection_id: str, environ) -> dict:
    try:
        data = json.loads(_quota_path(connection_id, environ).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def _codex_windows(record) -> dict:
    home = getattr(record.credential_ref, "path", None)
    if not home:
        return {}
    limits = _latest_rate_limits(Path(home) / "sessions")
    if not isinstance(limits, Mapping):
        return {}
    windows = {}
    for name in ("primary", "secondary"):
        window = limits.get(name)
        if isinstance(window, Mapping) and isinstance(window.get("used_percent"), (int, float)):
            windows[name] = dict(used=window["used_percent"], resets_at=window.get("resets_at"))
    return windows


def _latest_rate_limits(sessions: Path):
    """``rate_limits`` from the newest rollout that has any.
    ponytail: scans only the two newest day directories; an account idle
    longer than that reads as unknown, which is also what it is."""
    try:
        days = sorted((day for year in sessions.iterdir() for month in year.iterdir() for day in month.iterdir()),
                      reverse=True)[:2]
        files = sorted((f for day in days for f in day.glob("*.jsonl")), key=lambda f: f.stat().st_mtime,
                       reverse=True)[:_ROLLOUT_FILES_SCANNED]
    except OSError:
        return None
    for rollout in files:
        try:
            with rollout.open("rb") as handle:
                handle.seek(max(0, rollout.stat().st_size - _ROLLOUT_TAIL_BYTES))
                lines = handle.read().decode("utf-8", "replace").splitlines()
        except OSError:
            continue
        for line in reversed(lines):
            if '"rate_limits"' not in line:
                continue
            try:
                limits = (json.loads(line).get("payload") or {}).get("rate_limits")
            except (ValueError, AttributeError):
                continue
            if limits:
                return limits
    return None
