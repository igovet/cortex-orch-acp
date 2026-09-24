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
choice leads to) as its chat message. The question itself is one short line
with short answer options. A question asked while reconciling is how the
coordinator hands the user a decision after verified work, such as approving a
proposed fix plan. Before this, the coordinator wrote such a plan into its
synthesis, and the runtime reported the task as finished (`Завершено`) while
the plan still waited for approval. The question keeps the task open in
`WAITING_USER`. The answer resumes planning rather than reconciling: the
coordinator submits a plan for the chosen work, which becomes a new revision
with its own review and verification. If the user chose to finish, it closes
the task with a short `answer_submit`. The coordinator uses `synthesis_submit`
only when nothing is left for the user to decide.

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

An ordinary prompt answers the single open question. Multiple open questions
require the existing explicit `/cortex-answer` selection. A cancelled or
superseded question cannot be revived by a late reply. Answered questions are
included in the structured task context for the next coordinator turn.

`/cortex-stop` persists cancellation before process cleanup. Cancellation is
idempotent, fences late runtime results and prevents completion. Engine process
termination and ownership checks remain necessary; a cancellation event by itself
does not prove that an external process is gone.

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

The ACP bridge in Paseo 0.9.1 and 0.9.2 (the same code) has no native
`prompt.steer`. The Cortex
provider sends a text steer through the bridge's concurrent configuration
channel as `cortex_queued_steer`, waits for durable capture, then acknowledges
the steer for the still-active turn. If capture races with the end of that
turn, the provider still acknowledges the steer for that turn, and the agent
starts a background continuation using the same source identity so the queued
message is still processed once. Paseo accepts a steer only as a `steer` result
for the turn it expects. Any other result makes it cancel that turn
(`session/cancel`) and resend the text as a new prompt. Cortex cannot tell that
cancel from a Stop, so the task pauses and its running workers are cancelled.
So when the provider cannot queue a steer, it fails the send instead of
returning a result. Unsupported non-text content and a failed config
acknowledgment both do this. The chat shows a "Steering was not queued"
warning with the reason, Paseo reports the send as failed, and nothing reaches
Cortex. The current turn and its workers keep running, and the user can send
the text again. An explicit Stop
(`session/cancel`) or `/cortex-stop` remains an interruption: the former
moves the task to RECOVERING and fences unfinished work; the latter
permanently cancels it. Worker questions enter the persisted question protocol;
scope, replan and escalation requests retain their three-revision bound. Each enters
`RECOVERING`; the coordinator ends its turn and waits for the runtime to stop workers
and request the replacement plan before submitting it. See
[qualification status](runtime-rewrite-status.md).

The coordinator uses the language selected for the submitted message, including
when reports and context are English. Workers receive an English communication
rule in their shared generated protocol and in every ordinary, recovery and
fallback turn. Exact quotes and explicitly requested project-artifact languages
are preserved. This is model instruction, not automatic translation or a claim
that arbitrary model output can be deterministically language-validated.
