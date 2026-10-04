# Observability and resource attribution (T029)

Requirement IDs: S09-05; S09-06; S15-07; S17-06; S20-05; S20-06; S20-07;
S23-01; S25-04. Acceptance matrix: PR-07, QUAL-06.

This task makes route use, recovery, resource limits, and uncertainty
observable without exposing secrets or inventing telemetry. It never
replaces an existing durable evidence path (the T016 `task_events` log,
T027's `attempt_route_audit`, T022's `context_generations`/
`capsule_deliveries`, ...); it reads and joins those, plus two new tables,
to build the metrics/attribution this task's own acceptance bullets
require (`src/cortex_runtime/observability/`).

## 1. Structured events (`observability.events`)

`observability_events` (additive, `user_version` stays 11) is an
append-only, immutable table (see `observability/store.py`): every row
carries whichever runtime identities the occurrence actually has —
`assignment_ref`, `attempt_id`, `engine_session_ref`,
`context_generation_id`, `config_revision_id`, `connection_id`,
`route_engine`, `source_event_ref` — every one nullable, never fabricated
when absent, plus a mandatory `category`/`outcome` and a `basis`
classification chosen out of the same closed set
`recovery.compaction_guard`/`recovery.token_budget` already use:
`observed` | `engine_reported` | `derived` | `estimated` | `unknown`.
There is no default basis — every append site chooses explicitly
(`observability/events.py`'s `append_event`/`append_event_tx` raise on an
unknown value).

Two real append sites exist today:

- `store.py`'s `_call` records a `publication`/`duplicate_attempt` row, in
  the very same transaction, whenever a `(scope, operation, delivery_key)`
  already has a confirmed `deliveries` row — an idempotent replay of an
  already-confirmed write, never a new mutation, but a genuine *duplicate
  publication attempt* for the metric below.
- `delegation.mcp_tools.dispatch` records a `validation`/`<error code>` row
  whenever a channel's own tool-call arguments fail
  `delegation.contracts.validate` — the "schema/validation errors" metric.
- Rejected worker submissions also produce `submission_rejection` rows. The
  event records the safe error code and bounded field-path diagnostics, not
  rejected values or report content. This includes result-schema corrections
  and observable situational-skill gate refusals; exact field paths are capped
  by the runtime's violation bound.
- After sent session metadata is recorded, a best-effort `base_prompt` event
  records only the mode (`unified`, `native` or `unknown`) and session
  identifiers. It does not store the prompt or environment values. Because
  recording is best-effort, a missing event does not establish which prompt was
  used.

`observability.events.list_events(store, task_id, category=..., after=...)`
is the cursor-paginated read.

The diagnostics summary's `schema_validation_errors` is an observed count across
both `validation` and `submission_rejection` events. Its `base_prompt` field
counts observed session modes in recorded `base_prompt` events; it reports
`unavailable_reason` when none were recorded. This diagnostic is event-backed,
not an INFO log or a record of prompt text.

Worker-panel history uses the existing durable `task_events` stream rather
than a second transcript store. Each redacted `progress` row carrying
`kind=worker_activity` is bound to the task and worker attempt, and the panel
reads it in event-id order with an `after` cursor and bounded page size.
While a worker is active, the host panel refreshes that cursor incrementally
about every 750 ms. Exact lifecycle updates for one logical tool coalesce in
the panel, while worker deltas are held until a tool boundary or turn end and
persisted as complete message blocks. Each block carries a durable segment
identity across all runtime tools, so live and replay views preserve ordering.

### Bounded ACP event projections

Worker activity is retained in the durable task event history, while the ACP
timeline receives a bounded display projection. Worker tool cards omit
transcripts and raw inputs. Paseo cards carry status and title, with a bounded
failure line or terminal result; generic clients receive a rolling tail of at
most 20 action lines and 4 KiB. Runtime worker prose is buffered by message
block instead of being emitted per token.

Each worker card also has a fixed event budget, because Paseo keeps every
update however small. Status transitions move forward only (create as
`pending`, start as `in_progress`, then `completed` or `failed`). Each is sent
at once, and a stale earlier status never moves a card back. Any other change,
such as a title or generic-client body, is a progress update. The first
progress update is sent at once. Later ones are sent at most once per 30
seconds and at most 8 times per card. A settled card may be refined at most
twice more, for example with a later outcome label or result, and these
refinements are not delayed. A card therefore sends at most 13 updates,
whatever its tool count, output or duration. A withheld change is not lost:
the latest state is recomputed on every call and sent with the next permitted
update, and always with the terminal one. Every worker-card title passes
through one normalizer (300 characters, outcome suffix kept), so two code
paths cannot send two spellings of the same title. Live qualification
(`r_a3d87956eb0f`) found that alternating clipped and unclipped copies of one
long title caused 296 worker-card updates for three workers, against 9 on
the baseline. On Paseo, whose cards carry no body, a worker sends exactly its
three lifecycle updates. Workers' engine tool calls and context/usage updates
are recorded only in durable task history for the Workers panel. They are
never relayed to the host as separate events.

Coordinator tool-call fields such as raw input, raw output, locations and
content blocks are bounded to 16 KiB each. A serialized `session/update` has
a 32 KiB hard cap. Oversized values are shortened with explicit truncation
markers, leaving the full record in task history. These limits bound individual
host events and avoid resending cumulative worker transcripts; total durable
history still grows with the work performed.

Coordinator and engine tool cards are emitted as partial updates: unchanged
title, input and output fields are omitted, and a no-op update is suppressed.
When an MCP result's text is exactly a JSON mirror of its `structuredContent`,
the text copy is omitted because Paseo presents the structured value first;
non-mirror text is retained. If a completed call repeats an earlier call's
title, input and output exactly, the emitter sends a small
`repeated_output: true` reference with `same_as` pointing to the first card
instead of copying the output. The repeat cache retains at most 64
(tool, input) keys; a cache miss sends the full bounded result again.

The number of coordinator and engine tool cards is bounded per task, not only
per card (`wire_budget.COORDINATOR_*`). The first 150 distinct calls of a task
get their own card, unless those cards have already sent 4 MiB of serialized
updates. Each card sends at most 10 updates, create included. The last slot is
kept for its first terminal update, which also carries any change held back
past the cap. Later calls are not sent as separate cards. They are counted on
one aggregate card, `call:cortex-folded:<task>`, titled "N further tool calls
not shown individually". It is created, as `completed`, at the first folded
call. Updates follow at most once per 30 seconds, and the interval doubles
after each update. The count is always refreshed when a coordinator turn ends.
The card sends at most 20 updates per task, creates included, and its last
update says the count is no longer updated. A call that fails after the budget
still gets its own card, carrying its error, for up to 50 such failures.
Later failures are counted on the aggregate card. Permission requests and user
questions are `session/request_permission` host requests, not tool cards, so
the budget never folds or drops them.

The budget belongs to the task, not to an emitter, so it survives coordinator
turns, the per-prompt emitter, a reopened coordinator session and a Cortex
restart. A new runtime seeds the budget from the task's durable host journal
with a SQLite substring prefilter. Its bookkeeping is bounded: open-card
digests and per-call dispositions are 1,024-entry LRU maps. An evicted call
that later receives an update is treated as a new call and spends budget like
one. `session/load` applies the same bound. A journal written before the budget
existed replays the first 200 coordinator cards as recorded and counts the rest
on one aggregate card. That card's `_meta.cortexReplay` names no source
sequence and gives `foldedCards`. Durable task history (task events, the Workers
panel and reports) keeps every call. Only the live and replayed host stream is
bounded.

The regression scale test (`tests/test_emission_scale.py`) drives the real
Agent over a pipe transport with a Paseo client, the fake engine and
300-character worker titles. It records each repeated `task_get_context` poll
at 481–483 bytes after the first read. Each worker card sends 3 updates and
16,959 bytes for 10, 60 and 300 tools per worker, including with 20 KB
writes, and for a 60-tool worker on a simulated clock that advances 20 s per
card decision. The largest observed update was 16,108 bytes. An earlier run
of this fixture at 10, 200 and 1,000 tools, with the title normalizer and
event budget removed, sent about 60, 1,100 and 5,500 updates per worker. The
same file drives a coordinator making 10, 500 and 1,500 distinct calls over
four prompts: a question, its continuation, a follow-up turn and a follow-up
after a runtime restart with `session/load`. Card updates were 20, 312 and
324, and card bytes 11,380, 185,337 and 187,517. The aggregate card sent 8
and 10 updates, and the bookkeeping maps stayed at or under 1,024 entries;
the 1,500-call case fills them. An earlier 5,000-call run with the budget
disabled sent 9,563 card updates and 5,846,291 bytes. A journal holding 501
legacy cards replayed as 403 updates. These are test workload results, not a bound on the
whole daemon's memory or on durable history. Paseo 0.9.2 retains provider
events in a session's in-memory history until the session closes. Smaller and
non-repeated events reduce the bytes Cortex contributes to that retained
history. Tool cards are now bounded per task, but coordinator prose, relayed
thought and plan updates, and usage updates still grow with the number of turns
and the amount of text. None of this makes total host memory constant for other
session data.
Usage and thought updates can arrive between text chunks of one sentence;
they do not insert paragraph breaks into worker prose. Tool calls remain
observable message boundaries. Paseo compares the terminal response with complete
streamed prose blocks, allowing whitespace trimmed only at block boundaries;
it suppresses repeated text while preserving paragraph breaks inside the stream
and any new final content on both live display and history replay.
Meaningful Cortex MCP, native, external MCP and private-store operations remain
visible with readable action names. File and skill reads are static target/status
rows; their result bodies are excluded from new display history, including worker
activity, and projected out of old replay. The engine still receives the read result.
Raw data is never displayed. Other calls expand only for readable output or diffs. Thinking and exact protocol
bookkeeping operations (including wait) are hidden by a shared presentation policy
in both the main chat and worker panels, including old replayed history. Other input/output and display updates are retained subject to
secret redaction. Missing, malformed or unavailable evidence is never converted
into success. The runtime rewrite and its qualification gaps are tracked in
[implementation status](runtime-rewrite-status.md).

Paseo's worker history renders a retained native edit as a line diff only when
the durable ACP event contains an exact file path plus both `oldText` and
`newText` values (including new-file and delete-file null sides). It shows the
path above the numbered context/add/delete rows. A pending edit envelope with
no retained text stays a compact `Edit · Preparing file…` row and never turns
empty `{}`/`[]` transport placeholders into a raw JSON detail view; the panel
does not infer a diff from the current file or from a status-only event.

## 2. Usage/cost attribution (`observability.usage`)

`usage_records` (also additive) holds one immutable row per completed
engine `prompt()` call
(`delegation.live_adapters.LiveEngineAdapters.prompt`) — a coordinator
continuation, a worker assignment, a retry/fallback hop, or a
recovery-capsule/clarification delivery. Every call gets its own row
(`record_call_usage`, called from a `finally` block so a raised
`EngineTurnFailed`/`PortableError` still gets attributed): retries and
recovery calls are never collapsed into, or dropped in favor of, only the
final successful attempt.

`portable.engine_contract.UsageBasis` (`OBSERVED`/`ENGINE_REPORTED`/
`ESTIMATED`/`UNKNOWN`) is the wire vocabulary; `engine.session_manager`
already translates a `usage_update` wire event into
`EngineEvent(usage_basis=UsageBasis.ENGINE_REPORTED, usage_values={...})`
for both pinned adapters. `delegation.live_adapters.prompt()` now consumes
those events (previously dropped entirely) and, when the engine reported
nothing during the call, persists `basis='unknown'` with empty
`tokens_json` — never `0`/free.

Automatic worker routing writes its full choice into the existing
`attempt_route_audit.decision_detail_json` field. The `route_decision` object
identifies its ranking algorithm with `rank_version` and records model-hint
status/reason, quota state and plan usage (`usage`, per-candidate
`usage_band`), engine affinity, `decided_by`, policy exclusions, and effort
basis. Rows written before rank version 4 may also carry count-based
`balance` and `cross_tier` fields; they are kept and read, but no longer
written. These are decision inputs and outcomes,
not usage measurements. Unknown quota is retained as unknown, never reported as
zero usage. The recovery capsule exposes this detail only when the exact
attempt's audit row is available.

Route/connection/model/config-revision are **not** duplicated onto every
usage row: `observability.usage.list_usage`/`usage_summary` join
`attempt_route_audit` (T027) on `attempt_id` at read time
(`route_basis='derived'`), so the two tables never drift apart.

## 3. Metrics and the read API (`observability.metrics`)

`observability.metrics.diagnostics_summary(store, task_id)` returns, for
every metric, its own `unit` and either `availability="available"` or
`availability="unavailable_reason"` with a plain-text reason — an
unimplemented/uninstrumented metric is reported as unavailable, never
silently omitted or defaulted to zero:

| Metric | Source | Unit |
| --- | --- | --- |
| `resource_caps` | `configuration_revisions.ceiling_snapshot` (`PolicyCeiling`) | call/token/concurrency counts; monetary cap only when `spend_limit_enforceable` |
| `recovery_latency` | `attempt_route_audit` fallback rows (`recorded_at` deltas) | seconds |
| `route_acceptance_latency` | `delegation_requested` to `WORKER_COMPLETED`, joined to `attempt_route_audit` and accepted-plan complexity | seconds, grouped by engine/model/role/complexity |
| `route_fix_cycles` | `REVIEW_CHANGES_REQUESTED` plus dispatched fix nodes and `attempt_route_audit` | review cycles, grouped by engine/model/role/complexity |
| `repeat_calls` | `usage_records` (`call_sequence` > 1 per engine session) | count |
| `schema_validation_errors` | `observability_events` category=`validation` | count |
| `manual_interventions` | `pending_questions`; permission dialogs declared `unavailable_reason` (JSONL-only, not indexed by task) | count |
| `duplicate_publications` | `observability_events` category=`publication` | count |
| `tokens` | `usage_records` (`usage_summary`) | tokens, per reported field name |
| `concurrency` | `capacity_reservations` (`state='reserved'`) | count |

Route metrics are task-local, descriptive observations; they do not feed routing decisions and are not sufficient for automatic model selection.
`route_acceptance_latency` measures from the `delegation_requested` event to the runtime's `WORKER_COMPLETED` event for execution assignments. The latter is the accepted worker-result boundary in `TaskMachine.record_result`; it does not mean the whole task was independently reviewed, approved or completed. It is therefore not end-to-end task acceptance latency.

`route_fix_cycles` attributes a review cycle to the route(s) that executed dispatched fix assignments. It groups routes that ran fixes, not routes that caused the defect or originated the preceding work; a cycle may appear in multiple groups when fixes use multiple routes. Treat both route metrics as diagnostic context, not a model quality ranking or an automatic routing signal. Reviewer nodes, failed results, and samples missing dispatch time, route audit, or a valid non-negative duration are excluded and counted. Routes are taken from the exact provisioned attempt's `attempt_route_audit`; complexity comes from that revision's `PLAN_ACCEPTED` plan, using `normal` when the optional hint was omitted.

`route_fix_cycles` counts durable `REVIEW_CHANGES_REQUESTED` events. Its route groups count distinct cycles with at least one fix assignment dispatched on that engine/model/role/complexity. One review cycle can appear in multiple groups when its fixes use different routes, so group counts are not additive. Pending fixes and dispatched fixes without route audits remain explicit in the attribution counters. Both grouped outputs cap the displayed group list at 100 and set `groups_truncated` when more groups exist.

`resource_caps` never presents an exact monetary cap unless
`configuration.policy.PolicyCeiling.spend_limit_enforceable` is `True` —
that dataclass already refuses to construct a `spend_limit` without it
(`unenforceable_spend_limit`), so a non-`None` `monetary_cap` here is
always backed by that construction-time enforcement gate. Absent that,
only the conservative `attempts`/`calls`/`tokens`/`concurrency` limits are
surfaced.

**MCP read.** A coordinator channel's `read_diagnostics` tool
(`delegation.mcp_tools._coordinator_read_diagnostics`) returns this same
summary for `channel.task_id` — never a caller-supplied task id, same
pattern as `read_capsule`/`read_large_output`. Not offered to worker
channels (resource attribution is a coordinator-level concern here).

## 4. Redaction (`observability.redact`)

A single scrubbing path, reused by every free-text sink in this runtime:

- `engine.logs.EngineLogs.observe()` (the JSONL observation log) and the
  new `EngineLogs.stderr_line()` (replacing the old direct
  `stderr.append_line` call in `engine.process.EngineProcess._drain_stderr`
  — a launched engine's own stderr is untrusted text).
- `observability.redact.scrub_text()`/`scrub_mapping()` — the T033
  Codex-plugin-harness-era `gateway.diagnostics.safe_value()` it replaced only
  stripped *characters* (`_SAFE`), which does not redact a credential-shaped
  value using only allowed characters; this scrubs *patterns* first.

`scrub_text`: URL userinfo (`scheme://user:pass@host` →
`scheme://[REDACTED]@host`), `Bearer`/`token`-prefixed credentials,
credential-shaped `key=value`/`key: value` pairs (reusing
`connections.environment.DENYLIST_PATTERN`'s vocabulary), and any
remaining long (24+ char) opaque run **containing at least one digit** —
collapsed to a stable short digest. Requiring a digit is deliberate: a
plain English/snake_case identifier (an error code, an event name) never
matches, so "keep ids, status, error category and counts" holds; this
runtime's own generated ids (`t_`/`eirt_`/`obs_`/`use_` + 12 hex chars)
are well under the 24-character floor regardless.

`scrub_command`: a full `argv` is never logged verbatim — only `argv[0]`
(the fixed, connection-pinned launch binary path) plus a digest and count
of the complete argv.

Coordinator and worker message deltas use `StreamingRedactor` before they are
placed in the ACP feed. It retains only the current lexical suffix needed to
decide whether a credential expression continues across the next update; the
holdback is bounded. The emitted result is partition-invariant for a given
source, so a provider sending one large update and a provider sending the same
text in many chunks produce the same scrubbed replay. Overlong lexical fields
are replaced by a deterministic bounded marker. Labelled SHA-256 text is kept
pending until the turn outcome is known and is preserved only for an accepted
final response whose digest also passes the pipeline guard.

`scrub_mapping`: applied to every `observe()` field mapping — a
`command`/`argv`/`launch_command`-shaped list value goes through
`scrub_command`; a string value goes through `scrub_text` (or is replaced
outright with `[REDACTED]` when its *key* itself matches
`DENYLIST_PATTERN`); nested mappings/lists recurse (bounded depth).
Nothing here ever logs a prompt/report/source body — every existing
durable evidence path in this runtime already logs a ref + digest + size
instead (see `delegation.evidence`), never a content body.

Fixture-based tests (`tests/test_observability_redaction.py`) plant a
secret in an engine's environment, a launch argv, report/source text, and
simulated engine stderr, and assert nothing leaks through `EngineLogs`,
`observability.redact`, or `store.py`'s `create_task` redaction
path, while ids/status/error-category fields survive unredacted.

## 5. Missing telemetry never gates execution

`delegation.live_adapters.prompt()` records `basis='unknown'` and still
returns the engine's actual `PromptOutcome` — a `claude-agent-acp`/
`codex-acp` turn that emits no `usage_update` at all (either adapter, per
T024's own dist-evidence findings) completes exactly as it would with
usage reported; `observability.metrics.diagnostics_summary`'s `tokens`
section reports `availability="unavailable_reason"` in that case, never
`0`/free. Conservative caps (`PolicyCeiling.limits`: `attempts`/`calls`/
`tokens`/`concurrency`, enforced by T024/T027's own attempt/concurrency
gates) remain in force regardless of whether usage telemetry exists.
`tests/test_observability_usage.py` exercises this directly: a fake
adapter that never emits `usage_update` still lets `prompt()` return a
successful outcome, and the resulting `usage_records` row is
`basis='unknown'`.

## What's deferred

- Exact per-provider $-denominated cost (no adapter reports one; this task
  never fabricates a conversion rate).
- Permission-dialog counts as a queryable-by-task SQLite metric (currently
  JSONL-only per engine instance — see `manual_interventions` above).
- Paid live probes; this task requires no paid
  model invocation.

Worker history resolves the exact `delegation_requested` mapping from the
public delegation reference to its plan assignment and graph attempt, then
to its provisioned engine attempt. Explicit plan IDs are not interchangeable
with delegation references. Switching workers uses that attempt’s own event
cursor; structured native tool errors remain visible as text details.

Paseo's worker history and pipeline read the store the runtime used for the
session. The runtime keys that store by the logical project root of the session
cwd (`logical_project_root`), so a session opened in a linked `git worktree`
writes to the main repository's `.cortex/cortex.sqlite3`, not to one under the
worktree. The plugin resolves the workspace directory the same way, read-only,
before it opens the store. A forged, ambiguous or unsafe worktree layout gets
"unavailable". The plugin never guesses a different store and never creates
one. `tests/test_paseo_worktree_store.py` checks that the plugin resolves the
same root as the runtime.

Only the store is shared. The session records the worktree it was opened in
(`acp_session_workspaces`), and the task it creates is bound to that worktree in
the same transaction (`task_workspaces`, see `cortex_runtime/task_workspace.py`).
The binding is permanent. Every file-side consumer uses the task's tree: the
coordinator and worker engines' cwd, the gateway's `project_read`/`project_write`/
`project_delete`/`project_mkdir`/`project_skill_link`/`command_run`/`project_git`, verification,
review/completion revision captures, project docs catalogues and `resource_link`
containment. `tasks.project_root` and every store key stay the main repository.
A task without a binding (created before this existed, or opened directly in the
main checkout) works in the project root. `session/load` from a different
worktree of the same repository is refused as an unknown session. When the
engine jail's root is a worktree, the main repository's `.cortex` (otherwise
visible through the host-wide read-only mount) is masked with a read-only tmpfs.

Worker recovery does not replay terminal requests or provision an old request
under a new coordinator channel. Worker history filters displayable events
before bounding each page, and selected completed transcripts continue loading
while a further page exists. Catalogue polling never advances all workers with
one shared event cursor.

After a successful observed Cortex delegate call, trailing coordinator prose
is withheld until another tool starts; the worker card represents the handoff.
The engine turn is not cancelled, so further delegations still execute. This
presentation rule does not affect acceptance, worker evidence, or final replies.


## Paseo invoke latency and request coalescing

The plugin's GUI RPC handlers that previously used synchronous child-process calls now use asynchronous `execFile` with bounded concurrency. Worker-history identities are cached, and overlapping worker-history and pipeline poll requests are coalesced. `cortex-acp.invoke` timeout diagnostics identify the RPC method so a slow history read can be distinguished from other handlers. These measures remove the observed event-loop serialization cause and reduce duplicate work; they do not by themselves establish a timeout-free workload or a rendered GUI result. Use the plugin typecheck and unit-test commands in [verification guidance](../project/verification.md), and inspect the actual Electron timeline for GUI claims.

## Bound task inspection

`scripts/cortex_inspect_task.py` is a no-model-call, bounded metadata reader.
Use it only on a newly owned disposable qualification project with an actual
read grant. Obtain the exact ACP session from the owned host's current
`runtimeInfo`, using the [dev binding exporter](dev-paseo-environment.md#read-only-dev-binding)
and the existing strict `parsePaseoRuntimeSessionId` wrapper parser. The exporter
fetches/refetches only the exact owned agent and rechecks guarded durable ownership.
It returns host/wrapper, endpoint and binding digests with explicit provenance. An opaque host
agent ID or an acknowledged send alone is not an ACP/task binding. If the
installed CLI does not expose that observation, verify the existing plugin API
against the installed host; do not guess an ID, reopen a denied store or infer
binding from completion. This helper adds no host API or daemon startup.

Once the exact session is observed, discovery can read its task/current owner
without guessing either identity:

```bash
python3 -B scripts/cortex_inspect_task.py --discover --project "$PROJECT" --session "$ACP_SESSION"
```

`--discover` cannot be combined with task/epoch/cursor or non-default limit
arguments. It shares the same guarded, non-mutating storage refusal boundary;
it does not enumerate sessions or choose the newest task. Use its returned
binding for the explicit paged command below; ownership can change afterward.

```bash
python3 -B scripts/cortex_inspect_task.py --project "$PROJECT" \
  --session "$ACP_SESSION" --task "$TASK" --owner-epoch "$OWNER_EPOCH" --limit 100
# Continue with the exact returned next_cursor, retaining every page:
python3 -B scripts/cortex_inspect_task.py --project "$PROJECT" \
  --session "$ACP_SESSION" --task "$TASK" --owner-epoch "$OWNER_EPOCH" \
  --cursor "$NEXT_CURSOR" --limit 100
```

The project must be an absolute logical project root. Session/task/epoch must
match durable current ownership. A page reports `offset`, `total`,
`snapshot_sha256`, `next_cursor` and collection `complete`; only the last page
is complete. Retain all pages with that same digest, checking offsets/counts.
Any projected change invalidates the cursor. Wrong/missing/stale bindings,
unsafe paths, unsupported/legacy storage, more than 10,000 metadata rows,
over 32 MiB of source JSON per inspected stream or 4 MiB of projected metadata,
busy access locks and nonempty WALs refuse instead of yielding a truncated
success. The reader never creates files, checkpoints WAL, migrates or repairs.
The dev binding wrapper can retain a complete exit-1 inspector's allowlisted
refusal reason as `diagnostic` while still returning no binding. It also reports
bounded local endpoint/spawn/subprocess/output stages without forwarding private
exception text or treating a zero child exit as complete binding evidence; see
[refusal diagnosis](dev-paseo-environment.md#diagnose-a-refused-binding-without-bypassing-it).
A historical generic refusal does not establish busy storage, WAL or any other cause.
Wait for a checkpointed snapshot through the normal runtime connection lifecycle
and re-observe; do not stop a completed task to manufacture timestamps.

The projection contains configuration revisions (selected/pending/ACTIVE kept
distinct), dispatch audits, provisioned attempts, plan/review node identities,
report edition digests/sizes, verification command hashes/exit/completeness and
retained-output identities, synthesis hash, source identity hashes, ordered
task/runtime event IDs, action-receipt completeness, process-termination
observations and reservation releases. No report bodies, prompts, raw tool
arguments/output or private host logs are returned. Event payload hashes retain
identity without exposing those bodies. Missing records remain missing.

Dispatch audit is intent, not engine confirmation. Successful worker/reviewer
native configuration now appends `worker_configuration_confirmed` with the
exact attempt/session/config/connection/model, applied effort/mode and actual
confirmation reference. It is recorded only after the engine confirm-or-fail
call returns evidence. Coordinator confirmation remains the ACTIVE revision's
reference. Neither host Thinking text nor a dispatch effort substitutes for
these observations. Unknown native parameter keys are hash-only.

`closed_at` is not a transport disconnect receipt. Use a retained
`host_disconnected` event and its history-completeness/unknown-action counts;
truncated history or incomplete action receipts block qualification. The
inspector does not probe `/proc`, prove native resource liveness, verify report
body bytes, or invent SIGKILL telemetry. It has no acceptance verdict. Missing
host/cleanup/engine evidence still requires an explicit limitation, not a pass.
