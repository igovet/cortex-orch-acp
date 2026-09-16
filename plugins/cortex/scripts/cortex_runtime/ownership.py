"""One active, fenced mutating coordination owner per task.

``task_owners`` (schema declared in ``trusted_channel.IDENTITY_SCHEMA``)
holds exactly one current ``(owner_ref, epoch)`` row per task.  Acquisition
is an atomic compare-and-swap guarded by SQLite's own ``BEGIN IMMEDIATE``
write lock, so both concurrent threads and concurrent OS processes racing
against the same on-disk database serialize to exactly one epoch-1 winner.
A later explicit transfer bumps the epoch; any channel issued at an older
epoch then fails ownership checks (see ``server.Server.dispatch``) without
needing to track or revoke individual channels.
"""
from __future__ import annotations

from datetime import datetime, timezone

from .contracts import StoreError
from .portable.identity import OwnershipEpoch


def _now():
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def current_owner_epoch(db, task_id):
    """Return the current epoch integer for ``task_id``, or ``None`` if unowned."""
    row = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (task_id,)).fetchone()
    return row["epoch"] if row is not None else None


def acquire_task_owner_tx(db, task_id, owner_ref, expected_epoch):
    """Atomic compare-and-swap acquisition/transfer inside an open ``db`` transaction.

    ``expected_epoch`` must equal the task's current epoch (``None`` only
    when the task has no owner yet); any mismatch is a stale acquisition
    attempt and is rejected rather than silently overwritten.
    """
    if not isinstance(owner_ref, str) or not owner_ref:
        raise StoreError("invalid_owner_ref")
    row = db.execute("SELECT owner_ref,epoch FROM task_owners WHERE task_id=?", (task_id,)).fetchone()
    if row is None:
        if expected_epoch not in (None, 0):
            raise StoreError("stale_ownership_epoch")
        epoch = 1
        db.execute("INSERT INTO task_owners VALUES (?,?,?,?)", (task_id, owner_ref, epoch, _now()))
        return OwnershipEpoch(epoch=epoch, owner=owner_ref)
    if expected_epoch != row["epoch"]:
        raise StoreError("stale_ownership_epoch")
    epoch = row["epoch"]+1
    db.execute("UPDATE task_owners SET owner_ref=?,epoch=?,acquired_at=? WHERE task_id=?",
               (owner_ref, epoch, _now(), task_id))
    return OwnershipEpoch(epoch=epoch, owner=owner_ref)


def acquire_task_owner(store, task_id, owner_ref, expected_epoch):
    """Standalone acquisition/transfer in its own transaction."""
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            result = acquire_task_owner_tx(db, task_id, owner_ref, expected_epoch)
            db.commit()
        except BaseException:
            db.rollback(); raise
    return result
