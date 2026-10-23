# Runtime completion and lifecycle

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed.


The task state machine is defined in `orchestration/state.py`:

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

## Runtime-composed synthesis

On a fully closed first-review path, `orchestration/synthesis.py` composes the
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

For declines covered by its fixed vocabulary, `orchestration/synthesis.py`
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
never attempted. `scripts/cortex_timeline.py --aggregate` reports, per
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
stored or rendered English or Russian text), or an unambiguous answer to a
task-closure question stops task-owned work and
leaves the task terminal `CANCELLED`, with a `USER_FORCE_FINISHED` event recording
the captured owner source, prior state, current revision, unfinished nodes, and
the fact that review, verification, and synthesis were not certified by this
decision. The runtime never inserts a passing review or verification event for
force-finish. Unlike ordinary coordinator planning, it does not require another
worker or a model acceptance decision; unlike `COMPLETED`, it makes no quality
claim about the deliverable.

Only an immutable, attempt-owned report and its structured result close a node.
A reviewer, required whenever the project changed, submits APPROVE, REQUEST_CHANGES or BLOCK. Changes produce
fix nodes and another review, with a three-cycle ceiling. A fix node never keeps a read-only role: the runtime gives it a writing role chosen from its scope. Structured review findings confine fixes to the
named plan tasks when the runtime can verify them. With no findings and multiple
implementers, the runtime first returns to the coordinator to plan the remedy
while its return budget remains; otherwise unconfinable or absent findings give
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

For coordinator-submitted synthesis and direct answers, structured `not_verified`
is the sole source of the runtime's disclosure section. Items with the same blocker
render as one line (`check one; check two — Blocker: …`); the structured list keeps
every item (one device limit was listed about ten times in one answer, 2026-10-22). When that list is nonempty,
`orchestration/state.py:disclosed_synthesis` refuses an authored disclosure heading
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

State, graph and events survive process loss. Retry requires observed engine
absence and no uncertain action receipt. Review binds the project digest from
dispatch through verdict publication, verification and completion; a
`REVIEW_NOT_REQUIRED` record binds its observed digest the same way. See
[verification status](../../project/verification.md) for automated recovery test coverage.
No unsupported reconnect or absent observation is treated as successful recovery.
