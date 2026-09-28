# Deterministic recovery capsules (T021)

This page documents `src/cortex_runtime/recovery/capsule.py`
and the `read_capsule` MCP tool (`delegation/mcp_tools.py`,
`delegation/contracts.py`). A capsule is a **derived, rebuildable view**
over already-durable state — never a second authoritative pipeline, and
never a place a new fact is invented. Context-generation delivery uses this capsule on compaction and replacement.
Runtime task state and DAG remain authoritative; compacted model text cannot
change routes, permissions or completion. See [qualification status](runtime-rewrite-status.md).

## Shape

`recovery.capsule.build_capsule(store, task_id, recipient, at_revision=None)`
returns a plain JSON-serializable dict:

```
{
  "capsule_format_version": 1,
  "for_task_id": "...",            # top-level key avoided ("task_id") to
                                    # sidestep mcp_tools' generic technical-
                                    # id leak guard; the exact id is also
                                    # present, nested, at sections.identity.task_id
  "recipient": {"role": "coordinator"} | {"role": "worker", "assignment_ref": ..., "attempt_id": ...},
  "at_revision": null | int,
  "source_high_watermark": {...},  # max task_events id, max source_revisions
                                    # revision, answered_question_count,
                                    # max pipeline edition sequence,
                                    # requirement_revision, active config
                                    # revision, owner_epoch -- every "how
                                    # current is this" fact, never a timestamp
  "capsule_revision": "sha256...", # over the canonical JSON of everything
                                    # above plus every section
  "sections": { ...nine sections, see below... },
  "compaction": {"level": 0..7, "max_level": 7, "critical_budget_bytes": 96000,
                 "uncompacted_critical_bytes": N, "referenced_fields": N,
                 "omitted_entries": N, "note": "..."},  # see "Size bounds"
  "size_estimate": {"bytes": N, "approx_tokens": N//4, "critical_bytes": N, "oversize": false},
}
```

`render_capsule(capsule)` is a pure function of that dict (never touches
the store, never reads wall-clock time) producing a deterministic plain-text
rendering with every recipient-owned/reported text value wrapped in an
escaped `===CORTEX_CAPSULE_DATA_BEGIN(trust=...,ref=...)===` /
`===CORTEX_CAPSULE_DATA_END===` block.

## Determinism, not wall-clock

No field anywhere in the capsule is `datetime.now()` — every "how current"
fact is a durable revision/sequence/id already recorded by an earlier
task (`task_events.id`, `source_revisions.revision`,
`editions.sequence`, `task_orchestration.requirement_revision`,
`configuration_revisions.revision`, `task_owners.epoch`). Two builds
against byte-identical authoritative state — including from a **brand new**
`store.Store` instance after a full process restart — produce a
byte-identical `capsule_revision` and rendered text
(`test_deterministic_rebuild_after_restart_is_byte_identical`). No capsule
row is ever persisted anywhere: there is no cache to invalidate, and
deleting "the capsule" is meaningless — the very next `build_capsule` call
reconstructs the identical thing from the same authoritative rows
(`test_deleting_no_persisted_cache_loses_nothing`).

## Nine sections

Each section is either populated from authoritative rows/report bodies, or
an explicit `{"status": "unavailable"|"unknown", "reason": "..."}` marker
— never silently omitted.

1. **identity** — `task_id`, `project_root`, `recipient`, the resolved
   channel's `channel_id`/`principal_thread_id`/`issued_at`/`revoked`
   (never the secret), `owner_epoch` (current, from `task_owners`) and
   `stale_owner_epoch` (`True` when the channel's own issuance epoch no
   longer matches the task's current owner epoch — a capsule for a
   superseded owner is flagged, never silently served as current).
2. **route_and_config** — the coordinator's active `configuration_revisions`
   row, or (worker) the exact revision recorded at that assignment's own
   `delegation_provisioning.config_revision`, falling back to the
   coordinator's active revision with `source="inherited..."` when no
   explicit per-worker snapshot exists.
3. **constraints** — every durable `source_revisions` row (initial bootstrap
   input plus every accepted steering message) in revision order, each with
   its exact verbatim text read from its published report body (never a
   summary), `origin`/`event_id`, `requirement_revision`, `superseded`,
   `delivered`, `claimed_undelivered`, and its resolved attachment
   descriptors (`source_blocks`: availability/reason/trust_label/digest —
   never inlined bodies). **Worker scoping note**: there is no
   per-assignment "attach this exact constraint to that worker" mechanism
   anywhere in this codebase — runtime assignment `brief`/`acceptance` fields are
   the coordinator's own restatement, not a structured attachment of
   specific `source_revisions` rows. Per lead decision 4, every retained
   `user_direct` constraint is therefore included for **both** coordinator
   and worker recipients, rather than silently narrowing (and possibly
   dropping) a critical constraint a worker actually needs.
   The same section also carries `answered_questions` (with
   `answered_question_count`): every question a real user answer resolved
   (`pending_questions` rows with `status='answered'` and `answer_source`
   `text`/`elicitation`, see `acp.questions_store.answered_questions`), in
   the order asked, each with its exact `question_id`, `source_ref`
   (`<question_id>:answer`), `asked_by` (`runtime`/`coordinator`/`worker`),
   the question text and options as asked, the user's verbatim
   `answer_text` (`trust_label='user_direct'`), `asked_at`/`answered_at`,
   `answer_source`, and the answering ACP session (`session_id`, `origin`
   in the same `acp:<session>` form as captured sources) or
   `session_binding='unavailable'` with a reason when no runtime row
   recorded it. The view is derived on every build: answers recorded
   before it existed appear unchanged, nothing is migrated or synthesized,
   and an answered row is never updated again. An answer is not a source
   revision, so `at_revision` does not bound this list. `render_capsule`
   wraps the question in a `derived_summary` block (the asker wrote it)
   and the answer in a `user_direct` block.
4. **pipeline_and_assignments** — coordinator: the current pipeline
   report/edition ref+digest plus every assignment (title/role/
   brief/acceptance/state, computed from `task_events` + the
   `delegation_provisioning` row). Each assignment also has an
   `observed_worker_route` bound to its exact provisioned attempt and
   `attempt_route_audit` row (engine, connection, model, effort, audit
   identity and decision detail), or an explicit unavailable marker. For
   automatic routes, the detail can include `rank_version`, model-hint status,
   quota state, plan usage, policy exclusions and effort basis; capsules
   expose only retained audit data and do not recompute a route. The coordinator's active
   configuration revision does not establish a mixed-team worker's route;
   worker: only its **own** assignment
   (`own_assignment`), never another assignment's brief.
5. **publications_and_checks** — coordinator: every assignment's retained
   `worker_result`(s) (summary, checks with their claimed/observed/
   contradicted `evidence` label already resolved by
   `delegation.evidence`, report ref+digest) plus every coordinator
   decision; worker: only its own results and decisions.
6. **drafts_and_open_actions** — open `pending_questions`, `ambiguous`
   capacity reservations, claimed-but-undelivered `task_events`
   (`consumed_by` set, `delivered_at` NULL), unpublished drafts,
   non-terminal `delegation_provisioning` rows, and (coordinator only)
   `source_events` rows with `ledger_state IS NULL` — every uncertain
   mutation surfaced, never hidden.
7. **newer_events** — coordinator: unconsumed, non-superseded `task_events`
   (bounded to 50, with a `truncated`/`total` note pointing at
   `list_events` for the rest); worker: every retained clarification/
   cancellation event for its own assignment (documented: no per-worker
   delivery cursor exists in this schema, so this is "everything", not "only
   what's new since last seen").
8. **role_and_tool_access** — `role_id`, its canonical `portable.roles`
   source path + sha256 (role *text* is never persisted or duplicated here
   — see below), `instruction_delivery` qualification from
   `delegation_provisioning`, and `allowed_mcp_tools` matching the complete
   channeled Cortex `tools/list` surface: base report/memory tools plus
   `delegation.contracts.schemas_for(role_kind)`. The capsule lists names;
   the live MCP server advertises the authoritative schemas.
9. **continuation_intent** — `task_stopped`, and (coordinator)
   `task_orchestration.state`/`wait_reason`/`wait_detail`/
   `requirement_revision` plus the last coordinator decision and a
   `task_acceptance` field. The latter is true only when a retained
   runtime task is in COMPLETED state;
   an assignment-level decision or report statement cannot set it.
   (worker) its
   own assignment's cancel-requested/cancelled flags. No model-authored
   "continuation note" feature exists in this codebase; `continuation_note`
   is always an explicit `unavailable` marker rather than implying one was
   consulted — if such a feature is ever added, its value must carry
   `trust_label="derived_summary"` and can never override any other field
   (see "Trust" below).

## Why role instructions are a reference, not stored text

`delegation_provisioning` stores only `context_snapshot_ref` (an opaque
fingerprint hash, `_step_context_snapshot`) and `instruction_delivery`
(a qualification label) — the actual role instruction body is never
persisted anywhere in SQLite. The capsule therefore references the role by
id plus `portable.roles.load_role(role_id).sha256`/`source_path`, which
rebuilds deterministically from the packaged canonical `SKILL.md`, rather
than inventing or duplicating stored instruction text.

## Trust and provenance

Every constraint item carries `trust_label="user_direct"`. Every worker
report field rendered as text (`summary`) and every delegation-authored
field (`brief`, `acceptance`, `rationale`) is wrapped by
`render_capsule` in a `===CORTEX_CAPSULE_DATA_BEGIN(trust=...,ref=...)===`
/ `...END===` block with an explicit trust label
(`worker_report`/`derived_summary`/`user_direct`). A literal occurrence of
the delimiter tokens *inside* wrapped text is defanged
(`_defang`: `===CORTEX_CAPSULE_DATA_BEGIN` → `=xx=CORTEX_CAPSULE_DATA_BEGIN`)
so injected text can never forge a second, fake block boundary
(`test_malicious_summary_cannot_forge_a_delimiter_boundary`). Because
every structural field (ids, revisions, permissions, state, completion) is
computed **directly from durable rows** — never parsed back out of
rendered text — a malicious/inaccurate summary embedded in a worker report
cannot change any of them
(`test_malicious_worker_summary_cannot_change_ids_or_completion`).

## Size bounds

The four **critical** sections (`identity`, `constraints`,
`pipeline_and_assignments`, `drafts_and_open_actions`) are bounded by
construction, so a long task can no longer get stuck on
`capsule_oversize`. A real coordinator session reached that state after
~40 assignments: `pipeline_and_assignments.assignments` inlined every
assignment's full `brief`. That came to about 450 KB of a 515 KB critical
payload, and because the size is deterministic, the bounded delivery
retries could never succeed.

`build_capsule` first measures the verbatim critical sections. If their
canonical JSON is at most `CRITICAL_BUDGET_BYTES` (96 KB), the capsule is
verbatim (compaction level 0). Otherwise it applies the first of the
deterministic, escalating levels in `_COMPACTION_LEVELS` that fits:

| Level | Large text inline allowance (protected / active / other) | Lists |
| --- | --- | --- |
| 0 | verbatim | every entry |
| 1 | 64 KB / 16 KB / 4 KB | every entry |
| 2 | 32 KB / 4 KB / 512 B | every entry |
| 3 | 16 KB / 1 KB / reference only | every entry |
| 4 | 16 KB / reference only / reference only | every entry |
| 5 | 8 KB / reference only / reference only | 50 kept, up to 500 omitted refs named |
| 6 | 8 KB / reference only / reference only | 20 kept, up to 200 omitted refs named |
| 7 | 4 KB / reference only / reference only | 5 kept, first/last omitted ref named |

Priority tiers keep the most important content inline for longest:

- **protected**: the latest user message and the latest unsuperseded
  one, the latest answered question, and the task goal.
- **active**: other unsuperseded user messages, answered questions,
  unfinished assignments, pending/running/failed nodes and open
  questions.
- **other**: superseded messages, and assignments and nodes that are
  finished or cancelled.

A worker recipient's own `own_assignment.title`/`brief`/`acceptance` and
its own `pipeline_and_assignments.runtime.nodes[].objective` (the runtime
filters that list to exactly this worker's own node) are never subject
to any of the tiers above: no worker-available, channel-authorized tool
returns those fields, so replacing them with a `retrieve.tool=null`
reference would strip a recovered worker of the only readable copy of
its mandatory objective. They stay byte-for-byte verbatim at *every*
compaction level, however large, while every other critical-section
field compacts harder in their place to still fit under
`CRITICAL_BUDGET_BYTES`. Only if the worker's own assignment text alone
still exceeds `HARD_CAP_BYTES` after the most aggressive level does
`build_capsule` raise the explicit `capsule_oversize` error described
below -- never a silent reference. (A coordinator's cross-referenced
assignment brief and node objective, by contrast, are resolvable via
`task_get_context`, so they still compact normally.)

Every other string is clamped by a generic per-level allowance (32 KB
down to 512 B). Text of 512 bytes or less always stays inline.

Nothing is dropped silently:

- A text over its allowance becomes a reference record
  `{"status": "referenced", "truncated": true, "trust_label", "sha256",
  "size_bytes", "preview", "preview_bytes", "retrieve"}`. `sha256` and
  `size_bytes` identify the exact full text, and `preview` is its prefix.
  `retrieve` names the tool that returns the full text:
  - a user message: `read_source` with `<report_id>:-1`;
  - an answered question: `read_source` with `<question_id>:answer`;
  - the goal: `read_source`, when it equals a user message;
  - a coordinator's assignment brief, node objective or goal:
    `task_get_context`.

  When no tool is available to the recipient, `retrieve` says so
  explicitly with `tool: null`. A worker's own `own_assignment`
  title/brief/acceptance and its own node's `objective` are never turned
  into such a `tool: null` reference (see above) -- `tool: null` remains
  possible for other fields, such as a worker's own open-question text.
- A trimmed list keeps its highest-priority, newest entries in original
  order. A sibling `<key>_omitted` record (`status: "omitted"`) states the
  omitted `count`, `total`, their `sha256` digest, their refs (or only the
  first/last at the last level) and the listing tool. The latest user
  message is always kept.
- `render_capsule` prints the reference header as plain structure. It
  wraps the preview in the usual data block with the original field's
  trust label, so fences and delimiter defanging still apply.
- `capsule["compaction"]` records the applied level and counts. The level
  and every reference are part of the `capsule_revision` digest. They
  are a pure function of durable state (no wall clock), so identical
  state gives an identical revision and rendered text, including after a
  restart.

The last level is bounded by a constant independent of task size.
Measured results:

- The real task above lands on level 2 at about 94 KB.
- A synthetic production-shaped task (40 long briefs, about 760 KB
  verbatim) lands on level 4 with every entry listed.
- Synthetic tasks of 2 MB and 14 MB verbatim land on level 6 at
  60–77 KB. `HARD_CAP_BYTES`
(400 KB) stays only as a last-resort guard: `capsule_oversize` is raised
only if even the most aggressive level exceeds it, which cannot happen
with the shipped levels. Non-critical sections (`publications_and_checks`,
`newer_events`) already store only bounded refs and digests (never raw
report bodies). `newer_events` and coordinator decisions are additionally
capped at 50 entries, with an explicit `truncated`/`total` note pointing
at the paginated `list_events`/`list_sources` tools for the rest.

## `read_capsule` MCP tool

A shared, read-only operation (`delegation/contracts.py`'s
`CAPSULE_OPERATIONS`, merged into both `COORDINATOR_OPERATIONS` and
`WORKER_OPERATIONS`) with one optional `at_revision` field — no task id,
recipient selector, role, or assignment id is ever accepted as an
argument. The recipient is always derived from the channel's own already-
resolved identity (`ChannelContext.task_id`/`.role`/`.assignment_id`/
`.attempt_id` — see `trusted_channel.resolve_channel`), so a channel can
never read another task's or another role's/assignment's capsule; this is
a structural refusal (the schema has no field to abuse), not a runtime
check (`test_read_capsule_schema_accepts_no_task_or_recipient_selector`).
A worker channel whose own `delegation_provisioning` row does not exist
yet (assignment id not yet recorded) gets an explicit
`capsule_unavailable` error rather than a wrong/empty capsule.

## What this task does not do (deferred)

- Using a capsule to actually resume/deliver a coordinator or worker turn
  (full engine-context delivery) is T022 scope.
- `at_revision` only bounds the `constraints` section to a point in time;
  every other section always reflects current durable state (a full
  point-in-time snapshot across every section is not implemented — this
  is a documented, not silent, limitation).
- No capsule row is ever persisted/cached (by design — see "Determinism"
  above), so there is nothing to invalidate on writes.
