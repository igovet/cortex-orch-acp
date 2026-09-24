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
prompt can resume planning from durable state. It does not cancel unrelated tasks.

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
