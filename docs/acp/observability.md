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

`observability.events.list_events(store, task_id, category=..., after=...)`
is the cursor-paginated read.

Worker-panel history uses the existing durable `task_events` stream rather
than a second transcript store. Each redacted `progress` row carrying
`kind=worker_activity` is bound to the task and worker attempt, and the panel
reads it in event-id order with an `after` cursor and bounded page size.
While a worker is active, the host panel refreshes that cursor incrementally
about every 750 ms. Exact lifecycle updates for one logical tool coalesce in
the panel, while worker deltas are held until a tool boundary or turn end and
persisted as complete message blocks. Each block carries a durable segment
identity across all runtime tools, so live and replay views preserve ordering.
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
status/reason, quota state, same-tier balance (including in-flight counts),
policy exclusions, and effort basis. These are decision inputs and outcomes,
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
| `repeat_calls` | `usage_records` (`call_sequence` > 1 per engine session) | count |
| `schema_validation_errors` | `observability_events` category=`validation` | count |
| `manual_interventions` | `pending_questions`; permission dialogs declared `unavailable_reason` (JSONL-only, not indexed by task) | count |
| `duplicate_publications` | `observability_events` category=`publication` | count |
| `tokens` | `usage_records` (`usage_summary`) | tokens, per reported field name |
| `concurrency` | `capacity_reservations` (`state='reserved'`) | count |

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
- Live qualification rows (T035–T037/T038); this task requires no paid
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
`project_delete`/`project_mkdir`/`command_run`/`project_git`, verification,
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
