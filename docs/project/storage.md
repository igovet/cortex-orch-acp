# Storage and recovery

The canonical store is `<project>/.cortex/cortex.sqlite3`, with task documents,
reports, drafts and the newest-first `pipeline.md` beside it. A project never
uses a home-wide Cortex database.

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

## Explicit task retention

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
