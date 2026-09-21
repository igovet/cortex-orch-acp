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
    request = str(request_id)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "INSERT OR IGNORE INTO report_read_observations "
                "(task_id,attempt_id,report_id,request_id,outcome,created_at) "
                "VALUES (?,?,?,?,?,?)",
                (task_id, attempt_id, report_id, request, "success",
                 datetime.now(timezone.utc).isoformat(timespec="microseconds")),
            )
            inserted = db.total_changes > 0
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return inserted

