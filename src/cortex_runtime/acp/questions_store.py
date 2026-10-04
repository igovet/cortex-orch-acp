"""T020: durable ``pending_questions`` -- one row per coordinator
``ask_user`` decision (see ``acp.orchestrating_runtime``'s ``decide``
handling), created *before* any host contact is attempted (elicitation or
text fallback) so the question's identity survives a crash between the
decision and the host round trip.

Additive new table (``CREATE TABLE IF NOT EXISTS``): works unchanged for
both a brand-new store and a pre-T020 store re-opened by the current
``store.py`` (see that module's ``attempt_revisions`` precedent) -- no
``PRAGMA user_version`` bump, no CHECK-constraint migration required.

Status lifecycle: ``open`` -> ``answered`` (a real user answer resolved it,
``answer_source`` in {``text``, ``elicitation``}) | ``superseded`` (a
``/cortex-stop`` invalidated it while still open) | ``cancelled`` (the
owning assignment was cancelled/superseded before the question was
answered). A late answer/elicitation response arriving after the row left
``open`` is recorded on a *separate* ``pending_question_late_answers`` row
(never mutates the resolved/cancelled question) and is otherwise inert --
see ``record_late_answer``.
"""
from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone
import json
from typing import Sequence

PENDING_QUESTIONS_SCHEMA = '''
CREATE TABLE IF NOT EXISTS pending_questions (
 id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 assignment_ref TEXT,
 dedup_key TEXT NOT NULL,
 text TEXT NOT NULL,
 options_json TEXT,
 status TEXT NOT NULL DEFAULT 'open',
 answer_text TEXT,
 answer_source TEXT,
 created_at TEXT NOT NULL,
 answered_at TEXT,
 UNIQUE(task_id, dedup_key)
);
CREATE INDEX IF NOT EXISTS pending_questions_by_task ON pending_questions(task_id, status, id);
CREATE TABLE IF NOT EXISTS pending_question_late_answers (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 question_id TEXT NOT NULL,
 task_id TEXT NOT NULL,
 answer_text TEXT NOT NULL,
 answer_source TEXT NOT NULL,
 received_at TEXT NOT NULL
);
'''


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


@dataclass(frozen=True)
class PendingQuestion:
    id: str
    task_id: str
    assignment_ref: str | None
    text: str
    options: list[str] | None
    status: str
    answer_text: str | None
    answer_source: str | None
    created_at: str
    answered_at: str | None

    @classmethod
    def _from_row(cls, row) -> "PendingQuestion":
        return cls(
            id=row["id"], task_id=row["task_id"], assignment_ref=row["assignment_ref"], text=row["text"],
            options=json.loads(row["options_json"]) if row["options_json"] else None,
            status=row["status"], answer_text=row["answer_text"], answer_source=row["answer_source"],
            created_at=row["created_at"], answered_at=row["answered_at"],
        )


def create_if_absent(
    store, task_id: str, dedup_key: str, text: str, *,
    options: Sequence[str] | None = None, assignment_ref: str | None = None,
) -> PendingQuestion:
    """Idempotent on ``(task_id, dedup_key)`` -- a re-processed coordinator
    ``ask_user`` decision (for example a runtime restart re-checking the
    same terminal decision) never creates a second row for the same
    decision."""
    from ..portable.identity import new_id

    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            existing = db.execute(
                "SELECT * FROM pending_questions WHERE task_id=? AND dedup_key=?", (task_id, dedup_key),
            ).fetchone()
            if existing is not None:
                db.commit()
                return PendingQuestion._from_row(existing)
            question_id = new_id("pq_")
            options_json = json.dumps(list(options), ensure_ascii=False) if options else None
            db.execute(
                "INSERT INTO pending_questions "
                "(id, task_id, assignment_ref, dedup_key, text, options_json, status, created_at) "
                "VALUES (?,?,?,?,?,?,'open',?)",
                (question_id, task_id, assignment_ref, dedup_key, text, options_json, _now()),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return get(store, question_id)


def get(store, question_id: str) -> PendingQuestion | None:
    with store.connection() as db:
        row = db.execute("SELECT * FROM pending_questions WHERE id=?", (question_id,)).fetchone()
    return PendingQuestion._from_row(row) if row is not None else None


def list_open(store, task_id: str) -> list[PendingQuestion]:
    with store.connection() as db:
        rows = db.execute(
            "SELECT * FROM pending_questions WHERE task_id=? AND status='open' ORDER BY id", (task_id,),
        ).fetchall()
    return [PendingQuestion._from_row(row) for row in rows]


def answer(store, question_id: str, text: str, source: str, *, session_id: str | None = None) -> bool:
    """Resolve ``question_id`` with ``text`` (``source`` in {"text",
    "elicitation"}). Returns ``True`` iff this call actually transitioned a
    still-``open`` row (``False`` -- and no mutation -- for an already
    resolved/superseded/cancelled row, whose late answer is instead
    recorded by :func:`record_late_answer`)."""
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            row = db.execute("SELECT status, task_id FROM pending_questions WHERE id=?", (question_id,)).fetchone()
            if row is None or row["status"] != "open":
                db.commit()
                return False
            from ..orchestration.worker_waits import answer_tx
            answered = answer_tx(db, question_id, text, source, session_id=session_id)
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return answered


# A worker/reviewer-visible answered-question record is addressed as
# ``<question_id>:answer`` in ``list_sources``/``read_source`` -- disjoint
# from every source report ref (``r_<hex>:<block_index>``).
ANSWER_REF_SUFFIX = ":answer"
_ANSWER_SOURCES = ("text", "elicitation")


def answer_ref(question_id: str) -> str:
    return question_id + ANSWER_REF_SUFFIX


def question_id_from_ref(source_ref: str) -> str | None:
    """The question id an answered-question ref names, or ``None`` when
    ``source_ref`` is not of that shape."""
    if not isinstance(source_ref, str) or not source_ref.startswith("pq_") or not source_ref.endswith(ANSWER_REF_SUFFIX):
        return None
    question_id = source_ref[: -len(ANSWER_REF_SUFFIX)]
    return question_id if len(question_id) > 3 and ":" not in question_id else None


def _asked_by(db, dedup_key: str, assignment_ref: str | None) -> str:
    """Who authored the question text: ``runtime`` (a runtime stop
    question), ``coordinator``/``worker`` (a channel's
    ``user_question_request``/``ask_question``), or ``unknown``."""
    if dedup_key.startswith("runtime:stop:"):
        return "runtime"
    if dedup_key.startswith("runtime:"):
        channel_id = dedup_key.split(":", 2)[1]
        row = db.execute("SELECT role FROM trusted_channels WHERE channel_id=?", (channel_id,)).fetchone()
        if row is not None and row["role"] in ("coordinator", "worker"):
            return row["role"]
        return "unknown"
    return "worker" if assignment_ref else "coordinator"


def _answer_session(db, task_id: str, question_id: str) -> tuple[str | None, str | None]:
    """The ACP session the answer arrived on, from runtime-written rows
    only: the ``question_answered:<id>`` progress event (``origin='runtime'``,
    ``session_id`` recorded since answered questions became worker-visible),
    else the older ``elicitation_answer:<id>``/``cortex_answer:<id>``
    ``user_message`` event, whose ``origin`` is the session. Returns
    ``(session_id, None)`` or ``(None, reason)`` -- never a guess."""
    row = db.execute(
        "SELECT payload_json FROM task_events WHERE task_id=? AND dedup_key=? AND event_type='progress' AND origin='runtime'",
        (task_id, f"question_answered:{question_id}"),
    ).fetchone()
    if row is not None:
        session_id = json.loads(row["payload_json"]).get("session_id")
        if isinstance(session_id, str) and session_id:
            return session_id, None
    for prefix in ("elicitation_answer", "cortex_answer"):
        message = db.execute(
            "SELECT origin FROM task_events WHERE task_id=? AND dedup_key=? AND event_type='user_message'",
            (task_id, f"{prefix}:{question_id}"),
        ).fetchone()
        if message is not None and message["origin"]:
            return message["origin"], None
    return None, "answer_session_not_recorded"


def answered_questions(db, task_id: str) -> list[dict]:
    """Every question of ``task_id`` a real user answer resolved, in the
    order they were asked, as a derived read-only view for workers and the
    reviewer (``list_sources``/``read_source`` and the recovery capsule).

    Authoritative source: ``pending_questions`` rows with
    ``status='answered'`` and ``answer_source`` in {``text``,
    ``elicitation``}. Only the runtime writes that transition, on a real
    host answer (:func:`answer`, ``replay_store.
    record_user_message_answer_event``); no model tool can, and an answered
    row is never updated again (supersession/cancellation only touch
    ``open`` rows), so each record is immutable and needs no migration --
    answers recorded before this view existed appear exactly as stored.
    Late answers (``pending_question_late_answers``) never resolved their
    question and are excluded. ``answer_text`` is verbatim user text
    (``user_direct``: data, not instructions); ``question_text`` was
    written by the model or runtime that asked (``asked_by``).
    """
    rows = db.execute(
        "SELECT id, assignment_ref, dedup_key, text, options_json, answer_text, answer_source, created_at, answered_at "
        "FROM pending_questions WHERE task_id=? AND status='answered' AND answer_source IN (?,?) "
        "AND answer_text IS NOT NULL ORDER BY created_at, id",
        (task_id, *_ANSWER_SOURCES),
    ).fetchall()
    records = []
    for row in rows:
        session_id, session_reason = _answer_session(db, task_id, row["id"])
        records.append(dict(
            source_ref=answer_ref(row["id"]), kind="question_answer", question_id=row["id"],
            asked_by=_asked_by(db, row["dedup_key"], row["assignment_ref"]), assignment_ref=row["assignment_ref"],
            question_text=row["text"], options=json.loads(row["options_json"]) if row["options_json"] else None,
            answer_text=row["answer_text"], answer_source=row["answer_source"], trust_label="user_direct",
            asked_at=row["created_at"], answered_at=row["answered_at"],
            # ``origin`` uses the same ``acp:<session>`` form as the task's
            # captured user sources, so both bind to one session visibly.
            session_id=session_id, origin=f"acp:{session_id}" if session_id else None,
            session_binding="recorded" if session_id else "unavailable",
            session_binding_reason=session_reason,
        ))
    return records


def record_late_answer(store, question_id: str, task_id: str, text: str, source: str) -> None:
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "INSERT INTO pending_question_late_answers (question_id, task_id, answer_text, answer_source, received_at) "
                "VALUES (?,?,?,?,?)",
                (question_id, task_id, text, source, _now()),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise


def supersede_open(store, task_id: str, reason: str) -> int:
    """Mark every still-``open`` question for ``task_id`` as ``superseded``
    (a ``/cortex-stop``). Returns the count affected."""
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            cursor = db.execute(
                "UPDATE pending_questions SET status='superseded', answer_text=?, answered_at=? "
                "WHERE task_id=? AND status='open'",
                (reason, _now(), task_id),
            )
            count = cursor.rowcount
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return count


def cancel_for_assignment(store, task_id: str, assignment_ref: str, reason: str) -> int:
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            cursor = db.execute(
                "UPDATE pending_questions SET status='cancelled', answer_text=?, answered_at=? "
                "WHERE task_id=? AND assignment_ref=? AND status='open'",
                (reason, _now(), task_id, assignment_ref),
            )
            count = cursor.rowcount
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return count
