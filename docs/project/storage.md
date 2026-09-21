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
