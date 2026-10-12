"""T023: durable, cross-process attempt state used by recovery.

Two additive tables (see ``store.py``'s composed ``SCHEMA``):

- ``attempt_process_registry`` -- one row per attempt whose engine process
  was actually launched (coordinator or worker), recording enough OS-level
  identity (see ``engine.process_identity``) that a *different* Cortex
  process (a restart) can tell "still running" apart from "exited" apart
  from "cannot tell" for a process it never itself launched -- the T023
  fix for ``delegation.live_adapters.EngineSessionManagerAdapter.lookup_owned``
  otherwise always answering "absent" after a restart (see the task's own
  scout finding). Never a substitute for ``EngineProcess``'s own same-
  process observation, which is always preferred when the launching
  manager is still resident in this process.
- ``attempt_effective_context`` -- one row per attempt recording the
  configuration revision (selected == effective at open time, since it is
  read once from ``ConfigurationService.current``) and route
  (engine/connection) an attempt was actually opened with, plus the
  ``ResourcePolicy`` it was granted. A replacement session (T023) reads
  this back and reuses it verbatim unless an explicit new configuration
  revision has since been selected/applied (see
  ``recovery.coordinator_recovery``): recovery itself never grants a
  broader policy or a different route than what was actually recorded.

A worker's row is paired with an ``acp_host`` row (see :data:`HOST_ROLE`)
naming the Cortex ACP process that launched it. A host can be replaced
without any signal reaching the old process (SIGKILL, an OOM kill), so no
``host_disconnected`` record is written; a later process then reads the
owner row to tell "its owner is gone" apart from "still owned by a live
process" without writing anything.
"""
from __future__ import annotations

import dataclasses
import json
from datetime import datetime, timezone

from ..contracts import StoreError

PROCESS_REGISTRY_SCHEMA = '''
CREATE TABLE IF NOT EXISTS attempt_process_registry (
 attempt_id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),
 role TEXT NOT NULL, engine_session_ref TEXT, connection_id TEXT,
 pid INTEGER NOT NULL, pgid INTEGER NOT NULL,
 start_ticks TEXT NOT NULL, boot_id TEXT NOT NULL,
 launch_command_digest TEXT NOT NULL,
 recorded_at TEXT NOT NULL, terminated_at TEXT, termination_reason TEXT
);
CREATE TABLE IF NOT EXISTS attempt_effective_context (
 attempt_id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),
 role TEXT NOT NULL,
 selected_config_revision_id TEXT, effective_config_revision_id TEXT,
 route_engine TEXT NOT NULL, route_connection_id TEXT,
 resource_policy_json TEXT NOT NULL, recorded_at TEXT NOT NULL
);
'''


# The Cortex ACP process that launched a worker, recorded beside it.
HOST_ROLE = "acp_host"


def host_row_id(attempt_id: str) -> str:
    return f"{HOST_ROLE}:{attempt_id}"


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def record_process_launch(
    store, *, attempt_id: str, task_id: str, role: str, pid: int, pgid: int,
    start_ticks: str, boot_id: str, launch_command_digest: str,
    connection_id: str | None = None, engine_session_ref: str | None = None,
) -> None:
    """Record (or idempotently replace) this attempt's freshly-launched
    process identity. A replacement session for the same ``attempt_id`` --
    never expected in practice, since every replacement allocates a fresh
    attempt id (see ``recovery.coordinator_recovery``) -- would otherwise
    violate the primary key; ``INSERT OR REPLACE`` keeps this a pure
    best-effort observation table rather than a second source of failure."""
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "INSERT OR REPLACE INTO attempt_process_registry "
                "(attempt_id, task_id, role, engine_session_ref, connection_id, pid, pgid, "
                " start_ticks, boot_id, launch_command_digest, recorded_at, terminated_at, termination_reason) "
                "VALUES (?,?,?,?,?,?,?,?,?,?,?,NULL,NULL)",
                (attempt_id, task_id, role, engine_session_ref, connection_id, pid, pgid,
                 start_ticks, boot_id, launch_command_digest, _now()),
            )
            host = _own_identity() if role == "worker" else None
            if host is not None:
                db.execute(
                    "INSERT OR REPLACE INTO attempt_process_registry "
                    "(attempt_id, task_id, role, engine_session_ref, connection_id, pid, pgid, "
                    " start_ticks, boot_id, launch_command_digest, recorded_at, terminated_at, termination_reason) "
                    "VALUES (?,?,?,NULL,NULL,?,?,?,?,?,?,NULL,NULL)",
                    (host_row_id(attempt_id), task_id, HOST_ROLE, host.pid, host.pgid,
                     host.start_ticks, host.boot_id, HOST_ROLE, _now()),
                )
            db.commit()
        except BaseException:
            db.rollback()
            raise


def _own_identity():
    import os

    from ..engine import process_identity

    return process_identity.capture(os.getpid(), os.getpgrp())


def owner_liveness(db, attempt_id: str) -> str | None:
    """``"alive"``/``"absent"``/``"unknown"`` for the Cortex process that
    launched worker ``attempt_id``; ``None`` when no owner was recorded
    (an older store, or a launch whose identity could not be read).
    Read-only and callable from any process."""
    row = db.execute(
        "SELECT * FROM attempt_process_registry WHERE attempt_id=? AND role=?",
        (host_row_id(attempt_id), HOST_ROLE),
    ).fetchone()
    return probe_liveness(dict(row)) if row is not None else None


def bind_engine_session_ref(store, attempt_id: str, engine_session_ref: str) -> None:
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "UPDATE attempt_process_registry SET engine_session_ref=? WHERE attempt_id=?",
                (engine_session_ref, attempt_id),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise


def mark_terminated_tx(db, attempt_id: str, reason: str) -> None:
    """Bookkeep confirmed absence on the caller's transaction; never commit it."""
    row = db.execute(
        "SELECT terminated_at,termination_reason FROM attempt_process_registry WHERE attempt_id=?", (attempt_id,),
    ).fetchone()
    if row is not None and (row["terminated_at"] is None or row["termination_reason"] == "timeout_alive"):
        db.execute(
            "UPDATE attempt_process_registry SET terminated_at=?, termination_reason=? WHERE attempt_id=?",
            (_now(), reason, attempt_id),
        )


def mark_terminated(store, attempt_id: str, reason: str) -> None:
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            mark_terminated_tx(db, attempt_id, reason)
            db.commit()
        except BaseException:
            db.rollback()
            raise


def lookup_process(store, attempt_id: str) -> dict | None:
    with store.connection() as db:
        row = db.execute(
            "SELECT * FROM attempt_process_registry WHERE attempt_id=?", (attempt_id,),
        ).fetchone()
    return dict(row) if row is not None else None


def probe_liveness(row: dict) -> str:
    """``"alive"``/``"absent"``/``"unknown"`` for an already-fetched
    ``attempt_process_registry`` row (see :func:`lookup_process`). A row
    marked with a confirmed exit is absent. Older timeout_alive markers
    are not exit evidence and must be re-probed."""
    if row.get("terminated_at") is not None and row.get("termination_reason") != "timeout_alive":
        return "absent"
    from ..engine import process_identity

    if row.get("role") in {"worker", "coordinator"} and row.get("pgid") == row["pid"]:
        return process_identity.probe_group(process_identity.ProcessIdentity(
            row["pid"], row["pgid"], row["start_ticks"], row["boot_id"]))
    return process_identity.probe(row["pid"], row["start_ticks"], row["boot_id"])


def record_effective_context(
    store, *, attempt_id: str, task_id: str, role: str,
    selected_config_revision_id: str | None, effective_config_revision_id: str | None,
    route_engine: str, route_connection_id: str | None, resource_policy,
) -> None:
    policy_json = json.dumps(
        dataclasses.asdict(resource_policy) if dataclasses.is_dataclass(resource_policy) else dict(resource_policy),
        sort_keys=True,
    )
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "INSERT OR REPLACE INTO attempt_effective_context "
                "(attempt_id, task_id, role, selected_config_revision_id, effective_config_revision_id, "
                " route_engine, route_connection_id, resource_policy_json, recorded_at) "
                "VALUES (?,?,?,?,?,?,?,?,?)",
                (attempt_id, task_id, role, selected_config_revision_id, effective_config_revision_id,
                 route_engine, route_connection_id, policy_json, _now()),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise


def get_effective_context(store, attempt_id: str) -> dict | None:
    with store.connection() as db:
        row = db.execute(
            "SELECT * FROM attempt_effective_context WHERE attempt_id=?", (attempt_id,),
        ).fetchone()
    if row is None:
        return None
    result = dict(row)
    try:
        result["resource_policy"] = json.loads(result["resource_policy_json"])
    except (TypeError, ValueError):
        raise StoreError("corrupt_effective_context")
    return result


def latest_effective_context_for_task(store, task_id: str, role: str) -> dict | None:
    """The most recently recorded ``attempt_effective_context`` row for
    ``task_id``/``role`` -- used by recovery to find the *predecessor*
    attempt's carried-over context when a fresh replacement attempt id has
    not itself recorded one yet."""
    with store.connection() as db:
        row = db.execute(
            "SELECT attempt_id FROM attempt_effective_context WHERE task_id=? AND role=? "
            "ORDER BY recorded_at DESC LIMIT 1",
            (task_id, role),
        ).fetchone()
    if row is None:
        return None
    return get_effective_context(store, row["attempt_id"])
