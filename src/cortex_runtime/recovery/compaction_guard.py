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

1. **Frequency** -- this ``engine_session_ref`` already has
   :data:`MAX_COMPACTIONS_PER_WINDOW` recorded attempts within
   :data:`WINDOW_SECONDS` of the current attempt's timestamp, so the
   current one would exceed the limit. The window is wall-clock time
   from the persisted ``created_at`` of each row (so it survives a Cortex
   restart): a long-lived, healthy session that compacts occasionally
   never trips this however many compactions it accumulates; a session
   stuck compacting in a burst does. A row exactly one window old has aged
   out. A row stamped a window or more *after* the current attempt (a
   large clock correction) or with an unreadable timestamp is not evidence
   of a recent compaction and is not counted, so it cannot hold the
   session blocked.
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
   but not actually freeing meaningful space. An attempt whose
   pre-compaction size was never observed (:data:`UNMEASURED_BASIS`) has no
   ratio at all: it is recorded as unmeasured, never counted as a low
   recovery, and never part of a streak -- an absent signal is not evidence
   that compaction failed. "Last" means newest by ``created_at`` (row ids
   are random and carry no order); unmeasured attempts in between are
   skipped, neither breaking nor extending the streak.

A blocked result is a terminal, explicit state (never silently retried):
the caller surfaces the returned ``reason``/``message`` as a localized (via
T020's message plumbing) blocked/limit condition, exactly like an
:func:`recovery.token_budget.plan_budget` ``"overflow"`` decision -- never
loops back into another compaction attempt on its own.
"""
from __future__ import annotations

from datetime import datetime, timedelta, timezone

from ..portable.identity import new_id

MAX_COMPACTIONS_PER_WINDOW = 5
WINDOW_SECONDS = 15 * 60
MIN_RECOVERY_RATIO = 0.15
MIN_CONSECUTIVE_LOW_RECOVERY = 3
# ``before_basis`` of an attempt whose pre-compaction context size no
# source reported; its stored ratio is a placeholder, never a measurement.
UNMEASURED_BASIS = "unmeasured"


def _attempt_time(store) -> str:
    """The current attempt's timestamp, also stored as its ``created_at``."""
    return store._now()


def _parse_time(value) -> datetime | None:
    try:
        moment = datetime.fromisoformat(value)
    except (TypeError, ValueError):
        return None
    return moment if moment.tzinfo is not None else moment.replace(tzinfo=timezone.utc)


def _session_history(db, engine_session_ref: str) -> list[dict]:
    """Every recorded attempt of this session, newest first. Row ids are
    random, so chronology comes from ``created_at`` (insertion order breaks
    ties)."""
    rows = db.execute(
        "SELECT created_at, before_basis, recovered_ratio FROM compaction_recovery_events "
        "WHERE engine_session_ref=? ORDER BY created_at DESC, rowid DESC",
        (engine_session_ref,),
    ).fetchall()
    return [dict(row) for row in rows]


def _attempts_in_window(history: list[dict], now: datetime) -> int:
    window = timedelta(seconds=WINDOW_SECONDS)
    count = 0
    for event in history:
        moment = _parse_time(event["created_at"])
        if moment is not None and abs(now - moment) < window:
            count += 1
    return count


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
    measured = before_basis != UNMEASURED_BASIS
    recovered_tokens = max(before_tokens - after_tokens, 0) if measured else 0
    recovered_ratio = (recovered_tokens / before_tokens) if measured and before_tokens > 0 else 0.0

    event_id = new_id("cre_")
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            created_at = _attempt_time(store)
            now = _parse_time(created_at)
            if now is None:
                raise ValueError(f"compaction attempt timestamp is not ISO 8601: {created_at!r}")
            history = _session_history(db, engine_session_ref)
            status, reason, message = _decide(
                history, now, measured, recovered_ratio,
                mandatory_tokens_after, usable_window_tokens,
            )
            db.execute(
                "INSERT INTO compaction_recovery_events "
                "(id, task_id, engine_session_ref, generation_id, before_tokens, before_basis, "
                " after_tokens, after_basis, recovered_tokens, recovered_ratio, status, reason, created_at) "
                "VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
                (event_id, task_id, engine_session_ref, generation_id, before_tokens, before_basis,
                 after_tokens, after_basis, recovered_tokens, recovered_ratio, status, reason, created_at),
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


def _decide(
    history: list[dict], now: datetime, measured: bool, recovered_ratio: float,
    mandatory_tokens_after: int, usable_window_tokens: int | None,
) -> tuple[str, str | None, str | None]:
    status, reason, message = "ok", None, None
    in_window = _attempts_in_window(history, now)
    if in_window >= MAX_COMPACTIONS_PER_WINDOW:
        status = "blocked"
        reason = "compaction_frequency_exceeded"
        message = (
            f"{in_window} compactions already recorded for this session within the last "
            f"{WINDOW_SECONDS // 60} minutes (limit {MAX_COMPACTIONS_PER_WINDOW}); stopping rather "
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
    elif measured:
        low_recovery_streak = [recovered_ratio < MIN_RECOVERY_RATIO]
        measured_recent = [event for event in history if event["before_basis"] != UNMEASURED_BASIS]
        for event in measured_recent[: MIN_CONSECUTIVE_LOW_RECOVERY - 1]:
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
    return status, reason, message
