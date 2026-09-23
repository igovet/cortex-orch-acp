"""T024: compaction-loop guard -- bound compaction frequency, measure actual
recovered space, and detect a capsule that immediately refills the context.

Built on top of the T022 ``context_generations`` ledger
(:mod:`delegation.generations`): every observed/self-issued compaction
already opens a new generation there. This module adds a second, narrower
ledger (``compaction_recovery_events``, see :mod:`recovery.budget_store`)
that records *whether that generation's compaction actually helped* --
something ``context_generations`` alone cannot answer, since it only
records that a generation began, not the before/after size.

Three independent stop conditions, any one of which blocks (never a silent
retry loop -- task card line 43 / lead decision 4):

1. **Frequency** -- more than :data:`MAX_COMPACTIONS_PER_WINDOW`
   compaction attempts recorded for this ``engine_session_ref`` within the
   most recent :data:`WINDOW_SIZE` attempts' worth of history (i.e. the
   window itself already contains more than the allowed count -- a
   long-lived, healthy session that compacts occasionally never trips
   this; a session stuck compacting every turn does).
2. **Immediate refill** -- the *fresh* generation's own mandatory content
   (capsule + role instructions + tool schemas, i.e. exactly the
   "mandatory" total :func:`recovery.token_budget.plan_budget` computes)
   already exceeds the usable budget right after compaction, before any
   new chat turn has even happened -- compacting again cannot possibly
   help, since the refill is not coming from conversation growth.
3. **Low recovery ratio** -- the last :data:`MIN_CONSECUTIVE_LOW_RECOVERY`
   consecutive attempts each recovered less than
   :data:`MIN_RECOVERY_RATIO` of the pre-compaction size (using exact
   usage-derived before/after figures when available, the shared estimator
   otherwise -- see :mod:`recovery.token_budget`) -- compaction is running
   but not actually freeing meaningful space.

A blocked result is a terminal, explicit state (never silently retried):
the caller surfaces the returned ``reason``/``message`` as a localized (via
T020's message plumbing) blocked/limit condition, exactly like an
:func:`recovery.token_budget.plan_budget` ``"overflow"`` decision -- never
loops back into another compaction attempt on its own.
"""
from __future__ import annotations

from ..portable.identity import new_id

MAX_COMPACTIONS_PER_WINDOW = 5
WINDOW_SIZE = 5
MIN_RECOVERY_RATIO = 0.15
MIN_CONSECUTIVE_LOW_RECOVERY = 3


def _recent_events(store, engine_session_ref: str, limit: int) -> list[dict]:
    with store.connection() as db:
        rows = db.execute(
            "SELECT * FROM compaction_recovery_events WHERE engine_session_ref=? "
            "ORDER BY id DESC LIMIT ?",
            (engine_session_ref, limit),
        ).fetchall()
    return [dict(row) for row in rows]


def evaluate_compaction(
    store, *, task_id: str, engine_session_ref: str, generation_id: str | None,
    before_tokens: int, before_basis: str, after_tokens: int, after_basis: str,
    mandatory_tokens_after: int, usable_window_tokens: int | None,
) -> dict:
    """Evaluate one just-completed compaction attempt and record it.

    ``usable_window_tokens`` is ``None`` when the route's real window is
    unknown; the immediate-refill check then has no limit to compare against
    and is skipped rather than run against a guessed window.

    Returns ``{"status": "ok"}`` or ``{"status": "blocked", "reason": ...,
    "message": ...}``. Always records exactly one
    ``compaction_recovery_events`` row for this attempt (whether ``ok`` or
    ``blocked``), so the next call's window/consecutive-ratio checks see it.
    """
    recovered_tokens = max(before_tokens - after_tokens, 0)
    recovered_ratio = (recovered_tokens / before_tokens) if before_tokens > 0 else 0.0

    recent = _recent_events(store, engine_session_ref, WINDOW_SIZE)
    status, reason, message = "ok", None, None

    if len(recent) >= MAX_COMPACTIONS_PER_WINDOW:
        status = "blocked"
        reason = "compaction_frequency_exceeded"
        message = (
            f"{len(recent)} compactions already recorded for this session within the last "
            f"{WINDOW_SIZE}-attempt window (limit {MAX_COMPACTIONS_PER_WINDOW}); stopping rather "
            "than looping. Ask the coordinator to split the assignment or change the execution approach."
        )
    elif usable_window_tokens is not None and mandatory_tokens_after > usable_window_tokens:
        status = "blocked"
        reason = "immediate_refill"
        message = (
            f"After compaction, mandatory content alone is estimated at {mandatory_tokens_after} "
            f"tokens, already exceeding the usable budget {usable_window_tokens}; a fresh generation's "
            "capsule and mandatory role/tool content immediately refill the context, so compacting "
            "again cannot help. Ask the coordinator to split the assignment or change the execution approach."
        )
    else:
        low_recovery_streak = [recovered_ratio < MIN_RECOVERY_RATIO]
        for event in recent[: MIN_CONSECUTIVE_LOW_RECOVERY - 1]:
            low_recovery_streak.append(bool(event["recovered_ratio"] < MIN_RECOVERY_RATIO))
        if len(low_recovery_streak) >= MIN_CONSECUTIVE_LOW_RECOVERY and all(low_recovery_streak[:MIN_CONSECUTIVE_LOW_RECOVERY]):
            status = "blocked"
            reason = "consecutive_low_recovery"
            message = (
                f"The last {MIN_CONSECUTIVE_LOW_RECOVERY} compactions for this session each recovered "
                f"less than {int(MIN_RECOVERY_RATIO * 100)}% of pre-compaction size; compaction is not "
                "meaningfully freeing space. Ask the coordinator to split the assignment or change the "
                "execution approach."
            )

    event_id = new_id("cre_")
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "INSERT INTO compaction_recovery_events "
                "(id, task_id, engine_session_ref, generation_id, before_tokens, before_basis, "
                " after_tokens, after_basis, recovered_tokens, recovered_ratio, status, reason, created_at) "
                "VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
                (event_id, task_id, engine_session_ref, generation_id, before_tokens, before_basis,
                 after_tokens, after_basis, recovered_tokens, recovered_ratio, status, reason, store._now()),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise
    result = dict(status=status, event_id=event_id, recovered_tokens=recovered_tokens, recovered_ratio=recovered_ratio)
    if status == "blocked":
        result["reason"] = reason
        result["message"] = message
    return result
