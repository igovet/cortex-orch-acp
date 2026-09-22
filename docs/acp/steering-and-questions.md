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

An ordinary prompt answers the single open question. Multiple open questions
require the existing explicit `/cortex-answer` selection. A cancelled or
superseded question cannot be revived by a late reply. Answered questions are
included in the structured task context for the next coordinator turn.

`/cortex-stop` persists cancellation before process cleanup. Cancellation is
idempotent, fences late runtime results and prevents completion. Engine process
termination and ownership checks remain necessary; a cancellation event by itself
does not prove that an external process is gone.

User steering fences active worker channels and terminates owned sessions before
replanning. The original goal, additions, denials and prior evidence remain durable.
Worker questions enter the same persisted question protocol; execution resumes
with a new plan after the exact answer. Scope requests and replans are bounded to
three accepted automatic revisions. A blocked task can resume on real user input.
`session/cancel` pauses into RECOVERING and revokes active work; `/cortex-stop`
permanently cancels the task. See [qualification status](runtime-rewrite-status.md).

The coordinator uses the language selected for the submitted message, including
when reports and context are English. Workers receive an English communication
rule in their shared generated protocol and in every ordinary, recovery and
fallback turn. Exact quotes and explicitly requested project-artifact languages
are preserved. This is model instruction, not automatic translation or a claim
that arbitrary model output can be deterministically language-validated.
