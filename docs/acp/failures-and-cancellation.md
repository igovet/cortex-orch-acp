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

`OrchestratingRuntime.cancel(session_id)` sets this task's `cancel_event`
(the primary path the continuation loop observes between steps) **and**
invalidates the coordinator engine session's `BindingRegistry` entry (via
`SessionRuntime.invalidate_session`, forwarded by
`delegation.live_adapters.EngineSessionManagerAdapter`) so a permission
request already blocked in a host round trip
(`engine.permissions.PermissionBridge`) is answered `cancelled` — including
a reply that arrives *after* this call (`ResourceBinding.is_current`
re-checks at relay time). Scope: this task's own coordinator session only;
an independent task's authorized work is never touched (scope isolation).
Task-wide cancellation (`/cortex-stop`, `OrchestratingRuntime.stop_task`)
cancels every non-terminal assignment, invalidates every open pending
question, cancels capacity-queued opens, and marks steering superseded.
It also terminates every worker attempt durably bound to that task: a
terminal assignment event can precede process exit, so event state alone
does not release engine ownership. A worker open completing concurrently
with stop closes its exact new attempt before prompting. Other tasks'
attempts are outside this cleanup. A late
success arriving after cancel is durably recorded but never revives the
turn (`acp.agent.Agent._run_prompt_body` always forces
`StopReason.CANCELLED` once `cancel_event` is set, regardless of what the
runtime itself returns).

Paseo's Workers control uses an explicit `session/cancel` notification as a
task-wide stop. The ACP agent routes that notification to the task's owning
runtime, which performs the same owned-resource cancellation as
`/cortex-stop` without attempting to write a response to the notification
transport. Other ACP hosts use `session/cancel` for the current coordinator
turn/session only; it does not cancel that task's other worker assignments.
Each `assignment_cancelled` event also closes its linked work-graph attempt, so
a worker cannot remain durably `running` after its engine attempt has been
terminated. If the transport is already gone, the terminal card update is
retained in the host transcript and `session/load` replays the durable terminal
status for the existing card.

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
