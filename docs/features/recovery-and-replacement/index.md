# Recovery races and session replacement

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed.


Resuming the same logical task from current authoritative state after a
Cortex process restart, without duplicate work, stale permissions, or two
mutating coordinators live at once.

## Sequence

`acp.orchestrating_runtime.OrchestratingRuntime._ensure_coordinator` used
to fail closed (`coordinator_session_not_cached`) whenever a
`task_orchestration` row already existed for a task but this process held
neither the coordinator channel secret nor an open session for it (almost
always a restart). It now calls `recovery.coordinator_recovery.recover`:

1. **Fencing.** A fresh ownership epoch is acquired
   (`ownership.acquire_task_owner_tx`). Every trusted channel is keyed by
   the task's single current epoch (`server.Server._dispatch_channeled`),
   so this alone would fence out every channel task-wide, including a
   still-legitimate worker's. In the *same* transaction,
   `ownership.reassign_worker_channels_tx` migrates every non-revoked
   **worker** channel forward to the new epoch (never issuing it a new
   secret — a running worker's engine process already has its old secret
   baked into its own environment and cannot be handed a new one without
   itself being replaced). Only the **coordinator** channel is ever
   revoked and reissued.
2. **Old-attempt reconciliation.** The prior coordinator attempt's fate is
   checked via `SessionRuntime.lookup_owned`: `"alive"` is terminated and
   the exit re-verified (not confirmed → refuse the replacement outright,
   `coordinator_recovery_ambiguous` — never two mutating coordinators);
   `"unknown"` refuses immediately (`coordinator_recovery_indeterminate`,
   no replacement, no termination attempted); `"absent"` proceeds. A
   timeout on a call never implies death by itself; only an OS-level
   confirmation does (see "Cross-process liveness" below).
3. **Worker reconciliation** (`recovery.worker_reconciliation.reconcile`):
   for every assignment the task has ever delegated that is not yet
   terminal, durable state is inspected first. A `worker_result` already
   recorded (even one that lands *during* this very recovery, in the
   narrow pre-fencing window — its channel was migrated forward in step 1,
   not fenced out) is delivered by the ordinary continuation loop and
   never re-delegated; its process is opportunistically terminated to free
   capacity, but the assignment is never marked failed. Otherwise the
   attempt is inspected via `lookup_owned` and, since a fresh process holds
   no channel secret to safely resume it, terminated best-effort and
   marked `assignment_failed` with reason `recovery_worker_lost` — an
   input to runtime reconciliation. The runtime can retry a lost node at most
   twice, only after confirmed process absence and with no uncertain action
   receipts. Refusal, authorization loss, provider restrictions and cancellation
   cannot trigger an automatic retry or route bypass.
4. **Fresh coordinator channel + session.** The old coordinator channel is
   revoked; a new one is issued at the new epoch, bound to a freshly
   allocated attempt id (never the old one), and a new engine session is
   opened for it with `cause="session_replacement"` (context
   generations) — never `"session_open"`.
5. **Config/permission carry-over check.** `recovery.registry` records,
   per attempt, the configuration revision and `ResourcePolicy` it was
   actually opened with. Authorization is **field-specific**, never "any
   config change is enough":
   - A route/engine/connection switch is authorized only if (a) the
     config revision actually changed, (b) *that specific* revision's own
     recorded route differs from its immediate predecessor's (an
     effort/parameter-only revision — same route, different unrelated
     field — never authorizes a provider switch merely because *some*
     revision happened to change), and (c) the replacement session's
     actual route equals exactly what that revision recorded (no drift
     beyond what was authorized). Otherwise: `unauthorized_route_change`.
   - A broader `ResourcePolicy` (`fs_local`/`terminal`/
     `output_byte_limit_max`) is authorized only if the new config
     revision's own `parameters.resource_policy_override` explicitly
     declares a policy that covers the broader grant actually used.
     Absent that explicit declaration, broadening is refused
     (`recovery_cannot_broaden_permissions`) even when the config
     revision changed for some other reason.
   Route and policy authorization are checked independently — an
   authorized route change does not itself authorize a policy broadening,
   or vice versa. An authorized route change reports
   `coordinator_recovery_new_engine` (localized), built from
   canonical Cortex state (the recovery capsule), never a provider-native
   resume; the plain-replacement case records
   `coordinator_recovery_new_session` as a task progress event only, without
   a chat message (same task, same chat, rebuilt from recorded state).
   Neither ever says "resumed".

Worker reconciliation's own "already published" check and its
`assignment_failed(recovery_worker_lost)` write are not two independent
reads: the write happens only after a second, transactional re-check for a
`worker_result` immediately beforehand (same `BEGIN IMMEDIATE`), so a
result published concurrently — in the window between the first check and
`terminate_owned`/channel revocation above — always wins and no
contradictory failure is ever durably recorded. As further defense in
depth, `_coalesce` (the continuation-prompt renderer) never shows a
`recovery_worker_lost` failure alongside a `worker_result` for the same
assignment in the same claimed batch — the result always wins there too.

Capsule delivery and the state-delta watermark are not a separate
mechanism: recovery routes through the ordinary continuation loop, which
already claims every unconsumed durable event *before* building/attaching
the recovery capsule for the fresh generation, and re-checks
`is_stopped`/claims immediately before the next `engine.prompt` call — so
steering/publications/cancellations recorded after "the old process died"
are included before the first post-recovery continuation without any
recovery-specific redelivery code, and a cancelled/superseded requirement
revision stays cancelled (it is simply never claimed).

A host transport disconnect before the restart (for example a Paseo plugin
update) leaves task, node and assignment state as persisted rather than
cancelling it; step 3 then records its interrupted workers as
`recovery_worker_lost`, and a restarted runtime never re-dispatches the old
delegation request. Such a worker is retried with the failure class `worker_lost`: the retry still
counts against the retry limit, but it never raises the model tier. See
[failures and disconnects](../failures-and-cancellation/index.md#4-disconnect).

## Cross-process liveness

`delegation.live_adapters.EngineSessionManagerAdapter.lookup_owned`/
`terminate_owned` used to be purely in-memory: a fresh adapter instance
(a restart) always answered `"absent"` regardless of whether the process
was actually still running. `engine.process_identity` now captures, at
launch, the pid plus two additional facts that make pid reuse detectable:
its own start time (`/proc/<pid>/stat` field 22) and the current boot id
(`/proc/sys/kernel/random/boot_id`); `recovery.registry` persists this
triple per attempt (`attempt_process_registry`, additive). A fresh
adapter's `lookup_owned` for an attempt it never itself opened reads this
record back: matching pid+start-time+boot-id identifies the launched engine.
For private coordinator/worker groups, `process_identity.probe_group` also checks
active group members: a gone leader alone is not absence. A reboot, reused
leader identity or observed inactive group establishes absence; unreadable
observations remain unknown. Inactive zombies are not writers and are reaped by
their OS parent. Group-escaped descendants are not covered. Older
`timeout_alive` termination markers are re-probed, never treated as exit proof.
`terminate_owned` for such a cross-process attempt signals the recorded
process group only after positive liveness/identity observation (SIGTERM, grace,
SIGKILL; unknown liveness is not signalled) and only reports a
confirmed exit once the same identity probe reports `"absent"` again —
otherwise ambiguous, exactly like a same-process `EngineProcess`'s own
unconfirmed termination.

## Crash boundaries

`_fault_injection.checkpoint` is called at six points:
`t023_after_epoch_acquire`, `t023_after_old_session_reconciliation`,
`t023_after_channel_revoke_issue`, `t023_after_new_session_open`,
`t023_after_capsule_delivery_transmitted` (shared with the ordinary capsule
delivery path), and `t023_after_continuation_prompt`. Each step is its own
committed SQLite transaction; a crash at any checkpoint leaves the task in
a state a later `recover` call (or, before step 4, the very next
`_ensure_coordinator` call) can resolve without a false delivery/success
receipt or a duplicate coordinator/worker (see
`tests/test_recovery_cross_process.py`).

## Limits

- `EngineSessionManagerAdapter`'s process-identity/effective-context
  recording is best-effort: a manager with no real subprocess (a fake) or
  an unreadable `/proc` records nothing, and a later `lookup_owned` for
  that attempt then correctly answers `"unknown"` rather than a guessed
  `"alive"`/`"absent"`.
- The config/permission carry-over check compares against the *most
  recently recorded* `attempt_effective_context` row for the task/role; a
  store created before this row existed, with no such row for the old attempt, skips the check
  (nothing was ever recorded to compare against) rather than refusing
  every first post-upgrade recovery.
- Live cross-engine qualification (a real second engine actually
  installed/qualified) is not covered by the automated suite.


## Coordinator launch failure diagnostics

The initial coordinator engine launch can fail after task identity is persisted but before the `runtime_tasks` row is created. That failure records a safe structured diagnostic tagged with `engine_launch`, the task/session/attempt identities that exist, and the running runtime package version; the useful reason is included in the ACP error text because hosts may discard JSON-RPC `data`. Retry uses the existing task identity and recovery path, and initial pipeline publication is idempotent across that retry. `scripts/cortex_diagnose.py` reports the installed runtime package version separately from the checkout version. A diagnostic does not imply a live engine child was started successfully.
