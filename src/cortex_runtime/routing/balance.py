"""Recent project-local route choices used to balance automatic worker routing.

Reads only the immutable attempt audit. The bounded rowid scan makes the
result deterministic even when older records have identical timestamps.
"""
from __future__ import annotations

from collections import Counter

BALANCE_WINDOW = 20
BALANCE_SCAN = 256
BALANCE_TOLERANCE = 4


def recent_engine_attempts(store, tier: str, *, window: int = BALANCE_WINDOW,
                           scan: int = BALANCE_SCAN) -> dict[str, str]:
    """Return up to `window` newest attempt IDs and engines for one tier.

    Scan at most `scan` worker rows, including older rows with no route
    decision. Pinned choices count; fallback rows without a tier do not.
    """
    if window < 0 or scan < 0:
        raise ValueError("window and scan must be nonnegative")
    with store.connection() as db:
        rows = db.execute(
            """
            SELECT attempt_id, engine FROM (
                SELECT attempt_id, engine, decision_detail_json, rowid AS audit_rowid
                FROM attempt_route_audit
                WHERE role != 'coordinator'
                ORDER BY rowid DESC LIMIT ?
            )
            WHERE json_extract(decision_detail_json, '$.route_decision.tier') = ?
            ORDER BY audit_rowid DESC LIMIT ?
            """,
            (scan, tier, window),
        ).fetchall()
    return {row["attempt_id"]: row["engine"] for row in rows}


def recent_engine_counts(store, tier: str, *, window: int = BALANCE_WINDOW,
                         scan: int = BALANCE_SCAN) -> dict[str, int]:
    """Count engines in the bounded recent same-tier audit window."""
    return dict(Counter(recent_engine_attempts(store, tier, window=window, scan=scan).values()))
