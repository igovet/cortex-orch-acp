# Context generations and qualified recovery delivery

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed.


Builds on [recovery capsules](../recovery-capsules/index.md): this page tracks
*when* a fresh context generation begins for a live engine session, and
distinguishes intent from actually-observed delivery of the capsule/role/
tool-schema content Cortex sends into it.

## Compaction-support tiers

One tier per (engine, version), from static adapter dist evidence only
(`delegation.generations.tier_for`; see that module's docstring for exact
`file:line` citations):

- **automatic** — the adapter emits a verifiable compaction signal over
  ACP. The exact registrations qualify **codex-acp** 1.12.0 and 2.1.1 and
  **claude-agent-acp** 0.77.0, 0.78.0, 0.87.0 and 0.88.0; other versions of
  a known engine follow the wildcard convention of `delegation.generations`
  (`tier_for`), and an unknown version falls to `controlled_boundary`.
  The adapters report their own internal context compaction as a
  `tool_call`/`tool_call_update` pair carrying `_meta.contextCompaction` (the 0.87.0
  registration marks it with `_meta.claudeCode.toolName` set to `compact` instead)
  (`{"version": 1}`), title `"Compact conversation"` — the *legacy*
  presentation both adapters fall back to because Cortex does not
  advertise the `session.compaction` client capability (the
  `ResourcePolicy` default). claude-agent-acp 0.78.0 additionally supports
  the standardized `compaction_update`/`compaction_summary_chunk` session
  updates if that capability *is* advertised — not exercised here.
  **This is never a pre-compaction hook**: the signal arrives as
  compaction starts/finishes, never before it. Cortex reacts to it (opens
  a fresh generation, redelivers on the next continuation) but can never
  intervene beforehand or claim it stopped a hidden internal engine action.
- **controlled_boundary** — no compaction/reset signal is evidenced for
  this adapter+version. Cortex only refreshes context at boundaries it
  itself controls (a new session open, or a session it itself replaces).
  A known engine name at an unrecognized version falls here rather than
  assuming automatic support without exact/wildcard evidence.
- **unqualified** — a completely unknown engine: no compaction-support
  claim at all.

Exactly one compaction *owner* is recorded per engine session
(`compaction_owners`, primary-keyed by `engine_session_ref`, written once
via `INSERT OR IGNORE`): `engine_adapter` for automatic tier,
`cortex_controlled` for controlled-boundary, `none` for unqualified. Cortex
never runs a second, competing compaction controller for the same session.

## Context generations

`context_generations` (see `delegation.generations`) records one row per
distinct epoch for an `engine_session_ref`, 1-based `sequence`, and a closed
`cause` set: `session_open`, `observed_compaction`, `observed_reset`
(reserved — no pinned adapter has been shown to emit a *reset* distinct
from compaction), `session_replacement`, and `controlled_boundary_refresh`
(reserved — codex-acp exposes a client-invokable `/compact` slash command
over the ordinary `session/prompt` text channel, advertised via
`available_commands_update`, but codex-acp is already automatic tier via
its own signal, so this cause is not currently reached by either pinned
adapter — never fabricated as "supported" without a real code path using
it).

Duplicate compaction triggers for the same observed event are deduplicated
by `(engine_session_ref, evidence_id)` (`compaction_events_seen`, primary
keyed) — a re-delivered `tool_call_update` for the same `toolCallId` never
opens a second generation; only the first sighting of a terminal
(`"completed"`) phase for a given id does.

## Delivery ledger

`capsule_deliveries` (see `delegation.deliveries`), keyed by
`(recipient, context_generation_id, capsule_revision)`: a second
compaction with unchanged task state opens a new generation, so the
identical logical snapshot gets a fresh, distinct delivery key —
redelivery is permitted; the same key retried is idempotent.

States: `built` → `transmitted` (the `session/prompt` call carrying the
delivery was issued) → `delivery_confirmed` (that exact call's JSON-RPC
response arrived — proof the engine's transport received/processed the
request, never proof the model "understood" or "remembers" anything) →
`continuation_observed` (a *subsequent* `session/prompt` call was
initiated on the same session — evidence of continuation, not of use).
`blocked`/`superseded` are reachable from any non-terminal state:

- **Receipt timeout** — the turn that should have confirmed delivery
  instead failed transport-side (`EngineTurnFailed`): the row stays
  `transmitted` with `uncertainty` recorded, never promoted on a guess.
- **Stale/replaced session** — releasing/replacing a session (see
  `EngineSessionManagerAdapter.terminate_owned`) marks every one of its
  not-yet-`continuation_observed` deliveries `superseded`; a superseded
  delivery can never later report `delivery_confirmed`.
- **Immediate overflow** — `capsule_oversize` or the engine hitting
  its own token/turn-request limit right after the delivery turn both mark
  the delivery `blocked` with a named reason; `record_built` bounds retries
  (`MAX_DELIVERY_RETRIES = 3`) on the next ordinary continuation — never an
  automatic loop within one turn. The capsule's critical sections are
  bounded by deterministic compaction (see
  [recovery capsules](../recovery-capsules/index.md#size-bounds)), so
  `capsule_oversize` is a last-resort guard that the shipped compaction
  levels cannot reach.

A model's own turn text is never consulted by any of the above — every
transition takes an explicit `evidence_source` naming the technical channel
that justifies it.

## Fresh-generation delivery content

Before the first controlled continuation for a fresh generation (including
a session's very first turn — `session_open` is itself a generation),
`acp.orchestrating_runtime._begin_generation_delivery` prefixes that turn's
prompt with, in order: current role instructions (via the delivery
mechanism — `qualify_delivery`/`delivery_for`, respecting its existing
qualification/refusal policy) and the recovery capsule (`render_capsule`,
whose `role_and_tool_access` section names the currently allowed MCP tool
operations — the model's access to their *current* schemas is the already-
connected Cortex MCP server's own `tools/list`, never embedded verbatim in
the prompt). This closes a gap in which instruction
delivery was qualified but never actually wired into a real prompt).

The inline copy of a worker's assignment is deduplicated. For a worker's first or
continuation prompt, `recovery.capsule.build_capsule(..., inline_delivery=True)` keeps
the objective exactly once: `own_assignment.brief` stays verbatim, while the capsule's
`own_assignment.title` and the worker's own `runtime.nodes[].objective` become
`status: "referenced"` pointers to it. The literal assignment brief that the prompt carries
after the capsule is replaced by a short pointer back to the capsule above it. A capsule
read later through `read_capsule` is not built with this flag and keeps every field
verbatim, so a recovering worker still has the full objective.

## What's deferred

- Advertising `session.compaction` to get claude-agent-acp's richer
  `compaction_update` presentation (with an explicit terminal status and
  summary) — not implemented; the legacy tool-call presentation is
  sufficient for tier/generation tracking and is what both pinned adapters
  actually emit today.
- Using codex-acp's `/compact` slash command as a genuine
  `controlled_boundary_refresh` mechanism for a *different*, non-automatic
  adapter that might expose an equivalent command — no such adapter is
  currently evidenced.
- Live qualification of any of the above against a real engine is not covered by the automated suite.
