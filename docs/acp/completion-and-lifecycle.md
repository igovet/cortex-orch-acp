# Runtime completion and lifecycle

The task state machine is defined in `orchestration/state.py`:

`CREATED → PLANNING → PLAN_READY → EXECUTING → REVIEWING → VERIFYING → RECONCILING → COMPLETED`.

An informational answer before any plan or worker uses
`PLANNING → ANSWER_READY → COMPLETED`. Only the runtime completes it after checking
the exact answer receipt, revision zero, no plan, nodes or worker provisioning,
and no blocker/cancellation. It cannot certify implementation or tests, replace
an existing plan, or skip required review.

Additional states are BLOCKED, FAILED, CANCELLED, WAITING_USER and RECOVERING.
A direct EXECUTING → COMPLETED transition is rejected. Model-facing APIs do not
accept state transitions or task acceptance decisions.

Only an immutable, attempt-owned report and its structured result close a node.
The mandatory reviewer submits APPROVE, REQUEST_CHANGES or BLOCK. Changes produce
a new fix node and another review, with a three-cycle ceiling. Verification is a
runtime command observation, independent of model review. Nonzero exit, incomplete
output or project drift prevents a passing verification event.

Every not_run or unverified worker check carries its blocker; result_submit rejects
one without it. Synthesis is refused while a retained not_run or unverified check
of the current plan revision is neither passed by a later result under the same
name nor listed in synthesis not_verified with its blocker. The runtime renders
that list into the synthesis as "Not verified".

The final gate checks the current plan, completed nodes, final review, exact
verification revision, report content digests, synthesis and absence of live worker
sessions. A report removed or changed after publication, or a project changed during
synthesis, blocks completion. Chat text is never parsed as an acceptance signal.

State, graph and events survive process loss. Retry requires observed engine
absence and no uncertain action receipt. Review binds the project digest from
dispatch through verdict publication, verification and completion. See
[qualification status](runtime-rewrite-status.md) for the distinction between
automated recovery tests and real-client observations.
No unsupported reconnect or absent observation is treated as successful recovery.
