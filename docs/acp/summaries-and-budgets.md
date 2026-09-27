# Summaries, weak models, and budgets (T024)

Requirement IDs: S03-03; S09-06; S15-05; S16-01..S16-08; S19-05; S23-01;
S24-03; S25-03. Acceptance matrix: RC-06..RC-10, SEC-08, PR-07, QUAL-04,
QUAL-05.

This task hardens recovery against a trusted-sounding but wrong summary,
and prevents context-overflow/compaction loops without ever dropping a
mandatory user requirement. It never invents a new source of exact state:
the [T021 recovery capsule](recovery-capsules.md) remains the only
derived, provenance-preserving view of authoritative state.

## 1. Summarizer independence and adapter findings

The working model (the route actually executing the assignment) and any
separate summarizer are independent choices (`configuration.resolution`
already lists `"summarizer"` as a recognized dependent-setting name, so an
explicit value round-trips validation, but `configuration/config_options.py`
never surfaces it as a selectable UI option). `recovery.summarizer_policy`
is the single source of truth for why: static dist evidence for both
pinned adapters shows **no separate summarizer configuration path**:

- **codex-acp 1.12.0** — T013's `codex_option_mapping` maps only
  `model`/`reasoning_effort`/`mode` from observed `configOptions`; no
  `summarizer` option is ever emitted by the bundled adapter's
  `createSessionConfigOptions`.
- **claude-agent-acp 0.77.0/0.78.0** — same conclusion; compaction is
  entirely internal to `dist/context-compaction.js`'s
  `ContextCompactionLifecycle`, with no summarizer-selection surface.

`portable.engine_contract.EngineCapability.SUMMARIZER_SELECTION` exists as
a portable concept for a future adapter but is wired to neither pinned
engine today — `summarizer_capability(engine)` always reports
`supported=False` with a cited reason, never fabricated availability.
Compaction/summarization for both engines is therefore **engine-owned**:
Cortex observes that it happened (T022's `context_generations`) but never
configures or trusts *how*.

## 2. Semantic-summary trust boundary

`recovery.summary_validation.validate_summary` is the only path a
model-authored summary can take before anything relies on it. Valid JSON
is not semantic correctness: every candidate is checked, in order, for
structure (a fixed schema), a literal `provenance: "derived_summary"`
label (never a stronger trust label — a summary can never author
permissions, identities, revisions, or successful statuses), a size cap,
that every referenced id resolves against caller-supplied `known_refs`,
and contradiction with a caller-supplied `authoritative` snapshot
(assignment status, ids, check outcomes, and — always fatal, regardless of
`strict` — an omitted or altered constraint id). Fabricated completion
claims and omitted constraints can never change what a caller already
knows from authoritative state.

## 3. Bounded retries and deterministic fallback

`recovery.summarizer_policy.resolve_summary` retries an unavailable/invalid
summarizer up to a bounded attempt count **and** a cumulative estimated
token spend (whichever is hit first), then returns the deterministic,
prose-free capsule fallback (`used_deterministic_fallback=True`). It never
escalates to a different or more expensive route on its own — the caller
decides that, out of band, if it wants to (`escalated` is always `False`).

## 4. Context budgeting

`recovery.token_budget` provides the one shared, conservative token
estimator (`estimate_tokens`, always rounded up, documented as
`"estimated"`) used by both this module and `recovery.capsule`'s
`size_estimate` — never two independently tuned constants. `plan_budget`
accounts for role instructions, tool schemas, chat/continuation history,
the capsule, an expected-tool-output reserve, and a response reserve
together — never only the capsule text — against a route's context window
(`catalog.model.CatalogEntry.context_window_tokens` when a real evidenced
value exists, otherwise the window the engine itself reported as
`usage_update.size` for this session or, earlier, for the same connection
and route model — kept by `connections.context_window` under
`<state_root>/context_windows/<connection_id>.json` — otherwise a labeled
32 000-token default). A real `usage_update` exact figure always supersedes
the chat-history estimate. Four decisions: `"fits"`,
`"needs_reference_mode"` (large *optional* evidence should be an
addressable reference instead of inlined), `"overflow"` against a real
window — always reported with the concrete limit name and never resolved
by truncating a constraint or narrowing scope on Cortex's own authority;
the coordinator is asked to split the assignment or change approach
instead — or `"unverified_window"`. The last one means mandatory content
exceeds only the default because the route has not reported its window
yet. When mandatory content exceeds the default, the runtime blocks delivery
as a preflight guard, retains all content for bounded retry, and reports a
localized `budget_window_unverified` message. This prevents an engine overflow
when the real model window is unknown and the default budget is too small.
Once the engine reports its real window via `usage_update`, that window is
cached and used for subsequent retries.

## 5. Compaction-loop guard

`recovery.compaction_guard.evaluate_compaction` builds on T022's
`context_generations` ledger with a narrower `compaction_recovery_events`
table (new, additive schema group — see `recovery.budget_store`) recording
whether each compaction attempt actually helped. It blocks (never loops)
on any of: too many compactions within a recent window, a fresh
generation's mandatory content already exceeding the usable budget right
after compaction (immediate refill; skipped when the route's real window is
unknown), or several consecutive attempts each
recovering less than a minimum ratio of space. A blocked result is
localized via `acp.messages` (`compaction_blocked`) — the same mechanism
`budget_overflow`/`summarizer_fallback` use for the other two explicit,
never-silent runtime lines this task adds.

## 6. Persisted large outputs

`recovery.large_output_store` persists oversized tool/terminal output
(bytes already captured by e.g. `engine.terminal_bridge`) to one
owner-private file (mode 0600) under the task's storage directory, with a
`large_outputs` metadata row. `read_output` is scoped: a caller must supply
the exact `owner_scope` (`"coordinator"` or a specific `assignment_ref`)
the record was persisted under, or the read is refused as unknown — never
resolved cross-scope. Every chunked read still carries the full record's
`exit_status`, `failure`, and `truncated` flags and total `byte_count`,
never only on the first/last chunk.

## 7. Limited-model presentation

`recovery.presentation.select_presentation` preserves the full role,
brief and acceptance text. The former line-based compact mode could
silently remove an unmarked constraint, so budget overflow must be handled
explicitly by the caller without shortening the assignment on its own.

## 8. Worker brief provisioning budget

Worker provisioning keeps its 16,000-character `brief` validation. The runtime measures the final JSON-encoded brief with its shared serializer, so JSON escaping and Unicode are included exactly as provisioning will count them. The assignment objective, scope and dependency report references are mandatory and remain whole; if they cannot fit, dispatch fails closed with a clear provisioning error.

The original task goal and git review evidence are optional context fitted into the remaining budget. Any shortened value is a structured preview marked `truncated: true`, with original character and UTF-8 byte counts, shown character count and a worker-readable reference. The full goal is available via `read_capsule` under `constraints`; git evidence links to the existing `project_git` `status`/`diff` operation and to the captured `read_large_output` identity when present. Prefix selection is deterministic and slices Unicode code points, so it does not split a UTF-8 character.

## New schema

`recovery.budget_store.LARGE_OUTPUT_SCHEMA` adds `large_outputs` and
`compaction_recovery_events` as a fifteenth, additive optional table group
(no `user_version` bump — still v11), wired into `store.py`'s schema
composition, `cleanup.py`'s per-task deletion order, and `split.py`'s
presence-detection/copy/FK-coverage machinery, in its own module to avoid
conflicting with any other task's concurrent schema addition.

`catalog.model.CatalogEntry` gains an optional, backward-compatible
`context_window_tokens: int | None = None` field (validated positive
integer when present); a pre-T024 cached snapshot with no such key
deserializes to `None`, falling back to `token_budget`'s conservative
default.

## Tests

`tests/test_token_budget.py`, `tests/test_summary_validation.py`,
`tests/test_summarizer_policy.py`, `tests/test_compaction_guard.py`,
`tests/test_large_output_store.py`, `tests/test_presentation.py`, plus
`context_window_tokens` coverage in `tests/test_catalog_model.py` and
`tests/test_catalog_store.py`.
