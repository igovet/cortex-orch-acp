# Completion, waiting, and resource lifecycle (T017)

Extends [orchestration.md](orchestration.md) (T016 process boundary, event
types, provisioning) with how a coordination turn actually ends, when a
task-level accept is allowed to mean host success, how "claimed" evidence
differs from "observed"/"stored" evidence, what a waiting task is durably
waiting for, and how engine-process concurrency is accounted for.

## Turn-outcome table

| Outcome (coordinator turn) | Runtime effect |
| --- | --- |
| `end_turn`, no task-level accept | keep waiting or continue the loop (see below) |
| `end_turn` + task-level `decide(accept)` that passes the completion gate | `state=accepted`, `StopReason.END_TURN` (host success) |
| `end_turn` + `decide(ask_user)` | `state=waiting_for_user`, `wait_reason=user_input_required` |
| `max_tokens` / `max_turn_requests` | recorded as a `progress` event, counts toward the budget; loop continues only if a meaningful event or a running/pending assignment exists, else `state=blocked`, `wait_reason=limit:<kind>`, `END_TURN` (never success) |
| `refusal` | `state=refused`, `wait_reason=refused`, `StopReason.REFUSAL` returned to the host -- never re-routed to a second engine/connection/model |
| `cancelled` (cancel_event) | `state=cancelled`, `StopReason.CANCELLED`; already-published reports/results are untouched (cancellation is not rollback) |
| engine failure (`EngineTurnFailed`: `transport_eof`, `timeout_alive`, `poisoned`, ...) | claimed events released back to unconsumed (delivered exactly once later); `state=blocked`, `wait_reason=engine_failure:<kind>`; never silently treated as `end_turn` |
| repeated no-progress continuations | `state=blocked`, `wait_reason=blocked`, durable `no_progress_count`, `END_TURN` |

Worker turns mirror the failure/refusal handling: a worker `EngineTurnFailed`
or `refusal` durably records `assignment_failed` (payload `kind`) -- never a
silently dropped assignment, never a reroute. A worker `max_tokens`/
`max_turn_requests` is recorded as an informative `progress` event (the
worker may still ask a question or publish a partial result later).

`engine.session_manager`'s `EngineEventKind.ERROR` (`transport_eof`,
`timeout_alive`) and a poisoned-session `engine_session_unusable` error are
now surfaced from `delegation.live_adapters.EngineSessionManagerAdapter.prompt`
as `acp.orchestrating_runtime.EngineTurnFailed` instead of being silently
folded into a normal `end_turn` (a real, previously-unhandled gap fixed by
this task).

## Completion gating (host success)

`delegation.completion.gate(store, task_id)` is the single source of truth,
checked by both `delegation.mcp_tools._coordinator_decide` (refuses to even
*record* an ungrounded `decide(assignment_ref="task", decision="accept")`
with `StoreError("acceptance_refused", ...)`) and the runtime's own
`_check_terminal_decision` (defensive recheck before honoring an
already-recorded decision). It requires, purely from the durable
`task_events` log and the `reports` table:

1. Every distinct delegated `assignment_ref` (non-superseded
   `delegation_requested`) is terminal: a retained (non-superseded)
   `worker_result` -- the task-level accept "covers every retained
   assignment result" per its own tool description, no separate
   per-assignment accept step is required -- an `assignment_failed`, or an
   `assignment_cancelled`.
2. No unconsumed *meaningful* event remains (see
   `delegation.events.MEANINGFUL_EVENT_TYPES`).
3. Every `worker_result`-terminal assignment's `report_ref` resolves to a
   `reports` row for this task **and** the report file actually exists on
   disk -- a claimed success with no stored evidence never gates completion.

Coordinator-facing events (`delegation_requested`, `assignment_failed`,
`assignment_cancelled`, ...) key `assignment_ref` by the delegate-time
idempotency hash; worker-facing events (`worker_result`, `worker_question`)
key it by the provisioned `attempt_id` instead (`delegation.provisioning`
issues a fresh id, never the hash) -- `gate()` bridges the two via
`delegation_provisioning`.

## Evidence: claimed vs. observed vs. contradicted

`delegation.contracts.WORKER_OPERATIONS["publish_result"]` gains an
optional, bounded `checks: [{name, outcome: passed|failed|not_run,
tool_call_ref?}]` (max 20 items). `delegation.evidence_store` adds a
**separate new table**, `tool_result_observations` (its own optional
schema group, additive on top of `TASK_EVENTS_SCHEMA` -- never an
`ALTER`/rebuild of `task_events`'s fixed `event_type` CHECK constraint,
which already existed at d301299 and cannot be widened by `ADD COLUMN`).
`delegation.live_adapters.EngineSessionManagerAdapter.prompt` records one
row per observed `EngineEventKind.TOOL_CALL_OBSERVED` update (bounded
`tool_call_ref`/`kind`/`title` (<=200 chars)/`status`; never raw tool
arguments or output) for the prompting session's own attempt.

`delegation.evidence.resolve_check` labels each `publish_result` check,
stored in the durable `worker_result` event's payload and shown in the
coordinator's continuation text (`_coalesce` in
`acp.orchestrating_runtime`) as `name=outcome (label)`:

- **claimed** -- no `tool_call_ref`, or one that never resolves to an
  observation *for this exact attempt* (a `tool_call_ref` that only
  resolves under a *different* attempt is never trusted cross-attempt --
  indistinguishable from no match).
- **observed** -- the linked `tool_call_ref`'s latest observation for this
  attempt has `status == "completed"`.
- **contradicted** -- the linked observation's latest status is `"failed"`.

A claim is never itself treated as confirmed; only the `observed`/
`contradicted` label (grounded in a durable observation row) does.

## Waiting

`task_orchestration` persists `wait_reason` (`workers_running`,
`user_input_required`, `blocked`, `limit:<kind>`, `refused`,
`engine_failure:<kind>`, `idle_awaiting_user`) and `wait_detail_json`
(assignment refs / question / a concrete explanation) alongside the
existing bounded, no-model-prompt DB-polling wait
(`_wait_for_progress`/`_wait_timeout`). `no_progress_count` is durable and
reset at the start of every fresh `start_prompt` call (a new user prompt).

On the next `start_prompt` for a task left `blocked`/limited/refused, the
persisted explanation is delivered as part of the first coalesced
continuation (a `progress` event, `resume_context:...`) instead of being
silently dropped -- the coordinator sees why it stopped.

## Crash between claim and delivery

`task_events.delivered_at` is set by `delegation.events.mark_delivered`
only after `engine.prompt` for the claiming continuation actually returns.
`OrchestratingRuntime._ensure_coordinator` calls
`delegation.events.reconcile_undelivered_claims` once per task on this
runtime instance's first use of it, releasing any claim
(`consumed_by` set, `delivered_at` still NULL) left behind by a crash
between `claim_unconsumed` and delivery -- "delivered" is documented as
at-least-once to the model, never assumed.

A pre-T017 store (created by d301299/T016) gains `delivered_at` and the
`task_orchestration` wait columns via `ALTER TABLE ... ADD COLUMN` the next
time `store.Store` opens it (`delegation.events_store.ensure_task_lifecycle_columns`);
no data loss, no `user_version` bump (additive optional-group pattern).

## Capacity

`delegation.capacity`/`capacity_store` add an optional `capacity_reservations`
table (one row per owned engine process; wired through
`store.py`/`split.py`/`migrate.py`/`cleanup.py` the same way
`TASK_EVENTS_SCHEMA` was for T016). `reserve()` is atomic
(`BEGIN IMMEDIATE`, counts `reserved`+`ambiguous` against a connection's
limit) and refuses over-subscription; `release_confirmed()` only fires on a
positively observed process exit, `mark_ambiguous()` on an unconfirmed
termination (still counts against the limit; never auto-released -- a
bounded reaper that force-resolves a long-ambiguous reservation is T028).
Release never deletes history (`task_events`/`reports` stay readable).

`delegation.live_adapters.EngineSessionManagerAdapter` accepts an optional
`store`/`capacity_limit`. When given, `open()` **blocks** (bounded backoff
poll, 0.05s up to 0.5s, recording one deduped `capacity_queued:<attempt_id>`
`progress` event and never raising/failing) until a reservation is free --
deliberately *inside* `open()`, below
`delegation.provisioning.ProvisioningService.delegate`'s idempotency-key
claim/steal protocol (`_acquire_step`), so a queued delegation is
indistinguishable from "the engine is just taking a while to launch" and
never fights that protocol's own contention recovery (retrying `delegate()`
itself after a mid-step exception was evaluated and rejected as unsafe --
see `OrchestratingRuntime._provision_and_prompt`'s docstring). No model
prompt happens while queued (queuing is entirely inside worker-thread
`open()`/coordinator-provisioning calls, never the coordinator's own
`engine.prompt`).

`connections.model.ConnectionRecord` gains an optional, validated,
backward-compatible `max_concurrent_sessions: int = 4` field (1-64);
`connections.registry._record_from_dict` tolerates its absence in an
already-persisted pre-T017 `connections.json` record (defaults to 4).

`OrchestratingRuntime._terminate_completed_workers` (called from
`_process_new_events`, the coordinator's own event-processing step) shuts
down a worker's owned process once its assignment reaches a terminal state
(a retained `worker_result` or an `assignment_failed`) -- exactly once per
attempt (`self._terminated_attempts`) -- so capacity is only reused once
that shutdown reaches a confirmed (or ambiguous) release.

## What's deferred

- T028: reaper/force-resolution of long-`ambiguous` capacity reservations;
  richer failure classification.
- T020: steering (mid-turn worker redirection beyond `clarify`).
- T021: capsule/workspace isolation affecting cancellation semantics.
- T023: replacement fencing for a superseded/replaced coordinator.
- Coordinator secret recovery after a genuine process restart
  (`coordinator_session_not_cached` still fails closed; only the
  claim/delivery reconciliation within a *live* runtime instance's first
  use of a task is implemented by this task).
