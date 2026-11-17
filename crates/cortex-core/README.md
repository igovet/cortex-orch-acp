# Cortex Core foundation

This crate is an additive Rust library, not the active product entry point.
It does not start an ACP engine, replace the Python runtime, or establish full
lifecycle, permission, publication or qualification parity.

## Storage

`Store::open(ProjectKey, OpenMode)` resolves one logical Git project and fixed
release/dev channel. Dev uses only `.cortex-dev/cortex.sqlite3`; release uses
only `.cortex/cortex.sqlite3`. Validated linked worktrees share their main
checkout's store. Private files, cooperative access locks, application ID
1129467218 and format 12 are checked before writable initialization.

- `Initialize` installs additive schema, WAL, foreign keys and synchronous=FULL.
  Format 11 upgrades under the exclusive access fence, retaining a verified
  private SQLite backup and a format-history row. Format 10 and unknown formats
  are refused.
- `RuntimeValidate` reads current committed WAL and installs connection-local
  outbox hooks, without schema repair, migration or data backfill.
- `InspectImmutable` opens existing files only, holds an exclusive access fence,
  refuses nonempty WAL and cannot write or checkpoint.

The SQL and column/group manifests in `src/store/` are the authoritative format-12
schema. `tests/fixtures/python_format12.json` is a frozen snapshot of the schema the
retired Python Store initialized; tests compare against it as golden data and
nothing regenerates it.

## Connections and effects

`Store` is a blocking primitive with one reusable connection. `StoreExecutor`
holds one writer plus four read lanes, with bounded queues and awaitable replies.
Construct and drop it on a blocking service thread; its destructor joins all
lanes. It acquires a separate lifetime Core owner fence before initialization.
Older independent writers do not obey that fence.

Writes start with BEGIN IMMEDIATE. State, private events, request receipts and
public outbox metadata commit together. Temporary SQLite hooks use the audited
raw adapter in `src/store/outbox.rs`; they never recursively borrow a rusqlite
Connection or install persistent Rust-dependent triggers. The outbox retains
metadata only, not historical DTO deltas or private bodies.

External effects normally belong behind ports, outside SQLite transactions.
The publication domain is the deliberate filesystem exception: under access
and publication operation locks it stages/fsyncs private Markdown, persists an
intent, installs the file, then commits edition metadata, draft provenance and
thread-scoped request receipts together. Reconciliation checks committed digests
before retaining or rolling back files and finishing source-draft deletion.
`store::documents::Publisher` provides owned drafts, immutable reports, a separate
runtime-only pipeline-draft entry point, newest-first editions, bounded catalogue
and Unicode content cursors. Transport dispatch must supply authenticated task/
thread identities; these in-process methods are not authentication.

`store::migration::restore` requires stopped access, confirmation, private backup
validation and an expected digest. It excludes live Rust Store connections,
checkpoints existing WAL before replacing the database and never rewrites
Markdown/drafts. Migration failures restore the verified logical backup after
transaction rollback. Format-10 migration remains unsupported/fail-closed.

## Lifecycle and public updates

The task and attempt transition graphs follow the existing runtime. Generic
completion and stop-question behavior fail closed until their domain guards
are supplied. Dependency selection and capacity accounting are implemented;
effective scope, retry, worker waits and full scheduler policy remain injected
interfaces. Ambiguous process termination retains capacity.

Generation-bound small deltas have bounded replay (1,024 batches, 8 MiB,
five minutes), 128-frame subscriber queues and explicit resnapshot on gaps,
restarts, conflicting versions or overflow. Large history/report/file bodies
are not broadcast. Snapshot pinning and automatic committed-outbox projection
delivery still belong to the service integration.

Native Core command/query/event DTOs live in `workspace-protocol`, separately
from the unchanged active v1 protocol. V2 negotiation/dispatch, generated
TypeScript/schema assets and desktop cutover are not installed by this library.

## Checks

```sh
cargo build -p cortex-core
cargo test -p cortex-core
cargo test -p workspace-protocol
```

Passing these scoped checks does not qualify a release, a real-client flow,
the full migration, or the running desktop application's behavior.
