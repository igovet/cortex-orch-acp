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
 UNIQUE(task_id, dedup_key)
);
CREATE INDEX IF NOT EXISTS task_events_by_task ON task_events(task_id, id);
CREATE INDEX IF NOT EXISTS task_events_by_assignment ON task_events(task_id, assignment_ref);
CREATE TABLE IF NOT EXISTS task_orchestration (
 task_id TEXT PRIMARY KEY REFERENCES tasks(id),
 state TEXT NOT NULL,
 coordinator_attempt_ref TEXT,
 updated_at TEXT NOT NULL
);
'''
