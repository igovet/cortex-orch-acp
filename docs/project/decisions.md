## 2026-09-28 queued, cancelled and re-reviewed workers recover instead of failing

Paseo session 7bda9342 (task t_e1ff07ff2819) lost workers in three ways. All
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

**2026-09-28 follow-up (P1 fix, review r_f5018dc7eebf):** a worker recipient's
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

See `docs/acp/recovery-capsules.md` ("Size bounds").

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

Review `r_3fdf8c846b22` found that worker cards were bounded but
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
user-facing message without a shorter recap.

Timeline emission now sends only changed tool-card fields, drops exact
MCP-text mirrors of structured results, suppresses no-op updates, and uses
`same_as` references for repeated completed outputs with a bounded 64-key
cache. An identical `task_get_context` poll returns a compact receipt, and the
accepted-plan/context results advise the coordinator to end its turn when no
action is needed. The hint reduces needless polling but is not enforced, and
worker dispatch still begins after the coordinator turn ends. Scale tests show
bounded event bytes for the tested 10- and 200-tool workload; they do not prove
constant daemon memory or bound durable history. A live dev-Paseo heap run was
not performed.

Implementation evidence: runtime emission and race fix
(`r_f4598d9817a2`), schema visibility and communication
(`r_c5c2afa73253`), Paseo timeline bounds (`r_5bf00f25920a`), and the
phone Hermes diagnosis and fix (`r_6ec2006f6af1`). Current contracts are
documented in [ACP observability](../acp/observability.md#bounded-acp-event-projections)
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
unchanged. Requested via the delivered user steer captured in this task's
capsule (`r_31708993ed23`).

## 2026-09-23 audit remediation (user-approved)

- D1 build-verification profile: keep it (all 23 profiles are required);
  document it as runtime-reserved — not plannable, not dispatched — since the
  runtime itself executes command verification. Approved in
  `list_sources`/`read_source(pq_d977d180d44f:answer)`, re-confirmed in
  `read_source(pq_d4502996482e:answer)`.
- D2 retention: ship a user-run maintenance CLI, `python3 src/cortex_clear.py
  --project-root <path> --days N [--keep-thread <id>]`; the coordinator only
  explains it. Codex-era `CODEX_*` env protection is removed. Approved in
  `read_source(pq_d977d180d44f:answer)`, re-confirmed in
  `read_source(pq_d4502996482e:answer)`.
- D3 `docs/project/verification.md` is produced by a knowledge-harvest
  refresh rather than hand-written, per the user, in
  `read_source(pq_d977d180d44f:answer)`, re-confirmed in
  `read_source(pq_d4502996482e:answer)`.
- Worker routing: a route-caused failure (quota/rate limit, model
  unavailable, engine start failure, unconfirmed worker mode) is recorded
  unavailable with an expiry and retried on another available route; routing
  refusals (pin/deny/malformed `routing.json`/no available route) are named,
  non-retried blockers. Haiku workers configure only options their own
  session offers. Required by the user in
  `read_source(pq_d9fa85b27e5c:answer)`.
- Live checks for this remediation are user-authorized paid Claude-only runs
  in an isolated dev setup; the production plugin and the user's real
  configuration are never touched. Requested via
  `read_source(pq_bc3b03c3f491:answer)` (runtime fix to make answers visible
  to reviewers, which this citation form now reflects).

## 2026-09-24 verification gap disclosure

A structured unrun or unverified worker check carries a concrete blocker. At
synthesis, retained gaps from the current plan revision must either have a
same-name passing result or appear in the structured not-verified list with a
blocker. The runtime renders the list into the stored synthesis. This prevents
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

The default live model remains `gpt-5.6-luna` at medium/high effort; heavy
models remain prohibited in routine live qualification. This standing approval
authorizes the paid probe flags with approval reference
`owner-standing-2026-09-24`; the probe scripts still enforce both
`--i-authorize-paid-call` and `--approval-ref`. The default automated pytest
suite remains no-paid. General evidence rules still apply: never mark an
unobserved check as passed.
