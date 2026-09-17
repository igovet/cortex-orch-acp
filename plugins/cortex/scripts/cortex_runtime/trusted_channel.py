"""Session-bound trusted MCP channels: engine-supported identity delivery.

A channel is a server-issued (channel_id, secret) pair binding one runtime
principal (coordinator or worker) to exactly one task, with an embedded
ownership epoch that ties later mutations to the owner active when the
channel was issued.  The secret is returned exactly once at issue time and
is never stored, logged, or echoed; only its SHA-256 hash is persisted.

All table-mutating helpers here take an already-open ``db`` connection and
perform no transaction control of their own, so callers compose them inside
one larger transaction (see ``bootstrap.bootstrap_task``).  The ``store``-
taking wrappers (``issue_channel``, ``revoke_channel``) open one standalone
transaction for direct/test use.
"""
from __future__ import annotations

from dataclasses import dataclass
import hashlib
import hmac
import secrets
import uuid
from datetime import datetime, timezone

from .contracts import StoreError
from .store import identifier

IDENTITY_SCHEMA = '''
CREATE TABLE IF NOT EXISTS trusted_channels (
 channel_id TEXT PRIMARY KEY, secret_hash TEXT NOT NULL,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 principal_thread_id TEXT NOT NULL, parent_principal_thread_id TEXT,
 role TEXT NOT NULL, assignment_id TEXT, attempt_id TEXT,
 workspace_binding_id TEXT, context_generation_id TEXT,
 owner_epoch INTEGER NOT NULL, issued_at TEXT NOT NULL,
 expires_at TEXT, revoked_at TEXT
);
CREATE TABLE IF NOT EXISTS source_events (
 origin TEXT NOT NULL, event_id TEXT NOT NULL, task_id TEXT NOT NULL REFERENCES tasks(id),
 captured_at TEXT NOT NULL, meta_json TEXT, ledger_state TEXT, PRIMARY KEY(origin,event_id)
);
CREATE TABLE IF NOT EXISTS task_owners (
 task_id TEXT PRIMARY KEY REFERENCES tasks(id), owner_ref TEXT NOT NULL,
 epoch INTEGER NOT NULL, acquired_at TEXT NOT NULL
);
'''


def _now():
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def ensure_source_event_meta_column(db) -> None:
    """(T018) Idempotently add ``source_events.meta_json`` to an
    already-open connection whose ``source_events`` table predates it (a
    store created at/before b19d288). Safe to call on every
    :class:`store.Store` open; a no-op once the column exists. Mirrors
    ``delegation.events_store.ensure_task_lifecycle_columns`` exactly."""
    columns = {row[1] for row in db.execute("PRAGMA table_info(source_events)").fetchall()}
    if not columns or "meta_json" in columns:
        return
    db.execute("BEGIN IMMEDIATE")
    try:
        db.execute("ALTER TABLE source_events ADD COLUMN meta_json TEXT")
        db.commit()
    except BaseException:
        db.rollback()
        raise


def ensure_source_event_ledger_column(db) -> None:
    """(T018 fresh-review fix, HIGH) Idempotently add
    ``source_events.ledger_state`` to an already-open connection whose
    ``source_events`` table predates it, and backfill every pre-existing
    row as ``'legacy_recorded'``.

    This is the structural fix for the reproduced legacy re-delivery bug:
    ``acp.replay_store.find_unrecorded_source_events`` (crash recovery
    between source capture and coordinator delivery -- see
    ``acp.orchestrating_runtime``) now considers *only* rows with a NULL
    ``ledger_state``, set the same way for every source event regardless of
    era (see ``acp.replay_store.record_user_message_event``): non-NULL
    means "a ``task_events`` ``user_message`` row for this identity has
    already been durably recorded" -- never re-derived by pattern-matching
    a ``task_events.dedup_key`` string (the b19d288-era
    ``user_message:<session_id>:<turn_index>`` shape is not reconstructible
    from a bare ``source_events`` row, and *any* text-matching heuristic
    over it would be exactly the bug this fixes).

    Backfilling every pre-existing row as ``'legacy_recorded'`` (rather
    than leaving it NULL, or trying to infer per-row whether it was
    actually delivered) is a deliberate, documented decision: **no
    pre-T018 source event is ever auto-recovered** by this migration --
    only a source event captured *after* this column exists (and is
    therefore genuinely NULL until ``record_user_message_event`` marks it)
    is ever recovery-eligible. This trades a theoretical (and, before this
    fix, actually reproduced in the opposite direction) missed recovery of
    a handful of already-upgraded-era rows for never re-delivering a
    project's entire prompt history on first use after upgrade.
    """
    columns = {row[1] for row in db.execute("PRAGMA table_info(source_events)").fetchall()}
    if not columns or "ledger_state" in columns:
        return
    db.execute("BEGIN IMMEDIATE")
    try:
        db.execute("ALTER TABLE source_events ADD COLUMN ledger_state TEXT")
        db.execute("UPDATE source_events SET ledger_state='legacy_recorded' WHERE ledger_state IS NULL")
        db.commit()
    except BaseException:
        db.rollback()
        raise


def _hash_secret(secret):
    return hashlib.sha256(secret.encode()).hexdigest()


@dataclass(frozen=True)
class ChannelContext:
    """The resolved, trusted identity carried by one validated channel."""
    channel_id: str
    task_id: str
    principal_thread_id: str
    parent_principal_thread_id: str | None
    role: str
    owner_epoch: int
    assignment_id: str | None
    attempt_id: str | None
    workspace_binding_id: str | None
    context_generation_id: str | None


def issue_channel_tx(db, task_id, role, owner_epoch, *, parent_channel_id=None,
                      expires_at=None, assignment_id=None, attempt_id=None,
                      workspace_binding_id=None, context_generation_id=None,
                      principal_thread_id=None):
    """Allocate one channel and its principal thread binding inside ``db``.

    Returns ``(channel_id, secret)``; the secret is generated here, returned
    once, and never persisted (only ``secret_hash`` is stored).
    """
    if role not in ("coordinator", "worker"):
        raise StoreError("invalid_channel_role")
    parent_principal = None
    if role == "worker":
        if parent_channel_id is None:
            raise StoreError("channel_parent_required")
        parent_row = db.execute(
            "SELECT principal_thread_id,task_id FROM trusted_channels WHERE channel_id=?",
            (parent_channel_id,)).fetchone()
        if parent_row is None:
            raise StoreError("channel_unknown")
        if parent_row["task_id"] != task_id:
            raise StoreError("cross_task_channel")
        parent_principal = parent_row["principal_thread_id"]
    principal = principal_thread_id or str(uuid.uuid4())
    channel_id = identifier("ch_")
    secret = secrets.token_urlsafe(32)
    db.execute(
        "INSERT INTO trusted_channels VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        (channel_id, _hash_secret(secret), task_id, principal, parent_principal, role,
         assignment_id, attempt_id, workspace_binding_id, context_generation_id,
         owner_epoch, _now(), expires_at, None))
    db.execute("INSERT OR IGNORE INTO thread_bindings VALUES (?,?,?)",
               (principal, parent_principal, task_id))
    return channel_id, secret


def issue_channel(store, task_id, role, owner_epoch, **kwargs):
    """Standalone issuance in its own transaction (direct/test use)."""
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            result = issue_channel_tx(db, task_id, role, owner_epoch, **kwargs)
            db.commit()
        except BaseException:
            db.rollback(); raise
    return result


def revoke_channel(store, channel_id):
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            row = db.execute("SELECT revoked_at FROM trusted_channels WHERE channel_id=?", (channel_id,)).fetchone()
            if row is None:
                raise StoreError("channel_unknown")
            if row["revoked_at"] is None:
                db.execute("UPDATE trusted_channels SET revoked_at=? WHERE channel_id=?", (_now(), channel_id))
            db.commit()
        except BaseException:
            db.rollback(); raise


def resolve_channel(store, channel_id, secret, now=None):
    """Validate a (channel_id, secret) pair and return its trusted context.

    Unknown channel id and a wrong secret share the same ``channel_unknown``
    error so a forged identifier cannot be distinguished from a guessed one.
    """
    if not isinstance(channel_id, str) or not isinstance(secret, str) or not channel_id or not secret:
        raise StoreError("channel_unknown")
    with store.connection() as db:
        row = db.execute("SELECT * FROM trusted_channels WHERE channel_id=?", (channel_id,)).fetchone()
    if row is None or not hmac.compare_digest(row["secret_hash"], _hash_secret(secret)):
        raise StoreError("channel_unknown")
    if row["revoked_at"] is not None:
        raise StoreError("channel_revoked")
    current = now or _now()
    if row["expires_at"] is not None and current >= row["expires_at"]:
        raise StoreError("channel_expired")
    return ChannelContext(
        channel_id=row["channel_id"], task_id=row["task_id"],
        principal_thread_id=row["principal_thread_id"],
        parent_principal_thread_id=row["parent_principal_thread_id"],
        role=row["role"], owner_epoch=row["owner_epoch"],
        assignment_id=row["assignment_id"], attempt_id=row["attempt_id"],
        workspace_binding_id=row["workspace_binding_id"],
        context_generation_id=row["context_generation_id"])
