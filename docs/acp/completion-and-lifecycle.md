# Runtime completion and lifecycle

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

Additional states are BLOCKED, FAILED, CANCELLED, WAITING_USER and RECOVERING.
A direct EXECUTING → COMPLETED transition is rejected. Model-facing APIs do not
accept state transitions or task acceptance decisions.

An explicit owner force-finish is **not** verified completion. `/cortex-finish`
or an unambiguous answer to a task-closure question stops task-owned work and
leaves the task terminal `CANCELLED`, with a `USER_FORCE_FINISHED` event recording
the captured owner source, prior state, current revision, unfinished nodes, and
the fact that review, verification, and synthesis were not certified by this
decision. The runtime never inserts a passing review or verification event for
force-finish. Unlike ordinary coordinator planning, it does not require another
worker or a model acceptance decision; unlike `COMPLETED`, it makes no quality
claim about the deliverable.

Only an immutable, attempt-owned report and its structured result close a node.
A reviewer, required whenever the project changed, submits APPROVE, REQUEST_CHANGES or BLOCK. Changes produce
fix nodes and another review, with a three-cycle ceiling. Structured review findings confine fixes to the
named plan tasks when the runtime can verify them; otherwise every original plan task receives a fix
(see [targeted review fixes](orchestration.md#targeted-review-fixes)). Verification is a
runtime command observation, independent of model review. Nonzero exit, incomplete
output or project drift prevents a passing verification event.

Every not_run or unverified worker check carries its blocker; result_submit rejects
one without it. A passed claim the runtime could not verify is recorded unverified
with the runtime's blocker and `evidence_gap` (see
[check evidence](orchestration.md#check-evidence)), so it is a gap here too. A
failed check fails its node unless an exact later rerun of the same command in
the same attempt closed it; the failure stays in the result as history with
`closed_by` (see
[check history](orchestration.md#check-history-and-current-outcome)). Synthesis is refused while a retained not_run or unverified check
of the current plan revision is neither passed by a later result under the same
name nor listed in synthesis not_verified with its blocker. The runtime renders
that list into the synthesis as "Not verified".

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
[qualification status](runtime-rewrite-status.md) for automated recovery test coverage.
No unsupported reconnect or absent observation is treated as successful recovery.
