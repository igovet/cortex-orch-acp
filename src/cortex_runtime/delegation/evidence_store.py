"""``tool_result_observations``: T017's optional schema group for durable
engine-observed tool-call evidence (see ``delegation.evidence``).

A **separate new table** in its own optional group, deliberately *not* an
additive ``task_events.event_type`` CHECK value: the ``task_events`` table
already existed at d301299/T016 with a fixed CHECK constraint, and SQLite
cannot widen a CHECK via ``ALTER TABLE ADD COLUMN``/`ALTER TABLE ... ADD
CONSTRAINT` -- only a table-rebuild migration can, which is materially
riskier for an existing store than adding one more all-or-nothing optional
table. Composed the same way ``TASK_EVENTS_SCHEMA``/``CAPACITY_SCHEMA``
are: no ``user_version`` bump needed.

One row per observed tool-call status transition (an engine may report
``pending``/``in_progress``/``completed``/``failed`` for the same
``tool_call_ref`` over a turn) -- append-only, never updated in place, so
the full observation history for an attempt stays auditable. Only
``tool_call_ref``, a short engine-reported ``kind``, a bounded ``title``,
and ``status`` are stored -- never raw tool arguments/output/results.
"""
from __future__ import annotations

EVIDENCE_SCHEMA = '''
CREATE TABLE IF NOT EXISTS tool_result_observations (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 attempt_id TEXT NOT NULL,
 tool_call_ref TEXT NOT NULL,
 kind TEXT,
 title TEXT,
 status TEXT NOT NULL,
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS tool_result_observations_by_attempt ON tool_result_observations(attempt_id, tool_call_ref, id);
CREATE INDEX IF NOT EXISTS tool_result_observations_by_task ON tool_result_observations(task_id);
'''
