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
