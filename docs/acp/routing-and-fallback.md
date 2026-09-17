# Mixed-team routing and authorized fallback (T027)

This section covers the user-authorized route set a mixed-engine/mixed-
connection task may dispatch within, the per-attempt audit trail every
dispatch (including a fallback) leaves, the pre-dispatch checks that
refuse an unauthorized/over-budget attempt before any process starts, and
the narrow, explicitly-policed conditions under which Cortex ever opens a
*different* route for the same task than the one its active configuration
currently selects.

No universal vendor/role ranking exists anywhere in this code or in these
docs: every route a role may use is either the coordinator's own route (the
default) or a route this task's own stored profile/ceiling names.

## Route profile: the authorized route set

`configuration.policy.RouteProfile` (persisted per configuration revision
in the new `configuration_route_extras` table — see
`routing.store.insert_route_extras_tx`, written in the same transaction as
`configuration.service.ConfigurationService.propose`'s own revision
insert) is one of:

- **Default (single-route inheritance)** — `RouteProfile()`
  (`role_routes=None`). Every role, coordinator and every worker alike, is
  authorized only for that revision's own route. This is what every task
  gets unless a caller explicitly proposes otherwise.
- **Explicit mixed-team profile** — `RouteProfile(role_routes={role:
  {route_id, ...}})`. A role present in the mapping is authorized only for
  its own listed route ids; a role *absent* from the mapping still falls
  back to the coordinator's own route (never to "any route"); an empty set
  for a role means that role may never be dispatched at all.

`routing.authorization.allowed_routes_for_role` additionally narrows
whatever the profile grants by the effective `PolicyCeiling.allowed_routes`
at the same layer — a profile can name a route the ceiling has since
tightened away, but it is never actually authorized. `propose()` itself
also rejects, atomically, any profile that names a route outside the
ceiling being written.

`configuration.service.ConfigurationService.resolve_worker` is the
authoritative role→route resolver: an `explicit_override` is only ever
accepted when the override's route id is a member of the active
revision's own stored route profile (narrowed by ceiling) — never a bare
"is alternates allowed" boolean.

## Per-attempt audit

Every attempt — coordinator, worker, or fallback — that
`delegation.live_adapters.EngineSessionManagerAdapter.open` actually opens
is durably audited in the new `attempt_route_audit` table
(`routing.store.record_attempt_audit`, called from `open`'s own
`_record_route_audit`) with: `engine`, `connection_id`, `model` (the
route's real model string — `configuration_route_extras.route_model`,
never derivable from the opaque `route_id` fingerprint alone), a stable
`params_fingerprint` (`routing.authorization.fingerprint_params`, a
sha256 over the sorted effort/mode/model-config/summarizer/other
parameters), the `config_revision_id` this attempt used, a
`decision` (`'authorized'` or `'fallback'`) plus a small JSON detail, and
— only for a fallback attempt — `predecessor_attempt_id`, the attempt it
replaces. Rows are append-only (`attempt_route_audit_no_update`/
`_no_delete` triggers; deleted only via the existing task-retention path,
see `cleanup.finish_deletions`). `routing.store.list_audit_for_task`/
`list_audit_for_attempt` are the read side; a future capsule/audit-read
surface (T021 extension) is the natural next consumer — not added here to
stay within this task's own scope.

## Pre-dispatch enforcement

`EngineSessionManagerAdapter.open` calls `_authorize_dispatch` (via
`routing.authorization.authorize_dispatch`) **before** `_reserve_capacity_
blocking` (T017's own atomic per-connection concurrency gate) and before
any engine process is launched. It refuses, with a concrete
`PortableError.code`, before any allocation/dispatch:

- `role_route_not_authorized` — the role's resolved route is not a member
  of its authorized set (see above).
- `unsupported_dependent_setting` — when a `CatalogEntry` is supplied (the
  ordinary configuration-propose path already validates this once; `open`
  itself only re-validates when a caller explicitly asks it to), the
  route does not support the requested effort/mode/model-config/summarizer
  bundle.
- `attempt_budget_exhausted` — `PolicyCeiling.limits['attempts']`, when
  tracked, has already been reached for this task
  (`routing.store.count_task_attempts`, counted from the audit table
  itself — every past attempt, authorized or fallback, counts).

Per-connection concurrency remains `delegation.capacity.reserve`'s own
existing atomic job, called strictly after the checks above pass; a
connection at its `max_concurrent_sessions` limit still queues/refuses
exactly as T017 already specifies, unaffected by this task's own checks.
Only `PolicyCeiling.limits['attempts']` is enforced anywhere in this task
(both pre-dispatch, via `authorize_dispatch`, and pre-fallback, via
`authorize_fallback` — a fallback attempt is still an attempt, counted by
the same `routing.store.count_task_attempts`). The `calls` and `tokens`
dimensions exist on `PolicyCeiling.limits` (T008) but are **not metered by
anything in this codebase**: no counter anywhere increments them, so a
ceiling naming them has no enforceable effect today. This is a precise,
intentional gap, not a silent no-op disguised as coverage — T024's
summarizer/budget tracking (`recovery.token_budget`/`budget_store`) is a
different, session-scoped context-window accounting mechanism, not a
per-task dispatch/call counter, and is never conflated with these two
ceiling dimensions. Introducing a real `calls`/`tokens` counter (and wiring
it into both authorization functions the same way `attempts` is wired) is
future work.

## Authorized fallback

`routing.authorization.authorize_fallback` is the single fallback gate.
It refuses, before anything is dispatched or reconciled:

- **Hard denylist** — `cancellation`/`refusal`/`authorization_loss`/
  `provider_restriction` (`configuration.policy.FALLBACK_HARD_DENYLIST`,
  T008) can never trigger fallback, regardless of any policy.
- **Unknown failure class** — only `engine_failure` and
  `capacity_unavailable` (`configuration.policy.
  KNOWN_FALLBACK_ELIGIBLE_CLASSES`) are ever fallback-eligible; naming any
  other class in a `fallback_policy` does not make it eligible here.
- **No durable policy** — `PolicyCeiling.fallback_policy.permits(...)`
  must explicitly name the failure class (or leave the class dimension
  unrestricted) *and* set `data_recipient_consent=True`; a task with no
  fallback policy at all — the default — stops and reports, never
  attempts fallback.
- **Target route not authorized** — the target route id must be within
  both `fallback_policy.allowed_target_routes` (when restricted) *and*
  the role's own route-profile/ceiling authorization
  (`fallback_role_route_not_authorized`) — fallback never bypasses the
  mixed-team profile that would otherwise forbid the same route change.
- **Cross-provider consent** — when the target connection's `provider`
  differs from the current one, a durable, unrevoked
  `fallback_consent_records` row for that exact target connection/provider
  (`routing.store.record_consent`/`has_active_consent`) must already exist
  *in addition to* the `data_recipient_consent` policy flag — the boolean
  flag alone is necessary but never sufficient. Naming a data scope on
  that record is mandatory; no fallback ever infers scope from the fact
  that consent exists.
- **Budget** — `fallback_policy.budget`, when set, counts only
  `decision='fallback'` audit rows for the task
  (`routing.store.count_fallback_attempts`); exhausting it refuses further
  fallback attempts.

`delegation.live_adapters.EngineSessionManagerAdapter.attempt_fallback` is
the only production call site that ever opens a different route/connection
for a task than its active configuration currently selects. It re-derives
the active revision's ceiling/route-profile/current-connection fresh,
calls `authorize_fallback`, and — only on success — opens the new attempt
via `open(..., launch={"route_override": {...}, "fallback_of":
old_attempt_id, "fallback_failure_class": ...})`, which is audited with
`decision='fallback'` and the predecessor link. Reconciling the old
attempt (T023's `recovery.coordinator_recovery`/
`recovery.worker_reconciliation`) and rebuilding context canonically
(T021/T022 capsules — never vendor-opaque provider memory) remain the
caller's own responsibility around this call, exactly as for any other
T023 replacement.

### Automatic trigger: `OrchestratingRuntime._attempt_worker_route_fallback`

`acp.orchestrating_runtime.OrchestratingRuntime` wires the actual decision
point: a worker's own `EngineTurnFailed` (the T017-classified
`transport_eof`/`timeout_alive`/`poisoned`/generic engine-error kinds —
never refusal, which is a `PromptOutcome.stop_reason`, not an exception)
and a `capacity_unsatisfiable`/`capacity_timeout` `PortableError` from
`delegation.provisioning.ProvisioningService.delegate` both call
`_attempt_worker_route_fallback`, which:

1. Reads the active configuration revision's ceiling/route-profile/route-
   details (`configuration_route_extras`) fresh — never a cached/stale
   copy.
2. Computes the single deterministic candidate target route (the
   intersection of the role's authorized routes and
   `fallback_policy.allowed_target_routes`, excluding the current route
   and any route this revision never recorded resolvable details for).
3. Reconciles the old attempt first (T023's
   `recovery.worker_reconciliation._best_effort_terminate`/
   `_revoke_worker_channel` — best-effort process termination and trusted-
   channel revocation) and issues a **fresh worker channel** bound to the
   new attempt id (a channel is never valid for a different attempt;
   T023's own scout finding: cross-connection fallback needs its own
   channel-issuance path, closed here via `trusted_channel.issue_channel_tx`,
   the same primitive `delegation.provisioning._step_channel` uses for a
   first-time delegation).
4. Calls `SessionRuntime.attempt_fallback` (`authorize_fallback` runs
   inside it); on success, updates `delegation_provisioning.attempt_id`/
   `engine_session_ref` for this delegation's idempotency key so
   `delegation.completion.gate`'s existing coordinator-ref↔attempt-id
   bridge resolves the fallback attempt's own worker-facing events
   (`worker_result`/`worker_question`, always keyed by attempt id) exactly
   as it would for the original attempt — never a second, parallel
   bookkeeping path. Re-delivers the identical brief via the normal
   `_begin_generation_delivery`/`SessionRuntime.open` path (T021/T022
   capsule + T026 qualified `session/new` instruction delivery, fresh for
   the *new* engine).
5. On any refusal (no policy/consent, denylisted/unknown class, exhausted
   attempt or fallback budget, unauthorized route), records exactly one
   concrete `assignment_failed:...:fallback` event
   (`fallback_refused:<code>`) and returns — **never** a second attempt on
   a third route, never a retry of the same route.

When fallback is never even evaluated (no attempt id to reconcile, or no
real configuration revision resolvable — e.g. a test double with no T008
configuration behind it), the caller's own plain, pre-T027
`engine_failure:<kind>` message is the only one written, preserving every
pre-existing (non-T027) failure-reporting behavior byte-for-byte.

See `tests/test_orchestrating_runtime_routing_fallback.py` for the
fake-engine, real-`ConfigurationService`-revision coverage: an authorized
engine-failure fallback opening a second route with both attempts audited
and linked; refusal never triggering fallback (open-call count asserted);
cross-provider fallback refused before any session for the target
connection is ever opened; an exhausted attempt budget refusing fallback;
and a running worker keeping its already-recorded route/revision when the
coordinator's own selection changes mid-task.

## Anti-bypass invariants

- A newly available cheaper model is never auto-selected: nothing in this
  task ever changes `PolicyCeiling`/`RouteProfile`/`FallbackPolicy` on its
  own initiative; every widening requires an explicit, separately-narrowed
  proposal (T008).
- Refusal/cancel/authorization-loss/provider-limit can never reach
  `authorize_fallback`'s success path (hard denylist, checked first,
  unconditionally).
- An already-open attempt never re-resolves its route: `open` records the
  configuration revision/route it was actually launched with once, at
  launch time; a later coordinator selection/profile change only ever
  affects the *next* attempt's resolution (`resolve_worker`/
  `authorize_dispatch`, evaluated fresh per attempt), never a running
  one's already-recorded context (T023's own attempt_effective_context
  precedent, extended here by `attempt_route_audit`).
- Fallback attribution is immutable and additive-only (audit rows are
  never overwritten or deleted outside the standard task-retention path);
  the pre-fallback route stays visible in the same task's audit history.

See `tests/test_routing_policy.py` (pure policy/store unit coverage) and
`tests/test_delegation_routing_dispatch.py` (real
`EngineSessionManagerAdapter` + fake-engine-subprocess integration: mixed
engines in one task, the same model name on two connections with no
credential crossover, a live selection change mid-task leaving the
running attempt's audited route unchanged, an unauthorized/unavailable
route refused before dispatch, consent denial refusing a cross-provider
fallback before any data transfer, an exhausted attempt budget, and
concurrent allocation at a connection's own limit).
