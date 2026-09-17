"""``delegation_provisioning``/``delegation_effects``: the T015 optional schema group.

Composed into ``store.py``'s per-project SQLite database exactly like
``catalog.store.CATALOG_SCHEMA`` and ``configuration.store.CONFIGURATION_SCHEMA``
(see those modules and ``split.py``'s comments on the existing optional
groups): a pre-T015 v11 store has neither table; a store opened/created by
the current ``store.py`` always has both.

``delegation_provisioning`` holds exactly one row per delegation attempt,
keyed by ``idempotency_key = sha256(task_id|coordinator_channel_id|request_key)``.
``delegation_effects`` is an append-only log of intent/outcome markers for
the steps that touch state outside this single SQLite transaction
(workspace, channel, engine session) -- see ``delegation.provisioning`` for
the state machine that writes both tables.
"""
from __future__ import annotations

DELEGATION_SCHEMA = '''
CREATE TABLE IF NOT EXISTS delegation_provisioning (
 idempotency_key TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 assignment_id TEXT, attempt_id TEXT, role_id TEXT NOT NULL,
 step TEXT NOT NULL, status TEXT NOT NULL,
 workspace_binding_id TEXT, channel_id TEXT, config_revision TEXT,
 context_snapshot_ref TEXT, engine_session_ref TEXT, instruction_delivery TEXT,
 error_code TEXT, owner_epoch INTEGER NOT NULL,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS delegation_provisioning_by_task ON delegation_provisioning(task_id);
CREATE TABLE IF NOT EXISTS delegation_effects (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 idempotency_key TEXT NOT NULL REFERENCES delegation_provisioning(idempotency_key),
 step TEXT NOT NULL, phase TEXT NOT NULL CHECK(phase IN ('intent','outcome')),
 detail_json TEXT NOT NULL, at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS delegation_effects_by_key ON delegation_effects(idempotency_key,step,id);
'''
