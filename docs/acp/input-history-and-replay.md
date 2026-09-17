# Durable input history and replay (T018)

Extends [orchestration.md](orchestration.md) (T016 event log/process
boundary) and [completion-and-lifecycle.md](completion-and-lifecycle.md)
(T017 delivery ledger/crash recovery) with complete source/attachment
preservation, provenance-safe replay, and bounded reads. Covers
`acp/replay_store.py` (the tenth optional schema group:
`source_blocks`/`source_block_reads`/`host_transcript`), the `acp/agent.py`
capture path, `acp/orchestrating_runtime.py`'s delivery-identity/crash
recovery, and `session/load`.

## Event identity and deduplication

`source_events (origin, event_id)` (unchanged since T005's
`bootstrap.py`) is the identity for durable capture. `acp.agent.Agent`
mints a fresh `event_id` (a UUID) per *accepted* `session/prompt` -- after
its own JSON-RPC duplicate-request-id guard rejects a same-connection
retry with `-32600` before any durable capture is attempted. Two distinct
`session/prompt` calls carrying identical text are therefore two distinct
source events, both retained and both delivered -- text equality is never
used for dedup. A raw `PromptRequest._meta` value is never used for identity
either. It is recorded only for forensics in `source_events.meta_json`
(JSON, capped at 16 KiB; an oversize or unencodable value is replaced by a
`{"truncated": true, "original_size_bytes": N}` marker; absent `_meta` stores
NULL).

**What cannot be deduplicated**: a client reconnect (a new ACP
`session/new`, even against the same `cwd`) creates a new `origin`
(`acp:<new session_id>`), so a message resent only because the client
reconnected is *not* recognized as a retry of the prior connection's
message -- it becomes a new source event on the same durable task once
`session/prompt` binds it (see `bootstrap.append_source_event`). Reconnect
continuity (recognizing "this is the same logical conversation resuming")
is explicitly out of this task's scope -- see T023.

## Delivery ledger

Every source event flows through four durable states:

1. **persisted** -- `source_events` row committed (`bootstrap.bootstrap_task`/
   `append_source_event`, one `BEGIN IMMEDIATE` transaction with the
   published report body).
2. **recorded** -- a `task_events` `user_message` row exists with
   `dedup_key='user_message:<origin>:<event_id>'` (T018 changed this from a
   recomputed session-turn count to the source event's own identity --
   see `acp.orchestrating_runtime._run`). Two source events with identical
   text get two distinct `dedup_key`s and are both recorded/delivered; a
   replay of the identical `(origin, event_id)` is a no-op insert
   (`delegation.events.append_event`'s existing idempotency).
3. **claimed** -- `task_events.consumed_by` set by
   `delegation.events.claim_unconsumed` for one coordinator continuation.
4. **delivered** -- `task_events.delivered_at` set by
   `delegation.events.mark_delivered` once the engine's `prompt()` call for
   that continuation actually returns (T017).

### Crash recovery: persisted but never recorded

If the process crashes after step 1 (source event durably captured) but
before step 2 (recorded as a `task_events` row) -- for example a crash
inside `acp.agent._run_prompt_body` after `bootstrap_task`/
`append_source_event` returns but before/during the `runtime.start_prompt`
call -- the next `OrchestratingRuntime._run` for that task (a fresh prompt,
or the same connection retried with a *new* event id) calls
`_recover_unrecorded_source_events` first: it scans `source_events` for the
task for rows whose `ledger_state` is still NULL (the marker is set to
`recorded` in the same transaction that inserts the `user_message` event;
rows that existed before T018 are backfilled as `legacy_recorded` and are
never auto-recovered), and
records each in `captured_at` order with `payload["recovered"]=true`,
*before* the new prompt's own message. This never creates a second task or
assignment (`bootstrap_task`/`append_source_event` are never called from
here). Recovery itself is idempotent -- calling it again after the rows
already exist is a no-op (each `append_event` call is dedup-keyed).

If the process crashes after step 3 (claimed) but before step 4 (delivered),
T017's existing `reconcile_undelivered_claims` (`_ensure_coordinator`,
first use of a task by a fresh runtime instance) releases the claim
(`consumed_by` → NULL) so the next continuation reclaims and delivers it
exactly once -- unchanged by T018, documented here because the identity
change above is what the released row is keyed by.

Crash windows are tested directly (new runtime/agent instance over the same
store) in `tests/test_acp_input_history_and_replay.py`; this task never
promises exactly-once *external* (engine-side) execution, only a
reconcilable durable ledger.

## Attachments and availability

`protocol.py` now advertises `promptCapabilities.embeddedContext=true`
(the only capability flip in this task -- `image`/`audio` stay `false`,
still rejected `-32602`) and accepts an `EmbeddedResource` (`resource`)
content block alongside `text`/`resource_link`. `acp.replay_store.
resolve_prompt_blocks` classifies every non-text block truthfully, never
claiming retention that did not happen:

| Block | Condition | `availability` |
| --- | --- | --- |
| `resource` (embedded `text`/`blob`) | ≤ 1 MiB, prompt total ≤ 4 MiB | `stored` |
| `resource` | over either cap | `too_large` (metadata only, body discarded) |
| `resource_link` (`file://`, inside project root, owned regular file, no symlink, single hardlink) | ≤ 1 MiB, prompt total ≤ 4 MiB | `stored` (a snapshot copy, sha256-verified) |
| `resource_link` (`file://`, outside project root) | -- | `reference_only` (uri kept, never read) |
| `resource_link` (any other scheme, e.g. `https://`) | -- | `reference_only` (never fetched) |
| `resource_link` (symlink / non-owned / multiple hardlinks / unreadable) | -- | `inaccessible` (with a concrete `reason`) |

A `stored` body is written to an owner-only (`0600`) file under the task's
existing storage directory (`<task_dir>/.source_blocks/`), never inline in
SQLite -- the same Markdown/binary-body-vs-SQLite-metadata split the rest
of the store already uses. `source_blocks` holds only metadata (`kind`,
`mime`, `name`, `uri`, `sha256`, `size_bytes`, `availability`, `reason`,
`trust_label`, `body_ref`, `fetched_at`). Attachment capture is atomic with
the source-text capture: `acp.agent._run_prompt_body` resolves blocks and
stages their bodies under `.source_blocks_staging/` before any transaction
(no database lock held during file I/O), then `bootstrap_task`/
`append_source_event` rename the staged bodies and insert the
`source_blocks` rows inside the same `BEGIN IMMEDIATE` transaction as the
`source_events` row. A crash before commit leaves no rows (and at most an
unreferenced staged file, swept after one hour when a session opens); a
block whose resolution or staging fails is still recorded, as
`inaccessible` with reason `capture_error`, never dropped. `resource_link`
file snapshots are read through one `O_NOFOLLOW` descriptor whose real path,
type, owner, link count and size are checked on the descriptor itself.

`image`/`audio` blocks remain rejected with `-32602` (unadvertised).

## Trust labels

Vocabulary: `user_direct`, `file_content`, `worker_report`,
`derived_summary` (`replay_store.TRUST_LABELS`). Every block captured from
an inbound ACP prompt (the main text and every non-text block) is labeled
`user_direct` -- there is no other origin for prompt content in this task's
scope. The other three labels are reserved vocabulary for later
integration points (worker-published results, derived summaries, file
reads outside a prompt) not exercised by this task; `read_source` always
echoes back the block's own capture-time `trust_label`.

## Bounded reads

`acp.replay_store.read_source(store, task_id, source_ref, offset, limit)`
returns `{source_ref, origin, event_id, block_index, kind, trust_label,
availability, reason, sha256, total_size, offset, chunk, encoding,
next_offset}`. `limit` is clamped to 64 KiB server-side; `chunk` is UTF-8
text, or base64 (`encoding='base64'`) for a non-UTF-8 stored body.
`source_ref` is the opaque string `<report_id>:<block_index>`
(`block_index=-1` is the main captured text; `list_sources` is the only
place a caller learns a `source_ref`).

A stored body file that has vanished since capture, or a `resource_link`
whose target no longer resolves, is reported as `expired`/`inaccessible`
**at read time** via an *appended* row in `source_block_reads` -- the
original `source_blocks` capture-time row is never rewritten, so "what was
actually captured at prompt time" stays distinguishable from "what a later
read observed".

Exposed as MCP tools `list_sources`/`read_source` on **both** coordinator
and worker channels (`delegation.contracts.SOURCE_OPERATIONS`,
`delegation.mcp_tools._shared_list_sources`/`_shared_read_source`),
task-scoped by the caller's own `channel.task_id` -- a `source_ref` for a
different task simply resolves to `unknown_source_ref`, never leaking
whether it exists elsewhere. A coordinator continuation prompt
(`acp.orchestrating_runtime._coalesce`) still only ever gives the
coordinator the exact stored user text plus attachment descriptors
(name/kind/availability/`source_ref`) -- never an inlined large body.

## `session/load`: display-only replay

`protocolVersion` 1's `loadSession` capability is now `true` and
`session/load` is implemented -- **display-only**: `acp.agent.Agent.
_session_load` validates the request, resolves the durable
`acp_host_sessions` row for `sessionId` (erroring `-32602` for an unknown
or wrong-project session id), and streams the session's `host_transcript`
(every `user_message`/`agent_message` this agent has ever captured/emitted
for that session, in order) as `session/update` `user_message_chunk`/
`agent_message_chunk` notifications, then replies. It performs **zero**
`BEGIN IMMEDIATE` transactions and never calls `runtime.start_prompt`,
opens an engine session, or writes `source_events`/`task_events`/
`deliveries`/`reports` -- asserted directly in
`tests/test_acp_input_history_and_replay.py` via before/after row-count
equality and a runtime call-count assertion. A replayed
`agent_message_chunk` is never captured as a new source event (there is no
capture path anywhere near `_session_load`).

`host_transcript` (the sole read source for replay) is written by
`acp.agent`: once per accepted prompt (the exact captured user text plus
attachment descriptors) and once per successfully delivered
`agent_message_chunk` emitted to the host. It is never read by, or fed
into, `OrchestratingRuntime`/engine context -- display history and engine
context recovery are deliberately different data (engine context recovery
is T021/T022 scope).

`session/resume` stays unsupported (`protocol.UNSUPPORTED_METHODS`): its
semantics (resuming a *live* engine turn, not display history) are a
different, larger surface this task does not implement.

## Deferred (explicitly out of scope here)

- **T020**: steering injection into a *running* coordinator turn (this task
  only durably captures/replays completed prompts).
- **T021/T022**: engine context recovery (as opposed to display replay) --
  `session/load`'s transcript is never engine input.
- **T023**: reconnect continuity (recognizing a new ACP session as "the same
  logical conversation" instead of a new `origin`).
