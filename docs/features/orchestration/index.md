# ACP orchestration

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed.


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
permission change is introduced. Source: `orchestration/reviewer_verification.py`,
`orchestration/gateway.py`, `orchestration/controller.py` and
`delegation/evidence.py` below `src/cortex_runtime/`.

## Plan submission and coordinator polling

Plan task `scope` is a write reservation used to schedule overlapping writers,
not a read-access list. Reading needs no scope; writing tasks must not list
read-only dependencies there. Read-only/investigation roles retain the required
non-empty context scope, but it neither reserves writes nor serializes writers.
The existing access policy, not scope wording, decides mutation authority.


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

After accepting a plan, `plan_submit` tells the coordinator that workers are
dispatched after the current turn ends and to end the turn rather than poll.
When `task_get_context` shows no coordinator action is needed, it gives the same
turn-ending hint. An identical repeat read on that channel returns a compact
unchanged-state receipt instead of resending the plan and nodes. The hint is
advisory; the runtime does not force a model to end its turn. Worker dispatch
timing remains at the turn boundary.

`orchestration/state.py` stores task state, original goal, plan revision, dependency
nodes, structured results, events and request receipts in `.cortex/cortex.sqlite3`.
State changes and their events commit together. A plan is validated for exact fields,
known roles, unique node names, existing dependencies, cycles, bounded scope and
verification commands. Verification commands run as literal argv without a shell, so plan submission rejects command substitution, backticks, redirection, pipes, shell operators and standalone parentheses; use a documented project command or script instead. Each new plan node requires a `model` recommendation (`auto`, engine id, or `engine/model`) and may include `model_reason`; neither field grants dispatch authority. The runtime computes tier and effort and revalidates policy and availability. See [automatic routing](../routing-and-fallback/index.md#automatic-worker-routing-tiers-model-hints-and-ranking).

`orchestration/controller.py` drives planning, execution, independent review,
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
per-connection limit. A worker queued for available session or connection
capacity waits in bounded periods (`capacity_queue_timeout_seconds`, 600 s);
cancellation ends the wait at once. A finished worker continues to occupy its
session slot until its engine is confirmed gone and its reservation is released
or safely reclaimed. Scope overlap serializes writing nodes across every task of
the project, including tasks of other chats; review-stage nodes and READ_ONLY
roles neither block nor are blocked by overlapping scopes. Runtime routing resolves model and
effort from the current tier, policy, catalog and plan usage; plan hints remain
advisory. All 23 specialist/consultant profiles remain available.

A planned step that cannot start because of `scope_overlap` or
`capacity_limit` is shown in the chat as a runtime "Waiting" card
(`orchestration/wait_notices.py`). The card names what holds the step: for a
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
which bounds a slot or path release that is never observed. That deadline does
not run while every shown wait is a scope wait on a worker observed alive
through its recorded process identity (a matching pid, start time and boot):
such a worker, often in another chat, releases its paths when it ends. The
deadline starts afresh once the holder is gone, finished but still releasing,
or cannot be verified; capacity waits always keep it.

For streamed coordinator text, the controller reads only the task's durable
state; it does not rebuild the full context for each delta. A full
`TaskMachine.context()` read uses one SQLite read transaction, and assignment
attempt lookups for result checks are batched.

The same state read withholds the coordinator's text after its own question.
Once `user_question_request` has committed `WAITING_USER`, the controller drops
further coordinator deltas observed in that engine turn. No tool lifecycle
callback releases them: even the question's first callback can arrive after a
post-question rewrite, so a new id is not evidence of pre-question ordering.
Text received before the commit (including its buffered redactor tail) and text
in later turns after the answer are still shown. The runtime renders the question
itself. The conservative boundary can also omit genuine pre-question details
that the adapter delivers only after the commit; the callbacks cannot distinguish
them from a rewrite. Final-only turn text is likewise withheld when the question
was observed or the task is still `WAITING_USER`, rather than replayed unsplit.
This is display filtering, not engine cancellation or a history rewrite. The coordinator's
question receipt also carries a `next` hint telling the model to end the turn without
further text; like the plan and context hints, that hint is advisory.

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
queue the reviewer. Review remains independent and digest-bound; MCP revocation
does not intercept native filesystem edits, which still invalidate the review
binding. A late engine failure, timeout or cancellation does not replace an
accepted result or authorize a retry of that completed node. Verification and
final completion gates are unchanged, including confirmed absence of live worker
sessions at completion.

Retried nodes and fix nodes are covered because the decision
compares the whole project, not individual worker claims. No model argument or
hint can waive review; `EXECUTING → VERIFYING` is refused without the runtime's own
record. A review result binds to the project digest recorded in `REVIEW_STARTED`.
If the project changed while the review ran, for example because another writer
edited the same checkout, `result_submit` refuses the verdict with
`review_project_revision_changed` before publishing anything. It then records a new
`REVIEW_STARTED` for the current digest (`rebound_from`, `rebind`,
`refused_request_key`). The reviewer re-checks the changes and submits with a new
request_key. The refused request_key never records a verdict
(`review_resubmit_required`). After `MAX_REVIEW_REBINDS` (3) re-bindings, a further
change is refused as `review_project_revision_unstable`. Before this, the binding
was never refreshed, so one change refused every later submission of that review.
APPROVE proceeds
to verification; REQUEST_CHANGES creates fix nodes followed by a fresh reviewer;
BLOCK stops acceptance. Three unsuccessful review cycles block further automatic
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

Source: `orchestration/reviewer_handoff.py`, `controller.py`,
`worker_brief.py`, and `state.py`. Focused regressions:
`tests/test_reviewer_handoff.py`.

### Targeted review fixes

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
the plan, so related fixes keep the plan's order. In every other case — no
findings, an unknown task, a cross-cutting finding, a file outside the scope or
findings too long for one objective — the runtime keeps the conservative fix of
every original plan task with the review summary. These broad fallback fixes
retain the original plan's direct dependency edges: each fix depends on the
review and on generated fixes for its direct plan parents. Independent branches
remain parallel. It never infers a contract change; a reviewer marks one
`cross_cutting`. `REVIEW_CHANGES_REQUESTED` records
`targeted` and the `fallback` reason. The three-cycle ceiling is unchanged.

Worker results bind to a published report owned by the exact provisioned attempt.
The result receipt retains a content digest.

### Check evidence

`result_submit` `checks` are automated checks: each one is an executed command
with an exit code. Qualitative review remarks, read files and inspected code belong
in the report body, not in `checks`. `project_git` status/diff is context
only, not a check receipt: its retained `large_output_id` is an output identity,
not a `tool_call_ref`. The response does not expose command-receipt
`exit_code`/`complete` fields. To check a Git invariant, run a meaningful
exit-status check such as `git diff --check` through `command_run`.
The advertised `result_submit` schema carries
this contract in its property descriptions; `delegation.evidence.assess_check`
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
accepted as descriptive text on a passed or failed check but omitted from the
retained accepted check; only the gateway receipt establishes the run. Reviewers
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
   truncated, with the same exit code and no failure;
4. recorded with its `argv`, and that command is not a trivial stand-in (below);
5. cited by no earlier check of the same result (one command, one check).

**Trivial-command rule** (`delegation.evidence.trivial_command`). After leading
`NAME=value` assignments and the launchers `env`, `timeout`, `nice`, `nohup`,
`command`, `exec` and `time` are removed, a command is trivial when nothing is
left; when its program basename is a no-op, printer, listing, file read or search
(`pwd`, `ls`, `echo`, `printf`, `cat`, `head`, `tail`, `tee`, `grep`, `rg`, `find`,
`true`, `test`, `cd`, `env`, `which`, `stat`, `wc`, `sort` and the rest of
`TRIVIAL_PROGRAMS`); when it is a read-only `git` subcommand (`status`, `log`,
`show`, `branch`, `rev-parse`, `ls-files`, `remote`, `config`, `describe`) or
`git diff` without `--check`, `--exit-code` or `--quiet`; or when it is a
`sh`/`bash`/`dash`/`zsh`/`ksh` `-c` script whose exit status cannot come from a
nontrivial command. For a script, its last command decides: a trivial last command
masks everything before it unless joined by `&&` (also `;`/newline under `set -e`
or `-e`, and `|` under `set -o pipefail`); `exit`, `exit $?` and `|| exit N`
(N nonzero) pass the previous status through, `exit 0` masks it; an unparsable
script is trivial. So `pytest || true`, `pytest; echo done` and
`pytest | tail -5` are trivial, while `cd web && npm test` and
`set -o pipefail; pytest | tee log` are not. Anything else (a test runner, linter,
type checker, build or project script) may verify a check. The rule does not prove
that a nontrivial command tests *the named* check (for example an inline
`python3 -c` program); that correspondence is the independent reviewer's
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
request key reusable. The result receipt returned to the worker includes `node_status` and lists
every check's recorded `outcome` with its `claimed_outcome`/`evidence_gap`, and
`closed_by` for a closed failure (below).
`evidence_gap` codes: `tool_call_ref_missing`, `tool_call_ref_unknown`,
`foreign_attempt`, `ref_already_cited`, `not_a_command`,
`engine_command_unverifiable`, `receipt_missing`, `ambiguous_ref`,
`call_incomplete`, `nonzero_exit`, `zero_exit` (a failed claim citing exit 0),
`result_missing`, `result_truncated`, `command_unrecorded`, `trivial_command`,
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

### Check history and current outcome

A long attempt often runs the same check several times: it fails, the worker
fixes the code, and the rerun passes. The runtime keeps that run history
separate from each check's current outcome
(`delegation.evidence.current_outcomes`, over `command_history`, the
attempt's gateway command runs in `TOOL_AUTHORIZED` order).

Verification subprocess output is streamed from its temporary file into the
private large-output store, preserving the complete bytes. The tool event
carries a bounded tail excerpt and the output record ID, rather than the full
log. The excerpt is display-only; the retained output remains the complete
verification evidence. Existing exit-code, interruption and project-integrity
checks still determine whether verification passes.

A command's **identity** is its operation, exact `argv` and effective `cwd`;
`command_run` also includes its restricted or Development profile. Restricted
commands run in the task's working tree; Development may select another `cwd`.
Legacy `command_run` receipts without `cwd` map only to the task root and
restricted profile. An `outside_command` receipt records its `cwd` too. A receipt without
`argv` (or an `outside_command` receipt without `cwd`) has no identity and
matches nothing. A run the gateway rejected before executing it
(`TOOL_REJECTED`) is not history.

- **A failure stays in the result.** A `failed` check is always recorded as
  `failed`. It stops failing the assignment only when it is **closed**: its
  cited run is a verified failure (a complete receipt with a nonzero exit and
  complete retained output), and a `passed` check of the *same* result cites a
  strictly verified run (above) of exactly the same identity that is later
  than the failure and is still the latest run of that identity in the
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
  check. The advertised `checks[].outcome` description and the
  `open_failed_check` correction say so.
- **Ambiguity keeps it open.** A rerun with a different `argv` or `cwd`, a
  weak or uncited pass, a failure without its own verified receipt, a later
  run of the same command that failed or has no established result, or an
  unrecorded later run all leave the failure open. New submissions are refused
  with `open_failed_check` before publication/channel revocation unless
  `assignment_outcome: "failed"` intentionally submits a failed node. A current failure is never turned into a warning.
- **The latest run decides for a pass too.** A cited, verified pass whose
  command ran again later in the same attempt is recorded `failed` with
  `evidence_gap: "later_run_failed"` when that later run exited nonzero, or
  `unverified` with `later_run_unresolved` when the later run has no established
  result, even when the later run is not cited.
- **The runtime re-derives closure.** `TaskMachine.record_result` (also on
  recovery through `reconcile_results`) fails the node on any open failure
  (`delegation.evidence.open_failure`). A stored `closed_by` counts only when
  it equals the closing ref derived again from the runtime's own receipts, so a
  retained result without it, or with a forged one, still fails.

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
restricted `command_run` pass with exactly matching argv and the task's working
tree as its effective cwd in the same plan revision closes it, including across
nodes. Development or different-cwd receipts cannot substitute for that check; the runtime reads argv from the cited
`tool_call_ref` receipt and keeps the task and attempt evidence checks.
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
Command verification runs literal argv
in an explicit Linux namespace sandbox with no user home, inherited credentials,
network, or access to the real runtime store. Missing isolation, failed commands,
incomplete output or a changed project digest cannot establish a pass. Each
command runs until it exits, with no runtime time limit; stopping the task ends
it, and that interrupted run cannot establish a pass.

Completion of planned work requires current completed nodes, an approving review
(or the runtime's `REVIEW_NOT_REQUIRED` record for an unchanged project), complete passing
verification for the exact plan/project revision, intact reports, synthesis and no
live or unknown worker session. Runtime pipeline editions project the durable state;
the pipeline is one newest-first Markdown file and other reports remain immutable.

For a simple informational question, the coordinator reads relevant files and
submits an answer without a plan or workers. The runtime transitions through
ANSWER_READY and validates the exact answer receipt before completion. Any plan
revision, node or worker provisioning disallows this path. There are two
exceptions. First, the user answered a runtime stop question, or a question
asked after verification, by choosing to finish. Second, a new user message
reopened a task that had reached `COMPLETED`, and since that `TASK_REOPENED`
event no plan was accepted, no assignment or replan was created, no node is
pending or running, and no worker was provisioned outside an earlier
assignment. The runtime derives both from its own committed events, never from
the coordinator's text. `COMPLETED` is reached only through the runtime's
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
fingerprint. Periodic idle passes remain write-free. A current running node
conservatively reserves its declared paths even before an assignment binding
exists; stale unbound revisions do not hold those paths indefinitely.

The worker-only `access_request` route asks to add exact project-relative paths
to the worker's declared assignment scope. The runtime grants the complete
request automatically only when all paths pass safety and role checks, queued
and applied owner deny-write restrictions, and current writer-reservation
checks. Existing grants and plan scope remain unchanged; no partial grant occurs.
A refusal parks only that assignment for a runtime decision continuation, never
a user access question. Active-writer conflicts get three runtime rechecks after
delays of 1, 2 and 4 seconds (module-level, test-injectable
`RETRY_DELAYS_SECONDS`); deadlines and attempt indices persist in wait events.
The scheduler reconciles due checks without sleeping under a database lock.
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

These declared-scope extensions coordinate runtime writers; they do not
dynamically remount or widen the OS jail. In the current launch boundary,
ordinary writing workers already have whole-project write authority, and
native-tool actions remain subject to existing jail, role and owner-policy
boundaries. The logical wait fences Cortex gateway writes, command starts and
publication while active, but is not a universal freeze of arbitrary native
edits. See [permissions and containment](../permissions-and-containment/index.md).

Steering fences existing workers before replanning. A coordinator scope, replan
or escalation request enters `RECOVERING`; the coordinator ends that turn and
does not submit a plan until the runtime has stopped workers and requests a
replacement plan. Worker questions and additional-path access requests instead
create assignment-local waits: they park only the requesting worker and do not
require coordinator replanning. See [worker questions and additional access](#worker-questions-and-additional-access).
Every answered worker question, with its question text, the user's verbatim
answer, ids, timestamps and answering session, is visible to later workers and
the reviewer through `list_sources`/`read_source` and the recovery capsule's
constraints (see [input history](../input-history-and-replay/index.md#answered-questions)).
Lost workers are retried at most twice after confirmed absence, and uncertain
action receipts prevent replay. A worker whose route itself failed (an
engine-reported exhausted plan, rate limit or unavailable model, or an engine
that would not start or confirm the worker's mode) is instead retried on the
next available route, possibly another provider. Those retries don't count
against the two ordinary retries and are bounded separately. A routing refusal
stops the work under its own name and is never retried. See
[routing and fallback](../routing-and-fallback/index.md#automatic-worker-routing-unavailable-routes). Coordinator, worker and review turns have total
and idle deadlines. Scope reservations remain occupied until engine termination
is observed. A task waiting for another task's capacity remains pending with a
bounded wait. Senior escalation inserts a read-only consultant routed automatically
by the runtime. Three accepted automatic replans since the latest user message or
answer and three review cycles bound the respective loops; exhausted limits require
user input or a new plan, and any user message or answer restores the replan budget.

## Timing diagnostics

The runtime records metadata-only startup, first-update and changing scheduler
wait observations. They do not alter dependency, scope, capacity, review or
release decisions. Use the [read-only phase timeline](../observability/index.md#orchestration-phase-measurement)
to analyze delays; missing historical evidence is not inferred.
