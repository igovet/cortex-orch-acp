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
Current/session-bound tasks, non-terminal states and live/ambiguous capacity or
process activity are protected. Busy-store access and protected pending intents
are refused, not overridden. An exclusive access lock spans selection and
cleanup; transactional `pending_deletions`/`finish_deletions` still recover
committed filesystem deletion after interruption. Preview does not replay those
intents. Only linked task artifacts/drafts are removed, never unrelated drafts.
See [retention maintenance](../acp/packaging-and-onboarding.md#retention-maintenance)
for the direct, owner-run CLI alternative and its explicit thread protection.
