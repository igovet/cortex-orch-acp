# Runtime completion and lifecycle

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed. The former Python runtime was removed on 2026-11-16: references to its modules and tests were dropped, and statements that cited them describe behavior carried from that implementation that has not been re-verified against the Rust runtime.


Workspace's owner-only pause control additionally records a durable
`PAUSED` hold outside the model transition route. It saves the previous lifecycle
and pending question/worker-wait identities transactionally with public invalidation. Only
explicit owner Resume restores that state, checking the displayed version and owner
epoch; normal model transitions cannot leave the hold. Turn cancellation alone does
not create or remove it. Pending worker questions and runtime access decisions survive
service restart. The runtime fences the old worker and records a replacement continuation
with the saved decision; it does not revive the engine or transfer access grants.
Resume continues from the saved state: open and ready worker waits and pending
coordinator or worker questions stay answerable, and work that Pause interrupted
resumes through lost-worker retry or coordinator recovery. It never interrupts,
cancels or replans, and never invents a user message or answer. Only explicit Retry
and an owner message to a waiting, held or reopened task replan at once; a message to
a working task interrupts nothing and reaches the coordinator at its next boundary
(see [steering](../steering-and-questions/index.md)). Stopping an individual worker still
cancels its wait. See [Workspace Stop and Resume](../../cortex-workspace/local-task-execution.md#task-stop-and-resume).

The task state machine (`crates/cortex-core/src/lifecycle`) is:

`CREATED → PLANNING → PLAN_READY → EXECUTING → REVIEWING → VERIFYING → RECONCILING → COMPLETED`.

Planned work that changed no project file skips REVIEWING:
`EXECUTING → VERIFYING`. The runtime allows that transition only with its own
`REVIEW_NOT_REQUIRED` event for the current plan revision. It records the event
after every node has completed, when the project observed then equals the
`EXECUTION_BASELINE` captured before the task's first planned work (since the
task was created or last reopened) or equals a project state an independent
reviewer approved. A revision that already has a reviewer, or a project that
cannot be observed, is always reviewed.

An informational answer before any plan or worker uses
`PLANNING → ANSWER_READY → COMPLETED`. Only the runtime completes it after checking
the exact answer receipt, revision zero, no plan, nodes or worker provisioning,
and no blocker/cancellation. It cannot certify implementation or tests, replace
an existing plan, or skip required review. It never schedules a reviewer.

An answer released in the user's own words that closes planned work is certified
before it completes the task. The same holds for a final summary the coordinator
submits after a user message returned a verified revision to planning
(`RECONCILING → RECOVERING → PLANNING`); `synthesis_submit` is accepted only
while `RECONCILING`. If review or verification did not pass for that work,
the task ends `FAILED` with blocker `answer_not_certified:<gap>` and an
`ANSWER_NOT_CERTIFIED` event. The answer is still shown to the user.
Before recording a planned-work answer, the runtime applies the same unresolved
check assessment and disclosure guard as synthesis. An omitted gap or blank
blocker refuses the proposal without changing state. Accepted disclosures are
stored and shown in the answer's "Not verified" section; exact retries preserve
that content.

The additive Rust host now serves coordinator reconciliation, synthesis and
certified released answers, verification-command repair, and budgeted
replan/scope/escalation proposals. Its exact supported checks and remaining
evidence boundaries are in [native incoming ACP](../../project/native-incoming-acp.md#context-rollover-and-reconciliation);
the Python-specific check-assessment and runtime-composition details below are
not a claim that every native domain is complete.

## Omitted worker checks

Plan submission and verification repair probe check-program availability in the real read-only verification sandbox before recording commands. Workers and coordinators can declare a check early through the availability preflight; this lookup never executes the proposed check and cannot count as passing evidence. It uses verification's final launch preparation and working directory; an incomplete sandbox/launch probe reports `check_preflight_unavailable`, not a missing executable. A wrapper without an inner program is refused as an incomplete command. Missing programs require isolated project-private tools or another executable approach, never sudo or system changes. If neither is possible, the responsible model must ask the owner immediately through its existing question route, not defer that decision to publication. New executable gap declarations with absent programs are refused while the worker channel remains active. Human-only judgements have no command and appear separately as **Check manually**, never as a pass or failure or an owner-acceptance blocker. Policy-prohibited checks stay explicitly not run. A skipped paid qualification outside the runtime verification plan, reported as unrequested with no attempt, can remain an informational note after current work is verified. When automatic acceptance is blocked, **Needs you** links to review actions and explicit closure in the task.

Before reconciliation synthesis, executable worker checks reported as `not_run`
receive one runtime verification attempt per exact check and observed source
revision. Matching final-verification evidence avoids duplicate execution. The
runtime retains technical launch/policy blockers without automatic retry or a
fabricated pass. Fresh successful executions close gaps only with current source,
publication, review and complete-output bindings. Workspace exposes the same
runner for identified owner reruns; see [local task execution](../../cortex-workspace/local-task-execution.md#archive-and-service-owned-work).
Workspace result facts list current skipped worker and runtime checks individually
as `not_run`, with the recorded blocker when available. These rows are not
rerun targets. Displaying them does not change review or acceptance requirements.

## Runtime-composed synthesis

On a fully closed first-review path, the runtime composes the
final text in RECONCILING without another coordinator model turn. All current
plan nodes must be completed, independent review must be APPROVE on the current
digest, and the latest final verification must pass on that digest with exact,
unique runtime execution bindings and complete retained output. Deferred checks
must be closed by the existing evidence rules; open not_run, unverified or failed
checks keep the coordinator path. A no-change review waiver is not eligible.

Pending owner input (including claimed but undelivered input), steering, questions,
consent, access waits, meaningful runtime proposals or ask_user_required prevents
composition. Repair paths and any replan in task history also prevent it. The
stored `review_cycle` counts the initial review as 1: a second review or
REVIEW_CHANGES_REQUESTED is a repair, not the initial review itself. Verification
failure/revision, reopened work and returns to the coordinator are conservative
fallbacks too.

The deterministic user-facing text says the work is done, quotes the submitted
execution summaries, then lists changed file paths when there are any. Summaries
are attributed as the work summary, not promoted to runtime-verified semantic
facts. Their whitespace is flattened and Markdown punctuation escaped; review
prose, report bodies and internal check records are not included. The runtime
keeps detailed evidence in the task record and reports. Labels follow the selected
English or Russian chat language; quoted summaries retain their submitted language.
Missing/blank summaries, summaries over 1,600 characters in total, or non-whitespace
control/format characters keep coordinator synthesis rather than discarding result
content or caveats. Filenames containing backticks, control/format characters or
Unicode line/paragraph separators also keep coordinator synthesis, so they cannot
escape the inline-code spans. Missing baseline metadata or final text over 48 KiB
falls back rather than truncating.

In the native runtime the changed-file list comes from a per-file content manifest
that the runtime retains with the first `EXECUTION_BASELINE` since the task was
created or last reopened (`attempt_revisions`, attempt `baseline:<task>`, phase
`execution_baseline`; the manifest belongs to that baseline's digest). The files whose
content differs from it at composition time are listed. A project too large for the
manifest (more than 20,000 files), a missing manifest or a baseline whose digest
differs keeps the coordinator. Git-ignored files are not deliverable: a changed or unobserved ignored-files digest neither keeps the coordinator nor adds them to the list, which covers every file Git would track or show. Only execution-stage results are quoted; review
summaries never are. Open owner questions, a repeated review (`review_cycle` other
than 1) and any `REVIEW_NOT_REQUIRED`, `VERIFICATION_REVISED`, `VERIFICATION_FAILED`,
`REVIEW_CHANGES_REQUESTED`, `REPLAN_REQUEST_ACCEPTED`, `RETURNED_TO_COORDINATOR` or
`TASK_REOPENED` event keep the coordinator too.

A reviewer's packet lists its changed files from the same two manifests plus the
writes the task's sessions recorded (including writes under Git-ignored paths), with
`complete: true` whenever both manifests are trustworthy; changed Git-ignored files only
add an `ignored_files` note ("Build outputs and other git-ignored files changed; they are
not deliverable and are not listed."). Without a usable baseline manifest (for example a
baseline an earlier build observed while its ignored listing failed, which retained no
manifest) the packet is `complete: false` and lists the recorded writes and the paths the task's own commands and writing turns changed. That host
limit does not make approval impossible: the reviewer must still read every required listed
changed file completely in its current bytes (and paths outside its read scope still
count as missing), and an approval then records, in the review result and a
`REVIEW_CHANGE_SET_LIMITED` event, an `unverified` runtime check "Changes outside the
recorded writes" ("Changes made by commands outside the recorded writes could not be
listed; inspect the folder."; omitted when every command and writing turn of the task
kept a complete change record), or "Changed files beyond the review list" when more than
the 200 listed files changed (with complete attribution, only required files beyond the
list count). The final result must disclose it under Not verified; the
unlisted part is never recorded as reviewed.

Synthesis, `SYNTHESIS_RUNTIME_COMPOSED` and COMPLETED commit in one transaction,
under the unchanged report-integrity/project-digest completion gates and confirmed
absence of worker sessions. Before attempting composition, the controller waits
up to ten seconds for live workers whose post-result settlement/teardown is
already owned by the adapter. The wait is outside any SQLite transaction, never
terminates an engine, and is interrupted by cancellation or transport loss.
Unknown liveness, enduring live work or deadline expiry retains the normal
fallback; confirmed absence only permits retrying all existing composition,
digest, input, review and verification gates. Engines without an owned-settlement
query do not wait. The live adapter's registry termination bookkeeping
uses the completion transaction's SQLite connection, never a second writer.
Liveness and cancellation are checked again after composition and integrity
checks; a now-live or unknown worker keeps coordinator reconciliation. An engine
without this transaction-safe liveness gate also retains the coordinator path.

A worker that publishes a report but ends its turn without `result_submit` gets
a runtime-recorded result for that report, keyed by the assignment rather than
the attempt. The finished-worker shutdown resolves that key to the attempt the
assignment provisioned and stops and releases its engine as for a submitted
result; a key with no provisioned attempt is never stopped under the key itself.
When completion after coordinator synthesis still finds a worker that is not
confirmed absent, the task stops with `workers_still_live_or_unknown`, a
continuation that waits for the host to settle each such worker (node id, role,
engine and observed liveness are in the runtime explanation, at most five, then
a count). It never includes message or report content.

An interrupted pre-commit attempt rolls back. On the next runtime start, a task
left in RECONCILING after VERIFICATION_PASSED retries these same gates: eligible
work completes through runtime synthesis, otherwise it is handed to the
coordinator. Committed synthesis is reused without another completion or synthesis
event. Display uses
the existing ACP assistant-message path and retained pipeline, not a new host API:
on completion the stored synthesis -- runtime-composed or the coordinator's
`synthesis_submit` text with its "Not verified" list -- is sent as an assistant
message, and the chat ends on it: a completed task gets no closing label
("Завершено"/`Runtime: COMPLETED` were removed on the owner's request, 2026-10-22).
Only a failed, blocked or cancelled task ends with its label and blocker.
Once coordinator synthesis commits, the controller withholds later engine prose
for that turn (streamed deltas and final-only text), including paraphrases, rather
than displaying it alongside the stored synthesis. The commit, not a tool title or
callback ordering, establishes this boundary: synthesis leaves the task in
`RECONCILING` until completion checks finish. A rejected submission does not mute
text; an exact retry does not release it. Already accepted prose/redactor tails
still flush, so this is not semantic deduplication of earlier messages. A new
engine turn resets the display guard; completion and disclosure gates are unchanged.
The `answer_submit`/`synthesis_submit` `text` descriptions and the coordinator's per-turn
language directive ask for plain, simple Markdown for an ordinary user: start with the
answer in one or two short sentences using everyday words and no internal jargon unless
the user used it, keep it brief, use a few short bullets only when helpful, and backticks
for paths or commands -- never one unbroken block; this is model guidance, not a
runtime-enforced format.
Automated tests cover emission and recovery, not rendered Desktop qualification.

For declines covered by its fixed vocabulary, the runtime
records a durable, metadata-only `SYNTHESIS_RUNTIME_DECLINED` event in the same
transaction, at most once per plan revision (idempotent across recovery and
replay; a repeat attempt that finds one already recorded writes nothing more).
Its payload carries deterministic codes for the covered failing conditions --
`worker_live`, `replan_history`, `repair_path`, `pending_input`,
`node_incomplete`, `review_not_approved`, `verification_not_passed` or
`open_check` (with the node id, check name and current outcome, never check
content) -- as the first covered `reason` plus a `reasons` list. This is not
an exhaustive explanation of every fallback. The early liveness short-circuit
records only `worker_live`, without evaluating other reasons. Missing baseline
comparison metadata, unsafe filenames, unusable summaries and oversized text
have no reason code and can return without any decline event; post-composition integrity/liveness failure or
interruption rolls back without one too. A previously recorded event is not
updated by later attempts, even if conditions change or composition later succeeds.
Eligibility itself is unchanged. The event is never written when the
coordinator has already supplied its own synthesis text, since the gate was then
never attempted. The former aggregate timeline reader (removed) reported, per
engine/route/effort cohort, the share of reconciled tasks the runtime actually
composed and a tally of fallback reasons; a completed task with neither a
`SYNTHESIS_RUNTIME_COMPOSED` nor a `SYNTHESIS_RUNTIME_DECLINED` event (for
example, an unrecorded current-build fallback above or a build predating this
event) is tallied as the
explicit `unrecorded` reason, never guessed.

Additional states are BLOCKED, FAILED, CANCELLED, WAITING_USER and RECOVERING.
A direct EXECUTING → COMPLETED transition is rejected. Model-facing APIs do not
accept state transitions or task acceptance decisions.

After a terminal runtime decision, `OrchestratingRuntime.start_prompt` tears down
task-owned engines without changing that decision. Cleanup is outside completion
transactions; reservations still require observed absence. A waiting turn is not
terminal, and an unexpected prompt exception stops owned work without inventing
COMPLETED/FAILED/CANCELLED. See
[terminal cleanup](../failures-and-cancellation/index.md#terminal-resource-cleanup).

An explicit owner force-finish is **not** verified completion. `/cortex-finish`,
the **Finish** option of a runtime stop question an earlier version stored (by number, label, or its
stored or rendered English or Russian text) stops task-owned work and
leaves the task terminal `CANCELLED`, with a `USER_FORCE_FINISHED` event recording
the captured owner source, prior state, current revision, unfinished nodes, and
the fact that review, verification, and synthesis were not certified by this
decision. The runtime never inserts a passing review or verification event for
force-finish. Unlike ordinary coordinator planning, it does not require another
worker or a model acceptance decision; unlike `COMPLETED`, it makes no quality
claim about the deliverable.
The desktop treats this decision as the owner's close as is: the machine
presents a force-finished task with no saved disposition as closed as is and not
verified, so it leaves the Inbox, archives without a question (by Archive or
automatically) and settles its sprint dependency like any closed task. A saved
disposition always wins; nothing is certified.

An Automatic sprint can return a completed result to the coordinator when the checkout has changed or an ordinary required check still needs work. The coordinator must publish a fresh result. A pending permission or a dispatched action with an unknown outcome requires separate resolution. An owner can give a paused Automatic sprint permission to finish an eligible result on its current branch. The service binds that decision to the exact version and digest and retains the outcomes of acknowledged checks.

A queue hold of a task whose dispatched actions still have an unknown outcome is decided by that fact for every hold code. While only command receipts are unsettled, the runtime acts next ("Waiting for an unconfirmed command of this task to settle"): it re-settles such receipts by itself once nothing can still run them, and the hold is decided again; a command stays the runtime's to settle however long it takes (no time cap); only an unknown effect of a result the queue would finish (a write, an edit, a call that no continuation re-verified) goes to the owner. The owner's hold says in plain words that the outcome cannot be confirmed and offers only actions the service accepts in that state, always including Close as is; no agent repair, renewed attempts or automatic finish is offered. Confirming Close as is on that hold records each unknown receipt as settled with an unknown outcome (`settled_by: owner_close_as_is`) and an `OWNER_ACKNOWLEDGED_UNKNOWN_EFFECTS` event, never a success; a worker still being stopped refuses it. Closing a task with a run first checks the saved task's close without effect (`product.task` `close` with `preflight: true`) before the run records its resolution. See [sprint run modes](../../cortex-workspace/local-task-execution.md#sprint-run-modes).

### No-stop policy and the strategy ladder

Every task, manual or Automatic, finishes or keeps working: the runtime never parks a task for the owner because of its own failures, except for the one loop question it asks when the same failure used every rung of the strategy ladder and cannot be disclosed (see the loop guard below). There is no continuation cap, review-cycle stop, replan limit, coordinator no-progress stop, step-window stop, repair cap or outcome-unknown hold that waits for an owner decision. A stop of the runtime's own making (a failed or lost worker, a failed or unobserved check after its direct repairs, a blocking review, a review that made no progress, a review-cycle or step window used up, a degenerate model turn, an interrupted service, an effect with an unknown outcome) is recorded as `AUTOMATIC_CONTINUATION` with its reason, evidence and the next rung of the **strategy ladder**, enters RECOVERING (`automatic_continuation_pending`) and resumes with a fresh plan (`AUTOMATIC_RESUMED`) once the host has settled the processes. The relaunch is the runtime's own: a driver that ends starts a follower that continues the task with backoff (0.2 s doubling to 30 s), without the Workspace scheduler tick (an incoming ACP session has none); the scheduler's safety tick continues the same tasks too.

The rung is chosen per stop **fingerprint**: the stop reason, its classification detail, the items that block it (review dead ends by item name), the failed checks (argv and exit code) and the failed workers (node and blocker kind), normalized so that counters, ids, timestamps and absolute paths do not make a repeat look new; the checkout digest and the plan revision are left out. A stop takes the first rung its fingerprint has not used yet (`STRATEGIES` in `crates/cortex-core/src/lifecycle/automatic.rs`), and `escalate_route` is skipped when every worker of the stopped revision already ran escalated or had a single candidate route:

1. `repair`: a fresh plan that repairs the recorded failure on the current routes;
2. `escalate_route`: the workers of the next plan revision run on escalated routes (a stronger tier among the models the owner lists for each node's kind of work, preferably the other engine than the failed attempt). Escalation is never task-wide: later revisions route normally, and outside that revision a worker escalates only when its own node failed its work (not for another role's work, an owner decision, missing access, or a failure the infrastructure caused, such as a command sandbox that could not be prepared), after a quality retry of its node in that revision, or for the fix of a second review cycle; a retry after an infrastructure failure (a lost session, a restart) keeps its normal tier (owner 2026-11-21: one escalation had put every later worker of a long task on the senior models);
3. `consult`: the senior consultant is requested (`AUTOMATIC_CONSULTATION_REQUESTED`) and its advice joins the next plan through the ordinary consultation node;
4. `reassign`: the failed work goes to another role or a different decomposition, and the coordinator moves to another qualified model when one exists (`NATIVE_COORDINATOR_ESCALATED`).

Before the ladder, each failed node and each review dead end or failed reviewer is classified from recorded evidence (`FAILURE_CLASSIFIED`; rules in `classify`, `crates/cortex-core/src/lifecycle/obstacles.rs`):
**access** (an explicit `missing_access` blocker, or a recognized access-related command reporting a missing login, credential or permission, such as HTTP 401/403 or a missing sudo password; ordinary test output is not access evidence; a `missing_access` claim from a worker that ran no failed command, met no refused request and quotes no login or permission error is technical instead, `blocker_missing_access_unattempted`, and the next attempt is told to try the action first) records one open `OWNER_ACTION_REQUIRED` `{id, kind: access, service, detail, node, revision}` per task and service and parks the node while unrelated nodes keep running; `OWNER_ACTION_RESOLVED {id}`, or a later environment preflight that shows the login present (recorded as `OWNER_ACTION_RESOLVED` with `by: environment_recheck`), retries the node at once. The owner can dismiss an action even when the service is a registry hostname with no built-in login or probe.
**owner_decision** (an `owner_decision` dead end, or conflicting requirements) records `OWNER_DECISION_REQUIRED` and, once nothing else can run, an `AUTOMATIC_CONTINUATION` with strategy `ask_owner` that uses no rung; a plan is refused with `owner_question_required` until the coordinator asks.
**external_outage** (DNS failure, refused or timed-out connection, 5xx or 429 from a named remote host; local hosts stay technical; only the service's built-in endpoints `api.github.com`, `github.com`, `api.openai.com`, `chatgpt.com` and `api.anthropic.com` over HTTPS port 443 qualify, so a destination inferred from failure text or arguments that is not on that list stays technical with an unsupported-recheck diagnostic and is never probed) records `EXTERNAL_WAIT_STARTED` and is rechecked by the runtime without model calls (TCP connect or HEAD) after 1, 2, 5, 10 and then every 30 minutes (`EXTERNAL_WAIT_RECHECKED`, `EXTERNAL_WAIT_ENDED`). Every recheck revalidates the exact host, port and scheme before DNS, refuses the whole answer if any address is private, loopback, link-local, unspecified or otherwise special-use, and connects to the checked address. The HEAD check is a fixed `-q` curl with an empty environment, no proxy, no redirects and no netrc, so the owner's configuration and credentials are never used; a DNS, connect, status, 429 or 5xx failure never marks the wait ready. When only parked access or outage work remains, the task pauses with `pause_kind: obstacle` and resumes the same plan when every obstacle has closed.
Everything else is **technical** and takes the ladder. Real progress is marked with `TASK_PROGRESS` `{kind: plan_accepted | node_completed | check_passed | review_passed | verification_passed, node}`; this is the signal a "no progress" notice is built on. Review results of `REQUEST_CHANGES`, `INCOMPLETE` or `BLOCK` do not add progress markers, even if the review includes passing checks.

A degenerate model turn always takes at least `escalate_route` and moves the coordinator to another model; a review dead end that repeats on the same item takes `reassign` while that rung is unused.

The **loop guard** (`crates/cortex-core/src/lifecycle/loop_guard.rs`) records every sighting as `LOOP_OBSERVED {key, kind, node, count, rungs_used}`. Its count window starts at the owner's newest input that can change the situation (`TASK_REOPENED`, `USER_STEERING`, an owner Resume of an earlier version's cap, or an answer given while that key was the current loop) and never at `AUTOMATIC_RESUMED`, `PLAN_ACCEPTED` or a new plan revision, which every cycle passes through. A resumed continuation does not reset the host's relaunch spacing (counted per fingerprint) or the replan count after which a replan becomes a senior consultation (counted since the owner's last input) either. Once every rung was used for one fingerprint the loop is broken (`LOOP_BROKEN {key, kind, action, limits}`) instead of cycling again:

- a review whose remaining dead ends no strategy reached (`disclose`) records them as disclosed `unverified` limits on the review result (`REVIEW_LIMIT_DISCLOSED`) and the task continues to verification; a review whose dead ends are only items the owner already decided about and items its tools cannot reach is disclosed at once, without asking again;
- anything that cannot be disclosed, such as a check that keeps failing the same way or the same findings again (`ask_owner`), is put to the owner as one question the runtime asks itself, without a model call: in the task's language, naming the task, the repeated failure and how many full rounds of the ladder were tried. While it is open the task waits in WAITING_USER without model calls. The runtime applies the answer itself; its options have fixed ids: `retry_now` (a new round of the ladder at once), `other_approach` (the owner's own words, or a free-text answer, reach the next plan as the owner's direction, and a new round starts), `accept_unverified` (the failure becomes a disclosed `unverified` limit, `LOOP_BROKEN` action `accept_unverified`, that the final result lists under Not verified and is never a pass; an Automatic repair plan may then leave the accepted failed command out) and `retry_later` (the runtime continues by itself after 30 minutes, without model calls in between). The same failure asks again only after the chosen answer was tried and every rung was used again; that question names its attempt number, so the owner never gets the identical question twice. The question is not a coordinator question, so the coordinator's repeated-question refusal does not apply to it.


Infrastructure, host and owner events are never the model's failure: they take no rung, escalate no node and cause no replan. A stop for an effect with an unknown or unconfirmed outcome, a service restart or interrupted turn, a recovery the service could not finish, the owner's pause during the attempt or the owner's stop of a worker, an engine session that could not start, a retry limit reached only by infrastructure or route retries, a worker failure the command sandbox caused, or a verification whose every failing command failed for its environment (no complete output, a timeout, exit 126 or 127) continues on the rung the task was on (`class: infrastructure`). The same interruption repeating three times without real progress is waited out by the runtime (`AUTOMATIC_PAUSED {pause_kind: infrastructure}`, `LOOP_BROKEN` action `infrastructure_wait`) with growing waits of 5, 15, 30 and then every 60 minutes, then resumes by itself on that rung. A review that failed only because another actor kept changing the checkout waits on the runtime's timer and is then run again directly in REVIEWING, without a coordinator turn or a rung. The continuation prompt carries the rung's instruction and the evidence. Every coordinator session also carries the runtime's no-stop note: it asks the owner only about the work itself and keeps working on everything the answer does not block. A pause an earlier version recorded for the owner (`continuation_cap`, `owner_blocker`, `outcome_unknown`) is resumed by the runtime itself on an escalated rung.

What remains: a route, connection, quota or authentication problem waits as `external_blocker` and the runtime resumes it by itself as soon as that route, or another qualified coordinator route, is usable; a session or command whose end the host has not yet confirmed is waited for until the host settles it with process evidence (the runtime's own wait, never an owner hold). Genuine safety refusals are unchanged: Cortex private state, credential and deletion consent in Guarded and Development, the owner's own Pause and Stop, and questions the coordinator asks about the work. One completion refusal is such a work decision: outside Automatic, a final result that replaced a failed mandatory check (`completion_dropped_failed_check`) stays RECONCILING on its verified revision and returns to its coordinator (`ASK_USER_REQUIRED`), which asks the owner whether to accept the replacement.

An effect whose outcome is unknown is never replayed blindly and never holds the task. When the continuation resumes and every owned session is confirmed gone, each write, edit, call or other synchronous effect without a complete receipt is settled with outcome `unknown` (`settled_by: runtime_reverify`) and listed in `RUNTIME_UNCERTAINTY_RECORDED` and in the coordinator's prompt, which re-verifies it from the files, Git state and receipts before redoing it. A command receipt that may still run is waited for until the host settles it (`native_command_outcome_pending`).

A review whose only gaps are items its tools cannot reach (`tool_unavailable`), with nothing to repair, records them as disclosed unverified limits (`REVIEW_LIMIT_DISCLOSED`, `unverified` runtime checks on the review result that the final result must disclose) and the task continues to verification; the review is not waived. A failed final check returns to planning for three direct repairs per rung of the ladder, then continues on the next rung; a failed required check is never dropped from a repair plan. A verification outcome caused by the verifier's environment is not a failed check: the runtime reruns it and otherwise waits (see **Verifier environment** below). A limit the runtime disclosed when it broke a loop (`LOOP_BROKEN.limits`, since the last reopen) is a gap of the final result, which must list it under Not verified even after a replan. From the fourth replan since the owner's last input, a replan or scope request is accepted as a senior consultation (`decision: senior_consultation`, `strategy: consult`) instead of being refused. Continuations never waive review or verification and grant no permission.

A task that an Automatic sprint started is bound at start to `AUTOMATIC_POLICY_BOUND`, a trusted host option that commands, plans and model tools cannot set. It adds the Automatic finish rules (attributable file-tool writes, failed checks kept in repair plans); the continuation itself follows the no-stop policy above for every task. See [sprint run modes](../../cortex-workspace/local-task-execution.md#sprint-run-modes).

A sprint's planning task is bound at Start to the runtime's sprint planning policy (`SPRINT_PLANNING_BOUND`), a trusted host option that commands, plans and model tools cannot set. The runtime refuses a plan with a writing role (`read_only_task_writer_refused`) and every project write of the task; its study plan may name no verification commands. Its coordinator submits the sprint plan with `sprint_plan_submit` at a coordinator turn (PLANNING or RECONCILING, no worker running). A large plan arrives in ordered parts (`SPRINT_PLAN_PART`; a first part discards an unfinished draft); the final part validates the whole assembled plan (`SPRINT_PLAN_SUBMITTED`). The runtime refuses an unchanged plan, and schedules a mandatory independent plan review (`SPRINT_PLAN_REVIEW_SCHEDULED`) within the usual review-cycle budget, moving the task to REVIEWING. Findings record `SPRINT_PLAN_CHANGES_REQUESTED` and return the task to its coordinator under the bounded coordinator returns; repeated findings, the round limit or exhausted returns continue it on the strategy ladder. The continuation after a round-limit stop (`AUTOMATIC_CONTINUATION` with reason `review_cycle_limit`, once resumed), or an owner answer to an earlier version's round-limit stop, gives the next submitted plan a fresh review budget. After approval the runtime verification passes only for the newest, approved plan (`VERIFICATION_PASSED` with `runtime: sprint_plan`); otherwise it records `SPRINT_PLAN_PENDING` and completion stays refused. The task then completes through the ordinary gate. See [sprint planning limits](../../cortex-workspace/local-task-execution.md#sprint-planning-limits).

Only an immutable, attempt-owned report and its structured result close a node.
A reviewer, required whenever the project changed, submits APPROVE, REQUEST_CHANGES, BLOCK or INCOMPLETE. Every worker, a reviewer included, finishes its own assignment: `result_submit` refuses, correctably and before anything is published, a result that leaves something the worker can still do (an uncovered part without a dead-end blocker, an unread changed file the review packet marks required, that is a change of this task ([review of the task's own changes](../orchestration/index.md#review-of-the-tasks-own-changes)), or a required predecessor report of the review's round, a BLOCK without findings, a failure without its blocker), names each outstanding item in `missing`, and the same session continues on the same channel. A turn that ends without a result is asked once more in the same session to finish. A reviewer submits INCOMPLETE only for parts blocked by a named dead end (`missing_access`, `owner_decision`, `other_role_work` or `tool_unavailable`); no second reviewer is scheduled for the same revision. Items only its tools cannot reach become disclosed unverified limits and the task continues to verification; any other dead end is a continuation on the strategy ladder ([no-stop policy](#no-stop-policy-and-the-strategy-ladder), [review outcomes](../orchestration/index.md)). A fix and review round continues only on progress: unchanged task files or the same findings again end the round once (`review_no_progress`) as a continuation that changes approach. The runtime asks no stop question of its own; an earlier version's stored stop question still answers and relaunches the coordinator. Changes produce
fix nodes and another review, with a three-cycle ceiling. A fix node never keeps a read-only role: the runtime gives it a writing role chosen from its scope. Structured review findings confine fixes to the
named plan tasks when the runtime can verify them. With no findings and multiple
implementers, the runtime first returns to the coordinator to plan the remedy
while its return budget remains ([coordinator return budget](../orchestration/index.md#coordinator-return-budget)); otherwise unconfinable or absent findings give
every original plan task a fix
(see [targeted review fixes](../orchestration/index.md#targeted-review-fixes)). Verification is a
runtime command observation, independent of model review. Nonzero exit, incomplete
output or project drift prevents a passing verification event. A failed verification
records, per failed command, a bounded excerpt of what failed (`diagnostics`: failing
steps, panics with their next lines, assertion values, compiler errors and test
failures; at most 40 lines and 4,000 characters each), and the coordinator's and
workers' capsule carries it as `verification_failure` until a later verification
passes. The full verifier output stays the runtime's own record, which no worker
channel can read; before this, each failed verification cost a plan revision that only
rediscovered the failing test (owner store 2026-11-22). After a command exits, its
output streams may take up to 30 seconds to reach their end before the output counts as
unavailable (one second had turned a passing command under host load into an unknown
outcome, 2026-11-21). A dropped failed
verification command remains an obligation across later repairs; an unrelated
worker answer cannot acknowledge it. If a planned-work answer reaches
`ANSWER_READY` without satisfying this gate, `close_answer` ends it `FAILED`
with `answer_not_certified:completion_dropped_failed_check`, not a crash loop or
verified completion.

**Verifier environment.** A verification command whose outcome comes from the
verifier's environment rather than the reviewed work
(`verifier_environment`, `crates/cortex-core/src/lifecycle/completion.rs`) is
not a failed check. That covers a run that never completed (its output did not
reach its end, its launch or sandbox failed, or something outside killed it before
the verifier's own 20-minute deadline), exit 126 or 127 (the program could not be
executed or was not found), a full disk or exceeded quota, and a failure of a
command that a worker of the task ran identically and passed on the same checkout
(for example a loopback service the verifier sandbox cannot reach,
`network_refused`, or any other `environment_difference`). The runtime reruns the
identical command once itself, with no model call (`VERIFICATION_RERUN {revision,
argv, reason}`), and the rerun's outcome counts. If the rerun fails the same
environmental way, the task stays VERIFYING as the runtime's own wait
(`VERIFICATION_ENVIRONMENT_WAIT {revision, argv, reason, receipts, attempt,
retry_after_s}`): no replan, no ladder rung, no model call, nothing recorded as
passed. The runtime runs the verification again after 1, 2, 5 and 10 minutes
(`LOOP_OBSERVED` with kind `verifier_environment` from the second wait); both
receipts of each attempt stay in the retained output as diagnostics. If the
10-minute recheck still fails the same environmental way, the runtime does not
wait longer: it asks the owner one question itself, with no model call, in the
task's language, naming the command and the reason in plain words, with the
answers of the loop question (`retry_now`, `other_approach`, `accept_unverified`,
`retry_later`; the task waits in WAITING_USER). Retry now or later lets the
coordinator resubmit the same verification, whose rechecks start again; another
approach lets it change the verification procedure with `verification_submit`
(for example a check that needs a loopback service the verifier sandbox cannot
reach); accepting records the command as an `unverified` limit (`LOOP_BROKEN`
action `accept_unverified`) that the final result lists under Not verified and
that no longer holds completion, never a pass. When the owner accepted every
verification command of the plan, the coordinator submits an empty procedure: the
plan keeps no command, nothing runs, no `VERIFICATION_PASSED` is recorded
(`VERIFICATION_REVISED` names them in `accepted_unverified`), and the task
completes on the approving review's unchanged binding with every accepted command
under Not verified, an explicit owner decision like Close as is. With any other
command left an empty procedure is refused (`verification_required`). The same
question is asked again only after the chosen answer was tried and its rechecks failed the same way; it
names its attempt number. Exceeding the verifier's own deadline
(`command_timeout`), output past the retained limit, a signal, an owner stop and a
nonzero exit with complete output stay real failures. A plan's launcher prefix
(`timeout 900 cargo ...`, `env -C dir node ...`) is checked at plan acceptance on
the same search path for both the launcher and the program it runs, so a program
the verifier would not find is reported then (`verification_executable_unavailable`).

A verification run that started and never finished (a restart mid-run) is
settled with outcome `unknown` (`settled_by: runtime_reverify`, recorded as
`VERIFICATION_RERUN` with reason `outcome_unknown`) and verification runs again;
it no longer refuses every later verification of the revision. A run during which
the reviewed bytes changed is settled the same way and binds a fresh review,
whether or not its exit was observed. Every `VERIFICATION_FAILED` carries a
`failure_key` (each failing command's argv, exit code and a digest of its
normalized output tail), equal for the same failure on any run, which the strategy
ladder uses to recognize a repeated failure. A coordinator's `verification_submit`
may resubmit the identical commands once with nothing changed (one rerun of a
possibly flaky check); again, with no project write, no worker or outside command,
no owner input and no new plan since, it is refused `verification_unchanged`.

**Pending input at completion.** Owner messages and clarifications that the former
runtime stored as `task_events` rows are listed in every coordinator's
retained-state capsule but were never marked consumed, so one could hold
completion with `completion_pending_input` indefinitely (owner store: a message from
2026-10-05 held a task through three passed verifications). Such a row that a later
coordinator turn carried (a plan accepted, a coordinator continuation, a synthesis
or a replan after it) is marked consumed (`runtime:recovery_capsule`) at
completion. When input still holds completion (an open question or wait, an owner
message not yet applied or delivered, or a former runtime's message no coordinator
has carried yet), the runtime records `COMPLETION_INPUT_PENDING {items}` naming
each item and returns to the coordinator, whose next turn delivers it, instead of
stopping into the strategy ladder.

Every not_run or unverified worker check carries its blocker; result_submit rejects
one without it. A passed claim the runtime could not verify is recorded unverified
with the runtime's blocker and `evidence_gap` (see
[check evidence](../orchestration/index.md#check-evidence)), so it is a gap here too. A
failed check fails its node unless an exact later rerun of the same command in
the same attempt closed it (exact argv/cwd may cross between `command_run` and
`outside_command`; same-operation profiles must still match); the failure stays in the result as history with
`closed_by` (see
[check history](../orchestration/index.md#check-history-and-current-outcome)). An eligible own-probe syntax, regex or statically proven literal-expectation error can instead be retracted; project test/lint/build and runtime-verification failures cannot. A literal-expectation correction changes only one expected literal in a bounded self-contained data probe, with no product calls or imports. The failure remains as history with `retracted_by`, the check is `retracted` rather than passed, and it does not fail the node. Synthesis is refused while a retained not_run or unverified check
of the current plan revision is neither closed under the
[exact-command or command-less compatibility rules](../orchestration/index.md#check-history-and-current-outcome)
nor listed in synthesis not_verified with its blocker. A worker command receipt
closes a declared-command gap only with identical argv, the task working tree as
its effective cwd, and a read-only, task-mode writer, or Guarded (`restricted`) profile; a
host-writable receipt cannot close it once the task is Guarded.
Runtime final verification
can close a declared-command gap only with identical argv, complete exit-0
execution and untruncated runtime-owned evidence bound to both the gap author's
runtime-observed publication or review digest and the current reviewed project
digest, regardless of execution order. Its durable `closed_by` is rederived during recovery and
synthesis; it never upgrades the worker's original disclosure to a claimed pass.
The runtime renders that list into the synthesis as "Not verified" (its heading and
"blocker" label follow the task's turn language, e.g. "Не проверено"/"причина" for
Russian; item, its optional description and blocker stay the coordinator's own text).
Each `not_verified` item carries its `item` (for a retained worker check, the exact
check name, kept unchanged by language because gap matching depends on this exact
spelling), an optional `description`, and a `blocker`; the advertised schema asks the
coordinator to write `blocker`, and `description` whenever `item` is such an exact
check name, in the task's selected chat language. When `description` is nonblank the
rendered line shows only that description, without appending the raw check name
(e.g. "Проверка в приложении — причина: Владелец не разрешил платную проверку.").
The unchanged `item` remains in structured evidence for gap matching, not in the
displayed line. Otherwise the line renders `item` directly, unchanged, which is how an older
coordinator call without `description` still renders exactly as before. Each blocker
must name the concrete condition that prevented the check; "not run", "out of scope"
or "not requested" alone is insufficient. A documented policy requiring an
explicit owner request can be a concrete blocker when the missing request is
named (for example, "paid live check not requested by the owner", or its
equivalent written in the task's selected chat language). An Automatic sprint takes
a not-run check outside the verification plan whose blocker starts with that phrase
(or the Russian "платная живая проверка не запрошена владельцем"), optionally
followed by more detail, as an informational note rather than an owner hold.

Workspace result facts expose current skipped checks individually in `result.checks`
with outcome `not_run` and the recorded reason in `detail`; technical or policy
rows also provide `technicalReason` where applicable. Accept and Finish show
these as **Not run**, not as passing evidence. Displaying an unrequested paid
qualification does not add it to the verification plan or change acceptance rules.
A limitation the runtime itself disclosed (`source: "runtime"`: unread files, an
earlier failed check, an owner-accepted gap) is never read as a policy restriction
from the words of its explanation; it is a manual note that an Automatic sprint
accepts with the result, never as passed (owner store 2026-11-22: "after repeated
coverage refusals" matched a policy word and held the sprint for the owner). An
INCOMPLETE review whose unreachable parts the runtime disclosed as limits
(`review_limit_disclosed`) counts as the approving review for acceptance, as it did
for the runtime's own verification; its limits stay notes. Without that disclosure an
INCOMPLETE review is not approved.

For coordinator-submitted synthesis and direct answers, structured `not_verified`
is the sole source of the runtime's disclosure section. Items with the same blocker
render as one line (`check one; check two — Blocker: …`); the structured list keeps
every item (one device limit was listed about ten times in one answer, 2026-10-22). When that list is nonempty,
the runtime refuses an authored disclosure heading
in `text` with `synthesis_disclosure_in_text`, before storing or delivering the
answer. The caller must move gap disclosures into the structured list and keep
unrelated context elsewhere in the text; the runtime does not silently delete a
section that might contain unique information. Corrected submissions can retry
without a failed proposal changing state or claiming an answer delivery key.

The heading guard recognizes canonical and common alternate English/Russian
labels, including "What was not verified", "Unverified checks" and "Что не
проверено", regardless of the selected language. ATX and setext headings,
emphasis, trailing colons/closing hashes and case variations are recognized;
fenced/indented code and quoted examples are left alone. This is a bounded
structural guard, not a claim to detect arbitrary semantic repetition in prose.
All structured items, exact gap names and blockers remain intact and are rendered
once. Runtime-composed synthesis is separate and only runs after all verification
gaps are closed. Existing saved history is not rewritten.

The final gate checks the current plan, completed nodes, the final approving review
(or the revision's `REVIEW_NOT_REQUIRED` record), a latest verification event
that passed (a later failure supersedes an earlier pass) with its exact
verification revision matching that review's or record's project digest, report content digests, synthesis and absence of live worker
sessions. A report removed or changed after publication, or a project changed during
synthesis, blocks completion. Chat text is never parsed as an acceptance signal.

The project observation behind those digests never follows a link and never fails
because of what the project contains. A symlink is recorded by its target string (a
symlinked directory is one entry; nothing behind it is read). A submodule or nested
repository is recorded with the commit its checkout is at (`uninitialised` when it is not
checked out). Every regular file of the deliverable content (tracked and untracked
non-ignored files, nested checkouts included) is hashed in full by streaming, together
with its mode, so restoring an mtime cannot hide an edit. Generous safety bounds (4 GiB per
file, 8 GiB in total, five minutes) apply; a file past a bound is recorded by size, mtime
and mode with a marker, sets `metadata_only`, `cap_reached` and `incomplete`, and denies
the waiver. A fifo, socket or device cannot be hashed and does the same. Only genuinely
unreadable content (permission denied, a file that changes while it is read, an unlisted
or private path) refuses, with an explicit `project_unobservable_*` reason, never a silent
pass. Git-ignored files are not part of the delivered result: they are covered by a second,
metadata-only digest (type, mode, size, mtime, link target; no content), including the ignored
files of nested checkouts. The waivers require that digest to equal the baseline's, so an
unchanged `target/` or `node_modules/` still gets the waiver, while a changed, added or
removed ignored file, or an ignored set that could not be observed, denies it and the review
runs. The ignored digest is computed over Git's sorted listing without a name limit (the
metadata pass is bounded by the same five minutes). An ignored entry that vanishes after the
listing is recorded by name as missing. If an ignored entry's metadata cannot be read, the
ignored digest is unobserved. An unobserved ignored digest only denies the waivers: it never makes the
deliverable observation `incomplete` or its per-file manifest unusable. An equal
ignored digest compares only those metadata fields; it does not prove that the
ignored file contents are equal. Each Git listing the
observation uses (paths, index, ignored directories, ignored files) keeps up to 256 MiB and
two minutes, and a listing past the byte bound is never cut short into a smaller project:
the ignored-files listing then leaves the ignored digest unobserved, and any other listing
refuses with `project_unobservable_too_many_paths` (a nested checkout's is recorded as
unobserved and sets `incomplete`). The observation also covers the dirty state of
every submodule and nested repository (tracked changes, untracked files; recursively to four
levels) and empty directories, which Git does not track. A nested checkout that cannot be
observed sets `incomplete`. The `EXECUTION_BASELINE` receipt records `metadata_only`,
`cap_reached` and `incomplete`; a metadata-only or incomplete observation never supports
either `REVIEW_NOT_REQUIRED` waiver. It also records the observed project checkout's
resolved Git commit as `head`, whether attached or detached. `head` is null for an
unborn branch or a plain folder; a nested checkout's commit does not replace it.

A plain project folder, in no Git repository at all, runs the same lifecycle (execution
baseline, worker change lists, review packet, verification, completion and acceptance) on
an observation made without Git. It is chosen when Git itself answers "not a git
repository" and the folder holds no `.git` entry, or when the fixed Git executable is
absent and neither the folder nor any ancestor has a `.git` entry. Git checkouts are
observed by Git, not by the plain walk; a repository Git cannot read (damaged, locked,
owned by another user or untrusted) stays refused with `project_revision_unavailable`. The plain observation walks the folder without following
links: every non-directory entry is deliverable content hashed exactly as above (there are
no ignore rules without a repository, so nothing is ignored and the ignored digest covers
an empty set), the private stores (`.cortex`, `.cortex-dev`, and at the top
`.cortex-dev-owner.json` and `.cortex-workspace-dev.json`) are skipped, a directory holding
a `.git` entry is a nested checkout observed as above, and empty directories count. Its
digest is domain-separated from any repository's. An unreadable directory refuses with
`project_unobservable_unreadable_content`, and more than 300,000 walked entries (or
100,000 files) refuses with `project_unobservable_too_many_paths`; the walk never returns a
partial listing. When either refusal, or `project_revision_unavailable`, stops a task, the
recorded continuation names that cause.

A task never has two drivers: installing a driver for a task cancels and awaits any
prior one, and every exit path of a worker turn in a concurrent batch signals the
siblings. A worker question normally leaves the task Executing (only the asking worker
waits); the task moves to WAITING_USER only for a worker without an engine session
reference, and then sibling workers are stopped because worker channels are authorised
only while EXECUTING or REVIEWING. An incoming prompt on a paused task is refused
without any commit while an owned process is unconfirmed, exactly as Resume is.

State, graph and events survive process loss. Retry requires observed engine
absence and no uncertain action receipt. Review binds the project digest from
dispatch through verdict publication, verification and completion; a
`REVIEW_NOT_REQUIRED` record binds its observed digest the same way. See
[verification status](../../project/verification.md) for automated recovery test coverage.
No unsupported reconnect or absent observation is treated as successful recovery.
