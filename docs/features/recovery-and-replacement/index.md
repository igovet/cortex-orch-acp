# Recovery races and session replacement

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed. The former Python runtime was removed on 2026-11-16: references to its modules and tests were dropped, and statements that cited them describe behavior carried from that implementation that has not been re-verified against the Rust runtime.


Resuming the same logical task from current authoritative state after a
Cortex process restart, without duplicate work, stale permissions, or two
mutating coordinators live at once.

Worker-loss reconciliation closes the optional work-graph attempt in the same
transaction as the failure event. Only a missing `work_assignment_attempts`
table is tolerated for older stores. I/O errors, invalid historical payloads
and other SQL faults propagate and abort the transaction rather than commit a
failure event while leaving the graph running.

## Sequence

`_ensure_coordinator` used
to fail closed (`coordinator_session_not_cached`) whenever a
`task_orchestration` row already existed for a task but this process held
neither the coordinator channel secret nor an open session for it (almost
always a restart). It now calls the runtime:

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
3. **Worker reconciliation**:
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
5. **Config/permission carry-over check.** the runtime records,
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
`is_stopped`/claims immediately before the next the runtime call — so
steering/publications/cancellations recorded after "the old process died"
are included before the first post-recovery continuation without any
recovery-specific redelivery code, and a cancelled/superseded requirement
revision stays cancelled (it is simply never claimed).

A host transport disconnect before the restart (for example a Paseo plugin
update) leaves task, node and assignment state as persisted rather than
cancelling it; step 3 then records its interrupted workers as
`recovery_worker_lost`, and a restarted runtime never re-dispatches the old
delegation request. Such a worker is retried with the failure class `worker_lost`. The retry
falls under the transient-failure retry window, not the ordinary retry limit,
and it never raises the model tier. See
[failures and disconnects](../failures-and-cancellation/index.md#4-disconnect).

## Cross-process liveness

`lookup_owned`/
`terminate_owned` used to be purely in-memory: a fresh adapter instance
(a restart) always answered `"absent"` regardless of whether the process
was actually still running. `process_identity` now captures, at
launch, the pid plus two additional facts that make pid reuse detectable:
its own start time (`/proc/<pid>/stat` field 22) and the current boot id
(`/proc/sys/kernel/random/boot_id`); the runtime persists this
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

## Native attempts left unconfirmed by an earlier host

The native host's restart recovery
(`crates/workspace-service/src/native_recovery.rs`) settles an attempt whose
delegation is not `terminated` or whose capacity reservation is not `released`
only from durable evidence, for every task in the store (terminal ones
included, so a closed task can be deleted):

- **Never launched.** The attempt's own `NATIVE_SESSION_START_FAILED` event
  (bound to its exact channel, the only start failure of that channel) shows
  the start stopped before any engine process existed: phase `broker`, or phase
  `launch`/`external_mcp` with `authority_refused`/`invalid_arguments`. The
  session must never have been bound and the channel must have no other
  activity. The current host already records such a failure as a confirmed
  exit; earlier hosts recorded it as unknown. The delegation becomes
  `terminated`, the reservation `released` with observation `never_launched`,
  and one `NATIVE_PROCESS_SETTLED` event records evidence
  `session_start_failed_before_engine_launch`.
- **From an earlier boot.** Recovery stamps each attempt it still cannot
  confirm once per boot (`NATIVE_PROCESS_UNCONFIRMED` with the boot id). An
  attempt is driven only by the service that provisioned it and never
  relaunched, so a recovery in a later boot settles a stamped attempt with
  observation `previous_boot` (evidence
  `unconfirmed_attempt_from_previous_boot`). In the same boot it stays
  fail-closed unless the next rule settles it.
- **Service gone, no engine in the project.** A service that dies (for
  example by SIGKILL) while a session runs leaves that attempt's delegation and
  reservation open, with no process record (the native host tracks its engine
  processes only through these rows). Recovery runs only under the project's
  exclusive scheduler lease and before any session of the project starts, so
  the service that drove the attempt is gone. Recovery then reads the process
  table: when no process of this user other than the service runs an ACP
  engine adapter (`codex-acp`, `claude-agent-acp`, OpenCode), Cortex's Codex
  App Server, or anything handed a session's MCP relay endpoint (a
  `cortex-mcp-` broker directory) with its working directory inside the
  project or a task workspace (every engine session is launched there), no
  process of the attempt can still run. Each such attempt of a task with a
  runtime record is settled: the delegation becomes `terminated`, the
  reservation `released` with observation `no_engine_process`, and one
  `NATIVE_PROCESS_SETTLED` event records evidence
  `service_gone_and_no_engine_process_in_project`. An unfinished task then
  continues as an interrupted turn under the [no-stop policy](../task-lifecycle/index.md#no-stop-policy-and-the-strategy-ladder)
  (`AUTOMATIC_CONTINUATION`, reason `native_service_interrupted` or
  `native_session_outcome_unknown`), relaunched by the runtime itself; the owner's
  message, Stop, Resume, Close and Delete all work. A process whose command line or working directory cannot be read
  counts as such an engine, and so does an unreadable process table; while one
  exists the attempts stay fail-closed. Attempts with a process record are
  decided by the registry rules below.
- **Unconfirmed exit, re-checked by the running service.** When a session's
  shutdown is not confirmed, the running service records its delegation and
  reservation `ambiguous`. It does not wait for a restart: the sprint
  scheduler's passes (its safety tick, and task changes in a project with such
  an exit) re-check an attempt whose every unsettled row is `ambiguous`, that
  has no process record, whose task has a runtime record, and whose
  provisioning row or channel names the task's current owner epoch (none names
  another). The evidence is the same process-table read as above, inside the
  transaction that settles: when no engine process works in the project or a
  task's root, the attempt is settled with the same writes (observation
  `no_engine_process`) and one `NATIVE_PROCESS_SETTLED` event with evidence
  `unconfirmed_exit_and_no_engine_process_in_project`. Nothing is signalled,
  killed or relaunched. While a candidate process exists nothing changes and
  the re-check backs off (15 seconds, doubling to 10 minutes; a changed set of
  such attempts is checked at once). A settled task no driver runs continues, in every run mode: a
  continuation that waited for that exit resumes, and one held only by that exit
  (stopped with no recorded reason, or held for an unconfirmed process exit)
  records an `AUTOMATIC_CONTINUATION` with reason `native_process_exit_settled`.
  Effects with an unknown outcome are then re-verified, never replayed.
- **Never started by the retired Python host.** A `failed` or `in_progress`
  worker row with no engine session and no process record, whose capacity that
  host had already released (or, for `failed`, never reserved), is settled
  with evidence `no_engine_session_or_process_and_capacity_released`. The
  native host never leaves such a row: it releases a reservation and
  terminates its delegation in one transaction and never writes `failed`.
- **Registered processes ended.** A delegation whose every registered process
  (`attempt_process_registry`) has an end time is settled with evidence
  `registered_processes_terminated_or_previous_boot`. Recovery first gives
  the retired host's records their missing end time: a record from an earlier
  boot gets `termination_reason` `previous_boot`; one from the current boot gets
  `process_gone` only when neither its exact process (pid with the recorded
  start time) nor any member of its process group still exists. Deletion
  likewise ignores a record without an end time from an earlier boot.
- **Command receipts whose call was lost.** Every `command_run` and
  `outside_command` receipt ends settled. A brokered launch that fails, or
  whose call ends (a refused or ended reviewer, a revoked channel, a stopped
  session), before its supervisor receives the final launch grant is settled
  at once as `command_not_started` (`launched: false`, with a fixed-code `cause`
  such as `sandbox_unavailable:setup_timeout` or `store_busy`, see
  [permissions and containment](../permissions-and-containment/index.md)): the payload is released
  only after the supervisor's handle commits. A receipt left unsettled any
  other way is settled by startup recovery and by the sprint scheduler's live
  passes (same 15-second to 10-minute backoff as the exit re-check), but only
  on proof that nothing can still run it: the supervisor's `finished` status
  (its recorded receipt, or `outcome_not_recorded` with the exit code when the
  output cannot be verified), an earlier boot, a lost supervisor whose payload
  process group (recorded in its status after launch) has no member, a lost
  read-only supervisor (its bubblewrap runs with `--die-with-parent
  --unshare-pid`, so the namespace ends with it), or, for an unsupervised
  outside command, neither its exact process nor any member of its process
  group. An outside command receipt changes only once its channel can no
  longer act. Commands the service still runs or observes are never touched.
  Such a receipt is `settled: true`, `complete: false` with `outcome:
  "unknown"` and its `settlement_evidence`: it never counts as a passed check,
  the request is never run again, and close-as-is, repair and resume no longer
  refuse with `native_outside_outcome_unknown`. A live supervisor, or a lost
  writing supervisor without a recorded process group, keeps the receipt
  unknown; so does a Guarded or Development payload that outlives its lost
  supervisor (it has no PID namespace), until its process group is empty.
- **Command starts of earlier hosts.** Earlier hosts recorded a command as a
  start event (`COMMAND_PROCESS_STARTED`) and an exit event; a service that
  died between them left the start without its exit, and recovery holds the
  task (`legacy_effect_outcome_unknown`). Recovery records the missing exit
  once (`COMMAND_PROCESS_EXITED` with `recovered: true` and its `evidence`)
  when the command's own receipt (same task, channel and call reference) is
  complete (`receipt_complete`, with that receipt's exit code) or settled
  (`receipt_settled`), or when the runtime recorded an event in another boot
  after the start (`previous_boot`: outcome `outcome_not_recorded`, no exit
  code). A start without a call reference or such evidence stays unknown. A
  task held for such starts then continues with what its facts still show as
  the continuation's reason (for example an outside write with an unknown
  outcome, `native_outside_outcome_unknown`, or otherwise
  `native_session_outcome_unknown`), with one `NATIVE_RECOVERY_REQUIRED` event
  (`continued: true`, naming `settled_barrier` when the start was settled), and
  later restarts record nothing new.

A dead service never removes its sessions' broker directories
(`cortex-mcp-` and 24 hex digits, in the project's store directory). Recovery
removes one that no socket of the system is bound in (as `/proc/net/unix`
lists) once it has not changed for a minute; nothing is removed when the socket
table cannot be read.

A task the retired host ran before runtime records existed (rows in
`delegation_provisioning`, `capacity_reservations` or the process registry,
but no `runtime_tasks` row) is settled by the same rules without a runtime
event, since runtime events reference the runtime record; the settled rows
keep the evidence. Such a task is never stamped, so an attempt of it that no
rule settles stays unsettled. Each task's recovery runs in its own savepoint:
a task whose recovery fails (for example a damaged state) is rolled back
alone and continues as a `native_recovery_failed` continuation when its store
still accepts one (otherwise it is held with that blocker), and the project
still opens. Recovery is idempotent: a task already
held for the same reason, or a worker wait already marked lost, records
nothing again on the next restart.

A finished desktop task the retired host ran is adopted too (`NATIVE_TASK_ADOPTED`
with `finished: true`, nothing replayed): a new owner epoch fences the old host
and its execution follows, so Request changes, messages and Close work on it.
Its result reads as a read-only result from an earlier version of Cortex (never
acceptable here; Request changes reopens it for a fresh result).

Any other unconfirmed attempt (for example a failure after the engine
launch) keeps Resume, owner messages, Retry and deletion refused with
`native_shutdown_unconfirmed`/`active_execution`. **Close task** still records
the owner's cancellation and is reported done (receipt `processesUnconfirmed`,
`OWNER_TASK_RESOLVED` with `processes_unconfirmed`), so the window closes the
card; deletion stays refused until the attempt is settled, and restarting the
computer lets the next recovery settle it.
Once settled, the runtime continues the task by itself (an owner message waits
for that boundary), an answered stop question an earlier version stored
relaunches it, and a closed task can be deleted.

## Crash boundaries

`_fault_injection.checkpoint` is called at six points:
`t023_after_epoch_acquire`, `t023_after_old_session_reconciliation`,
`t023_after_channel_revoke_issue`, `t023_after_new_session_open`,
`t023_after_capsule_delivery_transmitted` (shared with the ordinary capsule
delivery path), and `t023_after_continuation_prompt`. Each step is its own
committed SQLite transaction; a crash at any checkpoint leaves the task in
a state a later `recover` call (or, before step 4, the very next
`_ensure_coordinator` call) can resolve without a false delivery/success
receipt or a duplicate coordinator/worker.

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

The current Rust launch diagnostics and the conditions for a rapid retry are
described under [native session start failures](#native-session-start-failures).
A final coordinator start failure remains part of ordinary task recovery; the
[owner wait for repeated turn interruptions](../failures-and-cancellation/index.md#transient-engine-failures)
applies only after a coordinator turn has been dispatched.

## Native session start failures

A native coordinator or worker session starts in phases: `broker`,
`external_mcp`, `launch`, `broker_bind`, `initialize`, `session_new`,
`configure` (the route's model and parameters), `access_mode` (the channel's
mode), then `bind_session`, which binds the engine session to its channel. The
session is bound only after it is configured, so no MCP call (tools need a
bound session) can run before that point.

If the ACP transport is lost (`transport_closed_outcome_unknown`, or a failed
write, `io_error`) in `configure` or `access_mode` and the engine process, its
broker and its external relays are confirmed to have exited, the host starts the
session once more under the same channel and capacity reservation, with a
fresh process, broker and relays. The first attempt is recorded as
`NATIVE_SESSION_START_RETRIED` (attempt 1, no blocker). Any other failure, an
unconfirmed exit or a failure of the second attempt is final: it is recorded as
`NATIVE_SESSION_START_FAILED` with its attempt number, phase, error, role and
blocker, the reservation is settled, the channel is revoked and the task stops
with the blocker as its reason: `coordinator_session_start_failed` for a
coordinator, `native_session_start_failed` for a worker. Each recorded attempt
whose engine process ran carries the process's exit status, the transport's
close reason and its stderr size, and names a private, redacted stderr tail file
(see `SECURITY.md`). A channel with a retried attempt is never settled as
"never launched" by restart recovery, because an engine process ran.

The host registers a new session from the engine's `session/new` reply before
that reply reaches the waiting caller. claude-agent-acp 0.107.0 sends the new
session's `available_commands_update` about 3 ms after the reply (measured
offline); when the caller resumed later than that, the host read an update for a
session it did not know yet, closed the transport, and the next `configure`
call failed with `transport_closed_outcome_unknown`.
