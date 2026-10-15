# User input, questions and cancellation

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed.


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
phase. Exact retries preserve the original question identity; changed deliveries
with the same key fail.

After three planning prompts without a structured proposal, the runtime asks a
recovery question with `structured_proposal_missing`. A recorded answer resumes
`RECOVERING`; after owned-work termination is confirmed, recovery returns to
`PLANNING` with a fresh three-prompt cycle. This also applies when elicitation
answers synchronously within the same host turn. Clearing the wrong phase's
counter previously caused immediate reblocking without any new engine prompt.
Dismissal, absent answers and unconfirmed worker termination do not grant a new
planning turn. The limit is unchanged, and a renewed cycle without a proposal
still blocks; successful answer mapping alone is not successful orchestration.
The no-paid regression is `tests/test_question_recovery_continuation.py`; it
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

The runtime's own stop question (asked when the runtime stops a task on a blocker) is different. The runtime recognises it by a structured marker on the question, not by its wording, so a
coordinator question with the same text never force-finishes the task. Its Finish option (by number,
stored or rendered English or Russian text, or label) ends the task `CANCELLED` with
`USER_FORCE_FINISHED`. Continue resumes the runtime flow and never releases the stop. Only an answer
in the user's own words releases the question. See
[the decisions log](../../project/decisions.md#runtime-stop-question-finish-and-continue).

The question text itself (`acp.messages`'s `stop_question`/`stop_reason.<code>` catalog
entries) never shows a bare internal code as the whole reason. Each recognised stop
code (worker failure, verification failure, review limits, an interrupted result
delivery the runtime could not reconcile against the worker's actual publication,
a lost worker whose recovery could not finish cleanly, and so on) renders as a short
plain-language sentence in English or Russian; the raw code still follows it in
brackets (for example `... [result_publication_mismatch]`) so it stays available for
diagnosis next to the readable reason, never instead of it.

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
choose wrongly. The no-paid controller regressions are
`tests/test_steering_after_verification.py`.

When the queued message carries an image or uploaded-file attachment (its
bytes/base64 never forwarded to the already-running turn), the retained
steering carries the same `source_ref`/`name`/`uri`/`kind`/`availability`
descriptor `acp.agent` already records for durable capture, plus a derived
short English `attachment_hint` such as `Attachments: image r_xxx:1
(screenshot.png) - open with read_source`, so the coordinator is never left
with only the plain `[image: ...]` text marker and has to guess an attachment
exists by calling `list_sources`. A text-only message leaves both keys absent.
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
deny-write rule is created only by an explicit `deny-write: <path>` instruction; wildcard
or mask characters are rejected, and natural-language phrasing such as "do not modify ..."
creates no denial;
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

The coordinator uses the language selected for the submitted message, including
when reports and context are English. Workers receive an English communication
rule in their shared generated protocol and in every ordinary, recovery and
fallback turn. User-facing structured question text and all options are the
exception: the advertised question contract requires the current turn's
`user_question_language` (`en` or `ru`). The same strings reach a form or
text fallback verbatim. Only genuine user decisions belong in that question
tool; missing project access goes to runtime `access_request`, and unavailable
evidence is reported as a limitation. Exact quotes and explicitly requested
project-artifact languages are preserved. This is model instruction, not automatic translation or a claim
that arbitrary model output can be deterministically language-validated.

## Worker cards across steering

A worker's coordinator-timeline card is one ACP tool call (`worker:<assignment>`)
that outlives the coordinator turn which created it; the runtime keeps sending
its updates, and its real outcome, in later turns. The Paseo ACP bridge settles
every tool call that is still `pending` or `in_progress` when a prompt ends:
`failed` when the turn was cancelled, `completed` when it completed. It does
this itself (`terminalizeTransientItems` in `@getpaseo/plugin` 0.9.1 installed
for this plugin, and the same routine in the inspected Paseo 0.11.2 application
bundle), not from an agent update, and merges later updates that carry no
status into that settled status. Under cancel-then-prompt steering this showed a
still-running worker as Failed with `Tool call failed: <title>`, and later
progress kept that state until the worker's real outcome arrived.

The Cortex provider keeps each worker card on the status Cortex last sent
(`integrations/paseo/server/worker-card-fence.ts`). It reads the raw ACP frames
before the bridge, restores that status in the bridge's merged snapshot, and
withholds the bridge's own settlement of a worker card Cortex still reports
running when the turn was cancelled (steering or Stop) or completed. The card
keeps its identity and stays running until Cortex reports the outcome. Limits:

- A prompt that itself failed (an agent error or a lost transport) keeps the
  bridge's failure: nothing then shows the worker survived.
- Cortex's own updates always pass. A real worker failure keeps its reason, and
  a lone Stop shows the runtime's `Cancelled: turn_interrupted` card update once
  the grace window pauses the task.
- A terminal status Cortex sent is final for that card: a late, duplicated or
  out-of-order frame does not reopen it.
- Other tool calls, and cards Cortex never reported in this connection, keep the
  bridge's behavior.

The runtime half is covered by `tests/test_cancel_prompt_pairing.py` (the
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
