# Changelog

## 1.0.0 (unreleased)

Cortex is an incoming ACP stdio agent with a project-local runtime and
outgoing Codex and Claude ACP engines. The 1.0.0 release resets the package
identity to a content-stamped ACP payload.

This revision adds durable requirements, assignments and attempts; isolated
worker snapshots; exact staged candidates; candidate-bound checks; recoverable
integration; and explicit revision continuation. Instruction and context packets
record their sources and actual delivery, including session replacement. Report
publication remains separate from worker completion and coordinator decision.

Claude workers work again: resolving the installed claude-agent-acp version
(0.81.0, which has no delivery registration of its own) made every Claude worker
refuse its first prompt while the coordinator kept working; an unregistered
installed version now resolves to the pinned one as before. A failed worker is
recorded with its error code and source line instead of a bare class name, and
a coordinator can read its own role through `read_skill`.

A long chat reopens again. Replay sent every update of a worker card, and each
update carried the card's whole log, so one long worker made 17.7 MB of history.
That overflowed Paseo's 16 MiB history buffer, and Paseo showed "Couldn't
refresh agent history" with no way to continue. Replay now sends each card's
final state once (0.36 MB for the same chat). The Paseo relay also exits when
Cortex exits, so a failed load reports an error instead of hanging.

A question card in Paseo now has an "Other..." field next to its options. The
typed text answers the question as a message typed into the chat would.

A plan the user still has to approve no longer ends the task as finished. The
coordinator used to write such a plan into its final synthesis, so the runtime
printed `Завершено` under a question. It now writes the details as its message
and asks a one-line question with short answer options. The task waits for
the answer and then goes back to planning: the approved work runs as a new plan
revision with its own review, or the coordinator closes the task if the user
chose to finish.

Every Cortex MCP tool is now fully described, and each channel is offered only
what it can use. Twelve tools (project_read, project_write, command_run and the
rest of the project, outside and re-plan tools) carried a placeholder
description; the re-plan requests did not say they stop every running worker.
A coordinator is no longer offered the six write and command tools the gateway
always refuses it, read-only roles and review stages no longer see write tools,
consultants no longer see command_run, and result_submit offers verdict only on
a review stage, where it is required -- the most common worker refusal was a
non-reviewer sending it. Rules that lived only in server code are now in the
schemas (plan and scope paths, plan ids, project paths, the "absent" hash for a
new file, the exact skill ids per role), and a self-contradicting check-evidence
description was corrected. A test audits every tool in every role profile, so a
placeholder or an undescribed field cannot come back.

A refused Cortex tool call now says what to change. 148 of the 203 error codes
had no text of their own and fell back to the storage-failure message ("check
disk space ... retry only the identical request"), so a coordinator whose plan
named a forbidden scope resubmitted the identical plan and failed again. Every
plan-validation code has precise guidance, any other unlisted code reads as a
refusal to correct (or, for an uncertain outcome, one not to repeat), and the
plan schema itself states the scope rules.

External MCP servers get a private runtime directory per launch, so
codebase-memory-mcp no longer fails its handshake whenever its daemon is already
running outside the sandbox.

A worker that learned a context-window size for the first time in a process
deadlocked the whole runtime on the non-reentrant runtime lock: the controller
loop stopped observing state (so a question a worker had asked was never shown)
and the ACP reader stopped handling any client message, freezing the turn with
nothing displayed.

codex-acp 1.13.0 is qualified alongside 1.12.0, so Cortex now runs the adapter
that bundles Codex CLI 0.155.1 and its newer models (`gpt-6-sol`,
`gpt-6-luna`) reach the model catalogue. An engine version is resolved from
the install a connection will really launch rather than from a fixed constant.

Cortex launches only the codex-acp adapter version it has qualified: engine
discovery prefers a qualified install over a newer one, the background engine
update never prunes a pinned version, and a private adapter copy left pointing
at an earlier install is repointed instead of refused. Without this, the
background install of codex-acp 1.13.0 made every Codex session and every
model-catalog refresh fail closed, so Codex was unavailable and no newly
published engine model could reach the catalogue.

The Paseo plugin no longer drops a workspace/agent notification that arrives
while it is already refreshing its catalogue, which could leave a just-created
session without its Workers and Pipeline composer pills.

The retired Codex plugin, gateway, native-subagent observer, capability manifest,
old hook storage and phase evaluation harnesses are removed from the active
package, tests and documentation. All 23 profiles and eight report templates
remain supported. Offline checks establish source behavior only. Real Zed and
engine qualification is tracked in docs/acp/real-client-runbook.md and remains
unverified until retained live observations exist.

## 1.15.9

Stable release candidate `1.15.9+codex.sha256.e4f332d43bf38024` is release-ready
after final consecutive real CLI/Desktop qualification (`r_648c4f40dcf9`),
offline tests, and independent reviews. The user cancelled the Phase 2 outcome
comparison; it is non-blocking and non-scoring, with no efficacy comparison,
score, or promotion claim. Earlier failures remain historical diagnostics.

Add the versioned Phase 0 held-out evaluation contract with blind paired-repeat
scoring, explicit stop/null/cost semantics, and coordinator-owned promotion
recommendations. Add optional Phase 1 guidance for evidence and unrun-check
mapping, fact-versus-hypothesis debugging, and parallel-dispatch declarations;
these cues remain advisory and do not add runtime gates or server enforcement.
Add optional risk-triggered senior consultation guidance: a transition must have
both a consequence and an uncertainty trigger, with bounded packet/record and
baseline quality/overhead evaluation. Consultation remains coordinator-selected,
non-blocking and outside acceptance, planning and steering ownership.

Preserve raw upstream model-catalogue responses by disabling aiohttp's implicit
`Accept-Encoding` injection in the gateway. Caller-supplied `Accept-Encoding`
values remain lossless, and response content is still streamed without automatic
decompression. Add focused coverage for absent, `gzip`, and `br` negotiation.

## 1.15.7

Restore explicit Codebase Memory discovery in all 22 worker profiles. Structural
code questions use the graph before filesystem symbol searches, with exact-workspace
index selection, relevant coverage checks and concrete source-fallback reasons.
Cortex-only tool discovery no longer implies that Codebase Memory is unavailable.
Delegations carry the exact worker-skill token and require complete skill loading
before tool discovery or project work. Discovery emits names before selected schemas.

Keep every worker progress update, question, blocker and verification message on
the native parent/subagent channel. Workers do not call app task-messaging tools
such as `codex_app.send_message_to_thread`; completed handoffs return natively.
The seven MCP operations and storage format remain unchanged.

## 1.15.6

Replaced the orchestration protocol with six storage operations. The coordinator
owns the pipeline, native agents, steering and completion. All 22 profiles use
one Markdown reporting protocol. Each task has one pipeline document with newest
editions first; ordinary reports are immutable files. Both the catalogue and
report bodies support bounded cursor reads. SQLite retains metadata and durable
publication intents. No compatibility protocol or orchestration hooks are shipped.
The server now creates typed project drafts, embeds each short draft ID in the
filename and Markdown, and binds it to the caller's native thread. The writer accepts
only that ID. Large reports and pipeline editions use bounded-memory publication
without an application total-size cap or executable-string encoding. The MCP contract now advertises input and
output schemas and provides safe English parameter corrections.
Editable drafts live under project `.cortex`, outside Codex's protected `.codex`
metadata directory; the MCP alone publishes final task files under `.codex/cortex`.
Tasks now bind to native host threads, including registered child/parent lineage.
Public tools contain no task selector or returned task identifier; pipeline reads
resolve automatically. Native profiles attach role instructions at spawn; named
skills load normally without copying their bodies into profiles or putting
installation paths in assignments. The coordinator preserves the user language
across progress messages and recovery, and owns pipeline/governance changes.
Its execution boundary explicitly forbids project reads, shell, build, test,
package, browser and edit operations except filling its returned pipeline draft;
all such work is delegated to native profiles.
Report field guidance treats Markdown as literal data in executable host wrappers.
Catalogue previews target half their enforced limit to leave writing margin.
Only the content-addressed cache suffix changes for this replacement.
