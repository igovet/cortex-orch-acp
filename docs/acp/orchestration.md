# ACP orchestration

Cortex is the incoming ACP agent and the owner of orchestration. ACP transports
messages and sessions; the models propose plans, solve assignments and synthesize
results. The runtime chooses what executes and whether the task can complete.
See [implementation status](runtime-rewrite-status.md) for retained test evidence
and its automated evidence limits.

## Plan submission and coordinator polling

The advertised `plan_submit` contract requires both `plan.tasks` and
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
verification commands. Verification commands run as literal argv without a shell, so plan submission rejects command substitution, backticks, redirection, pipes, shell operators and standalone parentheses; use a documented project command or script instead. Each new plan node requires a `model` recommendation (`auto`, engine id, or `engine/model`) and may include `model_reason`; neither field grants dispatch authority. The runtime computes tier and effort and revalidates policy and availability. See [automatic routing](routing-and-fallback.md#automatic-worker-routing-tiers-model-hints-and-ranking).

`orchestration/controller.py` drives planning, execution, independent review,
verification and synthesis. Only dependency-ready nodes are reserved for execution.
Reservations commit before provisioning. Each connection's
`max_concurrent_sessions` limit (default 4, configurable from 1 through 64)
applies separately to coordinator and worker sessions, so the combined count
may exceed that value. The project worker pool is sized to 64, the validated
per-connection maximum; it does not tighten any single connection's limit,
though it can cap aggregate workers across connections. A worker queued for a
full connection waits in bounded periods (`capacity_queue_timeout_seconds`,
600 s); a period ends in `capacity_timeout` only when some slot is held by a
reservation that is ambiguous or whose session is not observed alive. While every
slot is held by a live session, the wait continues, because that work will release
the slot. Cancellation still ends the wait at once. Scope overlap
serializes writing nodes; review-stage nodes and READ_ONLY roles neither block nor are blocked by overlapping scopes. Runtime routing resolves model and
effort from the current tier, policy, catalog and plan usage; plan hints remain
advisory. All 23 specialist/consultant profiles remain available.

For streamed coordinator text, the controller reads only the task's durable
state; it does not rebuild the full context for each delta. A full
`TaskMachine.context()` read uses one SQLite read transaction, and assignment
attempt lookups for result checks are batched.

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
schedules review. Retried nodes and fix nodes are covered because the decision
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
every original plan task with the review summary. It never infers a contract
change; a reviewer marks one `cross_cutting`. `REVIEW_CHANGES_REQUESTED` records
`targeted` and the `fallback` reason. The three-cycle ceiling is unchanged.

Worker results bind to a published report owned by the exact provisioned attempt.
The result receipt retains a content digest.

### Check evidence

`result_submit` `checks` are automated checks: each one is an executed command
with an exit code. Qualitative review remarks, read files and inspected code belong
in the report body, not in `checks`. The advertised `result_submit` schema carries
this contract in its property descriptions; `delegation.evidence.assess_check`
enforces it.
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
claim whose cited command exited nonzero is recorded as `failed` (an observed
failure is never softened) with `evidence_gap: "nonzero_exit"`. A claimed `failed`
check is always kept as `failed`. The result receipt returned to the worker lists
every check's recorded `outcome` with its `claimed_outcome`/`evidence_gap`, and
`closed_by` for a closed failure (below).
`evidence_gap` codes: `tool_call_ref_missing`, `tool_call_ref_unknown`,
`foreign_attempt`, `ref_already_cited`, `not_a_command`,
`engine_command_unverifiable`, `receipt_missing`, `ambiguous_ref`,
`call_incomplete`, `nonzero_exit`, `zero_exit` (a failed claim citing exit 0),
`result_missing`, `result_truncated`, `command_unrecorded`, `trivial_command`,
`later_run_failed`, `later_run_unresolved` (below).

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

A command's **identity** is its operation, its exact `argv` and its scope:
`command_run` always runs in the task's working tree; an `outside_command`
receipt records its `cwd`, which is part of the identity. A receipt without
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
- **Ambiguity keeps it open.** A rerun with a different `argv` or `cwd`, a
  weak or uncited pass, a failure without its own verified receipt, a later
  run of the same command that failed or has no established result, or an
  unrecorded later run all leave the failure open, and the node fails as
  before. A current failure is never turned into a warning.
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

At synthesis, a retained `not_run` or `unverified` check remains a gap unless a
passed check with the same normalized name belongs to that node or one of its
later fix nodes. A gap may also declare optional `command` as literal argv; a
verified, still-current `command_run` pass with exactly that argv in the same
plan revision closes it across nodes. The runtime reads argv from the cited
`tool_call_ref` receipt and keeps the existing task and attempt evidence checks.
An unrelated node's matching check name alone does not close a gap. An
`outside_command` has a separate working directory and cannot close a gap
by argv alone. Every remaining synthesis `not_verified` item requires the
gap's exact check name and a nonblank concrete blocker.
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
[bounded ACP event projections](observability.md#bounded-acp-event-projections).

Engine processes run in the engine jail, where their native tools write only the
assignment scope; native subagents are disabled. The runtime gateway
checks current channel authority and assignment scope before each operation,
contains commands and external MCP servers with bubblewrap, and retains exact
receipts. See [permissions and containment](permissions-and-containment.md).

Steering fences existing workers before replanning. A scope, replan or escalation
request enters `RECOVERING`; the coordinator ends that turn and does not submit a
plan until the runtime has stopped workers and asks for the replacement plan. Worker questions persist an
exact pending question and pause execution; only a real matching answer resumes
it. Every answered question, with its question text, the user's verbatim answer,
ids, timestamps and answering session, is visible to later workers and the
reviewer through `list_sources`/`read_source` and the recovery capsule's
constraints (see [input history](input-history-and-replay.md#answered-questions)).
Lost workers are retried at most twice after confirmed absence, and uncertain
action receipts prevent replay. A worker whose route itself failed (an
engine-reported exhausted plan, rate limit or unavailable model, or an engine
that would not start or confirm the worker's mode) is instead retried on the
next available route, possibly another provider. Those retries don't count
against the two ordinary retries and are bounded separately. A routing refusal
stops the work under its own name and is never retried. See
[routing and fallback](routing-and-fallback.md#automatic-worker-routing-unavailable-routes). Coordinator, worker and review turns have total
and idle deadlines. Scope reservations remain occupied until engine termination
is observed. A task waiting for another task's capacity remains pending with a
bounded wait. Senior escalation inserts a read-only consultant routed automatically
by the runtime. Three accepted automatic replans since the latest user message or
answer and three review cycles bound the respective loops; exhausted limits require
user input or a new plan, and any user message or answer restores the replan budget.
