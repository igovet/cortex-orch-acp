# Format migration, upgrade and rollback (T032)

This page covers offline SQLite format-version safety for one project's
own `.cortex` store: `cortex.sqlite3`, its `.access.lock`, and the
Markdown task/draft trees living alongside it. It does **not** cover the
already-completed legacy-root move (`<project>/.codex/cortex` ->
`<project>/.cortex`, T001/T004/T019 — see
`project_storage.migrate_legacy_project_storage()`), and it is not a
Codex-plugin-harness rollback: the plugin host, hooks, gateway and
`marketplace_bootstrap` are scheduled for deletion at T033 with no
rollback path to them (user decision).

## Compatibility conclusion: no new format version is needed

`store.py` has pinned `PRAGMA user_version=11` since T005 and never bumped
it again through T031. Every metadata addition since (T007 catalog routes,
T008 configuration, T009 ACP sessions, T015–T017 delegation/events/
capacity, T018 evidence, T019 replay/evidence-observations, T020 questions,
T021–T022 language/stopped/generations, T023 process registry, T024
budgets, T025 workspaces, T027 routing extras, T029 observability, and
T032's own `format_history`) is **one independent, self-healing,
all-or-nothing optional table group** on top of the same format-11 core:

- Every `Store()` open unconditionally re-runs the complete
  `CREATE TABLE IF NOT EXISTS ...` schema script (`store.py:322-324`) —
  a pre-existing store simply gains whichever groups it predates, with
  zero rows, and no version bump is ever required for this.
- `split.py`'s `_validate_format()`/`_expected_layout()` independently
  enumerate all seventeen optional groups and assert each one is present
  either completely or not at all — this is the same invariant `plan()`
  (below) relies on, verified by a large, pre-existing `test_project_split.py`
  suite (`test_split_accepts_a_legacy_v11_source_missing_the_*` cases).
- This was already the explicit conclusion of the T019 report ("Format:
  all additions additive; no T032 migration required").

**Evidence**: `tests/test_format_migration.py::test_plan_reports_no_op_for_current_format_without_mutating`
opens a fresh format-11 store and confirms `plan()` reports a no-op with
zero bytes changed; `test_split_accepts_a_legacy_v11_source_missing_*`
(pre-existing) proves the runtime already tolerates every possible subset
of optional groups.

The **only** versioned schema change this runtime has ever needed is
format 10 -> 11 (T005, before this task packet's traceable history), and
`migrate.py` already implemented it: verified backup, exclusive
stopped-access lock, one atomic SQLite transaction, immutable Markdown.
T032 hardens that existing tool with `plan`/`rollback` and a durable
`format_history` audit table, and proves the crash-recovery and
concurrency-exclusion properties the task requires — it does not invent a
new migration.

## Tooling: `scripts/cortex_migrate.py`

```
python3 scripts/cortex_migrate.py plan     --storage-dir <path-to-.cortex>
python3 scripts/cortex_migrate.py migrate  --storage-dir <path> --backup <fresh-path> --access-stopped
python3 scripts/cortex_migrate.py rollback --storage-dir <path> --backup <existing-backup> --access-stopped --confirm
```

All three require **stopped access** (`--access-stopped`): every CLI,
Desktop, MCP client and hook process using this project must be closed
first — none of the three operations is ever run concurrently with a live
runtime, and none is an MCP tool.

### `plan` — read-only, no mutation

Opens the database read-only (`mode=ro`), inspects
`application_id`/`user_version`, and returns exactly one of:

- `{"action": "none", ...}` — already format 11 (the common case; any
  table group this store predates self-heals on the next `Store()` open).
- `{"action": "migrate", "from_version": 10, "to_version": 11, ...}` —
  a genuine format-10 store; run `migrate` next.
- Raises `unsupported_storage` for any other `application_id`/version —
  never guesses, never reads an unrecognized format.

### `migrate` — the format 10 -> 11 upgrade

1. Takes the storage directory's `.access.lock` exclusively
   (`flock(LOCK_EX|LOCK_NB)`) — refused with `storage_busy` if any other
   process (an old runtime's own read/write connection, or a concurrent
   `migrate`/`rollback`) already holds it. This is the same lock file
   `Store.connection()` takes with `LOCK_SH`, so a live runtime and an
   offline migration can never overlap either direction.
2. Creates a **fresh** (`O_EXCL`, never overwritten) backup file, verifies
   it with `PRAGMA integrity_check`, `fsync`s it and its directory, and
   computes its SHA-256 digest — all before any schema change.
3. Applies the entire schema-creation + data-backfill + a
   `format_history` row as **one SQLite transaction**
   (`BEGIN EXCLUSIVE ... COMMIT`), verified by `PRAGMA foreign_key_check`
   before commit. Markdown files are never opened.
4. `PRAGMA wal_checkpoint(TRUNCATE)` and lock release.

Filesystem and SQLite are **not** treated as one transaction: the backup
file (step 2) is a separate, already-fsync'd artifact before step 3's
SQLite transaction begins, so a crash at any point before that
transaction's own `COMMIT` leaves the live database completely
untouched (SQLite auto-rolls back its own uncommitted transaction on the
next open) — a fresh `migrate` retry with a new backup path fully
recovers. A crash after `COMMIT` leaves a fully upgraded, `format_history`
-recorded store. There is no window where a half-applied schema is left
live and silently used.

### `rollback` — restore one verified backup, nothing else

Requires both `--access-stopped` and `--confirm` (default is refusal, not
a silent dry run of real deletion). Never recreates a second storage root
and never deletes `.cortex` — it only ever replaces
`cortex.sqlite3`(+`-wal`/`-shm`/`-journal`) inside the one storage
directory it is given, after:

1. Taking the same exclusive `.access.lock` (same concurrency guarantee
   as `migrate`).
2. Verifying the backup: `application_id` must match this runtime's, and
   `user_version` must be a format this runtime recognizes (10 or 11) —
   otherwise `rollback_backup_incompatible`. `PRAGMA integrity_check` must
   be `ok` — otherwise `rollback_backup_invalid`. **A rollback is refused,
   never a best-effort read of an incompatible or unverifiable backup with
   this runtime.**
3. Copying the backup to a private temp file in the same directory,
   `fsync`ing it, re-verifying its integrity, removing any stale
   sidecar files, and `os.replace()`-ing it over the live database
   (POSIX-atomic).
4. Appending one line to `rollback.log` (a plain JSON-lines audit file,
   never mutating the restored database itself — the restore is
   byte-for-byte, not touched afterward).

A crash before the replace leaves the live database completely untouched
(at most a stray `.rollback-*.tmp`, cleaned up automatically at the start
of the next attempt, exactly like the pre-existing `.pending_*.tmp`
convention). A crash during the atomic replace cannot leave a partial
file (POSIX `rename()` guarantee). A crash after the replace leaves a
fully restored database; `rollback.log`'s entry may be missing, which is
cosmetic only.

Restoring a format-10 backup produces a format-10 store this exact
runtime build will then itself refuse to open (`unsupported_storage`) —
rollback is for restoring the state a matching **older** Cortex build can
read, not for making this runtime read an older format. This is
deliberate: T032 never adds a downgrade-read path.

### `format_history`

A new, self-healing (`CREATE TABLE IF NOT EXISTS`, no version bump)
append-only table recording every `migrate` this database file has ever
had applied, with its backup path and SHA-256 digest — durable evidence
independent of any external log.

## Concurrency

Both `migrate` and `rollback` take the storage directory's `.access.lock`
exclusively; `Store.connection()` (every live MCP/hook operation) takes it
shared. Two mutating operations — old runtime vs. new, `migrate` vs.
`rollback`, or two concurrent `migrate` invocations — can never proceed
against the same project at once; the second is refused with
`storage_busy`. Proven in `tests/test_format_migration.py`
(`test_migrate_refused_while_a_store_connection_is_open`,
`test_rollback_refused_while_a_store_connection_is_open`,
`test_second_mutating_migration_process_is_refused_while_first_holds_the_lock`).

## What is explicitly not supported

- No downgrade-read of data written by a newer format than this runtime
  build recognizes — refused, never attempted.
- No rollback to the legacy Codex-plugin-harness runtime (gateway, hooks,
  `marketplace_bootstrap`, `.codex-plugin` marketplace wiring) — that
  runtime is deleted at T033 with no supported path back to it.
- No automatic migration on ordinary `Store()` open for format 10 — an
  operator must run `cortex_migrate.py migrate` explicitly with a backup;
  `Store()` only ever raises `unsupported_storage` for it
  (`contracts.py`).
- `rollback` restores exactly the backup's bytes; it never merges,
  reconciles, or replays work performed after that backup was taken.

## Tests

`tests/test_format_migration.py` (21 cases): `plan` no-op/exact-change
reporting without mutation; `migrate` backup digest and `format_history`
recording; `rollback` byte-for-byte restore, required
confirmation/access-stopped flags, refusal of a corrupt/foreign/
newer-format backup; concurrency refusal (live connection and a second
mutating process); preservation of Markdown and of an unrelated optional
table row (`workspace_snapshots`) across `migrate`; preservation of
Markdown and of completely separate connection/credential/engine-state
roots across `rollback`; and a `CORTEX_FAULT_INJECTION_POINT` crash matrix
at three `migrate` checkpoints and three `rollback` checkpoints (before
backup/write, after backup/write before the mutating step, after the
mutating step before confirmation), each followed by a fresh attempt that
reaches a consistent, `PRAGMA integrity_check`-clean, non-half-applied
state.
