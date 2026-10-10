# Observability and resource attribution

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed.


This page describes how route use, recovery, resource limits, and uncertainty
observable without exposing secrets or inventing telemetry. It never
replaces an existing durable evidence path (the `task_events` log,
`attempt_route_audit`, `context_generations`/
`capsule_deliveries`, ...); it reads and joins those, plus two new tables,
to build the metrics and attribution
require (`src/cortex_runtime/observability/`).

## 1. Structured events (`observability.events`)

`observability_events` (additive table in the current format-12 store; the metadata signals in this section add no schema change) is an
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

The current append sites are:

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

### Metadata signals for loop and write measurement

These records make gateway, engine-turn and write activity measurable. Each is
metadata only: none stores file content, tool arguments, reply bodies, prompt
text or credentials. A signal the runtime cannot observe is reported as
unavailable or partial, never as zero.

- **`project_read`** (`category='project_read'`, `basis='observed'`). One row per
  gateway `project_read` call, written in the call's own transaction
  (`orchestration/gateway.py`, `_record_project_read`). `outcome` is the kind:
  `text`, `dir`, `image`, `binary`, `missing` or `failed`. `detail` carries only
  `path`, `kind` and, where meaningful, `sha256`, `start_line`, `end_line`,
  `complete`, `size_bytes` or an error code. Row volume follows call count, not
  file size. Native engine reads never pass through the gateway and produce no row.
- **`gateway_refusal`** (`category='gateway_refusal'`, `basis='observed'`). A
  refusal before a mutation: schema validation, a mutation wait, a scope denial
  (`gateway_scope_denied`) or a request-key conflict (`request_key_conflict`).
  `outcome` and `detail.error` carry the error code; `detail` holds the operation
  name and error code only. Recording is best-effort and never changes the refusal
  the caller receives.
- **`external_refusal` and `external_error`** (`engine/external_proxy.py`). A
  denied external MCP tool (`external_refusal`), or a denied method, timeout,
  closed pipe, oversized frame or response mismatch (`external_error`). `detail`
  holds `server`, `tool` and `error` only.
- **Write size.** The `TOOL_COMPLETED` result of `project_write` and
  `project_edit` also carries `changed_bytes` and `changed_lines`, computed from
  the common prefix and suffix of the old and new content. Creation gives
  `changed_bytes` equal to the byte count. Only sizes are stored; no diff is added.
- **`EXTERNAL_TOOL_AUTHORIZED.arguments_sha256`.** SHA-256 of the canonical
  `{name, arguments}` object, without `_meta`, so repeats with different client
  metadata get the same value. The `runtime_requests` receipt digest is unchanged
  and still covers all of `params`. The digest is unkeyed: a guessed low-entropy
  argument set can be confirmed against it. Use it to detect repeats, not as
  redaction.
- **`COORDINATOR_TURN_ENDED`** (runtime event). Each coordinator `prompt()` call
  that recorded `COORDINATOR_TURN_STARTED` gets one paired end event carrying
  `stop_reason` when the call returned, or `failure_kind` (the exception class
  name) when it raised (`orchestration/controller.py`).
- **`FIRST_MODEL_OUTPUT` gating.** Recorded only for the first
  `agent_message_chunk`, `agent_thought_chunk` or `tool_call` update of an attempt
  (`engine/session_manager.py`, `_FIRST_MODEL_OUTPUT_UPDATE_KINDS`). Its payload
  carries `update_kind` only. The earlier rule, the first update of any kind, could
  record a non-model update; rows already written are not reclassified.
- **Usage across inner turns.** Still one `usage_records` row per `prompt()` call.
  When a stall-nudge or refusal continuation runs several inner engine turns inside
  that call, the row's tokens are the sum of the turns' reported usage
  (`delegation/live_adapters.py`, `_bank_turn_usage`); previously only the last inner
  turn counted. The basis stays `unknown` with empty tokens when no inner turn
  reported usage. When some inner turns reported usage and another inner turn
  reported none, the basis is also `unknown` with empty tokens: the sum would omit
  that turn's contribution, so it is never recorded as `engine_reported`. Only a call
  whose inner turns all reported usage keeps `engine_reported` and the summed tokens.

**Engine limits.** Claude's native Bash, Read, Edit, Write, Grep, Glob and
NotebookEdit tools are disabled by configuration (`engine/claude_adapter_acp.py`);
this page does not re-verify live compliance. The Codex CLI version examined in the
observability audit (0.154.0, `engine/process.py`) cannot disable `unified_exec`,
so Codex native `exec` reads appear only as ACP-reported events, and Codex
`apply_patch` edits have no runtime receipt. Codex read and edit figures are
therefore partial and must be labelled so. ACP v1 has no per-inference boundary:
model steps between tool calls are estimated from gaps between tool calls and are
never counted.

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
paths cannot send two spellings of the same title. A live qualification run found that alternating clipped and unclipped copies of one
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
[verification status](../../project/verification.md).

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
`ESTIMATED`/`UNKNOWN`) is the wire vocabulary;
`engine.session_manager` translates recognized token fields in a settled
`session/prompt` response (or a token-bearing `usage_update`) into
`EngineEvent(usage_basis=UsageBasis.ENGINE_REPORTED, usage_values={...})`.
Top-level `usage_update.used/size` is context occupancy only. Field values are
copied unchanged; their scope is the adapter's: Claude reports the whole turn,
Codex only the turn's last model request (see
[adapter token semantics](../token-and-plan-usage/index.md#adapter-token-semantics)).
`delegation.live_adapters.prompt()` consumes token events and,
when the engine reported nothing during the call, persists
`basis='unknown'` with empty `tokens_json` — never `0`/free.

After an accepted `result_submit`, the runtime gives the exact worker/reviewer
prompt consumer up to 15 seconds to persist usage on a separate cleanup thread;
a turn that ends sooner settles at once.
Dispatch is not synchronously blocked by this banking, but capacity and writer
locks remain reserved until confirmed exit. Explicit cancellation bypasses the
grace period; a timeout retains a `usage_settlement` progress diagnostic with
reason `post_result_prompt_settlement_timeout` and `usage_coverage:
"unavailable"` (teardown ends the engine before its only token carrier).
Runtime shutdown interrupts the wait without that diagnostic and waits a
bounded time for settlement teardown already in progress. The jailed engine's
authority watchdog does not preempt this: a channel revoked by the attempt's own
accepted result keeps the engine for at most 20 seconds, so the turn's final
`PromptResponse.usage` can be recorded as `engine_reported` with stop reason
`end_turn`; any other revocation still terminates at once.
See [settlement and historical evidence limits](../token-and-plan-usage/index.md#coverage-and-interrupted-turns).
Teardown can still interrupt an unfinished turn before its per-call usage arrives.
When teardown raises and the exact attempt already has a
`worker_result`, `prompt()` records `failed=False` and
`stop_reason='released_after_result'`. Without a real figure the basis
stays `unknown` with empty tokens; an already-observed `engine_reported`
figure is kept. This uses only existing columns and the original closed
basis vocabulary, so existing stores need no CHECK-constraint migration,
format change or recreation.

`usage_summary` derives `released_after_result_calls` from non-failed
unknown rows with that exact stop reason, distinct from
`unknown_basis_calls`. Token metrics remain unavailable when neither
group has a real reported figure. Paseo's worker-history and pipeline
totals omit only those marked release rows from `knownCalls` and
`unknownCalls`, preventing a false "call unavailable" alarm on successful
worker cards. Genuine unknown calls and failed rows still count, even if
a failed row has that stop reason. Historical unmarked rows remain
unknown; no usage or cost is inferred, and missing figures are never
zero/free.

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
`attempt_route_audit` on `attempt_id` at read time
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
| `repeat_calls` | `usage_records` (`call_sequence` > 1 per engine session) | count of ACP `prompt()` re-deliveries per engine session, never repeated tool calls (see `loop_profile`); `truncated` is set when the 2000-row read cap is reached |
| `schema_validation_errors` | `observability_events` category=`validation` | count |
| `manual_interventions` | `pending_questions`; permission dialogs declared `unavailable_reason` (JSONL-only, not indexed by task) | count |
| `duplicate_publications` | `observability_events` category=`publication` | count |
| `tokens` | `usage_records` (`usage_summary`) | tokens, per reported field name |
| `concurrency` | `capacity_reservations` (`state='reserved'`) | count |
| `loop_profile` | `TOOL_AUTHORIZED`, `EXTERNAL_TOOL_AUTHORIZED`, `project_read` rows, `tool_result_observations`, completion timestamps | per attempt: call counts by class, inference-gap estimate (count, sum, p50, p90 seconds), observed tool time |
| `reinvestigation` | `project_read` rows (path and `sha256` per call) | repeat-read ratios within an attempt and across attempts of one task; `partial` for non-Claude engines |
| `write_redundancy` | `TOOL_COMPLETED` of `project_write`/`project_edit` (bytes and changed bytes) | gateway byte ratios, separate project-write population; native share unavailable without classification |
| `critical_path` | `runtime_events` (per-class interval union, user waits excluded) | seconds and share of task wall time per class |

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

### Loop, re-investigation and write aggregates

`diagnostics_summary`, and therefore the coordinator's `read_diagnostics`, adds
four sections. Each has `unit`, `availability` and `basis`; an unobservable measure
is `unavailable_reason` with a reason, never 0.

- **`loop_profile`** (per attempt). Gateway, project-read and external counts come
  from their authoritative tables. `acp_reported_calls` includes their ACP echoes,
  so it is never added to gateway counts; `tool_calls_for_decision` uses the
  distinct ACP count. Generic ACP kinds do not identify delegation or native tools.
  Estimated inference gaps include streamed text and adapter latency; incomplete
  or concurrent spans withhold the proxy. Each execution attempt uses its own
  duration, excluding user waits; review attempts are not execution samples.
  Gateway durations bind actor and receipt; external durations bind actor, server
  and request ID, requiring unique ordered boundaries. The ACP, gateway-duration
  and external-duration queries have separate 5,000-row truncation flags.
- **`reinvestigation`**. Per-call gateway read rows support within-attempt repeats
  and repeats by another exact channel of the same task. Decision inputs use text
  reads in dependent execution attempts. Read time is estimated only when the read
  event lies in one complete ACP read span, containing no other read event; an
  absent or ambiguous binding withholds the time condition. The aggregate ratio
  is call-weighted; repeated-read seconds are divided by the summed execution
  attempt durations, with user waits excluded. Unknown engine identity and
  non-Claude engines are partial; native reads are not invented. The single-task
  cross-task field remains unavailable; multi-task mode uses retained per-call
  text-read history, not the overwritten last-read state.
- **`write_redundancy`** (derived). The overall ratio covers gateway
  `project_write` and `project_edit`; `project_write_redundancy_ratio` and the
  edit-tool gate use only `project_write` bytes and changed bytes. Gateway shares
  are separate from all-write shares. ACP `kind=edit` cannot identify Codex
  `apply_patch` or exclude gateway echoes, so native share is unavailable.
  All-write share is available only for a known Claude-only population, relying
  on configured native-file-tool disablement, not live proof of compliance.
- **`critical_path`**. First-to-last recorded task wall time excludes recorded
  user waits from every numerator and the denominator. Each class and the combined
  orchestration/review-verification classes use interval unions, not summed
  overlapping shares. Failed-attempt cost is separate. Worker boundaries bind
  exact attempt/session identity; a terminal lacking a session can use the single
  ready session of its explicit attempt. Ambiguous, absent, open or truncated
  phases are unavailable, not zero; a closed interval wholly inside a user wait
  can legitimately be zero. Phase-chain eligibility requires a completed task,
  completed execution attempt, same-revision completed review and passed
  verification after execution. Explicit closed scheduler observations can
  establish zero scheduler-wait time. Capped at 10,000 events.

`repeat_calls` now states what it counts: ACP `prompt()` re-deliveries per engine
session, not repeated tool calls. Its `truncated` flag is set when the 2,000-row
usage read cap is reached.

## 4. Redaction (`observability.redact`)

A single scrubbing path, reused by every free-text sink in this runtime:

- `engine.logs.EngineLogs.observe()` (the JSONL observation log) and the
  new `EngineLogs.stderr_line()` (replacing the old direct
  `stderr.append_line` call in `engine.process.EngineProcess._drain_stderr`
  — a launched engine's own stderr is untrusted text).
- `observability.redact.scrub_text()`/`scrub_mapping()` — the former
  `gateway.diagnostics.safe_value()` it replaced only
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
`scrub_mapping`'s own scope is `observe()`'s field mapping, and it never logs a
prompt/report/source body there. That narrow guarantee does not extend to every
durable evidence path in this runtime: the external MCP broker persists the
complete tool-call reply twice, once in `runtime_requests.result_json` and once
in the `EXTERNAL_TOOL_RESULT` event payload
(`src/cortex_runtime/engine/external_proxy.py:236-242`), and
`orchestrating_runtime`'s worker-activity panel event persists the scrubbed but
still content-bearing `raw_input`/`raw_output`/`content` fields rather than a
ref + digest + size
(`src/cortex_runtime/acp/orchestrating_runtime.py:3516-3537`). Treat
ref + digest + size as this page's policy for the new measurements it adds,
not as an existing guarantee across every stored record.

Fixture-based tests (`tests/test_observability_redaction.py`) plant a
secret in an engine's environment, a launch argv, report/source text, and
simulated engine stderr, and assert nothing leaks through `EngineLogs`,
`observability.redact`, or `store.py`'s `create_task` redaction
path, while ids/status/error-category fields survive unredacted.

## 5. Missing telemetry never gates execution

`delegation.live_adapters.prompt()` records `basis='unknown'` and still
returns the engine's actual `PromptOutcome` — a `claude-agent-acp`/
`codex-acp` turn that emits no `usage_update` at all (either adapter, per
the adapter distribution evidence) completes exactly as it would with
usage reported; `observability.metrics.diagnostics_summary`'s `tokens`
section reports `availability="unavailable_reason"` in that case, never
`0`/free. Conservative caps (`PolicyCeiling.limits`: `attempts`/`calls`/
`tokens`/`concurrency`, enforced by the attempt and concurrency
gates) remain in force regardless of whether usage telemetry exists.
`tests/test_observability_usage.py` exercises this directly: a fake
adapter that never emits `usage_update` still lets `prompt()` return a
successful outcome, and the resulting `usage_records` row is
`basis='unknown'`.

## What's deferred

- Exact per-provider $-denominated cost (no adapter reports one; this page
  never fabricates a conversion rate).
- A Cortex-delegation call count per attempt (`loop_profile.cortex_delegation_calls`):
  needs a per-call event that separates Cortex MCP operations from engine-native calls.
- The cross-task repeat ratio: needs a durable cross-task (path, sha) read history.
- Per-read history for Codex native `exec` reads, and per-attempt context-fill
  samples (the optional context-fill recording was not implemented).
- Live completeness of ACP tool events per adapter version: needs a live run the
  owner has requested.
- Permission-dialog counts as a queryable-by-task SQLite metric (currently
  JSONL-only per engine instance — see `manual_interventions` above).
- Paid live probes; this page requires no paid
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

The plugin's GUI RPC handlers that previously used synchronous child-process calls now use asynchronous `execFile` with bounded concurrency. Worker-history identities are cached, and overlapping worker-history and pipeline poll requests are coalesced. `cortex-acp.invoke` timeout diagnostics identify the RPC method so a slow history read can be distinguished from other handlers. These measures remove the observed event-loop serialization cause and reduce duplicate work; they do not by themselves establish a timeout-free workload or a rendered GUI result. Use the plugin typecheck and unit-test commands in [verification guidance](../../project/verification.md), and inspect the actual Electron timeline for GUI claims.

## Bound task inspection

`scripts/cortex_inspect_task.py` is a no-model-call, bounded metadata reader.
Use it only on a newly owned disposable qualification project with an actual
read grant. Obtain the exact ACP session from the owned host's current
`runtimeInfo`, using the [dev binding exporter](../../project/dev-environment.md#read-only-dev-binding)
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
[refusal diagnosis](../../project/dev-environment.md#diagnose-a-refused-binding-without-bypassing-it).
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

## Orchestration phase measurement

Inspect a checkpointed format-12 project store without a model call:

```bash
python3 -B scripts/cortex_timeline.py --project "$PROJECT" --task "$TASK"
python3 -B scripts/cortex_timeline.py --project "$PROJECT" --task "$TASK" --json
```

This reader uses the existing guarded immutable snapshot: SQLite URI
`mode=ro&immutable=1`, query-only, an existing access lock opened read-only,
and no Store initialization, migration, repair, checkpoint or file creation.
Unsafe paths, a busy lock, nonempty WAL, unsupported/legacy storage, a missing
task or more than 10,000 events refuse (exit 1), never return partial success.
Wait for the normal runtime to checkpoint; do not bypass a refusal.

Only allowlisted event metadata is projected. Text and JSON show coordinator
turn → plan, plan → baseline, assignment → engine ready → first update → first
gateway tool → completed → released, review, verification and synthesis
intervals. Exact event IDs and timestamps are returned with durations. Missing
endpoints remain `missing` with null durations; duplicate milestones are not
selected as a success. No start time is synthesized from task creation.
Historical runs without the new milestones cannot split startup latency.
Overlapping workers are shown separately, not summed into a claimed critical
path; this is a phase breakdown for analyzing the critical path, not a
reconstructed DAG critical-path verdict. Native engine actions are not gateway
tool receipts, and their first-call timing is not inferred.

### Multi-task aggregates

```bash
python3 -B scripts/cortex_timeline.py --project "$PROJECT" --aggregate --json
python3 -B scripts/cortex_timeline.py --project "$PROJECT" --aggregate --task "$TASK" --task "$TASK_2" --json
```

`--aggregate` uses the same guarded snapshot read as the single-task mode and refuses
in the same way (`unsafe_storage`, `legacy_storage_migration_required`,
`unsupported_storage`, `inspection_uncheckpointed_wal`, `inspection_busy`), plus
`timeline_task_missing` and `timeline_aggregate_overflow` above 500 tasks. Without
`--task` it covers every task in the store up to that cap. For each task it calls
`observability.metrics.diagnostics_summary` and prints counts, ratios, durations,
availability and task IDs only: no paths, report bodies or logs.

The output has two parts:

- `cohorts`: separate populations by dispatched engine, connection, model and full
  parameters fingerprint (including effort). Missing route identity is unattributed,
  not pooled. The validity gate requires 10 completed tasks or 30 execution attempts
  and at least 80% exact completed, non-truncated phase chains per cohort.
- `token_coverage`: complete retained-call counts and known token-field subtotals
  by audited engine and role, including coordinator and review. Missing engine
  or role evidence stays explicitly unattributed. Released calls remain missing
  for cost coverage even when the UI omits their alarm. Known subtotals never
  represent full verified-result cost with missing calls. See
  [interrupted-turn accounting](../token-and-plan-usage/index.md#coverage-and-interrupted-turns).
- `validity`: execution-worker token coverage counts calls, not tasks. Unknown-basis plus
  released-after-result calls must be at most 20% of that cohort's calls before
  token criteria are usable; missing call coverage withholds token criteria.
- `branches`: each cohort evaluates only its eligible execution chains. Missing
  approved input withholds the affected branch (`availability=unavailable_reason`,
  `holds=null`), not a false verdict. The three branches are:
  - `worker_loops`: holds when the mean worker-execution share is at least 50%, the
    median tool calls per execution attempt is at least 15 and the mean per-attempt
    inference-gap share of worker execution is at least 40%. Its edit fields
    (`code_apply_recommended`, `promote_project_edit`) also need `project_write` write
    redundancy of at least 5 and a `project_write` share of writes of at least 30%.
    `code_apply_recommended` keeps the name of an earlier plan: `true` means the
    file-edit gate holds, not that a new tool exists.
  - `reinvestigation`: holds when the mean within-task repeat ratio of dependent text
    reads is at least 25% and the repeated-read share of worker-execution time is at
    least 15%. `cross_task_reuse_considered` is `true` only when a retained per-call
    cross-task repeat ratio is at least 30%, and `null` without usable history.
    The read-count population is dependent attempts only; the time denominator
    is all eligible worker execution. Independent repeats remain descriptive
    per-attempt observations and cannot satisfy this gate. No dependent reads
    withholds the branch rather than manufacturing zero. Unknown dependency
    status also withholds it. Codex read evidence stays `partial` and is never
    pooled with Claude's.
  - `orchestration_chain`: holds when the orchestration share of critical-path
    wall time is at least 35%, or review plus verification is at least 25%.
  The top-level verdict mirrors a cohort only when there is exactly one cohort and
  no unattributed attempt. Otherwise the top-level branches are withheld and each
  cohort's own verdict applies.
- **Not computed by the reader.** The several-branch rule (take the largest share;
  do both when two are within 5 percentage points; stop when none qualifies), the
  pilot criteria and any branch that uses token figures are outside `--aggregate`.
  `token_criteria_usable` is reported per cohort, but no branch consumes it yet.
  Mixed cohorts have no pooled verdict. The owner applies these remaining rules to
  the reported branch values.
  Thresholds are recorded in
  [decisions](../../project/decisions.md#2026-10-09-observability-decision-gates-and-the-edit-tool-decision).

Run it as the owner, outside the worker sandbox, on a checkpointed store. The
observability audit saw the store refused as `unsafe_storage` from inside a worker
sandbox.

**Observation limits.** Historical tasks may lack the required phases, read
identities or ACP durations. No native-read history is reconstructed. Cross-task
history is available only for retained per-call text reads from at least two tasks;
truncation withholds the ratio. Current producer metadata does not always allow a
unique project-read-to-ACP-span binding, so the re-investigation time branch can
remain unavailable even with a known repeat ratio. Dispatch parameters identify
comparable cohorts; they are not native configuration confirmation. These outputs
do not change scheduling, authorization, independent review or final verification.

Additive `runtime_events`:
- `COORDINATOR_TURN_STARTED`: immediately before a planning/reconciliation
  prompt, carrying revision, state and engine session identity.
- `ENGINE_READY`: the downstream process has started, initialize and
  session/new have acknowledged a valid session. This is not native model
  configuration confirmation.
- `FIRST_MODEL_OUTPUT`: the first correlated session/update from that engine,
  including non-text updates; it is **not** a first-token assertion.
  Both worker/reviewer milestones carry only assignment, attempt and engine
  session IDs; created_at supplies the timestamp. They are transactionally
  deduplicated per actual attempt. Updates before session/new identity is
  available are not attributed or fabricated.
- `SCHEDULER_WAIT_REASON`: changes only, keyed by revision/node or attempt.
  `dependency`, `scope_overlap` and `capacity_limit` are actual scheduler
  decisions; `route_capacity` means the selected connection/session
  reservation refused capacity (not proof of provider quota exhaustion).
  A null reason clears an observed wait. Node waits for `scope_overlap` and
  `capacity_limit` also carry `blocked_by` (holder kind, task, node, role,
  objective, running state and the overlapping held/requested paths, or the
  worker limit); a change of holder is recorded as a new wait. The chat shows
  these waits as runtime "Waiting" cards (see
  [orchestration](../orchestration/index.md)); the cards only present recorded events. Legacy standalone adapter calls
  without a runtime task retain their existing progress event only.
- `WORKER_COMPLETED`/`WORKER_FAILED` retain their prior fields and now
  additionally carry attempt_id, preventing cross-attempt completion matching.

These are metadata-only observations in normal runtime storage, not full dev
wire logging. No prompt, tool content, credentials or private report body is
added; acceptance, authority, scheduling limits and process release semantics
are unchanged.

## Developer-only wire tracing

Wire tracing is **off by default**. Disabled instrumentation does not create
directories or files and only checks the trace flag. There is no routing,
production settings or MCP tool switch for it. The guarded wrapper clears
inherited trace opt-ins on ordinary calls. Enable it only on a fresh
`~/.paseo-dev` daemon start, between active tasks:

```bash
python3 -B scripts/cortex_paseo_dev.py --wire-trace -- start --timeout 60 --json
```

This command does not authorize a paid workload. Follow the
[dev environment rules](../../project/dev-environment.md) before any launch.
An already-running daemon does not acquire a new environment; stop it before
an explicitly authorized traced start. To disable tracing, stop that dev daemon
and start it through the wrapper without `--wire-trace`.

The wrapper accepts this opt-in only for `start` or `daemon start` at the
exact canonical `~/.paseo-dev` home, not alternate homes, GUI, status or binding
commands. The runtime additionally requires the matching dev config/state XDG
paths, the wrapper's explicit source root, and a real checkout with `.git`
and the guarded script. Installed production payloads/state refuse even with
the opt-in. These are source/state launch checks, not a cryptographic proof
of environment provenance; the account owner controls their environment.

JSONL goes only to
`~/.paseo-dev/cortex-xdg/state/cortex/wire-traces/`: private owned 0700
directories, 0600 files, no symlinks/hardlinked files, outside every Git
repository and project `.cortex` store. Existing non-private targets refuse
rather than being chmodded. All processes share a locked rotation ring:
`trace.jsonl` and three numbered predecessors, each at most 16 MiB
(64 MiB total plus an empty lock). Rotation replaces old history.
A record too large for one file becomes an explicit
`record_exceeds_rotation_size` diagnostic, never silently a full capture.
Path/redaction/write failure stops that process's trace with a sanitized stderr
diagnostic; it cannot change protocol, command or storage outcomes.

Channels and directions:
- `upstream_acp`: parsed client → Cortex (`in`), successfully written
  Cortex → client (`out`) JSON-RPC at Cortex's stdio boundary, including
  requests, responses and notifications.
- `downstream_acp`: the same traffic between Cortex and each engine,
  including initialization, session setup, updates, tool events and errors.
- `mcp`: actual Cortex MCP request/reply frames, including `tools/call`
  arguments, complete results and JSON-RPC errors.
- `verification`: literal argv/cwd (`out`), complete combined stdout/stderr
  lines and exit/interruption status (`in`); setup/launch failures are explicit.
- `lifecycle`: runtime event insert attempts (`internal`), labelled
  `transaction: pending`. They are **not** commit receipts; SQLite remains
  authoritative.

Rows include UTC timestamp, monotonic time, PID, direction/channel, session,
channel, task, assignment and attempt IDs, with engine-session/connection or
command references when available. Pre-session/pre-task/unbound IDs are null,
not guessed. Response correlation is bounded to 1,024 requests per transport;
evicted or unavailable identities stay missing. Malformed/oversized input has
a diagnostic rather than an invented parsed body.

Credentials are redacted before serialization: sensitive keys and environment
name/value pairs, inherited and runtime-resolved engine secret-looking environment literals,
auth headers,
Bearer/Basic values, token/API-key/password argv, relay tokens, embedded JSON,
common key formats, URL userinfo/credential claims through the shared redactor,
and verification private-key blocks. Learned credential
literals are also removed from echoes. Exhausting the bounded secret registry
stops capture. Redaction is not a guarantee against arbitrary unfamiliar
secrets or secrets fragmented across independent messages. Traces can contain
private prompts, report/file bodies and model thought updates: never commit,
publish or include raw traces in reports. Native actions invisible on these
wires are not synthesized as command receipts.

Read existing traces without a daemon, Store initialization or writes:

```bash
python3 -B scripts/cortex_trace_view.py --task "$TASK" --assignment "$ASSIGNMENT" --direction in --pretty
python3 -B scripts/cortex_trace_view.py --channel verification --pretty
```

Optional `--directory` selects an existing private trace directory; unsafe
targets and invalid/incomplete JSON return exit 1 (earlier output may be partial).
Stop the writer before exact analysis: concurrent rotation can change which
generations a reader sees. Rotation, explicit gaps or missing IDs prevent
claims of complete retained history. Traces are debugging data, never
acceptance evidence or substitutes for independent review and verified receipts.

## Runtime-composed-synthesis share and fallback reasons

`scripts/cortex_timeline.py --aggregate` reads, per engine/connection/model/
parameters-fingerprint cohort, the already-retained
`SYNTHESIS_RUNTIME_COMPOSED`/`SYNTHESIS_RUNTIME_DECLINED` events (see
[runtime-composed synthesis](../task-lifecycle/index.md#runtime-composed-synthesis))
for every task that reached RECONCILING: each cohort's `runtime_synthesis`
reports `reconciled_tasks`, `composed_tasks`, `pending_tasks`, their `share`, and a
`fallback_reasons` tally keyed by the decline's first reason code
(`worker_live`, `replan_history`, `repair_path`, `pending_input`,
`node_incomplete`, `review_not_approved`, `verification_not_passed`,
`open_check`), plus the explicit `unrecorded` key for a completed task with
neither event retained. This includes current-build missing-baseline or
oversized-text returns and post-composition rollback, not just older builds.
Decline reasons are not exhaustive: the early live-worker short-circuit records
only `worker_live`, and an event retained once per revision is not updated by
later attempts. A temporary condition recorded on an earlier attempt need not
explain the final fallback; `unrecorded` is absence of retained diagnostics, not
an inferred cause. An unfinished reconciliation carries
`reconciled=True`/`composed=None`/`fallback_reason=None`, even if a decline
was already recorded. It counts only in `reconciled_tasks` and `pending_tasks`,
never as fallback or `unrecorded`. The share uses all reconciled tasks as its
denominator but stays `null` while any are pending (or none are reconciled),
rather than treating missing outcomes as failures or excluding them to inflate
success. Completed-only cohorts retain their existing share. A task not yet reconciled carries
`reconciled=False`/`composed=None`/`fallback_reason=None` and is excluded from
the cohort's counts. Several cohorts withhold a pooled verdict the same way the
existing branch decisions do; read each cohort's own `runtime_synthesis`. This
is metadata only (event types and reason codes): never a report body, path
content or raw log.
Runtime event production and the reader's guarded read-only snapshot boundary
are unchanged.
