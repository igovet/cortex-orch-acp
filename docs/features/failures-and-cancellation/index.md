# Failures, cancellation, and disconnects

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed.


Scope: crash/uncertain-outcome handling without duplicate mutation, hidden
paid work, accidental rollback, or cleanup of user resources. It covers the
permission bridge and binding registry, capacity, delivery ledger, questions and steering, process identity and recovery, isolated workspaces, and the route-profile fallback denylist.

## 1. Failure classification

`engine.failure_classification.classify(kind, message=None, exit_code=None,
signal=None)` maps an observed `EngineTurnFailed.kind`/JSON-RPC error text
into one of eight closed classes: `engine_failure`, `transport_failure`,
`rate_limited`, `authorization_loss`, `provider_restriction`, `refusal`,
`cancellation`, `indeterminate`. Matching is bounded substring search over a
short, already-captured string — never a scan of raw engine stdout/stderr,
never a regex with backtracking risk. Unknown/ambiguous input is always
`indeterminate`, never guessed as `engine_failure`.

`acp.orchestrating_runtime.OrchestratingRuntime._process_new_events`'s
worker-prompt-failure path calls `classify()` before considering a
fallback attempt; only `engine_failure` is
`is_fallback_eligible()`. `routing.authorization.authorize_fallback`
independently refuses any class outside
`configuration.policy.KNOWN_FALLBACK_ELIGIBLE_CLASSES` (only
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
nonzero on its own, `engine.broker.McpBroker.readiness` reports
`mcp_server_crashed`. The running prompt turn is cancelled and fails as
`cortex_mcp_connection_failed:mcp_server_crashed`. That kind classifies as
`indeterminate`, so it is never fallback-eligible. It is recorded as
`assignment_failed` only when the assignment has no `worker_result` or
`worker_question`, and it never gets the same-session completion follow-up,
which could only ask for publication through the lost tools. The runtime's
bounded retry (`orchestration.recovery.retry_lost`) then opens a fresh attempt,
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

The MCP server distinguishes a typed portable failure from an unexpected storage
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

The no-paid scenario `tests/test_plan_error_classification.py` exercises real
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
opening a replacement session; `recovery.worker_reconciliation.reconcile`
checks for an already-published `worker_result` before writing
`recovery_worker_lost`, re-checked inside the same transaction as that
write so a concurrently-published result always wins;
`delegation.deliveries`/`delegation.provisioning` use idempotency keys for
every durable publication. No blanket exactly-once guarantee is made
anywhere for an external mutation whose fate cannot be observed.

### Transient engine failures

An engine that fails for an infrastructure reason is retried automatically,
and the chat shows each retry as it happens.
`engine.retry_policy.windowed_retry` sets the timing. The first retry waits
5 s, and each later wait doubles, up to 60 s. The work stops only after
failures have kept repeating for 10 minutes (`RETRY_WINDOW_SECONDS`). A
failure more than 10 minutes after the previous one starts a new streak.

- **Coordinator launch.** If the coordinator engine fails to start (stage
  `engine_launch`), `OrchestratingRuntime._ensure_coordinator_retrying`
  starts it again. The chat shows `⟳ The engine did not start (<cause>).
  Retry N in X s…`. A later success shows `✓ The engine started after N
  retries.`, and giving up shows `✗ The engine kept failing for N min …
  Stopping.` before the usual `engine_launch` error. Each new start goes
  through coordinator recovery, the same path a user's resend takes. Some
  failures are never retried, because starting again cannot help:
  - a structured route refusal (exhausted plan, rate limit, unavailable
    model);
  - a provider restriction, refusal or cancellation;
  - a rejected configuration;
  - a validation or authorization `PortableError`, such as a missing launch
    command.
- **Coordinator turn.** A coordinator turn can fail for an engine or
  transport reason (a dead session, `engine_error`) before it streams text
  or calls a tool. The controller then resends the same prompt, after
  reopening the session if it died, and the chat shows `⟳ The coordinator's
  engine failed before answering …`. A turn that already streamed or called
  a tool may have acted, so it is not resent. It blocks as before.
- **Workers.** In `orchestration.recovery.retry_lost`, an assignment that
  failed with class `engine_failure`, `transport_failure` or `worker_lost`
  is retried within the same window. This includes a worker engine that
  would not start and recorded no route failure. These retries do not use
  up `MAX_ATTEMPT_RETRIES`. The `ASSIGNMENT_RETRIED` event records
  `failed_at`, `retry_not_before` and `infrastructure_retry`.
  `TaskMachine._ready_nodes` holds the node until its delay ends, with the
  scheduler wait reason `retry_backoff`. The hold lives in the runtime
  process, so an idle scheduler pass costs no extra read; a restarted
  runtime retries the node at once. The chat shows that wait as a card
  titled `Retry N: <node> — the worker's engine failed`.

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

### Opt-in native CLI login keepalive (outside the runtime)

`scripts/cortex_auth_keepalive.py` is a standalone operational script, not
runtime behavior: the Cortex runtime never invokes it, and installing its
optional `systemd --user` timer (`scripts/systemd/`) is a manual owner
choice this repository never performs automatically. Run manually or on a
timer, it checks, per engine, at most one zero-cost refresh-forcing command
a prior investigation proved does not make a paid call; if no such command
is proven for an engine it reports that engine `unsupported` rather than
guessing one or falling back to a real model prompt. As of this writing no
zero-cost refresh-forcing command is proven for either the installed
Claude Code CLI or the installed Codex CLI (installed-CLI `--help` text
describes status/login display only, never a proven refresh or
persistence), so both engines currently report `unsupported`.

It never reads, copies or prints a credential/token value, and never logs
or persists a command's stdout/stderr (only the bare exit code is
observed): a sanitized `engine`/`status` (`ok`/`needs-login`/`unsupported`/
`error`)/`exit_code`/`detail` JSON line is all it prints. The process exits
non-zero exactly when a configured engine reports `needs-login` or `error`,
so a `systemd` timer surfaces a real re-login need in `journalctl` without
ever attempting one itself. See `scripts/systemd/README.md` for the manual
install/uninstall steps and `tests/test_auth_keepalive.py` for its
mocked-subprocess coverage.

### Project scans outside the store's write lock

Some write transactions decide on an observed project revision
(`delegation.revision.capture_revision`): a review result binding, completion
and answer checks, verification credit, synthesis, the execution baseline and
the review waiver. Each one used to scan the project inside `BEGIN IMMEDIATE`;
synthesis scanned at least four times. SQLite's write lock covers the whole
database, so every other writer, including another chat's runtime on the same
project, failed with `database is locked` once its 15 s busy timeout ran out.
`TaskMachine` and `synthesis.complete` now scan once before the transaction
(`orchestration.state.observed_project`,
`delegation.revision.observation_snapshot`), and every capture of that root
inside the transaction returns that observation. The scan holds the
project-change lock (`.cortex/.project-mutation.lock`, shared) until the
transaction ends. A gateway file effect (`project_write`, `project_edit`,
`project_mkdir`, `project_delete`, `project_skill_link`) holds that lock
exclusively, so such an effect still cannot land between the scan and the
decision. Native engine tools were never excluded by either lock.

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
the same assignments again (reason `replanning`); the work graph keeps an
attempt that is already failed with its first reason and records no
`work_attempt_sync_failed` for the repeat. It does not cancel unrelated tasks.

`/cortex-stop` permanently marks the task CANCELLED before cleanup. It closes
pending questions, fences late publications and cancels capacity-queued opens.
A captured `/cortex-finish` command or explicit owner answer to close the task
as-is uses the same cancellation cleanup, but also records `USER_FORCE_FINISHED`
with unfinished work. It is terminal `CANCELLED`, never verified `COMPLETED`.
A worker opened concurrently with cancellation is closed before its first prompt.
Reservations release only on observed process absence. Unknown engine liveness
remains a blocker; a terminal event alone never proves process exit. Terminal card
updates are retained and replayed if the client disconnected before seeing them.

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
disconnect callback finished. On the stdin-EOF path, `acp.agent.main` joins
the reader before joining prompt workers and shutting down the runtime.
This preserves recording/cancellation before process exit even when the EOF
callback is still running. The writer-closed event likewise precedes its
disconnect callback; on that path, `main` joins the writer before joining
prompt workers and shutting down the runtime. SIGTERM runs its disconnect
callback synchronously on the main thread. SIGTERM and writer-close must not
join a reader blocked on still-open stdin.

`acp.agent.Agent._handle_eof`/`handle_writer_closed` (host transport EOF or
writer-closed) already invalidate every session's `cancel_event` and close
the connection's `HostRequester` (resolving every pending host round trip —
permission, elicitation, fs — as closed). The runtime adds
`OrchestratingRuntime.disconnect_session(session_id)` (reached via
`ProductionOrchestratingRuntime.disconnect`): invalidates the coordinator's
own permission binding the same way `cancel` does, records a durable
`progress` event naming what was paused/cancelled/unconfirmed, then runs
the bounded owned-resource reaper (`engine.reaper.sweep`). No adapter
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
- The durable record is a `progress` event of kind `host_disconnected`
  (`recovery.worker_reconciliation.record_host_disconnect`) with the exact `session_id`,
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
  live or unidentifiable owner never qualifies, and the read writes nothing
  (`recovery.worker_reconciliation.interrupted_attempts`).
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
  gateway receipt. A node that does is returned to the coordinator with
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

`tests/test_t028_failures_cancellation_disconnects.py` covers
classification (including the rate-limit/auth-loss/provider-restriction
non-eligibility assertions), retry policy, reaper capacity/workspace
sweeps (confirmed-only release, bounded/idempotent, user-file
non-interference), and cancellation binding-invalidation forwarding.
Existing fallback regression tests
(`tests/test_orchestrating_runtime_routing_fallback.py`) continue to pass
unchanged, confirming classification did not alter existing fallback
eligibility for `transport_eof`/`timeout_alive`/`poisoned`.
`tests/test_engine_retry_window.py` covers the transient-failure retry
window: the policy itself, coordinator launch and turn retries with their
chat lines, and the worker retry delay with its wait card.
`tests/test_observation_outside_write_lock.py` covers scans outside the write
lock and the project-change lock; `tests/test_sandbox_scan_visibility.py`
covers the sandbox scan card and the home-directory refusal.

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
