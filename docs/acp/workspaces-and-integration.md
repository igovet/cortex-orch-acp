# Project-root writers and safe integration

## Current ACP contract

The project opened by the ACP client (Zed today) is the worker destination.
Every mutating attempt is bound to that exact `project_root`; Cortex does not
silently substitute a hidden snapshot, temporary checkout, or worker copy.
The coordinator's working directory is also that exact current project root.
It must see worker edits made during the same session; a startup copy can give
it stale source and test results. This does not grant the coordinator a Cortex
writer lease or make its native tools OS-level read-only.
Workers acquire a serialized writer lease, and the runtime records the lease
owner, ownership epoch, input revision, affected paths, output revision and
write outcome in the project-local `.cortex` store.

Immediately before mutation, Cortex rechecks the binding, current project
revision and any negotiated unsaved editor buffers. Immediately after the
write it captures the resulting revision and records the observed filesystem
delta. A stale revision, overlapping Cortex writer, unavailable binding,
partial write or uncertain process result is a durable failure/conflict. It is
never reported as successful completion and never redirected to another
directory. The writer lock serializes Cortex-owned workers only: it cannot
prevent Zed, the user, or another external process from editing the project.

The direct integration operation is reconciliation of the already-authorized
project root. When the recorded attempt delta is already present it returns a
durable `noop`/reconciliation result; it does not apply a hidden candidate.
When no host `fs` capability is negotiated, `host_buffer_state` remains
`unknown`. That is an explicit evidence limitation, not proof that there was
no unsaved Zed buffer and not a strict prevention guarantee. A coordinator
must keep that limitation visible when deciding acceptance.

Checks run against the exact project revision associated with the attempt and
retain command observation separately from the coordinator's semantic
assessment. A report, tool return, or process exit code alone is not proof that
the user's requirement is satisfied. Native check commands use the user's OS
permissions; the runtime does not enforce filesystem read-only or network
isolation for them. The coordinator decides whether to
accept, revise or stop after reading the durable report and current evidence.

The Cortex writer lock coordinates Cortex processes only. It cannot prevent an
unrelated editor or process from changing the same directory between two
observations; such drift is recorded and blocks acceptance until reconciled.

The coordinator-turn revision guard accounts for a direct-project worker that
edits while the coordinator prompt is still running and publishes later. Cortex
records the unresolved event boundary in the task store and waits for that
worker's exact observed final tree before allowing acceptance. A failed worker,
missing publication, unavailable capture or differing current tree leaves the
task blocked; a pause or restart does not erase the pending boundary. This is
observed revision reconciliation, not proof of which same-user OS process made
each write. Native coordinator tools remain outside a preventive filesystem
boundary, so their writes cannot be ruled out solely by a matching digest.

The remainder of this page documents the pre-direct-write snapshot/candidate
implementation retained for migration and audit of existing task stores. It is
not the destination model for new ACP worker attempts.

# Historical snapshot and candidate implementation (migration/audit only)

The following T016/T025 material describes the former snapshot-based
implementation. It is retained to explain migration of existing stores and
old evidence; it is not the current production destination contract above.

## Workspace model

Every mutating assignment's attempt gets a `WorkspaceBinding` (see
`delegation.provisioning.WorkspaceProvisioner`, `portable.identity
.WorkspaceBinding`): a workspace id, a root path, a known base artifact
revision (T019 `delegation.revision.capture_revision`), a resource owner
(the attempt), a change-transfer method, and a logical `.cortex` binding
that always resolves to the *main* project's own store (see below).

### `IsolatedSnapshotWorkspaceProvisioner` (default)

`delegation.workspace_isolation.IsolatedSnapshotWorkspaceProvisioner
.create(attempt_id)` makes **two** owner-private copies of the project tree
under this project's own `.cortex/workspaces/<attempt_id>/`, excluding the
top-level `.git`/`.cortex` directories (matching `delegation.revision`'s
own exclusion):

- `base/` — a pristine, read-only-for-the-rest-of-the-attempt's-lifetime
  copy, used later as the three-way merge's base text.
- `work/` — the mutable copy an engine actually receives as its workspace
  root (`cwd`/`fs_bridge` containment root) and freely writes into.

Only plain `os.scandir`/`open`/`read` calls ever touch the *real* project
root while copying — a user's uncommitted/dirty files are captured exactly
as they are on disk, never reset or stashed. A manifest (`{relpath:
{kind, sha256|target, mode}}`) and a T019 revision observation of `work/`
are recorded in the additive `workspace_snapshots` table (`user_version`
unchanged at 11). Caps (50k entries / 512 MiB total / 64 MiB per file / 20s
wall-clock — the same numbers `delegation.revision` already uses) make an
oversized or unreadable tree fail the whole snapshot atomically (never a
partial, silently-incomplete copy).

Two attempts delegated concurrently each get their own independent pair of
copies: a write into one is never observable from, or able to overwrite,
the other's copy or the real project tree — only an explicit, later
`delegation.integration.integrate` call ever touches the main tree.

### Snapshot location vs. `fs_bridge`/`terminal_bridge` containment

The snapshot's `work` directory lives *under* the main project's own
`.cortex/workspaces/<attempt_id>/work` — a descendant of the very
`.cortex` directory `engine.fs_bridge.FsBridge`/
`engine.terminal_bridge.TerminalBridge` refuse a path/`cwd` for containing
a `.git`/`.cortex` *component*. Verified (see
`tests/test_workspace_fs_bridge_containment.py`): both
bridges' checks are always computed *relative to* the bound
`ResourceBinding.workspace_root`, never by walking workspace_root's own
ancestor path components — a path inside the snapshot's `work` dir
(`os.path.relpath(path, workspace_root)`) never contains a `.git`/`.cortex`
segment itself (the resulting relative path is just `a.txt`, `sub/b.txt`,
...), so a worker bound to a snapshot root can read/write its own files
and run terminal commands normally. The *main* project's real
`.cortex/cortex.sqlite3` is a sibling subtree, not a path under that
binding's `workspace_root`, so plain containment (not the technical-storage
check) already refuses it — confirmed explicitly by test, not merely
inferred.

### Snapshot failure

Production delegation uses `IsolatedSnapshotWorkspaceProvisioner`. If its
entry, byte, or time cap is exceeded, or an entry cannot be read safely,
workspace creation fails. It does not switch to a shared project root.
`SerializedSharedRootWorkspaceProvisioner` remains a separate legacy class,
but is not an automatic production fallback; its existence does not make
shared-root worker writes an acceptable isolation substitute.

## Candidate staging before integration

`prepare_candidate` requests an isolated stage built from exact finished
assignment attempts. The main runtime copies the current project into a
candidate and applies those selected attempts there. The candidate records
source bindings, a project-base digest, content digest, and any conflict
paths. `candidate_ready` or `candidate_conflict`/`candidate_failed` appears
through `list_events`. Preparation does not write the user's project.

The coordinator may call `request_check` for a ready candidate and exact
content digest. The runtime runs its direct argv in a read-only candidate
sandbox, retains a bounded command observation, and reports unknown outcomes
without replaying the command. `read_check` returns that observation;
`assess_check` records the coordinator's separate interpretation. The
coordinator may then call `integrate` with the exact `candidate_ref` and
`expected_base_digest`.
The main runtime checks that binding and the current project base before
application. A changed base or overlapping path is a conflict, never a
silent replacement. Candidate selection remains a coordinator decision.

## Integration

`delegation.integration.integrate(store, task_id=..., project_root=...,
assignment_ref=..., request_key=..., attempt_id=..., workspace_binding_id=...,
host_fs_checker=None)` is the one technical-application entry point,
idempotent per `(assignment_ref, request_key)` (`workspace_integrations`
table — a retry returns the exact previously recorded outcome, never
re-applies or re-decides).

### Coordinator MCP operation

A coordinator channel calls the `integrate(assignment_ref, request_key)`
MCP operation (`delegation.contracts.COORDINATOR_OPERATIONS`,
`delegation.mcp_tools._coordinator_integrate`). Matching every other
delegation operation's process-boundary rule (see `delegation.mcp_tools`
module docstring), the MCP-subprocess-side handler only records a durable
`progress` event (`payload["kind"] == "integration_requested"`) — it never
touches the project tree itself. The *main* process's
`acp.orchestrating_runtime.OrchestratingRuntime._process_new_events` picks
up that event on its next pass (the same polling seam `delegation_requested`/
`assignment_cancel_requested` already use), calls
`delegation.integration.integrate` for real, and appends the outcome as its
own coordinator-visible `progress` event —
`payload["kind"] == "integration_applied"` (status `applied`/`noop`, with
`applied_paths`/`integrated_head`/`integrated_dirty_digest`) or
`integration_conflict` (with the full `conflict_paths` report). These are
carried as `progress` rows rather than a new `task_events.event_type`
because that column has a DB-level `CHECK` constraint fixed at table
creation — widening it would need a destructive rebuild for every existing
pre-T025 store, which this task does not do (see
`delegation.events.INTEGRATION_PROGRESS_KINDS`). `list_events` and the
continuation-text renderer both surface these two kinds explicitly.

Algorithm:

1. Diff the attempt's base manifest against a fresh rescan of its own
   `work/` copy → the worker's own change set (added/modified/deleted).
2. For every changed path, read the main project root's *current* state:
   - unchanged since base → apply the worker's version directly.
   - a host-negotiated unsaved buffer for that exact path (see below) →
     refused, regardless of any merge outcome.
   - changed since base, non-overlapping, both sides regular UTF-8 text →
     a conservative line-level three-way merge (`difflib`-based; any edit
     range that even touches the other side's edited range is a conflict,
     never a guess at whether it would have agreed).
   - anything else (binary content, kind mismatch, delete/modify, a
     directory or symlink change, an add/add divergence) → a conflict.
3. Every path is checked *before* any write; one conflicting path means
   **nothing** is written at all (pre-check, all-or-nothing to the extent a
   plain filesystem allows — see the module docstring's one documented gap:
   applying more than one already-conflict-free file is still not one
   atomic filesystem transaction).
4. Each accepted write is its own atomic temp-write + `fsync` + `rename`
   (mode preserved), never in place.
5. The resulting project root's own T019 revision is captured and recorded
   as this integration's `integrated_head`/`integrated_dirty_digest`.

No path is ever resolved by "last write wins" — an overlapping edit,
binary difference, or unsaved host buffer always surfaces as a conflict
report (`{relpath: {reason, ...}}`) to the caller instead.

## Host buffers

`host_fs_checker(relpath) -> bytes | None` is an optional, caller-supplied
read of the *host's* own negotiated buffer for one project-relative path
(see T014's `engine.fs_bridge.HostFsForwarder`, only ever active for a
host-visible coordinator binding). When it returns content that differs
from the path's current on-disk bytes, that path is refused
(`unsaved_host_buffer`) — an old engine-side result is never allowed to
silently overwrite newer, unsaved editor content.

`OrchestratingRuntime` accepts an optional constructor-level
`host_fs_requester` — the exact same `(method, params) -> dict` shape as
`acp.host_requests.HostRequester`, following the identical "`None` means
not negotiated/available, never guessed" convention as the existing
`host_elicitor` (T020) — supplied only once a real incoming ACP connection
has actually negotiated an `fs` capability. When present,
`_make_host_fs_checker` builds the per-path check by reusing the existing
T014 `engine.fs_bridge.HostFsForwarder`/`RevisionLog` abstraction directly
(never a bespoke second wire protocol), and every recorded integration's
`host_buffer_state` is `"checked"`. When no requester is supplied (the
host never negotiated fs, or no live connection is wired to this runtime
yet), `host_buffer_state` is durably recorded as `"unknown"` — a visible
limitation, not silently treated the same as "checked, no conflict" (see
T014's own report: "host buffers vs disk revisions... capture only;
integration later", now closed for the case where a host connection is
actually wired). Live wiring of a real ACP host's negotiated `fs`
capability into `cortex_acp.py`'s own construction of
`OrchestratingRuntime` remains T035–T037's scope; this task delivers and
tests the full production seam (constructor parameter through to the
`HostFsForwarder` call) with a fake forwarder function standing in for the
real host connection.

## Revision binding and re-verification

`acp.orchestrating_runtime.OrchestratingRuntime._capture_result_revisions`
now resolves each worker_result's own isolated `work/` root (via
`delegation.workspace_isolation.resolve_attempt_root`) when the attempt
used isolation, instead of always the task's shared project root. Because
the T019 tree digest is computed over *relative* paths/content, a
worker's isolated final tree and the main project root after a clean,
non-conflicting integration of that exact tree are byte-for-byte
comparable: identical digest if nothing else changed, a different digest
the instant integration merges in a concurrent main-root change or any
later drift. `delegation.completion.gate`'s existing `stale_verification`
comparison (T019) therefore already refuses to let a revision-A report
certify a differing revision B, with zero changes needed to the gate logic
itself — only to what revision worker_result binds to.

## Cleanup

`IsolatedSnapshotWorkspaceProvisioner.cleanup_binding` removes only a
Cortex-owned snapshot (`base/`+`work/`, verified owner-private, never a
symlink) after its own row is looked up by binding id — it never touches
`project_root` or any user-owned path. `acp.orchestrating_runtime
.OrchestratingRuntime._cleanup_isolated_workspace_if_integrated` calls it
automatically once an attempt is both terminal (T017's own
`_terminate_completed_workers`) and either an integration outcome was
durably recorded for it or the assignment was cancelled outright.
`cleanup.finish_deletions` (task-level retention) removes any remaining
`workspace_snapshots` directories before deleting the DB rows, so a
`/cortex-clear` never leaks a snapshot directory either.

## Tests

`tests/test_workspace_isolation.py`, `tests/test_workspace_integration.py`,
`tests/test_workspace_fs_bridge_containment.py`,
`tests/test_workspace_mcp_integration.py`.
Covers: two workers editing the same file in isolated workspaces (second
integration conflicts, first worker's applied content never overwritten) --
both at the `delegation.integration.integrate` function level and, in
`test_workspace_mcp_integration.py`, end to end over the real coordinator
`integrate` MCP operation and `OrchestratingRuntime` (idempotent dispatch,
coordinator-only enforcement, applied/conflict outcomes appearing as
durable `progress` events); a non-overlapping concurrent main-root change
merging cleanly; a conflicting overlapping edit surfaced, never
last-write-wins; dirty/uncommitted user files surviving snapshot and
integration untouched; an unsaved host buffer blocking an otherwise-clean
overwrite, both directly and through the real `host_fs_requester`
production seam with a fake forwarder (plus `host_buffer_state: "unknown"`
when no requester is wired); integration idempotency by `request_key`; a
worker `ResourceBinding` rooted at the snapshot `work` dir actually
reading/writing through `FsBridge` and running a contained terminal
command, while the real project `.cortex` store stays unreachable; a
worker's own execution files excluding `.cortex` while a real publish still
lands in the main project's logical store; revision A not certifying a
differing revision B (both the "integration hasn't happened yet" and the
"main drifted after a clean integration" cases); the separately testable
legacy serialized-writer class (not a production fallback); and cleanup
removing only the Cortex-owned snapshot
directories it created.
