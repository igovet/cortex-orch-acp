# Automatic routing, mixed teams, and authorized fallback

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed.


> **Current contract:** former built-in name/family/position-based model rules described in historical sections below are removed. Current automatic and coordinator routing requires explicit version-2 owner settings and observed catalog identities. See [settings-only routing](#settings-only-routing).

Cortex automatically selects worker engine, model, and effort from the runtime's
current model catalog, task tier, work kind, routing policy and each engine's
observed subscription plan usage; how often an engine was chosen before is not
an input. The model may submit a routing recommendation in
its plan. A recommendation alone is advisory. An explicit model choice
confirmed from the task's recorded user messages or answers has first priority
on covered exact-route nodes in every tier, including retries and review fixes.
It overrides pins, preferences, usage ranking, automatic membership and role-tier
caps, but never hard owner authorization or engine availability. Refusals are
visible and never silently substitute another model. Effort remains supported
and bounded by the configured per-model policy. `list_route_options` is a
read-only snapshot for coordinators; dispatch validates current eligibility
again.

The runtime-created mandatory review node has no plan hint. When a user message
names a model for review or audit ("Ревью делай через sonnet", "за аудит отвечает
Opus"), that model is its hint and routes as an explicit user request, ahead of pins
and preferences (`auto_route.user_purpose_model_request`, applied in
`orchestration.state.attempt_routing` to every node without an exact hint, by the
role's purpose: review/audit, design/UI/UX, tests/QA, docs, security, accessibility,
backend, frontend, devops/deploy, debugging, architecture, refactoring, planning). The
newest user sentence naming that purpose decides; a refusal there, or two models for
it, applies nothing. Before this the review node could never get the owner's model,
which then lost to pins (2026-10-22). Without such a request its route comes from:

1. an owner pin for `worker-code-reviewer`, its alias, or `*`;
2. failing that, automatic ranking with the cross-provider review preference.

To run an entire workload on one selected route, the owner pins the reviewer
role to it. Review fixes inherit the planned node's hint, so they stay covered.
Before a run, `python3 -B scripts/cortex_paseo_dev.py route-preflight --route
ENGINE/MODEL[:EFFORT]` reads only the dev profile's routing policy and exits 1
with a visible refusal for the following policy mismatches:

- `review_route_not_pinned`: no pin applies to the reviewer;
- `review_route_pin_mismatch` or `review_effort_mismatch`: the applicable pin
  names another route or effort;
- `route_denied_by_owner_policy`, `route_provider_not_allowed` or
  `route_policy_missing`: owner policy would refuse the route;
- `selected_effort_conflicts_with_policy`: execution or reviewer effort is
  outside configured policy even when the pin text matches.

It never refreshes quota or connection data, never grants consent and never
writes. Availability, quota and a task's ceiling are checked again at dispatch
(`src/cortex_runtime/routing/stage_preflight.py`).

### Explicit effort preservation

A covered exact-model request can also select an exact effort in that **same
retained user message or answer**: `Use engine/model:medium`,
`Use engine/model/medium`, or `Use engine/model at medium effort throughout`.
A uniquely enabled bare model id also works. Only adjacent effort syntax is
recognized; unrelated adjectives, report text, plan reasons and an editor-only
thinking selection do not become worker-effort consent. A newer model request
replaces the earlier selection; a newer model refusal revokes it. Conflicting
efforts for the same model in one source refuse as ambiguous. State one exact
model-and-effort selection rather than relying on distant prose.

`routing.auto_route.user_model_request` binds the effort to the existing
`source_ref`; no plan effort field is needed. `choose` preserves it within
observed support and the configured model policy, including quality retries,
infrastructure retries and review fixes. Deep-tier `high` is an automatic
default, not a minimum floor. A model whose explicit floor is `medium` may run
at `medium` even when placed in deep/standard tiers. The runtime does not lower
floors or alter owner policy to satisfy a request.

Policy/support conflicts raise `user_requested_effort_unavailable` before
worker launch. Explicit owner effort pins likewise raise
`pinned_effort_unavailable` rather than clamp; a model-only pin retains its
automatic effort calculation. Saved automatic settings without an explicit
selection keep their existing supported-effort narrowing. The mandatory reviewer
still follows its owner pin/ranking, not a planned-node model request. To require
the same effort throughout a workload, the reviewer needs a matching owner pin.

At dispatch, the audit distinguishes `effort_source=user_request`, `pin` and
`auto_route`. Auth replacement preserves exact-effort strictness. After
session creation/model selection, missing live support or a configuration
mismatch stops the worker before any prompt instead of narrowing the explicit
effort. The native `worker_configuration_confirmed` event and recovery capsule
remain bound to that exact assignment/attempt/session. An audit or coordinator
confirmation alone never proves a specialist's actual effort.

The read-only stage preflight now checks execution and reviewer effort against
policy, refusing `selected_effort_conflicts_with_policy` even when the reviewer
pin text matches the requested route. It does **not** inspect live engine
support, grant a user request, configure a coordinator, or prove specialist
readiness. Check exact native confirmations at each stage; see
[dev preflight](../../project/dev-environment.md#route-preflight-before-a-paid-cli-run).

Critical roles require qualified instruction delivery for the exact engine
version. Dispatch excludes unregistered Claude versions before engine launch.
A non-retryable `instruction_delivery_unqualified` failure is retained as route
unavailability and fences that task's engine/version across models and
connections, rather than escalating to a different model on the same unusable
adapter. An updated qualified version does not inherit the refusal. Existing
authorization, pins and user-requested identities still apply; an unavailable
pin is refused, not substituted. See [exact Claude support](../claude-engine/index.md#current-support-0880-2026-10-08). The owner-requested Desktop run on 0.88.0 (2026-10-08) is bounded evidence, not clean qualification: Desktop evidence.

This automatic worker selection is separate from configured coordinator routes
and from the explicitly authorized fallback path described below. Legacy
mixed-team route profiles and fallback checks remain documented here for
assignments that use them.

User **owner profiles** are distinct from the per-revision `RouteProfile`
below. The owner-only `$XDG_CONFIG_HOME/cortex/routing-profiles.json` (format
2) snapshots the eight managed user-routing keys plus user defaults and owner
ceiling, MCP usage policy, and connection concurrency limits. It excludes
project routing/ceilings, MCP `command`/`args`/`env` installation definitions,
and connection credentials. Saves are explicit: save to the selected profile
or create a new one; ordinary settings writes do not automatically update a
profile. Activation materializes the snapshot into the existing user backing
files, with journaled recovery. States are `none`, `in_sync`, `modified`,
`interrupted` and `unavailable`; modified activation requires an explicit
discard choice or saving first. Format-1 routing-only profiles migrate on
first mutation with a private verified backup and remain partial until saved
as a full snapshot.

The runtime routing loader still uses first-existing-file precedence:
project, `$CORTEX_ROUTING`, then user. Thus project/environment routing
shadows an activated user profile; activation does not override precedence or
change an already-open worker route. Owner model preferences (`preferred_for`)
rank within an eligible tier ahead of unconfirmed plan hints, plan-usage heuristics and retry diversification,
but grant no route: pins, deny, automatic membership, providers, tier,
availability and quota rules remain controlling. Preferences cannot promote
a model into a different tier. See [configuration](../../configuration.md#user-routing-profiles).

The current install snapshot follows the refreshed saved default profile:
explicit OpenCode denial and a `codex-acp/gpt-6.1-sol` coordinator at medium effort.
The 2026-10-21 production routing refresh removes Opus from the `standard` tier:
its configured tiers are now `flagship` and `deep` only. Standard-tier generic
`design` and `implementation_frontend` work cannot select Opus, even with an
exact coordinator hint (`ignored:tier_mismatch`). The UI/UX wording does not
create a strict role pin or override tier eligibility.

Opus retains the `opus`/`опус` request labels and its UI/UX condition, including
frontend layout and styling (`implementation_frontend_design`); its
`preferred_for` also regains `implementation_backend` and `implementation_frontend`.
Sol's `preferred_for` gains `testing`, `investigation` and `writing`. The latest
refresh removes Sol's free-text condition (`preferred_when: null`), so its listed
preferences apply without a route hint, including on quality retries. At equal
quota and without a provider preference, unhinted standard-tier generic design
and frontend work select Sol; quota pressure can still pass over that preference.
Explicit frontend-design classification uses structured preferences on every
attempt, independently of the conditional hint, and still prefers eligible Opus
on its affinity engine. Other kinds require an exact matching hint for models
that retain a non-null `preferred_when`; Sol is no longer conditional.
See [affinity-bound frontend design](#affinity-bound-frontend-design) and the
[fresh-install snapshot](../../configuration.md#fresh-install-owner-default-snapshot).

## Affinity-bound frontend design

`implementation_frontend_design` is affinity-bound in `routing.auto_route`.
Initial attempts, quality retries, escalations and review fixes use the inherited
work kind and only the effective affinity engine (Claude in the current preset).
On that engine, eligible `preferred_for` routes remain preferred independently of
`preferred_when`, exact coordinator hints and retry diversification. Selection
searches configured eligible fallback tiers for preferred routes before accepting
a non-preferred model in a higher tier; it never invents tier membership.
The current preset has no standard-tier frontend-design preference: Opus serves
only `deep` and `flagship`. When Opus is eligible and not passed over by the
quota rule below, even an initial node classified as `light` or `standard`
selects it at `deep` through the configured fallback order (light → standard →
deep or standard → deep → light, respectively). This promotes the serving tier
without waiting for a quality retry; it does not give Opus standard-tier membership.

The preset's sole frontend-design preference is Opus. If it is unconfigured,
unavailable, automatically excluded, outside the task ceiling or exhausted,
fallback first tries another eligible route on the same affinity engine
(Claude in the preset). Only when that engine has no authorized, available,
non-exhausted route at all -- unconfigured, denied, excluded by `providers`,
not auto-selected, outside the task ceiling, or every route unavailable --
does selection widen to the normal cross-engine candidates (Sol/Astra on Codex
in the preset), on quality retries, escalations and review fixes alike.
While the affinity engine has even one eligible route, a different engine is
never chosen. The existing hot-and-meaningfully-worse quota exception operates
within whichever pool (same-engine, or the cross-engine fallback) is active.
Rank version 12 records this as `affinity.cross_engine_fallback`. Only when no
eligible route exists anywhere, or the affinity is explicitly configured
neutral, does a `no_available_route` refusal occur, even when no worker
catalog entries exist; Cortex does not inherit the coordinator route instead.
A conflicting cross-engine role pin is refused as `pinned_route_unavailable`,
not replaced. Same-engine pins remain strict.

Confirmed explicit user model requests still precede this automatic restriction.
Hard authorization, deny/providers, automatic membership, tier/effort policy,
observed availability and quota exhaustion remain controlling in their existing
paths, in whichever engine pool selection currently considers. This is a
work-kind rule using settings-defined identities, not model-name inference or
new permission. Rank version 12 records `affinity.required: true` for the
constrained automatic decision, plus `affinity.cross_engine_fallback` (`true`
only when no affinity-engine route was eligible at all). Other work kinds
retain soft affinity and ordinary conditional-hint/retry rules.

## Current preset: Haiku 5.5 and Luna

The installation preset now enables `claude-agent-acp/haiku` at full
policy parity with `codex-acp/gpt-6-luna`: light tier, testing stretch,
high/xhigh/max effort ladder, max ceiling, the same preferred work kinds,
and null request names. Its exact quota label is `seven_day_haiku`.
This alias is observed as Haiku 5.5 only in the independently inspected
SDK 0.3.293 refresh; upstream adapter 0.87.0's SDK 0.3.287 resolves it to
Haiku 4.5. See [catalog/effort evidence](../claude-engine/index.md#haiku-55-catalog-and-effort-evidence).

The old exact `claude-agent-acp/claude-haiku-4-5-20251001` stays denied,
while bare `haiku` is no longer denied. Denies remain exact identities,
not family inference. The owner explicitly adopted the same addition in
production routing and the active `default` profile after private backups.
Existing installations do not acquire the preset by runtime overlay;
observed catalog support and all hard authorization checks still apply.
No-prompt ACP configuration success does not establish paid model execution.

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
  `auto`/`auto:pin`/`auto:user_request` basis and settings-eligible observed
  candidates narrowed by the task ceiling before opening a worker session.
- **Explicit mixed-team profile** — `RouteProfile(role_routes={role:
  {route_id, ...}})`. A role present in the mapping is authorized only for
  its own listed route ids; a named specialist *absent* from the mapping is
  denied. Only the generic `worker`/`coordinator` buckets inherit the
  coordinator route when absent; an empty set
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
surface is the natural next consumer — not added here to
stay within their own scope.

The audited `model` column stays the routing id/alias (`opus[1m]`,
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

ACP selections propagate owner/project ceilings into the selected and active
task revision and keep prior task narrowing across route/effort changes.
Automatic selection first filters observed candidates by route, connection and
provider-recipient ceilings and any explicit role profile. With no explicit
profile, settings-authorized mixed-engine workers/reviewers remain valid within
the ceiling; default single-route inheritance is not applied to reject all
automatic alternates. A pinned route outside that intersection refuses rather
than falling back. The actual choice carries its exact observed route ID and
must still match one current connection/engine/model identity before capacity
reservation. Missing/stale/unmatched/ambiguous automatic identities refuse with
`automatic_route_identity_unavailable`; a missing or mismatched task revision
refuses with `dispatch_configuration_unavailable`. No coordinator-proxy check
authorizes an automatic alternate. Empty allowlists grant nothing; null/absent
sets do not restrict their layer. Automated coverage is not live qualification.

`EngineSessionManagerAdapter.open` calls `_authorize_dispatch` (via
`routing.authorization.authorize_dispatch`) **before** `_reserve_capacity_
blocking` (the atomic per-connection concurrency gate) and before
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
  tracked, has already been reached for the assignment
  (`routing.store.count_task_attempts`, counted from the audit table
  itself — every past attempt, authorized or fallback, counts).

`delegation.capacity.reserve` atomically enforces a four-worker cap per chat
session (the ACP session and task it drives), counting reservations across all
engines and connections. The connection's `max_concurrent_sessions` limit
(default 4, configurable from 1 through 64) can further restrict that session's
workers on that connection; it cannot raise the session cap or combine capacity
across sessions. Coordinator reservations are recorded but never limited: any
number of chat sessions' coordinators run at once on one connection, and they
never take a worker slot. When capacity is full, a worker waits; the scheduler leaves excess
ready nodes `pending` instead of claiming them as running.
Only `PolicyCeiling.limits['attempts']` is enforced anywhere in the runtime
(both pre-dispatch, via `authorize_dispatch`, and pre-fallback, via
`authorize_fallback` — a fallback attempt is still an attempt, counted by
the same `routing.store.count_task_attempts`). The `calls` and `tokens`
dimensions exist on `PolicyCeiling.limits` but are **not metered by
anything in this codebase**: no counter anywhere increments them, so a
ceiling naming them has no enforceable effect today. This is a precise,
intentional gap, not a silent no-op disguised as coverage. The
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
  `provider_restriction` (`configuration.policy.FALLBACK_HARD_DENYLIST`)
  can never trigger fallback, regardless of any policy.
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
attempt (`recovery.coordinator_recovery`/
`recovery.worker_reconciliation`) and rebuilding context canonically
(recovery capsules — never vendor-opaque provider memory) remain the
caller's own responsibility around this call, exactly as for any other
replacement.

### Automatic trigger: `OrchestratingRuntime._attempt_worker_route_fallback`

`acp.orchestrating_runtime.OrchestratingRuntime` wires the actual decision
point: a worker's own `EngineTurnFailed` (the classified
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
3. Reconciles the old attempt first (
   `recovery.worker_reconciliation._best_effort_terminate`/
   `_revoke_worker_channel` — best-effort process termination and trusted-
   channel revocation) and issues a **fresh worker channel** bound to the
   new attempt id (a channel is never valid for a different attempt;
   A prior finding: cross-connection fallback needs its own
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
   `_begin_generation_delivery`/`SessionRuntime.open` path (recovery
   capsule + qualified `session/new` instruction delivery, fresh for
   the *new* engine).
5. On any refusal (no policy/consent, denylisted/unknown class, exhausted
   attempt or fallback budget, unauthorized route), records exactly one
   concrete `assignment_failed:...:fallback` event
   (`fallback_refused:<code>`) and returns — **never** a second attempt on
   a third route, never a retry of the same route.

When fallback is never even evaluated (no attempt id to reconcile, or no
real configuration revision resolvable — e.g. a test double with no
configuration behind it), the caller's own plain
`engine_failure:<kind>` message is the only one written, preserving every
pre-existing failure-reporting behavior byte-for-byte.

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
`flagship`. The built-in defaults name Astra and Fable as the flagship
families; routing.json `models` can place any model in any tier (see
[Owner-configured models](#owner-configured-models-selection-tiers-and-policy)).
Flagship models are never selected for a role's default tier or because a plan merely says
`complexity: high`; high complexity tops out at `deep` (Sol/Opus class).
The flagship tier requires an explicit runtime escalation after a prior
attempt fell short, or the user's own explicit request for that model family
in the task (see [User-requested flagship route](#user-requested-flagship-route)).
A user `routing.json` pin is also an explicit route choice and remains strict.

The role tier starts from `ROLE_TIERS` and is shifted by complexity (low moves
cheaper; high moves stronger). On escalation, the runtime normally advances the tier;
`flagship` is reserved for escalation. A retry caused by an infrastructure failure
(`engine_failure`, `transport_failure`) or by a worker lost to a runtime restart or host disconnect
(`worker_lost`) does not escalate; other retries, including a retry with no recorded failure class, do. Ordinary standard-tier testing
retries and review fixes retain the standard candidate pool described below;
other standard-tier retries escalate normally. If no usable candidate exists in the
requested tier, the configured fallback order searches other tiers. Explorer
remains light at every complexity and escalation; because its tier cannot rise,
escalation raises its effort one supported step (normally low to medium; on
Luna, high to xhigh).

Models are grouped into light, standard, deep, and flagship; the owner may
place one model in several tiers (`models.<engine>.tiers` as a list), which
makes it a candidate of each and runs it at the effort of the tier it was
chosen for. An unavailable
role tier searches the nearest tier permitted by policy; flagship is never a
starting tier except for a node the user explicitly asked to run on it. Haiku is excluded from automatic selection by default, but
`auto_allow` can opt in a model or `engine/model`; `deny` and `providers` still
apply. Setup writes the default user `routing.json`, `{"deny": ["gpt-5.5",
"haiku"]}`, whose bare `haiku` token denies every Haiku id and alias
everywhere (see [configuration](../../configuration.md#routing-rules)). A pin is
strict and has priority over automatic selection.

`enabled` is the per-engine list of models Cortex may use at all (Settings >
Models). A model its engine's list leaves out is handled exactly like a
denied one by the selector, dispatch, pins and automatic routing
(`auto_route._denied`); an engine the key does not name keeps its default:
every model on, except OpenCode, whose models stay off until listed. A
refused model reports `route_denied` with a remediation naming Settings >
Models.

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

### Owner-configured models: selection, tiers and policy

Every name-keyed rule in this section -- the family and line tier tables, the
Haiku and optional-engine exclusions, the flagship effort ceiling, the Luna
rule and the Russian request spellings -- is only a built-in default
(`routing.model_policy`). The owner overrides it per engine and per exact
model id with the `models` key of `routing.json` (reference:
[configuration](../../configuration.md#routing-rules)); with no `models` key
every routing decision is the same as before rank version 8.

- **Selection.** `models.<engine>.auto` present puts that engine in LISTED
  mode: exactly the listed models are eligible for automatic routing (`[]`:
  none of that engine's models), the built-in Haiku and optional-engine
  exclusions and `auto_allow` no longer apply to it, and supersede is not
  applied there (an older selected version still runs; `superseded_by`
  stays informational). A model left out is `not_selected`
  (`auto_reason`, route-catalog `automatic`, hint status
  `ignored:auto_excluded`). `deny` and `providers` still win, and a `pin`
  or the model selector still reaches every offered model. Without `auto`
  the engine stays in BUILTIN mode. This is how an optional engine such as
  OpenCode joins automatic routing.
- **Tiers.** A route's tier is, first that applies: `models.<engine>.tiers`
  (`tier_source: configured`), the legacy engine-agnostic `tiers`
  (`legacy_tiers`), the built-in tables (`builtin`), else list position
  (`position`).
- **Policy.** `models.<engine>.policy.<model>` overlays the built-in
  attributes key by key; `null` removes a built-in value:
  `effort_floor`, `max_effort` (without one, a model of the effective
  `flagship` tier stops at `high`), `effort_ladder` (1-3 ascending efforts
  for light work, standard work and standard work that fell short once; a
  ladder holding `max` permits `max` outside the light tier),
  `also_standard_for` (work kinds for which a light model is also a
  standard-tier candidate: the stretch rule below), `preferred_for` (work
  kinds the owner prefers this model for: rank key `purpose` below),
  `preferred_when` (an owner-written condition that makes that preference
  conditional on the coordinator naming the route; see `purpose` below) and
  `request_names` (extra spellings for the
  [user-requested route](#user-requested-flagship-route); a request,
  work, means or negation word is refused).

`list_route_options` is the coordinator's only source of model names: every
route carries `tier_source`, `selection`, `auto_reason` and its effective
`policy` with each attribute's source; the result adds `affinity` (the
effective engine per work kind, or `null` when it has no auto-eligible route),
`affinity_not_eligible` (the withheld engines), `affinity_source` (`routing.json` or `builtin`
per kind), `role_work_kinds`, `preferred` (only auto-eligible routes per
`preferred_for` work kind, without a `preferred_when` condition),
`preferred_when` (auto-eligible route to its `when` text and `work_kinds`), `flagship` (`routes`, `request_names`, `rule`) and
`stretch` (each light model with a stretch rule: `effort_floor`, `efforts` by
work, `standard_work_kinds`, `standard_candidate_roles`). The `plan_submit`
node schema and the orchestrator's `worker-routing` reference name no model
or engine preference; `tests/test_no_hardcoded_model_names.py` guards that.
An exact hint may name a route whose model id holds `/` or `,`
(`opencode/<provider>/<model>`; a model id can also contain `,`).

### Per-node implementation classification

`plan_submit.plan.tasks[].work_kind` optionally overrides the role default with
a current work-kind enum. The three implementation ids are
`implementation_backend`, `implementation_frontend` (no visual design), and
`implementation_frontend_design` (layout, styling, markup, responsive layout,
UI components and visual polish). Backend/data/refactor/fullstack/general roles
default to backend; frontend/mobile/accessibility-fixer roles default to frontend
without design. Classification affects preference ranking, affinity and stretch
eligibility, not authorization or tier. It is persisted in plan JSON and survives
retry/fix derivation; absent an override, the actual fix role supplies the default.
Plan hint previews and route audits report the effective classification.

The default preset places Astra only in flagship and lists frontend-with-design
only in Opus's `preferred_for`. Opus's conditional preference still requires
the coordinator's exact route hint, and quality escalation still ignores such
conditions. This is not a strict ban on other models, nor an automatic promotion
from the light tier. Existing version-2 files and profiles accept legacy
`implementation` preferences, stretch allowances and affinity: effective reads
expand it to all three new kinds without rewriting saved files. Editable source
rules remain unexpanded so unrelated Settings saves preserve legacy entries.
Explicit new-kind affinity overrides the legacy umbrella.

### User-requested flagship route

The historical heading is retained for links; this rule now applies to **every
configured tier**, not just flagship. The coordinator must name the exact model
on each covered node. Model-authored text is never consent. Authorization
comes only from user-authored text the runtime itself recorded for the same
task (`routing.auto_route.user_model_request`):

- every `source_revisions` body of the task -- the initial request and each
  later user message or steer the host delivered (`list_sources` entries,
  `trust_label: user_direct`); and
- the verbatim answer of every question a real user answer resolved
  (`<question_id>:answer`), never the question text, which the asking model
  wrote.

Coordinator prose, model reasons, question text, reports and worker output are
never read for consent. A source must request the complete, case-sensitive
configured engine/model identity, a bare model ID unique across enabled configured
engines, or an owner-configured request label.
No model family or version is inferred from the ID. Paired quotes separate
opaque IDs from prose punctuation (see [settings-only routing](#settings-only-routing)).
Owner-configured labels keep the conservative request/negation grammar below.
Enabled exact identities also support the broader owner-naming forms documented
in [settings-only routing](#settings-only-routing). For labels, in the same sentence:

- within the four words before it, either an explicit request word stands
  (`REQUEST_WORDS`: `use`, `run`, `switch`, `prefer`, `choose`, `request`,
  `requirement`, `используй`, `запусти`, `переключи`, `передавай`,
  `поручи`, `пусть`, `требование`, ... -- "use Astra", "run it on Astra", "switch the review to
  Fable", "используй астру", "требование использования астры"), or a work
  verb (`WORK_VERBS`: `implement`, `review`, `fix`, `сделай`, `реализуй`,
  `проверь`, ...) governs a preposition of means (`MEANS_WORDS`: `on`, `via`,
  `through`, `using`, `на`, `через`, `с помощью`, ...) that stands directly
  before the family, with at most an article or "model" between
  ("implement it on Astra", "Реализуй на Астре", "сделай на модели
  Астра"); and
- that word is in a request form: an English verb opens its clause or
  follows a request-mood word (`please`, `to`, `could you`, `let's`, `then`,
  ...; a verb of choice may also follow `I`/`we`: "I prefer Astra"), and a
  Russian verb is one of its listed imperative, infinitive or first-person
  plural forms and does not follow a past tense, or its past tense after
  `чтобы`/`чтоб`/`что бы` in the same clause (the Russian subjunctive:
  "хочу, чтобы ты сделал аудит через Опус", "чтобы ты использовал Астру")
  when no negation stands anywhere earlier in the sentence ("не хочу, чтобы
  ты сделал это через Опус" is refused); and
- no negation, comparison or substitution word stands within the five words
  before it (`NEGATION_WORDS`: `not`, `never`, `without`, `don't`, `can't`,
  `instead`, `rather`, `than`, `over`, `compare`, `based`, `не`, `без`,
  `никогда`, `вместо`, `чем`, `сравни`, ... -- "don't use Astra", "prefer
  the default route over Astra").

A bare mention grants nothing: a comparison ("compare Astra with Fable"),
an availability remark ("Astra is unavailable; use the default route"), a
quotation or report ("the last report said Astra found nothing") or a
preference for the default route. Neither does a preposition that no request
or work verb governs ("it failed on Astra", "посмотри на Астру", "sent via
Fable", "ревью через Фейбл"), nor a request word used as a noun, a
statement or a report ("the last run on Astra failed", "the tests run on
Astra", "we tried to run it on Astra", "I use Astra daily", "запустили на
Астре", "сделано на Астре", "пытались запустить на Астре",
"использование Астры"). This is a deterministic word match, not an
interpretation, and it errs towards refusing: a request phrased without an
explicit request word or a governing verb ("I want Astra", "Модель: Астра",
"ревью через Фейбл", "let Astra do it", "если запустить на Астре") is
not recognised, the node stays on its ordinary tier, and the receipt shows
`user_model_request.status: not_requested_by_user`, so the user rephrases
("use Astra", "используй Астру"). Conversely, a statement the rule cannot
tell from a request still grants (for example "the plan requires Astra"),
which is why the coordinator's exact hint remains a required second step.
At dispatch, choose runs the exact requested route at its configured tier,
including for non-flagship, light/stretch, high-complexity, retry and fix nodes.
Pins, owner preferred_for/preferred_when ranking, automatic membership,
automatic role tiers and usage at 95–99.999% cannot replace it.
Supported effort and explicit per-model effort constraints still apply.
The audit records basis auto:user_request, decided_by user_request and
user_model_request = {source_ref, family, matched, status: honoured}; the
historical family field holds the exact identity.

Hard deny/disabled/provider/route-profile/recipient authorization, a missing
observed identity or tier/effort policy, engine unavailability and 100% exhausted
quota refuse with user_requested_route_unavailable and a concrete corrective
reason before dispatch. There is no ordinary-tier substitution. Failure
fallback also refuses a different model identity; an authorized alternate
connection offering the same exact engine/model can still qualify.
Plan submission previews pending_dispatch:user_request with the user source
reference for every tier. Only covered exact-route nodes receive this priority;
other nodes retain automatic selection. Without user consent, flagship remains
escalation-only.

### Configured stretch policy

A stretch model is any owner-configured light-tier model with an
`effort_ladder` or `also_standard_for` policy. No model identity receives
this rule by name. The installation template is owner-editable data, not
a built-in fallback. Setting the attributes to `null` removes them.

- **Effort floor and ladder.** `effort_floor` applies to pins and worker
  dispatch as well as automatic selection. `stretch_effort` uses the
  configured ladder for light work, standard work and escalated standard
  work. For example, a ladder of `high`, `xhigh`, `max` uses those
  steps respectively; escalated light work advances one step. Required
  unsupported steps fail closed rather than silently inventing support.
- **Candidacy.** `stretch_status` records `light_tier`,
  `standard_candidate`, `not_candidate:work_kind_<kind>` or
  `excluded:<reason>`. Standard candidacy requires the work kind in the
  model's configured `also_standard_for`. Normal standard-tier retries
  of those kinds retain the standard pool before generic escalation.
  Ranking, hints, preferences, quota safety and retry diversification still
  apply; stretch candidacy does not guarantee selection.
- **Exclusions.** For ordinary automatic routing, high complexity, deep/flagship work and deep roles
  exclude light stretch candidates, even as a fallback. A model configured with several native tiers (for example light and standard) keeps its other tiers when its stretch promotion is excluded; only a light-only model is denied outright. When no permitted
  route remains, dispatch refuses with `no_available_route`.
- **Audits.** New decisions emit `stretch` (`status`, `candidate`,
  `chosen`, `outranked_by`; pins emit `status` and `chosen`).
  Ladder-derived effort uses `stretch:<tier>` or
  `stretch:<tier>:escalated`. These names do not depend on the configured
  model. The former `luna` key and `luna:<tier>` effort labels are not
  re-emitted. Stored audits/events/reports are not rewritten; historical
  rows remain readable by the existing model/effort/tier summary renderer,
  which does not depend on either policy key.

An exact stretch-model hint serves standard work only when the configured
work-kind allowance permits it, both at plan submission and dispatch.

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
considered only within the runtime-computed tier (except a
[user-requested route](#user-requested-flagship-route)) and cannot
override pin, deny, providers, availability, or the 100% quota exclusion. A hint from another tier is recorded as `ignored:tier_mismatch`;
using `auto` delegates all selection to ranking. Legacy stored plans without
`model` retain their old `provider` preference behavior; new plans use `model`.

On escalation, the previous hint is not inherited (a
[user-requested route](#user-requested-flagship-route) is the
exception). The runtime ranks the new
tier and prefers a different engine from the failed attempt when candidates
otherwise tie. A hint is honoured unless the hinted route is excluded,
unavailable or exhausted, or its usage is both at or above 95%
(`HINT_TOLERANCE`, equal to `HOT_PERCENT`) and meaningfully worse -- by more
than `USAGE_MARGIN` (10) points -- than an available alternative's known
usage (`ignored:near_quota_limit`). Crossing 95% alone, with no known
alternative or no meaningful gap, no longer disqualifies a hint (rank
version 10). How often an engine was chosen
before never affects a hint. Other hint statuses include
`ignored:provider_not_allowed`, `ignored:not_offered`,
`ignored:tier_mismatch`, `ignored:superseded`, `ignored:auto_excluded`, `ignored:no_candidate_in_tier`,
`ignored:unavailable`, `ignored:escalated`, `ignored:owner_preferred`, and
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
   when the preferred route's usage is both at or above 95% and meaningfully
   worse -- by more than `USAGE_MARGIN` (10) points -- than an available
   alternative's known usage).
2. Purpose (rank version 9): a model whose `routing.json` `models` policy
   lists the role's work kind under `preferred_for` -- the owner's choice --
   ranks ahead of a new plan's hint, plan usage and retry diversification.
   It never moves a tier or bypasses exclusions. Like a hint, it is passed
   over for quota only when hot (at least 95%) and more than 10 points worse
   than a known usable alternative. The audit records `purpose.status` as
   `none`, `honoured` or `ignored:near_quota_limit`. A conflicting hint loses
   with `ignored:owner_preferred`; coordinators use `auto` when the menu lists
   owner preferences for the work kind. A model with `preferred_when` counts
   as preferred only on a node whose hint names its exact route (the
   coordinator's judgement that the owner's condition holds) and, when its
   `preferred_for` is non-empty, only for those kinds; never on escalation.
   Affinity-bound frontend design is the exception described above: structured
   preferences survive escalation, and the candidate pool is restricted before
   this within-tier ordering. Legacy provider preference remains first for other
   pre-model stored plans.
3. Honoured model hint for a new plan (see above), ignored on the same
   hot-and-meaningfully-worse basis.
4. Quota band (rank version 11): a candidate whose known usage is at or above 95%
   (HOT_PERCENT) ranks behind every candidate that is not hot, whether that candidate's usage
   is known or unknown. The criterion sits here, ahead of the usage-gap band, diversification
   and affinity, so no affinity or diversification preference moves a hot route ahead of a
   non-hot one. Unknown usage is never treated as hot.
5. Plan usage, whenever every candidate's usage is known, at any level
   (rank version 13). Each full 10-point USAGE_MARGIN gap above the
   least-used candidate ranks it one step behind, so work goes to the plan
   with meaningfully more quota left. A smaller gap leaves the choice to the
   steps below. Rank versions 10 to 12 applied this step only once some
   candidate reached 95%. Since rank version 14 the step never reaches
   past the owner's preferred models for the work kind (step 2,
   `preferred_for`): while that preference is honoured, usage compares only
   those models with each other (`usage.scope` `preferred`). When the tier
   has such models but none is honoured -- all are hot and meaningfully
   worse, exhausted or unavailable -- usage ranks nothing (`usage.scope`
   `none`, status `not_applied:owner_preferred_unavailable`), and the
   replacement is chosen by the quota band and steps 6, 7 and 9. Without
   preferred models usage compares every candidate (`usage.scope` `all`). If
   applicable owner preferences remain configured but their preferred routes
   disappear from the observed catalog or are filtered out by hard eligibility,
   usage still ranks nothing (`usage.scope` `none`); fallback uses the normal
   quota, usage-gap, diversification, affinity and engine-order keys. This does
   not make an unavailable or ineligible preferred route executable. If no
   applicable preference is configured, usage ranking across candidates remains
   available.
6. On escalation, an engine other than the one whose attempt fell short --
   unless step 4 already sent the work to a meaningfully less-used plan,
   diversification does not retry onto a plan with meaningfully less room.
7. Engine affinity (effective work-kind suitability). Built-in defaults
   (`WORK_KIND_ENGINES`): backend implementation, frontend without design and
   operations lean to `codex-acp`; frontend implementation with UI design,
   investigation, review, design, writing, planning and analysis lean to
   `claude-agent-acp`; debugging and testing are neutral. `routing.json`
   `engine_affinity` overrides it per work kind (see
   [configuration](../../configuration.md#routing-rules)).
8. Exact used percentage, within step 5's usage scope.
9. Stable engine order.

`decided_by` names the first of these keys (`preference`, `purpose`, `hint`, `quota`, `usage`,
`diversify`, `affinity`, `used_percent`, `engine_order`) that
separated the chosen candidate from the runner-up, or `only_candidate`; a pin
records `pin`. It is never `balance`. The runner-up can be another model on
the same engine, in which case `engine_order` is the usual answer.

Unknown quota is not inferred to exceed the threshold. The audit's usage
status is `unavailable` when any candidate in the usage scope has unknown
usage, `not_applied:owner_preferred_unavailable` when the scope is `none`,
and `active` otherwise. Audits from rank versions 10 to 12 may also show `below_threshold`.
The audit keeps the `threshold` field (the 95% hot band of step 4), the
`active` and `compared` fields, and the raw candidate values. Steps 5 and 8
compare nothing while usage is unavailable. Step 4 applies whatever the usage status: a known-hot route loses to
any non-hot route, including one whose usage is unknown. That does not claim the unknown
route's percentage was observed.

Effort is derived from the runtime-selected tier, task complexity/escalation,
and each model's supported effort list. `effort_basis` records whether it came
from the role tier, escalation step, a stretch model's effort ladder (the
configured stretch rule; recorded as `stretch:<tier>` or `stretch:<tier>:escalated`), or a pin. A pin may explicitly name an
effort; otherwise the runtime chooses a supported effort. An unconfirmed model hint cannot
raise tier or effort; a confirmed user request selects the requested model's configured tier.

The read-only `list_route_options` tool reports currently engine-confirmed
routes, tier and tier source, supported effort, policy, automatic eligibility,
availability and quota band, plus role tiers for low/normal/high complexity
and the `affinity`, `role_work_kinds`, `preferred`, `flagship` and `stretch`
summaries (see
[Owner-configured models](#owner-configured-models-selection-tiers-and-policy));
the former `luna` and `flagship_note` fields are replaced by `stretch` and
`flagship`. It is advisory and may become stale; the
runtime validates the choice at plan submission and dispatch. The server
advertises `listChanged: false`, so clients must call the menu again when they
need a fresh snapshot; there is no push refresh contract.

Audit rows retain the decision under `decision_detail.route_decision`,
including `rank_version`, `role_tier`, tier and effort basis, `model_hint`,
`model_hint_status`, `model_hint_reason`, `diversify_from`, `usage`
(`compared`, `margin`, `least_used_percent`), `affinity` (work kind, engine,
`default` or `routing.json` source, and `required: true` for affinity-bound work), candidates with `used_percent`, `hot`,
`quota_state`, `usage_band`, `suitability` and `preferred`, `decided_by`,
`excluded_by_policy`, `unavailable`, `superseded`, `purpose` (`work_kind`
and the `preferred` candidates, plus `status` since rank version 9), `stretch` (`status`,
`candidate`, `chosen`, and `outranked_by`, the rank key that put the best stretch
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
and review fixes escalate as before version 5; 8 takes tiers, automatic
selection and per-model policy from `routing.json` `models` before the
built-in defaults, skips supersede on LISTED engines and adds the `purpose`
key between `diversify` and `affinity` (no decision changes without
`models`); 9 moves `purpose` just after legacy `preference`, ahead of hints,
usage and diversification, and adds the hot-and-meaningfully-worse safety
exception for owner preferences (no selection change without `preferred_for`). Rank version 10 gates usage bands and exact-percentage comparison at observed usage >=95% and prioritizes confirmed user model requests across every tier; 11 moves the hot-quota criterion back to right after `hint`, ahead of usage bands, retry diversification and engine affinity, so a known route at or above 95% always ranks behind any non-hot route, known or unknown. Rank version 13 removes the 95% gate from usage bands and exact-percentage comparison: whenever every candidate's usage is known, a plan at least 10 points less used ranks first. Rank version 14 confines that comparison to the owner's honoured preferred models for the work kind and applies none once those models are used up.
Rank version 12 adds affinity-bound frontend-design candidate restriction and
structured preference preservation across retries and eligible fallback tiers.
Older rows keep their own fields
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

### Owner model requests in plain words

Besides the request grammar above, a model made responsible for a role or stage in an
owner message is a request: "за аудит отвечает Opus", "чтобы за audit отвечал opus",
"Opus is responsible for the security review" (`auto_route.assigns_family`). A past form
("отвечал") counts only after "чтобы"/"пусть"; a remark ("Opus wrote the last review")
and a refused or negated mention assign nothing. An enabled model without configured
`request_names` gets its unique last id word as a label (`claude-sonnet-5` → `sonnet`,
`gpt-6.1-sol` → `sol`, `gpt-6-luna` → `luna`; `auto_route.derived_request_names`) when no
other enabled model shares it and it is not a generic word; it matches only as a
standalone word, so `gpt-6-luna.v2` or `vendor/gpt-6-luna` stay other identities. An
audit the owner assigned to Opus once ran on Sol because neither rule existed
(2026-10-22).

Only the owner's own words are read. In a host-imported `<chat-history-summary>`
only `[User]` turns count (for a "→ Answer:" line only the answer), so an earlier
assistant or runtime line such as "запущен Sol вместо Opus" is not a refusal
(`auto_route._user_authored`). Emphasis that forbids substitution ("без замены другой
моделью", "without substitution", "no substitutes") is stripped before matching, so it
confirms the request instead of reading as a negation. The split subjunctive "что бы"
counts like "чтобы". When the coordinator's `model_reason` says the user or owner
requires the hinted model and the owner's messages mention that model without refusing
it, the hint is honoured as a user request (`auto_route.user_mentions_model`, basis
`mentioned`). A security audit the owner assigned to Opus that way ran on another model
until these rules existed (2026-10-22).

A variant suffix is not part of a model's name: without configured `request_names`,
"opus" names `opus[1m]` (a one-word id without a suffix, such as `haiku`, is matched as
itself). "делает/выполняет/проводит" and "does/performs" assign a model to a role or
stage like "отвечает". A qualified audit ("security-аудит", "аудит безопасности",
"accessibility audit") names only its specialist, not the code reviewer; an unqualified
"аудит"/"audit" still covers review and both auditors. All three were found in a live
desktop check on 2026-10-22, where a dev profile without request names ran the
owner-assigned Opus audit on Sonnet. A worker card and the worker page show a route the
owner asked for as "by your request".

When the router still ignores a coordinator's model hint, the runtime says so once per
attempt in the chat: which model was asked for, which runs and why
(`orchestration.route_notices`, recorded as `MODEL_HINT_IGNORED_SHOWN`).

## Automatic worker routing: unavailable routes

Automatic worker routing (`routing.auto_route.choose`, called from
`EngineSessionManagerAdapter._resolve_runtime_route` for every worker attempt
that no mixed-team profile or fallback has already routed) skips any route
that `routing.availability` records as unavailable. This is separate from the
fallback above: nothing here re-opens a *running* attempt on another
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
  local `/usage` command (see `docs/features/token-and-plan-usage/index.md`'s "Claude
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
- A `fallback_refused:<code>` failure is not retried
  (`worker_failure_not_retryable`), except when its `failure_class` is
  `capacity_unavailable`: a worker whose capacity wait ended and found no
  alternate route. The same route can serve that worker again once a slot
  frees, so it is retried as an ordinary retry, bounded by
  `MAX_ATTEMPT_RETRIES`.
- A failure whose attempt has a `ROUTE_UNAVAILABLE` record (matched by attempt
  id, not by error text) is retried with `route_failure=true` on the
  `ASSIGNMENT_RETRIED` event. Such a retry:
  - does not use up the two ordinary retries (`MAX_ATTEMPT_RETRIES`);
  - does not escalate the model tier (`orchestration.state.attempt_routing`),
    because no model fell short;
  - is bounded by `MAX_ROUTE_RETRIES` (6).
- A retry whose failure class is `engine_failure`, `transport_failure` or
  `worker_lost` and whose attempt has no `ROUTE_UNAVAILABLE` record does not
  use up `MAX_ATTEMPT_RETRIES`. Such a retry waits a growing delay of at most
  60 s, and retries stop only after failures have kept repeating for 10
  minutes (see [transient engine failures](../failures-and-cancellation/index.md#transient-engine-failures)).
- A retry whose failure class is `engine_failure` or `transport_failure` (a crash,
  EOF or poisoned session) does not raise the model tier either. Only quality
  causes, or retries with no failure class, escalate, as before
  (`orchestration.state.attempt_routing`, `_INFRASTRUCTURE_RETRY_CLASSES`).

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
  proposal.
- Refusal/cancel/authorization-loss/provider-limit can never reach
  `authorize_fallback`'s success path (hard denylist, checked first,
  unconditionally).
- An already-open attempt never re-resolves its route: `open` records the
  configuration revision/route it was actually launched with once, at
  launch time; a later coordinator selection/profile change only ever
  affects the *next* attempt's resolution (`resolve_worker`/
  `authorize_dispatch`, evaluated fresh per attempt), never a running
  one's already-recorded context (the attempt_effective_context
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

## Settings-only routing

Routing decisions consume only effective settings and current engine-confirmed catalogs; first-existing project, $CORTEX_ROUTING, then user precedence remains non-merging. The required version-2 marker binds exact per-engine enabled/automatic identities, tier placement and optional policy attributes. Absence or invalidity is a visible migration/repair error, never a reason to use built-in model names, families, list position, or the installation template at runtime. Unlisted and renamed identities are not automatically eligible. A coordinator recommendation is limited to options generated from the same effective settings and current catalog; final dispatch revalidates eligibility. Preferences rank only inside eligibility. Role tier/complexity fallback is generic policy and cannot infer a model's quality from its name.

An explicit user request authorizes the complete, case-sensitive configured
`engine/model` identity, including provider nesting and punctuation in the ID.
A bare model token also matches when that exact model ID belongs to exactly one
enabled engine in the effective settings; no built-in family or name inference
is used. A shared bare ID grants neither engine. The plan preview retains an
ambiguity diagnostic asking the coordinator to obtain an explicit `engine/model`;
a unique bare match is explained as naming the model without its engine.

Whole whitespace-delimited tokens and isolated paired double or single quotes,
backticks, `«…»` or `“…”` are supported. One terminal prose punctuation character
(`. , ! ? ; :`) is accepted for a recorded request, including an unquoted
`Use engine/unit.`, unless the longer token itself names another enabled
configured ID (for example `unit.`). In that case it is identity data, never
consent for `unit`. Quote the intended exact ID when punctuation is ambiguous.
A request for `engine/unit.v2`, `engine/unit-extended` or `engine/unit[wide]`
cannot authorize `engine/unit`. Prefixes, suffixes, case changes and partial
provider paths grant nothing for a different identity.

For enabled exact identities (qualified or bare-unique), a means preposition
directly before the token (`на`, `on`, `via`, `with`) is a request, including
`Часть 2 на gpt-6-luna:` and `Part 2 with codex-acp/gpt-6-luna:`.
A request-form verb anywhere in the same retained user message governs every
exact identity in it, even when the model list precedes the request or spans
sentences. These deliberate owner-naming rules can include an exact token in
an otherwise descriptive sentence; there is no inferred model-family grant.
Negation/comparison/substitution guards apply around each mention. A refusal,
unavailability or cost word anywhere in the mention's own clause blocks a grant
(either side of the mention); substitution or stop words ("instead", "replace",
"stop", "except"; Russian equivalents) block only when they come before it. The
governing request verb must sit in the same clause as the mention. Message-wide
governance of a model list applies only to an identity that stands alone in its
clause, such as a listing item. A refusal of a model in a newer user message
revokes an earlier grant for that identity (the recorded `user_model_request`
status becomes `revoked_by_user`); a comparison or remark declines a grant without revoking one.
A negated request verb does not govern a later list. A verb in a different user
message cannot authorize a bare list.

Separately configured `request_names` retain their existing case-insensitive
label/ending and conservative local request matching; no label is inferred from
an ID, and message-wide/direct-preposition grants do not apply to labels.
Only retained user sources and user answers supply grants, never plan hints,
questions or model output; source references and exact matched text are retained. A grant does not
bypass enabled/deny/provider, observed-catalog, availability, hard authorization
and exhausted-quota checks. Confirmed user requests override automatic membership
and role tiers, but never those hard checks. These current identity rules
supersede the historical family recognition examples above.

The public policy/model-settings-default.json is validated install-only data. Fresh installation creates only missing owner settings without replacement. Existing settings/profiles require explicit preview/apply migration against exact checked revisions and profile-specific candidate data; see [model-policy migration](../../project/packaging.md#model-policy-installation-and-explicit-migration). Unknown or ambiguous portable coordinator bindings remain unavailable diagnostics; no substitute account or route is selected.


### Settings-only quota applicability

Model-window associations are optional `quota_windows` settings, never a built-in family/version substring table. Exact per-engine labels map to `models: null` (shared), exact model lists, or an explicit empty exclusion list; duplicate labels are refused. Unmapped windows conservatively apply to every model, including exhausted windows; no numeric evidence remains unknown rather than zero. The same effective first-existing routing file governs route menus, automatic selection and failure reset lookup. Invalid reset policy cannot erase a genuine engine refusal: availability retains its bounded default expiry. Provider time/cache/authentication algorithms are unchanged; ranking compares plan usage whenever every candidate's usage is known. See [exact field, diagnostics and migration semantics](../../configuration.md#exact-quota-window-associations). The route menu includes `quota_window_policy` with effective rules and its conservative-unmapped decision, while raw status remains provider evidence.
