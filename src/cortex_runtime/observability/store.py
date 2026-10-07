"""T029 additive optional schema group: ``observability_events``/
``usage_records``. ``user_version`` stays 11 -- composed into ``store.py``'s
per-project SQLite database the same way ``routing.store.ROUTING_SCHEMA``
is (see that module's docstring for the general pattern this follows).

``observability_events`` -- one immutable row per traceable execution/
recovery occurrence (a duplicate publication attempt, a schema/validation
rejection, ...). Carries every runtime identity the occurrence actually
has (task/assignment/attempt/engine-session/context-generation/config-
revision/connection/route/source-event -- each nullable, since not every
occurrence has every identity) plus a mandatory ``basis`` classification
(never coerced to ``observed`` or ``unknown`` by default -- every append
site chooses explicitly; see :mod:`observability.events`). This does not
replace ``delegation.events.append_event``'s ``task_events`` (the T016
coordinator-continuation log): it is a separate, append-only diagnostic
trail read by :mod:`observability.metrics`, not consumed by any
continuation-building logic.

``usage_records`` -- one immutable row per completed engine ``prompt()``
call (see ``delegation.live_adapters.LiveEngineAdapters.prompt``), whether
it was an initial attempt, a retry/fallback hop, a recovery-capsule
delivery, or a coordinator continuation: every call gets its own row, so
retries are never collapsed into (or dropped in favor of) only the final
successful attempt. ``basis`` is ``engine_reported`` when the engine
emitted at least one ``usage_update`` event during the call, ``unknown``
when no usage was ever reported. A post-result release is identified by
``failed=0`` and ``stop_reason='released_after_result'`` using existing
columns, not a new basis value (existing stores retain their CHECK constraints).
Route/connection/config-revision are not duplicated here -- read access joins ``attempt_route_audit`` on
``attempt_id`` (see :mod:`observability.metrics`).
"""
from __future__ import annotations

BASIS_VALUES = ("observed", "engine_reported", "derived", "estimated", "unknown")

OBSERVABILITY_SCHEMA = '''
CREATE TABLE IF NOT EXISTS observability_events (
 id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 category TEXT NOT NULL,
 outcome TEXT NOT NULL,
 basis TEXT NOT NULL CHECK(basis IN ('observed','engine_reported','derived','estimated','unknown')),
 assignment_ref TEXT, attempt_id TEXT, engine_session_ref TEXT,
 context_generation_id TEXT, config_revision_id TEXT, connection_id TEXT,
 route_engine TEXT, source_event_ref TEXT,
 detail_json TEXT NOT NULL DEFAULT '{}',
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS observability_events_by_task ON observability_events(task_id, created_at);
CREATE INDEX IF NOT EXISTS observability_events_by_attempt ON observability_events(attempt_id);
CREATE TRIGGER IF NOT EXISTS observability_events_no_update BEFORE UPDATE ON observability_events
BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE TRIGGER IF NOT EXISTS observability_events_no_delete BEFORE DELETE ON observability_events
WHEN NOT EXISTS (
 SELECT 1 FROM pending_deletions d WHERE d.task_id=OLD.task_id
) BEGIN SELECT RAISE(ABORT, 'immutable'); END;

CREATE TABLE IF NOT EXISTS usage_records (
 id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 attempt_id TEXT, engine_session_ref TEXT NOT NULL,
 call_sequence INTEGER NOT NULL,
 basis TEXT NOT NULL CHECK(basis IN ('observed','engine_reported','derived','estimated','unknown')),
 tokens_json TEXT NOT NULL DEFAULT '{}',
 stop_reason TEXT,
 failed INTEGER NOT NULL DEFAULT 0 CHECK(failed IN (0,1)),
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS usage_records_by_task ON usage_records(task_id, created_at);
CREATE INDEX IF NOT EXISTS usage_records_by_attempt ON usage_records(attempt_id, call_sequence);
CREATE TRIGGER IF NOT EXISTS usage_records_no_update BEFORE UPDATE ON usage_records
BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE TRIGGER IF NOT EXISTS usage_records_no_delete BEFORE DELETE ON usage_records
WHEN NOT EXISTS (
 SELECT 1 FROM pending_deletions d WHERE d.task_id=OLD.task_id
) BEGIN SELECT RAISE(ABORT, 'immutable'); END;
'''

OBSERVABILITY_TABLES = frozenset({"observability_events", "usage_records"})
