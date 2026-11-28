# Storage and recovery

## Where the database lives and how it moves

Each logical project has one live SQLite database per channel. Linked worktrees
resolve to the same main project root. By default release uses
`$XDG_STATE_HOME/cortex/projects/<key>/.cortex/cortex.sqlite3`, with
`~/.local/state` as the default XDG state root. Workspace Dev uses
`$XDG_STATE_HOME/cortex-workspace-dev/state/cortex/projects/<key>/.cortex-dev/cortex.sqlite3`
with the default Dev service configuration. An extracted Dev run supplies its
own isolated state root. The runtime derives `<key>` from the sanitized
canonical project basename and the first 32 hexadecimal characters of the
SHA-256 of the canonical project path. There is a separate directory for each
project, and Dev opens only its own channel.

For a home store, the project keeps a private `<project>/.cortex/cortex.sqlite3` location marker
(or the same name under `.cortex-dev`). It is a small SQLite file that binds
the project, channel and store identity to the home database. It carries no
task history. The same project directories can also hold channel ownership
files. Published Markdown, drafts, and the newest-first `pipeline.md` live in
the home store beside its live index. Back up the channel's projects directory
as well as the checkout. A checkout archive alone does not preserve task history.

| Open mode | Existing layout | Writes on open |
| --- | --- | --- |
| `Initialize` | Validates the home binding or handles an eligible old project-local store | Creates a fresh store or performs migration, then installs missing additive schema and indexes |
| `RuntimeValidate`, `LiveReadOnly` | Validates the home binding or reads supported legacy layout | No relocation, migration or schema repair. Normal SQLite WAL/SHM housekeeping is allowed |
| `InspectImmutable` | Validates the existing binding and reads a checkpointed store under the access lock | No creation, migration, checkpoints or repairs |

An initializing open checks both locations under the project access fence.
A home store with a matching binding can restore a missing location marker
only if the project and Git identities still match. Conflicting source and
destination databases, a marker for a different location, or a missing or
foreign destination are refused. Keep both locations and migration backups when
an open reports a conflict. See [format migration](format-migration.md) and
[Workspace channel stores](../cortex-workspace/backlog/CW-S12.md).

Relocation of a supported project-local store first fences ordinary access and
persistent SQLite connections. Format 11 is upgraded with its verified backup
before the format-12 move. The runtime copies SQLite and its document tree into
a private home staging directory, checks the source snapshot and copied files,
and rewrites store-owned paths. It installs the verified destination before the
atomic project-side marker replacement. The private copy of the original store
is retained under the project-side `.migration-backups/relocated-<store-id>/`.
An interrupted switch resumes only with matching retained evidence. An
occupied or stale destination is validated or refused, and stale staging can
be quarantined in the state tree. The runtime also refuses a relocation with
pending publication files or a linked checkout inside the old store.
Workspace shows “Moving project history…” while its runtime performs a
fenced relocation. A first home-store creation also preserves an existing
private project routing file, with its original bytes retained under
`.migration-backups/relocated-pre-database-routing/`.
The older offline format restore primitive applies to project-local stores.
It refuses relocated databases and location markers with
`relocated_store_restore_unsupported`.

### Administering home stores

`cortex-acp project-stores list` reports stores in the binary's compiled
channel, with each store ID, canonical project path, directory size, last
successful writable initialization and whether originals remain. A missing
or unavailable checkout is reported separately from an accessible one.

`cortex-acp project-stores delete STORE_ID --confirm` removes the exact home
store after exclusive checks and clears its matching checkout marker when
the checkout is accessible. If it is unavailable, the command retains the
removal intent so the owner can retry the same ID once it returns.
`cortex-acp project-stores cleanup-original STORE_ID --confirm` removes the
retained relocation originals from an accessible checkout. Run original
cleanup first if you want to remove both copies. User-scope
`cortex-paseo-tools uninstall` preserves the home project-store tree.

`cortex-acp project-stores return-to-project PROJECT STORE_ID --confirm`
copies the current database and documents back into the absolute, canonical
project path. Stop the project's runtime before calling it. The command
fences both locations, verifies the copy, and retains the former home store
under `cortex/projects/.migration-backups/returned-<store-id>/` within the
channel state root. A successful return keeps that store project-local on
subsequent opens. If a switch was interrupted, retry the same confirmed
command for `store_return_pending`. `store_return_conflict` preserves the
conflicting locations for inspection.

The writable format-12 installer maintains `native_worktree_starts` for Start
intent, provision evidence and cleanup disposition. `task_workspaces` retains
the execution root; checkout authority is verified against the provision record,
not a desktop projection. This adds neither a store nor a format change.
See [worktree recovery](../cortex-workspace/consistency-and-recovery.md#worktree-start-and-cleanup).

Each channel's project-side marker directory keeps itself out of `git status`: a
writable `Initialize` open creates `<project>/.cortex/.gitignore` or its Dev counterpart containing `*` (which also
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

Local Code's **Edit** action uses the shared runtime open and an ordinary WAL write
when the file-lease schema is installed. Live read snapshots can continue during
lease acquisition. A missing store, a format-11 migration, or a wholly absent
lease schema group still requires the exclusive initialization path. A partial
lease schema is refused. A conflicting writer or exclusive storage operation can
still block an edit, and reading a file never initializes storage.

SQLite records task and requirement revisions, assignments, attempts,
instruction delivery, candidates, check evidence, resource ownership and
idempotency keys. Published reports are immutable Markdown files. Bounded cursor
reads apply to both report bodies and catalogues.
The additive format-12 `fact_memory` table stores bounded source observations
with their task, published report, assignment, attempt, plan revision, code
path and SHA-256. It records typed failures in the same transaction as the source
result and cascades on source report or task purge. Each channel retains its
own memory. Freshness is evaluated against the receiving checkout when the
strategy selects facts, since different linked worktrees can contain different
bytes at the same path. See [source-backed fact memory](../features/knowledge-routing/index.md#source-backed-fact-memory).

Changing work uses an isolated workspace and a candidate snapshot. Integration
requires the exact candidate digest and expected project base revision. Runtime
operations use an operation lock, pre-write journal and atomic replacement;
partial or unknown outcomes remain recoverable states.

After a crash, reconcile the existing attempt and owned processes before
starting another attempt. Old sessions cannot mutate current state. Cleanup
removes a workspace only after exact publication or an explicit abandonment;
conflicts, failed reads and missing copies are retained.

Each project store has one FIFO writer with 128 ordinary queue slots and eight
reserved slots for Workspace owner draft creation and live settings. Ordinary
work cannot consume the reserve or overtake an accepted owner job, and owner jobs
do not jump ahead of accepted ordinary jobs or their deferred display writes.
A full owner reserve is refused before enqueue (`owner_queue_busy`); ordinary
submissions fail promptly with `busy` unless their caller explicitly uses the
two-second ordinary admission wait. Neither rule sets a commit deadline: an
in-flight job or SQLite lock can still delay the writer.

A separate per-project setting lane accepts at most eight requests at a time,
including its active request. It does not wait for the general execution-control
lock. Each change writes its admission and finish receipts and the settings event
in one transaction, for next use; it does not reconfigure an active engine
session. Draft and live settings requests replay the original receipt for the
same identity and payload; a changed payload on that identity is refused.
Sprint membership uses the saved project overlay with the machine-state lock
and the store's draft guard. It is a separate operation from draft creation;
a failed link does not erase the draft.

Task state is durable before recovery decisions. In-memory state may accelerate
reads but cannot be the only record of a mutation or background-task failure.

## Commit durability

The writer groups commits. Ordinary write bodies (`StoreExecutor::write`) that are
already queued run together in one `BEGIN IMMEDIATE` … `COMMIT`: at most 64 bodies,
and no further body is taken once the group has run for 2 ms. The writer never waits
for companions, so a lone write commits at once. Each body runs inside its own
savepoint. A body that fails is rolled back alone and gets its own error. The other
bodies of the group still commit. Every reply, success or failure, is sent only
after the shared COMMIT returns, in submission order. A caller therefore never sees
success for a write that is not durable, exactly as with one commit per write. If
SQLite rolls back the whole transaction (for example a full disk or an I/O error),
nothing in the group committed and every body that had succeeded gets that error.
Jobs that manage their own transactions (publications and owner jobs) are never
grouped: they end the group and run alone, in FIFO order. The owner reserve and the
order of deferred display writes are unchanged.

Read-only document tools (`read_report`, `read_draft`, `list_reports`,
`tools_discover`) run on the reader pool, against a WAL snapshot, and never queue
behind the writer or its fsync. A reader-pool store refuses every write with
`storage_read_lane`. A read that needs a crash reconciliation write is then repeated
whole on the writer. The `REPORT_PAGE_READ` receipt of a report page is submitted to
the writer without waiting for it, so the reply does not wait for its commit. It is
an ordinary grouped write with `synchronous=FULL`, ordered before every later write
of the same process, including the review result that relies on it. If that write
fails, the review gate still finds the report unread and asks for it again. An
oversized reply still uses the writer to store its deferred body.

Every commit uses `synchronous=FULL`, with one exception. A batch of deferred display
writes (`StoreExecutor::write_deferred`, such as streamed worker message chunks) that
commits on its own uses `synchronous=NORMAL`. The first fragment after 40 ms of
stream inactivity is due immediately. A continuing stream batches for up to 40 ms,
with earlier flushes at 64 fragments, before an ordered publication or owner job,
or at shutdown. Work already on the writer lane can extend the queue wait. In
WAL mode that commit does not wait for an fsync of the log. The connection is back
at FULL before the next write, and a later FULL commit makes every earlier relaxed
commit durable together with its own. The risk is limited to a power failure or an
operating-system crash, never a process crash. It can lose the newest display rows
committed since the last FULL commit. The database stays consistent. State
transitions, results, receipts, owner actions and every other write commit FULL,
and so do deferred rows that commit in the same group as such a write.

## Rust Core storage domain

The additive `crates/cortex-core` library preserves SQLite application ID
1129467218 and format 12, WAL, foreign keys and synchronous=FULL (NORMAL only for
deferred display batches, see [Commit durability](#commit-durability)). It is the
store of the Rust runtime. Project/channel resolution selects only
the selected, isolated state root. Disposable tests never migrate the owner's store.

`store::documents::Publisher` combines owned file-backed drafts, immutable
reports, one newest-first `pipeline.md` per task, edition/provenance metadata,
thread-scoped request receipts, bounded catalogue/content pages and durable
publication-intent recovery. Templates are compiled into the Rust library.
Report bodies stay in Markdown files, not SQL request receipts.
`cortex_core::pipeline::sync` publishes generated editions directly under
the `runtime` principal, with the edition, provenance and receipt in one
SQLite transaction. It uses the same durable file intent, replacing rename
and directory synchronization as ordinary publication. Committed edition
cleanup can be retried independently of the durable publication. Older pending
pipeline drafts resume under their original receipt. Worker publication uses
owned file-backed drafts. Callers must
supply already authenticated task/thread identities: the library is not a
transport authorization boundary. Reads in writable modes reconcile the selected
task under a publication operation lock; immutable inspection never reconciles.
A file/digest conflict is an error, not evidence of publication success.
A published file whose digest was verified in its exact current state (inode, size, mtime and
ctime) is not re-read for the next publication, catalogue or page; any change of that state forces a
real hash. A publication keeps the previous pipeline by a hard link instead of a second copy, and
recovery drops that link if an install was interrupted before the replacing rename.
Recovery removes private staging files left before the complete intent was installed. Initializing
opens write the header pragmas only when they differ, and a process skips the per-table column
check for a schema cookie it already validated. Recovery capsule sections read only the newest rows
that can fit their budget; an inexact total carries `total_is_lower_bound`.

Initialize never opens an existing database file with a plain descriptor:
closing it would drop the process's POSIX locks, including those of live SQLite
connections, and let another process delete the live WAL/SHM and corrupt the
store. A missing database is created `O_EXCL` with mode 0600 and an existing one
is only validated (`create_private_database` in
`crates/cortex-core/src/store/locks.rs`).

Writable initialization upgrades only format 11 under the exclusive access
fence and retains a verified logical SQLite backup (including committed WAL
rows) in private `.migration-backups/`. A failure before commit rolls back any
uncommitted changes without copying the backup over the main database. The backup
may remain; restoring it is a separate, explicit offline operation.
Non-initializing format-11 opens report
`legacy_storage_migration_required`; format 10 and unknown formats fail closed.
The upgrade also accepts the shapes the retired Python runtime wrote for format 11,
and validates the complete expected format-12 schema inside the migration
transaction, so an unsupported shape rolls back with the main database bytes
unchanged. Refusals carry a human-readable message and a stable reason
(`CoreError::storage_diagnostic()`); when the canonical store holds no project
records, Release reports `legacy_storage_root_detected` if older storage exists
under `.codex/cortex` (names and metadata only; nothing is moved, and Dev never
inspects it). See [format migration](format-migration.md#opening-python-era-projects-through-incoming-acp).
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

A successful Core write that recorded row changes can wake the active Workspace
reader for the same logical project and channel in the same process. Each key
keeps at most one pending wake, not a list of commits. A wake itself does not
assert a new outbox row. It is separate from the SQLite hooks and
has no effect on durable ordering or the storage format. On Linux, WAL activity
in a verified home store can wake the reader for writes from other processes.
Every reader keeps its [periodic fallback](../cortex-workspace/consistency-and-recovery.md#native-desktop-reads).

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

The `runtime_events_settled_channel_time` partial index covers settled native
turns by task, channel and timestamp. The transcript writer compares the latest
settlement for its active channel to the previous chunk's timestamp before
extending a streamed message.

Cortex keeps a derived list of unresolved command starts. It stores only the
task, start event ID and reference. Journal inserts maintain the list in the
same transaction. The first initializing open backfills it under the store's
exclusive access lock without changing format 12. A reader of an older store,
or one whose command journal was edited, computes the exact list from the
journal. The next initializing open rebuilds affected tasks before readers
resume using the stored list.

Two partial indexes cover assignment references for `WORKER_RELEASED` and
`ACCESS_GRANTED` events. The scheduler uses them when rebuilding active file
reservations and their granted scopes from the current transaction.

The runtime adds `runtime_events_type` on `runtime_events(type, task_id, id)` and `capacity_by_attempt` on `capacity_reservations(attempt_id)`. It also adds `runtime_events_by_tool_call_ref` on `runtime_events(task_id, json_extract(payload_json,'$.tool_call_ref'), type, id)` and the partial `runtime_events_finished_ref` on `runtime_events(task_id, json_extract(payload_json,'$.receipt.tool_call_ref'), id) WHERE type='NATIVE_COMMAND_FINISHED'`, so a tool-call or command-receipt lookup in one task no longer parses every `TOOL_COMPLETED`, `TOOL_AUTHORIZED`, `COMMAND_*` or `NATIVE_COMMAND_*` payload of that task (the first writable open builds them under the exclusive access lock; a lookup that can end on a missing reference took seconds on a large store before). The saved-engine-retry notice read in the board projection names `assignment_ref IS NULL`, so it uses `task_events_by_assignment` instead of parsing every worker-activity `progress` payload. The initializing schema also adds `task_events_by_activity_ref` on `task_events(task_id, json_extract(payload_json,'$.activity_ref'), id)` and `runtime_requests_by_operation` on `runtime_requests(task_id, actor, operation)`, and `host_updates_by_call` on `host_updates(session_id, json_extract(update_json,'$.toolCallId'), seq)` for updates whose `update_json` is valid JSON. The conversation history and coordinator pages find a tool call's earlier updates through `host_updates_by_call`; their lookups name the same `json_valid(update_json)` condition so SQLite can use the partial index. A malformed update is still stored and is never evaluated by it. Worker split-tool lookups use unary `+` on assignment and ordering expressions so SQLite chooses the activity index without `ANALYZE`. These are `CREATE INDEX IF NOT EXISTS` statements in the initializing schema. A writable `OpenMode::Initialize` open therefore installs them on an existing format-12 database, without a format change. Inspection and non-initializing opens never install them. The install runs inside that open's exclusive `.access.lock` and its `BEGIN IMMEDIATE` schema transaction, so a missing index is built once, the first time the runtime writer opens the project. Store validation checks the header, table groups and columns, never indexes. A store without one of these indexes is therefore still accepted, and its lookups are only slower. A store that has them is accepted by a binary whose schema predates them. The same install adds `host_transcript_by_message` on `host_transcript(session_id, kind, message_id)` once the `message_id` column exists, so it is also built when a format-11 store is upgraded. A worker page reads that worker's narration (agent messages whose identity starts with `{attempt}:`) as the range [`{attempt}:`, `{attempt};`) through it, instead of walking every message of the session once per worker. After the install, every initializing open runs SQLite's `PRAGMA optimize=0x10002` with `analysis_limit=1000`: it writes planner statistics (`sqlite_stat1`) for a table never analyzed or grown many times over since, sampling a bounded number of rows, so its cost does not grow with the store. It runs under the same exclusive `.access.lock` in SQLite's own transaction. Read-only and inspection opens neither build the indexes nor write statistics; they accept a store with or without them. On a private copy of a 191 MB release store, the first initializing open built the index and the statistics in 116 ms and 41 ms. No `VACUUM` runs on open: it would rewrite the whole file under the exclusive lock while every read waits, and that store had no free pages to reclaim. Task retention keeps its own `VACUUM`.

A read that an owner is waiting on never fails because Cortex itself holds the exclusive access lock. A project's migration or initialization (index install, `PRAGMA optimize`, format upgrade) can hold `.access.lock` for tens of seconds on a slow disk. Owner reads therefore open the store with `Store::open_waiting` (the Workspace service's `LiveReadOnly` opens in `machine/context.rs`, the native projection reader in `core_host.rs`, and the executor's read lanes), and a store opened that way keeps the same wait on every later `read`. The wait polls the shared lock with a growing back-off (10 ms up to 250 ms) in the calling thread, so it adds no threads beyond the existing read lanes, and it ends after `READ_ACCESS_WAIT` (10 minutes). Only a holder that outlasts that still yields `busy`; Workspace clients treat `busy` (and the "project store is busy" diagnostic) on a read as a waiting state, keeping the skeleton or last confirmed view and reading again, never as an error. Ordinary `Store::open` keeps the two-second wait, and mutations, retention and every writable open still refuse `busy` at once rather than queue.

Worker-panel activity envelopes are redacted before they are persisted. An envelope of 64 KiB or less stays inline in its `progress` event. A larger one is kept whole in the private large-output store under coordinator owner scope, with `tool_result` as its source kind and the event reference as its source reference. The event then keeps bounded 512-byte field previews, `payload_offloaded`, the `large_output_id`, the full SHA-256 and the byte count. The body is read through the coordinator's scoped `read_large_output`, and replaying the same event reuses the stored body. The body is written before the event, so an interruption between the two can leave an unreferenced body, and no cleanup runs for it. Offloading does not rewrite the rows a store already holds.

## Automatic Workspace retention

The Workspace service (`crates/workspace-service/src/storage_retention.rs`) runs one maintenance pass when a local project is activated and then every 60 minutes. It archives Done tasks after `autoArchiveDays` (default 7), purges archives older than `retentionDays` (default 30, counted from archive time; a Done task newly archived by sprint closure counts from its completion time plus `autoArchiveDays` instead, and an unknown completion time permits no age purge, see [closing sprints](../cortex-workspace/local-task-execution.md#editing-saved-tasks-and-closing-sprints)) while `automaticArchivePurge` is true (the default), and removes unreferenced attachment files after `unreferencedAttachmentRetentionDays` (default 1). The attachment sweep reads committed answer references from every retained project store, including projects that are removed from the active list and can be added again; if any such store cannot be read, the sweep deletes nothing. Settings and ranges: [configuration](../configuration.md#workspace-storage-retention).

Deletion goes through the same exclusive-lock, journaled core path as manual cleanup (`crates/cortex-core/src/store/retention.rs`) and rechecks closure, age and identity there. Only a closed task with a verified linked execution in COMPLETED, FAILED or CANCELLED state qualifies; open questions, unsettled attempts, running nodes, unreleased reservations, possibly live processes, loaded sessions and busy stores protect a task. A scheduled pass never forces. Dirty worktrees are retained, unmerged work needs the existing discard authorization, and only a clean Workspace-created task worktree is removed, without force. Migration backups and qualification evidence are never expired. A scheduled purge frees pages for reuse but does not run a whole-database `VACUUM`, so the database file may not shrink immediately; manual cleanup still reports reclamation.

## Closed-task activity compaction

A worker streams thousands of `progress` events (`kind=worker_activity`) per attempt: thought chunks, usage updates and every intermediate state of a tool call. On a measured 4.2 GB store they were 788,000 rows and 1.2 GB of payload, about 36% of the database. The Workspace service's hourly retention pass (see [automatic Workspace retention](#automatic-workspace-retention)) therefore compacts them for closed tasks (`Store::compact_closed_task_activity`, `crates/cortex-core/src/store/compaction.rs`).

- **Which tasks.** `runtime_tasks.state` COMPLETED, FAILED or CANCELLED with `updated_at` older than 7 days, no unreleased reservation, no running node and no live process (the same `quiet` check that guards deletion), re-checked inside every write transaction. Open, waiting, paused and recovering tasks, and anything newer, are never touched.
- **What is removed.** Only `progress` rows of `worker_activity` whose `tool_kind` the worker panel never displays (thought chunks, usage updates, mode/config/command/session updates, turn markers, wait notifications), and rows in `pending` or `in_progress` state whose `activity_ref` has a later `completed` or `failed` row. The newest 200 `progress` events of the task stay.
- **What stays.** Every other event type (delegation, assignment, worker result and question, user message, decisions), every other `progress` kind, all terminal tool states, an unfinished tool call's only state, and all rows of other tables (receipts, results, reports, state events, transcripts).
- **Summary.** One `progress` event per task, `dedup_key` `activity_compaction`, `kind` `activity_compacted`, keeps the removed row and payload-byte counts, the policy version and the newest event id covered. A task whose marker already covers its newest event is skipped without scanning.
- **How it runs.** Batches of 2,000 rows, each its own `BEGIN IMMEDIATE` transaction under the shared access lock (a busy store skips the pass, nothing waits), 100 ms apart, at most 200 batches or 20 s per pass, and no pass when the one-minute load average per CPU is above 0.5. It resumes at the next hourly pass. It publishes no Workspace invalidation: nothing a client shows changes for a closed task. No schema or format change; it needs no migration.
- **Space.** Deleted pages go to the SQLite free list and are reused by later writes, so the file stops growing but does not shrink by itself: databases created so far have `auto_vacuum=NONE`. If a store was created with `auto_vacuum=INCREMENTAL`, the pass runs `PRAGMA incremental_vacuum(4000)` after deleting. Returning the space of an existing store requires a one-off `VACUUM` (offline or by the manual cleanup), and switching it to incremental vacuum requires `PRAGMA auto_vacuum=INCREMENTAL` followed by a `VACUUM`; both rewrite the whole file under the exclusive lock and are not run automatically. On a private copy of the 4.2 GB store the pass removed 439,073 of 788,224 progress rows (255 MB of payload, 589 MB of free pages with their indexes) and a following `VACUUM` brought the file to 3.49 GB.

## Explicit task retention

The explicit `/cortex-clear` command in the native ACP host does not yet delete tasks: a confirmed `/cortex-clear` that would delete something refuses and writes nothing (see [native incoming ACP](native-incoming-acp.md#known-limits)); previews work. This limit covers only the explicit command; the Workspace service's background pass does purge archived closed tasks (see [automatic Workspace retention](#automatic-workspace-retention)). The retention rules below describe the intended contract of the explicit command.

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
