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

## Streaming message identity

Each coordinator prompt turn gets one assistant `messageId`. Every streamed
`agent_message_chunk` for that turn carries the same id, so clients can append
the chunks into one visible message. When a new tool call starts, Cortex rotates
the id before sending later prose; a later continuation turn also receives a
fresh id. This prevents Paseo from merging text across tool boundaries or
separate turns. The id is stored with the host transcript and is sent again by
`session/load`; older rows without an id receive a deterministic replay id for
that replay entry, without claiming that the old stream had a historical id.

## Attachments and availability

`protocol.py` now advertises `promptCapabilities.embeddedContext=true`
(the capability set now includes `image: true`; `audio` stays `false`,
still rejected `-32602`) and accepts an `EmbeddedResource` (`resource`)
content block alongside `text`/`resource_link`. `acp.replay_store.
resolve_prompt_blocks` classifies every non-text block truthfully, never
claiming retention that did not happen:

| Block | Condition | `availability` |
| --- | --- | --- |
| `resource` (embedded `text`/`blob`) | ≤ 10 MiB, prompt total ≤ 10 MiB | `stored` |
| `resource` | over either cap | `too_large` (metadata only, body discarded) |
| `resource_link` (`file://`, inside project root, owned regular file, no symlink, single hardlink) | ≤ 10 MiB, prompt total ≤ 10 MiB | `stored` (a snapshot copy, sha256-verified) |
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

`image` blocks are accepted, decoded with strict base64 validation, stored as
owner-private bytes with their exact MIME type, and forwarded unchanged in
the engine's ACP `session/prompt` content array. `audio` remains rejected
with `-32602` because it is unadvertised.

Images travel inline as base64, so the whole `session/prompt` line must fit
the transport's 15 MiB inbound frame limit (`acp.transport.MAX_FRAME_BYTES`):
room for the full 10 MiB attachment budget once encoded, plus the prompt text,
and below the Paseo relay's 16 MiB per-frame ceiling. A larger line is drained
without being buffered or echoed and answered with `-32700` ("attach fewer or
smaller images"). The reply carries the request's own `id` when the frame's
first 64 KiB show it as a top-level integer or string member, so the client
ends that turn instead of waiting forever; otherwise the id is `null`.

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

### Answered questions

Every `list_sources` page also returns `answered_questions` (with
`answered_questions_total` and `answered_questions_complete`, bounded to
200 entries): each question of this task that a real user answer resolved
-- by elicitation card, a typed reply, or `/cortex-answer` -- with its
`question_id`, `source_ref` (`<question_id>:answer`), `asked_by`, question
text and options, the user's verbatim answer (`trust_label='user_direct'`),
`answer_source`, `asked_at`/`answered_at` and the answering session
(`session_id`, `origin` as `acp:<session>`, `session_binding`). Question and
answer texts over 1000 characters are previewed there with a
`*_truncated` flag. `read_source(<question_id>:answer)` returns the whole
record, with `chunk`/`offset`/`next_offset` paging over the answer text.

The view is derived from `pending_questions` (`acp.questions_store.
answered_questions`): only the runtime marks a question answered, on a real
host answer, and never updates it afterwards, so no model tool can create
or change such a record; an open, superseded or cancelled question and a
late answer are never listed. Since this view was added every runtime answer
path records the answering `session_id` on its `question_answered` progress
event. An older elicitation or `/cortex-answer` answer is bound through its
session-origin `user_message` event. An older typed answer never recorded its
session and is listed with `session_binding='unavailable'`.
The answer text is stored and shown as the user typed it, exactly like
captured prompt text; no redaction pass applies to ACP-captured user input.

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

The Cortex Paseo provider also preserves this boundary when reopening a
persisted native session. Paseo's ACP bridge otherwise reapplies its cached
composer model, mode, effort and settings after `session/load`, before
`session.opened`. A formerly selectable model can now be denied or absent,
so this extra `session/set_config_option` used to fail an otherwise valid
history load with "Couldn't refresh agent history." The replay connector
omits those cached selection fields only for a persisted-session open;
the configuration returned by Cortex remains authoritative. Request-local
cwd, environment and MCP configuration are still forwarded. New-session
selection and explicit later `session.configure` requests remain unchanged
and invalid choices still fail normally. No route is enabled, no policy
denial is bypassed, and no history is reconstructed.

During that same read-only replay, Cortex also checks any worker card that
is still `in_progress` against its durable process identity.  If `/proc`
proves that exact worker process is absent, Cortex sends one ephemeral
`tool_call_update(status=failed)` for the existing card with the reason
`Worker process exited before reporting a result.` This is a display
correction only: it does not append a task event, mark the process registry,
open a replacement engine session, or make a model call. A live or
indeterminate identity is left untouched. The next `session/prompt` remains
the recovery boundary that can reconcile the old coordinator and delegate a
fresh worker; `session/load` never starts paid work. When the worker was
lost with its host -- a durable `host_disconnected` record (see
[failures and disconnects](failures-and-cancellation.md#4-disconnect)) lists
the attempt, its owning Cortex process is provably gone with no terminal
event, or recovery wrote it off as `recovery_worker_lost` (including the
replan cancellation after that) -- the same correction instead reads the
localized `Interrupted when the host connection closed; no result yet. The
next message reconciles this worker from saved state.` ACP has no
interrupted status, so the card status is still `failed`; the title gains the
suffix ` · Interrupted` (after any earlier status suffix is removed) and the
update carries `_meta.cortexWorkerInterrupted: {"reason": "host_disconnected"}`.
The installed Paseo bridge drops `_meta`, so the Paseo card reads the title
suffix, which the runtime appends to the whole title
(`Worker: role · route — name · Interrupted`).

After the replayed history, while this session's latest `host_disconnected`
record has no later `user_message`, `session/load` appends one localized
`agent_message_chunk` (message id `cortex-connection:<event id>`, with
`_meta.cortexConnectionNotice`). It reports the saved task state, states that
a completed result is unchanged, and counts interrupted workers and actions
with an unknown outcome. Paseo's own `Provider connection closed` error is
host-owned; this notice is the persisted outcome shown beside it. It is sent
before the load response, so the Paseo shim includes it in the checked replay
manifest. Building it only reads the store.

`host_transcript` and the companion `host_updates` table (the sole read
sources for replay) are written by
`acp.agent`: once per accepted prompt (the exact captured user text plus
attachment descriptors) and once per runtime-produced
`agent_message_chunk` offered to the host. Slash-command prompts and their
text replies follow the same display-history path. Structured updates such as
`tool_call`, `tool_call_update`, and display-only plan/thought updates are
stored as the exact update payload before enqueueing the notification and
replayed as `session/update` notifications in the same sequence, except
that each card's run of `tool_call_update`s is replayed as one update that
carries the run's final value for every field. The merged update sits at the
run's last position, and a new `tool_call` for the same id starts a new run.
ACP updates replace the fields they carry, so the rendered card is the same.
A worker card resends its whole cumulative log on every update, so replaying
every row grew quadratically. One long worker run produced 17.7 MB, more than
the Paseo history buffer below, and that chat could not reopen. The stored
rows are not changed. This
preserves a real runtime event even if the transport writer fails after
enqueueing, without claiming that the host rendered it. These records are
never read by, or fed into,
`OrchestratingRuntime`/engine context -- display history and engine context
recovery are deliberately different data (engine context recovery is
T021/T022 scope). Paseo may apply its presentation filters while replaying:
hidden Cortex MCP and private `.cortex` operations remain durable records even
when they are not rendered in the coordinator or worker feed.

The native Paseo provider also accounts for a Paseo 0.8 load ordering quirk:
the SDK can emit historical `timeline.item` events before `session.opened`.
The provider buffers those items separately per session, forwards
`session.opened` first, and then forwards the buffered items in their original
order. The buffer is bounded at 4,096 items and approximately 16 MiB of JSON;
an overflow fails closed and is reported as an unsupported/incomplete client
replay rather than silently dropping or partially accepting history. A
`session.closed` or `session.runtime_failed` event clears only that session's
buffer.

Paseo 0.8 has a second ordering defect inside its bundled ACP bridge: it can
discard `session/update` notifications received before the `session/load`
response because it assigns the native session id too late. The installed
Cortex provider therefore launches the repository-owned
`integrations/paseo/server/acp-load-shim.mjs` stdio relay. It buffers only
`session/update` frames whose `params.sessionId` exactly matches the active
`session/load` request, forwards the exact response first, and flushes the
frames on the next event-loop turn. The relay is bounded at the same 4,096
items and approximately 16 MiB, rejects malformed or truncated frames, and
terminates the child on failure. The relay exits as soon as the child exits.
Paseo keeps the relay's stdin open, so a relay that only stopped its child
used to leave a pending `session/load` unanswered forever. Paseo showed that
as "Couldn't refresh agent history". This changes only the client-facing ordering
needed by Paseo 0.8; Cortex remains ACP-compliant toward its own engine
client. It is a compatibility path for Paseo 0.8, not a general ACP ordering
rule.

### Failed tool items in Paseo

Paseo 0.8 requires every failed timeline tool item to carry a non-null error
display value, while ACP permits a failed tool update whose `rawOutput` is
omitted. The repository-owned native `cortex-acp` Paseo provider applies a
presentation-boundary transformer for this case: when the observed status is
`failed` and the output is null, it retains `failed` and supplies the bounded
message `Tool call failed: <title>`. The transformer does not invent an
underlying cause, change the runtime result, or turn a failure into success.
This is a client rendering compatibility path, not a replay reconstruction
or a runtime fallback attempt.

The Paseo 0.8 production observation retained in the [real-client
record](evidence/paseo-2026-09-20/live-acp-regressions.md) predates this
transformer and reproduced the failed-history problem. Offline coverage and
source inspection do not qualify the client behavior; the post-fix live agent
run is not established by automated fixtures;
result.

There is deliberately no reconstruction fallback for sessions created before
`host_updates` was introduced, or for a run whose structured update was not
recorded. Task events, assignment rows, and tool-result observations prove
that work happened in the runtime, but they do not prove which ACP update was
delivered to this client, in what order, or whether the client rendered it.
Synthesizing a tool card from those records during `session/load` would turn
runtime evidence into a false host-delivery claim. Such a session replays its
durable text transcript only; exact structured updates are available from the
point at which this recording path is active.

Replay applies the live per-task coordinator tool-card budget (see
[bounded ACP event projections](observability.md#bounded-acp-event-projections)).
Updates of the first 200 distinct coordinator cards in a session's journal
replay as recorded. Later coordinator cards are counted onto one aggregate card
appended after the history. That card is a count of recorded cards, not a
synthesized runtime event: its `_meta.cortexReplay` has an empty `sequences`
list and `foldedCards`. Journals written under the budget never exceed it and
replay unchanged; the durable rows are never rewritten.

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
