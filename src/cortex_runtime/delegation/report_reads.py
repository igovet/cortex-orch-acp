"""Durable receipts for successful trusted-channel ``read_report`` calls."""
from __future__ import annotations

from datetime import datetime, timezone


def record_report_read(store, *, task_id: str, attempt_id: str | None,
                       report_id: str, request_id: str | int | None) -> bool:
    """Record an exact successful MCP read when the channel is attempt-bound.

    A missing attempt or JSON-RPC id is deliberately unrecordable: it cannot
    establish the exact execution identity needed for handoff evidence.
    Replaying the same request is idempotent.
    """
    if not attempt_id or request_id is None:
        return False
    return record_report_reads(store, task_id=task_id, attempt_id=attempt_id,
                               reads=[(report_id, request_id)])


def record_report_reads(store, *, task_id, attempt_id, reads, authorization=None):
    """Commit one delivery's observations together, optionally under live authority."""
    if not attempt_id:
        return False
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            if authorization is not None:
                authorization(db)
            for report_id, request_id in reads:
                if request_id is None:
                    continue
                db.execute(
                    "INSERT OR IGNORE INTO report_read_observations "
                    "(task_id,attempt_id,report_id,request_id,outcome,created_at) "
                    "VALUES (?,?,?,?,?,?)",
                    (task_id, attempt_id, report_id, str(request_id), "success",
                     datetime.now(timezone.utc).isoformat(timespec="microseconds")),
                )
            inserted = db.total_changes > 0
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return inserted

