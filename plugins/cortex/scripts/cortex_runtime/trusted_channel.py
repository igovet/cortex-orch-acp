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
 captured_at TEXT NOT NULL, PRIMARY KEY(origin,event_id)
);
CREATE TABLE IF NOT EXISTS task_owners (
 task_id TEXT PRIMARY KEY REFERENCES tasks(id), owner_ref TEXT NOT NULL,
 epoch INTEGER NOT NULL, acquired_at TEXT NOT NULL
);
'''


def _now():
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


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
