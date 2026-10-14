"""The user's access mode for one task: Development, Guarded or Auto.

Three trust levels over two axes -- where a writing worker may work, and
whether a deletion waits for the user:

* Guarded: the project. A deletion, a write outside the project and a
  command run outside the sandbox each wait for the user's answer on a
  permission card, and workers run with the engine mode that keeps every
  mutating tool call visible to ``engine.permissions.PermissionBridge``
  (``configuration.policy.WORKER_MODE_FLOOR``).
* Development: the whole computer (:data:`HOST_WIDE`). A writing worker's
  ``command_run`` gets host-writable mounts (the role and scopes still
  decide the project); a role without a write grant (reviewer, auditor,
  explorer...) and the runtime's automatic verification keep the read-only
  host profile (:data:`READ_ONLY_COMMAND_PROFILE`) in every mode; and
  an outside file write needs no card; a recognized deletion, a write to a
  protected control-plane path and a command run outside the sandbox still ask.
* Auto: the whole computer, and nothing asks. The same host-writable command
  profile as Development, and the runtime answers every approval card itself
  by selecting the engine's own one-shot allow option (see
  :func:`auto_approve`, used by
  ``acp.orchestrating_runtime.OrchestratingRuntime.permission_requester_for_task``,
  the one requester both approval paths reach). A worker opened while the
  task is in Auto runs with the most permissive engine mode its session
  actually offers (:data:`WORKER_AUTO_MODES`), still confirm-or-fail.

No mode lifts what is not a user approval: the engine jail, its ``.cortex``
and ``.git`` config/hooks protection, the protected control-plane mounts of
the host command profile, the channel-verified MCP gateway (scopes, write
denials, role rules -- read-only roles never write -- receipts), the
native-subagent ban, the coordinator's own engine mode floor, the paid-call
probe safeguards and secret handling all hold in every mode.

The mode is changed only by the user, through the ACP client (the session's
access option or mode) -- no MCP tool writes it, so a model cannot. Every
switch is an append-only revision in ``access_mode_events`` with its origin;
a task with no revision is Guarded, and anything unreadable or unknown reads
as Guarded. Fresh ACP sessions durably start Development; legacy sessions
remain Guarded.
"""
from __future__ import annotations

import json
import sqlite3
from datetime import datetime, timezone
from typing import Mapping

GUARDED = "guarded"
AUTO = "auto"
DEVELOPMENT = "development"
DEFAULT = GUARDED  # Missing, legacy or unreadable authority is restrictive.
FRESH_SESSION_DEFAULT = DEVELOPMENT
MODES = (DEVELOPMENT, GUARDED, AUTO)
#: Modes whose writing workers work on the whole computer: host-writable
#: brokered commands (``orchestration.sandbox.development_command``) and
#: outside file writes without a routine card.
HOST_WIDE = frozenset({DEVELOPMENT, AUTO})


#: The receipt profile of a ``command_run`` by a role without a write grant
#: and of an automatic verification check, in every mode: the host read-only
#: except tool caches and private temp
#: (``orchestration.sandbox.readonly_command``). Distinct from the Guarded
#: writer profile, so receipts, check observations, reviewer reuse and gap
#: closure can tell the two apart.
READ_ONLY_COMMAND_PROFILE = "read_only"
#: The receipt profile of a writing worker's ``command_run`` in Guarded
#: (``orchestration.sandbox.gateway_command``: writable only inside its
#: scopes). Receipts recorded before the read-only profile had its own name
#: (including earlier read-only and verification runs) also carry it.
GUARDED_COMMAND_PROFILE = "restricted"
#: The receipt profile of a writing worker's ``command_run`` in Development
#: and Auto (``orchestration.sandbox.development_command``: host-writable).
HOST_WRITABLE_COMMAND_PROFILE = "development"


def command_profile(mode) -> str:
    """The sandbox profile a writing worker's ``command_run`` of a task in
    ``mode`` gets: ``development`` (host-writable) or ``restricted``.
    Receipts record it. A role without a write grant and an automatic
    verification check always get :data:`READ_ONLY_COMMAND_PROFILE`."""
    return HOST_WRITABLE_COMMAND_PROFILE if normalize(mode) in HOST_WIDE else GUARDED_COMMAND_PROFILE
LABELS = {DEVELOPMENT: "Development mode", GUARDED: "Guarded mode", AUTO: "Auto mode"}
DESCRIPTIONS = {
    DEVELOPMENT: "Workers can work anywhere on this computer without asking. A deletion still asks you first. Docker and arbitrary code can bypass filesystem masks.",
    GUARDED: "Workers work inside the project. Asks you before a deletion, a write outside the project or a command outside the sandbox.",
    AUTO: "Workers can work anywhere on this computer and nothing asks you, deletions included. .git/.cortex protection, role limits and MCP checks still apply.",
}

#: Where a switch may come from: the user's own client controls. Nothing a
#: model can call writes the mode (no MCP tool exists for it).
USER_ORIGINS = frozenset({"acp_config_option", "acp_set_mode", "acp_pre_task_selection"})

#: Engine modes for a worker opened in Auto, most permissive first. The
#: first one the worker's own session offers is confirmed over the wire, or
#: the worker does not run (``delegation.live_adapters
#: .EngineSessionManagerAdapter._apply_worker_mode_floor``). codex-acp's
#: per-turn approval policy is pinned by Cortex's own adapter patch
#: (``engine.process``), so a codex worker still sends its permission
#: requests, which Auto then answers.
WORKER_AUTO_MODES: Mapping[str, tuple[str, ...]] = {
    "claude-agent-acp": ("bypassPermissions", "acceptEdits", "default"),
    "codex-acp": ("agent-full-access", "agent"),
}

SCHEMA = '''CREATE TABLE IF NOT EXISTS access_mode_events (
 id INTEGER PRIMARY KEY AUTOINCREMENT, task_id TEXT NOT NULL, kind TEXT NOT NULL,
 mode TEXT, origin TEXT, detail_json TEXT NOT NULL, created_at TEXT NOT NULL
);'''


class InvalidAccessMode(ValueError):
    """An unknown mode or a switch from outside the user's own controls."""


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def normalize(value) -> str:
    """Unknown or missing reads as Guarded, never as Auto."""
    return value if value in MODES else DEFAULT


def _exists(db) -> bool:
    return db.execute(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='access_mode_events'").fetchone() is not None


def current(store, task_id: str | None) -> str:
    """The task's mode: its latest switch, else Guarded. Read-only (never
    creates the table), and fails closed to Guarded."""
    if not task_id or store is None:
        return DEFAULT
    try:
        with store.connection() as db:
            if not _exists(db):
                return DEFAULT
            row = db.execute(
                "SELECT mode FROM access_mode_events WHERE task_id=? AND kind IN ('switched','initialized') ORDER BY id DESC LIMIT 1",
                (task_id,)).fetchone()
    except (sqlite3.Error, OSError):
        return DEFAULT
    return normalize(row[0] if row is not None else None)


def switch(store, task_id: str, mode: str, *, origin: str, session_id: str | None = None) -> dict:
    """Record the user's switch as the task's next revision. A switch to the
    current mode is still recorded: every explicit choice is visible."""
    if mode not in MODES:
        raise InvalidAccessMode(f"unknown access mode {mode!r}; expected one of {list(MODES)}")
    if origin not in USER_ORIGINS:
        raise InvalidAccessMode(f"access mode may only be changed from the user's client, not {origin!r}")
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(SCHEMA)
            previous = db.execute(
                "SELECT mode FROM access_mode_events WHERE task_id=? AND kind IN ('switched','initialized') ORDER BY id DESC LIMIT 1",
                (task_id,)).fetchone()
            revision = db.execute(
                "SELECT COUNT(*) FROM access_mode_events WHERE task_id=? AND kind IN ('switched','initialized')", (task_id,)).fetchone()[0] + 1
            detail = dict(revision=revision, previous=normalize(previous[0] if previous else None), session_id=session_id)
            db.execute(
                "INSERT INTO access_mode_events(task_id,kind,mode,origin,detail_json,created_at) VALUES(?,?,?,?,?,?)",
                (task_id, "switched", mode, origin, json.dumps(detail, sort_keys=True), _now()))
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return dict(task_id=task_id, mode=mode, origin=origin, **detail)


def history(store, task_id: str) -> list[dict]:
    with store.connection() as db:
        if not _exists(db):
            return []
        rows = db.execute(
            "SELECT kind, mode, origin, detail_json, created_at FROM access_mode_events WHERE task_id=? ORDER BY id",
            (task_id,)).fetchall()
    return [dict(kind=row[0], mode=row[1], origin=row[2], detail=json.loads(row[3]), created_at=row[4]) for row in rows]


def _allow_option_id(params) -> str | None:
    """The engine's own one-shot allow option (else its standing one);
    never an id the request did not offer."""
    options = (params or {}).get("options") if isinstance(params, Mapping) else None
    if not isinstance(options, list):
        return None
    for kind in ("allow_once", "allow_always"):
        for option in options:
            if (isinstance(option, Mapping) and option.get("kind") == kind
                    and isinstance(option.get("optionId"), str) and option["optionId"]):
                return option["optionId"]
    return None


def auto_approve(store, task_id: str, params) -> dict:
    """Answer one permission request as the user would by allowing it, and
    record that the runtime did. A request offering no allow option is
    cancelled, exactly as an unanswered card is."""
    option = _allow_option_id(params)
    call = (params or {}).get("toolCall") if isinstance(params, Mapping) else None
    call = call if isinstance(call, Mapping) else {}
    # Identity only: a command line or path may carry a secret, so its text
    # stays out of this audit (the consent request row already has its subject).
    detail = dict(tool_call_id=str(call.get("toolCallId") or "")[:200], kind=str(call.get("kind") or "")[:40],
                  allowed=option is not None)
    try:
        with store.connection() as db:
            db.execute(SCHEMA)
            db.execute(
                "INSERT INTO access_mode_events(task_id,kind,mode,origin,detail_json,created_at) VALUES(?,?,?,?,?,?)",
                (task_id, "auto_approved" if option is not None else "auto_unanswerable", AUTO, "runtime",
                 json.dumps(detail, sort_keys=True), _now()))
            db.commit()
    except (sqlite3.Error, OSError):
        pass  # the audit never decides the answer
    if option is None:
        return {"outcome": {"outcome": "cancelled"}}
    return {"outcome": {"outcome": "selected", "optionId": option}}


def worker_mode(engine: str, offered, *, access: str, floor: str | None) -> str | None:
    """The engine mode a worker is confirmed in: the floor in Guarded; in
    Auto the most permissive of :data:`WORKER_AUTO_MODES` its session
    offers, falling back to the floor when it offers none of them."""
    if access == AUTO:
        for mode in WORKER_AUTO_MODES.get(engine, ()):
            if mode in offered:
                return mode
    return floor
