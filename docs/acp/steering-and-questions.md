# User input, questions and cancellation

The runtime retains the original goal separately from coordinator messages and
worker reports. User sources continue to be captured with exact source/session
identity before they are delivered. Replayed user text does not become a new
source event merely because it is read again.

The coordinator requests a user question through the advertised
`user_question_request` schema. The runtime atomically records its delivery
receipt, pending question, resume phase and `WAITING_USER` transition before
contacting the ACP client. Supported clients receive the existing elicitation
form; other clients receive the question and options as ordinary chat text.
A declined, cancelled or timed-out elicitation leaves the question open.
Only an answer recorded against that exact question resumes the requesting
phase. Exact retries preserve the original question identity; changed deliveries
with the same key fail.

The coordinator writes the details (the proposed plan, the evidence, what each
choice leads to) once, as its chat message before the question call. The
question itself is one short line with short answer options. The receipt tells
the model to end the turn without writing anything more, and the runtime
withholds coordinator text received after the committed question in that turn,
even if the question's first lifecycle callback is delayed. This conservative
boundary can omit pre-question text delivered late by the adapter
(see [orchestration](orchestration.md#plan-submission-and-coordinator-polling)).
A prompt-only rule against a recap did not hold: in one observed session the
model rewrote the full details after the question, and both copies reached
the chat. A question asked while reconciling is how the
coordinator hands the user a decision after verified work, such as approving a
proposed fix plan. Before this, the coordinator wrote such a plan into its
synthesis, and the runtime reported the task as finished (`Завершено`) while
the plan still waited for approval. The question keeps the task open in
`WAITING_USER`. The answer resumes planning rather than reconciling: the
coordinator submits a plan for the chosen work, which becomes a new revision
with its own review and verification. If the user chose to finish, it closes
the task with a short `answer_submit` only when the answer is an ordinary
planning choice. An explicit owner choice to close the task as-is takes the
runtime-owned force-finish path below instead. The coordinator uses
`synthesis_submit` only when nothing is left for the user to decide.

A question can carry a `skill_proposal` for a reusable project skill. The
runtime validates action-matched approval and decline labels before persistence,
shows its own evidence list, and derives an approve, decline or unclear decision
from the exact answer. Persisted unsafe labels fail closed after restart; see
[project skill management](skill-management.md#proposed-skills-from-repeated-work).

In Paseo the question is a card with its options and an "Other..." field for
an answer of the user's own. Paseo's stock ACP bridge can answer a card only
with one of its options, so the Cortex provider
(`integrations/paseo/server/provider.ts`) sends the typed text to Cortex as the
`cortex_question_answer` config value, with the question id. Paseo's bridge
forwards any config id. Cortex accepts that id only when it runs under the
Paseo provider, and only for an existing question of the session's own task.
The text answers the question exactly as a message typed into the chat would,
and is kept in the chat history. Once Cortex has accepted it, the provider
retires the card as declined; Cortex ignores that late decline. If Cortex
refuses the text, the card stays open.
For a free-text question with no supplied options, Cortex sends one instructional
card action so Paseo still renders the question and its "Other..." field. That
action is not an answer: selecting it leaves the durable question open. The user
must type an answer under "Other..." or reply in chat.

An ordinary prompt answers the single open question. Multiple open questions
require the existing explicit `/cortex-answer` selection. A cancelled or
superseded question cannot be revived by a late reply. Answered questions are
included in the structured task context for the next coordinator turn.

`/cortex-stop` persists cancellation before process cleanup. Cancellation is
idempotent, fences late runtime results and prevents completion. Engine process
termination and ownership checks remain necessary; a cancellation event by itself
does not prove that an external process is gone.

`/cortex-finish` (with no argument) is the explicit owner command to close the
current task **without** claiming unfinished checks passed. It captures the
command as a real user source before the runtime's stop marker, uses the same
task-owned worker/coordinator termination and question/steering fencing as
`/cortex-stop`, and records `USER_FORCE_FINISHED`. The terminal state is
`CANCELLED`, not verified `COMPLETED`. Paseo's queued-message channel handles an
exact `/cortex-finish` during an active turn immediately after durable capture,
instead of waiting for the current worker and review to finish. A replay of
the same queued message identity cannot create a second closure event.

When a saved question about closing the **task** receives an exact owner answer
such as `Закрыть как есть`, `Завершить задачу`, `close as is`, or a bare
`Завершить`/`Finish` to a question that explicitly asks to finish the task,
the runtime force-finishes from that recorded answer. This includes a Paseo
question-card choice or typed answer and works after process recovery. Similar
but non-closure requests (for example, asking for one more check or a release
build) continue through ordinary planning. The match is deliberately narrow;
use `/cortex-finish` if wording is uncertain. Model text alone cannot trigger
this transition.

User steering is durably queued by message identity and requirement revision. It
does not cancel the coordinator turn, change an executing plan, revoke worker
channels, or classify a worker interruption as an engine failure. The controller
finishes the current workers, independent review and verification before applying
queued steering at a planning or reconciliation boundary. Application and the
transition back to planning commit together. A queued `deny_write` restriction
takes effect at that boundary; workers already running under their original
grants may finish. The coordinator receives the retained steering and completed
worker evidence in its next planning turn. The execution scheduler leaves
queued `user_message` events unclaimed for that coordinator turn rather than
consuming their text as a worker-card update. Repeated delivery of the same
message identity does not create another requirement revision.

A message sent to a task that already reached `COMPLETED` reopens the task
(`TASK_REOPENED`) and returns it to planning. If the message only asks about
the finished work, the coordinator answers with `answer_submit` and the task
completes again. If it asks for changes, the coordinator submits a plan, and
that new revision gets its own review and verification. See
[orchestration](orchestration.md) for the exact eligibility rule.

The installed Paseo 0.9.2 ACP bridge has no native `prompt.steer` (observed in the live implementation checks). The Cortex provider sends a message (text, and any inline
image or uploaded-file content) through the bridge's concurrent configuration
channel as `cortex_queued_message` -- an ordinary queue item, not steering --
waits for durable capture, then acknowledges Paseo's own `steer` result for
the still-active turn (Paseo's own ACP contract, unrelated to Cortex's
terminology for the item). If capture races with the end of that turn, the
provider still acknowledges it for that turn, and the agent starts a
background continuation using the same source identity so the queued message
is still processed once, in order, after the turn (and any worker it started)
finishes. A session loaded before coordinator recovery retains its old ownership
epoch until the replacement opens; a queued message may use the replacement
epoch only while this process has an active turn for that exact host session
and a matching coordinator channel. A different owner or an ended turn is
refused rather than adopting the database's newer epoch. An image or uploaded
file is resolved and staged exactly as it would be for an ordinary new-turn
`session/prompt` (capped at 10 MiB per block, 10 MiB per message); an oversized
or unreadable image is refused before
capture, same as a first prompt. Paseo accepts this only as a `steer` result
for the turn it expects. Any other result makes it cancel that turn
(`session/cancel`) and resend the message as a new prompt. Cortex cannot tell
that cancel from a Stop, so the task pauses and its running workers are
cancelled. So when the provider cannot queue a message, it fails the send
instead of returning a result. An attachment kind this queue path does not
carry (a forge/review/GitHub attachment, for instance) and a failed config
acknowledgment both do this. Paseo 0.9.2 displays a rejected send as a host
error. It cannot suppress that host error while preserving the running turn,
and a provider notice would display the same error a second time. The provider
therefore emits no notice on refusal; the single host error says that nothing
was sent and, when available, includes Cortex's reason and retry guidance.
Nothing reaches Cortex. The current turn and its workers keep running, and the
user can send the message again. This failure is not displayed in the styled
plugin notice block because Paseo does not expose a safe single-channel outcome
for it. An explicit Stop
(`session/cancel`) or `/cortex-stop` remains an interruption: the former
moves the task to RECOVERING and fences unfinished work; the latter
permanently cancels it. Worker questions enter the persisted question protocol;
scope, replan and escalation requests share a bound of three accepted requests since
the user last spoke; a follow-up message or a question answer restores it. Each enters
`RECOVERING`; the coordinator ends its turn and waits for the runtime to stop workers
and request the replacement plan before submitting it. See
[qualification status](runtime-rewrite-status.md).

The coordinator uses the language selected for the submitted message, including
when reports and context are English. Workers receive an English communication
rule in their shared generated protocol and in every ordinary, recovery and
fallback turn. Exact quotes and explicitly requested project-artifact languages
are preserved. This is model instruction, not automatic translation or a claim
that arbitrary model output can be deterministically language-validated.
