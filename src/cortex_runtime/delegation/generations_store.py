"""``context_generations``/``capsule_deliveries``/``compaction_owners``/
``compaction_events_seen``: the T022 optional schema group.

Composed into ``store.py``'s per-project SQLite database the same way
``delegation.capacity_store.CAPACITY_SCHEMA`` is (an additive optional
group on top of the existing v11 schema; no ``user_version`` bump needed --
see that module's docstring and ``split.py``'s comments on the existing
optional groups).

``context_generations`` -- one row per distinct context "epoch" Cortex has
observed or itself created for one ``engine_session_ref`` (see
``delegation.generations``): ``sequence`` is 1-based and strictly
increasing per session, ``cause`` records why this generation began
(``session_open`` -- the session's very first generation;
``observed_compaction``/``observed_reset`` -- an adapter-reported signal;
``session_replacement`` -- a new engine session opened for an attempt that
already had one; ``controlled_boundary_refresh`` -- reserved for a future
adapter-verifiable, Cortex-issued compaction request -- see
``delegation.generations`` module docstring for why no pinned adapter
currently reaches this cause), and ``evidence_source``/``evidence_id``
record exactly what was observed (never inferred from model text).

``compaction_owners`` -- exactly one row per ``engine_session_ref``,
written once (``INSERT OR IGNORE``, enforced by the primary key) the first
time that session is provisioned: which single mechanism owns compaction
for it (``engine_adapter`` for an automatic-tier adapter, ``cortex_controlled``
for controlled-boundary, ``none`` for unqualified) -- Cortex itself never
runs a second, competing compaction controller for the same session.

``compaction_events_seen`` -- append-only dedup ledger: one row per
distinct ``(engine_session_ref, evidence_id)`` the adapter has ever
reported (``INSERT OR IGNORE`` again enforces "duplicate triggers for the
same observed event are deduplicated" -- a re-delivered/duplicated
``tool_call``/``tool_call_update`` for the same ``toolCallId`` can never
open a second generation).

``capsule_deliveries`` -- the recovery-delivery ledger (see
``delegation.deliveries``): one row per distinct
``(recipient, context_generation_id, capsule_revision)`` -- a second
compaction with unchanged task state produces a new
``context_generation_id`` (a fresh row, permitting redelivery of the same
logical snapshot) while an identical retry against the *same* generation
is idempotent (``INSERT OR IGNORE``/lookup, never a duplicate row).
"""
from __future__ import annotations

CONTEXT_GENERATIONS_SCHEMA = '''
CREATE TABLE IF NOT EXISTS context_generations (
 id TEXT PRIMARY KEY,
 engine_session_ref TEXT NOT NULL,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 assignment_ref TEXT,
 recipient TEXT NOT NULL CHECK(recipient IN ('coordinator','worker')),
 sequence INTEGER NOT NULL,
 cause TEXT NOT NULL CHECK(cause IN (
   'session_open','observed_compaction','observed_reset',
   'session_replacement','controlled_boundary_refresh'
 )),
 evidence_source TEXT NOT NULL,
 evidence_id TEXT,
 created_at TEXT NOT NULL,
 UNIQUE(engine_session_ref, sequence)
);
CREATE INDEX IF NOT EXISTS context_generations_by_session ON context_generations(engine_session_ref, sequence);
CREATE INDEX IF NOT EXISTS context_generations_by_task ON context_generations(task_id);

CREATE TABLE IF NOT EXISTS compaction_owners (
 engine_session_ref TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 owner TEXT NOT NULL CHECK(owner IN ('engine_adapter','cortex_controlled','none')),
 tier TEXT NOT NULL CHECK(tier IN ('automatic','controlled_boundary','unqualified')),
 engine TEXT NOT NULL,
 engine_version TEXT NOT NULL,
 limitation TEXT,
 set_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS compaction_owners_by_task ON compaction_owners(task_id);

CREATE TABLE IF NOT EXISTS compaction_events_seen (
 engine_session_ref TEXT NOT NULL,
 evidence_id TEXT NOT NULL,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 phase TEXT NOT NULL,
 observed_at TEXT NOT NULL,
 generation_id TEXT,
 PRIMARY KEY (engine_session_ref, evidence_id)
);
CREATE INDEX IF NOT EXISTS compaction_events_seen_by_task ON compaction_events_seen(task_id);

CREATE TABLE IF NOT EXISTS capsule_deliveries (
 id TEXT PRIMARY KEY,
 recipient TEXT NOT NULL CHECK(recipient IN ('coordinator','worker')),
 context_generation_id TEXT NOT NULL REFERENCES context_generations(id),
 capsule_revision TEXT NOT NULL,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 assignment_ref TEXT,
 engine_session_ref TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN (
   'built','transmitted','delivery_confirmed','continuation_observed','blocked','superseded'
 )),
 evidence_source TEXT NOT NULL,
 uncertainty TEXT,
 reason TEXT,
 retry_count INTEGER NOT NULL DEFAULT 0,
 created_at TEXT NOT NULL,
 updated_at TEXT NOT NULL,
 UNIQUE(recipient, context_generation_id, capsule_revision)
);
CREATE INDEX IF NOT EXISTS capsule_deliveries_by_task ON capsule_deliveries(task_id);
CREATE INDEX IF NOT EXISTS capsule_deliveries_by_session ON capsule_deliveries(engine_session_ref, id);
'''
