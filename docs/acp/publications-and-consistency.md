# Recoverable publications and storage consistency (T019)

This page documents the durable-publication and storage-recovery protocol
in `src/cortex_runtime/store.py` and
`delegation/mcp_tools.py`, and records exactly which T019 scope items are
covered here versus still open. It does not replace or duplicate the
existing storage model; it closes two concrete gaps in the
already-working staged-write/backup/rollback/immutability protocol
described in `store.py`'s own docstrings.

## Publication states

Every Markdown body write (report, pipeline edition) goes through the same
sequence, unchanged by this task:

1. **prepared** — body written to a private `.pending_*.tmp` file, fsynced,
   directory fsynced. For an existing pipeline, a full `.backup_pipeline_*`
   copy of the current committed file is made first.
2. **confirmed** — `os.replace` swaps the temp file into place, the
   directory is fsynced, and the SQLite `editions`/`reports` row is
   committed in the *same* enclosing transaction as every other metadata
   change for that call (`Store._transaction`). Backups are removed only
   after that commit (`Store._finalize_files`).
3. **indeterminate** — a crash between steps leaves either an orphan
   `.pending_*.tmp` (never confirmed; always safe to delete), a pipeline
   file whose digest no longer matches the last committed edition but
   *does* match a surviving `.backup_pipeline_*` (restorable), or a
   pipeline digest mismatch with **no** matching backup, which is left as
   a raised `file_conflict` — never silently deleted or reconstructed from
   prose.

Because the file store and SQLite are two separate systems, there is no
attempt to make them share one atomic transaction; recovery instead
distinguishes these three outcomes on every access.

## Orphan quarantine (this task)

Previously, an orphan `.md` body (file present, no `reports`/`editions` row
referencing it — e.g. a report or pipeline write whose metadata commit
never completed) was deleted outright by `Store._recover_task_files`. It
is now moved into a private, owner-only `<task_dir>/.orphaned/` directory,
named `<original filename>.<sha256[:12]>`, and recorded as a
`task_changes` row of kind `orphan_quarantined`. An identical duplicate
(same name and digest) already quarantined is discarded without ever
overwriting the existing quarantine entry. Quarantine content is **never**
read by any query path (`list_reports`, `read_report`, `read_draft`) and
is **never** auto-published — recovering it is an out-of-band operator
action on the filesystem, not a store operation. An incomplete
`.pending_*.tmp` write is still deleted outright (it is never confirmed
content, so there is nothing to preserve).

`Store.reconcile(task_id=None)` runs this same recovery for one task, or
every task in the store, and returns a structured report:
`confirmed_restored`, `quarantined`, `deleted_temps`, `indeterminate`
(a task left as a raised conflict rather than silently resolved). This is
the same recovery every store call already performs for its own task
before acting (`Store._recover`, called from `Store._transaction`); the
production ACP entry point
(`acp.production_runtime.ProductionOrchestratingRuntime._runtime_for_project`)
now also calls it once, best-effort, per project at first use (recorded on
`self.last_reconcile`), so crash leftovers are surfaced/quarantined
proactively rather than only lazily on the next call that happens to touch
that exact task.

## `publish_result` idempotency and report_ref binding (this task)

`delegation.mcp_tools._worker_publish_result` now enforces two rules that
were previously unchecked:

- **Authorship binding**: `report_ref` must resolve to a report published
  from a `drafts` row whose `owner_thread_id` is this exact worker
  channel's `principal_thread_id` — never a coordinator's report or a
  different worker's report. A mismatch raises `report_not_own`.
- **Canonical publication identity**: a re-delivery of the *same*
  `assignment_ref` (attempt) with the *same* `report_ref` and identical
  `summary`/resolved `checks` returns the existing `worker_result` event
  (unchanged dedup-by-`report_ref` behavior). The same `report_ref` with a
  *different* `summary`/`checks` from the same attempt raises
  `delivery_conflict` — it is never silently overwritten, since
  `worker_result` events are append-only. A *different* attempt
  referencing the same `report_ref` is refused with
  `report_ref_already_published` rather than silently taking over the
  first attempt's canonical publication.

See `tests/test_delegation_mcp_tools.py` (`test_worker_cannot_publish_result_for_a_report_it_did_not_author`,
`test_worker_publish_result_is_idempotent_and_rejects_changed_payload`,
`test_a_different_attempt_cannot_claim_a_report_refs_publication`).

## Fault injection at write_report/pipeline boundaries

`tests/fault_injection_harness.py` runs a real `write_report` call in a
child process, with `cortex_runtime._fault_injection.checkpoint(name)`
calls placed at five named points in `store.py` (`before_temp_write`,
`after_temp_fsync`, `after_backup_copy`, `after_replace_before_commit`,
`after_commit_before_cleanup`); the child sets
`CORTEX_FAULT_INJECTION_POINT` to the target checkpoint and the matching
`checkpoint()` call is the only one that ever fires `os._exit(137)` (no
Python cleanup) — a no-op everywhere else, in every production path.
`tests/test_fault_injection.py` parametrizes both report and pipeline
kinds over every applicable point, reopens a fresh `Store` after the
crash, runs `reconcile()`, and asserts: no confirmed publication is ever
lost (a confirmed pipeline edition's digest survives, restored from backup
when the crash left a stale-but-matching replacement); an unconfirmed
write is invisible (quarantined, or an orphan `.pending_*.tmp` deleted);
exactly one current `pipeline.md` survives with no stray backups; a
same-`request_key` retry publishes exactly once (or, past the commit
point, returns the identical already-confirmed response); and
`PRAGMA integrity_check` is `ok` throughout.

## Observed artifact revision binding

`delegation/revision.py`'s `capture_revision(workspace_root)` is pure,
read-only Python — it never shells out to `git` or any other
repo-configurable program. An earlier revision ran `git status`/
`git rev-parse` against the shared workspace with an isolated env; that
was found to still be exploitable, because repo-local `.git/config`
(`core.fsmonitor`, `core.hooksPath`) and `.gitattributes` (clean/smudge
filters) make plain git plumbing execute an arbitrary program chosen by
whoever last wrote into the workspace — including a delegated worker — no
env isolation defeats that. The capture now has two independent parts:

- **HEAD** — best-effort resolution by parsing `<root>/.git` (a directory,
  or a `gitdir:` pointer file for a worktree, resolved without following a
  final symlink), the `HEAD` file, a loose ref, or `packed-refs`, all
  opened with `O_NOFOLLOW` and small size caps, with strict sha (40/64 hex)
  and ref-name format validation. Missing/unresolvable HEAD leaves
  `head=None` — it never makes the whole capture unavailable.
- **tree digest** (still stored in the `dirty_digest` column, for schema
  compatibility) — a sha256 over a sorted, bounded manifest of every
  file/dir/symlink/other entry under the workspace root (excluding the
  top-level `.git` and `.cortex` dirs), walked with `os.scandir(...,
  follow_symlinks=False)` so a symlinked directory is recorded as a
  symlink and never traversed (no loop/escape risk). Regular files are
  opened `O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC`, immediately `fstat`-checked
  `S_ISREG` (refusing anything else outright, `O_NONBLOCK` cleared only
  after that check) before a normal blocking read, and sha256'd; symlinks
  record their target string; FIFOs/sockets/devices are recorded by type
  only, never opened. This defends a TOCTOU window: a path classified as a
  regular file by `scandir`/`lstat` can be swapped for a FIFO before the
  open runs (a worker with an in-flight write into the shared workspace
  can do this); opening it blocking would hang past the wall-clock budget
  and, transitively, the completion gate — `O_NONBLOCK` makes that open
  return immediately instead, and the `fstat` check discards it. Caps:
  50,000 entries, 512 MiB total hashed, 64 MiB per file, 20s wall-clock
  budget (checked once per directory entry *and* inside each file's read
  loop, so one large/slow read cannot itself exceed the budget); any cap
  exceeded, or any unreadable entry, makes the *whole* capture
  `kind="unavailable"` (reason `workspace_too_large` / `capture_timed_out`
  / `unreadable_entry:...`) — never a partial digest.

  **Symlink boundary note**: `O_NOFOLLOW` only refuses a symlink as the
  *final* path component being opened; it does not sandbox a path whose
  *directory* components are symlinks, and this module does not re-derive
  or canonicalize the workspace root itself against a symlink swap of an
  ancestor directory (out of scope: the root is provided by the caller,
  not attacker-controlled path text). Similarly, `gitdir:`'s target (and
  a worktree's `commondir`) is resolved with `os.path.normpath`/`lstat`
  and required to be a directory, but that resolved path is not confined
  to be under the workspace root — a `.git` file pointing `gitdir:` at an
  arbitrary absolute path only ever causes HEAD to be read from
  wherever it points (a small, format-validated, capped read; HEAD is
  optional and never gates `kind`), it is never used for anything else
  (in particular, the tree digest walk never follows it).

This is intentionally *stricter* than `git status` (a git-ignored build
artifact changing now counts as a workspace change) — a deliberate,
documented trade-off for never executing repo-configurable code.

One narrow engine-housekeeping exception is applied while building this
manifest: a regular, non-symlink `.claude/scheduled_tasks.lock` is omitted
because the Claude ACP runtime may create or remove that transient lock
between worker publication and the acceptance recheck. The exception is an
exact path and type allowlist; a directory, symlink, or other object at that
path remains observable (and a replacement race fails closed). The rest of
`.claude`, including project settings and any other files, remains observable
and can invalidate a revision. It does not authorize ignoring unknown
`.claude` drift.

`capture_revision` returns `kind="observed"` (`head` possibly `None`,
`dirty_digest` the tree digest) or `kind="unavailable"` (`head`/
`dirty_digest` both `None`, `detail` a concrete reason). Captures are
durably appended (never overwritten) to the additive `attempt_revisions`
table (in the current format-12 store; additive migration also adds the
manifest column to older format-12 tables; the table is
created for both a fresh store and, via a `CREATE TABLE IF NOT EXISTS`
migration guard, an existing pre-T019 store).

Observed captures also retain their bounded sorted manifest. Same-turn
direct-worker attribution uses it when the current digest differs from the
worker publication: only a delta consisting entirely of Python bytecode cache
entries (`__pycache__/`, `.pyc`, or `.pyo`) can be reconciled. Missing,
malformed, or any other path delta remains unattributed and blocks the
coordinator turn. The completion gate still requires an exact full digest, so
this attribution does not weaken stale-verification checks.

For direct/serialized shared-root workers, provisioning also records a durable
`worker_start:<attempt_id>` manifest before the engine session is opened. A
publication is accepted as a same-turn attribution only when its current tree
matches and every concurrent shared-root attempt has complete start/publication
manifests with disjoint changed paths. A missing baseline, an unpublished
concurrent mutator, or overlapping A+B path deltas is refused with an explicit
`shared_root_*` reason. This is the strongest evidence available from the
runtime's durable observations; it does not establish which same-user process
performed a write. Isolated snapshot workers retain their existing ownership
and integration path.
When a later shared-root publication is durably complete, its disjoint delta
may explain the current tree for an earlier result; any remaining uncovered
path is still refused as external drift.
Acceptance also refuses duplicate retained `worker_result` events, duplicate
start baselines, or duplicate publication captures for one attempt/report,
even when their payloads are identical. A revision attempt has its own
attempt and report binding, so distinct predecessor/current attempts remain
valid independently.

`OrchestratingRuntime._capture_result_revisions` captures one revision per
`worker_result` event at the moment this runtime claims it (phase
`worker_result:<event id>`), against that attempt's isolated workspace. The continuation text `_coalesce` builds
labels each `worker_result` line with that capture (`revision: <head>
tree=<digest prefix>`, or `revision: unknown (unavailable)`).

`delegation.completion.gate` re-captures the *current* revision of the
same project root (phase `gate_acceptance`) whenever an accepted
assignment's `worker_result` included at least one `evidence="observed"`
check, and compares it against that check's own captured revision (both
`head` and tree digest — a HEAD-only difference with an unchanged tree
still counts as stale): a mismatch refuses acceptance with reason
`stale_verification:<assignment_ref>`. The current capture must be
`kind="observed"`; an unavailable, capped, racy, or otherwise non-observed
current capture refuses acceptance with
`current_revision_unavailable:<kind>:<detail>`, preserving the exact reason
for the incomplete observation. Production worker workspaces are isolated.
After integration, checks must be reconciled to the resulting project
revision; a workspace check alone does not certify a different project tree.

Tests: `tests/test_artifact_revision.py` (real temporary git repos and
plain non-git directories under the isolated test sandbox — never the
real checkout; direct `capture_revision` behavior including proof that a
planted `core.fsmonitor`/`.gitattributes` clean filter is never executed
and no subprocess is ever spawned, tree-change detection, cap/timeout
handling, worktree `gitdir:` and `packed-refs` HEAD resolution, and
`gate()` accept/stale-refusal/unknown-label integration cases).

## Immutability and entrypoint scope (re-verified; one new direct test)

`editions_no_update`/`editions_no_delete` triggers make a published
edition row immutable outside the existing `pending_deletions` tombstone
path (`tests/test_markdown_store.py::test_atomic_rollback_and_immutable_reports`).
`Store._draft`/`_draft_source` scope every draft mutation to its
`owner_thread_id`, and a cross-thread `read_draft`/`write_report` is
refused (`tests/test_thread_context.py::test_draft_is_bound_to_creator_thread_and_many_coordinator_drafts_are_distinct`).
`engine.fs_bridge` refuses every concrete `.cortex/**` technical-storage
subpath — SQLite metadata (including its `-wal` sibling), per-task
report/pipeline bodies, the T019 orphan-quarantine directory, source-block
staging, and draft directories — not only the bare `.cortex` prefix in the
abstract (`tests/test_engine_fs_bridge.py::test_cortex_technical_storage_subpaths_refused_for_read_and_write`,
new in this pass). `delegation.mcp_tools.dispatch`'s defensive
`NO_TECHNICAL_ID_FIELDS` check (previously unexercised,
`pragma: no cover`) is now directly tested against a handler forced to
leak a technical id (`tests/test_delegation_mcp_tools.py::test_dispatch_refuses_a_handler_result_that_would_leak_technical_metadata`).
No gap was found in this pass beyond the report_ref-authorship binding
already closed above.
