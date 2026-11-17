# Storage and recovery

The canonical store is `<project>/.cortex/cortex.sqlite3`, with task documents,
reports, drafts and the newest-first `pipeline.md` beside it. A project never
uses a home-wide Cortex database. Workspace Dev selects `<project>/.cortex-dev/cortex.sqlite3` instead, with its documents and drafts beside it. Each channel has exactly one store per logical project, shared by linked worktrees. The host selects a fixed channel, not an arbitrary path. Dev never reads or imports the default/release store or legacy `.codex/cortex` storage; an existing release `.cortex` does not prevent opening the folder in Dev. See [channel stores](../cortex-workspace/backlog/CW-S12.md).

Each channel's store directory keeps itself out of the project's `git status`: a
writable `Initialize` open creates `<store>/.gitignore` containing `*` (which also
ignores itself) when it is absent. It is written without following a link and
never replaced, so an existing file is left as it is; read-only and inspection
opens never create it. The project's tracked `.gitignore` and `.git/info/exclude`
are not changed for this.

A normal writable `Store::open(.., OpenMode::Initialize)` installs additive schema
groups and upgrades format 11 to 12 under the migration lock with a verified
private backup. `OpenMode::InspectImmutable` does neither: it validates the
existing directory, database and access lock without creating them, and uses an
immutable read under the exclusive access lock. A nonempty WAL is refused as
`storage_inspection_requires_checkpoint`, never ignored or checkpointed by the
inspection. Close writable runtime connections normally before inspection.
Missing optional tables remain absent; open normally before using endpoints
that need those tables. Format 11 still reports
`legacy_storage_migration_required`; format 10 and unknown formats fail closed.
This constructor contract does not make later explicitly requested Store
operations read-only.

Live gateway, external-tool relay, runtime startup, configuration API project reads,
Workspace product review-action reads and owner command-control/retention callers use `OpenMode::RuntimeValidate` (or `LiveReadOnly` for projections) instead.
This validation-only mode takes the ordinary shared access lock and reads current
committed WAL data alongside other runtime connections. It never migrates,
repairs schema or backfills rows, and retains the same format-11 migration-required
and format-10/unknown rejection rules. It permits normal SQLite WAL/SHM creation
and checkpoint housekeeping, so it is not a non-mutating inspection surface.
Exclusive maintenance still fences these shared opens; it is not bypassed.

SQLite records task and requirement revisions, assignments, attempts,
instruction delivery, candidates, check evidence, resource ownership and
idempotency keys. Published reports are immutable Markdown files. Bounded cursor
reads apply to both report bodies and catalogues.

Changing work uses an isolated workspace and a candidate snapshot. Integration
requires the exact candidate digest and expected project base revision. Runtime
operations use an operation lock, pre-write journal and atomic replacement;
partial or unknown outcomes remain recoverable states.

After a crash, reconcile the existing attempt and owned processes before
starting another attempt. Old sessions cannot mutate current state. Cleanup
removes a workspace only after exact publication or an explicit abandonment;
conflicts, failed reads and missing copies are retained.

Task state is durable before recovery decisions. In-memory state may accelerate
reads but cannot be the only record of a mutation or background-task failure.

## Rust Core storage domain

The additive `crates/cortex-core` library preserves SQLite application ID
1129467218 and format 12, WAL, foreign keys and synchronous=FULL. It is the
store of the Rust runtime. Project/channel resolution selects only
`.cortex` or `.cortex-dev`; disposable tests never migrate the owner's store.

`store::documents::Publisher` combines owned file-backed drafts, immutable
reports, one newest-first `pipeline.md` per task, edition/provenance metadata,
thread-scoped request receipts, bounded catalogue/content pages and durable
publication-intent recovery. Templates are compiled into the Rust library.
Report bodies stay in Markdown files, not SQL request receipts. Pipeline draft
creation is a runtime-only API, separate from worker draft creation. Callers must
supply already authenticated task/thread identities: the library is not a
transport authorization boundary. Reads in writable modes reconcile the selected
task under a publication operation lock; immutable inspection never reconciles.
A file/digest conflict is an error, not evidence of publication success.

Initialize never opens an existing database file with a plain descriptor:
closing it would drop the process's POSIX locks, including those of live SQLite
connections, and let another process delete the live WAL/SHM and corrupt the
store. A missing database is created `O_EXCL` with mode 0600 and an existing one
is only validated (`create_private_database` in
`crates/cortex-core/src/store/locks.rs`).

Writable initialization upgrades only format 11 under the exclusive access
fence and retains a verified logical SQLite backup (including committed WAL
rows) in private `.migration-backups/`. Migration failure rolls back and restores
the verified logical snapshot. Non-initializing format-11 opens report
`legacy_storage_migration_required`; format 10 and unknown formats fail closed.
Explicit offline restore requires stopped access, confirmation, the backup's
expected SHA-256, compatible application/format and integrity validation. It
checkpoints the current database before replacing it, fences mutable Rust
connections (read-only providers detect replacement before reuse), and replaces only the database bytes, not Markdown/drafts or engine
state. Cooperative locks do not exclude arbitrary same-user or old independent
writers. Restore is not automatically performed merely because a read fails.

`OpenMode::LiveReadOnly` is the native desktop projection mode. It opens an
existing format-12 database read-only, observes committed WAL and never installs
schema or writer hooks. Its short read transactions take the existing shared
access lock, and inode replacement is detected before connection reuse. SQLite
may perform normal WAL/SHM housekeeping; this is not strict immutable inspection.
`InspectImmutable` remains the non-mutating, checkpoint-required inspection mode.
Neither mode can call `Store::write` or migrate a legacy store.

Scoped checks are `cargo build -p cortex-core`, `cargo test -p cortex-core` and
`cargo fmt --check -p cortex-core`. Passing them is library evidence only, not
release or installed-client qualification. See [architecture](architecture.md#native-host)
and the [release gates](verification.md).

## Workspace adapter tables

The Cortex Workspace adapter adds an optional, additive table group to the project store: `workspace_projects`, `workspace_tasks`, `workspace_executions`, `workspace_starts`, `workspace_requests`, `workspace_outbox_state` and `workspace_outbox`. The group is installed only by a normal writable open, with the other schema groups, and the store format stays 12. A store without these tables remains valid. A partial group fails closed. Splitting a project copies only that project's adapter rows and counters. Public invalidation hooks are TEMP triggers installed per Store connection only when the adapter tables are available, not persistent dependencies on Workspace code for independent or older SQLite writers. Writable initialization removes the previous persistent outbox hooks in the schema transaction; non-initializing opens leave main unchanged, including any legacy hooks awaiting a writable repair. Legacy hooks are not duplicated by TEMP hooks. The connection-local hooks and transactional guarantees are described in [consistency and recovery](../cortex-workspace/consistency-and-recovery.md).

## Attempt tree listings

Each worker attempt records a project observation when it starts and when it
publishes (`attempt_revisions`). Only the start capture keeps its full tree
listing, which the attempt's own publication checks compare against while its
process lives. When the process registry records a confirmed exit, that listing
is dropped; kind, head and
digest stay. Publication captures store kind, head and digest only. A writable
store open clears listings that already qualify. The space is reused by later
writes; the file shrinks only after a `VACUUM`, such as the one task retention
runs below.

## Runtime indexes and offloaded worker activity

The runtime adds `runtime_events_type` on `runtime_events(type, task_id, id)` and `capacity_by_attempt` on `capacity_reservations(attempt_id)`. The initializing schema also adds `task_events_by_activity_ref` on `task_events(task_id, json_extract(payload_json,'$.activity_ref'), id)` and `runtime_requests_by_operation` on `runtime_requests(task_id, actor, operation)`. Worker split-tool lookups use unary `+` on assignment and ordering expressions so SQLite chooses the activity index without `ANALYZE`. These are `CREATE INDEX IF NOT EXISTS` statements in the initializing schema. A writable `OpenMode::Initialize` open therefore installs them on an existing format-12 database, without a format change. Inspection and non-initializing opens never install them.

Worker-panel activity envelopes are redacted before they are persisted. An envelope of 64 KiB or less stays inline in its `progress` event. A larger one is kept whole in the private large-output store under coordinator owner scope, with `tool_result` as its source kind and the event reference as its source reference. The event then keeps bounded 512-byte field previews, `payload_offloaded`, the `large_output_id`, the full SHA-256 and the byte count. The body is read through the coordinator's scoped `read_large_output`, and replaying the same event reuses the stored body. The body is written before the event, so an interruption between the two can leave an unreferenced body, and no cleanup runs for it. Offloading does not rewrite the rows a store already holds.

## Explicit task retention

The native host does not yet delete tasks: a confirmed `/cortex-clear` that would delete something refuses and writes nothing (see [native incoming ACP](native-incoming-acp.md#known-limits)); previews work. The retention rules below describe the intended contract.

The owner uses `/cortex-clear before YYYY-MM-DD` in an idle project ACP chat
for a counts-only preview, then the displayed command ending in `confirm` to
remove old unprotected tasks. The date cutoff is strictly before midnight UTC;
`N`, `Nd` or `days N` use retention days (0..36500). Invalid/future input never
deletes. Activity is the maximum of creation and published edition updates.
By default loaded-session tasks, non-terminal states and live/ambiguous capacity
or process activity are protected. Append `force` to preview their deletion and
`force confirm` to authorize it. Force always keeps the invoking session's task
and explicit kept threads. It assumes stale processes are already dead, releases
reservations and marks process records terminated without killing OS processes.
Busy-store access and pending intents conflicting with remaining keeps are refused. An exclusive access lock spans selection and
cleanup; transactional `pending_deletions`/`finish_deletions` still recover
committed filesystem deletion after interruption. Preview does not replay those
intents. Only linked task artifacts/drafts are removed, never unrelated drafts.
Old unbound/orphan ACP host sessions are removed with their transcript/update
and session-workspace/access-default rows. Bound-task retention deletes session
access defaults before their host-session parents; surviving sessions retain their
stored defaults and explicit mode selections. Orphan-session last activity is the
maximum of creation,
close and latest transcript/update timestamps; invoking and (unless forced)
loaded sessions are excluded. Preview includes `would_delete_orphan_sessions`.
After anything is deleted and committed, `VACUUM` runs outside a transaction
while the exclusive lock is still held. Output includes reclaimed bytes and
before/after DB file sizes; reclamation failure leaves deletion committed and
is reported separately. Deleted sessions are removed from the agent's cache.
The manual CLI JSON includes `deleted_tasks`, `retention_days`,
`skipped_protected`, `cutoff`, `force`, `deleted_orphan_sessions`,
`vacuum_status` and `reclaimed_bytes`; measured before/after database sizes
are `db_bytes_before`/`db_bytes_after`, and a vacuum failure adds
`vacuum_error`. Dry runs instead include `would_delete_tasks` and
`would_delete_orphan_sessions`. See [retention maintenance](packaging.md#retention-maintenance)
for its command and explicit thread protection.
