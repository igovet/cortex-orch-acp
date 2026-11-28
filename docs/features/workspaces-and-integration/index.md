# Project scope and filesystem effects

Linked worktrees share the logical project's [private home store](../../project/storage.md#where-the-database-lives-and-how-it-moves) for their channel.

> Project binding and integration below are grounded in the Rust source. Detailed claims about host behavior still need independent verification where no source or recorded observation is cited.


Worker changes normally target the canonical project root for incoming ACP or project-folder execution. The Workspace service can also select an [assignment-level worktree](#assignment-level-worktree-custody) for eligible work. In Workspace's [separate worktree mode](../../cortex-workspace/local-task-execution.md#separate-worktrees), they target the verified linked checkout instead. File and command evidence cannot cross checkout roots, even when the file bytes match.
There are no hidden candidate copies or model-controlled integration operations.
Workspace can dispatch a writer in its own linked checkout while another task's
independent review reserves the project folder. The runtime requires disjoint,
literal scopes on existing files, a clean committed project checkout, a supported
engine route and retained duration estimates. The predicted overlap must exceed
the conservative checkout cost derived from observed worktree provisioning and
completed worktree verification. Small nodes and cases without enough evidence
use the selected execution checkout when its reservation allows.

The assignment worktree journal stays in the logical project's existing channel
store. It
records an intent before Git provisioning and retains the verified checkout
identity. Its bounded integration operation applies modifications of tracked
regular files to the canonical checkout only after the engine exit is confirmed.
It rechecks the write scope, target state and file leases, and records success
only after reading back the resulting files. Conflicts keep both checkouts intact.
Unresolved effects block dependents, review, verification and completion. Final
review and verification run on the canonical checkout with the integrated changes.
In Workspace, [closing a sprint](../../cortex-workspace/local-task-execution.md#editing-saved-tasks-and-closing-sprints) archives the sprint and its tasks without changing checkout files or branches. Incomplete started work, disputed completion and an ongoing merge or pull-request operation block closure. Drafts can be archived without starting, and existing purge safeguards still protect unmerged work.

Creating a sprint task first saves the draft, then links that same task to the
sprint. If the link is not confirmed, refresh the project and check the saved
draft before retrying. Replaying the same create request returns the same draft;
linking its existing membership again does not add a duplicate. Linking has
four machine-wide slots separate from general machine actions; it still requires
the machine-state lock and a valid sprint and task. Draft creation and active
model/effort settings use [reserved writer capacity](../../project/storage.md#storage-and-recovery)
without changing the order of accepted writer work. Live model and effort
choices take effect at the next use, not in a prompt already in progress.

The runtime reserves dependency-ready scopes and runs at most four workers in
one chat session across all engines, roles and connections. Excess dependency-
ready nodes remain pending until a worker slot is free. A connection's
`max_concurrent_sessions` limit (default 4, configurable from 1 through 64) can
further lower the session's worker capacity on that connection; coordinators
are never limited and never take a worker slot. Overlapping write scopes are
serialized. When execution finishes with recorded changes or incomplete change
evidence, new writers in the same checkout queue while active writers drain for
code review. The reservation lasts through verification and final reconciliation.
Verification that runs host commands needs a stable checkout even without
recorded changes. Sprint plan reviews, sprint revision reviews and reviews with
no project changes to certify do not wait for writers or take a new reservation.
A sprint revision review preserves an existing reservation if the task's current
revision has already passed host verification. Writers in separate worktrees can proceed. External digest drift during verification starts fresh independent review within the review-cycle limit. Each observed command output remains in the original run. A finished worker keeps its session slot until its
engine is confirmed gone and the reservation is released or safely reclaimed.
A finished, failed or cancelled worker's write scope is likewise held until
its engine's reservation is released on a confirmed exit (or every delegation
row of its attempt is terminated with no reservation left unreleased). A node
of a finished (completed, cancelled or failed) task, or of a task being
deleted, holds no files at all, whatever rows its attempt left: its channels
are revoked and its nodes never run again. Rows written by earlier builds are
read the same way (`holdings_tx` in `crates/cortex-core/src/scheduler/policy.rs`),
so a dead worker of an abandoned task never blocks new work. A dependency-ready
writing step whose files another unfinished task's worker still holds waits
for that holding, with one `NATIVE_WRITE_CONFLICT_WAITING` record naming the
holder task, the step and the overlapping paths
(`crates/workspace-service/src/native_runtime.rs`), instead of stopping its task
with `native_continuation_unavailable`. The same holds for a step queued behind
another task's open access request. While it waits, the Working card and opened
task read "Waiting for the task “<title>” to release <files>" (projection warning
`waiting_for_files`), using the title shown on the holder's card when available.
The step starts by itself once the holding ends; the owner is not asked. A stop that does name `native_continuation_unavailable`
says, from the scheduler's own rules (`claim_obstacles_tx`), which step could
not start and why: the files and the owner's `deny-write:` rule that denies
them, an earlier step of the same task whose engine exit was not confirmed, the
task holding the files, a retry pause, or that no reason was recorded.
Terminal task and unexpected prompt-error cleanup also stop owned engines
without requiring manual stop. Partial-launch writer locks remain held through
shutdown and stay quarantined when exit cannot be confirmed; a leader exit alone
does not release a group with active children. See
[cleanup boundaries](../failures-and-cancellation/index.md#terminal-resource-cleanup).

All project operations by Cortex MCP tools use the authenticated runtime gateway.
Native-tool disabling is engine-specific, not a uniform guarantee: Claude's native
`Bash`/`Read`/`Edit`/`Write`/`Grep`/`Glob`/`NotebookEdit` are refused through
`disallowedTools` (see [claude-engine](../claude-engine/index.md)), so its project
reads and writes stay fully observable through the gateway. Codex disables native
`shell_tool`, but native `apply_patch` remains available, and the installed Codex
CLI exposes no supported switch to turn off `unified_exec` either; both stay native actions, visible
only through partial ACP tool-call events rather than the gateway (see
[codex-engine](../codex-engine/index.md)'s current runtime gateway policy).
Coordinator, consultant and review roles are read-only; other
workers may write only within their assigned scopes. File replacement checks an
expected hash or absence and rejects private paths and symlinks. A hard-linked regular file inside the project reads normally and a write replaces its directory entry atomically without touching the shared inode. `project_write` also takes `encoding: "base64"` for binary content (decoded size at most 750000 bytes); command `changed_paths` are attributed project writes too, and `project_edit` treats an unchanged result as success and applies when the old text still matches uniquely.
Direct private/Git/denied-path overlays are not a guarantee against arbitrary host
code, outside aliases or Docker. A project digest does not cover outside resources.
Auto grants writing workers full host execution. Development and Guarded use the broker's Landlock rules for writes. Reviewer and verification commands run with restricted writes. Verification can write to eligible existing Git-ignored directories. Command environments filter Cortex/credential variables but
use owner toolchains, validated SSH-agent access and shared network. `command_run`
defaults to 1800 seconds (30 minutes; `timeout_seconds` sets more); outside commands
default to 600 seconds. On a timeout, stop or authority loss the command and every
process it started are stopped, including a child in its own process group (Playwright's
detached `webServer` once kept its port after a timed-out run). A command's stdin is
`/dev/null`: a killed Node child once left the shared MCP-server stdin non-blocking and
every later Cortex call of that worker failed with "Transport closed" (2026-10-22). Declared scope is coordination, not the writing worker's whole-project
OS grant. See [access modes and limitations](../permissions-and-containment/index.md#access-mode-development-guarded-and-auto). Ambiguous effects retain
an unresolved receipt and cannot be replayed as though nothing happened.

External editors can still change project files. Review binds the complete observed
project digest from dispatch to verdict, verification and completion. Drift blocks
acceptance; a digest does not prove which OS process authored a change. Unsaved
editor buffers are not inferred from an absent client capability.

Project snapshots bind review and verification to the current checkout. See the [Core workspace implementation](../../../crates/cortex-core/src/workspace.rs) for path handling and revision capture.

Fixed read-only Git status/diff commands provide retained review context, with
external diff, text conversion, hooks and fsmonitor disabled. The diff includes
pre-existing edits and is not an authorship claim. Model commands cannot read `.git`.

## Code lease and managed file effects

A local desktop Code lease prevents overlapping writes through this project's
structured file tools (`project_write`, `project_edit`, `project_delete`,
`project_mkdir` and `project_skill_link`) in the same channel. A conflicting
effect is refused before mutation with `owner_file_lease_held`. The scheduler
waits with that reason when managed writer admission overlaps an active owner
lease. Disjoint file-scoped work need not wait. Code saves validate the lease
and disk revision. This does not constrain host commands, outside writes from
other projects, native engine writes, Git or external editors and terminals.
See [Code editing and write leases](../../cortex-workspace/ide-and-git.md#code-editing-and-write-leases).


## Integration

Workspace owner actions operate on the registered checkout or the verified task worktree. The service implements merge, squash and pull-request actions with retained admission and outcome evidence. See [separate worktrees](../../cortex-workspace/local-task-execution.md#separate-worktrees), [IDE and Git](../../cortex-workspace/ide-and-git.md) and [service integration handlers](../../../crates/workspace-service/src/machine/integration.rs).

### Assignment-level worktree custody

The Workspace service can provision a linked checkout for a writing assignment that would otherwise wait behind another task's active review of the canonical checkout. The scopes must be disjoint. Selection uses retained worker and reviewer durations and a conservative cost from matching checkout preparation and verification measurements. The project must have a clean committed checkout, and the selected scope must consist of bounded literal tracked files. Missing timing or unsupported host or engine evidence leaves the ordinary shared-checkout wait in force.

The service records the intent before Git runs and binds the observed checkout to the exact assignment, plan revision and owner epoch. The worker uses that checkout for scoped effects. After a completed assignment, the runtime validates and transfers its bounded patch under the repository and destination write fences. It checks scope, base and destination evidence before applying. Dependent work remains blocked until integration settles. Conflicts or uncertain effects retain the checkout and custody record, prevent automatic replay, and block completion. Task-level worktree controls remain separate. See [Core selection and integration](../../../crates/cortex-core/src/workspace_worktrees/assignments.rs) and the [service launch and settlement](../../../crates/workspace-service/src/machine.rs).
