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
