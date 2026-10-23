## 2026-10-14 orchestration audit decisions

An owner-approved audit of the orchestration prompts and logic produced these
decisions. Each one states the owner's choice and what the code does now. Where
the implementation is partial, the entry says so.

### Read-only roles and the command host profile

Read-only roles (reviewer, auditor, explorer, architect) never receive a host-writable
`command_run`. Their commands run through `sandbox.readonly_command`, in every access
mode, with an explicit allowlist of writable home caches (`READ_ONLY_HOME_CACHES`),
the scratch directory, the host `/tmp`, and a private PID namespace. The receipt profile is
`read_only`, which is distinct from the Guarded writer profile `restricted` and the host-writable
`development` profile. Outside-project working directories stay allowed in Development and Auto,
and refused in Guarded, as before. The earlier entry below ("read-only roles, review,
verification ... unchanged in every mode") described the intended restriction; this entry
records what `command_run` now enforces.

Automatic runtime verification (`controller._verification_command`) uses the same
`sandbox.readonly_command` profile in every access mode, and its receipts record
`read_only`. Inside a check only the project's git-ignored directories are writable; the
source and the protected control plane stay read-only. The reviewer reuse rule
(`reviewer_verification.reuse`) reuses only a check that ran in that read-only profile. The
synthesis gap check (`state.synthesis_checks`) accepts a declared-command gap closed by a
pass in the read-only profile, in the task mode's own command profile, or in the Guarded writer
profile (`restricted`, which also names read-only runs recorded before the read-only profile had
its own name). So in Development and Auto a writing worker's host-writable pass with the same argv
and cwd can close it. Once a task is Guarded, a host-writable pass never closes a gap. The
read-only profile limits writes, not network egress: as with every owner command, the host network
is shared. Writes also remain reachable through the host `/tmp`, the session D-Bus and systemd user
manager, the host `/dev` and a Docker socket (see the permissions page).

The owner chose restoring the read-only intent over rewriting it, on the condition that
nothing breaks. Cache writes stay possible through the allowlist, so ordinary package
manager and test runs keep working. Residual risk: a read-only command can still poison
a cache that a later run reads. This narrows exposure; it is not hostile-code isolation.

### Runtime stop question: Finish and Continue

On the runtime's own stop question, **Finish** ends the task `CANCELLED` with a
`USER_FORCE_FINISHED` event. The question is recognised by a structured runtime marker, not by
its wording, so a coordinator question worded the same way never force-finishes the task. Its
Finish option is recognised by number, by stored text, by the rendered English or Russian text,
and by its label. **Continue** resumes the runtime flow and never
releases a direct answer. Only an answer in the user's own words releases the question.

An answer that closes planned work is certified before it becomes `COMPLETED`. If a work
gate (review or verification) did not pass, the task ends `FAILED` with blocker
`answer_not_certified:<gap>` and an `ANSWER_NOT_CERTIFIED` event. Informational answers,
and follow-up answers after a reopened completed task, are unchanged.

### User revocation of a model grant

A newer user message that refuses the model, or a refusal that is newer than the grant,
revokes the grant within the task. The routing audit then records
`user_model_request.status = revoked_by_user`. A revocation is never treated as consent.
Comparisons and remarks ("compare X with Y") decline a grant but do not revoke one.
The live dispatch path carries the revocation to the recorded route decision.

### No escalation after infrastructure failures

A retry whose failure class is `engine_failure` or `transport_failure` (crash, EOF,
poisoned session), or `worker_lost` (a worker that vanished in a runtime restart or a host
disconnect), does not raise the model tier. Only quality causes, or retries recorded with no
failure class, escalate as before. No owner setting that would allow flagship escalation
after such a failure exists; it is not implemented.

### Quota ranking with a hot account

A known candidate at or above 95% of its plan usage ranks behind any non-hot candidate,
whether that candidate's usage is known or unknown. This happens before engine affinity.
The rank key moved the hot flag to position four, and `RANK_VERSION` is 11. Rejected
quota windows without a reset time expire after a bounded TTL, and a CLI window that stops
being reported is pruned on the next successful refresh.

### Plan usage ranks at any level

The owner restored usage-based ranking on 2026-10-15. Whenever every
candidate's plan usage is known, a plan at least 10 points less used ranks
ahead within the tier, before engine affinity. Rank versions 10 to 12 applied
this only once some candidate reached 95%. Owner preferences, honoured hints
and the 95% hot band still come first. `RANK_VERSION` is 13.

### Plan usage stays within the owner's preferred models

The owner decided on 2026-10-16 that usage-based ranking must not override
models chosen for a kind of work in the settings (`preferred_for`). While
that preference is honoured, plan usage compares only those models with each
other. Once they are used up (hot and meaningfully worse, exhausted or
unavailable), the replacement is chosen by the quota band, retry
diversification, engine affinity and engine order, not by usage.
`RANK_VERSION` is 14.

### Paseo cancel followed by a prompt

In a Paseo session, `session/cancel` followed by `session/prompt` on the same session is
steering, not Stop. The cancelled coordinator turn ends at once. Running workers continue,
and the new prompt is applied as steering. A lone cancel still pauses the task, but only
after a grace window of 5 seconds (`DEFAULT_HOST_CANCEL_GRACE_SECONDS`) with no follow-up
prompt. Non-Paseo clients keep the immediate pause. A user who wants Stop sends a lone
cancel and waits out the window. The pause notice is shown on the next turn, because the cancelled
turn can no longer emit updates. A failed deferred pause leaves a `host_cancel_pause_failed`
progress event. A prompt that answers a question on the held session also resolves the hold, so
it is steering, not a later pause.

### Natural-language deny-write

Only the explicit form `deny-write: <path>` (bare, single-quoted, double-quoted or
backtick-quoted) creates a write denial. Wildcard or mask characters are not stored in new
entries. A denial that an earlier runtime stored with a wildcard still guards writes, matched as
a glob. Natural
language such as "do not modify ..." creates no denial. A denial is stored in its
normalized path form, and a write is matched after the same normalization, so a `./`
spelling cannot bypass it.

### Senior consultation is escalation-only

A senior consultant is reached only through `escalation_request`. A plan that names the
consultant is refused with `consultant_requires_escalation`. The runtime builds the
consultant's packet, from the reports the escalation names and then the reports of the
escalated revision, with each report's SHA-256 and recorded artifact versions. The
consultant has no `access_request`, `ask_question`, `replan_request`, `scope_request`
or `escalation_request` route: these are withheld and refused by dispatch. It cannot
consume the replan budget or request a recursive consultation. It records a missing
fact as a limitation in its report and asks the user nothing.

## 2026-10-14 fix-node roles and own-probe retraction

The owner asked for the runtime defects found after the audit to be fixed directly. Two
runtime behaviours changed. Each entry states the rule now enforced and its limits.

### Fix nodes choose a writing role

A fix node used to keep the role of the node it repaired, even when that role cannot
write files. Now a read-only source role is replaced by a writing specialist chosen from
the original node's effective scope: `worker-technical-writer` for documentation-only
scopes, `worker-qa-engineer` for tests-only scopes, and `worker-backend-dev` for any other
scope. A writing role is kept. The fix keeps the effective scope exactly, so a finding
cannot widen it. The rule is `fix_role` in `orchestration/state.py`, applied when the fix
node is recorded after a REQUEST_CHANGES review, for targeted and fallback fixes alike.

### Own-probe errors are retractable; outside commands have a full identity

A failed check that came from the worker's own standalone Python probe (a syntax error,
or a regex or assertion error from a literal pattern) can be retracted only by a later
verified passing correction with the same name, operation, cwd and profile. The failure
remains as history with a runtime-derived `retracted_by`; the current outcome is
`retracted`, never `passed`. Every other failure keeps the rule that only an exact rerun
with the same argv (and cwd, for outside commands) closes it. Test, lint, build and
runtime-verification failures cannot be retracted.

An `outside_command` identity now also includes its profile (`outside_confirmed`), so an
outside failure closes only on an exact outside rerun, and a later unrelated run with a
recorded identity no longer makes that rerun ambiguous. A later unrecorded run still does.

Limits: the retraction is a narrow runtime check of receipts. The reason is the worker's
claim, which independent review evaluates; the runtime does not judge the product. Historical
receipts without an argv (or without a cwd for an outside command) still have no identity
and fail closed. No identity is invented for them.

## 2026-10-13 access modes as trust levels

The owner defined the three modes as trust levels over two axes, where a
writing worker may work and whether a deletion waits for the user: Guarded is
the project with consent for deletion and every outside action; Development is
the whole computer with consent for deletion; Auto is the whole computer with
no question. Auto therefore uses the Development host-writable command profile
(it previously kept restricted mounts, so its automatic approval could not reach
anything the restricted sandbox refused without a card). In Development an
`outside_write` is routine except to a protected control-plane path, which keeps
its card; `outside_command` runs outside every sandbox mask and keeps its card
in Development. Read-only roles, review, verification, engine jails, `.cortex`
and `.git` configuration/hooks protection are unchanged in every mode. The
deletion classifier remains lexical, not a semantic guarantee.

## 2026-10-12 brokered Development command capability

Use Development as the durable fresh-ACP-session default, not Auto. Ordinary
writing-worker commands use host-writable resources through the existing host-side
broker, while engines, read-only/review commands and runtime verification retain
their existing jails. Channel/epoch/role/revocation, owner denials, review and
completion remain runtime decisions. Declared deletion and exceptional actions
retain consent; capability does not authorize hazardous/production/publication
effects. Lexical command classification cannot enforce arbitrary program intent.

Full existing Docker-daemon access and arbitrary owner code are incompatible with
a guarantee of hostile-code private-store isolation. Direct masks reduce accidental
effects, not daemon-mediated effects, alternate aliases, outside hardlinks or
same-user process access. Do not claim both full Docker and adversarial containment.
Stopping CLI processes does not roll back daemon or outside state.

Reject making Auto the default (it confuses capability with blanket approval),
removing every engine jail (unnecessarily broadens native/role authority), or
depending on uninstalled proxy/VM/host APIs. Keep old sessions/tasks without
initialization Guarded, preserve explicit choices and fail closed on unknown or
unreadable state. New initialization is additive project-local SQLite metadata;
owner settings are never silently replaced. See [permissions](../features/permissions-and-containment/index.md#access-mode-development-guarded-and-auto).

## 2026-10-09 observability decision gates and the edit-tool decision

The owner approved the following decision gates for observability measurement. They
were fixed before any task data was collected and are not adjusted from the data.

- **Validity gate.** If unmet, revise the instrumentation; do not act on branch
  verdicts. Require at least 10 completed tasks or 30 worker attempts per engine, with
  the same routes and efforts; at least 80% of attempts with a complete phase chain
  (engine ready, worker completed, review and verification events). Use token criteria
  only when unknown-basis and released-after-result calls are at most 20% of calls;
  otherwise use wall time only. Codex re-investigation figures are always labelled
  partial and are never compared directly with Claude's.
- **Worker loops.** Worker execution is at least 50% of critical-path wall time
  (excluding user waits), median tool calls per execution attempt are at least 15, and
  estimated inference-gap time is at least 40% of worker execution time. Only then does
  a new file-edit tool qualify, and only if `project_write` write redundancy (sum of
  bytes written divided by sum of changed bytes) is at least 5 and `project_write` is at
  least 30% of writes. Otherwise the branch is to promote `project_edit`.
- **Re-investigation.** At least 25% of text reads in dependent attempts hit a
  (path, sha) already read by another channel of the same task, and those reads are at
  least 15% of worker execution time. Cross-task reuse is considered only if the
  cross-task repeat ratio is at least 30%.
- **Orchestration chain.** Coordinator turns, engine startup, review, verification,
  synthesis and scheduler waits are at least 35% of critical-path wall time, or review
  plus verification alone are at least 25%. A qualifying chain becomes a separate task
  that shortens it without weakening review independence.
- **Several branches.** Take the largest share. If two are within 5 percentage points,
  do both, ordered by expected saving. **None qualifies:** stop; no further
  restructuring work.
- **Pilot criteria (not yet run).** Per engine, repeated-area group against the baseline
  variant, 5 to 10 tasks per variant, medians with IQR reported. The primary metric is
  cost per verified result: engine-reported tokens, or wall time when the token gate
  fails. Continue when that improves by at least 20% and verified completions are not
  below baseline; first-pass verification failures are at most baseline plus 1 per 10
  tasks; median review fix cycles are at most baseline and their total at most baseline
  plus 10%; no new blocking or high-severity review finding; median wall time to a
  verified result is at most baseline plus 10%; the stale-pointer share is at most 20%
  for a handoff variant; the control group cost is at most 10% worse; and Codex edits
  go through `project_edit` or the replacement tool for at least 50% of edits, otherwise
  the Codex result is inconclusive. Revise at a 5% to 20% gain or when only one engine
  improves. Stop below a 5% gain or on any regression breach; fewer reads with more
  errors is a failure.

**Edit tooling.** The planned `code_apply` tool is replaced by measuring and promoting
the existing `project_edit` (ordered exact replacements, atomic write, bounded diff,
prior-read check), unless observability data shows a gap it cannot cover. The known
differences, a missing `.py` syntax pre-check and no explicit `expected_sha256`
argument, are gaps to measure, not reasons to add a tool by default.

**Current verdict implementation.** `cortex_timeline.py --aggregate` evaluates
these gates separately by dispatched engine, connection, model and parameters
fingerprint (including effort). Only completed, exact, non-truncated execution
chains contribute branch inputs. Missing approved inputs withhold the affected
verdict, including unjoinable repeated-read ACP duration and unknown native-write
share. Retained per-call text-read history supports the cross-task ratio; absent
history is not zero. The gates are applied per cohort (engine, connection, model
and parameters fingerprint), which is stricter than per engine. The reader does not
compute the several-branch rule, the pilot criteria or any token-based branch; the
owner applies them to the reported branch values. The edit field
`code_apply_recommended` keeps its earlier name and means that the file-edit gate
holds. See the definitions and remaining observation limits in
[observability](../features/observability/index.md#multi-task-aggregates).

---

## 2026-10-05 accepted access and prompt boundaries

The owner selected the following behavior and limitations for Cortex 1.0.0:

- **Outside-project actions:** Auto intentionally permits `outside_write` and
  `outside_command` without a permission question/card, even if no client is
  connected. Guarded requires explicit accepted permission-card consent and
  fails closed if approval is denied, absent, cancelled, malformed or times out.
  (Development, added 2026-10-12, is described in the 2026-10-13 entry.)
  This is a deliberate Auto-mode choice, not a residual policy mismatch.
- **Engine jail exposure:** The engine sandbox may write to the host's real
  `/tmp` and engine state directories (except protected session-configuration
  entries); `/run/user` remains visible inside the jail. These are accepted
  limitations and are not mediated by the project gateway. `/var/tmp` remains
  per-run scratch-backed; other documented jail and gateway constraints remain.
- **Senior consultant:** Route selection follows normal policy and may use
  Codex or Claude. Codex native read tools cannot be disabled with the current
  adapter, so safe reads such as `cat`, `rg` and `ls` may reach project source
  without a permission card. This is accepted as a limitation of the
  reports-only prompt and Cortex-tool boundary; the consultant is not forced to
  Claude.

These choices are documented in [ACP permissions and containment](../features/permissions-and-containment/index.md),
[roles and delegation](../features/roles-and-delegation/index.md) and [security policy](../../SECURITY.md).
They do not claim broader host enforcement or complete technical isolation.

---

## 2026-09-30 quota applicability uses exact owner settings

`routing.quota_windows` maps supported engines to unique exact window-label rules with shared (`null`) or exact-model-list applicability. No model-family substring table remains in executable quota logic. Unmapped evidence conservatively applies to all models; unknown values stay unknown. Menu/automatic routing/reset lookup consume the same effective settings. Installation data preserves canonical old scoped-window behavior for the retained configured identities without granting routes or assuming provider capability. API v1/policy v2 and explicit profile snapshots carry the optional field; no runtime template merge, profile rewrite, or production adoption occurs. See [contract and migration](../configuration.md#exact-quota-window-associations).

## 2026-09-30 owner profiles snapshot full user settings; routing preferences rank within eligibility

This supersedes the earlier 2026-09-30 decision “user routing profiles
materialize into routing.json”. Owner profiles in
`$XDG_CONFIG_HOME/cortex/routing-profiles.json` use format 2 and snapshot
four user-level sections: the managed routing keys (including exact quota associations); defaults and owner
policy ceiling; MCP server usage policy; and per-connection concurrent-session
limits. They are a management surface, not another runtime source. Activation
validates and materializes those sections into existing backing files under
the owner configuration lock, writes a pending journal before replacement, and
rolls back in-process on failure.

Profile states are `none`, `in_sync`, `modified`, `interrupted` and
`unavailable`. Saving is explicit: save current applied settings to the
selected profile or create a new profile; ordinary edits never write through.
Metadata update changes only name/description. Switching away from a modified
snapshot requires an explicit save or discard; detaching leaves working
settings unchanged. Format 1 is read as a routing-only snapshot and migrated
to format 2 on first mutation with a private verified backup; it stays partial
until a full snapshot is explicitly saved. Project routing and ceilings, MCP
`command`/`args`/`env`, and connection credentials/login state are excluded.
The runtime continues to read materialized backing files using the existing
precedence: project routing, then `$CORTEX_ROUTING`, then user routing. No
profile bypasses project scope or a policy ceiling.

A separate routing defect was fixed in rank version 9: within the role's
eligible tier, the owner's model `preferred_for` choice now outranks a new
plan hint, plan-usage heuristics and retry diversification. This fixes the
reproduced case where an otherwise eligible preferred OpenCode model lost to
a coordinator engine hint or retry diversification. It does not grant routes
or remove gating: a preference cannot cross tiers or bypass pin/deny,
`enabled`/automatic selection, providers, connection/catalog readiness,
recorded unavailability or quota rules. In particular, OpenCode remains
excluded from automatic selection unless explicitly enabled/selected (or
pinned), and the first existing project/environment/user routing file still
wins wholesale, so project or `$CORTEX_ROUTING` configuration can shadow a
user profile. A test that loads the original source and sees five expected
pre-fix regression failures is causal reproduction, not a post-fix failure.


## 2026-09-30 model selection, tiers and policy come from configuration

Automatic worker routing knew models only by name: tier tables (families,
Codex ids and lines), the Haiku and optional-engine exclusions, the flagship
effort ceiling, the Luna rule and the Russian request spellings were
constants in `routing/auto_route.py`, and the `plan_submit` node schema and
the orchestrator's `worker-routing` reference named models and engine
preferences. The owner could not choose which models (including OpenCode
models) take part in automatic routing, how they are classified, or
which model to prefer for a kind of work.

`routing.json` gained a `models` key, per engine and keyed by exact model id
(OpenCode ids contain `/`, so a flat `engine/model` key would be
ambiguous): `auto` (the complete automatic-routing selection; absent keeps
the built-in rules), `tiers` and `policy` (`effort_floor`, `max_effort`,
`effort_ladder`, `also_standard_for`, `preferred_for`, `request_names`). The
name-keyed rules moved to `routing/model_policy.py` as built-in defaults that
apply only where the file is silent, so an existing installation routes
exactly as before (rank version 8; the golden tests are unchanged apart from
the version). `preferred_for` answers the owner's request to say which models
suit which purposes: rank version 9 evaluates the owner's preference after
legacy provider preference and ahead of new-plan hints, plan-usage heuristics
and retry diversification, within the eligible tier. It never changes a tier
or authorizes a route; pins and hard eligibility rules still control. A selection list disables supersede for its engine,
because silently dropping an explicitly selected older version would
contradict the owner's list.

Prompts carry no model names: `list_route_options` is the single live source
(tiers, tier sources, policy, `flagship`, `stretch`, `preferred`,
`affinity`), the node schema and `worker-routing.md` refer to it, and
`tests/test_no_hardcoded_model_names.py` fails when a built-in model or
family name reappears in skills, profile sources, tool schemas or the unified
system prompt. The live-qualification references keep the AGENTS.md owner
rule's model, which is not a routing input. Rejected: a per-session templated
schema (engines cache schemas per session and the reference files are
hashed into the package), a separate config file (a second precedence
chain), and extending the legacy `tiers`/`auto_allow` keys (engine-agnostic
and family-matched, so they cannot express "only these models").

## 2026-09-29 a flagship route runs when the user asked for it

Flagship models (Astra, Fable) were escalation-only, and a plan hint never
raised the tier, so an owner's explicit "use Astra" relayed by the coordinator
was recorded as `ignored:tier_mismatch`. Now an exact flagship route hint runs
its node at the flagship tier when the user named that model family in the
task. The grant is read by the runtime (`routing.auto_route.user_model_request`) from the
task's `source_revisions` bodies and real users' answers -- the same records
`list_sources` labels `user_direct` and addresses by `source_ref` -- never from
the hint, `model_reason`, question text, reports or worker output, so the
coordinator cannot grant itself a flagship model. The `USER_STEERING` runtime
events were considered and rejected as the source: they only cover later
steers applied at a planning boundary, while every steer is already a source
revision. No per-task structured model choice exists to use instead.

The match is deterministic, not an interpretation: the route's family (Latin
name or a known Russian spelling, whole word) must follow, within four words
in the same sentence, either an explicit request word (`REQUEST_WORDS`: "use
Astra", "run it on Astra", "используй астру", "требование использования
астры") or a work verb governing a preposition of means right before it
(`WORK_VERBS` + `MEANS_WORDS`: "implement it on Astra", "сделай на Астре"),
with no negation, comparison or substitution word in the five words before
it (`NEGATION_WORDS`: "don't use Astra", "compare Astra with Fable",
"instead of Astra"). Review of the first cut found that a bare mention
granted the route ("Astra is unavailable; use the default route" raised the
node to Astra), so the request word became mandatory. Review of that fix
found that the prepositions it also accepted as request words (`on`, `via`,
`through`, `на`, `через`) still granted remarks such as "it failed on
Astra" and "посмотри на Астру", so a preposition now counts only when a work
verb governs it, and a request or work word counts only in a request form:
an English verb opening its clause or following a request-mood word
("please", "to", "could you"), not an article, noun or auxiliary ("the last
run on Astra", "the tests run on Astra", "we tried to run it on Astra"), and
a Russian verb only in its listed imperative, infinitive or first-person
plural forms, never a past tense, participle or verbal noun ("запустили на
Астре", "использование Астры"). The one exception is the Russian subjunctive:
a past tense after "чтобы"/"что бы" in its clause asks ("хочу, чтобы ты сделал
аудит через Опус"); it was added on 2026-10-13 after such a request was missed,
and any negation earlier in the sentence ("не хочу, чтобы ...") still refuses
it. A request phrased otherwise ("I want Astra",
"ревью через Фейбл", "let Astra do it") is deliberately not recognised and
leaves the node on its ordinary tier with `not_requested_by_user` in the
receipt: a false negative costs a rephrase, a false positive a flagship run
the user did not ask for. The coordinator's
exact hint is the required second step, and it applies only to the nodes
carrying it. Pin, deny, providers, availability and quota still win; a
refused request routes the node normally with its reason in
`route_decision.user_model_request`.

## 2026-09-29 the runtime withholds coordinator text after its own question

In Paseo session bb142018 the coordinator wrote the full details, asked with
`user_question_request`, and then, in the same turn, wrote a second full
version of the same details (similarity 0.69). The runtime forwarded both
(re-checked at the then-current HEAD).
The 2026-09-27 fix below was prompt-only: it banned a *shorter* recap and
lowered the frequency, but a same-length rewrite still happened
intermittently.

After the question commits `WAITING_USER`, `controller.run` suppresses newly
received coordinator text for the rest of that engine turn, using the state
read that already suppressed the `ANSWER_READY` echo. Review found that releasing
held text on an unseen tool-call id leaked the rewrite when the question's first
lifecycle callback was delayed. That release heuristic was removed; final-only
text is also withheld when the question was observed or remains pending. Text
already accepted before the commit still flushes, and later turns stream normally.
The trade-off is explicit: genuinely pre-question text delivered after the commit
is indistinguishable from a rewrite and is omitted too. No engine history is
rewritten. The prompts changed to match:

- The coordinator's question receipt now carries a `next` end-turn hint.
- The RECONCILING prompt says to write the details once before the call and
  nothing after it; it no longer says "your final message".
- The orchestrator skill and the tool contract ban any restatement or rewrite
  after the question, of any length.
- The unified base prompt gives the coordinator its one communication rule.

## 2026-09-28 queued, cancelled and re-reviewed workers recover instead of failing

Paseo session 7bda9342 lost workers in three ways. All
evidence was read from a copy of the project store.

- **Capacity.** Seven Opus workers ran on a four-slot connection. The three queued
  workers timed out after 600 s (`capacity_timeout`) while the four live holders
  still worked. With no alternate route, fallback was refused, and `retry_lost`
  treated `fallback_refused:*` as final. A queued open now extends its wait while
  every slot is held by a live session. The timeout still applies behind an
  ambiguous or unobserved holder. A capacity-class fallback refusal is retried
  within `MAX_ATTEMPT_RETRIES`.
- **Read-only roles given output files.** The ux-designer workers finished their
  audits and then could not save them: the role is read-only, so its jail binds
  nothing writable. A concurrent change to this checkout now makes
  `plan_submit` refuse nonexistent paths in a read-only task's scope
  (`read_only_scope_not_writable`).
- **User cancel.** A Paseo cancel at 08:36:19 UTC cancelled the remaining
  workers by design. The two still queued were also reported as
  `assignment_failed` (`attempt`, `channel_revoked`); that duplicate report is
  now suppressed for a durably cancelled node.

Separately, a review result was refused for good once the project changed during
review, because the `REVIEW_STARTED` binding was never refreshed. A changed
project now re-binds the review, refuses that request before publication and
asks for a new submission. This is bounded by `MAX_REVIEW_REBINDS`.

## 2026-09-28 bounded recovery-capsule critical sections

A long coordinator task got stuck on `capsule_oversize`. Its critical capsule
sections held about 515 KB, above the 400 KB hard cap. Most of that, about
450 KB, was 40 inlined assignment briefs. The size is deterministic, so bounded
delivery retries could never succeed. The critical sections are now bounded by
construction:

- Content within 96 KB is delivered verbatim.
- Otherwise, a deterministic escalating compaction level replaces large text
  with exact references: sha256, size, a bounded preview, a `truncated` marker
  and the retrieving tool.
- Only at the later levels are long lists trimmed. Trimmed entries are named in
  explicit `<key>_omitted` records.
- The latest user message and the task goal keep the largest inline (`protected`)
  allowance among the tiered fields.
- The hard cap remains only a last-resort guard.
- The delivery retry path is unchanged.

**2026-09-28 follow-up (P1 fix):** a worker recipient's
own `own_assignment.title`/`brief`/`acceptance` and its own runtime node's
`objective` are never subject to the tiered inline allowances above -- no
worker-available tool can retrieve them, so compacting them into a
`status="referenced"`/`retrieve.tool=null` object would strip a recovered
worker of the only readable copy of its mandatory objective. Those fields now
stay byte-for-byte verbatim at every compaction level regardless of size;
everything else in the critical sections compacts harder in their place to
still fit `CRITICAL_BUDGET_BYTES`. Only if the worker's own assignment text
alone still exceeds `HARD_CAP_BYTES` after the most aggressive level does
`build_capsule` raise the explicit `capsule_oversize` error -- never a silent
reference. A coordinator's cross-referenced assignment brief/node objective is
unaffected (still resolvable via `task_get_context`, so it still compacts
normally).

See `docs/features/recovery-capsules/index.md` ("Size bounds").

## 2026-09-28 orchestration overhead and evidence optimizations

The completed optimization set keeps durable task state and existing evidence
checks authoritative while reducing repeated reads and oversized event payloads.
The no-paid overhead harness was run before and after the changes. These are
controlled, single-host observations, not provider latency or end-to-end task
duration measurements.

| Scenario | Baseline | After | Interpretation |
| --- | --- | --- | --- |
| 1,000 coordinator text deltas | 5,054 store connections; 1,005 full contexts; 18 immediate write transactions | 1,036 connections; 5 full contexts around the run; 1,005 state reads including setup (one per delta); 18 write transactions | The delta callbacks use a state-only query and build no full contexts. The remaining five context builds are controller work outside individual callbacks. |
| Idle execution window, about 2 seconds | 120 connections; 41 contexts; 78 `BEGIN IMMEDIATE` statements | 51 connections; 6 contexts; 0 immediate transactions during the 2.045-second idle window | The after-run had 62 connections, 11 contexts and 2 immediate transactions over the whole scenario; those two were setup and cancellation. Full passes reconcile at least every 2 seconds. |
| Verification output, 5 MiB | 5,242,880 bytes persisted and sent in the event; 10,505,717-byte Python allocation peak | All 5,242,880 bytes persisted; 2,563-byte tool-card body; 156,555-byte Python allocation peak | Full output is streamed to durable large-output storage; the event carries a bounded excerpt and record ID. |

The baseline verification case used mocked sandbox gateway helpers, as did the
harness's output measurement; it tests output buffering and payload size, not
sandbox isolation or process-wide memory. The idle case uses a scope-blocked
fake worker. The harness records zero retry-history reads in both its idle
case and the measured coordinator path; that does not establish retry-query
counts for ready-node workloads.

### Adopted behavior

- **#2, state reads and consistent context:** coordinator text checks read only
  the durable task state. Full context uses one SQLite read transaction and
  batches assignment-attempt lookup. No in-memory task-state cache was added.
- **#4, reduced scheduler polling:** a database fingerprint gates expensive
  scheduler passes, with worker completion and runtime notifications able to
  wake it sooner. A full reconciliation still occurs at least every 2 seconds;
  permission-card relay retains its 0.1-second cadence. `claim_ready` uses a
  read-only candidate check before taking a write lock, rechecks inside the
  transaction, and batches retry-history reads.
- **#8, launch order and route diagnostics:** ready nodes are prioritized by
  longest remaining dependency chain, with row order breaking ties.
  Task-scoped accepted-result latency and review fix-cycle aggregates are
  descriptive diagnostics only; they do not affect route selection.
- **#5, verification logs:** complete output is persisted from a stream and
  only a bounded tail plus its output record ID is sent in the event. Command
  exit, interruption and project-integrity checks remain in force; verification
  commands remain sequential.
- **#1, review findings:** a fully runtime-validated set of local findings
  creates fixes only for named plan tasks, with their plan-order dependencies.
  Missing, invalid, out-of-scope or cross-cutting findings retain the broad
  all-task fix fallback. Contract impact is not inferred automatically.
- **#7, check identity:** a same-name check on an unrelated node cannot close
  a synthesis gap. Closure requires the same node/fix lineage or a current,
  verified `command_run` whose recorded argv exactly matches the declared
  command; evidence ownership and receipt validation remain enforced.
- **#3, inline result reports:** `result_submit` accepts exactly one of the
  existing `report_ref` or inline `report` forms. Inline publication uses
  the worker's owned-draft and existing publication path, keyed to the result
  request so retries reuse the same report; existing authority and evidence
  validation remains.
- **#6, compact coordinator context:** prompts retain the active cycle in full
  and compact superseded nodes while preserving historical check gaps and all
  other context fields. Persisted context and `task_get_context` remain full.

### Rejected approaches and trade-offs

- An in-memory state cache was rejected because MCP updates commit to SQLite
  independently and a cache would need a correctness protocol; a direct durable
  state read is small and sees those updates.
- A separate state-version/event system was rejected. The existing database
  fingerprint, notifications and periodic reconciliation cover local wakeups
  while the database remains authoritative.
- Parallel verification was rejected because commands can share ignored build,
  dependency and cache paths, making write conflicts and attribution ambiguous;
  command counts are typically small and parallel resource contention can also
  produce misleading timeouts.
- Automatic route selection by time-to-accepted-result was rejected for lack of
  enough comparable observations. The new aggregates expose descriptive data
  while current routing rules remain unchanged.
- A context-delta protocol over generation delivery was rejected as unnecessary
  complexity for the bounded review-cycle projection. Full context remains
  available for recovery and decisions.
- Automatic expansion to contract-dependent fix areas was rejected because the
  runtime has no reliable signal for inferring contract impact. Reviewers mark
  cross-cutting findings explicitly, which trigger the conservative broad fix
  cycle.
- A scheduler pass that the fingerprint does not trigger may now wait up to
  the 2-second reconciliation interval. Worker completion and runtime
  notifications can wake the scheduler earlier; this bounded delay is the
  trade-off for avoiding repeated full passes.

The measurements do not estimate total task-time savings or provider behavior.

## 2026-09-28 paid live checks only on an explicit owner request

The owner revoked the 2026-09-24 standing paid-call approval. Live QA through
Paseo Desktop took hours per change, mostly on GUI driving rather than on the
model run. Default verification is now no-paid: targeted pytest, the Paseo
plugin type-check and `test:*` scripts, and the no-paid smoke harness. A paid
live check runs only when the owner explicitly requests it for the current work
and names the mode: `cli` (headless through the guarded Paseo CLI, checked
against the disposable project's `.cortex` store) or `desktop` (the Electron
app, for rendered behavior). Without a request, an applicable live check is
reported as not run with the blocker "paid live check not requested by the
owner", never as passing. The isolated dev Paseo, production-account and model
rules are unchanged. The probe scripts still enforce both flags; the approval
reference names the request instead of `owner-standing-2026-09-24`.

Rejected alternatives: keeping the standing approval and moving the checks to
the CLI, which still spends paid calls on every change; and treating `cli`
output as rendered GUI evidence, which it is not.

## 2026-09-27 proactive skill proposals use the durable question

The explicit `/skill-management` workflow gained an approved proactive phase.
The coordinator may propose one project skill per task. It uses
`user_question_request` with `skill_proposal`, never a worker `ask_question`,
because that stops workers. The runtime validates 2-5 ordinary retained
reports as evidence, checks the action against `.agents/skills/<name>`, and
refuses duplicates across the project. It accepts only these exact
action-matched option pairs, with approval first:

- create: [Create the skill, Not now]; [Создать навык, Не сейчас];
  [Approve creating the skill, Decline creating the skill];
  [Одобрить создание навыка, Отклонить создание навыка]
- update: [Update the skill, Not now]; [Обновить навык, Не сейчас];
  [Approve updating the skill, Decline updating the skill];
  [Одобрить обновление навыка, Отклонить обновление навыка]

Answer 1 selects the displayed pair's approval label; answer 2 selects its
decline label. Other answers are unclear, and an arbitrary first option never
approves. Approval passes the same workflow instruction as the explicit
command to the next planning turn. A decline or unclear answer writes nothing.
The data lives in `runtime_events` payloads and `pending_questions`, so no
storage format change was needed.

Rejected alternatives: a suggestion only in coordinator prose, which gives no
durable answer and no duplicate control; free-text approval matching, which is
language-dependent guessing; and gating plans on the decision, because the
runtime cannot read plan intent and the explicit command leaves no durable
marker.

## 2026-09-27 per-task budget for coordinator tool cards

A review found that worker cards were bounded but
coordinator/engine tool cards were not. A coordinator making N distinct calls
sent N retained Paseo cards, and the emitter's `seen` set grew without bound.
The fix adds a total budget per task, not per emitter. The first 150 distinct
calls, or 4 MiB of their updates, get normal delta cards with at most 10
updates each; the last slot is kept for the terminal update. Later calls are
counted on one aggregate card. It is created at once, then updated at most
once per 30 seconds, with the interval doubling after each update, and
refreshed at turn end. It sends at most 20 updates per task. Failures past the
budget keep their own card for up to 50 calls. The budget survives turns,
emitter recreation and restarts because a new runtime seeds it from the host
journal. Bookkeeping uses 1,024-entry LRU maps. `session/load` folds older
journals to the same bound.

Rejected alternatives: a per-turn budget (turns are unbounded), dropping
folded calls silently (the user would lose the count), and an unbounded final
count at every turn end (turns are unbounded). The trade-off: after 20
aggregate updates, the card stops counting and says so. The complete list stays
in durable task history. Permission requests and questions are host requests,
not cards, and are never folded. Coordinator prose, thought, plan and usage
updates are not covered by this budget.

## 2026-09-26 bounded ACP emissions, schema visibility and mobile compatibility

The approved change set is P1–P6, A1–A3 and B1–B5; P7 (compressing old
journals) is excluded. Runtime worker cards omit transcripts and raw inputs,
generic clients receive a rolling tail capped at 20 entries/4 KiB, result
projections are capped at 16 KiB, coordinator tool fields at 16 KiB each, and
serialized `session/update` payloads at 32 KiB. Each worker card also has a
fixed event budget: forward-only status transitions are immediate; other
changes are progress updates, the first immediate and then at most one per
30 seconds and eight per card; and a settled card allows at most two further
refinements. One card is therefore at most 13 updates, whatever the worker's
size. Titles pass through one normalizer; two spellings of one long title had
produced 296 worker updates in the live three-worker run, against 9 on the
baseline. These are bounds on streamed projections; complete task history is
still retained and grows with work performed. Paseo 0.9.2 retains provider
events in session history until the session closes, which makes limiting each
emitted event material to host memory use.

The same release makes array contracts self-describing for Codex's MCP
TypeScript-schema view, returns multiple argument validation errors together,
and instructs the coordinator to write user-facing messages once without a
short recap. A later Codex probe confirmed the schema reached the model; an
undocumented project's empty `verification` list came from a missing fallback
in the contract. Required array descriptions now reject empty lists up front,
require verification for every plan, and define a project-local literal-argv
fallback when no verification is documented. Validation corrections repeat
the field guidance. The coordinator communication rule remains one complete
user-facing message without a shorter recap (runtime-enforced after a user
question since 2026-09-29, see above).

Timeline emission now sends only changed tool-card fields, drops exact
MCP-text mirrors of structured results, suppresses no-op updates, and uses
`same_as` references for repeated completed outputs with a bounded 64-key
cache. An identical `task_get_context` poll returns a compact receipt, and the
accepted-plan/context results advise the coordinator to end its turn when no
action is needed. The hint reduces needless polling but is not enforced, and
worker dispatch began after the coordinator turn ended (since 2026-10-13 it
begins as soon as the plan is accepted, during that turn). Scale tests show
bounded event bytes for the tested 10- and 200-tool workload; they do not prove
constant daemon memory or bound durable history. A live dev-Paseo heap run was
not performed.

Implementation covered runtime emission and race fix, schema visibility and communication, Paseo timeline bounds, and the phone Hermes diagnosis and fix. Current contracts are
documented in [ACP observability](../features/observability/index.md#bounded-acp-event-projections)
and the Codex schema rendering gotcha in [gotchas](gotchas.md).

## 2026-09-26 remove the Paseo Workers composer button and popover

The user steered that the Paseo "Workers" composer pill and the popover it
opened duplicated the Pipeline panel and should be removed entirely, keeping
only the popup surface (not the Pipeline panel/tab, worker cards in the chat
timeline, or the worker detail panel/tab those cards and Pipeline's
"open worker" action still open). Implemented by removing the
`workers-${agentId}` composer pill and `WorkerPopover` wiring from
`integrations/paseo/index.client.tsx`, deleting `client/worker-popover.tsx`
and the now-dead `client/worker-history-poll.ts` (its only caller), and
trimming the now-dead `orderPopoverWorkers` helper out of
`client/worker-popover-status.ts` (its `workerPopoverStatus` export stays:
`worker-panel.tsx` and `pipeline-model.ts` still use it). The "workers"
workspace panel, its command-center entry, and the Pipeline panel/pill are
unchanged. Requested via a delivered user steer.

## 2026-09-23 audit remediation (user-approved)

- Build-verification profile: keep it (all 23 profiles are required);
  document it as runtime-reserved — not plannable, not dispatched — since the
  runtime itself executes command verification. Approved by the user.
- Retention: ship a user-run maintenance CLI, `python3 src/cortex_clear.py
  --project-root <path> --days N [--keep-thread <id>]`; the coordinator only
  explains it. Codex-era `CODEX_*` env protection is removed. Approved by the user.
- `docs/project/verification.md` is produced by a knowledge-harvest
  refresh rather than hand-written, per the user.
- Worker routing: a route-caused failure (quota/rate limit, model
  unavailable, engine start failure, unconfirmed worker mode) is recorded
  unavailable with an expiry and retried on another available route; routing
  refusals (pin/deny/malformed `routing.json`/no available route) are named,
  non-retried blockers. Haiku workers configure only options their own
  session offers. Required by the user.
- Live checks for this remediation are user-authorized paid Claude-only runs
  in an isolated dev setup; the production plugin and the user's real
  configuration are never touched. Requested by the user.

## 2026-09-24 verification gap disclosure

A structured unrun or unverified worker check carries a concrete blocker. At
synthesis, retained gaps from the current plan revision must either close
under the [exact-command or command-less compatibility rules](../features/orchestration/index.md#check-history-and-current-outcome)
or appear in the structured not-verified list with a blocker. The runtime
renders the list into the stored synthesis. This prevents
silent omission while leaving the coordinator able to replan verifiable gaps.

## 2026-09-24 standing live-call approval and isolated Paseo decision

Superseded on 2026-09-28: paid live checks now need an explicit owner request
naming `cli` or `desktop`; the isolation rules below still apply.

The owner authorized paid model calls by default for live verification and
directed that such testing run through the isolated dev Paseo instance. For
applicable live checks, use the permanent `~/.paseo-dev` daemon, its separate
Electron profile at `~/.config/Paseo-dev`, a disposable project, and the
Cortex runtime launched by that dev daemon from the explicit absolute checkout
plugin path. Never run these checks through production `~/.paseo` or the
production Cortex agent. The existing production Codex and Claude accounts may
be used through the approved isolated setup; do not create test accounts, copy
credentials, or perform isolated engine login. Check native login status
without printing credentials before relying on either engine.

Historical rule (superseded and revoked): the 2026-09-24 standing paid-call approval and named route are no longer valid. Current qualification is no-paid by default; only a new explicit owner request naming an allowed mode and exact supported route can authorize a paid check. Heavy models remain prohibited in routine live qualification. The probe scripts still require both `--i-authorize-paid-call` and `--approval-ref`. Never mark an unobserved check as passed.

## 2026-09-28 review follow-ups, prompt rollback and worker hand-off

The follow-up defects and operational improvements were
implemented in source before this documentation update; see the linked behavior
contracts in [ACP orchestration](../features/orchestration/index.md), [unified system
prompt](../features/unified-system-prompt/index.md), [roles and delegation](../features/roles-and-delegation/index.md)
and [observability](../features/observability/index.md).

- Synthesis gaps with an explicit command require a current verified exact-argv
  `command_run` or a runtime-owned final-verification command with identical
  argv, complete exit-0 execution, untruncated output and the current reviewed
  digest matching the gap author's runtime-observed publication or review binding,
  regardless of execution order. Only authoritative credit makes a speculative
  run eligible. Closure references are durable but rederived, not trusted as claims. Same-name fix-lineage closure is reserved
  for command-less legacy gaps. Broad fallback fix cycles preserve direct plan
  dependency edges
  while retaining parallelism between independent branches.
- The skill-catalog frontmatter reader remains a bounded stdlib scalar reader,
  not a general YAML parser or new YAML dependency.
- `CORTEX_UNIFIED_BASE_PROMPT` stays on by default; setting it to `0` before
  startup restores native base-prompt paths for new sessions. Real-model behavior
  with the unified prompt remains unverified by paid comparison. This is separate
  from runtime performance and routing changes.
- Coordinator hand-off is prompt-only and evidence-based: include only locations
  actually looked up, and workers still inspect current source. Use the resolved
  code-index project directly when available, with `list_projects` as fallback.
  Do not forward coordinator transcripts. Structured digest-bound location
  fields (F3) and pre-first-write discovery metrics (F4) are deferred.
- No in-memory task-state cache, parallel verification, self-learning routing or
  controller rewrite is adopted. Durable SQLite reads and current polling and
  reconciliation remain; route metrics remain diagnostic only, not automatic
  model-selection evidence.

## 2026-09-30 exact model policy is owner data; installation asset is not runtime policy

Following the production-derived baseline review, executable model classification and fixed model-name instruction defaults were removed. Current routing requires version-2 explicit owner settings and current catalogs. Exact engine/model identities explicitly define enabled and automatic membership, tiers and optional per-model policy. No model-name classifier, family/version table, list-position tier, seed list, or hidden installation-template overlay may fill missing settings. The public policy/model-settings-default.json is a validated install-only candidate; fresh installation writes only absent owner files, while existing settings and saved profiles require explicit digest/revision-bound preview/apply migration. The production-derived candidate retains observed policy without claiming quality validation or live qualification; portable coordinator resolution refuses ambiguous/unavailable identities. See [settings-only model policy](../configuration.md#settings-only-model-policy) and [migration](packaging.md#model-policy-installation-and-explicit-migration).

Saved-profile deletion is explicit, revision-checked and local: selection unlink does not reset current materialized settings, deleting the final profile is allowed, and refusal/failure preserves data.
