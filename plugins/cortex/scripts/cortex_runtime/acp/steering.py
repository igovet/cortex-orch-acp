"""T020: steering revision counter, urgent-stop bookkeeping, and language
selection helpers shared by ``acp.orchestrating_runtime`` and ``acp.agent``.

See docs/acp/steering-and-questions.md for the full design (cancel vs stop
vs steering vs new task vs answers, delivery boundaries, question
lifecycle, language policy, noise policy).
"""
from __future__ import annotations

from datetime import datetime, timezone

from ..delegation import events as delegation_events
from ..portable.identity import new_id

# T020 (lead follow-up, closing a real race): the per-task language
# override lives in its own additive table, deliberately independent of
# ``task_orchestration`` (owned exclusively by
# ``acp.orchestrating_runtime.OrchestratingRuntime._ensure_coordinator``'s
# provisioning lifecycle -- inserting into that table from here would race
# its own "no row yet" vs "already provisioned" branching and could corrupt
# coordinator provisioning). A row here can be written the instant a task
# exists (right after ``bootstrap_task``/``append_source_event``), exactly
# like ``configuration_revisions`` already does for model/thought_level --
# never gated on whether the coordinator's own first turn has run yet.
TASK_LANGUAGE_SCHEMA = '''
CREATE TABLE IF NOT EXISTS task_language (
 task_id TEXT PRIMARY KEY REFERENCES tasks(id),
 language TEXT
);
'''

# T020 (fresh-review fix, HIGH): the urgent-stop marker independently
# additive, exactly like TASK_LANGUAGE_SCHEMA above and for the identical
# reason -- ``task_orchestration`` is created *lazily*, only by
# ``OrchestratingRuntime._ensure_coordinator``'s own first-ever-provisioning
# branch. The previous design stored ``stopped_at`` as a
# ``task_orchestration`` column: a ``/cortex-stop`` issued before that first
# provisioning ever ran updated zero rows (no row existed yet), returned
# ``True`` unconditionally (rowcount never checked), and the stop was then
# silently lost the moment ``_ensure_coordinator`` later inserted a fresh
# row with no ``stopped_at`` at all -- reproduced: stop confirmed to the
# user, then the coordinator provisioned and ran anyway. This table is
# writable/readable the instant a task exists (right after
# ``bootstrap_task``), entirely independent of whether the coordinator has
# ever been provisioned.
TASK_STOPPED_SCHEMA = '''
CREATE TABLE IF NOT EXISTS task_stopped (
 task_id TEXT PRIMARY KEY REFERENCES tasks(id),
 stopped_at TEXT NOT NULL
);
'''


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def is_stopped(store, task_id: str) -> bool:
    with store.connection() as db:
        row = db.execute("SELECT 1 FROM task_stopped WHERE task_id=?", (task_id,)).fetchone()
    return row is not None


def task_exists(store, task_id: str) -> bool:
    with store.connection() as db:
        row = db.execute("SELECT 1 FROM tasks WHERE id=?", (task_id,)).fetchone()
    return row is not None


def orchestration_exists(store, task_id: str) -> bool:
    with store.connection() as db:
        row = db.execute("SELECT 1 FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
    return row is not None


def get_language_override(store, task_id: str) -> str | None:
    with store.connection() as db:
        row = db.execute("SELECT language FROM task_language WHERE task_id=?", (task_id,)).fetchone()
    return row["language"] if row is not None else None


def set_language_override(store, task_id: str, language: str | None) -> None:
    """Upsert -- writable the instant a task exists, independent of
    whether ``task_orchestration`` has been provisioned yet (see the
    module-level ``TASK_LANGUAGE_SCHEMA`` docstring)."""
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "INSERT INTO task_language (task_id, language) VALUES (?,?) "
                "ON CONFLICT(task_id) DO UPDATE SET language=excluded.language",
                (task_id, language),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise


def recent_user_direct_texts(store, task_id: str, limit: int = 20):
    """The most recent ``user_message`` event texts for ``task_id``, in
    delivery order (oldest first) -- used only as the ``auto`` language
    heuristic's input (see ``acp.messages.select_language``); never a
    worker report or coordinator-authored text."""
    import json

    with store.connection() as db:
        rows = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='user_message' "
            "ORDER BY id DESC LIMIT ?",
            (task_id, limit),
        ).fetchall()
    texts = []
    for row in reversed(rows):
        payload = json.loads(row["payload_json"])
        content = payload.get("content")
        if isinstance(content, str):
            texts.append(content)
    return texts


def bump_requirement_revision(store, task_id: str) -> int:
    """Atomically increment and return ``task_orchestration.requirement_revision``."""
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            row = db.execute("SELECT requirement_revision FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
            revision = (row["requirement_revision"] if row is not None else 0) + 1
            db.execute("UPDATE task_orchestration SET requirement_revision=? WHERE task_id=?", (revision, task_id))
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return revision


def record_steering(store, task_id: str, origin: str, event_id: str, text: str) -> int:
    """Record an ordinary in-active-task prompt as steering: bumps and
    returns the task's ``requirement_revision``, and appends a durable
    ``progress``-typed event carrying ``kind="user_steering"`` (see the
    module docstring on why this reuses the existing, CHECK-constrained
    ``progress`` event_type rather than adding a new CHECK value: SQLite
    cannot additively widen a CHECK constraint on an existing table without
    a full rebuild, which is out of this task's minimal-migration scope --
    see docs/acp/steering-and-questions.md). This event is *informative*
    only (see ``delegation.events.MEANINGFUL_EVENT_TYPES``); the
    already-recorded ``user_message`` row for the very same prompt is what
    actually gates/delivers the continuation -- this row exists purely so
    the requirement_revision boundary is durably visible in the event log,
    independent of the coalesced text."""
    revision = bump_requirement_revision(store, task_id)
    delegation_events.append_event(
        store, task_id, "progress", f"user_steering:{origin}:{event_id}",
        dict(kind="user_steering", requirement_revision=revision, text=text), origin="runtime",
    )
    return revision


def _undelivered_steering_ids_tx(db, task_id: str) -> list[int]:
    """Every still-``superseded=0``, still-undelivered (``delivered_at IS
    NULL``) ``user_message`` row, plus every ``progress``/
    ``kind=user_steering`` bookkeeping row, for ``task_id`` -- regardless of
    ``consumed_by`` (see :func:`mark_stopped`'s docstring on why
    ``consumed_by`` alone is the wrong test: an in-flight continuation can
    have already *claimed* a batch -- ``consumed_by`` set -- without it
    ever having actually reached ``engine.prompt``/``mark_delivered``)."""
    import json

    rows = db.execute(
        "SELECT id, event_type, payload_json FROM task_events WHERE task_id=? "
        "AND delivered_at IS NULL AND superseded=0 AND event_type IN ('user_message', 'progress')",
        (task_id,),
    ).fetchall()
    ids = []
    for row in rows:
        if row["event_type"] == "user_message":
            ids.append(row["id"])
        else:
            payload = json.loads(row["payload_json"])
            if payload.get("kind") == "user_steering":
                ids.append(row["id"])
    return ids


def mark_stopped(store, task_id: str) -> bool:
    """Insert into ``task_stopped`` iff not already stopped (and iff
    ``task_id`` actually exists -- see below), and -- in that exact same
    transaction -- supersede every still-undelivered steering/user_message
    row for this task (see :func:`_undelivered_steering_ids_tx`). Returns
    ``True`` iff this call performed the transition (idempotent -- a
    second ``/cortex-stop`` is a no-op, never a second cancellation wave).

    T020 fresh-review fix (HIGH, reproduced): the previous version wrote
    ``stopped_at`` as a plain ``UPDATE ... task_orchestration`` column
    *without checking rowcount* -- ``task_orchestration`` is created
    lazily, only by ``OrchestratingRuntime._ensure_coordinator``'s own
    first-provisioning branch, so a stop issued before that first
    provisioning ever ran affected zero rows, returned ``True``
    unconditionally, confirmed "stopped" to the user, and was then
    silently lost the instant ``_ensure_coordinator`` inserted a fresh row
    with no ``stopped_at`` at all. Storing the marker in its own additive
    ``task_stopped`` table (see ``TASK_STOPPED_SCHEMA``), entirely
    independent of ``task_orchestration``'s provisioning lifecycle, removes
    the race outright: this table exists (and is checked by
    ``is_stopped``/``OrchestratingRuntime._run``, which gates
    ``_ensure_coordinator`` -- see that method's own stopped-task check,
    run *before* any provisioning) from the instant the task itself exists.

    Raises :class:`LookupError` for a ``task_id`` that does not exist at
    all (never a false "stopped" confirmation for a task that was never
    real -- see ``OrchestratingRuntime.stop_task``, which surfaces this as
    an explicit, non-silent error to the caller).
    """
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            if db.execute("SELECT 1 FROM tasks WHERE id=?", (task_id,)).fetchone() is None:
                raise LookupError(f"unknown task_id {task_id!r}")
            row = db.execute("SELECT 1 FROM task_stopped WHERE task_id=?", (task_id,)).fetchone()
            if row is not None:
                db.commit()
                return False
            db.execute("INSERT INTO task_stopped (task_id, stopped_at) VALUES (?,?)", (task_id, _now()))
            ids = _undelivered_steering_ids_tx(db, task_id)
            if ids:
                db.executemany("UPDATE task_events SET superseded=1 WHERE id=?", [(event_id,) for event_id in ids])
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return True


def supersede_undelivered_steering(store, task_id: str) -> int:
    """Defense-in-depth standalone call (the primary guarantee is
    :func:`mark_stopped`'s own atomic step -- this is redundant with it by
    design, safe to call any number of times, and a no-op once nothing
    undelivered remains). Same ``delivered_at IS NULL``-keyed criterion,
    regardless of ``consumed_by``. Returns the count affected."""
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            ids = _undelivered_steering_ids_tx(db, task_id)
            if ids:
                db.executemany("UPDATE task_events SET superseded=1 WHERE id=?", [(event_id,) for event_id in ids])
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return len(ids)


def record_post_stop_message(store, task_id: str, origin: str, event_id: str, text: str) -> None:
    """A prompt/steering/answer that arrives *after* ``/cortex-stop``:
    durably recorded (never silently dropped -- an operator/reviewer can
    still see it happened) but marked already-superseded so it can never be
    claimed/delivered and never restarts work."""
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            event = delegation_events.append_event_tx(
                db, task_id, "progress", f"post_stop:{origin}:{event_id}",
                dict(kind="post_stop_message", text=text), origin="runtime",
            )
            db.execute("UPDATE task_events SET superseded=1 WHERE id=?", (event.id,))
            db.commit()
        except BaseException:
            db.rollback()
            raise
