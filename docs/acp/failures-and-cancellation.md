# Failures, cancellation, and disconnects (T028)

Scope: crash/uncertain-outcome handling without duplicate mutation, hidden
paid work, accidental rollback, or cleanup of user resources. Builds on
T014 (permission bridge/binding registry), T017 (capacity), T019 (delivery
ledger), T020 (questions/steering), T023 (process identity/recovery), T025
(isolated workspaces), and T027 (route profile/fallback denylist).

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
worker-prompt-failure path calls `classify()` before ever considering a
T027 fallback attempt; only `engine_failure` is
`is_fallback_eligible()`. `routing.authorization.authorize_fallback`
independently refuses any class outside
`configuration.policy.KNOWN_FALLBACK_ELIGIBLE_CLASSES` (only
`engine_failure`/`capacity_unavailable`), so a rate-limit/auth-loss/
provider-restriction reading of the same failure never reaches route/
consent evaluation at all — two independent gates, not one.

Note: a downstream engine's own transport EOF (`kind="transport_eof"`) is
classified `engine_failure`, not the new `transport_failure` class — this
codebase already raises exactly that kind for "the engine process itself
became unreachable", and T027's existing fallback tests depend on that
being fallback-eligible. `transport_failure` is reserved for ACP
host-connection-level markers (`broken_pipe`, `connection_reset`,
`writer_closed`) this codebase does not currently raise as an
`EngineTurnFailed` kind.

Separately, `engine.failure_classification.route_failure(error)` decides
whether an engine's JSON-RPC error is about the route rather than the work.
It reads only the adapters' own structured `data` fields, never message text:
codex-acp 1.13.0's `codexErrorInfo` (`usageLimitExceeded`, `rateLimitExceeded`)
and claude-agent-acp 0.81.0's `errorKind` (`rate_limit`, `billing_error`,
`account_on_hold`, `model_not_found`). A worker turn refused this way fails
as `EngineTurnFailed("route_unavailable:<reason>")`. That classifies as
`rate_limited` or `provider_restriction`, so it is still never fallback-eligible,
and the route is recorded unavailable for automatic routing (see
[routing and fallback](routing-and-fallback.md#automatic-worker-routing-unavailable-routes)).

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
attempt (T023 lookup_owned/terminate_owned, channel revocation) *before*
opening a replacement session; `recovery.worker_reconciliation.reconcile`
checks for an already-published `worker_result` before writing
`recovery_worker_lost`, re-checked inside the same transaction as that
write so a concurrently-published result always wins;
`delegation.deliveries`/`delegation.provisioning` use idempotency keys for
every durable publication. No blanket exactly-once guarantee is made
anywhere for an external mutation whose fate cannot be observed.

## 3. Cancellation (`session/cancel`)

`session/cancel` interrupts the current turn, revokes current worker channels,
terminates owned sessions and persists RECOVERING. It is distinct from a queued
Paseo steer, which uses a concurrent configuration request and does not call
`session/cancel`. The cancellation path records `TURN_INTERRUPTED`, not
`USER_STEERING`; `turn_interrupted` classifies as `cancellation` even when
an adapter prefixes it with `engine_error`, and a cancelled worker never
enters fallback routing. A later user
prompt can resume planning from durable state. That RECOVERING pass cancels
the same assignments again (reason `replanning`); the work graph keeps an
attempt that is already failed with its first reason and records no
`work_attempt_sync_failed` for the repeat. It does not cancel unrelated tasks.

`/cortex-stop` permanently marks the task CANCELLED before cleanup. It closes
pending questions, fences late publications and cancels capacity-queued opens.
A worker opened concurrently with cancellation is closed before its first prompt.
Reservations release only on observed process absence. Unknown engine liveness
remains a blocker; a terminal event alone never proves process exit. Terminal card
updates are retained and replayed if the client disconnected before seeing them.

## 4. Disconnect

`acp.agent.Agent._handle_eof`/`handle_writer_closed` (host transport EOF or
writer-closed) already invalidate every session's `cancel_event` and close
the connection's `HostRequester` (resolving every pending host round trip —
permission, elicitation, fs — as closed). T028 adds
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
  `cortex-connection:<event id>`, `_meta.cortexConnectionNotice`). It
  states the saved task state (and that a completed result is unchanged),
  the number of interrupted workers and the number of actions with an
  unknown outcome. Loading writes nothing.

Native engine tools are not gated per call, so a native edit or shell
command in flight at the disconnect has no gateway receipt and cannot be
listed as an unknown outcome; recovery still never replays the lost turn.
The `plugin reload` path was exercised in the isolated dev Paseo 0.9.2 on
2026-09-26 (see [the rewrite status](runtime-rewrite-status.md)); until the
deployed package carries it, apply plugin updates between active tasks.

After the agent joins its prompt workers at daemon teardown, it also calls
`ProductionOrchestratingRuntime.shutdown()` (and each cached
`OrchestratingRuntime.shutdown()`). This closes the owned engine processes
and releases confirmed capacity for an ordinary EOF or writer-close. A hard
host kill can still leave an ambiguous registry row; the next
`session/prompt` uses the existing cross-process recovery path, while
`session/load` only applies the read-only card correction described in
`input-history-and-replay.md`.

## 5. Owned-resource reaper

`engine.reaper.sweep(store, engine)` — two bounded, idempotent sweeps,
invoked once per project at first runtime use
(`ProductionOrchestratingRuntime._runtime_for_project`) and after a
disconnect:

- `sweep_capacity`: re-probes every `ambiguous` capacity reservation via
  the caller's `lookup_owned`/`terminate_owned` (T023 process identity —
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

## 6. Fault injection

New `t028_*` checkpoints (`_fault_injection.checkpoint`, same
env-var-gated `os._exit(137)` mechanism as T019/T023):
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
Existing T027 fallback regression tests
(`tests/test_orchestrating_runtime_routing_fallback.py`) continue to pass
unchanged, confirming classification did not alter existing fallback
eligibility for `transport_eof`/`timeout_alive`/`poisoned`.

## Known limitations

No adapter currently implements a confirmed engine-turn pause (only
cancel); `OrchestratingRuntime.cancel`/`disconnect_session` are in-process
only (a session started on a different, now-crashed Cortex process is not
reachable this way — the reaper's cross-process capacity/workspace sweep is
the durable fallback for that case, not a substitute for live
cancellation). Live disconnect/kill-signal behavior against a real engine
is deferred to T035–T037 per the task's own live-verification boundary.

Recovery dispatch excludes superseded and terminal delegation requests,
including results bound through the exact provisioning attempt. An old
request must never be provisioned under a replacement coordinator channel:
that changes its idempotency identity and creates an orphan worker. Such a
request records `recovery_worker_lost`; the coordinator can issue a fresh
explicit delegation.


## 5. Reload recovery limits

A plugin disconnect is transport state, not task completion or failure. `host_disconnected` preserves the exact identities that exist, runtime package version, available-history status and any open gateway receipts; recovery consults the persisted task state after restart. An action with an open receipt remains unknown and is not automatically repeated. Paseo renders interrupted worker state through the title suffix because the installed ACP bridge drops `_meta` from `tool_call_update`. Until reload recovery is qualified for the deployed version, apply plugin updates between active tasks. A host `SIGKILL` without `SIGTERM` still bypasses disconnect recording; a later reader then identifies the interrupted worker from its provably gone owning process instead, which needs a readable `/proc` (an unreadable identity leaves the worker's status unchanged). Native engine actions lack the retained gateway receipt that permits exact outcome reconciliation.
