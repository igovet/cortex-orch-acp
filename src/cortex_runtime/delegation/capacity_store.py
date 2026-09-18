"""``capacity_reservations``: T017's optional schema group for concurrency
accounting (see ``delegation.capacity``).

Composed into ``store.py``'s per-project SQLite database the same way
``delegation.events_store.TASK_EVENTS_SCHEMA`` is (an additive optional
group on top of the existing v11 schema; no ``user_version`` bump needed).

One row per reservation, one reservation per owned engine process (one
``EngineSessionManager``/attempt -- see
``delegation.live_adapters.EngineSessionManagerAdapter``, "no two engine
instances share a process"). ``state``:

- ``reserved`` -- counts against the connection's concurrency limit; the
  process is believed alive/owned.
- ``ambiguous`` -- termination was attempted but exit was never *observed*
  (a timeout, or a wait that expired without ``wait_exited`` returning
  True); still counts against the limit -- never auto-released here (a
  bounded reaper that re-checks/force-kills a long-ambiguous reservation is
  T028 scope, not this one).
- ``released`` -- exit was positively observed after
  terminate/shutdown; no longer counts against the limit.

Releasing a reservation never deletes it (or any task_events/reports
history) -- history stays readable via ``list_events``/the reports table
after release, same as every other retention rule in this codebase.
"""
from __future__ import annotations

CAPACITY_SCHEMA = '''
CREATE TABLE IF NOT EXISTS capacity_reservations (
 reservation_id TEXT PRIMARY KEY,
 connection_id TEXT NOT NULL,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 assignment_ref TEXT,
 attempt_id TEXT NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('coordinator','worker')),
 state TEXT NOT NULL CHECK(state IN ('reserved','ambiguous','released')),
 reserved_at TEXT NOT NULL,
 updated_at TEXT NOT NULL,
 release_observation TEXT
);
CREATE INDEX IF NOT EXISTS capacity_by_connection ON capacity_reservations(connection_id, state);
CREATE INDEX IF NOT EXISTS capacity_by_task ON capacity_reservations(task_id);
'''
