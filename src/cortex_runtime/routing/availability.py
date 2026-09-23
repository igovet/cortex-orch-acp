"""Routes that just failed for a reason about the route itself -- an
exhausted or rate-limited account, a model the account cannot run, an
engine that would not start or confirm a worker's configuration -- are
recorded here, so automatic routing (``routing.auto_route``) stops choosing
them until they expire instead of retrying the same route in a loop.

Each record is one ``ROUTE_UNAVAILABLE`` runtime event of the task whose
attempt failed; every task of the project sees it (an exhausted account is
exhausted for all of them). A record names the engine, connection and --
for a model-scoped failure -- the model, the reason, the structured
evidence that decided it and when it expires: the engine's own reset time
when it reported one, else a bounded default. Never a credential, a prompt
or engine prose.

Only runtime code records; a model never does. Expiry is the only way a
record stops counting -- a route is tried again after it, not before.
"""
from __future__ import annotations

import json
import time
from datetime import datetime, timezone
from typing import Iterable, Mapping

from ..engine.failure_classification import MODEL_UNAVAILABLE, QUOTA_EXHAUSTED
from ..engine.failure_classification import ROUTE_RATE_LIMITED as RATE_LIMITED

EVENT = "ROUTE_UNAVAILABLE"

CONFIGURATION_NOT_CONFIRMED = "configuration_not_confirmed"
ENGINE_START_FAILED = "engine_start_failed"

# How long a failure keeps a route out when the engine gave no reset time.
DEFAULT_SECONDS = {
    QUOTA_EXHAUSTED: 3600,
    RATE_LIMITED: 900,
    MODEL_UNAVAILABLE: 3600,
    CONFIGURATION_NOT_CONFIRMED: 1800,
    ENGINE_START_FAILED: 900,
}
# An engine-reported reset is honoured up to this far ahead (a weekly
# window plus a day); anything later is treated as this bound.
MAX_SECONDS = 8 * 24 * 3600
_READ_LIMIT = 500


def record(store, task_id: str, *, engine: str, connection_id: str, model: str | None, reason: str,
           scope: str, evidence: str, attempt_id: str | None = None, resets_at: float | None = None,
           now: float | None = None) -> dict | None:
    """Record one unavailable route; returns the payload, or ``None`` when
    ``task_id`` has no runtime task to hold it (nothing is recorded)."""
    from ..orchestration.state import event

    if scope not in ("connection", "model"):
        raise ValueError("scope must be 'connection' or 'model'")
    now = time.time() if now is None else now
    default = now + DEFAULT_SECONDS.get(reason, 900)
    until = resets_at if isinstance(resets_at, (int, float)) and resets_at > now else default
    until = min(until, now + MAX_SECONDS)
    payload = dict(
        engine=engine, connection_id=connection_id, model=model if scope == "model" else None,
        scope=scope, reason=reason, evidence=evidence[:200], attempt_id=attempt_id,
        expires_at=_iso(until), expiry_basis="engine_reported" if until != default else "default",
    )
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            if db.execute("SELECT 1 FROM runtime_tasks WHERE task_id=?", (task_id,)).fetchone() is None:
                db.rollback()
                return None
            event(db, task_id, EVENT, payload)
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return payload


def active(store, now: float | None = None) -> list[dict]:
    """Every unexpired record in this project store, newest first."""
    now = time.time() if now is None else now
    with store.connection() as db:
        rows = db.execute(
            "SELECT payload_json FROM runtime_events WHERE type=? ORDER BY id DESC LIMIT ?", (EVENT, _READ_LIMIT),
        ).fetchall()
    entries = []
    for row in rows:
        try:
            entry = json.loads(row[0])
            expires = datetime.fromisoformat(entry["expires_at"]).timestamp()
        except (KeyError, TypeError, ValueError):
            continue
        if expires > now:
            entries.append(entry)
    return entries


def blocking(entries: Iterable[Mapping], connection_id: str, model: str | None) -> Mapping | None:
    """The record that keeps ``model`` on ``connection_id`` out, if any."""
    for entry in entries:
        if entry.get("connection_id") != connection_id:
            continue
        if entry.get("scope") == "connection" or (model is not None and entry.get("model") == model):
            return entry
    return None


def for_attempt(db, task_id: str, attempt_id: str) -> Mapping | None:
    """The record this exact attempt's failure wrote, if any (``db`` is an
    open store connection, so a caller's transaction can read it)."""
    row = db.execute(
        "SELECT payload_json FROM runtime_events WHERE task_id=? AND type=? "
        "AND json_extract(payload_json,'$.attempt_id')=? ORDER BY id DESC LIMIT 1",
        (task_id, EVENT, attempt_id),
    ).fetchone()
    return json.loads(row[0]) if row else None


def describe(entry: Mapping) -> str:
    """One readable line: which route, why, until when."""
    target = f"{entry.get('engine')}/{entry.get('model')}" if entry.get("model") else f"{entry.get('engine')} (all models)"
    return f"{target} on {entry.get('connection_id')}: {entry.get('reason')} until {entry.get('expires_at')}"


def _iso(epoch: float) -> str:
    return datetime.fromtimestamp(epoch, timezone.utc).isoformat(timespec="seconds")
