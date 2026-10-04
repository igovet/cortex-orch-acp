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
    """Queue one source-identified steer and its revision in one transaction.

    The already-captured user_message remains the delivery event. The
    progress row records the requirement boundary; the runtime event is
    applied only when the controller reaches a safe planning boundary.
    """
    from ..orchestration.policy import parse_denials
    from ..orchestration.state import event as runtime_event

    actor = f"user:{origin}:{event_id}"
    dedup_key = f"user_steering:{origin}:{event_id}"
    denials = parse_denials(text)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            row = db.execute(
                "SELECT requirement_revision FROM task_orchestration WHERE task_id=?", (task_id,),
            ).fetchone()
            current = row["requirement_revision"] if row else 0
            if db.execute(
                "SELECT 1 FROM task_events WHERE task_id=? AND dedup_key=?",
                (task_id, dedup_key),
            ).fetchone():
                db.commit()
                return current
            if db.execute(
                "SELECT 1 FROM task_stopped WHERE task_id=?", (task_id,),
            ).fetchone():
                db.commit()
                return current
            task = db.execute(
                "SELECT state FROM runtime_tasks WHERE task_id=?", (task_id,),
            ).fetchone()
            if task and task["state"] == "CANCELLED":
                db.commit()
                return current
            if task:
                runtime_event(db, task_id, "USER_STEERING",
                              dict(text=text, deny_write=denials), actor)
            revision = current + 1
            if row:
                db.execute(
                    "UPDATE task_orchestration SET requirement_revision=? WHERE task_id=?",
                    (revision, task_id),
                )
            delegation_events.append_event_tx(
                db, task_id, "progress", dedup_key,
                dict(kind="user_steering", requirement_revision=revision, text=text),
                origin="runtime",
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise
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


def mark_stopped(store, task_id: str, *, source: str = "cortex_stop_command", session_id: str | None = None,
                 question_id: str | None = None, source_origin: str | None = None,
                 source_event_id: str | None = None) -> bool:
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
    if source not in {"cortex_stop_command", "paseo_session_cancel", "cortex_finish_command", "user_answer_force_finish"}:
        raise ValueError("unknown task stop source")
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            if db.execute("SELECT 1 FROM tasks WHERE id=?", (task_id,)).fetchone() is None:
                raise LookupError(f"unknown task_id {task_id!r}")
            row = db.execute("SELECT 1 FROM task_stopped WHERE task_id=?", (task_id,)).fetchone()
            if row is not None:
                db.commit()
                return False
            if source == "user_answer_force_finish":
                from ..orchestration.questions import is_force_finish_answer
                answer = db.execute(
                    "SELECT status,answer_text,answer_source,text FROM pending_questions WHERE task_id=? AND id=?",
                    (task_id, question_id),
                ).fetchone()
                if (
                    not answer or answer["status"] != "answered"
                    or answer["answer_source"] not in {"text", "elicitation"}
                    or not is_force_finish_answer(answer["answer_text"], answer["text"])
                ):
                    raise ValueError("force finish requires an exact captured owner answer")
            elif source == "cortex_finish_command":
                source_row = db.execute(
                    "SELECT 1 FROM source_events WHERE task_id=? AND origin=? AND event_id=?",
                    (task_id, source_origin, source_event_id),
                ).fetchone()
                if source_row is None or not source_origin or not source_event_id:
                    raise ValueError("force finish requires a captured owner command")
            task = db.execute("SELECT state,revision FROM runtime_tasks WHERE task_id=?", (task_id,)).fetchone()
            if (source in {"cortex_finish_command", "user_answer_force_finish"}
                    and task and task["state"] in {"COMPLETED", "FAILED", "CANCELLED"}):
                db.commit()
                return False
            if source in {"cortex_finish_command", "user_answer_force_finish"} and task is None:
                from ..orchestration.state import event, now, transition
                first = db.execute(
                    "SELECT report_id FROM source_revisions WHERE task_id=? ORDER BY revision LIMIT 1",
                    (task_id,),
                ).fetchone()
                if first is None:
                    raise ValueError("force finish requires the original captured goal")
                goal = store._location(db, first["report_id"]).read_text(encoding="utf-8")
                stamp = now()
                db.execute(
                    "INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES(?,?,?,?,?)",
                    (task_id, "CREATED", goal, stamp, stamp),
                )
                event(db, task_id, "TASK_CREATED", dict(goal=goal))
                transition(db, task_id, "PLANNING")
                task = db.execute("SELECT state,revision FROM runtime_tasks WHERE task_id=?", (task_id,)).fetchone()
            db.execute("INSERT INTO task_stopped (task_id, stopped_at) VALUES (?,?)", (task_id, _now()))
            from ..orchestration.worker_waits import cancel_tx
            cancel_tx(db, task_id, 'task_stopped')
            # Commit provenance with the stop marker, before process teardown.
            # ACP identifies the client transport, not the human/UI action.
            delegation_events.append_event_tx(
                db, task_id, "progress", "task_stop_observed",
                dict(kind="task_stop_observed", source=source, session_id=session_id,
                     **({"question_id": question_id} if question_id else {})),
                origin="runtime",
            )
            if source in {"cortex_finish_command", "user_answer_force_finish"}:
                from ..orchestration.state import event
                pending = db.execute(
                    "SELECT node_id,status FROM runtime_nodes WHERE task_id=? AND revision=? AND status!='completed' ORDER BY node_id",
                    (task_id, task["revision"]),
                ).fetchall()
                event(db, task_id, "USER_FORCE_FINISHED", dict(
                    source=source, session_id=session_id, question_id=question_id,
                    source_origin=source_origin, source_event_id=source_event_id,
                    previous_state=task["state"], revision=task["revision"],
                    unfinished_nodes=[dict(node_id=r["node_id"], status=r["status"]) for r in pending],
                    review_verification_synthesis="not_certified_by_force_finish",
                ))
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
