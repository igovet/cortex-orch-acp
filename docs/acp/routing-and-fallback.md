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

T035: the audited `model` column stays the routing id/alias (`opus[1m]`,
`default`, `gpt-5.6-luna`, ...) — it is the value pin/deny and every other
routing decision key on, and it is never replaced. Alongside it,
`_record_route_audit` calls the new
`live_adapters._resolve_route_model_name(connection_id, model, environ)`,
which reads this connection's latest cached catalog snapshot
(`catalog.store.latest_connection_entries`) and resolves the identical
real, engine-observed display name `acp.config_options.presented_entries`
would show for a fresh `session/new` (`catalog.labels.presented_model_name`:
the engine's own named `configOptions` entry, `default`'s own resolved
description, or a deterministically decodable versioned id such as
`claude-opus-5-5[1m]` → `Opus 5.5 (1M)`). When resolved, it is written into
the audit's own `detail["model_name"]`; every display reader
(`acp.orchestrating_runtime._delegation_route_label` for worker cards/ACP
titles, and Paseo's `worker-history.ts`/`worker-display.ts` for the
Workers popover and panel) prefers this resolved name over the bare id and,
when it is absent (no cached snapshot, no matching entry, or no evidence
beyond the alias itself), shows the routing id explicitly marked as an
alias (`opus[1m] (alias)`) rather than presenting it as though it were the
concrete model. Never a guessed version string.

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

## Automatic worker routing: tiers and the choice within a tier

`routing.auto_route.choose` picks the role's tier (`ROLE_TIERS`, shifted by
the planned `complexity` and by escalation), then a model of that tier (the
nearest other tier when none is usable), then an effort.

### Economy policy

Automatic routing uses four tiers: `light`, `standard`, `deep`, and
`flagship`. Astra and Fable are the only named flagship families. They are
never selected for a role's default tier or because a plan merely says
`complexity: high`; high complexity tops out at `deep` (Sol/Opus class).
The flagship tier requires an explicit runtime escalation after a prior
attempt fell short. A user `routing.json` pin is also an explicit route
choice and remains strict.

Explorer stays `light` for low, normal, high, and escalated attempts. It can
therefore select only an engine-confirmed light-tier model such as Luna or
Haiku; it never falls back upward to strong or flagship models. Unknown catalog
models are assigned only among deep, standard, and light, so an unrecognized
model is never inferred to be flagship.

Independent review derives the completed execution engines from the immutable
attempt-route audit. If every completed worker used Claude, it prefers Codex
for the reviewer; if they all used Codex, it prefers Claude. Availability,
quota, provider restrictions, and pins can prevent that preference from being
honored. The audit records both `preference_status` and
`cross_provider_review`; mixed execution engines record
`no_single_opposite_provider:mixed_execution` and use normal ranking.

A model's tier comes from, first match wins:

1. `tiers` in `routing.json`;
2. a Claude family name (`FAMILY_TIERS`: fable `flagship`, opus `deep`, sonnet
   `standard`, haiku `light`);
3. a known Codex model (`KNOWN_MODEL_TIERS`: `gpt-6-astra` `flagship`, `gpt-6-sol`
   `deep`; `gpt-5.6-sol`, `gpt-5.6-terra` `standard`; `gpt-6-luna`,
   `gpt-5.6-luna`, `gpt-5.5` `light`);
4. only for a model none of these names: its position among its
   connection's other unnamed models, split into thirds.

codex-acp lists its newest generation first (`gpt-6-luna` before
`gpt-5.6-sol`), so its list order is not strength order. With both
providers configured, every tier has a candidate from each.

`auto_route.rank` orders the usable candidates of the chosen tier (not
recorded unavailable, plan window below 100%). The first difference wins:

1. **Provider preference.** The plan task's optional `provider`, when
   honoured (see below).
2. **Quota band.** For `deep`/`standard` work, accounts below `HOT_PERCENT`
   (85%) come first. For `light` work, accounts above it come first.
3. **Suitability.** The engine that suits the role's kind of work
   (`ROLE_WORK_KINDS` → `WORK_KIND_ENGINES`) comes first. Codex takes
   implementation, debugging, testing, operations and investigation; Claude
   takes review, design, writing, planning and analysis. This is a
   judgement, not a benchmark.
4. **Used percentage.** Lower first. An unknown reading counts as 0.
5. **Engine order.** The final tiebreak.

Before this change the order was only quota band, then used percentage, then
engine order. Codex's rollout window had reset and Claude reports usage only
during a turn, so both readings were usually unknown and tied. The tie then
went to whichever connection the registry listed first.

The coordinator's `provider` preference is set per plan task through
`plan_submit`. It names an engine id, never a model, and a review fix
(`_fix_N_<node>`) keeps its node's preference
(`orchestration.state.attempt_routing`). It never changes the tier, and the
runtime still owns routing, deny, strict pin and failover. It is ignored,
with the reason recorded, in these cases:

| `preference_status` | When |
| --- | --- |
| `ignored:pinned` | a `pin` applies to the role |
| `ignored:provider_not_allowed` | `providers` in `routing.json` excludes it |
| `ignored:provider_not_configured` | no engine-confirmed, non-denied route of that engine exists |
| `ignored:provider_unavailable` | its routes in the tier are recorded unavailable or at 100% |
| `ignored:no_candidate_in_tier` | it offers no model in the chosen tier |
| `ignored:provider_near_quota_limit` | `deep`/`standard` work, every preferred candidate at or above 85% while another candidate is below |

Otherwise the status is `honoured`, or `none` when no preference was given.

The decision basis is stored in the attempt's route audit as
`decision_detail.route_decision`
(`live_adapters._record_route_audit`). It holds the role, the role tier, the
chosen tier, the basis (`rank` or `pin`), the preference and its status,
every candidate of the tier in rank order (route, connection,
`used_percent`, `hot`, `suitability`, `preferred`), the tier's routes that
were skipped as unavailable, and the chosen route.

See `tests/test_auto_route.py` (both providers in every tier, suitability
between idle accounts, quota outranking suitability, preference honoured and
ignored) and `tests/test_auto_route_delegation.py` (the same through real
provisioning: the audit's `route_decision`, preference outside `providers`,
under a pin, and on an exhausted provider).

## Automatic worker routing: unavailable routes

Automatic worker routing (`routing.auto_route.choose`, called from
`EngineSessionManagerAdapter._resolve_runtime_route` for every worker attempt
that no mixed-team profile or fallback has already routed) skips any route
that `routing.availability` records as unavailable. This is separate from the
T027 fallback above: nothing here re-opens a *running* attempt on another
route. A failed worker's node is retried through the ordinary
`orchestration.recovery.retry_lost` path, and its **next** attempt is routed
without the failed route.

What is recorded, and only from real evidence:

| Where it fails | Evidence | Reason | Scope |
| --- | --- | --- | --- |
| `session/prompt` or `session/new` JSON-RPC error | `data.codexErrorInfo="usageLimitExceeded"` (codex-acp 1.13.0) or `data.errorKind` `billing_error`/`account_on_hold` (claude-agent-acp 0.81.0) | `quota_exhausted` | whole connection |
| same | `codexErrorInfo="rateLimitExceeded"` or `errorKind="rate_limit"` | `rate_limited` | whole connection |
| same | `errorKind="model_not_found"` | `model_unavailable` | that model |
| worker `open()` | `WorkerModeNotConfirmed` (see `claude-agent-acp-adapter.md`) | `configuration_not_confirmed` | that model |
| worker `open()`, engine launch/`initialize`/`session/new` | `EngineTransportClosed`, `EngineTimeout` or `OSError` | `engine_start_failed` | whole connection |

`engine.failure_classification.route_failure` decides the first three rows
from the structured fields alone. Message text such as "You've hit your
usage limit" is never matched, so an unrelated error can't read as
exhaustion. A cancellation, a capacity wait, a policy refusal, an
authentication failure, and a failure before the engine process is created
record nothing.

Each record is one `ROUTE_UNAVAILABLE` row in `runtime_events` on the failed
attempt's task (no schema change). It holds the engine, connection, model (for
model-scoped records), reason, the structured evidence
(e.g. `codexErrorInfo=usageLimitExceeded`), the attempt id and `expires_at`.
It never holds credentials, prompts or engine prose. `availability.active`
reads every unexpired record in the project store, so all tasks of the project
see an exhausted account.

Expiry works like this:

- For quota and rate limits, the reset time the engine itself reported is used
  (`connections.quota.resets_at`: the most-used open window's `resets_at`).
  Codex reads it from its rollouts, Claude from its `rate_limit_event`.
- Otherwise a default applies: 60 minutes for quota exhaustion and an
  unavailable model, 30 for an unconfirmed configuration, 15 for a rate limit
  or an engine start failure.
- Every expiry is capped at 8 days.

Expiry is the only way a record stops counting. The route is tried again after
it, not before.

Choosing and refusing:

- A recorded route is excluded from `auto_route.pick` without moving any other
  model's tier. Selection otherwise follows the usual tier fallback order, so
  the next usable route may be another provider.
- The existing rule still applies: a route whose engine-reported plan window
  is at 100% is never chosen.
- When routes are configured but none is usable, `choose` raises
  `no_available_route` (non-retryable). Its remediation names every
  unavailable route, with the reason and expiry. It never silently falls
  back to the coordinator's route.
- A worker is also refused with `no_available_route` when its fixed route is
  recorded unavailable. "Fixed" means the coordinator's inherited route when no
  catalog is engine-confirmed, a mixed-team route or a fallback target
  (`_refuse_unavailable_route`).
- `routing.json` keeps its meaning:
  - `deny` is applied before availability and is never lifted to make room.
  - A strict `pin` whose route is recorded unavailable is refused as
    `pinned_route_unavailable` with the reason, never substituted.
  - `providers` (a list of engine ids, e.g. `["claude-agent-acp"]`) limits
    which engines `auto_route.choose`/`pick` and failover consider; an
    explicitly chosen route is unaffected. A `pin` naming an engine outside
    `providers` is refused as `pinned_route_unavailable`. When `providers`
    excludes every engine-confirmed route, `choose` fails closed with
    `no_available_route` rather than falling back to the coordinator's
    (possibly excluded) route -- that fallback only applies when nothing is
    engine-confirmed at all.

Retries (`orchestration.recovery.retry_lost`):

- A routing refusal (`no_available_route`, `pinned_route_unavailable`,
  `invalid_routing_overrides`, `route_denied`) is never retried. It becomes the
  named blocker `worker_failure_not_retryable:<code>: <remediation>`, which the
  coordinator and the stop question show.
- A failure whose attempt has a `ROUTE_UNAVAILABLE` record (matched by attempt
  id, not by error text) is retried with `route_failure=true` on the
  `ASSIGNMENT_RETRIED` event. Such a retry:
  - does not use up the two ordinary retries (`MAX_ATTEMPT_RETRIES`);
  - does not escalate the model tier (`orchestration.state.attempt_routing`),
    because no model fell short;
  - is bounded by `MAX_ROUTE_RETRIES` (6).

Known limits:

- An engine that reports exhaustion only as chat text and then ends its turn
  normally, with no structured error, is not recorded. That turn ends as
  `worker_ended_without_result` and is retried like any other.
- codex-acp 1.13.0 puts `codexErrorInfo` on the prompt's JSON-RPC error
  because Cortex does not advertise typed session failures. A client that did
  advertise them would get the failure as a session update instead, which
  nothing here reads.
- A Claude usage-limit turn whose SDK reports no error kind carries no
  `errorKind`, so it is not recorded. The adapter's `rate_limit_event` with
  `status: rejected` still sets that window to 100% (`connections.quota.observe`),
  which already keeps the account from being chosen.
- Records are per project store: another project's first attempt on an
  exhausted account still fails once before it records the route.

See `tests/test_route_availability.py` (classification, expiry, retry
accounting, named refusals), `tests/test_auto_route.py` (exclusion without
tier movement, cross-provider pick, the reset of the most-used window,
`providers` validation and filtering) and `tests/test_auto_route_delegation.py`
(real provisioning over fake engines: exhausted provider skipped, a dispatch
failure recorded and the next delegation opened on the other provider, a
codex-shaped `usageLimitExceeded` turn recorded until its engine-reported
reset, an unrelated error recording nothing, everything unavailable refused
with each route named, deny/pin/providers interaction, expiry).

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
