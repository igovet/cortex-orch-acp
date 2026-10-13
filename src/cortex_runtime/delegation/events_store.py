"""``task_events``/``task_orchestration``: the T016 optional schema group.

Composed into ``store.py``'s per-project SQLite database exactly like
``delegation.store.DELEGATION_SCHEMA`` (see that module and ``split.py``'s
comments on the existing optional groups): a pre-T016 v11 store has neither
table; a store opened/created by the current ``store.py`` always has both.
No ``user_version`` bump is needed -- this is an additive optional group on
top of the existing v11 schema, the same pattern ``DELEGATION_SCHEMA`` uses.

``task_events`` is the durable, append-only semantic event log the T016
``OrchestratingRuntime`` drives continuation from (see
``delegation.events``): every row is one meaningful occurrence (a
delegation request, a worker result/question, a coordinator decision, an
inbound user message, ...), never raw transport chatter. ``dedup_key`` is a
caller-chosen identity (a ``request_key``, ``report_ref``, or origin event
id) -- rows are deduplicated by identity, never by comparing payload text.

``task_orchestration`` holds exactly one row per task: its current
coordination state and the coordinator's own provisioning reference (the
``delegation_provisioning.idempotency_key`` used to open the coordinator's
own engine session, recorded the same way a delegated assignment's
``assignment_ref`` is -- see ``acp.orchestrating_runtime``).

T017 additive columns (see :func:`ensure_task_lifecycle_columns`):

- ``task_events.delivered_at`` -- set by ``mark_delivered`` after
  ``engine.prompt`` for the continuation that claimed a row actually
  returns, so a crash between ``claim_unconsumed`` and delivery is
  reconcilable on the next runtime start (``consumed_by`` set but
  ``delivered_at`` still NULL -> release the claim).
- ``task_orchestration.wait_reason`` / ``wait_detail_json`` /
  ``no_progress_count`` -- durable waiting state (see
  ``acp.orchestrating_runtime``): what a blocked/limited/refused task is
  waiting for, a JSON detail blob (assignment refs, question, explanation),
  and a durable no-progress counter reset whenever a new user prompt
  arrives.

These are additive on top of the table definitions below so both a fresh
store (columns present from ``CREATE TABLE``) and a pre-T017 store (columns
added via ``ALTER TABLE ... ADD COLUMN`` the first time it is opened) end up
with the same shape; no ``user_version`` bump is needed for either change.
"""
from __future__ import annotations

TASK_EVENTS_SCHEMA = '''
CREATE TABLE IF NOT EXISTS task_events (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 event_type TEXT NOT NULL CHECK(event_type IN (
   'delegation_requested','assignment_ready','assignment_failed',
   'assignment_cancel_requested','assignment_cancelled',
   'worker_result','worker_question','coordinator_clarification',
   'coordinator_decision','user_message','progress'
 )),
 assignment_ref TEXT, dedup_key TEXT NOT NULL,
 payload_json TEXT NOT NULL, origin TEXT NOT NULL,
 created_at TEXT NOT NULL, consumed_by TEXT,
 superseded INTEGER NOT NULL DEFAULT 0,
 delivered_at TEXT,
 UNIQUE(task_id, dedup_key)
);
CREATE INDEX IF NOT EXISTS task_events_by_task ON task_events(task_id, id);
CREATE INDEX IF NOT EXISTS task_events_by_assignment ON task_events(task_id, assignment_ref);
CREATE INDEX IF NOT EXISTS task_events_by_type ON task_events(task_id, event_type, id);
CREATE INDEX IF NOT EXISTS task_events_unconsumed ON task_events(task_id, id) WHERE consumed_by IS NULL AND superseded=0;
CREATE TABLE IF NOT EXISTS task_orchestration (
 task_id TEXT PRIMARY KEY REFERENCES tasks(id),
 state TEXT NOT NULL,
 coordinator_attempt_ref TEXT,
 updated_at TEXT NOT NULL,
 wait_reason TEXT,
 wait_detail_json TEXT,
 no_progress_count INTEGER NOT NULL DEFAULT 0,
 requirement_revision INTEGER NOT NULL DEFAULT 0,
 stopped_at TEXT,
 language TEXT
);
'''


def ensure_task_lifecycle_columns(db) -> None:
    """Idempotently add the T017/T020 additive columns to an already-open
    connection whose ``task_events``/``task_orchestration`` tables predate
    them (a store created by d301299/T016 or earlier). Safe to call on
    every :class:`store.Store` open; a no-op once the columns exist.

    T020 additions: ``task_orchestration.requirement_revision`` (a
    monotonically increasing counter bumped once per steering prompt --
    see ``acp.steering``). ``stopped_at`` and ``language`` were an initial
    T020 attempt at storing the urgent-stop marker and a per-task language
    override as columns here; both were superseded before release by
    independent tables (``acp.steering.TASK_STOPPED_SCHEMA``/
    ``task_stopped`` and ``TASK_LANGUAGE_SCHEMA``/``task_language``,
    respectively), never gated on this table's own lazy
    provisioning-lifecycle ownership -- see that module's docstrings
    (``mark_stopped`` in particular documents the reproduced bug a
    column-on-``task_orchestration`` design caused: a stop issued before
    the coordinator's first-ever provisioning updated zero rows and was
    silently lost). Both columns are left in place, unused, rather than
    attempting a SQLite column drop.
    """
    events_columns = {row[1] for row in db.execute("PRAGMA table_info(task_events)").fetchall()}
    orchestration_columns = {row[1] for row in db.execute("PRAGMA table_info(task_orchestration)").fetchall()}
    statements = []
    if "delivered_at" not in events_columns:
        statements.append("ALTER TABLE task_events ADD COLUMN delivered_at TEXT")
    if "wait_reason" not in orchestration_columns:
        statements.append("ALTER TABLE task_orchestration ADD COLUMN wait_reason TEXT")
    if "wait_detail_json" not in orchestration_columns:
        statements.append("ALTER TABLE task_orchestration ADD COLUMN wait_detail_json TEXT")
    if "no_progress_count" not in orchestration_columns:
        statements.append("ALTER TABLE task_orchestration ADD COLUMN no_progress_count INTEGER NOT NULL DEFAULT 0")
    if "requirement_revision" not in orchestration_columns:
        statements.append("ALTER TABLE task_orchestration ADD COLUMN requirement_revision INTEGER NOT NULL DEFAULT 0")
    if "stopped_at" not in orchestration_columns:
        statements.append("ALTER TABLE task_orchestration ADD COLUMN stopped_at TEXT")
    if "language" not in orchestration_columns:
        statements.append("ALTER TABLE task_orchestration ADD COLUMN language TEXT")
    if not statements:
        return
    db.execute("BEGIN IMMEDIATE")
    try:
        for statement in statements:
            db.execute(statement)
        db.commit()
    except BaseException:
        db.rollback()
        raise
