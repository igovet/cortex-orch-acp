# User input, questions and cancellation

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed. The former Python runtime was removed on 2026-11-16: references to its modules and tests were dropped, and statements that cited them describe behavior carried from that implementation that has not been re-verified against the Rust runtime.


The runtime retains the original goal separately from coordinator messages and
worker reports. User sources continue to be captured with exact source/session
identity before they are delivered. Replayed user text does not become a new
source event merely because it is read again.

`/cortex-clear` is runtime-owned retention, not a question answer or steering
message. It is refused while a prompt is in flight, including a background
continuation or open question waiter. Retry in an idle session; the current
bound task stays protected. See [retention maintenance](../../project/packaging.md#retention-maintenance).

The coordinator requests a user question through the advertised
`user_question_request` schema. The runtime atomically records its delivery
receipt, pending question, resume phase and `WAITING_USER` transition before
contacting the ACP client. Supported clients receive the existing elicitation
form; other clients receive the question and options as ordinary chat text.
A declined, cancelled or timed-out elicitation leaves the question open.
Only an answer recorded against that exact question resumes the requesting
phase. A completed worker-local question (or complete batch) also restores the
three-replan budget with a durable `WORKER_USER_ANSWER_RECEIVED` marker in the
same transaction. Duplicate deliveries, partial batches and runtime access
decisions do not reset it. This marker never acknowledges a removed failed
verification command; that requires an answer to a task-global question asked
after the removal, or an exact complete passing verification rerun. Exact retries preserve the original question identity; changed deliveries
with the same key fail.

After three planning prompts without a structured proposal, the runtime asks a
recovery question with `structured_proposal_missing`. The second and third
planning or reconciling prompts begin with a repair note: the previous turn's
message text was not recorded, and this turn must call the named structured
`cortex` tool. When `answer_submit` is offered, the prompt also says that even a
one-word answer reaches the user only through it, and that a user request not to
use tools, delegate or create files never forbids that call. A recorded answer resumes
`RECOVERING`; after owned-work termination is confirmed, recovery returns to
`PLANNING` with a fresh three-prompt cycle. This also applies when elicitation
answers synchronously within the same host turn. Clearing the wrong phase's
counter previously caused immediate reblocking without any new engine prompt.
Dismissal, absent answers and unconfirmed worker termination do not grant a new
planning turn. The limit is unchanged, and a renewed cycle without a proposal
still blocks; successful answer mapping alone is not successful orchestration.
The no-paid regression is in the Rust lifecycle suites; it
covers option/custom recovery answers, actual plan continuation, exactly-once
answer consumption and the unchanged no-proposal/decline bound. Earlier live
evidence must be read in two parts: the original scalar answer was followed by
three successful engine turns but no accepted structured proposal; the later
recovery answers were blocked by the exhausted-counter defect and did not start
a new engine turn. The repair addresses the latter controller defect only.
A successful answer mapping is not proof of a plan, and the historical Desktop
scenario remains partial until a new requested live run reaches its remaining
acceptance cases.

The coordinator writes the details (the proposed plan, the evidence, what each
choice leads to) once, as its chat message before the question call. The
question itself is one short line with short answer options. The receipt tells
the model to end the turn without writing anything more, and the runtime
withholds coordinator text received after the committed question in that turn,
even if the question's first lifecycle callback is delayed. This conservative
boundary can omit pre-question text delivered late by the adapter
(see [orchestration](../orchestration/index.md#plan-submission-and-coordinator-polling)).
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
[project skill management](../skill-management/index.md#proposed-skills-from-repeated-work).

The question contract is role-neutral: structured questions emitted from coordinator, worker, and runtime fallback paths use the same provider handling. The runtime request carries the question text and any answer options; a supported client can render them as a structured form. Paseo uses its installed `QuestionFormCard` with choice labels and an "Other..." field. Related questions can be carried in one batch and displayed in one **paginated** form; the user navigates among members and must answer every member before one all-member submission. This does not mean every field is simultaneously visible. Tabs use the supplied short topic (or bounded question-text fallback), never raw ids. Duplicate headers gain ` 2`, ` 3`, etc., within 24 characters. The provider uses that same unique header list to map Paseo's header-keyed submission back to exact question identities; complete legacy id-keyed maps remain accepted, but mixed key spaces are rejected.

Paseo's provider binds a response to the exact question/session (and, for a batch, each member identity) and sends it to Cortex through the `cortex_question_answer` session-config route. It waits for Cortex's durable acknowledgement before echoing answers or retiring the native card; it retires with deny, never grants an option action. Partial, foreign, conflicting, or rejected answers do not complete the question. Retries preserve the delivery key and payload. On rejection the form remains recoverable; after a successful answer, replay does not add duplicate answer history. A chat reply is also an answer through the normal runtime route.

After that acknowledgement, the live chat echo is one user bubble per answered question, reading `Question: <question text> → Answer: <answer>`. A batch member whose question carried a topic gets the topic as a prefix (`<topic>: Question: … → Answer: …`). A single question, a free-text "Other..." answer and each member of a batch all get their own bubble, so the timeline shows what was asked next to what was chosen. The echo is presentation only: the answer Cortex records and the durable replay event are unchanged. The durable replay event is worded differently, `Answered question ('<question>'): <answer>` (or `Answered question ('<question>') via elicitation: <answer>`), so a reloaded chat and a live chat show the same pair in different phrasing. Unit tests cover the echo text in `integrations/paseo/server/provider-question.test.ts` and `provider-question-batch.test.ts`. No rendered Paseo GUI check of the bubble has been recorded.

The "Other..." field permits a custom answer for each question. Paseo trims custom text at its UI boundary; the provider does not reinterpret ordinal-looking custom text as an option number. Dismissing/cancelling a form does not invent an answer, and the open question can be presented again during recovery. For legacy scalar cards lacking an exact question identity, the provider retains the native option path. A free-text legacy question may use an instructional card action to expose "Other..."; selecting that action is not an answer and leaves the durable wait open.

A task-global coordinator question still needs an exact answer to its pending question; multiple independently open questions require explicit `/cortex-answer` selection. Text fallback prefixes each question with its current open-list number and separates blocks with a blank line. Both display and command lookup use creation order, immutable batch-member order for tied timestamps, then id as a final tie-breaker. Answering removes a member from the open list, so remaining numbers change; use the latest displayed list. Worker assignment-local questions resume the matching assignment/session when its bound channel permits; a recovered channel without retained session metadata uses task-global recovery instead, without promising same-session continuation. Cancelled or superseded questions cannot be revived by late replies. Other ACP hosts may lack structured forms and receive question/options as chat text: neither the runtime nor plugin can force every model to ask a question or every host to render a UI form. See [Paseo structured-question tests](../../project/verification.md#paseo-plugin-integrationspaseo).

`/cortex-stop` persists cancellation before process cleanup. Cancellation is
idempotent, fences late runtime results and prevents completion. Engine process
termination and ownership checks remain necessary; a cancellation event by itself
does not prove that an external process is gone.

### Stop one worker command with a reason

`/cortex-kill <command-ref> <reason>` requests stopping one exact running
worker command without cancelling the assignment or current turn. In the Paseo
CLI, send the same slash command as a user chat message with `paseo send` to the
bound chat. The required reason is delivered to that worker; it continues its assignment with that
constraint and must not repeat the same command unchanged. The command ref is
the gateway command identity shown in the Paseo worker panel or returned by the
local listing command. Cortex records the request durably; acknowledgement means
requested, not that the process has already exited. A race with natural command
exit is reported as missed/already finished, not as a successful stop.

In Paseo Desktop, open the **Workers** panel, select the worker, then use
**Running commands → Stop** on the command. Enter a reason and confirm; the
control remains in the worker panel, not on a native chat tool card. The panel
shows sending, requested, refused or unconfirmed states and the terminal command
row displays **Stopped by user** with its reason. Rendered Desktop behavior has
not been live-qualified for this change.

For a local owner CLI, inspect or stop commands without launching a model:

The former owner CLI (`cortex_commands`) was removed. In an ACP chat
send `/cortex-kill <command-ref> <reason>`; for a read-only listing use the
`cortex-paseo-tools running-commands <project-root> <task-id> <assignment-ref>`
reader (an empty assignment ref lists every assignment). The list includes only live or unresolved brokered worker commands and a
scrubbed, bounded argument summary. A stop request must identify one command and
include non-whitespace reason text (up to 2000 Unicode characters). A refusal
is reported with its code. These controls cover
Cortex gateway commands, not native Codex `apply_patch` or OpenCode `execute`
actions. The Paseo UI and the ACP command have automated coverage, but live
Paseo Desktop/CLI qualification was not run.

`/cortex-finish` (with no argument) is the explicit owner command to close the
current task **without** claiming unfinished checks passed. It captures the
command as a real user source before the runtime's stop marker, uses the same
task-owned worker/coordinator termination and question/steering fencing as
`/cortex-stop`, and records `USER_FORCE_FINISHED`. The terminal state is
`CANCELLED`, not verified `COMPLETED`. Paseo's queued-message channel handles an
exact `/cortex-finish` during an active turn immediately after durable capture,
instead of waiting for the current worker and review to finish. A replay of
the same queued message identity cannot create a second closure event.

Force-finish keywords (`Stop`, `Закрыть как есть`, `close as is`, `finish now` and
the like) apply **only** to the runtime's own stop question, recognised by its structured
marker. On a coordinator or worker question they are ordinary answer text: an option labelled
`Stop` (for example "Stop the daemon first or keep it?") answers that question and never
cancels the task. Use `/cortex-finish` to close a task at any time. Model text alone cannot
trigger this transition.

The runtime's own stop question (asked when the runtime stops a task on a blocker) is different. It is asked in the task's language (the task's `ru`/`en` setting; for `auto`, the language of the task's own text) and names the reason in plain words (for example "a worker session could not start" / "не удалось запустить сессию воркера"); an unrecognised reason is named as an internal stop with its code in parentheses. Below that line it says, from recorded facts, what happened (the step by role and goal, what it needed and what prevented it: the reviewer's verdict, words and report; the failing verification commands with their exit codes; the denied files and the rule; the task holding the files), then what each of its two answers does and which one is recommended when one clearly is (`crates/cortex-core/src/lifecycle/stop_notice.rs`). The answers keep their labels. The question's decision record also carries each answer's consequence (`detail`, `action`, `recommended`), its one-line `headline` and `targets` the desktop opens: the blocking task, the review report ("Open review findings") and the denied path. A question the runtime replaces with a later one names it (`replacedBy`), and the Conversation reads it as "Replaced by the question below: <reason>". A driver error that arrives while the task already waits on such a question never replaces it. The runtime recognises it by a structured marker on the question, not by its wording, so a
coordinator question with the same text never force-finishes the task. Its Finish option (by number,
stored or rendered English or Russian text, or label) ends the task `CANCELLED` with
`USER_FORCE_FINISHED`. Continue resumes the runtime flow and never releases the stop. Only an answer
in the user's own words releases the question. See
[the decisions log](../../project/decisions.md#runtime-stop-question-finish-and-continue).

A stop for a work blocker that is not a completion gap (a lost worker whose retries gave up, a failed
check) asks this question at once; only a completion gap first returns to the coordinator. Earlier
builds left such a stop waiting in recovery with no question; the next start asks it, once, when
nothing followed the stop. A worker's refused `result_submit` returns its refusal code and what to
correct to the worker (for content refusals such as `open_failed_check` or `omitted_failed_check`),
and every refusal is retained as a `WORKER_RESULT_REFUSED` event carrying only the code and the
attempt binding, so the worker's card can say why it has no result.

On the native desktop host the stopped driver has already exited, so a committed Continue relaunches
the coordinator from the retained state, and a service restart resumes a Continue whose relaunch it
interrupted. If the relaunch fails again, the task asks a new stop question with that reason instead
of waiting in recovery. A native recovery with a recorded reason, or whose owned processes all
settled, is shown to the owner as waiting for them, with Stop and Close task available. A
recovery without that evidence stays unconfirmed.

The question text itself (the runtime's `stop_question`/`stop_reason.<code>` catalog
entries) never shows a bare internal code as the whole reason. Each recognised stop
code (worker failure, verification failure, review limits, an interrupted result
delivery the runtime could not reconcile against the worker's actual publication,
a lost worker whose recovery could not finish cleanly, and so on) renders as a short
plain-language sentence in English or Russian; the raw code still follows it in
brackets (for example `... [result_publication_mismatch]`) so it stays available for
diagnosis next to the readable reason, never instead of it.

An owner message to a working task (planning, a plan not yet started, execution,
review, verification or reconciliation, with no question of the coordinator open
for the owner) is recorded once by its request id (`USER_STEERING`,
`"delivery":"boundary"`) and interrupts nothing: running workers, their waits and
channels, review, verification and the coordinator's current turn continue, and
the task keeps its state. A worker's own open question stays open and answerable.
The runtime applies the message at the next safe boundary of the coordinator's own
work (`STEERING_APPLIED`, committed together with the return to planning) and
hands it to the next coordinator turn (`STEERING_HANDED`):

- sent during a planning turn: when that turn ends; a plan it submitted that has
  not started is discarded and the coordinator plans again with the message;
- sent while workers, review or verification run: when the revision reaches
  reconciliation after its verification passed, or earlier when the runtime
  returns to the coordinator anyway (failed verification, a failed or stopped
  worker, a replan). The native host runs one coordinator turn at a time and
  never beside running workers, so a message does not start a coordinator turn
  while they run;
- several messages: in the order sent, in one turn.

That turn runs on the task's coordinator route; its prompt lists the messages
with their attachment names, and the engine prompt carries the attached files.
The message is delivered (`STEERING_DELIVERED`) once the engine's reply to that
turn's prompt is observed (`NATIVE_TURN_SETTLED` of the coordinator channel). If
the service dies, or the turn ends, before that reply, the next coordinator turn is
handed the message again, marked `(repeated)` in its prompt. The guarantee is: every
message reaches at least one coordinator prompt the engine answered, and it is
recorded delivered exactly once; it can reach the engine twice only when the engine
received a prompt whose reply the service never saw. The coordinator changes
running work only through its own channel-verified tools (a new plan or replan
request), never because the runtime cancelled it. A result never completes past a
message that is pending or not yet delivered (`completion_pending_input`). Stopping
work stays an explicit owner action (Stop, worker Stop, command stop).

Messages the owner queues in the desktop (Queue) are sent by the Workspace service,
window open or not: when the task reaches such a boundary (planning, a plan not yet
started, reconciliation or a direct answer, nothing open for the owner), the service
takes the task's queued messages in queue order (each becomes `sending`), records
each as a boundary message under its own id and settles it in the queue. Until it
is taken a queued message can be edited, moved or removed; once `sending` it
refuses `message_taken`. A take interrupted by a restart is finished by the next
one (the message id makes it idempotent). Send now on a queued row records it at
once for the earliest boundary.

A message to a task whose coordinator waits for the owner (its question is open),
is held in recovery, is paused (ACP resumes it) or has finished (it reopens) keeps
the restart delivery (`"delivery":"restart"`): it is applied at admission, the
task's unfinished work is fenced and the relaunched coordinator is handed the
message in the same way.

An explicit `deny-write: <path>` in a message is recorded with it and takes
effect at once for writes not yet made, whatever the delivery, and
`allow-write: <path>` lifts it: see the deny-write rules below.

The conversation shows each owner message's state from these facts: taken from
the queue, received and waiting for the coordinator's next step, or delivered to
the coordinator with its time. The facts are read with the task's product facts,
which its runtime events already trigger; nothing polls for them.

A message that arrives while workers, review or verification run is therefore
applied only after the runtime verification passed: it returns a reviewed and
verified revision to planning before completion (`RECONCILING → RECOVERING →
PLANNING`) and clears any synthesis not yet completed. `synthesis_submit` stays
refused while planning; its `synthesis_not_expected` refusal now carries the
current state and the choices valid there. The planning prompt tells the
coordinator that the work was verified and offers two outcomes: a plan for
requested changes, which becomes a new revision with its own review and
verification, or `answer_submit` with the final summary. That answer must disclose
unresolved worker checks with concrete blockers, using the same assessment and
"Not verified" rendering as synthesis; omitting a gap refuses the proposal without
changing the task state. A gap closed by exact verified evidence needs no disclosure.
Both final-answer tools put gap disclosures only in structured `not_verified`.
With that list nonempty, an authored not-verified heading (including common
English/Russian aliases) refuses the proposal before persistence, rather than
silently removing context. Correct the text and retry while keeping the complete
structured list; see [disclosure ownership](../task-lifecycle/index.md#runtime-composed-synthesis).
The runtime completes
such an answer only after re-checking the revision's completion gates (completed
nodes, unchanged report digests, approved review, passing verification on the
unchanged project); a project changed since verification ends the task `FAILED`
with `answer_not_certified:completion_project_revision_changed`. A later failed
verification, an accepted plan, assignment, replan or verification repair, or a
newly provisioned worker removes the answer option. Previously such a message,
even one that needed no decision, left the coordinator only refused synthesis
attempts until `structured_proposal_missing` asked the user. The runtime still
does not judge a message harmless: the coordinator decides, and a model can still
choose wrongly. The no-paid controller regressions are in the Rust lifecycle suites.

When the queued message carries an image or uploaded-file attachment (its
bytes/base64 never forwarded to the already-running turn), the retained
steering carries the same `source_ref`/`name`/`uri`/`kind`/`availability`
descriptor the runtime already records for durable capture. The coordinator turn
that receives the message names the attachments next to its text, and its engine
prompt carries the stored files as content blocks. A text-only message has no
`attachments` key.
If capture commits before steering is recorded, an identical retry rebuilds the
descriptors from that task's exact durable source revision, including the original
availability/reason, before recording the missing steering event. Already-recorded
steering remains idempotent: one event and one requirement revision.

A message sent to a task that already reached `COMPLETED` reopens the task
(`TASK_REOPENED`) and returns it to planning. If the message only asks about
the finished work, the coordinator answers with `answer_submit` and the task
completes again. If it asks for changes, the coordinator submits a plan, and
that new revision gets its own review and verification. See
[orchestration](../orchestration/index.md) for the exact eligibility rule.

The previously inspected Paseo 0.9.2 ACP bridge had no native `prompt.steer` (observed in the live implementation checks). The Cortex provider sends a message (text, and any inline
image or uploaded-file content) through the bridge's concurrent configuration
channel as `cortex_queued_message` -- durably captured pending steering, not a
Paseo composer draft --
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
(`session/cancel`) and resend the message as a new prompt. A `session/cancel`
followed by a `session/prompt` on the same session is treated as steering, not
Stop: the cancelled coordinator turn ends, running workers continue, and the
prompt is applied as steering. A lone `session/cancel` pauses the task only if no
such prompt follows within a 5-second grace window (`DEFAULT_HOST_CANCEL_GRACE_SECONDS`).
This pairing rule applies to Paseo sessions only; other clients' cancel pauses at
once. So when the provider cannot queue a message, it fails the send
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
(a lone `session/cancel` that is not followed by a prompt, after the grace window) or
`/cortex-stop` remains an interruption: the former moves the task to RECOVERING and
fences unfinished work; the latter permanently cancels it. A worker that steering
leaves running keeps a running card (see
[worker cards across steering](#worker-cards-across-steering)). Worker questions are assignment-local persisted waits.
The worker ends its tool turn without an incomplete result; after a real answer,
the runtime resumes that same assignment and attempt with the next prompt in the
same healthy engine session. Sibling work can continue. The worker is not
reprovisioned and the coordinator does not need to submit a replacement plan for
this ordinary question flow. Multiple open worker questions require explicit
selection; decline, timeout or a text-only host response leaves the wait open.
Stop, cancellation, replan or loss of the owning session can cancel or orphan a
wait; process-loss recovery may replace the worker rather than continue its old
session. These worker waits are distinct from the coordinator's task-global
`WAITING_USER` question described above. If a recovered worker channel has no
retained engine-session binding, ordinary tools still use their normal authority
checks and its question uses task-global `WAITING_USER` recovery instead.
The exact answer resumes the saved execution/review phase, without a guarantee
of continuing the original session. Access-decision continuation still requires
a retained session binding.

Workers may also request additional exact project paths needed for their
assignment. Safe requests are granted only after runtime validation against role,
owner deny-write policy, path safety and current workspace writer reservations. An owner
deny-write rule is created only by an explicit `deny-write: <path>` instruction in an owner
message (`deny-write`, any ASCII case, optional spaces around `:`; the path bare or in
single, double or backtick quotes; trailing `.`, `,` and `;` dropped); wildcard or mask
characters create no rule, and natural-language phrasing such as "do not modify ..."
creates no denial. The path is stored in normalized form (`./a//b/` is `a/b`); a path
outside the project or a private path (`.git`, `.cortex`, ...) refuses the whole message
with `gateway_path_denied` and nothing is recorded. The rule is recorded with the message
and applies at once to every write not yet made: file tools refuse it (`write_denied`),
a writing command already running is stopped (`authority_changed`) and a new one starts
with an existing denied path read-only, a plan scope touching it is refused (`write_denied`), and a
scope or access request for it is refused (`scope_conflicts_with_user_policy`,
`deny_write_policy`). The coordinator's capsule lists the rules
(`owner_write_denials`, the set in force). The owner lifts a rule with an explicit
`allow-write: <path>` in a message (same forms, normalization and refusals as
`deny-write:`; one message that both denies and allows a path refuses with
`write_rule_conflict`) or with Remove on the task's Write denials list in Work details
(`allow_write` control; a path not denied refuses with `write_denial_unknown`). A lift
removes only the owner's own denial of exactly that normalized path (a file inside a
denied folder is not that denial) and grants nothing else; natural language lifts
nothing. A lifted path is writable again for later writes, plans and requests; a
writing command keeps running when a denial is lifted. Rules and lifts are durable
events, applied in their recorded order, and another task does not inherit them;
a multi-path request is granted all-or-none. A contested request gets bounded
runtime retries, then a grant or denial through the same healthy-session
continuation, without a user access question. Policy, path and role refusals
are immediate runtime decisions; retained old access cards are retired. A request itself performs no write and
user text does not override policy. After denial the worker can use an allowed
alternative or request a replacement decomposition through the existing
scope/replan flow. This does not dynamically widen the operating-system jail:
ordinary writers already have whole-project write authority in the current
launch boundary, while the runtime's declared reservations coordinate writers.
See [orchestration](../orchestration/index.md#worker-questions-and-additional-access).

Language follows the reader. Everything another worker or the coordinator reads
is English: worker reports (headings included), summaries, briefs, worker
messages and the coordinator's instructions to workers. Everything the owner reads
directly is in the owner's language: the coordinator's messages and questions to
the owner, the final result text and its not-verified list, a worker's question to
the owner (question text and option labels) and the runtime's own stop questions.
The owner's language is the task's `en`/`ru` setting, otherwise the language of the
goal (`domain::task_language_tx`); it is recorded in `task_language` at task start
and refreshed at every turn. Every turn's route instructions state it per role
(`Owner-selected response language: <lang>.` with the coordinator's owner-facing
scope, or with the worker's `user_question_language: <lang>.` and its English
rule), and every worker brief carries `user_question_language`. The coordinator
composes the owner-facing result from English reports in the owner's language. The
same question strings reach a form or text fallback verbatim. Only genuine user
decisions belong in that question tool; missing project access goes to runtime
`access_request` for a writing worker, a read-only role reports it, and unavailable
evidence is reported as a limitation. Exact quotes and explicitly requested
project-artifact languages are preserved. This is model instruction, not automatic translation or a claim
that arbitrary model output can be deterministically language-validated.

## Stopping from a question and acknowledging steering

Every coordinator or worker question with options also offers the runtime's own
"Stop the task" ("Остановить задачу"). Choosing it, or answering any question with a
plain stop ("стоп", "stop", "остановить задачу", "stop the task"), stops the task through
the same path as `/cortex-stop` (`force_finish`), keeping finished work and never
certifying unfinished work. The runtime stop question keeps its own Finish option.
Two plain "stop" answers once failed to stop a task because only the coordinator
interpreted them (2026-10-22).

A message that arrives while workers, review or verification run is recorded at once
and waits for the next safe point; the desktop shows it as "Received, waiting for the
coordinator's next step", then "Delivered to the coordinator" with the time. It used to wait 13-35 minutes with no sign it was received.

A long runtime verification command updates its tool card once a minute with its
position and elapsed time (`Verify project · 2/4 · 12 min`); a full Storybook run was
63 minutes without a progress event.

## Worker cards across steering

A worker's coordinator-timeline card is one ACP tool call (`worker:<assignment>`)
that outlives the coordinator turn which created it; the runtime keeps sending
its updates, and its real outcome, in later turns. The ACP feed of a
cancelled prompt therefore still carries `worker:` card updates
(`Agent._make_emit_tool_call`); its other updates stop with the turn. Before
that, every update to a card created by a turn Paseo cancelled was dropped
(not sent and not journaled), so a steered worker's outcome and a lone Stop's
cancellation never reached the card.

The Paseo ACP bridge settles every tool call that is still `pending` or
`in_progress` when a prompt ends. It does this itself
(`terminalizeTransientItems`), not from an agent update, and merges later
updates that carry no status into that settled status. In `@getpaseo/plugin`
0.9.1 (this plugin's pinned dev dependency) the settled timeline item is
`failed` for a cancelled turn and `completed` for a completed one; in the
bridge bundled with the Paseo 0.11.4 application, a cancelled turn's
item is `canceled` (its merged snapshot is still `failed`). Under
cancel-then-prompt steering this showed a still-running worker as Failed
(`Tool call failed: <title>`) or, on 0.11.4, Canceled.

The Cortex provider keeps each worker card on the status Cortex last sent
(`integrations/paseo/server/worker-card-fence.ts`). It reads the raw ACP frames
before the bridge, restores that status in the bridge's merged snapshot, and
withholds the bridge's own settlement (`failed`, `canceled` or `completed`) of
a worker card Cortex still reports running when the turn was cancelled
(steering or Stop) or completed. The card
keeps its identity and stays running until Cortex reports the outcome. Limits:

- A prompt that itself failed (an agent error or a lost transport) keeps the
  bridge's failure: nothing then shows the worker survived.
- Cortex's own updates always pass. A real worker failure keeps its reason, and
  a lone Stop shows the runtime's `Cancelled: turn_interrupted` card update once
  the grace window pauses the task.
- The first terminal status Cortex sent is final for that card: a late,
  duplicated, out-of-order or contradictory terminal frame cannot replace its
  projected status or reopen it.
- Other tool calls, and cards Cortex never reported in this connection, keep the
  bridge's behavior.

The runtime half is covered by the Rust incoming-ACP suite (the
runtime never settles the steered worker's card; the same card later reports
its result). `integrations/paseo/server/provider-worker-steer.test.ts` (in
`npm --prefix integrations/paseo run test:steer`) drives the real installed
bridge through the assembled provider with a scripted agent: durable steer,
repeated cancel-then-prompt steering, a turn that completes while the worker
runs, the real outcome, a stale late frame, genuine failures, a lone Stop, a
failed prompt and history reload. `integrations/paseo/client/client-bundle.test.ts` checks the
resulting card props. These establish provider and presentation inputs, not
rendered Paseo Desktop behavior. Timeline records Paseo already stored from an
earlier plugin version are not rewritten.

## Native composer queue versus captured steering

Read-only inspection of the installed Paseo 0.10.2 client on 2026-10-02 confirmed a separate native composer queue with Edit and Send now, but no Cancel/Remove action. Its plugin runtime exposes no composer-write or native-queue mutation API. Installed-helper tests exercise native queue/edit/send behavior without launching Paseo; they are not rendered GUI qualification.

### Why sent messages have no Edit icon

Paseo renders the pencil only for items in its **client-side composer queue**,
as chips above the composer, not for sent timeline bubbles. In default Steer
mode, Enter sends the message to Cortex immediately. Cortex acknowledges durable
capture and emits a normal `user_message` plus a `steer` result; waiting for the
workers to finish does not turn that captured source into a native composer
queue item. No provider capability, pending-message status or edit handler in
the installed API makes a sent bubble editable. The earlier change implemented
safe native queue draining and documented native Edit, not editing delivered
Cortex steering. Those are different queues.

After the first successful busy-turn send per provider session, Cortex now
shows an informational notice explaining this distinction and the actual
Ctrl/Cmd+Enter/pencil workflow. It does not change settings, resurrect a draft,
or claim the delivered message can be edited. Capture refusals still use only
Paseo's host error, without a duplicate notice.

This native queue is not Cortex's durably captured steering ledger described above. Once Cortex acknowledges steering, its immutable source history and requirement revision already include that input, and recovery capsules include it even before reconciliation. A plugin cannot safely implement Cancel by deleting captured history or Edit by pretending that history was never received. Full per-message Cancel/Edit restoring all content to the existing host composer is not implemented. The owner-selected alternative uses native Edit and the two-step cancellation workaround below; Cortex adds no Withdraw action, separate editor or simulated native controls. CLI checks cannot qualify that rendered interaction.

### Owner-selected native Edit workaround

Keep Paseo's default `sendBehavior: "steer"`; this guidance does not change the
owner's settings or drafts. While Cortex works:

- **Ctrl/Cmd+Enter** puts the message in Paseo's native, pre-delivery composer
  queue. Plain **Enter** sends durable Cortex steering immediately instead.
- The **pencil on the chip above the composer** is Edit; it is not an icon on
  a sent chat bubble. On a still-queued native item, **Edit** removes it from
  that queue and restores it to the existing composer without sending. Edit
  the content and send again.
- To cancel a still-queued native item, use **Edit**, then clear the composer
  text **and all restored attachments**. There is no native Cancel/Remove button.
- Save an existing draft before Edit: native Edit overwrites the draft's text
  and attachments without confirmation. Native enqueue trims surrounding text
  whitespace. Edit restores user attachments only as the host supports; the
  installed `userAttachmentsOnly` helper filters out review, browser-element,
  chat-history and pull-request-context attachments. Helper tests verify that
  files are retained and review context is filtered; full rendered restoration
  remains unqualified. Do not promise all attachment kinds or byte-for-byte
  restoration.
- The native queue appears to be in-memory; loss on reload/restart is inferred,
  not qualified. Do not rely on it as durable storage.

Do **not** switch to `sendBehavior: "queue"` for this workflow. In the installed
Paseo 0.11.2 bundle, **Send now** in that mode, and sending while a question card
is pending, replace the active turn: the host sends `session/cancel`, then
`session/prompt` once the cancelled run settles. Cortex treats that pair as steering
(see the grace-window rule above), so the running workers are not cancelled. A lone
cancel still pauses the task after the grace window. In default `steer` mode, Send
now uses the durable steering path. Re-verify this against the installed Paseo before
relying on it; the rendered GUI behaviour was not qualified.

Native Edit happens **before Cortex receives anything**: only the replacement
text is sent/captured when resubmitted; the original native item is gone.
Once the host starts sending/draining an item, Edit returns no draft for that
removed identity. After delivery, send a correction as a new message rather
than trying to rewrite history. Cortex binds each new config-channel capture
to a canonical content-block checksum in runtime-owned source metadata. An
identical retry is idempotent; a changed text/image/attachment payload reusing
the captured message id is refused, whether pending or consumed, with guidance
to send a new correction. A legacy capture lacking this checksum (or damaged
metadata) also fails closed on retry because exact content cannot be established.
No source history, requirement revision or pending steering is replaced by a
refused edit. This adds no schema migration or new client edit API.

The explicit no-paid installed-helper check is
`npm --prefix integrations/paseo run test:native-queue -- "$PASEO_CLIENT_BUNDLE"`,
where `PASEO_CLIENT_BUNDLE` is the inspected installed Metro JavaScript bundle
under `/opt/Paseo/resources/app-dist/`. It executes only selected queue helpers
with stand-in dependencies and the real Cortex provider wrapper: edited content
only, consumed-id refusal, and send-failure restoration. It does not start a
daemon, call an engine, or qualify rendered controls or reload persistence.

### Continuation delivery ownership

A normal new prompt/native queue drain arriving while Cortex runs a background
continuation waits for that continuation's natural completion; it no longer sets
the continuation's cancellation event. Waiting prompts are session-local FIFO
requests (bounded at 64), with their original text, content blocks and metadata.
Ownership is reserved under the session lock before the next prompt starts,
so two controllers cannot run for that session. Ordinary foreground-in-flight
prompts remain refused, and an existing live question waiter still receives its
answer rather than a second turn.

A waiting normal prompt has **not** yet been durably captured or acknowledged.
When admitted it uses the unchanged normal capture/provenance path. Stop cancels
waiting requests with a cancelled response before capture; disconnect discards
unacknowledged requests and never starts them. Captured steering history is
never erased. Config-channel acknowledgements still follow durable capture, and
replaying their message identity adds no source, steering revision or turn.
Explicit `session/cancel`, `/cortex-stop` and `/cortex-finish` retain their
documented interruption/terminal semantics; normal queue draining cannot
masquerade as those actions.

Deterministic no-paid fixtures reproduced the former drain hazard: a normal
prompt set the background token, moved the task to RECOVERING and cancelled its
active worker. Regression fixtures now cover non-interrupting delivery, ordered
handoff, captured content/provenance, pending questions and explicit Stop races.
They establish runtime/protocol behavior, not rendered native Edit or host
queue persistence.
