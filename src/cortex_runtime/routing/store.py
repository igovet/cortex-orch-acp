"""T027 additive optional schema group: three new tables, none of which
alter any existing table's shape (see the task's own hard rule: new tables
are additive optional groups; ``user_version`` stays 11).

- ``configuration_route_extras`` -- one row per ``configuration_revisions``
  row (1:1, same lifecycle, referenced by id), carrying the two fields T008
  never persisted in plain text: the route's actual ``model`` string (only
  embedded, unrecoverably, inside ``route_id``'s content fingerprint -- see
  ``portable.identity.ExecutionRoute``) and this revision's
  :class:`~configuration.policy.RouteProfile` (mixed-team role->route-id
  map, or the default single-route marker). Never mutated after insert
  (mirrors ``configuration_revisions``' own immutable identity columns);
  deleted only via the same retention path as its parent row (see
  ``cleanup.finish_deletions``, extended to include this table).
- ``attempt_route_audit`` -- one immutable row per *attempt* recording the
  actual engine/connection/model/parameters-fingerprint/config-revision it
  was dispatched with, plus a small JSON "profile decision" (why dispatch
  was authorized, or which policy/consent authorized a fallback) and --
  only for a fallback attempt -- the predecessor attempt id it replaces.
  This is the audit trail the task's own acceptance bullets #1/#2 require;
  never overwritten, one row per attempt id.
- ``fallback_consent_records`` -- one durable row per explicit recipient
  consent to transfer this task's data to a *different* provider for
  fallback purposes (referencing the target connection and provider, plus
  a free-text data scope). ``routing.authorization.has_active_consent``
  requires a live (non-revoked) row here before ever authorizing a
  cross-provider fallback; ``PolicyCeiling.fallback_policy
  .data_recipient_consent`` alone (a boolean) is necessary but not
  sufficient -- this table is the durable per-recipient record the task's
  lead decisions require instead of a bare flag.
"""
from __future__ import annotations

from datetime import datetime, timezone
import json

from ..configuration.policy import RouteProfile
from ..portable.identity import new_id

ROUTING_SCHEMA = '''
CREATE TABLE IF NOT EXISTS configuration_route_extras (
 revision_id TEXT PRIMARY KEY REFERENCES configuration_revisions(id),
 task_id TEXT NOT NULL REFERENCES tasks(id),
 route_model TEXT NOT NULL,
 route_profile_json TEXT NOT NULL,
 route_details_json TEXT NOT NULL DEFAULT '{}',
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS configuration_route_extras_by_task ON configuration_route_extras(task_id);

CREATE TABLE IF NOT EXISTS attempt_route_audit (
 id TEXT PRIMARY KEY,
 attempt_id TEXT NOT NULL,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 role TEXT NOT NULL,
 engine TEXT NOT NULL,
 connection_id TEXT NOT NULL,
 model TEXT NOT NULL,
 params_fingerprint TEXT NOT NULL,
 config_revision_id TEXT,
 decision TEXT NOT NULL CHECK(decision IN ('authorized','fallback')),
 decision_detail_json TEXT NOT NULL,
 predecessor_attempt_id TEXT,
 recorded_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS attempt_route_audit_by_task ON attempt_route_audit(task_id, recorded_at);
CREATE INDEX IF NOT EXISTS attempt_route_audit_by_attempt ON attempt_route_audit(attempt_id);
CREATE TRIGGER IF NOT EXISTS attempt_route_audit_no_update BEFORE UPDATE ON attempt_route_audit
BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE TRIGGER IF NOT EXISTS attempt_route_audit_no_delete BEFORE DELETE ON attempt_route_audit
WHEN NOT EXISTS (
 SELECT 1 FROM pending_deletions d WHERE d.task_id=OLD.task_id
) BEGIN SELECT RAISE(ABORT, 'immutable'); END;

CREATE TABLE IF NOT EXISTS fallback_consent_records (
 id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 target_connection_id TEXT NOT NULL,
 target_provider TEXT NOT NULL,
 data_scope TEXT NOT NULL,
 granted_by TEXT NOT NULL,
 granted_at TEXT NOT NULL,
 revoked_at TEXT
);
CREATE INDEX IF NOT EXISTS fallback_consent_by_task ON fallback_consent_records(task_id, target_connection_id);
'''

ROUTING_TABLES = frozenset({"configuration_route_extras", "attempt_route_audit", "fallback_consent_records"})


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def _encode_profile(profile: RouteProfile) -> str:
    if profile.role_routes is None:
        return json.dumps({"role_routes": None})
    return json.dumps(
        {"role_routes": {role: sorted(ids) for role, ids in profile.role_routes.items()}},
        sort_keys=True,
    )


def _decode_profile(raw: str) -> RouteProfile:
    data = json.loads(raw)
    role_routes = data.get("role_routes")
    if role_routes is None:
        return RouteProfile(role_routes=None)
    return RouteProfile(role_routes={role: frozenset(ids) for role, ids in role_routes.items()})


def insert_route_extras_tx(
    db, *, revision_id: str, task_id: str, route_model: str, route_profile: RouteProfile | None,
    route_details: dict[str, dict] | None = None,
) -> None:
    """Called from inside ``ConfigurationService.propose``'s own
    transaction, right after ``configuration_store.insert_revision_tx`` --
    same atomicity as the parent row, no separate commit.

    ``route_details`` (T027 fallback-trigger wiring) maps every route id a
    non-default ``route_profile`` names to ``{"engine":..., "connection_id":
    ..., "model":...}`` -- the one place any route id outside this
    revision's own ``inputs.route`` becomes concretely resolvable again
    later (a bare route id is an opaque content fingerprint; nothing can
    recover engine/connection/model from it alone). A candidate fallback
    target route absent from this map is never chosen -- fails closed,
    never guessed.
    """
    profile = route_profile if route_profile is not None else RouteProfile(role_routes=None)
    db.execute(
        "INSERT INTO configuration_route_extras "
        "(revision_id, task_id, route_model, route_profile_json, route_details_json, created_at) "
        "VALUES (?,?,?,?,?,?)",
        (revision_id, task_id, route_model, _encode_profile(profile), json.dumps(route_details or {}, sort_keys=True), _now()),
    )


def get_route_extras(store, revision_id: str) -> dict | None:
    with store.connection() as db:
        return get_route_extras_tx(db, revision_id)


def get_route_extras_tx(db, revision_id: str) -> dict | None:
    row = db.execute(
        "SELECT * FROM configuration_route_extras WHERE revision_id=?", (revision_id,)
    ).fetchone()
    if row is None:
        return None
    return dict(
        revision_id=row["revision_id"],
        task_id=row["task_id"],
        route_model=row["route_model"],
        route_profile=_decode_profile(row["route_profile_json"]),
        route_details=json.loads(row["route_details_json"]) if row["route_details_json"] else {},
        created_at=row["created_at"],
    )


def record_attempt_audit(
    store, *, attempt_id: str, task_id: str, role: str, engine: str, connection_id: str,
    model: str, params_fingerprint: str, config_revision_id: str | None,
    decision: str, detail: dict, predecessor_attempt_id: str | None = None,
) -> str:
    if decision not in ("authorized", "fallback"):
        raise ValueError(f"unknown attempt_route_audit decision {decision!r}")
    audit_id = new_id("ara_")
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "INSERT INTO attempt_route_audit "
                "(id, attempt_id, task_id, role, engine, connection_id, model, params_fingerprint, "
                " config_revision_id, decision, decision_detail_json, predecessor_attempt_id, recorded_at) "
                "VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
                (
                    audit_id, attempt_id, task_id, role, engine, connection_id, model, params_fingerprint,
                    config_revision_id, decision, json.dumps(detail, sort_keys=True), predecessor_attempt_id, _now(),
                ),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return audit_id


def list_audit_for_task(store, task_id: str) -> tuple[dict, ...]:
    with store.connection() as db:
        rows = db.execute(
            "SELECT * FROM attempt_route_audit WHERE task_id=? ORDER BY recorded_at ASC", (task_id,)
        ).fetchall()
    return tuple(_audit_row(row) for row in rows)


def list_audit_for_attempt(store, attempt_id: str) -> tuple[dict, ...]:
    with store.connection() as db:
        rows = db.execute(
            "SELECT * FROM attempt_route_audit WHERE attempt_id=? ORDER BY recorded_at ASC", (attempt_id,)
        ).fetchall()
    return tuple(_audit_row(row) for row in rows)


def _audit_row(row) -> dict:
    return dict(
        id=row["id"],
        attempt_id=row["attempt_id"],
        task_id=row["task_id"],
        role=row["role"],
        engine=row["engine"],
        connection_id=row["connection_id"],
        model=row["model"],
        params_fingerprint=row["params_fingerprint"],
        config_revision_id=row["config_revision_id"],
        decision=row["decision"],
        decision_detail=json.loads(row["decision_detail_json"]),
        predecessor_attempt_id=row["predecessor_attempt_id"],
        recorded_at=row["recorded_at"],
    )


def count_task_attempts(store, task_id: str) -> int:
    """The number of dispatch attempts (authorized or fallback) already
    audited for ``task_id`` -- used to enforce
    ``PolicyCeiling.limits['attempts']`` before a *new* attempt is
    authorized (never after it starts)."""
    with store.connection() as db:
        row = db.execute(
            "SELECT COUNT(*) AS n FROM attempt_route_audit WHERE task_id=?", (task_id,)
        ).fetchone()
    return row["n"]


def count_fallback_attempts(store, task_id: str) -> int:
    with store.connection() as db:
        row = db.execute(
            "SELECT COUNT(*) AS n FROM attempt_route_audit WHERE task_id=? AND decision='fallback'", (task_id,)
        ).fetchone()
    return row["n"]


def record_consent(store, *, task_id: str, connection_id: str, provider: str, data_scope: str, granted_by: str) -> str:
    consent_id = new_id("frc_")
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "INSERT INTO fallback_consent_records "
                "(id, task_id, target_connection_id, target_provider, data_scope, granted_by, granted_at, revoked_at) "
                "VALUES (?,?,?,?,?,?,?,NULL)",
                (consent_id, task_id, connection_id, provider, data_scope, granted_by, _now()),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return consent_id


def revoke_consent(store, consent_id: str) -> None:
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "UPDATE fallback_consent_records SET revoked_at=? WHERE id=? AND revoked_at IS NULL",
                (_now(), consent_id),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise


def has_active_consent(store, task_id: str, connection_id: str) -> bool:
    with store.connection() as db:
        row = db.execute(
            "SELECT 1 FROM fallback_consent_records WHERE task_id=? AND target_connection_id=? AND revoked_at IS NULL LIMIT 1",
            (task_id, connection_id),
        ).fetchone()
    return row is not None
