# ACP orchestration

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed. The former Python runtime was removed on 2026-11-16: references to its modules and tests were dropped, and statements that cited them describe behavior carried from that implementation that has not been re-verified against the Rust runtime.


Cortex is the incoming ACP agent and the owner of orchestration. ACP transports
messages and sessions; the models propose plans, solve assignments and synthesize
results. The runtime chooses what executes and whether the task can complete.
See [verification status](../../project/verification.md) for retained test evidence
and its automated evidence limits.

## Runtime-composed synthesis

Independent review and final verification remain separate mandatory gates for
changed work. After their first-pass success on the same current digest, the
runtime can compose metadata plus attributed report-summary quotations and
complete without a coordinator reconciliation turn. Any open evidence gap,
pending decision/input, repair or historical replan uses the ordinary coordinator
path. No-change review waivers also retain that path. The runtime does not infer
semantic conclusions from reports or translate quoted summaries.

See [runtime-composed lifecycle](../task-lifecycle/index.md#runtime-composed-synthesis)
for exact eligibility, atomicity, recovery and display limits. The distinct
`SYNTHESIS_RUNTIME_COMPOSED` event identifies this mechanism; it does not invent
a coordinator turn or token observation. A decline instead records a
metadata-only `SYNTHESIS_RUNTIME_DECLINED` event with a deterministic reason
code (see [runtime-composed lifecycle](../task-lifecycle/index.md#runtime-composed-synthesis)
and [observability's cohort aggregates](../observability/index.md)); eligibility
itself is unchanged.

## Review after a replan or reopen

The review packet lists paths changed against the current execution baseline and paths recorded by this task across its plans. For each listed task write, the packet identifies its latest event, node and plan revision. A matching SHA-256 confirms the currently observed bytes. A mismatch calls for a fresh read. A missing comparison or incomplete baseline remains explicit. The reviewer examines the objective and material changes. Each listed file is marked `required` when this task changed it (see [review of the task's own changes](#review-of-the-tasks-own-changes)); approval and verification bind those files' bytes (the whole checkout only when the task changed more than 20,000 files).

## Review of the task's own changes

A shared checkout holds other actors' uncommitted work: the owner's edits, other
tasks, other tools. The execution-baseline difference lists all of it, so a
review that had to read every listed file, and a binding to the whole checkout,
were refused or re-bound by changes no worker of the task made (owner store
2026-11-20: a packet of 121 changed files, 83 of them unrelated, three reviews
refused `review_file_coverage_required` for up to 32 unrelated files, and one
`review_project_revision_changed` from an unrelated edit). Review now covers the
task's own changes.

**Attribution.** A file has a writer from this task when:

- a gateway file tool of one of the task's channels wrote, edited, moved or
  deleted it (`NATIVE_PROJECT_WRITTEN`, `NATIVE_PROJECT_CHANGED`, across plans);
- it changed while one of the task's commands ran. Around every writable
  `command_run` and every owner-confirmed `outside_command`, the host lists the
  checkout's dirty deliverable paths before launch and after the process ended
  (`git status --porcelain=v2 -z --untracked-files=all --no-renames`: tracked
  changes and untracked files, never Git-ignored ones, each with its status
  record plus size, mtime, ctime, mode and inode; a folder outside any repository
  is walked instead). Clean files are neither visited nor hashed. The paths whose
  entry differs are recorded on the command's `NATIVE_COMMAND_FINISHED`
  observation (`changed_paths`, `changed_paths_complete`,
  `crates/workspace-service/src/native_change_set.rs`). A read-only `command_run`,
  `project_git` (status or diff only), a runtime verification run (deliverable
  files mounted read-only) and a payload that provably never started record an
  empty, complete list without listing;
- it changed during a turn of a writing worker whose engine could change files
  without a receipt. The same listings bracket each such turn
  (`NATIVE_WRITER_TURN_STARTED`, `NATIVE_WRITER_TURN_CHANGES`; `NATIVE_TURN_SETTLED`
  carries `writer`). codex-acp (`workspace-write`) and OpenCode can write inside
  their own sandbox without asking, so every writing turn of theirs counts
  (`native_mutation: true`); Claude Code runs with its native shell and file tools
  disallowed and asks for every other tool, so its turn counts only when the
  runtime allowed a request in it other than a Cortex gateway tool or a read.

A path that changed during a command or a counted turn is attributed to that
node even when another actor changed it at the same time: attribution is
conservative, never narrowed by guessing who made a concurrent edit. The packet
names the writer in `by`, marks the file `required: true`, and counts
`changed_files.required`; `change_boundary.command_changed_paths` counts the paths
attributed this way.

**Coverage.** `review_coverage_missing_tx` asks only for required files. Every
other changed file stays listed with `required: false` as context, with the
packet's guidance that it has no writer from this task, and is never a missing
item, an outside-read-scope item (`outside_read_scope` lists required files only)
or a `Changed files without reviewable text` limit. A listing longer than the
packet's 200 entries keeps the task's own changes first; `more.required` counts
the required files beyond the list, and only those become the `Changed files
beyond the review list` limit. Deletions, ignored files, binary, image and opaque
content, long-line segment coverage and `review_lines` ranges are handled as
before, for required files.

**Binding.** The host records the packet's binding with the recorded packet
(`REVIEW_PACKET.binding`, not in the reviewer's brief) before the review starts:

- `{"kind": "task_files", "paths": [...]}`: every required path. `REVIEW_STARTED`
  (`binding: "task_files"`), each re-binding, the verdict's `review_digest` and
  final verification and completion bind a digest of those paths' current states
  only: content hash and mode, link target or `absent`, identified by the host at
  each observation (`EvidenceSnapshot.task_files`; past a 256 MiB hashing budget a
  file is identified by its metadata including ctime). An edit of a required file,
  by anyone, still refuses the verdict with `review_project_revision_changed` and
  re-binds, and after approval still refuses verification with
  `verification_review_revision_changed` and schedules a fresh review. An edit of
  any other file neither refuses nor re-binds a review and does not invalidate an
  approval or a verification. A bound file whose state the host did not supply
  makes the binding unavailable (`review_revision_unavailable`), never satisfied.
- `{"kind": "checkout", "reason": ...}`: the whole checkout, exactly as before.
  This is the conservative fallback whenever attribution is incomplete: a command
  or counted turn since the baseline without a complete change record (a receipt
  or turn from an earlier build, `command_without_change_record`,
  `turn_without_change_record`; a listing that failed, `listing_failed`, or
  exceeded its bounds, `too_many_paths` at 200,000 dirty paths or 64 MiB of
  listing, `too_many_changed_paths` past 5,000 changed paths in one run; a nested
  checkout, `nested_checkout`; a run settled without listings after a restart,
  `settled_without_listing`; an unknown outcome, `outcome_unknown`,
  `turn_outcome_unknown`; a command or turn started and never settled,
  `command_unsettled`, `turn_unsettled`). The packet then sets
  `changed_files.attribution_complete: false` with `attribution.reason`. The
  required set stays the task's recorded changes and the review binds only those
  files; unattributed files stay context, and `attribution.note` tells the
  reviewer so (owner store 2026-11-21: one
  writer turn with an unknown outcome had made all 197 changed files of a dirty
  checkout required, 92 of them other actors' work). A task-file
  set past 20,000 paths also binds the whole checkout (`too_many_task_files`) while
  requiring only the task's files. A packet recorded by an earlier build has no
  binding and binds the whole checkout.

Verification compares the same binding: `VERIFICATION_PASSED.revision_digest` and
each runtime check's `revision_digest` are the approving review's binding, and
`checkout_digest` records the whole checkout the commands ran in (it is what a
worker's check receipt, the presented result identity and an owner's check rerun
compare). The runtime still runs the plan's verification commands against the
real checkout, so a breakage another actor's change causes still fails there.

The guarantee and its limit are exact. Approval and verification certify that the
bytes of every file this task wrote or changed are the reviewed bytes, and that
the plan's commands passed on the checkout as it was when they ran. They do not
certify other actors' concurrent changes: those were not reviewed, and a checkout
that also contains them was verified, not reviewed. Attribution sees what the
listings see: a path changed and restored to the same status record and metadata
within one run, a Git-ignored file and the inside of a nested checkout are
invisible (a nested checkout makes the record incomplete); between two turns of a
writing worker no listing runs, so an engine-native background process that
outlives its turn and changes a file before the next turn is not attributed.
Very large dirty checkouts make attribution incomplete rather than slow: past the
listing bounds above the review falls back to the whole checkout, and an
observation of the whole checkout keeps its own bounds.

## Reviewer verification reuse

The gateway's `command_run` can reuse a runtime-produced verification receipt
only for the task's current, running review attempt and a byte-identical
`plan.verification` argv on the same project revision digest. Completed checks
return their actual exit code (including nonzero), completeness, digest and
bounded output reference; output is paged with `read_large_output`, not copied
into the response. A scoped metadata alias points to the same immutable output
body. The receipt's `runtime_verification` records the original runtime command
and output identities; accepted `result_submit` check evidence retains
`evidence_source=runtime_verification` and `runtime_command_ref`.

An in-flight check is polled outside the write transaction for at most five
seconds, or the shorter requested timeout. Runtime shutdown or revoked channel
authority ends the wait without launching another command. Shutdown commits a
runtime-only cancellation marker for owned tasks before engine/broker teardown,
so a gateway in a separate MCP process observes it too. If storage cannot retain
that marker, shutdown still tears down the owned engine and MCP server; no
successful wait cancellation receipt is inferred from the missing marker.
Different argv/digest,
a discarded or stale run, missing/incomplete/truncated output, launch failure or
wait expiry uses normal command execution and its existing sandbox policy.
Implementation workers are unaffected. Reuse validates exact runtime start/finish
events and the retained output body; a model assertion cannot supply provenance.
Two aliases of one runtime command cannot verify two checks of one result.
Failure-closure ordering uses the original runtime execution start, not the later
alias request, so returning an old receipt cannot masquerade as a new rerun.

This is observation reuse, not final acceptance: speculative checks remain
candidates until the existing controller credits the approving reviewer and
unchanged digest. Independent diff analysis, review approval, worker teardown,
mandatory verification and synthesis gates are unchanged. No storage-format or
permission change is introduced. Implemented in `crates/cortex-core/src/lifecycle` and the `workspace-service` native host.

## Plan submission and coordinator polling

Plan task `scope` is a write reservation used to schedule overlapping writers,
not a read-access list. Reading needs no scope; writing tasks must not list
read-only dependencies there. Read-only/investigation roles retain the required
non-empty context scope, but it neither reserves writes nor serializes writers.
The existing access policy, not scope wording, decides mutation authority.
Reservations use one lexical canonical form: leading/interior `./`, repeated
separators and trailing slashes collapse; `.` still denotes the project root
and glob syntax is preserved. Plan admission, scope requests, access grants and
comparisons of older stored reservations share that identity. This does not
resolve symlinks or add a filesystem security boundary.


The advertised `plan_submit` contract requires `plan.tasks` to contain 1–64 items and
`plan.verification` to contain 1–16 items; empty arrays are rejected. A
verification command is required for every plan, including an investigation-only
or trivial plan, because runtime verification still runs when no project change
is observed. Copy commands exactly from documented project verification. If the
project documents none, provide the smallest literal-argv command that fails
when the requested result is wrong, using project files and standard tools; for
an investigation-only plan, check the investigated files. `plan.verification`
is the one place the project's whole documented suite runs; worker objectives
name only the tests covering each task's change. Shell operators,
pipes, redirection and substitution are not accepted. When Codex renders the
MCP schema as TypeScript, the required count and meaning are repeated in the
array's own description because it may omit JSON Schema item/count constraints
and nested item descriptions. A count-validation correction repeats that field
description, including the no-documentation fallback.

Each task objective is bounded to 1–8000 Unicode code points. The objective
property description repeats this bound for description-only host rendering.
Measure the complete value before submission; concisely rewrite overlong text
without dropping requirements or silently truncating it. Rejection retains no
accepted plan or worker reservation. Codex and Claude receive the same channel
MCP schema; this does not guarantee model compliance.

Plan model-policy preflight failures retain safe portable classification, rather
than becoming uncertain storage errors. Authorization and identical key/payload
replay precede current policy reads; accepted replay does not revalidate changed
routing settings. Unknown-stage failures remain uncertain, never a guessed
rollback. Preserve the original request and key for reconciliation; see
[MCP tool errors](../failures-and-cancellation/index.md#mcp-tool-errors).

After accepting a plan, `plan_submit` tells the coordinator that its workers
are starting and to end the turn rather than poll. The runtime does not wait
for that turn to end: once the coordinator's next tool or text update shows the
task in `PLAN_READY`, a runtime thread records the execution baseline, moves
the task to `EXECUTING` and dispatches the dependency-ready nodes. That thread and the
controller loop share one per-task lock, so the start and the first
reservations happen once. A planning turn that ends with an error after its
plan has already started does not block the running plan.
When `task_get_context` shows no coordinator action is needed, it gives the same
turn-ending hint. An identical repeat read on that channel returns a compact
unchanged-state receipt instead of resending the plan and nodes. The hint is
advisory; the runtime does not force a model to end its turn. Worker dispatch
timing remains at the turn boundary.

The store (`crates/cortex-core`) holds task state, original goal, plan revision, dependency
nodes, structured results, events and request receipts in `.cortex/cortex.sqlite3`.
State changes and their events commit together. A plan is validated for exact fields,
known roles, unique node names, existing dependencies, cycles, bounded scope and
verification commands. Verification commands run as literal argv without a shell, so plan submission rejects command substitution, backticks, redirection, pipes, shell operators and standalone parentheses; use a documented project command or script instead. Each new plan node requires a `model` recommendation (`auto`, engine id, or `engine/model`) and may include `model_reason`; neither field grants dispatch authority. JSON `null` for an optional node field (`model_reason`, `complexity`, `work_kind`) is treated as omitted; a required field sent as `null` is still refused. An `invalid_arguments` refusal for a field sent at the wrong level (for example `verification` inside a task or beside `plan`) names the advertised location, such as `arguments.plan.verification`, instead of only telling the model to omit it. The runtime computes tier and effort and revalidates policy and availability. See [automatic routing](../routing-and-fallback/index.md#automatic-worker-routing-tiers-model-hints-and-ranking).

The native host drives planning, execution, independent review,
verification and synthesis. Only dependency-ready nodes are reserved for execution.
Reservations commit before provisioning. A single chat session (the ACP session
and task it drives) can run at most four workers at once across all engines,
roles and connections. The scheduler claims only as many dependency-ready nodes
as that session has free slots; remaining nodes stay `pending`. Atomic capacity
reservation enforces the same session-wide ceiling. A connection's validated
`max_concurrent_sessions` (default 4, configurable from 1 through 64) can only
lower that session's worker share on that connection; it neither raises the
four-worker session cap nor pools unused slots across chat sessions.
Coordinator reservations do not consume worker slots and retain their own
per-connection limit. Two different waits exist; neither fails the worker.
The scheduler claims only as many ready nodes as the session has free worker
slots, so excess nodes stay `pending`. A claimed worker whose engine connection
has no free slot (other holders, other chats, or an ambiguous unconfirmed exit)
is not refused: it waits locally. The task view shows the warning
`waiting_for_engine_capacity` with the reason "Waiting for an engine connection
slot" (`NATIVE_WORKER_CAPACITY_WAITING`, closed by
`NATIVE_WORKER_CAPACITY_WAIT_ENDED`). The worker wakes when a slot's release is
confirmed (and on a one-second recheck) and is admitted in the scheduler's claim
order for that connection. The authority check, slot check and reservation are
one store transaction, so two contenders can never take the same slot. This wait
has no timeout of its own; it lasts while the task's authority stands. An owner
pause, stop, cancellation or service shutdown ends it without launching
anything. Siblings and workers on other connections keep running, and an
ambiguous holder never counts as released. The top-level Start limit (32 tracked
jobs) is a separate admission rule: it still refuses with
`native_capacity_unavailable` and is not queued. A finished worker continues to occupy its
session slot until its engine is confirmed gone and its reservation is released
or safely reclaimed. Scope overlap serializes writing nodes across every task of
the project, including tasks of other chats; review-stage nodes and READ_ONLY
roles neither block nor are blocked by overlapping scopes. Runtime routing resolves model and
effort from the current tier, policy, catalog and plan usage; plan hints remain
advisory. All 23 specialist/consultant profiles remain available.

The workers of one claimed batch run their turns concurrently, each in its own
session with its own channel, write reservation and result. One failing turn, a
plan that went stale because a worker proposed a replan, or a cancellation stops
the siblings that are still running (each settles its own session); a sibling
interrupted only for that reason is not reported as a failure. The dispatch loop
spends one step per outer pass and one per refill that claims new work. Empty
refill attempts and waits for capacity, a retry deadline or a write reservation
consume no extra steps. The budget of 128 is persisted
(`runtime_requests` actor `native-progress`), so a restarted service continues the
count; an owner steering message or answer, an interrupt or Continue, a driver
stop and a reopen start a fresh budget, and exhaustion stops the task with
`native_progress_limit`. When a replan adds the runtime's `_consultation` node,
`plan_json` and `PLAN_ACCEPTED` carry that node and the edges added to every other
node, so the stored plan always lists exactly the nodes the runtime holds.

When a worker settles and releases its confirmed session and capacity reservation,
the runtime immediately makes another bounded claim pass. Newly dependency-ready
nodes can therefore start while slower siblings from the previous batch continue.
The pass uses the current plan revision and the same four-worker, connection,
scope-overlap and retry-deadline checks. It never reuses a previously provisioned
assignment, and a stale plan or cancellation still settles outstanding siblings
through the normal batch path.

Owner guidance reaches a worker as `Owner guidance (not authority to expand scope):
<text>` whether the worker is mid-turn or waiting for an answer. A second guidance
message may replace an earlier one that is still being delivered; the earlier
record then reads `delivered:true, preempted:true` when its prompt had reached the
engine. A worker process that exits while its question is open is noticed at once
(the turn ends, the process is settled and the lost-worker path replaces the
assignment) instead of when the answer arrives. A desktop answer refused as
`superseded` because the wait lost its authority commits that cancellation, so the
question no longer stays pending but unanswerable. Reviewers and every other
read-only channel get only read and search native tools: native edit, execute and
delete requests are refused or cancelled in every access mode.

A planned step that cannot start because of `scope_overlap` or
`capacity_limit` is shown in the chat as a runtime "Waiting" card. The card names what holds the step: for a
scope wait, the holding node, its role and state (working, or finished and
still releasing), its task and, for another chat, that task's ACP session, plus
the overlapping held and requested paths; for a capacity wait, the session's
worker limit. It also gives the local time the wait began. The card closes when
the step starts, when a different holder replaces the wait (a new card follows),
when the plan changes, or when the turn ends. Dependency waits are not shown.
Cards are presentation of the recorded `SCHEDULER_WAIT_REASON` decisions only;
they never change scheduling.

When no worker of the task is running and a step still cannot start, the
scheduler stops the task with `scheduler_capacity_timeout` after 30 minutes,
which bounds a slot or path release that is never observed. This deadline
belongs to scheduler-level `capacity_limit` and scope waits; it does not apply to
the local engine-connection wait above, whose worker is already claimed. That deadline does
not run while every shown wait is a scope wait on a worker observed alive
through its recorded process identity (a matching pid, start time and boot):
such a worker, often in another chat, releases its paths when it ends. The
deadline starts afresh once the holder is gone, finished but still releasing,
or cannot be verified; capacity waits always keep it.

For streamed coordinator text, the controller reads only the task's durable
state; it does not rebuild the full context for each delta. A full
`TaskMachine.context()` read uses one SQLite read transaction, and assignment
attempt lookups for result checks are batched.

Coordinator text around the coordinator's own question is never withheld. Every
display session, the Cortex Workspace session and the incoming ACP host session
(`cortex-acp` used from another client), records every message chunk the engine
emits, in order, including chunks the service processes after
`user_question_request` committed `WAITING_USER`: a message is never cut short at
the question. Avoiding a restatement after the question is left entirely to the
coordinator's instructions; the coordinator's question receipt carries a `next`
hint telling the model to end the turn without further text, and like the plan
and context hints that hint is advisory.

While workers run, an unchanged database change fingerprint skips the expensive
scheduler pass. Worker completion and runtime notifications can wake it sooner;
a periodic full reconciliation still runs at least every 2 seconds. Permission
card relay keeps its 0.1-second cadence. The database remains authoritative.
`claim_ready()` first checks for candidates with a read-only query, then takes
a write transaction only when candidates exist and rechecks them there. Ready
nodes are ordered by longest remaining dependency chain, with original row
order as the tie-breaker. Retry-history reads are batched per claim. A change
missed by the fingerprint can therefore wait up to the 2-second reconciliation
interval.

### Coordinator prompt context

Coordinator prompts compact nodes superseded by earlier review cycles. The
current implementation batch and its latest reviewer remain full; each older
node keeps its `node_id`, role, status, report reference and verdict.
Historical `not_run`, `unverified` and open `failed` checks remain in
`historical_check_gaps`. Only the prompt's `nodes` projection changes:
all other context fields stay intact, and `task_get_context` continues to
return the full persisted state.

### Worker brief assembly

Each reserved assignment receives a compact JSON brief capped at 16,000 Unicode characters, measured after encoding with the same serializer used by the runtime. The assignment objective, scope and dependency report references stay complete. If those required fields alone exceed the cap, dispatch fails closed with a provisioning error.

The original task goal and review git evidence use whatever room remains. A shortened field is marked with `truncated: true`, original character and UTF-8 byte counts, shown character count, and a source reference. Workers can read the full task goal from the `constraints` section of `read_capsule`. Review evidence points to `project_git` with its exact `status` or `diff` kind and, when captured already, to the exact `read_large_output` id. Brief previews are informational; the worker can inspect the full source through these existing worker tools.

The runtime inserts an independent code reviewer after execution whenever the
planned work changed the project. When a plan revision starts executing, the runtime
records an `EXECUTION_BASELINE` observation of the project (the same read-only tree
digest and HEAD used for review and verification binding). When every node has
completed, review is skipped only if the project observed now equals the first
baseline since the task was created or last reopened, or a state an independent
reviewer approved. The runtime then records `REVIEW_NOT_REQUIRED` with the reason
and digest, projects it into the pipeline's "Independent review" section, and goes
straight to verification, which binds to that digest. Any other observation, an
unavailable observation, or a revision that already has a reviewer (a fix cycle)
schedules review. The controller makes this decision as soon as every execution
node has an accepted completed result; it does not wait for result-bearing worker
engine turns to finish. Result acceptance revokes the exact attempt's worker MCP
channels transactionally with the node result. New authenticated wire calls,
including result replays, are then refused as `channel_revoked`; the durable
result receipt remains available to runtime recovery without republishing.
Lingering engines still consume
capacity until their exit is confirmed, so a full session or connection can
queue the reviewer. Review independence is checked against the recorded routes. The reviewer's route
is chosen to run on a different model than the execution nodes of the same plan
revision, and preferably on a different engine, whenever the usable routes offer
one. When no other model is available, or an owner pin or request fixes the model,
the review proceeds and carries an explicit warning: the route decision records
`independence.status = "warning"`, the review result carries `review_independence`
with the same status, the runtime records a `REVIEW_INDEPENDENCE_WARNING` event, and
the task facts show a warning for that review. A reviewer recorded on the same model
without that warning is refused as `review_not_independent`; a review whose routes
were not recorded (for example a task in flight across an upgrade) is stored with
`review_independence.status = "unverified"` and shown with the same warning event and
task-facts warning, never as a silent field. Such a warning is shown on the result card; it does not by itself make the result
unacceptable (the accept gate still needs approval, current verification and an unchanged
project folder). In the Workspace owner gate for a native task, once the runtime's own
verification of the current revision passed on the unchanged folder, a worker's or
reviewer's self-reported `unverified` or `not_run` check is a visible note on the result,
not an open check: it needs no acknowledgement and does not block Accept. Self-reported
`failed` checks, policy-blocked checks, failed or missing runtime verification and a
missing approval still block (`crates/workspace-service/src/machine/task_facts_work.rs`). A withdrawn probe that completion accepted is omitted again when its withdrawal is re-derived from the run's recorded output (kept under the channel that ran it, its stored file still hashing to the recorded digest), and a check that still blocks is named in the reason.
A project-folder sprint task's "unchanged" is judged on the files it recorded writing,
because the later tasks of its sprint write the same folder. Independently of routes, a reviewer attempt
that is also an execution attempt of the task is refused as `review_not_independent`.
Review remains independent and digest-bound; MCP revocation
does not intercept native filesystem edits; a writing turn's edits are attributed to the task and invalidate the review
binding. A late engine failure, timeout or cancellation does not replace an
accepted result or authorize a retry of that completed node. Verification and
final completion gates are unchanged, including confirmed absence of live worker
sessions at completion.

Retried nodes and fix nodes are covered because the decision
compares the whole project, not individual worker claims. No model argument or
hint can waive review; `EXECUTING → VERIFYING` is refused without the runtime's own
record. A review result binds to the digest recorded in `REVIEW_STARTED`: the
task's own files' states, or the whole checkout when the task changed too many files
([review of the task's own changes](#review-of-the-tasks-own-changes)).
If what the review binds changed while it ran, for example because another writer
edited one of the task's files, `result_submit` refuses the verdict with
`review_project_revision_changed` before publishing anything. It then records a new
`REVIEW_STARTED` for the current digest (`rebound_from`, `rebind`,
`refused_request_key`). The reviewer re-checks the changes and submits with a new
request_key. The refused request_key never records a verdict
(`review_resubmit_required`). After `MAX_REVIEW_REBINDS` (3) re-bindings, a further
change is refused as `review_project_revision_unstable` and recorded as
`REVIEW_REVISION_UNSTABLE` with the observed digest; nothing is re-bound while the
project keeps changing between submissions. A submission that finds the same digest
as the previous unstable refusal (nothing changed in between) re-binds the review to
that state (`REVIEW_STARTED` with `after_unstable: true`) and is refused
`review_project_revision_changed`, so the reviewer re-reads what changed and the
next submission on that state is accepted. A key refused as unstable never records
a verdict either. Earlier, nothing was recorded past the budget, so every later
submission was compared with the stale binding and the review could never clear.
Final verification and completion certify exactly the reviewed binding. With a
task-file binding they certify the task's own files, not the rest of the checkout,
which no review saw; the plan's commands still run against the real checkout. Each of these reaches the
reviewer as a correctable `isError` result naming the code, `field` `request_key`
and a hint to re-check the change and resubmit with a fresh key. Before this, the binding
was never refreshed, so one change refused every later submission of that review.
The refusal names the other Cortex task when one of that project's writing workers
made the change.

A change by another actor outside the task's own files is charged to the runtime,
never to the reviewer. It can refuse a review only when the review binds the whole
checkout (attribution was incomplete). When no writer of this task changed a file
since the review first bound its revision, the re-binding and unstable records carry
`drift: "external"`, and `review::checkout_drift_tx` returns the evidence for the
reviewer's assignment. A review that then fails is classified `external_outage`
(`rule: checkout_drift`) with a time-only wait (`probe: timer`, host
`project-checkout`): the runtime waits with the usual backoff, without model calls,
and reviews again, without a rung of the strategy ladder and without a review cycle.
A task-file binding re-binds only when one of the task's own files changed, which the
reviewer must re-read, so it never counts as external.

When the project changed after the review and the change came from another Cortex
task's writer in the same project (`state.foreign_project_writers`: a write
operation or command of a non-read-only role since the review bound its revision),
verification (including its ordinary preflight) or completion no longer stops for the user. The runtime records
`PROJECT_CHANGED_BY_OTHER_TASK` (the other task ids), clears the synthesis, adds a
new review cycle on the current revision and says so in one chat line; review and
verification then run again. An unattributed change, or one past
`MAX_REVIEW_CYCLES`, still stops with `verification_review_revision_changed` or
`completion_project_revision_changed` (observed 2026-10-22: one task's edit
invalidated another's verified completion three seconds before it finished).

A review that requests changes without structured findings, on a plan with more than
one implementing task, goes back to the coordinator (`RETURNED_TO_COORDINATOR`,
`review_changes_without_findings`) instead of a blind fix of every task; the planning
prompt tells it to read the review report and plan fixes by the roles that own the
affected files. Blind fixes once sent application code to a devops task.
APPROVE proceeds
to verification; REQUEST_CHANGES creates fix nodes followed by a fresh reviewer;
BLOCK, which names the confirmed problems as findings, stops acceptance and goes
back to the coordinator to address them. INCOMPLETE, which names in `not_covered`
each part of the change the review cannot cover itself, with its dead-end
`blocker` (`missing_access`, `owner_decision`, `other_role_work`,
`tool_unavailable`) and why, is neither an approval nor a block. The reviewer
finishes its own review first: `result_submit` refuses, before publication and
with the exact items in `missing`, an INCOMPLETE part without a valid blocker
(`review_coverage_unfinished`), a required changed regular file (the task's own change) it has not read completely
in its current bytes (`review_file_coverage_required`; a line longer than one
`project_read` segment counts once its segments were followed with `next_column`
to its end, and a file's `review_lines` ranges end at its last line as
`project_read` counts lines, so a final line break adds no unreadable line), a required predecessor report
(its packet's `predecessor_reports`) not read to its last page
(`review_predecessor_reports_required`) and a BLOCK without
findings (`review_block_findings_required`); the review stays the one running
assignment and the same session continues. A later review round reads only what
changed since an earlier one: a listed file that an earlier completed review of
the same task read completely (or through all of its own packet's `review_lines`)
at exactly the current `current_sha256` counts as read, and the packet marks it
`reviewed_before: true` with `reviewed_before_guidance`; any change produces new
bytes that need a fresh read. The file-coverage refusal is a chance to
read, never a loop: after two `review_file_coverage_required` refusals of the same
review assignment the next result is recorded, and an approval carries the
`unverified` runtime check `Changed files the reviewer did not read` naming every
required file still unread, disclosed under Not verified (owner store 2026-11-21:
one review was refused five times for 32 small generated SVG files and the task
waited on the owner). The count belongs to the review lineage and the file's exact
bytes, not to one assignment: refusals of every review assignment of the task since
it was last reopened (reassignments, later review cycles and later plan revisions
alike) count together per path and `current_sha256`, so once a file's bytes were
named in two refusals, or an approval already disclosed them as unread, no later
review of the task is required to read those bytes again; they are disclosed the
same way (`settled_unread_tx`, `crates/cortex-core/src/lifecycle/completion.rs`;
owner store 2026-11-21: three assignments were each refused twice for the same 32
files). Changed bytes are a new file to read. The refusal hint points to
`read_batch`, up to eight inline `project_read` items per call. An accepted INCOMPLETE schedules no
second review of the same revision: actionable findings start their fixes,
otherwise `REVIEW_DEAD_END_ROUTED` records the blocked parts. When every blocked
part is one the reviewer's tools cannot reach (`tool_unavailable`), the parts
become disclosed unverified limits (`REVIEW_LIMIT_DISCLOSED`) and the task
continues to verification. The same holds for an `owner_decision` part the owner
already decided, in this plan revision or an earlier one since the newest reopen or
owner message: its reason cites an owner question that was answered, or an earlier
review routed the same item as an `owner_decision` dead end and an owner answer
followed. The same item is recognised after rewording: the same item name without
the reviewer's detail, or only file names the decided items already named (one item
for three generated files split into one item per file). It becomes a disclosed
limit (`Review did not read …`, with `LOOP_BROKEN` naming the earlier answer), not
another question (owner store 2026-11-21: ten identical questions after the owner
chose to close as is, the last ones after the decided item had been split per file); otherwise the task continues with `review_incomplete`
on the next rung of the strategy ladder, in every run mode, and a dead end that
repeats on the same item takes the `reassign` rung ([no-stop
policy](../task-lifecycle/index.md#no-stop-policy-and-the-strategy-ladder)). A decided
owner item is skipped when that stop is classified, so it never becomes an `ask_owner`
continuation either. A fix and review round
continues only on progress: when a REQUEST_CHANGES review finds the reviewed bytes
unchanged since the previous review of its revision, or the same findings (files and
fix, with case and spacing normalized) as the previous review of the task, also when
that review belongs to an earlier plan revision, the runtime records
`REVIEW_NO_PROGRESS` (`previous_revision`, `across_revisions`, the findings `key`)
and `LOOP_OBSERVED` (`kind: review_findings`, with how many reviews of the task
returned those findings) and stops once with `review_no_progress`, a continuation
that changes approach, instead of another round. The review-cycle budget belongs to
the task, not to one plan: `review_cycle` still numbers the reviews and fixes of one
revision and bounds them at three, and every new revision gets its mandatory review,
but once three change-requesting reviews were recorded across the task's revisions
(since the newest reopen), a further fix round is granted while the review raises a
finding no earlier review of the task raised (same files and normalized fix): a new
problem is progress, however many there are. When every finding was already raised
before, or after twelve change-requesting reviews in any case, the review stops with
`review_cycle_limit` (`LOOP_OBSERVED`, `kind: review_cycle`, with both counts).
Counting findings instead had stopped a task whose reviews each found one different
problem (owner store 2026-11-22). Before this, each continuation's new plan started a fresh budget, and
one task ran nine review-cycle ladders with eighteen change requests (owner store
2026-11-21). Every reviewer's assignment
carries a `review_packet`: the files the reviewed work changed (path, change kind,
the plan node that wrote it; at most 200, the rest counted by folder), built from
the difference between the execution baseline's per-file manifest and the folder
now plus the writes the sessions recorded, never from Git alone, with
`complete: false` when only recorded writes were available (changed Git-ignored
files are not deliverable: they only add an `ignored_files` note); the predecessor
reports with their size and page count, split into the required reports of this
review's round (`predecessor_reports`) and optional context (`optional_reports`:
earlier rounds, work without recorded writes, earlier plans; see
[roles and delegation](../roles-and-delegation/index.md)); and the review's read scope, checked to
cover every required changed file. A change set the host could not enumerate never makes
approval impossible: the required listed files must still be read completely, and an
approval then carries an `unverified` runtime check (`REVIEW_CHANGE_SET_LIMITED`)
that the final result discloses under Not verified, telling the owner to inspect
the folder. A listed file whose current bytes (exactly those of its
`current_sha256`) have no reviewable text is not a missing read either: the packet
marks it `review_content` `binary` (not UTF-8, or containing NUL), `image` (a
recognised raster image, or an SVG: text that opens with `<svg`, an XML prolog or
a comment and names `<svg` within its first 4 KiB), or
`opaque` (encoded or minified text of at least 8 KiB whose lines average over 1,000
bytes, or whose lines are all base64-like), or `bulk` (text whose required bytes, its
`review_lines` ranges or else the whole file, exceed 64 KiB, about eight
`project_read` pages, such as large generated data; the packet then adds
`review_content_guidance`), and a read of such bytes that found no text proves the
same. A `bulk` file the review did not read completely is disclosed as the
`unverified` runtime check `Changed files too large to require a complete read`. Unless the review read it completely (an image through a
direct `project_read`, which shows it), an approval carries the `unverified` runtime
check `Changed files without reviewable text` naming those paths, disclosed the same
way. Every binary read records hash-bound coverage of its bytes, single or in
`read_batch`, and so does a direct image read; a batched image read records nothing,
because the batch refuses image replies (`batch_multimodal_unsupported`) and the
image never reaches the model. A batched text page larger than the batch's inline
item bound (which the batch defers) records nothing and claims only the coverage
recorded before it (`coverage_recorded: false`); before this, the page reported the
coverage it would have added.
Three unsuccessful review cycles block further automatic
work. The loop never treats an ordinary final model message as a worker result.

### Reviewer handoff

Reviewers default to publishing their complete Markdown report inline with
`result_submit`. The same immutable publication and shared decision brief apply;
verdicts remain APPROVE, REQUEST_CHANGES or BLOCK. Draft publication stays
available for genuine incremental editing and other roles. This is guidance,
not a claim that either engine necessarily follows it.

Before the existing git status/diff, the runtime adds a metadata-only
`runtime_verified_facts` block, capped at 4,096 serialized UTF-8 bytes:
- Current execution nodes' checks backed by verified same-attempt receipts:
  submitted name labels, recorded outcomes, current outcomes and receipt refs.
  Names are labels, not verified interpretations; these receipts are explicitly
  not asserted to bind to the reviewed digest.
- Exact planned verification argv, labelled as planned, not passing evidence.
  The runtime itself runs that exact argv, authoritatively, on the approved
  digest, or credits an eligible speculative run after approval; the reviewer
  should not re-run the same argv solely to duplicate it, and never records it
  as its own deferred or not-run check, since the runtime closes it from its
  own receipt. Matching deferred checks close only through complete passing
  runtime evidence and identical argv with matching author, approved and
  current digests.
- Changed paths and before/after kind, SHA-256 and byte size versus the current
  plan's execution baseline, not HEAD or a claim of worker authorship. Regular
  files use content hashes; symlinks use hashes of link text without following
  targets and have no content size. Directories/special entries have no hash/size.
  Deleted paths have no after metadata.

Execution baselines retain path metadata with the existing revision observation.
Older baselines without it report `baseline_unavailable`; unavailable or drifted
captures report a diagnostic, never an observed empty change set. Tab/newline-containing
regular and directory paths are preserved. Ambiguous unescaped symlink path/target
boundaries or malformed manifest records make the complete metadata comparison
unavailable (`path_metadata_unavailable` for the current capture), not partially
observed. Link targets remain hashed, never disclosed or followed. Facts carry
the review and capture digests; only a matching capture supplies path deltas.
A rebound refusal refreshes the block for the new binding or explicitly reports
refresh unavailable. The block carries no worker summaries, report bodies,
blocker prose, file contents or command output. Existing dependency report
references are unchanged; two-step withholding of worker interpretations is not
implemented.

Oversized entries are omitted whole with `truncated` and per-section omitted
counts. The bounded block is retained under the review assignment's existing
`read_large_output` access, and brief truncation retains that exact reference.
The retained block still reports its omitted entries; it is not a complete
inventory when truncated. This context does not replace independent analysis,
targeted checks or final runtime verification.

Implemented in `crates/cortex-core/src/lifecycle/review.rs` and the `workspace-service` native host.

### Coordinator return budget

A failed worker (`worker_failed`), a worker the owner stopped when nothing else can run (`worker_stopped_by_owner`), a lost worker whose retries are used up, or a review that requests changes without findings on a plan with several implementers (`review_changes_without_findings`) returns the task to the coordinator instead of stopping. The runtime records `RETURNED_TO_COORDINATOR`, revokes the worker channels, cancels the live assignments and moves through RECOVERING to PLANNING. `MAX_COORDINATOR_RETURNS` bounds these returns. The count covers `RETURNED_TO_COORDINATOR` events after the task's latest `PLAN_ACCEPTED`, `USER_ANSWER_RECEIVED` or `USER_STEERING` event. An accepted plan or explicit user guidance therefore renews the allowance, and failures in earlier plan revisions do not exhaust later work. A second bound, `MAX_AUTONOMOUS_COORDINATOR_RETURNS` (6), counts returns since the user last spoke (`USER_STEERING`, `USER_ANSWER_RECEIVED` or `WORKER_USER_ANSWER_RECEIVED`), across plan revisions. A new plan alone does not renew it, so a coordinator that answers every return with another plan of failing work stops for the user instead of looping. The boundaries are event ids, so existing stores need no migration.

When either allowance is exhausted, the return is refused. A worker failure or an exhausted lost-worker retry then becomes a continuation on the strategy ladder (no-stop policy): the next attempt escalates its route, consults, or hands the work to another role; the owner is not asked. A review without findings instead falls through to the conservative fallback fixes described under [targeted review fixes](#targeted-review-fixes).

An owner-stopped worker or reviewer (its node is `cancelled` and a `WORKER_STOPPED_BY_OWNER` event follows the latest return to the coordinator) does not end the task. Cancellations the runtime makes itself when it returns to the coordinator carry no such event, so they never trigger another return. The stopped worker's open question is cancelled with it. Independent siblings keep running. Once no node is running and none is runnable, the runtime returns the task to the coordinator with the reason `worker_stopped_by_owner`, so it can replan or finish. Dependents of the stopped node never start silently: the return cancels them with the other unfinished nodes. When the return allowance is exhausted, the stop question says in plain words that a worker was stopped and the remaining planned work depends on it or has nothing left to run; it never shows an internal code.

For a worker failure, the controller first waits for every running or pending independent sibling to finish, so the coordinator or the user sees all results before the decision. Repeated returns without a new boundary stay bounded. Several accepted revisions can still follow one another over a task's lifetime, but only up to the autonomous bound between user inputs.

### Targeted review fixes

A reviewer that requests changes may cite a failing check as its finding. That
review is recorded as completed and starts a fix cycle; it needs no
`assignment_outcome=failed`, which stays reserved for a review that itself failed. An
APPROVE that cites an open failed check is still refused, and an explicit
`assignment_outcome=failed` still fails the review.

A reviewer may add optional structured `findings` to a REQUEST_CHANGES result:
each names one original plan task (`node`), the `files` the fix changes, the
`fix` itself and whether the problem is `cross_cutting`. The field is offered and
accepted only on a review assignment (`reviewer_only_findings` otherwise) and is
ignored with any other verdict. `TaskMachine.add_fix()` confines fixes to the named
tasks only when every finding names a task of the accepted plan, is
`cross_cutting: false`, and lists only files the runtime itself finds inside that
task's scope (the same path rules and scope matcher as the gateway). Each such
fix receives only its own findings and the review's `report_ref`, and depends on
the review plus the fixes of the selected tasks it transitively depended on in
the plan, so related fixes keep the plan's order. When findings are omitted or empty and there are multiple implementing tasks,
the runtime returns to the coordinator to plan the remedy while its return
budget remains (see [coordinator return budget](#coordinator-return-budget)). Otherwise, absent findings or unconfinable findings (an unknown
task, a cross-cutting finding, a file outside the scope or findings too long
for one objective) produce the conservative fix of every original plan task
with the review summary. These broad fallback fixes
retain the original plan's direct dependency edges: each fix depends on the
review and on generated fixes for its direct plan parents. Independent branches
remain parallel. It never infers a contract change; a reviewer marks one
`cross_cutting`. `REVIEW_CHANGES_REQUESTED` records
`targeted` and the `fallback` reason. The three-cycle ceiling is unchanged.

**Fix role.** A review finding may name, in its optional `role`, the writing specialist who should make the fix (any domain: documentation, build scripts, interface code, a render pipeline). A fix goes to that specialist when every finding it carries (its own findings for a targeted fix, every finding for a fallback fix) proposes the same writing role; a read-only or conflicting proposal is ignored. Otherwise it goes by the files those findings name: `worker-technical-writer` when every named file is under `docs/` or is a Markdown file, `worker-qa-engineer` when every named file is a test (under a `tests`, `test` or `__tests__` folder, or named `*.test.*`, `*.spec.*`, `*_test.*` or `test_*`), and, on a QA, technical-writer or read-only node, a developer for the product code they name (`worker-frontend-dev` for interface sources, `worker-devops-engineer` for scripts and CI, `worker-fullstack-dev` for a mix, `worker-backend-dev` otherwise). Owner store 2026-11-21: documentation findings on a QA node were routed to QA and the coordinator had to replan them. Without named files a fix node never keeps a read-only source role. The runtime replaces a role in its `READ_ONLY` class (for example reviewer, architect, UX designer, consultant or auditor) with a writing specialist chosen from the original node's effective scope: `worker-technical-writer` when every path is under `docs/` or is a Markdown file, `worker-qa-engineer` when every path is under `tests/`, and `worker-backend-dev` for any other scope, including mixed and whole-project scopes. A writing role is kept unchanged. The fix keeps that effective scope exactly, so neither a finding nor its proposed role can widen it, and the choice never reads the findings' prose. The same selection applies to targeted and fallback fixes. The review after the fixes remains a fresh reviewer assignment.

### Execution failure repair

When a writing execution worker submits a failed assignment with an open,
verified failed check, its first submission receives repair guidance. The same
attempt can fix the code and rerun the exact command within its authorized
scope. If the failure persists, the worker may publish the failed result and
the runtime returns it to the coordinator within the existing recovery limits.
Read-only workers report the failure for a writing assignment.

A published execution failure with a verified open check survives a replan.
Completion requires a later verified pass of the same intent on the current
revision, bound to the reviewed digest: a worker check with the same argv and
working directory (on any profile, `command_run` or `outside_command`), a worker
check passed under the same check name, or, for a project command, a complete
final verification receipt with the same argv and working directory. The
original failure remains in its report. An unresolved prior project check sends the
task back to the coordinator once (`unresolved_prior_worker_check`); after that one
continuation, a prior check that is still neither rerun nor superseded is
disclosed instead of blocking completion. A worker's own diagnostic probe from an
earlier failed revision is disclosed at once, without that continuation (owner store
2026-11-22: returning one cost a whole plan revision): `PRIOR_WORKER_CHECK_DISCLOSED` records
it as an `unverified` runtime limit naming its revision, node and whether it was
the worker's own diagnostic probe (an inline `python -c` or `sh -c` script, a
`git status` or `git diff` scope probe) or a project check. It is never recorded
as passed. The coordinator has no tool to retract a prior probe with a reason.

Worker results bind to a published report owned by the exact provisioned attempt.
The result receipt retains a content digest.

### Qualification is not completion alone

A read/search response and an execution worker's QA label are not automated
check receipts or mandatory-review verdicts. Inspect the complete nested tool
reply: errors may exist only in content blocks when structured data is absent.
Use a real same-attempt command receipt on every passed automated check;
never manufacture a gateway ID. Preflight readiness is not dispatch; dispatch
is not assertions; `COMPLETED` alone does not qualify an API.

The qualification assertion reader preserves valid runtime unchanged-project
waivers. Its `--require-review` option instead requires an actual review node
and the matching attempt's structured `APPROVE`. Use a tiny real edit in a
disposable fixture for that scenario; never force review by changing lifecycle
metadata. Arbitrary prose `REQUEST_CHANGES` is not a runtime verdict, although
a real unresolved assertion or failed check still blocks qualification.
See [public read evidence](../tool-discovery-and-read-batch/index.md#executable-public-evidence).

### Check evidence

`result_submit` `checks` are automated checks: each one is an executed command
with an exit code. Qualitative review remarks, read files and inspected code belong
in the report body, not in `checks`. `project_git` status/diff is context
only, not a check receipt: its retained `large_output_id` is an output identity,
not a `tool_call_ref`. The response does not expose command-receipt
`exit_code`/`complete` fields. To check a Git invariant, run a meaningful
exit-status check such as `git diff --check` through `command_run`.
The advertised `result_submit` schema carries
this contract in its property descriptions; `assess_check`
enforces it.

A worker result must contain `report`, a required top-level plain string: the
exact `report_id` of a report published for that worker attempt, or the complete
inline Markdown, titled by the optional `report_title` or else by its first
heading. When publishing a draft with `write_report`, pass its returned
`report_id` alone as the later `result_submit` `report`. Without a draft, inline
submission publishes the report as part of the result. The report is in the
schema's plain `required` list, not only in a conditional: models fill that list,
and with a root if/then/else alone reviewers sent just summary, checks and
verdict. It is a string rather than an object because Claude engines emit a
top-level string argument verbatim but hand-write JSON for object values: the
former `report: {title, markdown}` object let Sonnet 5 workers break the escaping
of nested Markdown, and the engine refused those calls before Cortex received
them. Any other identifier-shaped `report` (a draft id, a receipt, a malformed
report id) is refused with `invalid_arguments` before anything is published. If the report is missing, the
correction may suggest a report only when exactly one eligible own published
report is available; this hint does not bind the report, and the worker must still
submit an explicit reference or inline report. A report identifier is not a
check receipt.

The advertised check object uses `name` and `outcome` (not `status`), with
`passed`, `failed`, `not_run` or `unverified`. Only `not_run`/`unverified` require a
concrete `blocker`; `passed`/`failed` omit it. A `passed` check requires same-attempt receipt evidence as
described below. Optional argv-form `command` is descriptive, not proof. Malformed
submissions return field-path corrections where available.
For a verified pass, copy `tool_call_ref` from a completed, exit-0
`command_run` or confirmed `outside_command` result of the same attempt.
`closed_by` is supplied by the runtime for a failed check closed by an exact
later passing rerun; workers do not submit it. A missing or weak receipt does
not become a pass merely because its string has the right shape or a worker
also supplies `checks[].command`. That argv is
accepted as descriptive text on a passed or failed check; only the gateway receipt
establishes the run. Neither submission nor completion compares it with the
receipt's argv, so a verified pass whose description differs (an abbreviated
inline script, say) does not hold completion. Results retained by earlier builds
with such text complete the same way, and a result an earlier build held in
recovery for only that reason (`completion_worker_check_unverified`) is checked
again on the next start. Reviewers
must send `verdict` as `APPROVE`, `REQUEST_CHANGES` or `BLOCK` (the lowercase
spellings are normalized); other workers must omit it. Malformed submissions
report the exact argument field path and accepted form where available.

A `passed` check is **verified** only when its `tool_call_ref` names exactly one
gateway receipt that is:

1. a `command_run` or user-confirmed `outside_command` (its `TOOL_AUTHORIZED`
   operation), authorized on a channel of the same task **and** the same attempt;
2. complete: a completed durable receipt with no `failure` (not pending,
   interrupted, timed out, authority-revoked or cancelled) and an integer exit
   code of 0;
3. backed by a complete retained output: the receipt's `large_output_id` names a
   `large_outputs` row of the same task, bound to the same ref and attempt, not
   truncated, with the same exit code and no failure. The receipt itself inlines
   at most 64 KiB of `output`, with `output_truncated` and the full `byte_count`;
   that inline cut never affects verification, and `read_large_output` pages the
   complete retained bytes;
4. recorded with its `argv`, and that command is not a trivial stand-in (below);
5. cited by no earlier check of the same result (one command, one check);
6. bound to the submitted work (`revision_binding`, see
   [check history](#check-history-and-current-outcome)): the whole checkout was
   unchanged from launch to exit and is the submitted one, or the run's work set was
   unchanged from launch to exit and is unchanged at submission. Otherwise
   `evidence_gap: "revision_mismatch"`.

**Trivial-command rule** (`trivial_command` in
`crates/cortex-core/src/lifecycle/completion.rs`). After leading
`NAME=value` assignments and the launchers `env`, `timeout`, `nice`, `nohup`,
`command`, `exec` and `time` are removed, a command is trivial when nothing is
left; when its program basename is a no-op, printer, listing, file read or search
(`pwd`, `ls`, `echo`, `printf`, `cat`, `head`, `tail`, `tee`, `grep`, `rg`, `find`,
`true`, `test`, `cd`, `env`, `which`, `stat`, `wc`, `sort` and the rest of that
list); when it is a read-only `git` subcommand (`status`, `log`,
`show`, `branch`, `rev-parse`, `ls-files`, `remote`, `config`, `describe`) or
`git diff` without `--check`, `--exit-code` or `--quiet`; or when it is a
`zsh`/`ksh` `-c` script, or an `sh`/`bash`/`dash` `-c` script outside one recognized
form. That form is setup commands followed by exactly one simple command, joined by
`;` or `&&`: setup is any number of `umask NNN` (three or four octal digits),
`export NAME=value...`, `cd DIR` and `set` with only `-e`, `-u`, `-x` or
`-o pipefail`; the last command is then classified like direct argv. The script may
contain only words of unquoted letters, digits and `_./:,+@%=-~*?^`, single-quoted
text, double-quoted text and plain `$NAME`/`${NAME}` expansions. A pipe, `||`, a
single `&`, a redirection, grouping, a subshell, a newline, a comment, a backslash,
a backtick, command or process substitution, `$'...'` and every other `$` form make
the script trivial, as do an empty command (`;;`, a trailing `;`), unbalanced
quotes, and a last command whose program word holds an expansion, glob or tilde
(other than a leading `$HOME/` when the script does not assign `HOME`) or is a
shell keyword or code-running builtin (`if`, `for`, `while`, `coproc`, `eval`,
`source`, `.`, `trap`, `alias`, `umask`, `wait` and the like). The exit status of
such a script is therefore the last command's, or a failing `&&` setup command's,
never a masking one's. So `bash -c "umask 022; cargo test --locked"`,
`sh -c "umask 022 && cd web && npm test"` and
`bash -c 'umask 022; exec timeout 900 "$HOME/.cargo/bin/cargo" test'` verify,
while `pytest || true`, `pytest; echo done`, `pytest | tail -5`,
`cargo test > log 2>&1` and `umask 022` alone are trivial. Before this, every
inline script was trivial, and the `umask 022` this repository requires pushed
workers' checks into exactly that form. This is a bounded recognizer, not a shell
parser or a shell security boundary; direct argv is always the simpler route,
because the gateway retains the complete output anyway. Anything else (a test
runner, linter, type checker, build or project script) may verify a check. The
rule does not prove that a nontrivial command tests *the named* check (for example
an inline `python3 -c` program); that correspondence is the independent reviewer's
judgement, and the reviewer sees the exact `argv` in the receipt.

Engine-native tool calls (the engine's own shell, read, edit or search tools) are
recorded in `tool_result_observations` for display and audit only. The runtime
never observes their exit code or retains their complete output, so they never
verify a check; run checks through `command_run`.

**Insufficient evidence is kept, never refused silently and never promoted.** A
`passed` claim that is not verified is recorded as `outcome: "unverified"` with
`claimed_outcome: "passed"`, the structured `evidence_gap` code and a runtime
blocker; it is a disclosed gap that synthesis must list in `not_verified` unless
a later result closes it under the check identity rule below. A `passed`
claim whose cited command exited nonzero is treated as `failed` (an observed
failure is never softened) with `evidence_gap: "nonzero_exit"`. A claimed `failed`
check is always kept as `failed`. A new submission containing an open failure
is refused with `open_failed_check` before anything is recorded or published,
unless the worker explicitly sends `assignment_outcome: "failed"` to submit an
intentional failed assignment. The refusal leaves the channel active and its
request key reusable. Its `missing` items name each open check by its name and
`tool_call_ref`, the gap that keeps it open and what closes it
(`worker_checks::open_items`), naming a program and receipt references, never argv
text or output: `no_passing_rerun` (no later run of the same argv, cwd and
profile), `later_run_unresolved` or `later_run_failed` (the latest rerun has no
complete result, or failed too), `rerun_not_cited` (the latest rerun passed but no
check cites it), `rerun_named_differently`, `rerun_<gap>` (the cited passing rerun
does not verify, for example `rerun_revision_mismatch` when it observed neither the
submitted checkout nor an unchanged work set, with that remedy rather than advice to
rerun what already passed), `later_run_unrecorded`, `nonzero_exit` and
`later_run_failed` for a passed claim kept as failed, and the gap of a failed claim
without a verified failing receipt. `omitted_failed_check` likewise names each
omitted receipt with its program and exit code. Only the explicit failed
assignment remains when nothing can close the failure. The result receipt returned
to the worker includes `node_status` and lists every check's recorded `outcome`
with its `claimed_outcome`/`evidence_gap`, and `closed_by` for a closed failure
(below).
`evidence_gap` codes (`assess` in `crates/cortex-core/src/lifecycle/worker_checks.rs`):
`tool_call_ref_missing`, `tool_call_ref_unknown`, `ref_already_cited`,
`ambiguous_ref`, `call_incomplete`, `nonzero_exit`, `zero_exit` (a failed claim
citing exit 0), `command_unrecorded`, `trivial_command`, `revision_mismatch`,
`later_run_failed`, `later_run_unresolved` (below).

### Speculative verification during review

Once the capacity-checked reviewer is running and `REVIEW_STARTED` binds its
assignment to a project digest, the controller may run the plan's verification
commands concurrently. There is at most one speculative command sequence at a
time per task turn; it creates no engine session or extra worker reservation. Reviewer
capacity and the four-worker/per-connection limits are unchanged; a pending,
capacity-queued reviewer does not start speculation. Commands use the same
verification sandbox and retained-output path as ordinary verification: only
git-ignored project directories and private scratch are writable, and the real
runtime store remains inaccessible.

A speculative result is candidate evidence, not a `VERIFICATION_PASSED` or
`VERIFICATION_FAILED` event. After APPROVE and confirmed worker teardown, credit
requires the same plan revision, reviewer assignment and digest, complete
commands and a fresh unchanged-project observation. Credit and the existing
authoritative verification event commit together. A failed command on that
approved digest follows the existing coordinator repair path and repair limit;
it is never softened into a diagnostic pass.

REQUEST_CHANGES, replan, a rebound review or digest drift discards the candidate.
Verification runs again for the new approved binding; a run that itself changes
the digest is stale. A change after approval cannot authorize verification on
an unreviewed digest: the existing review-revision gate still applies.
Stopping/disconnecting/ending the controller turn cancels and joins its owned
sequence, killing the running subprocess group. Interrupted candidates cannot
pass. Restart recovery reruns ordinary verification rather than replaying
speculative diagnostic events. Metadata-only `SPECULATIVE_VERIFICATION_*`
events record start, finish, discard and credit; command output stays in the
private retained-output store, not in those events. The runner records
`CHECK_EXECUTION_STARTED` for each literal command before launch. This neutral
execution marker is diagnostic metadata, not a `VERIFICATION_*` lifecycle event;
it cannot close a deferred check without authoritative passing verification on
the approved digest. Closure binds execution and the gap author to that same
current digest, regardless of publication or credit ordering. Discarded or stale
candidates never close gaps.

### Automatic repair of failed verification

In an Automatic sprint, a failed final verification returns the task to bounded
planning with the failed command identities retained in runtime evidence. The
coordinator must keep every failed or unresolved command in the repair plan.
The runtime runs the repaired assignment and the exact verification command
again, preserving the original failed receipt and requiring a later complete
passing receipt before completion. Omitting a failed command is refused. The
repair budget is bounded by the existing verification-repair limit, after which
the task pauses with the failure and evidence exposed to the owner. Automatic
continuation never turns a missing, failed, truncated or unobservable check into
a pass and never waives independent review, permissions or final verification.
An outcome caused by the verifier's environment (an incomplete run, exit 126 or 127,
a full disk, or a failure of a command a worker passed identically on the same
checkout) is not a failed verification and uses no repair: the runtime reruns it
once, rechecks it itself after 1, 2, 5 and 10 minutes and then asks the owner one
question (see the verifier environment in
[task lifecycle](../task-lifecycle/index.md#no-stop-policy-and-the-strategy-ladder)).
A failed verification carries its `failure_key` for the strategy ladder.

### Check history and current outcome

A long attempt often runs the same check several times: it fails, the worker
fixes the code, and the rerun passes. The runtime keeps that run history
separate from each check's current outcome
(`current_outcomes`, over `command_history`, the
attempt's gateway command runs in `TOOL_AUTHORIZED` order).

Verification subprocess output is streamed from its temporary file into the
private large-output store, preserving the complete bytes. The tool event
carries a bounded tail excerpt and the output record ID, rather than the full
log. The excerpt is display-only; the retained output remains the complete
verification evidence. Existing exit-code, interruption and project-integrity
checks still determine whether verification passes. Each retained verification
receipt (`retain_check_tx`) keeps, besides `complete` and `exit_code`, the run's
`failure` (for example `timeout`, `command_signalled` or an unavailable sandbox) and
`signal` from the runtime's own output receipt of the same run, null when that
receipt names none; `VERIFICATION_FAILED` carries them to the coordinator and the
task's pipeline document, and the owner's stop notice names the failure with the
exit code. Earlier receipts kept only `complete` and `exit_code`, so a timed-out
command read as a bare "no exit code".

A command's **identity** is its operation, exact `argv` and effective `cwd`;
`command_run` also includes its profile: `restricted` (a Guarded writing worker), `development`
(a host-writable writing worker in Development or Auto) or `read_only` (read-only roles and automatic
verification). Guarded writer commands run in the task's working tree; Development may select another `cwd`.
Eligible Development/Auto writer commands may use a non-private canonical host
`cwd`; that is distinct from an explicit `outside_command`.
An `outside_command` identity also retains its profile, `outside_confirmed`. Closure compares exact argv/cwd across `command_run` and `outside_command` in either direction; the approved outside route necessarily has a different profile. Within one operation the profile must still match. Both receipts retain their original operation/profile. This exception does not grant permission to run either command. Legacy `command_run` receipts without `cwd` map only to the task root and
restricted profile. An `outside_command` receipt records its `cwd` too. A receipt without
`argv` (or an `outside_command` receipt without `cwd`) has no identity and
matches nothing. A run the gateway rejected before executing it
(`TOOL_REJECTED`) is not history. Neither is a call the runtime settled as
provably never launched (`settled`, `launched:false`, `outcome:"not_started"`,
failure `command_not_started`: its payload never left sandbox preparation): it
does not keep an earlier failure open and is not a later run of the same command
(`never_launched` in `crates/cortex-core/src/lifecycle/worker_checks.rs`). A settled
call with an unknown outcome still counts as an unrecorded later run.

**Revision binding.** A passed run must have checked the work it is cited for. The
check's `revision_binding` says which guarantee holds:

- `{"kind": "checkout"}`: the whole checkout's digest (every deliverable file,
  tracked or untracked, not Git-ignored) was the same before launch and after
  exit (`revision_digest` on the receipt) and equals the digest observed at
  submission. The check ran against exactly the submitted project.
- `{"kind": "work_set", "work_set": [...], "digest": ...}` (`command_run` only):
  the checkout changed during or after the run, but the run's **work set** did
  not. The work set is taken in the transaction that records the command's start:
  the assignment's declared scope entries plus every path that assignment's own
  tools (`project_write`, `project_edit`, `project_delete`, `project_mkdir`)
  recorded so far, across its attempts (`worker_checks::work_set_tx`). The host
  hashes the set's deliverable files (Git-listed tracked and untracked files under
  each entry, plus each literal entry, with mode and link targets;
  `crates/workspace-service/src/native_work_set.rs`) before launch and again once
  the process ended and its receipt settled, and retains both
  (`NATIVE_CHECK_WORK_SET`). A pass binds this way only when both observations are
  equal, the set hashes the same at submission, the record belongs to the
  submitting assignment, and that assignment's tools wrote nothing outside the set
  since the run started. Completion re-establishes the binding from the retained
  record and the recorded result.

Without either, the pass is kept unverified with `revision_mismatch`. The work-set
binding exists because a shared checkout has concurrent writers: an Automatic
sprint runs other tasks' writers in the same folder, and a check that passed
against the worker's own unchanged files was refused every time another task
saved a file anywhere (in one owner store, an exact rerun passed three times while
another task wrote Rust files elsewhere, and the attempt was replanned).

Its limits are exact. It proves that the files of this assignment's declared scope
and its own tool writes are byte-for-byte what the check ran against. It does not
prove that files outside the work set were the same: a test that reads another
task's files, a shared dependency or generated code may pass against a state that
no longer exists, and the binding does not show that. Changes this assignment's
own commands (a formatter, a code generator) made outside its scope are not in the
work set either; a change inside it that was reverted to identical bytes within
the run is invisible, as it is for the checkout digest, and Git-ignored files are
outside both digests. No work set is recorded for a scope of the whole project
(`.`, `**`, an entry with no literal directory such as `*.md`), more than 256
entries, a folder outside a Git repository, a nested checkout in the set, or more
than 20,000 files or 256 MiB; such a run binds only to the whole checkout, as
before. `outside_command` receipts bind only to the whole checkout. None of this
weakens what is accepted later: the independent review binds the task's own files
(or the whole checkout when there are too many of them), and final runtime
verification runs the plan's commands against the real checkout while that binding
holds, so a cross-task breakage still fails there.

- **A failure stays in the result.** A `failed` check is always recorded as
  `failed`. It stops failing the assignment only when it is **closed**: its
  cited run is a verified failure (a complete receipt with a nonzero exit and
  complete retained output), and a `passed` check of the *same* result cites a
  strictly verified matching run (same argv/cwd, with the cross-operation exception above) that is later
  than the failure and is still the latest matching run in the
  attempt, with no later command run of the attempt whose identity is
  unrecorded. The closed check then carries `closed_by` with the closing
  `tool_call_ref`, in the node result, the `worker_result` event and the
  receipt.
- **A mistyped command line is not a failed check.** When a run's own output
  shows the check never started because its command line was mistyped (a usage,
  unknown-option or file-not-found error for the command itself), the worker does
  not cite it as a failed check: it cites the corrected command's own receipt
  and describes the mistyped run in the report. The run stays in the attempt's
  command history; nothing closes it or infers a pass from it, and a test, lint,
  type check or build that ran and reported failures is always a real failed
  check. The advertised `checks[].outcome` description says so. Once a
  submission has cited the run as a failed check, the next rule applies.
- **A presented failure cannot be dropped.** After schema validation and
  before the situational-skill, `.gitignore` and check gates, every new
  `result_submit` retains, in its own transaction, each gateway receipt it
  cites as a passed or failed check that is a verified failed run of the
  attempt or a run of the attempt without a result yet (a
  `CHECK_FAILURE_PRESENTED` runtime event; `record_presented_failures`).
  Refused submissions count. A later submission must cite each retained
  receipt that is now a verified failure again with outcome `passed` or
  `failed`, also with `assignment_outcome: "failed"`; otherwise it is refused
  with `omitted_failed_check` before anything is recorded or published, and the
  channel stays active. Omitting it, relabelling it `not_run` or `unverified`,
  or citing an unrelated passing argv does not count. The retained failure is
  then closed as below, retracted, or reported in an explicitly failed
  assignment. Commands never cited as checks (an incidental search that exited 1),
  and foreign, unknown or non-command references, are not retained.
  `open_failure` also re-derives the omission when a result is recorded.
- **An own exploratory probe can be withdrawn.** A failed check of the worker's own probe whose failure was expected or uninformative (a 404 for a resource that should not exist, a throwaway diagnostic) may carry `probe_withdrawn: <reason>`. It must cite its real nonzero, verified receipt; the submitted outcome stays `failed` as history, the current outcome is `retracted` with `retracted_by: probe_withdrawn`, never `passed`, and reviewers see the reason. A project test, lint, type check or build failure must never be withdrawn. One cited 404 probe once turned an approving review into a failed assignment (2026-10-22).
- **An own-probe invocation error can be retracted.** A standalone `python` or `python3 -c` probe that the worker wrote, with only `-B`, `-I` or `-S` flags, may fail with a Python syntax error, a regex or assertion error from a literal `re` pattern, or a statically provable wrong literal-data expectation. Its complete retained output must show that error, and the probe must not import `pytest` or `unittest`. Such a failed check may carry `invocation_error: {corrected_by, reason}`. `corrected_by` must name a `passed` check of the same result with the same name, a verified receipt of the same attempt, the same operation, cwd and profile, a different argv, and a later order than the failure. No later run may be unrecorded, or be of the failed identity and not pass, and each correction is cited by exactly one check. A syntax correction may change the probe freely if it still parses. A regex correction may change exactly one pattern literal; every other assertion and product call must stay the same. A literal-data correction may change exactly one expected literal in an equality assertion, with the rest of the AST unchanged. The original comparison must be statically false, all corrected comparisons true, and complete output must show a `<string>` `AssertionError`. This shape allows only single-assignment bindings, literal values, earlier bindings and unary `len`, `sorted`, `sum`, `min`, `max`, `all`, `any`, `abs`; no imports, IO, product calls or dynamic execution. Probes are bounded to 8,192 source characters and this literal-data shape to 256 AST nodes. Other languages and unprovable expectations remain ineligible, rather than trusting the worker to waive a failure. The submitted outcome stays `failed` as history, the receipt carries the runtime-derived `retracted_by`, and the current outcome is `retracted`, never `passed`. A retracted check does not fail the node, and both receipts stay in the command history. The reason is an attributed claim for independent review; the runtime does not judge whether the product is correct. Test, lint, build and other project-command failures, and runtime verification, cannot be retracted. A submission whose `invocation_error` is not eligible is refused with `invalid_check_retraction` before anything is recorded.
- **A supported correction supersedes a mistaken invocation.** A check may carry `command_correction: {corrected_by, reason}` instead of `invocation_error` when its own command was mistyped. The `corrected_by` receipt must be a `passed` check of the same name, with a different argv, in the same attempt, operation, cwd and profile, and later than the failure, under the same binding and output gates as the own-probe retraction below. Eligible mistakes are the standalone `python`/`python3` probe errors listed below and a direct Python script's argparse startup error: exit code 2, complete output consisting only of argparse usage and an `unrecognized arguments` or `the following arguments are required` error. The correction must keep the interpreter, its flags and the script path, and may change the arguments. The failed check stays `failed` as history with the runtime-derived `superseded_by`, its current outcome is `superseded`, and the original argv never passes. Shell and module wrappers are not eligible. Real project test, lint and build failures, and runtime verification, cannot be superseded.
- **Whole-tree whitespace diagnostics can be attributed to siblings.** A direct whole-worktree `git diff --check` (exit code 2) whose complete output names only paths reserved by non-review writing sibling nodes of the same plan revision, including siblings that already finished, is recorded as `not_attributable` with those sibling node identities. The original outcome stays `failed` in the history, and a `not_attributable` current outcome is not an open failure. Any unrecognized diagnostic line, path outside those reservations, path in this attempt's own scope, explicit successful own gateway write, read-only or other-revision node, or runtime verification keeps the failure open. The attribution is coordination evidence for the reviewer and does not establish that the working tree is clean.
- **Ambiguity keeps it open.** A rerun with a different `argv` or `cwd`, a
  weak or uncited pass, a failure without its own verified receipt, a later
  run of the same command that failed or has no established result, or an
  unrecorded later run all leave the failure open. A later nonmatching run with a recorded identity does not affect the closure. New submissions are refused
  with `open_failed_check` before publication/channel revocation unless
  `assignment_outcome: "failed"` intentionally submits a failed node. A current failure is never turned into a warning.
- **The latest run decides for a pass too.** A cited, verified pass whose
  command ran again later in the same attempt is recorded `failed` with
  `evidence_gap: "later_run_failed"` when that later run exited nonzero, or
  `unverified` with `later_run_unresolved` when the later run has no established
  result, even when the later run is not cited.
- **The runtime re-derives closure.** `TaskMachine.record_result` (also on
  recovery through `reconcile_results`) fails the node on any open failure. A stored `closed_by` counts only when
  it equals the closing ref derived again from the runtime's own receipts, so a
  retained result without it, or with a forged one, still fails. A stored `retracted_by` is accepted the same way, only when it equals the retraction derived from the receipts.

Runtime verification follows the same rule: completion requires the *latest*
`VERIFICATION_PASSED`/`VERIFICATION_FAILED` event to be a pass for the current
revision. A failed verification recorded after an earlier pass is the current
outcome and blocks completion (`completion_verification_missing`). Earlier failed
verification runs stay in the event history and still count toward the repair
limit.

**Older results are never rewritten.** Stored node results, `worker_result`
events and reports keep what an earlier runtime accepted. Reads through the task
context derive `evidence_strength` for each check under the current rule
(`verified`, `weak` with its `evidence_gap`, or `none` for a `not_run`/`unverified`
disclosure), so an older pass that, for example, relied on an engine `pwd` shows
as `passed` with `evidence_strength: "weak"`. Receipts written before `argv` was
recorded read as `command_unrecorded`.

At synthesis, a retained `not_run` or `unverified` check remains a gap unless
closed by the exact-command rule or the command-less compatibility rule. When
the gap declares a non-empty `command` as literal argv, a verified, still-current
`command_run` pass with exactly matching argv and the task's working
tree as its effective cwd in the same plan revision closes it, including across
nodes. The pass may come from the read-only profile, from the task's own writer profile (for a writing worker in
Development or Auto, the `development` profile), or from the Guarded writer profile (`restricted`); a `development` profile
pass never counts once the task is Guarded. A different cwd cannot substitute for that check. The runtime
reads argv from the cited `tool_call_ref` receipt and keeps the task and attempt evidence
checks.
Alternatively, a runtime-owned final-verification command closes a declared
command gap only with byte-for-byte identical argv, complete exit-0 execution,
complete untruncated retained output and a digest equal to both the gap author's
runtime-observed revision and the current reviewed project digest. Closure reads
the backing task-private output through no-follow descriptors: it must be a
readable owner-only regular file with one link, exactly the recorded byte count,
and stable identity and metadata throughout the read. Missing, shortened,
unreadable, linked or unsafe bodies keep the gap open; validation never repairs
or creates storage. For a reviewer,
the runtime completion event retains the reviewed `project_digest`; for an executor,
the exact attempt/report's `worker_publish` observation supplies the digest. Missing,
ambiguous or different author bindings leave the gap open. Execution may precede
the disclosure: unchanged digest bindings, not temporal order, establish eligibility.
The verification event, execution-start binding and output must belong to this exact
task and plan revision; model claims or unrelated worker receipts cannot supply
that runtime evidence. The runtime retains `closed_by` with the verification
command reference in the node result, preserving the original disclosure and
immutable report. Context and synthesis rederive closure from evidence, never
trust a stored `closed_by`. Failed, pending, ambiguous, missing-start, stale or
truncated evidence keeps the gap open. Speculative verification can close a
reviewer's later-recorded gap only after authoritative credit on the same approved
and current digest; diagnostic starts or discarded candidates cannot close it. A
same-name passing check in the original node or one of its later fix nodes does
not override a different or missing command. For a legacy gap without a
command, the compatibility rule allows a same-name passing check in that node's
fix lineage to close it; a same-name result on an unrelated node is
insufficient. An `outside_command` has a separate working directory and cannot
close a command-bearing gap by argv alone. Every remaining synthesis
`not_verified` item requires the gap's exact check name and a nonblank concrete
blocker.
Command verification runs literal argv in the read-only host profile
 in every access mode. The host and HOME are read-only except
the tool-cache allowlist; scratch and `/tmp` are private or writable; the command has its own
PID namespace; the protected control plane is read-only and the task store is masked; only
the project's git-ignored directories (caches, builds, installed dependencies) are writable.
The check records the `read_only` profile. Credentials are filtered from the environment,
but the owner's network is shared, as for every owner command. Missing isolation, failed
commands, incomplete output or a changed project digest cannot establish a pass. Each
command runs until it exits, with no runtime time limit; stopping the task ends it, and that
interrupted run cannot establish a pass.

Verification argv contract. `plan.verification` is one top-level field of the plan, not a
property of each task. Each entry is one literal argv command (an array of string arguments,
no shell); `plan_submit` needs at least one task and at least one verification command.
Submission rejects shell syntax before any work starts. The coordinator may call
`verification_submit` only after a runtime verification failed and the work itself is
correct: it replaces the verification commands and verifies again without redoing the
reviewed work. Wrong code requires a new plan. Any failed command of the failed
run that the revision no longer contains, whatever the list length, is recorded
in `VERIFICATION_REVISED.dropped_failed`. Ordinary repair plans carry every
unresolved final-verification argv into the next revision. Completion requires either a
complete passing runtime verification of the exact removed argv or an answer to
a task-global question asked after that removal (`completion_dropped_failed_check`;
the planning prompt tells the coordinator to ask which way to go). Answers to
older questions, unknown questions and worker-local questions do not acknowledge
removals. A one-for-one corrected command is a replacement, so the failed command is
recorded as dropped too, and cannot erase an earlier removal. The policy does not rest
on recognizing weak commands: only the same argv (a re-run) clears a failed check. Any
different command that replaces it, however strong or weak, leaves the failed command
as an open obligation, so the task cannot complete until the exact command passes or the
owner answers a task-global question asked after the replacement. A retry reply,
worker-local answer or a new ordinary plan does not waive the obligation. As an
extra signal, a
new command that the plan did not already contain and that the evidence classifier
treats as checking nothing (`true`, `:`, `echo ...`, `sh -c ...`) is refused outright as
`verification_trivial_replacement`. At plan acceptance, a plan whose verification
contains such a command is refused as `verification_trivial_command`.
Twice a desktop placement check that failed only for environment reasons was removed
and the task completed on lint and unit tests while the popup it should have checked
still moved (2026-10-20/22). Runtime verification runs in one fixed sandbox; the
`verification_submit` contract tells the coordinator that no user consent, permission or
mode changes it.

Reconciliation no longer asks for `replan_request` only to close retained `not_run` or
`unverified` checks; the coordinator lists each in `not_verified` with its blocker and
replans for verification only when the user asked for that check or an unverified changed
surface puts the requested result at real risk. Evidence-only replans made one task take
8 revisions (2026-10-22). The `SYNTHESIS_RUNTIME_DECLINED` diagnostic no longer reports
`review_not_approved` when the runtime waived review for an unchanged project.

Completion of planned work requires current completed nodes, an approving review
(or the runtime's `REVIEW_NOT_REQUIRED` record for an unchanged project), complete passing
verification for the exact plan/project revision, intact reports, synthesis and no
live or unknown worker session. Runtime pipeline editions project the durable state;
the pipeline is one newest-first Markdown file and other reports remain immutable.

For a simple informational question, the coordinator reads relevant files and
submits an answer without a plan or workers. The runtime transitions through
ANSWER_READY and validates the exact answer receipt before completion. Any plan
revision, node or worker provisioning disallows this path. There are three
exceptions. First, the user answered a runtime stop question, or a question
asked after verification, by choosing to finish. Second, a new user message
reopened a task that had reached `COMPLETED`, and since that `TASK_REOPENED`
event no plan was accepted, no assignment or replan was created, no node is
pending or running, and no worker was provisioned outside an earlier
assignment. Third, a user message was applied after the current revision's
latest verification passed, returning it to planning before completion, and
since that verification no plan, assignment, replan or verification repair was
accepted, no node is pending or running and no worker was provisioned outside an
earlier assignment; that answer completes only if the revision still passes
every completion gate (see
[steering after verification](../steering-and-questions/index.md)). The runtime
derives all three from its own committed events, never from the coordinator's text. `COMPLETED` is reached only through the runtime's
completion checks. So a follow-up question about finished work gets a direct
answer; before this, it ended blocked with
`structured_proposal_missing`. Once a reopened task accepts a new plan, that
revision again needs its review and verification. The PLANNING prompt offers
`answer_submit` only when the runtime would accept it. Project mutations stay
unavailable to the coordinator. A direct answer never reaches the review stage,
and planned work that changes the project keeps the full review gate.

Paseo hides thinking and protocol bookkeeping (including wait) in chat and worker
panels, live and on replay. Meaningful MCP operations, private-store tool cards,
CUA calls and generic/titleless tool updates retain readable, expandable details. Logical tool IDs remain stable across lifecycle updates and replay.
Credential redaction remains; a downstream terminal reference cannot be relayed as
if it belonged to the incoming client's terminal namespace. Only actually exposed
engine events can be forwarded—absent native events are never fabricated.
Each worker has one timeline card with a fixed event budget of at most 13 updates,
whatever its tool count, output or duration. Its complete activity lives in the
durable Workers panel history, not in host timeline events. The coordinator's
own tool cards share one per-task budget: after 150 distinct calls, further
calls are counted on a single aggregate card, and failures keep their own card
for another 50 calls. User questions and permission requests are never folded. See
[bounded ACP event projections](../observability/index.md#bounded-acp-event-projections).

Engine processes run in the engine jail, whose launch-time writable project
mount for a writing worker can be broader than its runtime-declared assignment
scope; native subagents are disabled. The runtime gateway checks current
channel authority and declared assignment scope before each operation,
contains commands and external MCP servers with bubblewrap, and retains exact
receipts. Worker access requests extend declared writer reservations only, not
the OS jail. See [permissions and containment](../permissions-and-containment/index.md).

## Worker questions and additional access

The worker-only `ask_question` contract reserves user questions for genuine
decisions only the user can make, not missing project access, permissions,
work paths or evidence available through runtime tools. Workers use
`access_request` for additional project paths and the existing scope/replan
routes only when decomposition must change; unavailable evidence remains an
explicit report limitation. The runtime does not classify arbitrary question
prose with keyword heuristics, and a question or answer never grants access.

The owner is never asked the same question twice. A worker `ask_question` or a
coordinator `user_question_request` whose question and set of options equal (case,
spacing and trailing punctuation aside, options in any order) a question of the same
task the owner already answered is not asked: the tool reply is `outcome: refused`,
`error: question_already_answered`, with the earlier `question_id` and the owner's
recorded `answer` (every repeated member in `answers` for a batch), no wait or
pending question is opened and `LOOP_BROKEN` (`kind: owner_question`) is recorded.
Only answers since the newest reopen or owner message count, so after either the
question may be asked again. A skill proposal keeps its own rules.

An obstacle the owner already settled is not raised again. A failed node's or
stop's owner decision that repeats one the owner answered in any revision since the
newest reopen or owner message (its text cites the answered question, names only
the same files, or uses mostly the same words) opens no second
`OWNER_DECISION_REQUIRED` and no `ask_owner` continuation: it is classified
`technical` (`rule: owner_decision_answered`) with the owner's answer in its detail,
and the failed node's result carries the disclosed `unverified` runtime check `Owner
decision applied: …`. Missing access whose owner action for the same service was
resolved or dismissed twice (`ACCESS_REPEAT_LIMIT`) opens no third action: the
failure is `technical` (`rule: access_repeats`) and the node's result discloses
`Access to <service> not available`. Both record `LOOP_BROKEN` (`action:
disclosed_limit`). A rate limit (an HTTP 429 or a rate-limit phrase in the worker's
words, the recorded texts or a service command's output) is an outage for any host
and beats a missing-access reading of the same evidence: a built-in endpoint is
rechecked as before, any other host gets a time-only wait (`probe: timer`) that ends
after its backoff (`EXTERNAL_WAIT_ENDED` reason `backoff_elapsed`) and retries the
node without contacting the host. "Disk quota exceeded" is not a rate limit.

Each worker turn receives `user_question_language` (`en` or `ru`) derived
from the same selected language/automatic language resolution used by the
coordinator. The advertised question contract requires the question and every
option, including batch members, in that language. Worker-facing briefs,
reports and other communication remain English. Ordinary, recovery, answer and
fallback turns refresh this directive. Strings are relayed verbatim to structured
forms or text fallback; there is no runtime translation or deterministic
natural-language validation. Automated scripted tests prove directive delivery
and string transport, not arbitrary model compliance.

The route registers an assignment-local wait and returns promptly. The worker ends its turn without publishing an incomplete
result. Once a real answer is recorded, the runtime sends it as the next prompt
to the same assignment, attempt and retained healthy engine session. Sibling
assignments can continue; the coordinator is not restarted and need not submit a
replacement plan for this normal flow. A declined, expired or text-only host
delivery leaves the wait open. Multiple open worker questions require explicit
selection. Stop, cancellation, replan, ownership changes or loss of the owning
session can cancel or orphan a wait; a lost process may require replacement
rather than seamless continuation.

A recovered, current-epoch worker channel can lack retained engine-session
metadata. Ordinary tools remain available under their normal authority checks.
Its question then uses the existing task-global `WAITING_USER` fallback and
resumes the saved execution/review phase after an exact answer; it does not
register an assignment-local wait or promise same-session continuation.
Access requests still require a retained session for their decision continuation.

Without active node waits, the execution/review scheduler skips question delivery
and access reconciliation, reusing its single-snapshot context and cheap change
fingerprint. A live worker with an open access wait is an exception: its persisted
`retry_at` is rechecked by the same waiting session when due, so an active-writer
conflict cannot leave the worker parked after its timer. Periodic idle passes remain
write-free. A current running node
conservatively reserves its declared paths even before an assignment binding
exists; stale unbound revisions do not hold those paths indefinitely.

The worker-only `access_request` route asks to add exact project-relative paths
to the worker's declared assignment scope. The runtime grants the complete
request automatically only when all paths pass safety and role checks, queued
and applied owner deny-write restrictions, and current writer-reservation
checks. Existing grants and plan scope remain unchanged; no partial grant occurs.
A refusal parks only that assignment for a runtime decision continuation, never
a user access question. Active-writer conflicts get three runtime rechecks after
persisted delays of 1, 2 and 4 seconds (module-level, test-injectable
`RETRY_DELAYS_SECONDS`); deadlines and attempt indices persist in wait events.
The live worker session wakes at each persisted deadline and reconciles the wait
without sleeping under a database lock. A due wait is therefore processed even
when the task has no other runnable node.
Scheduling can deliver a check later than its deadline; there is no catch-up
busy loop. A successful check grants transactionally and continues the same
healthy session. An exhausted conflict cycle returns a runtime denial.
Private/non-canonical/unsafe paths, owner deny-write policy and read-only roles
return runtime denials immediately, without retries. Authority loss cancels or
orphans the continuation.

Access waits never create pending user questions or synthesize user answers.
Retained pre-fix access cards are cancelled and policy is rechecked by the
runtime; late card answers cannot grant access. No decision overrides private
paths, deny-write policy, read-only roles or live writers. A ready grant is
revalidated before continuation. The complete request remains all-or-none.
After denial the worker may use an allowed alternative, issue a new logical
access request when conditions change, or use scope/replan for a replacement
decomposition; it must not replay a refused action.

A granted extension reserves additional paths to coordinate parallel writers.
The ordinary assignment reservation is not a gateway write boundary. It does not
remount or widen the OS jail, command mounts or read scope, and native-tool actions
remain subject to existing jail, role and owner-policy boundaries. The logical wait fences Cortex gateway writes, command starts and
publication while active, but is not a universal freeze of arbitrary native
edits. See [permissions and containment](../permissions-and-containment/index.md).

Steering fences existing workers before replanning. A coordinator scope, replan
or escalation request enters `RECOVERING`; the coordinator ends that turn and
does not submit a plan until the runtime has stopped workers and requests a
replacement plan. RECOVERING cancels and terminates only the assignments that were live when the stop or replan cancelled their nodes; completed and failed assignments are left alone (see [recovery cancels only live assignments](../failures-and-cancellation/index.md#recovery-cancels-only-live-assignments)). Worker questions and additional-path access requests instead
create assignment-local waits: they park only the requesting worker and do not
require coordinator replanning. See [worker questions and additional access](#worker-questions-and-additional-access).
Every answered worker question, with its question text, the user's verbatim
answer, ids, timestamps and answering session, is visible to later workers and
the reviewer through `list_sources`/`read_source` and the recovery capsule's
constraints (see [input history](../input-history-and-replay/index.md#answered-questions)).
Lost workers are retried only after their absence is confirmed, and an
uncertain action receipt prevents replay (a receipt settled with an observed
end, such as an owner stop, or that provably never launched, is not uncertain). A worker failure about the work is
retried at most twice. A lost worker, or one whose engine crashed or would not
start, falls under the [transient-failure retry window](../failures-and-cancellation/index.md#transient-engine-failures).
The delay before each retry grows to at most a minute, and retries stop only
after failures have kept repeating for 10 minutes. A worker whose route itself failed (an
engine-reported exhausted plan, rate limit or unavailable model, or an engine
that would not start or confirm the worker's mode) is instead retried on the
next available route, possibly another provider. Those retries don't count
against the two ordinary retries and are bounded separately. A routing refusal
stops the work under its own name and is never retried. See
[routing and fallback](../routing-and-fallback/index.md#automatic-worker-routing-unavailable-routes). Coordinator, worker and review turns have no runtime total or idle kill
deadline; a stalled engine receives an adapter nudge rather than being killed. Scope reservations remain occupied until engine termination
is observed. A task waiting for another task's capacity remains pending with a
bounded wait. Senior escalation (`escalation_request`, the only consultant route) inserts a
read-only consultant routed automatically by the runtime. The runtime builds that
consultant's packet from the reports the escalation names and the escalated revision's
reports; a plan that names the consultant is refused. Three accepted automatic replans since the latest user message or
answer and three review cycles bound the respective loops; exhausted limits require
user input or a new plan, and any user message or completed question answer restores
the replan budget. Worker-local answers emit `WORKER_USER_ANSWER_RECEIVED` in the
answer transaction; a batch resets the budget only once all its answers are
recorded. Runtime access decisions never count as user input.

Materialized sprint coordinators can inspect their authenticated dependency
graph with `sprint_graph` and read a member's bounded durable outcome and
published reports with `sprint_task_result`. The runtime supplies direct
predecessor/dependent context at coordinator start and keeps these reads within
the current sprint. Code availability is reported only from exact, receipt-bound
file evidence. The graph view does not alter dependency scheduling, review,
verification or acceptance.

## Timing diagnostics

The runtime records metadata-only startup, first-update and changing scheduler
wait observations. They do not alter dependency, scope, capacity, review or
release decisions. The former phase timeline reader was removed (see [removed readers](../observability/index.md#removed-python-era-readers)); missing historical evidence is not inferred.
