"""How much of a connection's subscription plan is already used, as the
engine itself reports it -- never estimated from Cortex's own token counts.

- ``codex-acp``: the Codex CLI writes its account-wide ``rate_limits``
  (``primary``/``secondary`` windows, ``used_percent``, ``resets_at``) into
  every session rollout under ``<CODEX_HOME>/sessions/YYYY/MM/DD/*.jsonl``.
  codex-acp does not forward them over ACP, so the newest rollout in the
  connection's own native directory is read directly.
- ``claude-agent-acp``: two independent sources are merged, newest
  ``observed_at`` per window wins:

  1. The adapter forwards the SDK's ``rate_limit_event`` as
     ``usage_update._meta["_claude/rateLimit"]`` (one window per event:
     ``five_hour``, ``seven_day``, or a provider-reported scoped label).
     Nothing on disk
     holds it, so ``engine.session_manager`` hands each one to
     :func:`observe`. This rarely carries ``utilization``, so on its own it
     leaves Claude usage unknown almost always.
  2. This module also lazily runs the installed Claude Code CLI's local
     ``/usage`` command (``claude -p /usage --output-format json
     --restricted --strict-mcp-config``) for the connection's own
     ``CLAUDE_CONFIG_DIR`` and parses its plain-text ``result``. The
     subprocess environment comes from
     ``connections.environment.build_process_env`` -- the same call this
     connection's ``account_identity._claude_account_identity`` already
     uses for its own CLI spawn -- which re-verifies the connection
     against the on-disk registry before trusting its ``credential_ref``,
     rather than reading ``record.credential_ref.path`` directly. This is a
     documented local (``type: "local"``, ``supportsNonInteractive: true``)
     command with no model call: it is only ever trusted when the parsed
     JSON has ``is_error: false``, ``num_turns: 0`` and
     ``total_cost_usd: 0`` -- any other shape is discarded and the window
     stays whatever it already was (never treated as a paid call, never
     synthesized). Refreshed at most once per connection per
     ``_CLAUDE_USAGE_CLI_TTL_SECONDS`` (a failure is cached negatively for
     the same window), serialized by a per-connection file lock so
     concurrent callers never spawn it twice at once. A CLI read never
     replaces a *newer* ``rate_limit_event`` observation for the same
     window, and vice versa.

Both sources are kept under ``<state_root>/quota/<connection_id>.json``.

A window whose ``resets_at`` has passed no longer counts. ``None`` means
the engine never said -- unknown, not zero. Applicability comes only from
exact ``quota_windows`` settings; unmapped labels conservatively apply to
all models, without family inference. When the CLI is unavailable or
fails, a window is simply not refreshed by it; it still expires under the
same ``resets_at`` rule as any other source, never falls back to a stale
value beyond that.
"""
from __future__ import annotations

import fcntl
import json
import os
import re
import shutil
import subprocess
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Mapping

from . import environment, registry
from ..routing.model_policy import _model_id
from .model import SUPPORTED_ENGINES

_ROLLOUT_TAIL_BYTES = 256 * 1024
_ROLLOUT_FILES_SCANNED = 10
# Paid extra usage is not part of the plan and must not look like headroom.
_IGNORED_WINDOWS = ("overage",)

# The Claude Code CLI's own local `/usage` command: refreshed lazily, at
# most this often per connection (successes and failures alike).
_CLAUDE_USAGE_CLI_TTL_SECONDS = 300.0
_CLAUDE_USAGE_CLI_TIMEOUT_SECONDS = 20.0

_SESSION_LINE = re.compile(
    r"^Current session:\s*(?P<used>\d+(?:\.\d+)?)%\s*used\s*[·-]\s*resets\s+(?P<date>.+?)\s*\((?P<tz>[^)]*)\)\s*$"
)
_WEEK_LINE = re.compile(
    r"^Current week \((?P<scope>[^)]+)\):\s*(?P<used>\d+(?:\.\d+)?)%\s*used\s*[·-]\s*resets\s+(?P<date>.+?)\s*\((?P<tz>[^)]*)\)\s*$"
)
_RESET_DATE_FORMATS = ("%b %d, %I:%M%p", "%b %d, %I%p")


def used_percent(record, model: str | None, environ: Mapping[str, str] | None = None,
                 now: float | None = None, *, routing: Mapping | None = None) -> float | None:
    """The highest still-open window's used percentage (0-100) that applies
    to ``model`` on this connection, or ``None`` when nothing is known."""
    now = time.time() if now is None else now
    rules = _quota_associations(routing, record.engine)
    windows = _windows_for(record, environ, now)
    live = [
        float(window["used"]) for name, window in windows.items()
        if _applies(name, model, rules) and isinstance(window.get("used"), (int, float))
        and not (isinstance(window.get("resets_at"), (int, float)) and window["resets_at"] <= now)
    ]
    return max(live) if live else None


def resets_at(record, model: str | None, environ: Mapping[str, str] | None = None,
              now: float | None = None, *, routing: Mapping | None = None) -> float | None:
    """When the most-used still-open window that applies to ``model``
    resets (epoch seconds), as the engine reported it; ``None`` when the
    engine never said."""
    now = time.time() if now is None else now
    rules = _quota_associations(routing, record.engine)
    windows = _windows_for(record, environ, now)
    live = [
        (float(window["used"]), float(window["resets_at"])) for name, window in windows.items()
        if _applies(name, model, rules) and isinstance(window.get("used"), (int, float))
        and isinstance(window.get("resets_at"), (int, float)) and window["resets_at"] > now
    ]
    return max(live)[1] if live else None


def visible_windows(record, environ: Mapping[str, str] | None = None,
                    now: float | None = None) -> list[dict]:
    """Read engine-reported plan windows for display, without estimating spend.

    An expired window is unknown until the engine reports its replacement.
    Cache reads, token totals and context occupancy are not subscription use.
    """
    now = time.time() if now is None else now
    if record.engine == "codex-acp":
        source = _codex_windows(record)
    elif record.engine == "claude-agent-acp":
        source = _windows_for(record, environ, now)
    else:
        return []
    result = []
    for name, window in source.items():
        if not isinstance(name, str) or not isinstance(window, Mapping) or "overage" in name.casefold():
            continue
        used, reset = window.get("used"), window.get("resets_at")
        if isinstance(used, bool) or not isinstance(used, (int, float)) or not 0 <= used <= 100:
            continue
        if isinstance(reset, bool) or not isinstance(reset, (int, float)):
            reset = None
        elif reset <= now:
            continue
        result.append({"name": name, "usedPercent": float(used), "resetsAt": reset})
    return sorted(result, key=lambda item: item["name"])


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
        windows[str(name)] = dict(used=used, resets_at=info.get("resetsAt"), observed_at=time.time(), source="event")
        _write_windows(connection_id, environ, windows)
    except Exception:  # noqa: BLE001 - quota is advisory, never fails a turn
        return


def _windows_for(record, environ: Mapping[str, str] | None, now: float) -> dict:
    """``codex-acp``'s own rollout windows, or ``claude-agent-acp``'s cached
    windows after a lazy CLI refresh."""
    if record.engine == "codex-acp":
        return _codex_windows(record)
    if record.engine == "claude-agent-acp":
        _maybe_refresh_claude_usage(record, environ, now)
    return _load(getattr(record, "id", None) or "", environ)


class QuotaValidationError(ValueError):
    def __init__(self, field, message):
        self.field = field
        super().__init__(message)


def validate_quota_windows(value) -> None:
    if not isinstance(value, dict):
        raise QuotaValidationError("quota_windows", "must map engines to window rule lists")
    for engine, rules in value.items():
        field = f"quota_windows.{engine}"
        if engine not in SUPPORTED_ENGINES or not isinstance(rules, list) or len(rules) > 1000:
            raise QuotaValidationError(field, "needs a supported engine and at most 1000 rules")
        seen = set()
        for index, rule in enumerate(rules):
            field = f"quota_windows.{engine}[{index}]"
            if not isinstance(rule, dict) or set(rule) != {"window", "models"}:
                raise QuotaValidationError(field, "needs exactly window and models")
            window, models = rule["window"], rule["models"]
            if (not isinstance(window, str) or not window or len(window) > 256
                    or any(ord(c) < 32 or ord(c) == 127 for c in window)):
                raise QuotaValidationError(field, "window must be nonempty text without controls, at most 256 characters")
            if window in seen:
                raise QuotaValidationError(field, "ambiguous quota window: each exact label may appear only once per engine")
            seen.add(window)
            if models is not None and (not isinstance(models, list) or len(models) > 1000
                    or not all(_model_id(m) and (engine != "opencode" or
                               ("/" in m and not m.startswith("/") and not m.endswith("/")))
                               for m in models) or len(set(models)) != len(models)):
                raise QuotaValidationError(field, "models must be null (shared) or unique exact model ids (OpenCode: provider/model)")


def _quota_associations(routing, engine) -> dict:
    value = {} if routing is None else routing.get("quota_windows", {})
    validate_quota_windows(value)  # Before any provider/cache IO, including direct accessors.
    return {rule["window"]: rule["models"] for rule in value.get(engine, [])}


def _quota_applies(window, model, rules) -> bool:
    models = rules.get(window)
    return models is None or model in models


def _applies(window: str, model: str | None, rules: Mapping) -> bool:
    if any(marker in window.casefold() for marker in _IGNORED_WINDOWS):
        return False
    return _quota_applies(window, model, rules)


def _quota_dir(environ) -> Path:
    return registry.state_root(dict(environ) if environ is not None else None) / "quota"


def _quota_path(connection_id: str, environ) -> Path:
    return _quota_dir(environ) / f"{connection_id}.json"


def _cli_probe_path(connection_id: str, environ) -> Path:
    return _quota_dir(environ) / f"{connection_id}.cli-probe.json"


def _cli_lock_path(connection_id: str, environ) -> Path:
    return _quota_dir(environ) / f"{connection_id}.cli-probe.lock"


def _read_json_dict(path: Path) -> dict:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def _write_json_atomic(path: Path, payload: dict) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(payload), encoding="utf-8")
    os.replace(temporary, path)


def _load(connection_id: str, environ) -> dict:
    return _read_json_dict(_quota_path(connection_id, environ))


def _write_windows(connection_id: str, environ, windows: dict) -> None:
    _write_json_atomic(_quota_path(connection_id, environ), windows)


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


# --- Claude Code CLI `/usage` refresh -------------------------------------


def _maybe_refresh_claude_usage(record, environ: Mapping[str, str] | None, now: float) -> None:
    """Lazily refresh this connection's cached windows from the local Claude
    Code CLI ``/usage`` command, at most once per TTL. Never raises."""
    try:
        connection_id = getattr(record, "id", None)
        if not isinstance(connection_id, str) or not connection_id:
            return
        lock_path = _cli_lock_path(connection_id, environ)
        lock_path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        with open(lock_path, "a+") as lock_file:
            fcntl.flock(lock_file.fileno(), fcntl.LOCK_EX)
            try:
                probe_path = _cli_probe_path(connection_id, environ)
                probe = _read_json_dict(probe_path)
                checked_at = probe.get("checked_at")
                if isinstance(checked_at, (int, float)) and 0 <= now - checked_at < _CLAUDE_USAGE_CLI_TTL_SECONDS:
                    return
                _refresh_claude_usage_locked(record, environ, now, probe_path)
            finally:
                fcntl.flock(lock_file.fileno(), fcntl.LOCK_UN)
    except Exception:  # noqa: BLE001 - quota is advisory, never fails a turn
        return


def _refresh_claude_usage_locked(record, environ: Mapping[str, str] | None,
                                  now: float, probe_path: Path) -> None:
    ok = False
    try:
        parsed = _read_claude_usage_cli(record, environ, now)
        if parsed is not None:
            _merge_claude_windows(record.id, environ, parsed, now)
            ok = True
    except Exception:  # noqa: BLE001 - a CLI failure only leaves usage unknown
        ok = False
    try:
        _write_json_atomic(probe_path, {"checked_at": now, "ok": ok})
    except OSError:
        pass


def _merge_claude_windows(connection_id: str, environ: Mapping[str, str] | None, parsed: dict, now: float) -> None:
    windows = _load(connection_id, environ)
    for name, window in parsed.items():
        existing = windows.get(name)
        existing_observed = existing.get("observed_at") if isinstance(existing, Mapping) else None
        if isinstance(existing_observed, (int, float)) and existing_observed > now:
            continue  # a newer rate_limit_event (or a racing CLI read) already won
        windows[name] = dict(used=window["used"], resets_at=window.get("resets_at"), observed_at=now, source="cli")
    _write_windows(connection_id, environ, windows)


def _read_claude_usage_cli(record, environ: Mapping[str, str] | None, now: float) -> dict | None:
    """Run and parse ``claude -p /usage``, or ``None`` on any guard failure."""
    base = dict(os.environ) if environ is None else dict(environ)
    path_value = base.get("PATH", "")
    binary = shutil.which("claude", path=path_value) if path_value else shutil.which("claude")
    if not binary:
        return None
    try:
        # Re-verifies this connection against the on-disk registry before
        # trusting its credential_ref -- the same guarantee
        # account_identity._claude_account_identity relies on for its own
        # CLI spawn -- rather than reading record.credential_ref.path
        # directly. Any rejection (stale/non-canonical/unknown connection)
        # is treated as a CLI failure below: the window stays unknown.
        process_env = environment.build_process_env(record, {"PATH": path_value}, environ=environ)
    except Exception:
        return None
    config_dir = process_env.get("CLAUDE_CONFIG_DIR")
    if not isinstance(config_dir, str) or not Path(config_dir).is_dir():
        # No point spawning the CLI against a directory that isn't there
        # yet (e.g. a connection that has never actually logged in) -- it
        # would only ever fail, so this is treated the same as any other
        # guard rejection: unknown, without a spawn.
        return None
    # TZ=UTC makes the printed reset times parseable without a per-user
    # timezone table.
    process_env["TZ"] = "UTC"
    argv = [binary, "-p", "/usage", "--output-format", "json", "--restricted", "--strict-mcp-config"]
    with tempfile.TemporaryDirectory() as cwd:
        try:
            completed = _run_claude_usage_subprocess(argv, process_env, cwd, _CLAUDE_USAGE_CLI_TIMEOUT_SECONDS)
        except Exception:
            return None
    return _parse_claude_usage_result(completed, now)


def _run_claude_usage_subprocess(argv: list[str], env: dict[str, str], cwd: str, timeout: float):
    """The real subprocess call. Monkeypatched by tests so the default
    (no-paid) test suite never spawns a real ``claude`` process."""
    return subprocess.run(
        argv, env=env, cwd=cwd, stdin=subprocess.DEVNULL, capture_output=True, text=True,
        timeout=timeout, check=False,
    )


def _parse_claude_usage_result(completed, now: float) -> dict | None:
    """Only ever trust a zero-cost, zero-turn, non-error local command
    result; anything else -- including a real model turn somehow having
    run -- is discarded rather than risking a paid call being silently
    treated as free."""
    stdout = getattr(completed, "stdout", None)
    if not isinstance(stdout, str):
        return None
    try:
        payload = json.loads(stdout)
    except ValueError:
        return None
    if not isinstance(payload, Mapping):
        return None
    if payload.get("is_error") is not False:
        return None
    if payload.get("num_turns") != 0:
        return None
    cost = payload.get("total_cost_usd")
    if not isinstance(cost, (int, float)) or isinstance(cost, bool) or cost != 0:
        return None
    result_text = payload.get("result")
    if not isinstance(result_text, str):
        return {}
    return _parse_usage_text(result_text, now)


def _parse_usage_text(text: str, now: float) -> dict:
    windows: dict = {}
    for raw_line in text.splitlines():
        line = raw_line.strip()
        match = _SESSION_LINE.match(line)
        if match:
            window = _build_window(match.group("used"), match.group("date"), now)
            if window is not None:
                windows["five_hour"] = window
            continue
        match = _WEEK_LINE.match(line)
        if match:
            scope = match.group("scope").strip().casefold()
            name = "seven_day" if scope == "all models" else "seven_day_" + re.sub(r"\s+", "_", scope)
            window = _build_window(match.group("used"), match.group("date"), now)
            if window is not None:
                windows[name] = window
    return windows


def _build_window(used_text: str, date_text: str, now: float) -> dict | None:
    try:
        used = float(used_text)
    except ValueError:
        return None
    if not 0 <= used <= 100:
        return None
    return {"used": used, "resets_at": _parse_reset_time(date_text, now)}


def _parse_reset_time(date_text: str, now: float) -> float | None:
    """``date_text`` is UTC wall-clock text (the CLI ran with TZ=UTC): an
    unparseable value leaves the window's reset unknown, never 0."""
    date_text = date_text.strip()
    reference = datetime.fromtimestamp(now, tz=timezone.utc)
    for fmt in _RESET_DATE_FORMATS:
        try:
            parsed = datetime.strptime(date_text, fmt)
        except ValueError:
            continue
        candidate = parsed.replace(year=reference.year, tzinfo=timezone.utc)
        if candidate < reference:
            candidate = candidate.replace(year=reference.year + 1)
        return candidate.timestamp()
    return None
