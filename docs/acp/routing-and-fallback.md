# Automatic routing, mixed teams, and authorized fallback

Cortex automatically selects worker engine, model, and effort from the runtime's
current model catalog, task tier, work kind, routing policy and each engine's
observed subscription plan usage; how often an engine was chosen before is not
an input. The model may submit a routing recommendation in
its plan; only the runtime decides and dispatches. A plan hint never grants a
route or raises effort/tier beyond runtime policy. `list_route_options` is a
read-only snapshot for coordinators; dispatch validates current eligibility
again.

This automatic worker selection is separate from configured coordinator routes
and from the explicitly authorized fallback path described below. Legacy
mixed-team route profiles and fallback checks remain documented here for
assignments that use them.

## Route profile: the authorized route set

`configuration.policy.RouteProfile` (persisted per configuration revision
in the new `configuration_route_extras` table — see
`routing.store.insert_route_extras_tx`, written in the same transaction as
`configuration.service.ConfigurationService.propose`'s own revision
insert) is one of:

- **Default (single-route inheritance)** — `RouteProfile()`
  (`role_routes=None`). For explicit profile resolution and fallback, each
  role inherits the revision's own route unless the task proposes a different
  profile. Runtime automatic worker routing uses its separately audited
  `auto`/`auto:pin` basis before opening a worker session.
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
worker detail panel) prefers this resolved name over the bare id and,
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

`delegation.capacity.reserve` atomically enforces each connection's
`max_concurrent_sessions` limit (default 4, configurable from 1 through 64)
separately for coordinator and worker reservations. Reservations of the other
kind do not consume that kind's slots, so the combined active count can exceed
the configured value. A worker waits for worker capacity to become available;
coordinator reservations cannot consume worker slots.
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
never refusal, which is a `PromptOutcome.stop_reason`, not an exception), or a
`capacity_timeout` `PortableError` from
`delegation.provisioning.ProvisioningService.delegate`, can call
`_attempt_worker_route_fallback`. That handler:

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

## Automatic worker routing: tiers, model hints, and ranking

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

The role tier starts from `ROLE_TIERS` and is shifted by complexity (low moves
cheaper; high moves stronger). On escalation, the runtime normally advances the tier;
`flagship` is reserved for escalation. Ordinary standard-tier testing
retries and review fixes retain the standard candidate pool described below;
other standard-tier retries escalate normally. If no usable candidate exists in the
requested tier, the configured fallback order searches other tiers. Explorer
remains light at every complexity and escalation; because its tier cannot rise,
escalation raises its effort one supported step (normally low to medium; on
Luna, high to xhigh).

Models are grouped into light, standard, deep, and flagship. An unavailable
role tier searches the nearest tier permitted by policy; flagship is never a
starting tier. Haiku is excluded from automatic selection by default, but
`auto_allow` can opt in a model or `engine/model`; `deny` and `providers` still
apply. Setup writes the default user `routing.json`, `{"deny": ["gpt-5.5",
"haiku"]}`, whose bare `haiku` token denies every Haiku id and alias
everywhere (see [configuration](../configuration.md#routing-rules)). A pin is
strict and has priority over automatic selection.

Weaker models do light work and stronger models do hard work: a task never
runs on a stronger tier than its own because another engine lacks a model in
that tier. With Haiku auto-excluded, the light tier has only Codex Luna, so
light work (explorer, planner, technical writer, general, and low-complexity
work) runs on Luna even when a Claude plan has more room. This single-engine
light tier is accepted; `auto_allow` for Haiku in `routing.json` is the
owner's lever for letting Claude take light work. The rank falls back to
another tier only when **no** engine has a usable model in the task's tier
(`_FALLBACK_ORDER`). Rank version 3 briefly added a stronger model from the
other engine as a `cross_tier` candidate; that was removed in rank version 4,
and older audit rows that carry `cross_tier`/`chosen_cross_tier` stay
readable.

### Codex Luna

The Codex Luna line (`gpt-6-luna`, `gpt-5.6-luna`, and any later
`gpt-<n>-luna`; the newest non-denied version is routed) is very cheap and
very effective, so it has its own rule in `routing.auto_route` (rank version
7). The rule is hard-coded: `routing.json` classifies models by tier, and has
no per-work-kind or per-complexity setting that would fit it.

- **Effort floor.** Luna never runs below `high` (`LUNA_EFFORT_FLOOR`). The
  floor also applies to a pin (`codex-acp/gpt-6-luna:low` runs at `high`) and
  to a worker's requested effort at dispatch. `max` is allowed on Luna even
  when a `tiers` override moves it out of the light tier. A Luna build that
  offers nothing at or above `high` keeps what it offers.
- **Effort by work** (`LUNA_EFFORTS`, `luna_effort`): `high` for light work;
  `xhigh` for ordinary standard-tier testing; `max` for standard-tier testing
  that is a retry or a fix after review requested changes (`escalate`). That
  work has already proved harder than planned but does not justify a deep
  model. Eligibility uses the base task tier before generic escalation:
  normal standard-tier testing stays in the standard pool on a retry or
  review fix. Luna targets `max`; another selected model raises effort one
  step (normally medium to high). Retry diversification and quota still
  apply, and an unusable standard pool falls back through the usual tier
  order. The audit retains `base_role_tier` as well as the effective
  `role_tier`. Other standard-tier work, explicit high complexity and deep
  roles retain their normal escalation and Luna exclusions. Escalated light work (explorer) gets `xhigh`.
- **Candidacy** (`luna_status`, recorded as `luna.status`). Luna remains the
  light tier's model (`light_tier`). In the standard tier it is a candidate
  only for testing work (`LUNA_WORK_KINDS`: testing, that is `qa-engineer` and
  `build-verification`), where it ranks next to the tier's own models, such as
  `gpt-5.6-terra` and Claude Sonnet (`standard_candidate`). The ordinary rank then decides: preference,
  hint, plan usage, diversify, affinity, quota band, used percent, engine
  order. Luna is not always chosen. A Claude plan at least 10 points less
  used, a much hotter Codex plan than the alternative, or a hint for another
  model all move the work away from it. On one
  Codex plan with equal affinity, Codex's own list order (newest generation
  first) puts `gpt-6-luna` ahead of the older `gpt-5.6-terra`. All other
  standard-tier work -- implementation, operations, debugging, design,
  review, planning, writing and analysis -- stays on the tier's own models
  (`not_candidate:work_kind_<kind>`).
- **Never Luna** (`excluded:<reason>`): `complexity: high` work, deep or
  flagship work, and a deep role (architect, database architect, security
  auditor, senior consultant) even at `complexity: low`. This applies even as
  a fallback. When only Luna is usable, such work is refused with
  `no_available_route` and the reason is named.
- A `tiers` override that moves a Luna model out of the light tier makes it
  route like any model of that tier. Only its effort floor and `max` still
  apply.

An exact Luna hint on standard-tier testing work is not a tier mismatch,
either at plan submission (`hint_serves_tier`) or at dispatch.

Only the newest version of each model family per engine is routed
automatically (`routing.auto_route.superseded_routes`). Family and version
come from the engine-confirmed model id (`gpt-6-luna` → luna 6,
`claude-opus-4-8` → opus 4.8). For a versionless alias, they come from the
engine's display name for the same family (`opus[1m]`, "Opus 5.5" → opus 5.5).
No version table is maintained. `deny` is applied first, so the newest
*non-denied* version wins. A model with no parseable version is never
superseded. Older versions stay selectable in the editor and pinnable. The
route menu shows them with `auto_eligible: false` and `superseded_by`, the
decision records them under `superseded`, and a plan hint naming one gets
`ignored:superseded`. An unavailable or exhausted newest version does not
hand its place to an older one.

`plan_submit` requires every node to provide `model`: `auto`, an engine id, or
an exact `engine/model` route label from `list_route_options`. A non-`auto`
recommendation requires a short `model_reason`; it is optional for `auto`. This is a recommendation only. A model hint is
considered only within the runtime-computed tier and cannot override pin, deny,
providers, availability, or the 100% quota exclusion. A hint from another tier is recorded as `ignored:tier_mismatch`;
using `auto` delegates all selection to ranking. Legacy stored plans without
`model` retain their old `provider` preference behavior; new plans use `model`.

On escalation, the previous hint is not inherited. The runtime ranks the new
tier and prefers a different engine from the failed attempt when candidates
otherwise tie. A hint is honoured unless the hinted route is excluded,
unavailable or exhausted, or its usage is both at or above 85%
(`HINT_TOLERANCE`, equal to `HOT_PERCENT`) and meaningfully worse -- by more
than `USAGE_MARGIN` (10) points -- than an available alternative's known
usage (`ignored:near_quota_limit`). Crossing 85% alone, with no known
alternative or no meaningful gap, no longer disqualifies a hint (rank
version 6). How often an engine was chosen
before never affects a hint. Other hint statuses include
`ignored:provider_not_allowed`, `ignored:not_offered`,
`ignored:tier_mismatch`, `ignored:superseded`, `ignored:auto_excluded`, `ignored:no_candidate_in_tier`,
`ignored:unavailable`, `ignored:escalated`, and
`ignored:pinned`; the audit includes the reason and status. Older audit rows
may carry `ignored:engine_overused` from the removed count balance.

### Within-tier rank order

Route choice depends on the task (role tier, complexity, work-kind engine
affinity, coordinator hint) and on each engine's subscription plan usage.
How often an engine was chosen before is **not** an input: frequency is not
evidence of fit, so there is no count-based balance, window or in-flight
count. Hard exclusions are applied before ranking: unavailable routes, `deny`,
`providers`, superseded older versions, strict pins, and quota at 100%. Among
eligible candidates the rank criteria are, first difference wins:

1. Honoured legacy provider preference for a stored pre-model plan (ignored
   when the preferred route's usage is both at or above 85% and meaningfully
   worse -- by more than `USAGE_MARGIN` (10) points -- than an available
   alternative's known usage).
2. Honoured model hint for a new plan (see above), ignored on the same
   hot-and-meaningfully-worse basis.
3. Plan usage, only when every candidate's usage is known: each full 10
   points (`USAGE_MARGIN`) above the least-used candidate ranks a candidate
   one step behind (`usage_band`). The plan with clearly more remaining quota
   takes the work; a smaller difference is left to the rest of the order
   below. This usage-gap signal, not the quota band, is what usually
   separates candidates on remaining quota.
4. On escalation, an engine other than the one whose attempt fell short --
   unless step 3 already sent the work to a meaningfully less-used plan,
   diversification does not retry onto a plan with meaningfully less room.
5. Engine affinity (role suitability). Built-in defaults
   (`WORK_KIND_ENGINES`): implementation and operations lean to `codex-acp`;
   investigation, review, design, writing, planning and analysis lean to
   `claude-agent-acp`; debugging and testing are neutral. `routing.json`
   `engine_affinity` overrides it per work kind (see
   [configuration](../configuration.md#routing-rules)).
6. Quota band, as a late tie breaker only (rank version 6): known usage at
   or above 85% (`HOT_PERCENT`) ranks behind candidates below 85%, but only
   once steps 1-5 have not already separated the candidates. Unknown quota
   is not hot. Crossing 85% alone no longer overrides plan usage, retry
   diversification or engine affinity, and no longer causes a cliff right at
   the boundary -- a candidate at 86% used is barely behind one at 84% used,
   not sent to the back of the rank. Light work used to spend a hot plan
   first (commit 7b7aca22); quota was then promoted to an early, absolute
   cut ahead of plan usage because `used_percent` is the most-used open
   window (often a weekly one) and the rank never sees its reset time, so it
   could otherwise push cheap work into a plan about to run out -- rank
   version 6 keeps that protection through the usage-gap signal in step 3
   instead, and only demotes the raw 85% crossing itself.
7. Exact used percentage, only when every candidate has a known value.
8. Stable engine order.

`decided_by` names the first of these keys (`preference`, `hint`, `usage`,
`diversify`, `affinity`, `quota`, `used_percent`, `engine_order`) that
separated the chosen candidate from the runner-up, or `only_candidate`; a pin
records `pin`. It is never `balance`. The runner-up can be another model on
the same engine, in which case `engine_order` is the usual answer.

Unknown quota remains neutral and is never coerced to zero for percentage
comparison: when any candidate's usage is unknown, steps 3 and 7 compare
nothing and the order falls through to diversify, affinity or the quota tie
breaker (`quota` is `false`/not-hot for unknown usage, so it still separates
a known-hot candidate from an unknown one).

Effort is derived from the runtime-selected tier, task complexity/escalation,
and each model's supported effort list. `effort_basis` records whether it came
from the role tier, escalation step, the Luna rule (`luna:<tier>` or
`luna:<tier>:escalated`), or a pin. A pin may explicitly name an
effort; otherwise the runtime chooses a supported effort. A model hint cannot
raise tier or effort.

The read-only `list_route_options` tool reports currently engine-confirmed
routes, tier, supported effort, availability and quota band, plus role tiers
for low/normal/high complexity and the Luna rule (`luna`: effort floor, efforts
by work, and the roles for which Luna is a standard-tier candidate). It is advisory and may become stale; the
runtime validates the choice at plan submission and dispatch. The server
advertises `listChanged: false`, so clients must call the menu again when they
need a fresh snapshot; there is no push refresh contract.

Audit rows retain the decision under `decision_detail.route_decision`,
including `rank_version`, `role_tier`, tier and effort basis, `model_hint`,
`model_hint_status`, `model_hint_reason`, `diversify_from`, `usage`
(`compared`, `margin`, `least_used_percent`), `affinity` (work kind, engine,
`default` or `routing.json` source), candidates with `used_percent`, `hot`,
`quota_state`, `usage_band`, `suitability` and `preferred`, `decided_by`,
`excluded_by_policy`, `unavailable`, `superseded`, `luna` (`status`,
`candidate`, `chosen`, and `outranked_by`, the rank key that put the best Luna
candidate behind the chosen route), and whether usage percentages were
comparable. `rank_version` history: 2 ranked by quota band,
count balance (tolerance 4) and suitability; 3 added plan usage, engine
affinity, cross-tier stand-ins and a two-attempt balance; 4 removed attempt
counts and cross-tier stand-ins; 5 made Luna a standard-tier candidate for
development work; 6 moved the quota band from an early hard cut (right
after `hint`) to a late tie breaker (right before `used_percent`), behind
plan usage, retry diversification and engine affinity, and made the
preference/hint quota exclusion require the excluded route's usage to be
both hot and meaningfully worse (`USAGE_MARGIN`) than an available
alternative's, not merely at or above `HOT_PERCENT`; 7 narrowed Luna's
standard-tier candidacy to testing work only, so other standard-tier retries
and review fixes escalate as before version 5. Older rows keep their own fields
(`balance`, `balance_excess`, `cross_tier`, `chosen_cross_tier`) and still
render. These fields are evidence of runtime decisions, not proof of
the exact observed live surface.

The worker card title and each `pipeline.md` assignment line show a one-line
summary of the recorded decision (`routing.decision_display`): engine, model,
effort, tier (`light→standard` when the routed model's tier differs from the
task's), hint status when a hint was given, `by <decided_by>`, and the chosen
plan's usage (`usage 42%` or `usage unknown`). On a card the line is at most
80 characters (`CARD_SUMMARY_MAX`): the alias marker, then the hint, then
part of the model name are dropped before anything is clipped;
`pipeline.md` keeps the full line. Count balance is never shown.

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
  Codex reads it from its rollouts; Claude merges its adapter's
  `rate_limit_event` with a lazy, zero-cost read of the Claude Code CLI's own
  local `/usage` command (see `docs/acp/token-and-plan-usage.md`'s "Claude
  plan usage: the local `/usage` CLI"), newest observation per window
  winning.
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
  which already keeps the account from being chosen; the CLI-read `/usage`
  windows (above) also converge on the same 100% within their own TTL.
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
