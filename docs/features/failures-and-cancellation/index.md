# Failures, cancellation, and disconnects

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed. The former Python runtime was removed on 2026-11-16: references to its modules and tests were dropped, and statements that cited them describe behavior carried from that implementation that has not been re-verified against the Rust runtime.


Scope: crash/uncertain-outcome handling without duplicate mutation, hidden
paid work, accidental rollback, or cleanup of user resources. It covers the
permission bridge and binding registry, capacity, delivery ledger, questions and steering, process identity and recovery, isolated workspaces, and the route-profile fallback denylist.

A worker that ends its turn after receiving a pending command reply or a `command_not_settled` refusal can receive up to two same-session follow-ups. Workspace waits for the supervised commands identified on that attempt and supplies their final receipts. This also covers a command that settles between the pending reply and the end of the turn. The command supervisor retains its original deadline. Missing terminal evidence blocks continuation.

## Closing a task with conflicting records

Workspace shows **Not confirmed** when its saved execution records disagree.
The owner can inspect them through **Open Diagnostics** or confirm **Close as
is…**. Only the close disposition is recorded: no work is repeated, no process
is stopped or resumed, and the conflicting evidence is kept. After restart,
the task still shows **Closed by you · Not verified**, not a decision waiting
for the owner. It is grouped under **Closed, not verified**, not **Done**, and
cannot unblock a dependent task. See [Workspace task execution](../../cortex-workspace/local-task-execution.md#recorded-data-and-pending-questions).

## 1. Failure classification

`engine.failure_classification.classify(kind, message=None, exit_code=None,
signal=None)` maps an observed `EngineTurnFailed.kind`/JSON-RPC error text
into one of eight closed classes: `engine_failure`, `transport_failure`,
`rate_limited`, `authorization_loss`, `provider_restriction`, `refusal`,
`cancellation`, `indeterminate`. Matching is bounded substring search over a
short, already-captured string — never a scan of raw engine stdout/stderr,
never a regex with backtracking risk. Unknown/ambiguous input is always
`indeterminate`, never guessed as `engine_failure`.

`_process_new_events`'s
worker-prompt-failure path calls `classify()` before considering a
fallback attempt; only `engine_failure` is
`is_fallback_eligible()`. `authorize_fallback`
independently refuses any class outside
`KNOWN_FALLBACK_ELIGIBLE_CLASSES` (only
`engine_failure`/`capacity_unavailable`), so a rate-limit/auth-loss/
provider-restriction reading of the same failure never reaches route/
consent evaluation at all — two independent gates, not one.

Note: a downstream engine's own transport EOF (`kind="transport_eof"`) is
classified `engine_failure`, not the new `transport_failure` class — this
codebase already raises exactly that kind for "the engine process itself
became unreachable", and the existing fallback tests depend on that
being fallback-eligible. `transport_failure` is reserved for ACP
host-connection-level markers (`broken_pipe`, `connection_reset`,
`writer_closed`) this codebase does not currently raise as an
`EngineTurnFailed` kind.

A worker's Cortex MCP channel is separate from its engine process. When the
brokered Cortex MCP server has already answered `tools/list` and then exits
nonzero on its own, the runtime reports
`mcp_server_crashed`. The running prompt turn is cancelled and fails as
`cortex_mcp_connection_failed:mcp_server_crashed`. That kind classifies as
`indeterminate`, so it is never fallback-eligible. It is recorded as
`assignment_failed` only when the assignment has no `worker_result` or
`worker_question`, and it never gets the same-session completion follow-up,
which could only ask for publication through the lost tools. The runtime's
bounded retry then opens a fresh attempt,
with a fresh server, only after it confirms the old attempt is absent. An
uncertain action receipt or the retry limit returns the node to the coordinator
instead. A clean exit (the engine closed its own client) and the runtime's own
release are not losses. Before this, the broker kept reporting `ready`, so a
worker whose Cortex calls then all failed with `Transport closed` could end its
turn and be recorded as `worker_ended_without_result`. The stdio server itself no longer
ends on a reply it cannot encode. A non-UTF-8 file name (decoded with
`surrogateescape`) is shown as U+FFFD. A reply that is not JSON becomes that
request's own `-32603` error, which directs only an exact retry.

An authenticated replacement relay starts a new server generation and discards
the previous generation's readiness and tool/schema receipt. A known failure
remains visible until a matching response to that replacement's actual
`tools/list` proves a complete list with the required publication tools and
object input schemas. An incomplete, malformed or paginated list fails closed
as `mcp_tools_incomplete`; merely connecting or answering `initialize` cannot
suppress the prompt watcher's named loss. Replies and exits from older
generations, or replies arriving after their server's exit, cannot alter current
readiness. A complete replacement creates fresh evidence; it does not authorize
replaying an uncertain action or bypassing attempt reconciliation.

Separately, `engine.failure_classification.route_failure(error)` decides
whether an engine's JSON-RPC error is about the route rather than the work.
It reads only the adapters' own structured `data` fields, never message text:
codex-acp 1.13.0's `codexErrorInfo` (`usageLimitExceeded`, `rateLimitExceeded`)
and claude-agent-acp 0.81.0's `errorKind` (`rate_limit`, `billing_error`,
`account_on_hold`, `model_not_found`). A worker turn refused this way fails
as `EngineTurnFailed("route_unavailable:<reason>")`. That classifies as
`rate_limited` or `provider_restriction`, so it is still never fallback-eligible,
and the route is recorded unavailable for automatic routing (see
[routing and fallback](../routing-and-fallback/index.md#automatic-worker-routing-unavailable-routes)).

### MCP tool errors

The native gateway reports every failed Cortex tool call by its stable code;
enforcement does not depend on how a failure is reported. After the channel is
authenticated, a handler failure is an `isError` tool result whose structured
content carries `code`, the offending `field` when known and a short corrective
`hint`, for example `request_key_conflict`, `gateway_write_conflict`,
`invalid_scope`, `synthesis_gaps_undisclosed`
(with the current gap names) or `pipeline_missing`. Authority failures stay
JSON-RPC `-32001` refusals named `authority_refused: <code>` with
`data: {code, retryable: false, hint}`: a channel that is no longer current, a
tool not offered to the role or stage (`tool_not_offered`), a read-only role,
an owner-protected path (`write_denied`) or a missing owner confirmation
(`deletion_not_confirmed`). `result_submit` keeps its own split between
correctable content refusals (tool errors) and named refusals. Arguments that do
not match the advertised schema are `-32602 invalid_arguments` with a pointer to
the live schema. Hints are fixed text; they never include another task's data, a
path outside the project or a credential.

The former Python MCP server distinguishes a typed portable failure from an unexpected storage
exception. Its portable envelope retains a bounded identifier code, `category`,
`retry_eligible` and `write_state`, with fixed, value-free guidance. It does not
copy exception remediation/details or arbitrary fields, which can contain local
paths or policy values. Route-policy migration, unsupported-version and malformed
policy errors have specific safe corrections.

`TaskMachine.submit_plan` checks authorization and the recorded key/payload before
reading current model policy. An exact accepted replay returns the stored result,
even if that policy is no longer available; a changed payload under that key is
rejected. The model preview is read-only and precedes plan writes in the same
transaction. A portable refusal there is marked `rejected` (except a declared
`indeterminate_write`); it records no accepted plan, nodes or request receipt.
`list_route_options` uses the same safe translation for its read-only failure.

A portable failure caught at an unknown stage is not proof of rollback:
`write_state=uncertain` and `retry_eligible=false` require reconciliation using
only the identical request and delivery key. Even a validation-category error can
occur while preparing a reply after commit. A raw storage/unknown exception still
returns conservative `storage_error`; that label alone does not prove a disk
outage or a commit outcome. Its public cause uses the exception class and a
standard errno reason, never the exception-owned reason or filename.

The former Python no-paid scenario exercised real
policy validation through dispatch and stdio, controlled precommit/commit/reply
faults, rollback, exact replay, key conflicts and concurrent identical requests.
It observes transaction state in disposable stores. It neither diagnoses an
earlier runtime nor injects faults into live tasks. See the
[scoped command](../../project/verification.md#plan-error-recovery).

## 2. Retry policy and reconciliation

`engine.retry_policy.decide(operation, attempt_number, reconciled=...)`
names which operations are safely repeatable
(`CAPACITY_RESERVATION`/`WORKER_SESSION_OPEN` — no prior external effect)
versus which require an explicit, positive `reconciled=True`
(`ENGINE_PROMPT_TURN`/`PUBLICATION` — may already have mutated externally).
Without reconciliation, `decide` always refuses retry
(`reconciliation_required_but_not_performed`); callers then record an
`indeterminate` outcome rather than ever claiming exactly-once.

This formalizes patterns already present in the codebase:
`OrchestratingRuntime._attempt_worker_route_fallback` reconciles the old
attempt (`lookup_owned`/`terminate_owned`, channel revocation) *before*
opening a replacement session; the runtime
checks for an already-published `worker_result` before writing
`recovery_worker_lost`, re-checked inside the same transaction as that
write so a concurrently-published result always wins;
the runtime/the runtime use idempotency keys for
every durable publication. No blanket exactly-once guarantee is made
anywhere for an external mutation whose fate cannot be observed.

### Transient engine failures

- **Coordinator launch.** The native host makes one rapid second attempt only
  for a transport loss during configuration or access-mode setup before the
  session is bound, and only after the first attempt is confirmed exited. A
  final `coordinator_session_start_failed` follows the existing task recovery
  strategies, not a separate timed retry window. See
  [native session start failures](../recovery-and-replacement/index.md#native-session-start-failures).
- **Coordinator turn.** If the reply to a dispatched turn is lost after it
  advanced the task state or plan revision, or saved the final synthesis, the
  runtime continues from that checkpoint without replaying the turn.
  Otherwise `native_turn_outcome_unknown` continues on the current
  approach. Without real progress, the third identical interruption starts a cooldown;
  each recurrence schedules the next one. The first five waits are 5, 15, 30,
  60 and 60 minutes. On the sixth series, the runtime records the reason and
  asks the owner instead of scheduling another cooldown. The question survives
  restart. Real progress such as a completed node or passed check resets the
  series; accepting a new plan by itself does not. On resumption, the runtime
  re-verifies uncertain effects rather than replaying the interrupted prompt.
  See [the task lifecycle](../task-lifecycle/index.md#no-stop-policy-and-the-strategy-ladder).
- **Workers.** In `retry_lost`, an assignment whose attempt ended for an
  infrastructure reason (see [Why a worker attempt ended](#why-a-worker-attempt-ended))
  is retried on the runtime's backoff. The `ASSIGNMENT_RETRIED` event records
  `failure_class`, the observed `ending`, `failed_at`, `retry_not_before` and
  `infrastructure_retry`. These retries never escalate the node, never take a
  rung of the strategy ladder and never end in `worker_retry_limit`: a streak
  longer than the 600 s window waits the longest ordinary delay (60 s), and a
  node lineage (the node and its review fixes, in every plan revision since the
  task was last reopened) has 6 ordinary infrastructure retries. After that the
  node waits on a growing delay (5 minutes, doubling up to one hour;
  `infrastructure_wait: true`, one `LOOP_BROKEN` with action
  `infrastructure_wait`) and is retried by the runtime when the delay ends,
  without a replan, or at once when the owner chooses Retry (`OWNER_RETRY_REQUESTED`
  ends every pending delay of the task). A route failure (`capacity_unavailable`) is bounded the same
  way. A full engine connection and a clearly typed temporary provider
  rate/quota refusal are not failures at all: the worker waits
  (`waiting_for_engine_capacity`, `waiting_for_provider_capacity`), is not
  retried or counted, and restarts on the same route; cancellation, pause and stop
  end the wait with nothing launched. Ambiguous, auth, billing and invalid-model
  errors keep the failure path above. An owner pause (`owner_paused`) or a restart (`runtime_restart`) is
  retried at once and not counted. A model that lost the same node lineage twice
  is routed away from on the next dispatch (another model when one is usable,
  preferably another engine) without raising the tier. A quality retry
  (`result_refused`, `degenerate_turn`, `indeterminate`) escalates the node, and
  after two quality retries of a node in one plan revision the next quality
  failure stops with `worker_retry_limit` (a rung of the strategy ladder).
  `TaskMachine._ready_nodes` holds the node until its delay ends, with the
  scheduler wait reason `retry_backoff`. The chat shows that wait as a card
  titled `Retry N: <node> — the worker's engine failed`.

### Why a worker attempt ended

When a worker attempt ends without an accepted result, the runtime records why
(`ATTEMPT_ENDED`, and the attempt's `assignment_failed` event that `retry_lost`
classifies): how the turn ended (the model ended it, the engine process exited,
the owner stopped the worker, the service stopped, the task was interrupted,
or a turn error code), the engine's exit status or signal, whether it exited
before the host asked, the transport close reason, a runtime cancel of one of
its permission requests under the read-only rule, the results it had refused
(count and last code), and the last 400 characters of its already redacted
stderr tail. No credential, argument or output text is kept.

| Class | Recorded when | Kind |
| --- | --- | --- |
| `result_refused` | the model ended its turn after refused results and no accepted one | quality |
| `degenerate_turn` | the turn tripped the degenerate-turn watchdog | quality |
| `indeterminate` | the model ended its turn without submitting anything | quality |
| `worker_lost` | the process vanished without the model ending its turn, or the only refusals were for a checkout that kept changing | infrastructure |
| `sandbox_unavailable` | the attempt's last command could not get a sandbox (`native_command_sandbox_unavailable`, `git_context_unavailable`) | infrastructure |
| `owner_paused` | the owner stopped the worker or paused the task | infrastructure |
| `runtime_restart` | the service stopped or restarted, or the driver was cancelled | infrastructure |
| `transport_failure` | the turn ended with an unknown outcome or a transport error | infrastructure |
| `engine_failure` | any other engine error | infrastructure |

The infrastructure classes are `engine_failure`, `transport_failure`,
`worker_lost`, `authorization_loss`, `capacity_unavailable`, `owner_paused`,
`runtime_restart`, `sandbox_unavailable` and `verifier_environment`. An attempt
with an accepted result records nothing.

### Repeated refusals inside one attempt

A worker may correct a refused `result_submit` and submit again in the same
turn. When the same correctable refusal (the same code and the same named items)
reaches its third occurrence in one attempt, the runtime records the submission
instead, with what stays open disclosed, and records `LOOP_BROKEN` (kind
`result_refusal`). It never records a pass:

- A check without a blocker is recorded as not run or unverified with a runtime
  blocker; unread required predecessor reports are disclosed as an `unverified`
  runtime limit.
- A review whose parts stay uncovered, that returned INCOMPLETE without naming
  them, or that returned BLOCK without findings, is recorded INCOMPLETE with those
  parts not covered (dead end `other_role_work`) and an `unverified` limit.
- A failed check that stays open (`open_failed_check`, `omitted_failed_check`, an
  invalid retraction or supersession) makes an execution assignment failed, with
  the open check as its blocker (`other_role_work`; the omitted failed runs are
  cited as failed), and an approving review INCOMPLETE. A failed outcome without a
  dead end is recorded with one.

Stricter bounds stay: two refusals for missing report sections or unread changed
files, one repair offer for a failed check. A refusal for a checkout another
writer keeps changing (`review_project_revision_*`, `review_resubmit_required`),
a command that is still running (`command_not_settled`) and a malformed
submission are never relieved this way and never count toward this limit.

### Native authentication loss

A positively classified native-account auth failure has a separate **one-shot,
same-route** recovery path. Cortex terminates the owned engine process and
requires observed absence before launching a replacement, allowing the native
CLI to re-read its own account state. Cortex neither reads/copies token values
nor calls a login, token-refresh, usage prompt or keepalive command.

- **Coordinator launch and turn:** the allowance is committed before teardown
  as a `task_events` progress event with kind `COORDINATOR_AUTH_RETRY`. Launch
  and turn share that fence, including after runtime recovery. Only a successful
  prompt records `COORDINATOR_AUTH_RECOVERED` and ends the episode; a successful
  open alone does not reset it. A second auth failure retains
  `COORDINATOR_AUTH_BLOCKED`, checked before another launch during recovery.
  An explicitly identified new user input may make one manual-login probe
  (`COORDINATOR_AUTH_PROBE`), without replenishing the automatic retry allowance.
- **Worker:** `retry_lost` reconciles action receipts, confirms process absence
  and revokes the old channel before making the node pending. The
  `ASSIGNMENT_RETRIED` event commits `auth_retry=true` with that state change.
  One allowance applies to a node/plan revision, including runtime restarts.
  The replacement pins the previous audited connection, engine, model and effort
  and still passes current dispatch authorization. Missing or no-longer-authorized
  route identity blocks rather than choosing another account.
- Both paths use the existing retry window/backoff. Auth replacement is not a
  quality retry and does not raise model tier or authorize fallback. Rate-limit
  and provider-restriction behavior is unchanged.
- A turn with observed prose/tools or changed coordinator state is not blindly
  replayed. Worker replay additionally requires an explicitly safe first-turn
  failure; an auth loss during a later continuation blocks. Unknown process exit
  or incomplete action receipts still block replacement.

If the replacement also loses authentication, Cortex stops automatic recovery
and shows `engine_authentication_required`, directing the user to
`claude /login` or `codex login` before resuming. Unsafe replay and an exhausted
retry window also stop visibly. There is no automatic alternate route or
escalation for this blocker. Restarting can repair stale in-memory account state;
it does not guarantee renewal of an expired or revoked native login.

### Native CLI login keepalive (removed)

The standalone login keepalive script and its optional `systemd --user` timer were Python-era tooling and were removed on 2026-11-16 without replacement. The runtime never performed a re-login on its own; a needed re-login is reported by the engine, and the owner re-authenticates the native CLI.

### Project scans outside the store's write lock

Some write transactions decide on an observed project revision: a review result binding, completion
and answer checks, verification credit, synthesis, the execution baseline and
the review waiver. Each one used to scan the project inside `BEGIN IMMEDIATE`;
synthesis scanned at least four times. SQLite's write lock covers the whole
database, so every other writer, including another chat's runtime on the same
project, failed with `database is locked` once its 15 s busy timeout ran out.
`TaskMachine` and `synthesis.complete` now scan once before the transaction
(`observed_project`,
`observation_snapshot`), and every capture of that root
inside the transaction returns that observation. The scan holds the
project-change lock (`.cortex/.project-mutation.lock`, shared) until the
transaction ends. A gateway file effect (`project_write`, `project_edit`,
`project_mkdir`, `project_delete`, `project_skill_link`) holds that lock
exclusively, so such an effect still cannot land between the scan and the
decision. Native engine tools were never excluded by either lock.

### Contended writes and transient lock errors

Scheduler and lifecycle writes run through `Store.immediate_write`: event append and claim, delivery and release marks, `return_to_coordinator`, coordinator turn start and end events, and assignment cancellation. It begins with `BEGIN IMMEDIATE`, so the busy timeout applies to the write lock itself. It retries a still-contended lock up to eight attempts, sleeping 20 ms and doubling up to 0.4 s between attempts. A read on the same connection before `BEGIN IMMEDIATE` can fail at once with `SQLITE_BUSY_SNAPSHOT` regardless of the timeout, so these writers never read before taking the lock. Their transaction bodies must be replayable, because they can run more than once, and they neither begin nor commit a transaction themselves.

The controller treats a storage-contention error (`database is locked` or `storage_busy`) raised during a turn as a transient failed pass. It waits from 0.5 s, doubling up to 10 s, on the turn's cancellation event, then resumes from durable state. The turn is not unwound, so running workers are not torn down for it. The controller re-raises the error after 900 s of contention, when the turn is cancelled, or when the host transport is lost. Contention met while reconciling results, completing the task or retrying a lost worker is re-raised as well, instead of becoming a `BLOCKED` stop or a coordinator return that cancels the siblings.

A coordinator prompt that returned normally is not a failed engine call, even when recording its turn end is contended. Its bookkeeping writes (the `COORDINATOR_TURN_ENDED` event, the authorization-recovered mark, the delivered mark for the claimed events and the context-delivery confirmation) sit outside the prompt's failure handler. On contention they retry in place with the same backoff and one shared 900 s allowance, and the failure handler's own release, turn-end and delivery writes do too. Re-entering the pass instead would prompt the coordinator again. When the allowance is spent, the turn is cancelled or the transport is lost, the error ends the turn without a re-entry; it never becomes `coordinator_prompt_failed`, a `BLOCKED` stop or a sibling cancellation.

## 3. Cancellation (`session/cancel`)

`session/cancel` interrupts the current turn, revokes current worker channels,
terminates owned sessions and persists RECOVERING (paused: the next message
continues the task; `/cortex-stop` closes it). When it pauses a non-terminal
task, the turn ends with a localized `turn_paused` line saying so, plus
`turn_paused_unknown_actions` with the number of worker actions this stop cut
off mid-way (open receipts whose channel the stop revoked). It is distinct from a queued
Paseo steer, which uses a concurrent configuration request and does not call
`session/cancel`. On a Paseo session, a `session/cancel` followed by a
`session/prompt` on the same session is steering rather than Stop: the cancelled
coordinator turn ends at once, running workers continue (their cards stay running;
see [worker cards across steering](../steering-and-questions/index.md#worker-cards-across-steering)),
and the prompt is applied as steering. A lone cancel pauses the task only after a 5-second grace window with no
following prompt (`DEFAULT_HOST_CANCEL_GRACE_SECONDS`); other clients pause at once. A deferred
pause cannot write its `turn_paused` line into the cancelled turn, so the runtime shows a
`turn_paused_earlier` notice, with the cut-off action count, at the start of the next turn. If the
deferred pause itself fails, a `host_cancel_pause_failed` progress event records it, because the
task may still be running. A prompt that answers a question on the held session also resolves the
hold as steering.
The cancellation path records `TURN_INTERRUPTED`, not
`USER_STEERING`; `turn_interrupted` classifies as `cancellation` even when
an adapter prefixes it with `engine_error`, and a cancelled worker never
enters fallback routing. A worker whose engine open was still queued, or still
starting, when its node was cancelled ends with `attempt` or `channel_revoked`.
That worker gets only its `assignment_cancelled` event, never an
`assignment_failed` as well. Its runtime node is durably `cancelled` first. A
later user
prompt can resume planning from durable state. That RECOVERING pass cancels
only the assignments that were live at the stop (reason `replanning`); the work graph keeps an
attempt that is already failed with its first reason and records no
`work_attempt_sync_failed` for the repeat. It does not cancel unrelated tasks.

`/cortex-stop` permanently marks the task CANCELLED before cleanup. It closes
pending questions, fences late publications and cancels capacity-queued opens.
A captured `/cortex-finish` command or the Finish answer to a runtime stop question an earlier version stored (the runtime asks none now; see the [no-stop policy](../task-lifecycle/index.md#no-stop-policy-and-the-strategy-ladder))
uses the same cancellation cleanup, but also records `USER_FORCE_FINISHED`
with unfinished work. It is terminal `CANCELLED`, never verified `COMPLETED`.
A worker opened concurrently with cancellation is closed before its first prompt.
Reservations release only on observed process absence. Unknown engine liveness
remains a blocker; a terminal event alone never proves process exit. Terminal card
updates are retained and replayed if the client disconnected before seeing them.

### Recovery cancels only live assignments

Every path that stops or replans live work records the live set in the same transaction that cancels the pending and running nodes: `session/cancel` interruption, `propose_replan`, `return_to_coordinator` and both stop-question branches. Each writes a `LIVE_ASSIGNMENTS_CANCELLED` event naming the assignment references it cancelled. The RECOVERING step reads the captures not yet settled, cancels and terminates exactly those assignments, and sweeps only the owned provisioning attempts whose idempotency keys match them. Completed and failed assignments, and any assignment an earlier recovery already settled, are not touched. A completed worker still settling its result is left to its own settlement.

Recovery needs each captured attempt's capacity released, which happens only when its session process has been shut down. A worker turn that is still running when its channel is revoked or its owner epoch replaced is therefore ended by the runtime rather than left to finish by itself: besides the engine's own authority monitor, which closes the transport on observed authority loss, the active prompt checks once a second whether its worker channel was revoked, gives a revoked turn 10 seconds to end by itself (its closing reply after its own accepted replan or escalation), then sends ACP `session/cancel`, and lets the turn settle with stop reason `cancelled`. If the engine does not reply within 25 seconds, the turn ends with its outcome unknown and the session's ordinary shutdown terminates and settles the process (`SessionDriver::prompt` in `crates/workspace-service/src/native_sessions.rs`). Coordinator sessions and workers whose authority is intact are not affected.

The captures survive a restart in RECOVERING. The RECOVERING-to-PLANNING transaction writes `LIVE_CANCELLATIONS_SETTLED` for the captures it covered. If a live worker's termination is not confirmed, the task moves to BLOCKED with `replanning_worker_termination_unconfirmed`, and a later recovery rechecks the same captured attempts. A node cancelled by a stop recorded before captures existed is captured on its first recovery with reason `unrecorded` and handled the same way.

### Degenerate model turns

A model turn can stream unusable text instead of acting: replay its own prompt,
loop on the same words, or write tens of kilobytes without a tool call. The
runtime does not settle such a turn as an ordinary reply. Every agent message of
a coordinator or worker turn streams through a watchdog
(`degenerate` in `crates/workspace-service/src/native_sessions.rs`) with four rules:

| Rule | Trips when |
| --- | --- |
| `echo` | an agent message contains text only the runtime's own prompt carries: `Cortex retained-state recovery capsule`, `<system-reminder>` or `Runtime status:` |
| `repetition` | a message of at least 1,000 characters repeats 30 % or more of its 16-character windows (sampled every 4 characters) |
| `length` | one agent message reaches 16,000 characters with no tool call between its parts |
| `refusal` | a runtime-driven turn ends with the ACP `refusal` stop reason |

The bounds come from retained production sessions: the largest ordinary message
was 4,748 characters (one legitimate consultant message 13,121), the largest
ordinary repeated-window share 0.045, and none of the markers appeared in seven
days of ordinary agent text. Agent thoughts and tool calls are not watched.

On a trip the runtime records `NATIVE_TURN_DEGENERATE` (channel, session, role,
the worker's node and plan revision, the rule, the engine message id, the message
size and, for `repetition`, the share; never the text), sends `session/cancel`
and gives the engine the same 25 seconds as a revoked worker before the turn
ends with its outcome unknown and the session is terminated. Agent text that
arrives after the trip is not added to the conversation. The turn is then
retried once in a fresh engine session:

- A coordinator turn that left the task's state and plan revision unchanged runs
  again with the same prompt in a new session. A degenerate turn that still
  changed the task (an accepted plan, a question) stands, and the driver
  continues from that state.
- A worker turn ends with its node still open and its session terminated, so the
  lost-worker path (`ASSIGNMENT_RETRIED`, failure class `degenerate_turn`, a quality
  class) replaces the assignment in a fresh session on an escalated route at the
  next step.

A degenerate turn never stops the task and never runs a third time on the same
model. When the coordinator's retry degenerates too, the coordinator moves to
another qualified model, preferably on another engine or account
(`NATIVE_COORDINATOR_ESCALATED`; later coordinator turns stay there until the
owner chooses a model again), and the turn runs there. Only when no other model
qualifies, or that one degenerates as well, does the turn end with
`native_turn_degenerate`, which the [no-stop policy](../task-lifecycle/index.md#no-stop-policy-and-the-strategy-ladder)
records as a continuation on at least the `escalate_route` rung that again moves
the coordinator to another model. A worker node that degenerates again is
replaced each time on an escalated route (another model or engine than the
previous attempt), and the lost-worker retries then continue on the strategy
ladder. Direct `SessionDriver::prompt` callers outside the
runtime's turn loop still see a `refusal` stop reason as an ordinary reply.


### Stopping one worker command

Stopping a brokered gateway command is distinct from task or assignment
cancellation. A user-originated `/cortex-kill` input, Paseo command-stop control,
or owner-local CLI request targets one exact command reference and requires a
reason. Cortex stores the request and the runner terminates that command's
process group. The worker receives a structured `stopped_by_user` outcome and
the reason, while the assignment remains active; the worker is instructed to
continue without rerunning the same command unchanged. The command activity is a
failure (not a successful command or a cancelled worker). A durable request
acknowledgement is not proof of termination: confirmed termination is recorded
only after kill and wait, while a command that exited first is recorded as a
missed stop. An unresolved started-command record after a runtime crash is not
proof that its operating-system process is still alive.

### Close and Continue on a task with an unknown outcome

**Close task** commits the owner's cancellation at admission. If an earlier
attempt's process is still unconfirmed when the close finishes, the close is
reported done anyway (receipt `processesUnconfirmed: true`, and
`OWNER_TASK_RESOLVED` records `processes_unconfirmed`): the task is closed
whatever that process did, so the window closes the card. Nothing claims the
process exited; Delete keeps refusing `active_execution` until restart recovery
settles it (see [recovery](../recovery-and-replacement/index.md#native-attempts-left-unconfirmed-by-an-earlier-host)).

A paused task that a restart held with an unknown outcome (for example
`native_session_outcome_unknown`) refuses a desktop message as `task_paused`.
**Continue** resumes it as RECOVERING with that reason; when every owned
process is confirmed gone (Resume refuses `native_shutdown_unconfirmed`
otherwise) it then continues like an owner message: the coordinator is
relaunched to replan from the retained state, instead of the task silently
staying in RECOVERING with no driver.

### Service restarts

A service restart is infrastructure, never the model's failure. When recovery
finds a task that was planning, executing or reviewing, interrupted by the
restart or left with an unknown session or effect outcome, and every process it
owned is confirmed gone, the task resumes in the same state on the same rung of
the strategy ladder, without a continuation or a new plan revision
(`NATIVE_RECOVERY_RESUMED` with `restart_resume: true`):

- effects the gone sessions left without a final receipt are settled once with
  outcome `unknown` (`settled_by: runtime_reverify`, `RUNTIME_UNCERTAINTY_RECORDED`);
- each attempt the restart ended without a result is recorded as
  `runtime_restart`, so its node is retried at once, uncounted and not escalated;
  the retry carries those effects (`reverify`) and the fresh attempt is told to
  re-verify them from the files, Git state and receipts before redoing anything.

A command receipt that is still unsettled, a process that is not confirmed gone,
an open question or wait, an answer being delivered, a legacy start, a
verification or a desktop control keep the recovery rules above. When restarts
find the task at the same plan revision a fourth time without progress
(`boot_recovery` loop key; `LOOP_OBSERVED` from the second), the task pauses
instead (`AUTOMATIC_PAUSED`, `pause_kind: infrastructure`, `LOOP_BROKEN` with
action `infrastructure_pause`) for the runtime's own recheck, without a model
call. `NATIVE_RECOVERY_REQUIRED` is recorded once per restart burst: a restart
that finds the task held for the same reason in the same state, with nothing
recorded for it since, adds no event.

## Terminal resource cleanup

`OrchestratingRuntime.start_prompt` and explicit `answer_question` turns check
durable terminal state in their exit paths and invoke task-owned teardown for
COMPLETED, FAILED and CANCELLED.
It does not call the task cancellation transition for successful or failed work.
Unexpected prompt exceptions run the same resource cleanup and propagate the
original failure; the persisted task remains available to fenced recovery.
Ordinary waiting, paused/recoverable or steering prompt returns are not terminal.

Cleanup first revokes channels of exactly its owned coordinator/worker attempts,
including queued and partial opens, then stops their engines. Revoked coordinators
cannot mint new worker channels while provisioning unwinds. Cached worker sessions
are matched to exact attempts, not a replacement's updated assignment row. Cleanup
drops stale coordinator/requester caches and records scope release only on observed
absence. Shutdown runs outside SQLite transactions and the general runtime lock;
a per-task resource lock serializes cleanup with replacement-turn admission.
Explicit-answer continuations adopt their current session before opening an engine;
answers that only wake an already-active worker do not take over that turn.
Old-connection EOF and stale turn generations cannot clean up replacement resources.
Repeated stop on a terminal task retries teardown without changing
its outcome. Unconfirmed cleanup retains reservations/quarantine and a
metadata-only `resource_cleanup` progress diagnostic; it is not successful exit
evidence.

`EngineProcess` retains the launched Linux session/group identity, waits for
active group members as well as its reaped leader, and escalates TERM to KILL
even when the leader exited first. Unreadable identity remains unknown and is not
signalled. The runtime cannot reap grandchildren whose OS parent is elsewhere;
zombies are inactive, and descendants that leave the owned group are outside this
observation. Brokers and terminal-bridge children keep their separate owned
shutdown paths. Partial-launch writer locks release only after confirmed exit.

Unexpected ACP prompt errors include exception class and source basename/line
in the error message as well as its data, since a host may discard data.
Exception messages, credentials and raw logs are excluded. This identifies the
observed failure, not the historical cause of a generic error.

The incoming ACP process is connection-owned and may serve multiple sessions.
It shuts down on host EOF/writer-close/SIGTERM; a prompt return alone does not
authorize exiting the host connection. Local fake-process tests qualify teardown
and writer reuse, not paid engine or rendered client behavior.

## 4. Disconnect

The transport's reader EOF event signals that input ended, not that its
disconnect callback finished. On the stdin-EOF path, the runtime joins
the reader before joining prompt workers and shutting down the runtime.
This preserves recording/cancellation before process exit even when the EOF
callback is still running. The writer-closed event likewise precedes its
disconnect callback; on that path, `main` joins the writer before joining
prompt workers and shutting down the runtime. SIGTERM runs its disconnect
callback synchronously on the main thread. SIGTERM and writer-close must not
join a reader blocked on still-open stdin.

`_handle_eof`/`handle_writer_closed` (host transport EOF or
writer-closed) already invalidate every session's `cancel_event` and close
the connection's `HostRequester` (resolving every pending host round trip —
permission, elicitation, fs — as closed). The runtime adds
`OrchestratingRuntime.disconnect_session(session_id)` (reached via
`ProductionOrchestratingRuntime.disconnect`): invalidates the coordinator's
own permission binding the same way `cancel` does, records a durable
`progress` event naming what was paused/cancelled/unconfirmed, then runs
the bounded owned-resource reaper. No adapter
currently supports a confirmed pause, so cancel is the correct default per
the task's own "confirmed pause, else cancel" rule — never hidden
continuing paid work.

A disconnect cancels the in-flight engine turns, never the task's outcome.
A Paseo plugin update restarts the plugin process and closes every open
Cortex connection at once; Paseo then marks each open chat, including
already completed ones, with `Provider connection closed`. That message is
host-owned and cannot be suppressed from inside the plugin. Inside Cortex:

- The disconnected task is marked before its turn is woken, so the
  controller ends the turn without `policy.interrupt`: no
  `TURN_INTERRUPTED`, no `turn_interrupted` assignment cancellation, and
  task, node and assignment state stay as persisted. Only a user's cancel
  pauses the task that way.
- `OrchestratingRuntime.shutdown()` sets a teardown flag first. A worker
  whose engine the teardown kills, even one whose turn event the
  disconnect cannot reach, is not recorded as `engine_failure`, is not
  routed to fallback and is not re-dispatched during teardown.
- A coordinator turn that ends with an unhandled error tears down its workers the same way. The task is marked host-disconnected before its cancellation, so a worker that teardown kills is recorded as transport loss, never `engine_failure`, and gets no fallback. Its work stays as persisted for recovery.
- The durable record is a `progress` event of kind `host_disconnected`
 with the exact `session_id`,
  `task_id`, `coordinator_attempt_id`, each interrupted worker's
  `node_id`/`assignment_ref`/`attempt_id`, each gateway action whose
  receipt is still open (`unknown_outcome_actions`: operation,
  `tool_call_ref`, attempt), the task state at disconnect, the runtime
  package version and `history`: `complete`, `truncated` (more than 50
  entries) or `unavailable:<error>`. A chat only restored by `session/load`
  in this process, with no turn here, is recorded by the ACP agent with
  `turn: no_turn_in_process`. `trigger` names what the process observed
  first: `sigterm`, `stdin_eof` or `writer_closed`. In the isolated dev
  Paseo 0.9.2, `plugin reload cortex-acp` produced `sigterm` (the load shim
  forwards it); the record was written about 0.4 s after the reload began
  and the process released its engines and exited before any SIGKILL.
- Every worker launch also records the Cortex ACP process that owns it
  (an `acp_host` row beside the worker's `attempt_process_registry` row).
  When no signal reached the old process (SIGKILL, OOM), there is no
  `host_disconnected` record; a later reader treats a worker with no
  terminal event whose owning process is provably gone as interrupted. A
  live or unidentifiable owner never qualifies, and the read writes nothing.
- Paseo 0.9.2 does not reload the session after a plugin reload and sends
  nothing until the next prompt, so a worker card would otherwise keep its
  last `running` status through a page reload. The plugin's card polls the
  durable attempt status while the host shows it running and settles on
  Interrupted (or any other settled status) without a new message; the
  Pipeline panel reads the same projection.
- After restart, persisted state is the authority. The next prompt runs
  coordinator recovery: interrupted workers are recorded
  `recovery_worker_lost`, and `retry_lost` retries a lost node at most
  twice only after observed process absence and only when it owns no open
  gateway receipt. A receipt is closed when it is complete, or settled with an
  observed end: the supervisor's final status (an exit code, a signal, an owner
  stop, a timeout, or an unverifiable output with a finished supervisor), a
  launch that provably never started (`launched: false`), or a read-only
  command whose PID namespace ended. A receipt settled only because its process
  is provably gone, with no observed end (outcome `unknown`), stays open for
  this rule: the action may have taken effect, and a fresh attempt of the same
  node could repeat it. A node that owns an open receipt is returned to the coordinator with
  `lost_worker_action_outcome_unknown`; the coordinator's planning request
  lists each such action and states that it was not re-run. The
  gateway's own `request_key` fence still refuses a repeat of the same call.
  A worker written off as `recovery_worker_lost`, and the replan
  cancellation that follows, still displays as Interrupted, never as a
  generic failure; the retry is a new card. The recovery turn sends no update
  for a lost card the host already shows, because Paseo 0.9.2 renders an
  update from a later turn as a second row.
- A restarted runtime never re-dispatches a delegation request that
  recovery already closed with `assignment_failed`/`assignment_cancelled`.
- On `session/load`, while this session's latest `host_disconnected`
  record has no later `user_message`, the agent appends one read-only,
  localized notice after the replayed history (message id
  `cortex-connection:<event id>`, `_meta.cortexConnectionNotice`), but only
  when that disconnect interrupted something: a running worker or a worker
  action in flight (open receipt on a still-open worker channel). A finished,
  paused or idle chat reopens without it, and actions an earlier stop already
  fenced are not re-counted. It states the saved task state (and that a
  completed result is unchanged) and those two counts from the disconnect
  record. External MCP calls to a tool the server lists in `read_only_tools`
  are never counted as unknown outcomes. Loading writes nothing.

Native engine tools are not gated per call, so a native edit or shell
command in flight at the disconnect has no gateway receipt and cannot be
listed as an unknown outcome; recovery still never replays the lost turn.
The `plugin reload` path was exercised in the isolated dev Paseo 0.9.2 on
2026-09-26 (see [verification status](../../project/verification.md)); until the
deployed package carries it, apply plugin updates between active tasks.

After the agent joins its prompt workers at daemon teardown, it also calls
`ProductionOrchestratingRuntime.shutdown()` (and each cached
`OrchestratingRuntime.shutdown()`). This closes the owned engine processes
and releases confirmed capacity for an ordinary EOF or writer-close. A hard
host kill can still leave an ambiguous registry row; the next
`session/prompt` uses the existing cross-process recovery path, while
`session/load` only applies the read-only card correction described in
`input-history-and-replay.md`.

## 5. Session worker capacity and stale-slot reclamation

One chat session (the ACP session and task it drives) runs at most four workers
across engines and connections. The scheduler leaves excess dependency-ready
nodes pending. Confirmed worker termination releases a reservation exactly
once; conditional reservation transitions prevent a late ambiguous-state update
from resurrecting a released slot. A slot is reclaimed only with evidence that
the worker is gone: the owning adapter observed its process exit, the durable
process registry reports the identity absent, or (when no process record exists)
the unrecorded reservation is older than one hour and is not currently opening.
An alive or unknown process remains reserved. Reclaims emit a safe
`capacity_reclaimed:<reservation_id>` diagnostic event. The scheduler retries
stale-capacity reclamation while pending work exists, and a queued open reclaims
eligible dead holders before waiting; unavailable evidence does not free a slot.

## 6. Owned-resource reaper

`engine.reaper.sweep(store, engine)` — two bounded, idempotent sweeps,
invoked once per project at first runtime use
(`ProductionOrchestratingRuntime._runtime_for_project`) and after a
disconnect:

- `sweep_capacity`: re-probes every `ambiguous` capacity reservation via
  the caller's `lookup_owned`/`terminate_owned` (process identity —
  pid + start ticks + boot id). Capacity is released only on a freshly
  confirmed absent; an `unknown` probe leaves the reservation exactly as
  ambiguous — never falsely released.
- `sweep_owned_workspaces`: removes only a Cortex-owned isolated snapshot
  workspace whose attempt reached terminal+integrated (or cancelled),
  mirroring `_cleanup_isolated_workspace_if_integrated`'s exact predicate.
  Never touches `project_root`.

Both are bounded (`limit`, default 200 rows) and never touch a process or
path this Cortex process (or a durable prior-process record) did not
itself launch/own.

## 7. Fault injection

Fault-injection checkpoints `t028_*` (`_fault_injection.checkpoint`, same
env-var-gated `os._exit(137)` mechanism):
`t028_cancel_before_permission_invalidate`, `t028_disconnect_before_cancel`,
`t028_late_event_after_cancel`,
`t028_between_terminate_and_capacity_release`,
`t028_reaper_capacity_probed`/`t028_reaper_capacity_released`,
`t028_reaper_workspace_before_cleanup`,
`t028_cleanup_before_remove_owned_tree`.

## Tests

The former Python suites covered classification (including the rate-limit/auth-loss/provider-restriction non-eligibility assertions), retry policy, reaper capacity/workspace sweeps (confirmed-only release, bounded/idempotent, user-file non-interference), cancellation binding-invalidation forwarding, the transient-failure retry window (the policy itself, coordinator launch and turn retries with their chat lines, and the worker retry delay with its wait card), scans outside the write lock and the project-change lock, and the sandbox scan card and home-directory refusal. Those suites were removed on 2026-11-16 and their coverage was not re-verified in the Rust suites (`cortex-core` scheduler/retry tests and `workspace-service` tests); treat these statements as carried over.

## Known limitations

No adapter currently implements a confirmed engine-turn pause (only
cancel); `OrchestratingRuntime.cancel`/`disconnect_session` are in-process
only (a session started on a different, now-crashed Cortex process is not
reachable this way — the reaper's cross-process capacity/workspace sweep is
the durable fallback for that case, not a substitute for live
cancellation). Live disconnect/kill-signal behavior against a real engine
is not covered by the automated suite.

Recovery dispatch excludes superseded and terminal delegation requests,
including results bound through the exact provisioning attempt. An old
request must never be provisioned under a replacement coordinator channel:
that changes its idempotency identity and creates an orphan worker. Such a
request records `recovery_worker_lost`; the coordinator can issue a fresh
explicit delegation.


## 5. Reload recovery limits

A plugin disconnect is transport state, not task completion or failure. `host_disconnected` preserves the exact identities that exist, runtime package version, available-history status and any open gateway receipts; recovery consults the persisted task state after restart. An action with an open receipt remains unknown and is not automatically repeated. Paseo renders interrupted worker state through the title suffix because the installed ACP bridge drops `_meta` from `tool_call_update`. Until reload recovery is qualified for the deployed version, apply plugin updates between active tasks. A host `SIGKILL` without `SIGTERM` still bypasses disconnect recording; a later reader then identifies the interrupted worker from its provably gone owning process instead, which needs a readable `/proc` (an unreadable identity leaves the worker's status unchanged). Native engine actions lack the retained gateway receipt that permits exact outcome reconciliation.
