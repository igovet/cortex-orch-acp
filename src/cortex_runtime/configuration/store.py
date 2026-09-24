"""Configuration persistence: project defaults and task revision history.

Two tables composed into ``store.py``'s per-project SQLite database
(mirroring the ``catalog.store``/``CATALOG_SCHEMA`` split):

- ``project_configuration_defaults``: one row per project, holding the
  project's own default parameters and its :class:`PolicyCeiling` (may only
  narrow the user ceiling; enforced by ``configuration.service``, not by
  this module). Freely updatable -- it is a live default, not history.
- ``configuration_revisions``: an append-mostly, numbered sequence per task.
  A row's identity columns (id/task_id/revision/route_id/parameters/
  ceiling_snapshot/created_at) are immutable; only its lifecycle ``state``
  (plus ``confirmation_evidence_ref``/``error_code``/``updated_at``) may
  change, and only along the state machine documented on
  ``CONFIGURATION_SCHEMA``. No DELETE except via the existing retention
  ``pending_deletions`` path (see ``cleanup.finish_deletions``).
"""
from __future__ import annotations

from datetime import datetime, timezone
import json
import sqlite3

from ..connections import registry as connection_registry
from ..portable.errors import PortableError, PortableErrorCategory
from ..portable.identity import _validate_prefixed_id
from .policy import FallbackPolicy, PermissionLevel, PolicyCeiling

CONFIGURATION_SCHEMA = '''
CREATE TABLE IF NOT EXISTS project_configuration_defaults (
 project_root TEXT PRIMARY KEY, defaults TEXT NOT NULL, ceiling TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS configuration_revisions (
 id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id), revision INTEGER NOT NULL,
 route_id TEXT NOT NULL, route_engine TEXT NOT NULL, route_connection_id TEXT NOT NULL,
 parameters TEXT NOT NULL, ceiling_snapshot TEXT NOT NULL,
 state TEXT NOT NULL, confirmation_evidence_ref TEXT, error_code TEXT,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL, UNIQUE(task_id,revision)
);
CREATE INDEX IF NOT EXISTS configuration_revisions_by_task ON configuration_revisions(task_id,revision);
CREATE TRIGGER IF NOT EXISTS configuration_revisions_guard BEFORE UPDATE ON configuration_revisions
BEGIN
 SELECT CASE
  WHEN NEW.id!=OLD.id OR NEW.task_id!=OLD.task_id OR NEW.revision!=OLD.revision
   OR NEW.route_id!=OLD.route_id OR NEW.route_engine!=OLD.route_engine
   OR NEW.route_connection_id!=OLD.route_connection_id OR NEW.parameters!=OLD.parameters
   OR NEW.ceiling_snapshot!=OLD.ceiling_snapshot OR NEW.created_at!=OLD.created_at
  THEN RAISE(ABORT,'immutable')
  WHEN OLD.state='selected' AND NEW.state NOT IN ('selected','pending_next_use','rejected')
  THEN RAISE(ABORT,'invalid_transition')
  WHEN OLD.state='pending_next_use' AND NEW.state NOT IN ('pending_next_use','active','failed','indeterminate')
  THEN RAISE(ABORT,'invalid_transition')
  WHEN OLD.state='indeterminate' AND NEW.state NOT IN ('indeterminate','active','failed')
  THEN RAISE(ABORT,'invalid_transition')
  WHEN OLD.state IN ('active','rejected','failed') AND NEW.state!=OLD.state
  THEN RAISE(ABORT,'invalid_transition')
  WHEN NEW.state='active' AND (NEW.confirmation_evidence_ref IS NULL OR NEW.confirmation_evidence_ref='')
  THEN RAISE(ABORT,'missing_confirmation_evidence')
  WHEN NEW.state='active' AND EXISTS (
   SELECT 1 FROM configuration_revisions o WHERE o.task_id=NEW.task_id AND o.revision>NEW.revision
    AND o.state NOT IN ('rejected','failed')
  )
  THEN RAISE(ABORT,'stale_reconcile_target')
 END;
END;
CREATE TRIGGER IF NOT EXISTS configuration_revisions_no_delete BEFORE DELETE ON configuration_revisions
WHEN NOT EXISTS (
 SELECT 1 FROM pending_deletions d WHERE d.task_id=OLD.task_id
) BEGIN SELECT RAISE(ABORT, 'immutable'); END;
'''

CONFIGURATION_TABLES = frozenset({"project_configuration_defaults", "configuration_revisions"})

STATES = frozenset({"selected", "pending_next_use", "active", "rejected", "failed", "indeterminate"})


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def _fail_closed(code: str, remediation: str, *, field: str | None = None) -> PortableError:
    return PortableError(
        category=PortableErrorCategory.VALIDATION,
        code=code,
        remediation=remediation,
        retry_eligible=False,
        field=field,
    )


def _encode(value: object) -> str:
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"))


def serialize_ceiling(ceiling: PolicyCeiling) -> str:
    fallback = ceiling.fallback_policy
    return _encode(
        {
            "allowed_routes": sorted(ceiling.allowed_routes) if ceiling.allowed_routes is not None else None,
            "allowed_connections": sorted(ceiling.allowed_connections) if ceiling.allowed_connections is not None else None,
            "allowed_data_recipients": sorted(ceiling.allowed_data_recipients) if ceiling.allowed_data_recipients is not None else None,
            "permission_level": ceiling.permission_level.name if ceiling.permission_level is not None else None,
            "limits": dict(ceiling.limits),
            "spend_limit": ceiling.spend_limit,
            "spend_limit_enforceable": ceiling.spend_limit_enforceable,
            "fallback_policy": {
                "allowed_target_routes": sorted(fallback.allowed_target_routes) if fallback.allowed_target_routes is not None else None,
                "allowed_failure_classes": sorted(fallback.allowed_failure_classes) if fallback.allowed_failure_classes is not None else None,
                "budget": fallback.budget,
                "data_recipient_consent": fallback.data_recipient_consent,
            },
        }
    )


def deserialize_ceiling(raw: str) -> PolicyCeiling:
    try:
        data = json.loads(raw)
        fallback_data = data["fallback_policy"]
        fallback = FallbackPolicy(
            allowed_target_routes=set(fallback_data["allowed_target_routes"]) if fallback_data["allowed_target_routes"] is not None else None,
            allowed_failure_classes=set(fallback_data["allowed_failure_classes"]) if fallback_data["allowed_failure_classes"] is not None else None,
            budget=fallback_data["budget"],
            data_recipient_consent=fallback_data["data_recipient_consent"],
        )
        return PolicyCeiling(
            allowed_routes=set(data["allowed_routes"]) if data["allowed_routes"] is not None else None,
            allowed_connections=set(data["allowed_connections"]) if data["allowed_connections"] is not None else None,
            allowed_data_recipients=set(data["allowed_data_recipients"]) if data["allowed_data_recipients"] is not None else None,
            permission_level=PermissionLevel[data["permission_level"]] if data["permission_level"] is not None else None,
            limits=data["limits"],
            spend_limit=data["spend_limit"],
            spend_limit_enforceable=data["spend_limit_enforceable"],
            fallback_policy=fallback,
        )
    except (KeyError, TypeError, ValueError):
        raise _fail_closed("invalid_ceiling_snapshot", "a stored ceiling snapshot has an unexpected shape.") from None


# --- user defaults (user-scoped, filesystem; no secrets, never ceiling here
#     beyond what a project may only narrow) ----------------------------------

def _user_defaults_path(environ: dict[str, str] | None = None):
    return connection_registry.config_root(environ) / "defaults.json"


def read_user_defaults(environ: dict[str, str] | None = None) -> dict[str, object]:
    """``{"defaults": {...}, "ceiling": PolicyCeiling}``; empty when unset."""
    path = _user_defaults_path(environ)
    connection_registry.ensure_private_dir(path.parent, root=connection_registry.config_root(environ), environ=environ)
    data = connection_registry.read_owner_file(path)
    if data is None:
        return dict(defaults={}, ceiling=PolicyCeiling())
    try:
        raw = json.loads(data.decode("utf-8"))
        return dict(defaults=raw.get("defaults", {}), ceiling=deserialize_ceiling(_encode(raw["ceiling"])) if "ceiling" in raw else PolicyCeiling())
    except (UnicodeDecodeError, ValueError, KeyError):
        raise _fail_closed("invalid_user_defaults", "defaults.json is not valid UTF-8 JSON with the expected shape.") from None


def inspect_user_defaults(environ: dict[str, str] | None = None) -> dict[str, object]:
    """Read user defaults without initializing the connection config root."""
    path = _user_defaults_path(environ)
    try:
        info = path.parent.lstat()
    except FileNotFoundError:
        return dict(defaults={}, ceiling=PolicyCeiling())
    import stat
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        raise _fail_closed("unsafe_storage", f"{path.parent} is not a real directory.")
    data = connection_registry.read_owner_file(path)
    if data is None:
        return dict(defaults={}, ceiling=PolicyCeiling())
    try:
        raw = json.loads(data.decode("utf-8"))
        return dict(defaults=raw.get("defaults", {}), ceiling=deserialize_ceiling(_encode(raw["ceiling"])) if "ceiling" in raw else PolicyCeiling())
    except (UnicodeDecodeError, ValueError, KeyError):
        raise _fail_closed("invalid_user_defaults", "defaults.json is not valid UTF-8 JSON with the expected shape.") from None


def write_user_defaults(defaults: dict[str, object], ceiling: PolicyCeiling, environ: dict[str, str] | None = None) -> None:
    path = _user_defaults_path(environ)
    connection_registry.ensure_private_dir(path.parent, root=connection_registry.config_root(environ), environ=environ)
    payload = _encode({"defaults": defaults, "ceiling": json.loads(serialize_ceiling(ceiling))}).encode("utf-8")
    connection_registry.atomic_write_private(path, payload, directory=path.parent)


# --- project defaults (project-scoped, freely updatable) --------------------

def project_ceiling_tx(db, project_root: str) -> PolicyCeiling | None:
    """Read only the project's current ceiling, on the caller's own ``db``.

    Never opens a nested ``store.connection()`` (which would re-acquire the
    storage lock); used by ``configuration.service`` to recheck a possibly
    tightened project ceiling from inside an already-open transaction.
    """
    row = db.execute(
        "SELECT ceiling FROM project_configuration_defaults WHERE project_root=?", (project_root,)
    ).fetchone()
    return deserialize_ceiling(row["ceiling"]) if row is not None else None


def read_project_defaults(store, project_root: str) -> dict[str, object] | None:
    with store.connection() as db:
        row = db.execute(
            "SELECT * FROM project_configuration_defaults WHERE project_root=?", (project_root,)
        ).fetchone()
    if row is None:
        return None
    return dict(
        project_root=row["project_root"],
        defaults=json.loads(row["defaults"]),
        ceiling=deserialize_ceiling(row["ceiling"]),
        updated_at=row["updated_at"],
    )


def write_project_defaults(store, project_root: str, defaults: dict[str, object], ceiling: PolicyCeiling) -> dict[str, object]:
    updated_at = _now()
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "INSERT INTO project_configuration_defaults VALUES (?,?,?,?) "
                "ON CONFLICT(project_root) DO UPDATE SET defaults=excluded.defaults,ceiling=excluded.ceiling,updated_at=excluded.updated_at",
                (project_root, _encode(defaults), serialize_ceiling(ceiling), updated_at),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return dict(project_root=project_root, defaults=defaults, ceiling=ceiling, updated_at=updated_at)


# --- configuration revisions (task-scoped, immutable except state) ---------

def latest_revision_tx(db, task_id: str):
    return db.execute(
        "SELECT * FROM configuration_revisions WHERE task_id=? ORDER BY revision DESC LIMIT 1", (task_id,)
    ).fetchone()


def has_live_newer_revision_tx(db, task_id: str, revision: int) -> bool:
    """True when a revision numbered higher than ``revision`` exists for
    ``task_id`` whose state is not ``rejected``/``failed`` (i.e. it is
    still ``selected``, ``pending_next_use``, ``active``, or
    ``indeterminate``). Mirrors the ``configuration_revisions_guard``
    trigger's own ``NOT IN ('rejected','failed')`` condition exactly, so a
    revision this returns ``False`` for can never be blocked by that
    trigger either.
    """
    row = db.execute(
        "SELECT 1 FROM configuration_revisions WHERE task_id=? AND revision>? AND state NOT IN ('rejected','failed') LIMIT 1",
        (task_id, revision),
    ).fetchone()
    return row is not None


def revision_row_tx(db, task_id: str, revision_id: str):
    row = db.execute(
        "SELECT * FROM configuration_revisions WHERE task_id=? AND id=?", (task_id, revision_id)
    ).fetchone()
    if row is None:
        raise _fail_closed("configuration_revision_not_found", "revision_id must reference an existing revision of this task.", field="revision_id")
    return row


def insert_revision_tx(db, task_id: str, revision: int, route, parameters: dict, ceiling_snapshot: PolicyCeiling) -> dict[str, object]:
    if db.execute("SELECT 1 FROM tasks WHERE id=?", (task_id,)).fetchone() is None:
        raise _fail_closed("unknown_task", "task_id must reference an existing task.", field="task_id")
    revision_id = "cr_" + __import__("uuid").uuid4().hex[:12]
    now = _now()
    db.execute(
        "INSERT INTO configuration_revisions(id,task_id,revision,route_id,route_engine,route_connection_id,parameters,ceiling_snapshot,state,confirmation_evidence_ref,error_code,created_at,updated_at) "
        "VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
        (revision_id, task_id, revision, route.id, route.engine, route.connection_id, _encode(parameters), serialize_ceiling(ceiling_snapshot), "selected", None, None, now, now),
    )
    return _row_dict(db.execute("SELECT * FROM configuration_revisions WHERE id=?", (revision_id,)).fetchone())


_TRIGGER_MESSAGES = frozenset({"immutable", "invalid_transition", "missing_confirmation_evidence", "stale_reconcile_target"})


def transition_tx(db, task_id: str, revision_id: str, new_state: str, *, confirmation_evidence_ref: str | None = None, error_code: str | None = None) -> dict[str, object]:
    if new_state not in STATES:
        raise _fail_closed("invalid_state", f"state must be one of {sorted(STATES)}.", field="state")
    try:
        db.execute(
            "UPDATE configuration_revisions SET state=?,confirmation_evidence_ref=?,error_code=?,updated_at=? WHERE task_id=? AND id=?",
            (new_state, confirmation_evidence_ref, error_code, _now(), task_id, revision_id),
        )
    except sqlite3.IntegrityError as error:
        message = str(error)
        code = next((candidate for candidate in _TRIGGER_MESSAGES if candidate in message), "invalid_transition")
        raise _fail_closed(code, f"The configuration_revisions_guard trigger rejected this transition: {message}", field="state") from None
    return _row_dict(revision_row_tx(db, task_id, revision_id))


def history_tx(db, task_id: str) -> tuple[dict[str, object], ...]:
    rows = db.execute(
        "SELECT * FROM configuration_revisions WHERE task_id=? ORDER BY revision ASC", (task_id,)
    ).fetchall()
    return tuple(_row_dict(row) for row in rows)


def current_active_tx(db, task_id: str):
    row = db.execute(
        "SELECT * FROM configuration_revisions WHERE task_id=? AND state='active' ORDER BY revision DESC LIMIT 1",
        (task_id,),
    ).fetchone()
    return _row_dict(row) if row is not None else None


def _row_dict(row) -> dict[str, object]:
    return dict(
        id=row["id"],
        task_id=row["task_id"],
        revision=row["revision"],
        route_id=row["route_id"],
        route_engine=row["route_engine"],
        route_connection_id=row["route_connection_id"],
        parameters=json.loads(row["parameters"]),
        ceiling_snapshot=deserialize_ceiling(row["ceiling_snapshot"]),
        state=row["state"],
        confirmation_evidence_ref=row["confirmation_evidence_ref"],
        error_code=row["error_code"],
        created_at=row["created_at"],
        updated_at=row["updated_at"],
    )
