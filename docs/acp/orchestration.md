# Model-owned coordination and managed workers (T016)

This covers `acp/orchestrating_runtime.py` (`OrchestratingRuntime`, an
`acp.runtime.IncomingRuntime`), the `task_events`/`task_orchestration`
schema group (`delegation/events_store.py`, `delegation/events.py`), and the
delegation MCP tool handler layer (`delegation/mcp_tools.py`,
`delegation/contracts.py`'s new `decide` operation).

## Process boundary

Two separate processes, never one pass-through proxy:

- **The engine's own Cortex MCP subprocess** (`cortex.py` → `server.py` →
  `delegation/mcp_tools.py`) is what actually performs a delegation
  operation call: it validates the caller's channel role
  (`delegation.contracts.schemas_for(role_kind)`), writes exactly the
  durable `task_events` row(s) the operation names in one transaction, and
  returns. It never launches an engine and never calls
  `delegation.provisioning.ProvisioningService` itself.
- **The main Cortex process** (`acp.orchestrating_runtime.OrchestratingRuntime`)
  owns every engine session (coordinator and worker) and reacts to the
  `task_events` log a subprocess call produced: it calls
  `ProvisioningService.delegate` for a new `delegation_requested` event
  (in a bounded worker pool), prompts the resulting worker engine session,
  and streams only the coordinator's own visible text via `emit_update`.

## Event types and states

`task_events.event_type` is a closed set: `delegation_requested`,
`assignment_ready`, `assignment_failed`, `assignment_cancel_requested`,
`assignment_cancelled`, `worker_result`, `worker_question`,
`coordinator_clarification`, `coordinator_decision`, `user_message`,
`progress`. Rows are deduplicated by an explicit `dedup_key` identity
(request key / report ref / origin event id), never by comparing payload
text. `delegation.events.MEANINGFUL_EVENT_TYPES` (`worker_result`,
`worker_question`, `assignment_failed`, `assignment_cancelled`,
`user_message`) is what gates a coordinator continuation; every other
claimed event is still coalesced into the continuation prompt for
auditability but never by itself wakes the coordinator.

`task_orchestration.state` (one row per task): `provisioning` → `running` →
one of `accepted`, `waiting_for_user`, `blocked`, `idle_awaiting_user`,
`cancelled`.

## Claim/release semantics and the crash window

`delegation.events.claim_unconsumed` atomically marks a batch of
not-yet-consumed, not-superseded rows as consumed by one continuation id,
in the same transaction that reads them. If the subsequent
`engine.prompt()` call then raises (the coordinator's engine session
failed to actually receive/process that turn), `OrchestratingRuntime`
calls `delegation.events.release_claim(task_id, continuation_id)` before
propagating the exception: every row still marked `consumed_by` =
that continuation id (never a row a later claim already reassigned, never
a superseded row) is set back to unconsumed, so the next continuation
re-claims and delivers it exactly once instead of losing it silently. This
covers every in-process exception from `engine.prompt()`, including a
mid-call cancellation the adapter surfaces as a raise.

**Remaining crash window (documented, not solved here):** if this process
crashes *after* `claim_unconsumed` commits but *before* `engine.prompt()`
either returns or raises (for example the whole Cortex process is killed
mid-call), the claimed rows stay marked `consumed_by` by a continuation id
whose in-memory turn no longer exists, and nothing in this task
automatically releases them on restart -- they are not lost from the
durable log (still readable via `list_events`), but they will not
contribute to a future continuation prompt in that state. A resumable
completion/lifecycle owner (T017) is the natural place to add a
startup-time reconciliation pass (for example: release any claim older
than a configured age with no other evidence the turn completed); T016
does not implement that reconciliation.

## Waiting without paid polling

Once a coordinator turn ends with no unconsumed meaningful event, the
runtime blocks on an in-process `threading.Condition` (notified when a
worker's prompt call finishes or an assignment is cancelled) plus a bounded
local-DB backoff poll (50ms→1s) for events a *different* process (a
worker's own MCP subprocess) wrote. It never re-prompts the coordinator
model just to check for progress. A configurable no-new-meaningful-event
budget (default 3) ends the turn `blocked` instead of waiting forever.

## Acceptance is explicit, never inferred

`decide(assignment_ref, decision, rationale[, question])` is the only way
overall/assignment acceptance is recorded. `decision="accept"` with
`assignment_ref="task"` is the overall accept the runtime maps to
`state=accepted`; a decision assignment `accept` for a single ref still
requires a retained (non-superseded) `worker_result` event for that ref, or
`mcp_tools.dispatch` raises `no_result_to_accept`. `decision="ask_user"`
requires a `question` and ends the turn in `waiting_for_user`; the next
`start_prompt` for the same task resumes the same coordinator session. A
long, confident-sounding final coordinator message never by itself changes
state.

## Qualification refusal

Both the coordinator (`orchestrator` role) and every delegated worker role
go through `portable.instruction_delivery.qualify_delivery`; an UNQUALIFIED
route is refused with `instruction_delivery_unqualified` before any
provisioning side effect, unless the runtime was constructed with
`allow_unqualified_delivery=True` (test/opt-in only).

## Workspace isolation: deferred to T025

`delegation.live_adapters.SharedProjectRootWorkspaceProvisioner` is
intentionally **not** isolated: every delegated attempt shares the task's
one project root. Real per-attempt git-worktree isolation is T025 scope.

## MCP wire exposure

`server.py` resolves the process's own trusted channel (from
`CORTEX_CHANNEL_*` env, exactly the auth path it already used) once, lazily,
and caches it. With a channel: `tools/list` returns the frozen 7 tools plus
`delegation.mcp_tools.tool_definitions(channel.role)`; `tools/call` for a
name in that role's own delegation operations dispatches to
`delegation.mcp_tools.dispatch` with the resolved `ChannelContext`, before
falling through to the unchanged legacy/channeled path for the 7 tools.
With no channel env, `tools/list` returns exactly the frozen 7 tools (the
existing catalogue/stamp-digest tests are unaffected). See
`tests/test_delegation_mcp_wire.py`, which spawns the real `cortex.py`
subprocess.

## Production wiring

`cortex_acp.py` constructs `acp.production_runtime.ProductionOrchestratingRuntime`
(not `UnavailableRuntime`): it resolves each task's live route lazily from
the existing T008 `ConfigurationService` + connections registry (no new
user configuration) and opens/prompts real engine sessions through
`delegation.live_adapters.EngineSessionManagerAdapter` (one
`EngineSessionManager` per attempt, exactly like a normal non-orchestration
engine session, with that attempt's own trusted-channel credentials wired
into `create_session`'s `mcpServers`). A task with no resolvable
configuration revision, no known connection, or an UNQUALIFIED route still
refuses the prompt explicitly: `RouteUnresolved` (an
`OrchestrationUnavailable` subclass) carries the concrete reason code
(`no_configuration_revision`, `connection_not_found`,
`instruction_delivery_unqualified`, ...), which `acp.agent.Agent` places in
the ACP `-32001` error response's `data.reason` -- never a silent generic
refusal, never a hidden provider/native fallback. See
`tests/test_orchestrating_runtime_live_session.py` (coordinator driven
through the real `EngineSessionManager` + the standalone
`tests/fake_engine_process.py` subprocess) and
`scripts/cortex_acp_smoke.py`'s `normal_prompt:orchestration_unavailable`
check (no connections registered -> explicit refusal, unchanged wire
message).

## What's deferred

- **Coordinator recovery across a process restart**: a coordinator's
  channel secret is (by the T005 trusted-channel security model) never
  persisted, so `OrchestratingRuntime._ensure_coordinator` can only reuse a
  coordinator it opened itself, in this process, since the last restart.
  Resumable coordinator/worker recovery after a crash/restart is T017 scope.
- **`session/cancel` for in-flight workers**: cancelling the coordinator's
  turn returns `cancelled` promptly; already-running worker engine sessions
  are not force-terminated by that alone (T020/T028 refine session/task
  lifecycle further).
- **Native engine subagent observation** (design point 5, optional) is not
  implemented in this change.
- Full completion/acceptance state-machine semantics beyond the `decide`
  operation itself are T017 scope; steering-message/conversation detail
  beyond one coalesced continuation prompt is T020 scope.
