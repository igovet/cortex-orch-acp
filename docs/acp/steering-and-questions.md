# Steering, urgent stop, questions, and coherent conversation (T020)

This covers the parts of `acp.orchestrating_runtime`, `acp.steering`,
`acp.questions_store`, `acp.messages`, and `acp.commands` this task added:
how new user input reaches an already-active task, how urgent safety stops
differ from ordinary interruption, how coordinator questions reach the user
(structured or plain text), and the language/noise policy for Cortex's own
system lines.

## Five distinct things, never confused

| Action | Effect on the active task | Effect on running assignments |
| --- | --- | --- |
| `session/cancel` | Interrupts *this coordination turn* only; task state becomes `interrupted`, stays active | None -- workers keep running, their events keep accumulating durably |
| An ordinary `session/prompt` on an active task | **Steering**: persisted first (T018 path), then recorded as an additional bookkeeping event carrying a bumped `task_orchestration.requirement_revision`; delivered coalesced with any accumulated worker events at the next controlled boundary | The coordinator decides (via its own `clarify`/`cancel`/`delegate`/`decide` tool calls) which assignments are affected; unaffected ones are untouched |
| `/cortex-stop` | Urgent, everything-cancelling stop (`OrchestratingRuntime.stop_task`); task state becomes `stopped` and never resumes | Every non-terminal assignment is cancelled, every capacity-queued open is aborted (`engine.cancel_task`), every open question is invalidated |
| `/cortex-new-task <text>` | A brand-new, separate task/coordinator; the active task's own state is never read or mutated | N/A (new task) |
| `/cortex-answer <n> <text>` | Resolves one specific, already-disambiguated open question and delivers the answer to the coordinator in a continuation | Whatever the coordinator does in reaction, same as any other decision |

### Why `session/cancel` never cancels workers

ACP allows at most one in-flight `session/prompt` per session; Cortex keeps
the existing `-32602 "A prompt is already in flight for this session."`
rejection (`acp.agent.Agent._session_prompt`) rather than silently queueing
a second one. `session/cancel` therefore only ever needs to unblock *this*
coordinator turn -- it is observed at the top of
`OrchestratingRuntime._run`'s main loop (before the next claim/prompt
cycle, and while the loop is waiting for worker progress in
`_wait_for_progress`), never mid-`engine.prompt()` call (no current engine
adapter supports aborting a call already in flight; see `HostElicitor`'s
`mid_turn_input` note below). Cancelling never calls
`SessionRuntime.cancel_task`, which is reserved for the urgent
`/cortex-stop` path -- calling it here would abort capacity-queued worker
opens too, contradicting "workers keep running."

### Steering delivery boundary

`OrchestratingRuntime._run` records every prompt's text as a durable
`user_message` event *before* any delivery is attempted (unchanged T018
behavior). When the prompt lands on a task whose `task_orchestration` row
already existed (a follow-up prompt, not the task's first), it is
additionally recorded via `acp.steering.record_steering`: this bumps
`task_orchestration.requirement_revision` and appends a `progress`-typed
event with `payload.kind == "user_steering"` and the new revision number.

That extra event is deliberately *informative-only* (see
`delegation.events.MEANINGFUL_EVENT_TYPES`) -- the real `user_message` row
for the same prompt is what actually gates/delivers the continuation, so a
crash between the two never loses steering (the `user_message` alone is
enough to redeliver). Both are claimed and coalesced into the SAME
continuation text as any accumulated worker events (`_coalesce`), matching
"one coalesced continuation at the next controlled boundary."

**Deviation from a literal new `task_events.event_type` value:** the T016
`task_events` table has a `CHECK(event_type IN (...))` constraint. SQLite
cannot widen a `CHECK` constraint on an existing table without a full
table rebuild (copy + rename), which is out of this task's additive,
minimal-migration scope (the established pattern here is `ALTER TABLE ...
ADD COLUMN`/new `CREATE TABLE IF NOT EXISTS`, never a `CHECK` rewrite --
see `delegation.events_store`'s and `store.py`'s `attempt_revisions`
precedent). Steering therefore reuses the existing `progress` event type
with a `kind` payload marker instead of a new `user_steering` event type.
This is recorded durably and is fully queryable (`payload_json LIKE
'%"kind": "user_steering"%'` or, more robustly, parsed JSON), just not a
distinct SQL-enforced type.

### Mid-turn delivery: `mid_turn_input`

No currently qualified engine adapter (`engine.codex_adapter_acp`, or any
future adapter) supports injecting input into an *already-running* prompt
turn. `OrchestratingRuntime.HostElicitor`'s docstring and this document
record that as a capability flag name (`mid_turn_input=False`, the
implicit default for every current adapter) for a future engine that does
qualify -- unused until then, never fabricated as supported now.

### Coordinator decides what's affected

The coordinator (the model) reacts to a steering continuation the same way
it reacts to any other coalesced batch: by calling its own `clarify`,
`cancel`, `delegate`, or `decide` tool. Cortex does not automatically
cancel or reroute any assignment on steering -- see
`tests/test_acp_steering_and_questions.py::test_two_workers_steering_cancels_one_other_finishes`
for a scripted coordinator that cancels one of two running assignments in
reaction to steering (embedding the current `requirement_revision` in its
own `cancel` tool call's `reason` text) while the other continues
untouched. A cancelled assignment's `assignment_cancelled` event, and any
of its results already superseded, carry whatever revision text the
coordinator itself included -- Cortex does not inject it automatically
(the coordinator sees the current revision via the coalesced steering
line and is expected to cite it).

## Urgent stop (`/cortex-stop`)

`OrchestratingRuntime.stop_task` (see `acp.steering.mark_stopped`) is
idempotent and routed entirely through existing cancellation machinery,
never a normal steering queue entry:

- Inserts a row into the independent `task_stopped` table (a second call
  is a no-op; see `acp.steering.mark_stopped`'s docstring for why this is
  a separate additive table rather than a `task_orchestration` column: the
  latter is created lazily, only on the coordinator's first provisioning,
  so a stop issued before that would update zero rows and be silently
  lost -- reproduced and fixed).
- Calls `SessionRuntime.cancel_task` (aborts every capacity-queued open).
- Cancels every non-terminal assignment via the existing
  `_cancel_assignment` path (same one `assignment_cancel_requested`
  events use).
- Marks every still-`open` `pending_questions` row `cancelled` (assignment-
  scoped) or `superseded` (task-scoped).
- Marks every not-yet-delivered (`consumed_by IS NULL`) steering event
  `superseded=1` (`acp.steering.supersede_undelivered_steering`) -- a
  steering message persisted earlier but never delivered is never
  delivered after a stop.

Any later ordinary prompt, steering message, `/cortex-answer`, or
elicitation response on a stopped task is durably recorded (via
`acp.steering.record_post_stop_message`, or `questions_store
.record_late_answer`) but pre-marked inert/superseded -- it can never be
claimed/delivered and never restarts work. `OrchestratingRuntime._run`
replies with a fixed explanation (`acp.messages`' `task_stopped` key)
naming `/cortex-new-task` as the only way to continue. This is a
deliberate choice among the task card's allowed options ("reply explaining
the task is stopped" vs. "require `/cortex-new-task`" are the same thing
here: the reply names the command).

## Questions: structured elicitation with a text fallback

1. The coordinator calls `decide` with `decision="ask_user"`, a `question`,
   and an optional, bounded (`maxItems=8`) `options` list of short answer
   choices (`delegation.contracts.COORDINATOR_OPERATIONS["decide"]`).
2. `OrchestratingRuntime._handle_ask_user` persists a `pending_questions`
   row (`acp.questions_store.create_if_absent`, idempotent per coordinator
   decision id) **before** any host contact.
3. If a `HostElicitor` was injected (see below) it is tried: on an accepted
   answer, the SAME `pending_questions` row is resolved
   (`answer_source="elicitation"`) and a `user_message` event carrying the
   answer is appended so the SAME session/prompt turn continues (no
   `END_TURN`) with the answer coalesced into the next continuation.
4. Otherwise (no elicitor, or it returned `None` for decline/cancel/
   timeout/unsupported) the plain question text is sent as a normal
   `agent_message_chunk`, the task state becomes `waiting_for_user`, and
   the turn ends (`END_TURN`) -- unchanged pre-T020 behavior.
5. The next ordinary prompt, if exactly one question is open, is treated as
   its text answer (same `pending_questions` id, `answer_source="text"`).
   If two or more questions are open, Cortex never guesses: it lists them
   and asks for `/cortex-answer <question-number> <answer text>`
   (`acp.commands.CORTEX_ANSWER`, `acp.commands.parse_answer_argument`).
6. A late answer/elicitation response for an already-answered/superseded/
   cancelled question is recorded (`questions_store.record_late_answer`)
   and otherwise inert.

Worker questions (`ask_question`) still only ever reach the *coordinator*
(`worker_question` events are visible to the coordinator's own
continuation text, never streamed to the host directly) -- unchanged by
this task.

### Client capability detection (implementation status)

`clientCapabilities.elicitation.form` (schema-1.4.0) is the host signal a
real `HostElicitor` adapter must check before ever sending
`elicitation/create`. This task implements and directly tests the
runtime-side contract (`OrchestratingRuntime.HostElicitor` Protocol,
persist-then-try-elicit-then-fallback, per-call injection via
`start_prompt(..., host_elicitor=...)` so one project-scoped
`OrchestratingRuntime` never leaks one connection's elicitor into
another's) against a fake elicitor
(`tests/test_acp_steering_and_questions.py`). **`acp.agent.Agent` does not
yet construct and pass a real `HostRequester`-backed `HostElicitor`** --
every prompt through the production agent currently uses the plain-text
fallback unconditionally, even against a host that advertises form
support. Wiring that adapter (an `elicitation/create` call scoped to the
session, honoring `ElicitationSchema`'s single-field/enum form shape) is a
documented follow-up, not fabricated as done here.

## Language policy for Cortex's own system lines

`acp.messages` is a small, explicitly bounded EN/RU catalog for lines
*Cortex itself* authors (turn-limit/blocked/refused/resume/acceptance-
refused explanations, the stop/new-task/ambiguous-question/answer-fallback
replies). It never renders a worker's raw report text or the coordinator's
own free-form reply -- those pass through verbatim in whatever language the
coordinator itself chose (see the orchestrator role instruction, already
in place before this task: "Use the user's language for conversation and
English for internal handoffs" -- `skills/orchestrator/SKILL.md`; T020
found this equivalent rule already present and left it unchanged rather
than duplicating it).

Selection (`acp.messages.select_language`, `OrchestratingRuntime
._resolve_language`):

1. An explicit per-task override (`task_orchestration.language`, set via
   `acp.steering.set_language_override`) always wins.
2. Otherwise `"auto"`: the script of the most recent `user_message` event's
   text for this task (Cyrillic letters >= 30% of all letters -> `"ru"`,
   else `"en"`). This is a limited heuristic covering only these Cortex
   system lines.

**Deviation from "add to T010 config options as a select":** T010's
`configOptions`/`session/set_config_option` pipeline
(`acp.config_options`) is tightly coupled to model/route selection
specifically (`ConfigurationInputs` carries a route + parameters, and
every write goes through `ConfigurationService`'s revision-gated history).
Wedging an unrelated setting into that exact pipeline risked the
model/thought_level invariants T010 established. This task instead exposes
the override as a per-task column set through direct calls
(`acp.steering.set_language_override`) with `acp.commands` room reserved
for a future explicit `/cortex-language auto|en|ru` command -- not added
in this pass since no caller currently needs it (the `auto` heuristic
already covers the acceptance line: a Russian conversation never sees an
English system line, and an English worker report never leaks into a
Russian conversation -- see
`tests/test_acp_steering_and_questions.py::test_russian_conversation_gets_russian_system_lines_and_no_worker_leak`).

## Noise policy

`OrchestratingRuntime._emit_and_record`/`_emit_localized` de-duplicate an
identical consecutive status line (same task, same message key + params)
via `acp.messages.Dedup` -- an in-memory-only guard (never persisted; a
fresh runtime instance/process starts with a clean slate, which only means
one possible extra repeated line at a process boundary, never a lost one).
The durable `task_events` row is still appended every time the method is
*called* -- only the host-visible chunk is suppressed. Per-poll wait
ticks (`_wait_for_progress`) were never emitted before this task and still
are not.

## UNKNOWNs (owned by T035)

- Whether Zed/codeg render `session/cancel` promptly in their own UI while
  a coordinator turn is mid-`engine.prompt()` (Cortex's own handling is
  covered above and tested against fakes; the host's own UX during that
  window is unverified against a real client).
- Whether either real client currently advertises
  `clientCapabilities.elicitation.form` in practice, and how it renders
  the `ElicitationSchema` form this task's `decide.options` maps to --
  moot today since `acp.agent` does not yet send `elicitation/create` (see
  above), but relevant once that adapter is wired.

## See also

- [Model-owned coordination and managed workers (T016)](orchestration.md)
- [Completion, waiting, and resource lifecycle (T017)](completion-and-lifecycle.md)
- [Durable input history and replay (T018)](input-history-and-replay.md)
- [Recoverable publications and storage consistency (T019)](publications-and-consistency.md)
