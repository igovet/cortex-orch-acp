"""T024: the ``large_outputs``/``compaction_recovery_events`` optional
schema group.

Composed into ``store.py``'s per-project SQLite database the same way
``delegation.generations_store.CONTEXT_GENERATIONS_SCHEMA`` is (an
additive optional group on top of the existing v11 schema; no
``user_version`` bump needed -- kept in its own module, separate from any
other in-flight task's schema additions, to minimize merge conflicts).

``large_outputs`` -- one row per oversized tool/terminal output persisted
to an owner-private file under the task's storage directory (see
``recovery.large_output_store``): ``owner_scope`` is either ``coordinator``
or a specific ``assignment_ref`` (never both/ambiguous -- a worker's own
oversized output is never readable through another assignment's scope).
``byte_count``/``truncated``/``exit_code``/``signal``/``failure`` are
captured once, at persistence time, from the exact same fields the
in-memory bridge already tracks (``engine.terminal_bridge.LocalTerminal
.output()``'s ``truncated``/``exitStatus``) -- persisting to a file must
never lose any of them.

``compaction_recovery_events`` -- one row per evaluated compaction attempt
for a session (see ``recovery.compaction_guard``): records the *actual*
before/after token estimate (or exact usage, when available) and whether
this attempt was allowed or blocked, so a caller can look back over a
bounded window and detect "N consecutive compactions recovered < X%" or an
immediate refill without re-deriving history from context_generations
sequence numbers alone.
"""
from __future__ import annotations

LARGE_OUTPUT_SCHEMA = '''
CREATE TABLE IF NOT EXISTS large_outputs (
 id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 owner_scope TEXT NOT NULL,
 source_kind TEXT NOT NULL CHECK(source_kind IN ('terminal','tool_result')),
 source_ref TEXT,
 filename TEXT NOT NULL,
 byte_count INTEGER NOT NULL,
 truncated INTEGER NOT NULL CHECK(truncated IN (0,1)),
 exit_code INTEGER,
 signal TEXT,
 failure TEXT,
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS large_outputs_by_task ON large_outputs(task_id);
CREATE INDEX IF NOT EXISTS large_outputs_by_owner ON large_outputs(task_id, owner_scope);

CREATE TABLE IF NOT EXISTS compaction_recovery_events (
 id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 engine_session_ref TEXT NOT NULL,
 generation_id TEXT REFERENCES context_generations(id),
 before_tokens INTEGER NOT NULL,
 before_basis TEXT NOT NULL,
 after_tokens INTEGER NOT NULL,
 after_basis TEXT NOT NULL,
 recovered_tokens INTEGER NOT NULL,
 recovered_ratio REAL NOT NULL,
 status TEXT NOT NULL CHECK(status IN ('ok','blocked')),
 reason TEXT,
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS compaction_recovery_events_by_session
 ON compaction_recovery_events(engine_session_ref, id);
CREATE INDEX IF NOT EXISTS compaction_recovery_events_by_task ON compaction_recovery_events(task_id);
'''

BUDGET_TABLES = frozenset({"large_outputs", "compaction_recovery_events"})
