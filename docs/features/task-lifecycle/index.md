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
differs keeps the coordinator, and so does a changed or unobserved ignored-files digest, because the list covers only files Git would track or show. Only execution-stage results are quoted; review
summaries never are. Open owner questions, a repeated review (`review_cycle` other
than 1) and any `REVIEW_NOT_REQUIRED`, `VERIFICATION_REVISED`, `VERIFICATION_FAILED`,
`REVIEW_CHANGES_REQUESTED`, `REPLAN_REQUEST_ACCEPTED`, `RETURNED_TO_COORDINATOR` or
`TASK_REOPENED` event keep the coordinator too.

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
confirmed absent, the task stops with `workers_still_live_or_unknown`, and the
stop question names each such worker by node id, role, engine and observed
liveness (at most five, then a count). It never includes message or report content.

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
the **Finish** option of the runtime's own stop question (by number, label, or its
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

A task that an Automatic sprint started is bound at start to the runtime's automatic continuation policy (`AUTOMATIC_POLICY_BOUND`). The policy is a trusted host option that commands, plans and model tools cannot set. When a process stop or route problem would otherwise end recovery with a question, the runtime records `AUTOMATIC_CONTINUATION` with its reason and evidence instead, enters RECOVERING and resumes planning once the host has settled the process (`AUTOMATIC_RESUMED`). The fourth continuation of a task requests the senior consultant once (`AUTOMATIC_CONSULTATION_REQUESTED`); its answer joins the next plan through the ordinary consultation path. After eight continuations, the next stop pauses the task (`AUTOMATIC_PAUSED`, kind `continuation_cap`). The owner can open the paused task and use Resume; fresh planning renews the bounded budget only when process and effect evidence permits it. The lifetime count and one-time consultation are retained. A route problem pauses it as `external_blocker` and resumes when the route is qualified again. An unconfirmed effect pauses it as `outcome_unknown` and is never replayed or resumed by the runtime. A pause only for an unconfirmed process exit is the exception: once the running service settles that exit with process evidence, the task continues (`AUTOMATIC_CONTINUATION`, reason `native_process_exit_settled`) unless a dispatched effect is still unknown. Continuations and pauses never waive review or verification and never create an owner work decision. See [sprint run modes](../../cortex-workspace/local-task-execution.md#sprint-run-modes).

A sprint's planning task is bound at Start to the runtime's sprint planning policy (`SPRINT_PLANNING_BOUND`), a trusted host option that commands, plans and model tools cannot set. The runtime refuses a plan with a writing role (`read_only_task_writer_refused`) and every project write of the task; its study plan may name no verification commands. Its coordinator submits the sprint plan with `sprint_plan_submit` at a coordinator turn (PLANNING or RECONCILING, no worker running). A large plan arrives in ordered parts (`SPRINT_PLAN_PART`; a first part discards an unfinished draft); the final part validates the whole assembled plan (`SPRINT_PLAN_SUBMITTED`). The runtime refuses an unchanged plan, and schedules a mandatory independent plan review (`SPRINT_PLAN_REVIEW_SCHEDULED`) within the usual review-cycle budget, moving the task to REVIEWING. Findings record `SPRINT_PLAN_CHANGES_REQUESTED` and return the task to its coordinator under the bounded coordinator returns; repeated findings, the round limit or exhausted returns stop it the run-mode way. After approval the runtime verification passes only for the newest, approved plan (`VERIFICATION_PASSED` with `runtime: sprint_plan`); otherwise it records `SPRINT_PLAN_PENDING` and completion stays refused. The task then completes through the ordinary gate. See [sprint planning limits](../../cortex-workspace/local-task-execution.md#sprint-planning-limits).

Only an immutable, attempt-owned report and its structured result close a node.
A reviewer, required whenever the project changed, submits APPROVE, REQUEST_CHANGES, BLOCK or INCOMPLETE. Every worker, a reviewer included, finishes its own assignment: `result_submit` refuses, correctably and before anything is published, a result that leaves something the worker can still do (an uncovered part without a dead-end blocker, an unread changed file or predecessor report, a BLOCK without findings, a failure without its blocker), names each outstanding item in `missing`, and the same session continues on the same channel. A turn that ends without a result is asked once more in the same session to finish. A reviewer submits INCOMPLETE only for parts blocked by a named dead end (`missing_access`, `owner_decision`, `other_role_work` or `tool_unavailable`); no second reviewer is scheduled for the same revision, and the dead ends go the run mode's way ([review outcomes](../orchestration/index.md)). A fix and review round continues only on progress: unchanged files or the same findings again stop it once (`review_no_progress`). When the runtime stops a task with its own question, the driver ends and the question is the one stop shown; its answer relaunches the coordinator. Changes produce
fix nodes and another review, with a three-cycle ceiling. A fix node never keeps a read-only role: the runtime gives it a writing role chosen from its scope. Structured review findings confine fixes to the
named plan tasks when the runtime can verify them. With no findings and multiple
implementers, the runtime first returns to the coordinator to plan the remedy
while its return budget remains ([coordinator return budget](../orchestration/index.md#coordinator-return-budget)); otherwise unconfinable or absent findings give
every original plan task a fix
(see [targeted review fixes](../orchestration/index.md#targeted-review-fixes)). Verification is a
runtime command observation, independent of model review. Nonzero exit, incomplete
output or project drift prevents a passing verification event. A dropped failed
verification command remains an obligation across later repairs; an unrelated
worker answer cannot acknowledge it. If a planned-work answer reaches
`ANSWER_READY` without satisfying this gate, `close_answer` ends it `FAILED`
with `answer_not_certified:completion_dropped_failed_check`, not a crash loop or
verified completion.

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
equivalent written in the task's selected chat language).

Workspace result facts expose current skipped checks individually in `result.checks`
with outcome `not_run` and the recorded reason in `detail`; technical or policy
rows also provide `technicalReason` where applicable. Accept and Finish show
these as **Not run**, not as passing evidence. Displaying an unrequested paid
qualification does not add it to the verification plan or change acceptance rules.

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
removed ignored file, or an ignored set that could not be listed completely (200,000
entries), denies it and the review runs. The observation also covers the dirty state of
every submodule and nested repository (tracked changes, untracked files; recursively to four
levels) and empty directories, which Git does not track. A nested checkout that cannot be
observed sets `incomplete`. The `EXECUTION_BASELINE` receipt records `metadata_only`,
`cap_reached` and `incomplete`; a metadata-only or incomplete observation never supports
either `REVIEW_NOT_REQUIRED` waiver.

A plain project folder, in no Git repository at all, runs the same lifecycle (execution
baseline, worker change lists, review packet, verification, completion and acceptance) on
an observation made without Git. It is chosen only when Git itself answers "not a git
repository" for the folder and the folder holds no `.git` entry; a repository, or a folder
inside one, is always observed by Git exactly as above, and a repository Git cannot read
(damaged, locked, owned by another user or untrusted) stays refused with
`project_revision_unavailable`. The plain observation walks the folder without following
links: every non-directory entry is deliverable content hashed exactly as above (there are
no ignore rules without a repository, so nothing is ignored and the ignored digest covers
an empty set), the private stores (`.cortex`, `.cortex-dev`, and at the top
`.cortex-dev-owner.json` and `.cortex-workspace-dev.json`) are skipped, a directory holding
a `.git` entry is a nested checkout observed as above, and empty directories count. Its
digest is domain-separated from any repository's. An unreadable directory refuses with
`project_unobservable_unreadable_content`, and more than 300,000 walked entries (or
100,000 files) refuses with `project_unobservable_too_many_paths`; the walk never returns a
partial listing. When either refusal, or `project_revision_unavailable`, stops a task, the
runtime's stop question names that cause in the owner's language.

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
