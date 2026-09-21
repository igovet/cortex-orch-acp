# Security policy

## Current ACP boundary (1.0.0)

Cortex is an incoming ACP stdio agent. It starts Codex or Claude ACP engines as separate processes and keeps one private project-local .cortex store. The coordinator model chooses assignments, evaluates results and makes the final task decision. Runtime checks enforce identity, storage integrity and the transitions that can be verified mechanically. They do not establish that a model followed every instruction or that a result satisfies a user requirement.
For an explicit harvest refresh, model guidance calls for a source-backed
post-write inventory of affected examples, safe exact exit/stdout comparison,
and separate source/documentation review when behavior, defaults or output
change. Unverifiable examples remain unresolved. A clean task decision does
not establish semantic accuracy of all examples; nontrivial visible UI review
defaults to an independent reviewer unless a concrete low-risk exception is
recorded. Coordinator and implementer checks are not independent, and the
reviewer must report what it actually inspected and any remaining gaps.

The normal writable store-open path automatically migrates an exact format-11
database to format 12 only after taking the project-local exclusive access
lock and creating a verified private backup. The migration is transactional;
the backup remains available under `.migration-backups/`. Inspection opens
with `initialize=False` never perform this mutation and return
`legacy_storage_migration_required`. Format 10 and unknown formats are
refused rather than guessed or migrated automatically.

Every store mutation is dispatched through MCP tools/call with a verified task channel and current ownership epoch. A worker channel cannot make coordinator decisions or publish a pipeline edition. Report publication saves an immutable Markdown result; publish_result ends the worker attempt; decide records the coordinator's decision. Replayed deliveries recover their original receipt, while a reused key with different content conflicts. See [ACP security boundaries](docs/acp/security-boundaries.md), [orchestration](docs/acp/orchestration.md), and [publications and consistency](docs/acp/publications-and-consistency.md).
The coordinator's `list_events` operation is read-only and accepts only the
advertised cursor page (`{}` initially, then the exact returned `after` cursor,
with `limit` 1–200); it has no delivery key. `write_report` is a mutating
publication: the model must preserve `required_first_line` and replace every
exact line in the server-returned `replaceable_markers`, including guidance
comments, before submitting the draft. A fabricated cursor, marker, report
reference, or shortened server value is rejected or remains unverifiable.
The advertised `write_report.report_id` maps to the worker publication
argument `publish_result.report_ref`; an invalid first-call shape remains a
tool error and is not silently accepted.
Candidate checks run as the user in direct-project mode. Their environment disables Python bytecode writes, and Cortex compares the staged candidate before and after each check; this detects changes but is not OS-level containment. A changed candidate or failed check remains failed with a bounded reason code.
The first `request_check` binds the observation to the current requirement
revisions, candidate digest, literal argv, checked-file scope and environment.
An exact, complete, explicitly assessed pass may be reused only while every
binding remains applicable. A report claim, an exit code without an
assessment, truncated output, changed candidate or changed requirement cannot
substitute for that evidence or be reused.
For an assignment that explicitly requires rendered or UI evidence, a current
candidate check may supersede only a worker `not_run` or `unverified` result
when its exact attempt, candidate digest, command, scope, environment and
current requirement bindings match and the coordinator records
`assess_check(pass)`. An explicit worker `failed` result remains a blocker and
cannot be overwritten by a later candidate pass. `publish_result` rejects a
missing or empty structured `checks` array for such an assignment before
recording `worker_result`; report prose is not evidence. Assignments without
an explicit rendered/UI requirement may publish without checks.
An assignment-level acceptance only records that assignment decision; task completion requires a separate overall accept decision and a current completion gate. Before overall acceptance, the coordinator must publish a current `pipeline.md` edition with its decision basis and evidence. The edition describes actual unresolved deliverable or verification work without a temporary acceptance status; the durable task decision and state record the outcome. The gate refuses a missing or bootstrap-only pipeline and binds the acceptance event to the exact current edition; a later edition invalidates that event at completion recheck. The runtime does not write that edition or make the semantic decision for the coordinator. Routine report templates do not prefill incident-only decision fields.
The authoritative worker route context is the durable assignment's provisioned
attempt and its immutable route audit. For a mixed-team assignment this records
the engine, connection, model and effort actually opened, or an explicit
unavailable marker; the coordinator's current route is not used as a proxy.
Structured predecessor reports, revision feedback and questions are labelled
evidence data. The current `pipeline.md` is coordinator-authored decision
prose: its digest and edition bind the completion gate, but its wording does
not become runtime route or acceptance state and is not treated as independent
verification.
Worker handoff prompts name the exact predecessor `report_ref`; only its
trusted `read_report` receipt proves that the coordinator read that report.
An explicit overall accept can discharge only its own assignment-accept reminder in the same transaction as the task decision. The reminder remains auditable as superseded; pending worker or user evidence still blocks completion.
Two observed same-turn task-accept refusals caused by pending meaningful events trigger cancellation of that downstream coordinator engine turn only. The trigger recognizes the exact Cortex MCP `decide` identity reported by Codex ACP or Claude ACP, including titleless result updates correlated to the original tool-call reference; a different server's `decide` cannot trigger it. Cortex then delivers those events in a fresh continuation, where the coordinator may make another task decision. This does not consume undelivered evidence, bypass the completion gate, cancel the host task, or make an engine failure look successful. A real-client exercise of this transition remains unverified for the fixed Claude identity.
The coordinator instruction also directs it to resolve the stated missing durable evidence or state before retrying a refused acceptance. This is model guidance, not a runtime guarantee that the model will comply.
After a successful observed `delegate` tool batch, Cortex likewise ends only the downstream coordinator engine turn and waits for durable worker/user events without model polling. Intermediate coordinator prose from nonterminal Cortex-tool turns is suppressed as a display boundary, not evidence of model compliance or a security boundary; tools and durable decisions remain subject to their normal checks.
If a worker reports a normal `end_turn` without a durable `worker_result`,
`worker_question`, or owned publication, the runtime sends exactly one bounded
corrective continuation through that same worker session and keeps the
assignment working while the coordinator waits. The continuation cannot
create evidence, change the route, or become a second assignment. A second
no-result `end_turn` records `assignment_failed` with
`worker_ended_without_result`; an engine failure, refusal, or cancellation
follows its own terminal path and is never reclassified as recoverable. This
is a lifecycle safeguard, not a claim that the model will comply with the
correction or that an incomplete turn produced a result.
After an accepted final response is emitted and the accepted state is committed, Cortex terminates the task's owned coordinator engine attempt. Its connection capacity is released only upon confirmed process exit; an uncertain exit remains an ambiguous reservation for reconciliation. This does not terminate another task's session or erase reports.
Capacity starvation checks use one reservation snapshot for both count and kind, so a confirmed worker release cannot be misread as coordinator-only saturation between two reads.
An urgent `/cortex-stop` also terminates worker attempts durably bound to that task, even when an assignment already has a terminal event. The task ID scopes process cleanup; confirmed exit releases capacity, while an uncertain exit remains reserved for reconciliation.
Paseo's `session/cancel` notification has the same task-wide scope because its
Workers control is a task stop. Other ACP hosts map `session/cancel` to the
current coordinator engine turn/session; it does not terminate unrelated worker
attempts. Worker cards retain their first terminal `completed`, `failed`, or
`canceled` state and replay that state after reconnect, so a stale running
display is not treated as proof that a worker is still active.

The private .cortex directory contains SQLite metadata, task sources, drafts, reports, staged work and receipts. These may contain secrets or other private project data. Cortex does not claim automatic secret detection. Do not commit or export the store, raw engine logs or private reports without authorization. Source text, predecessor reports and external documents are data with their own provenance; they do not acquire user or system authority by appearing in a prompt.

Codex role instructions are forwarded by an exact-source-verified, owner-private
codex-acp 1.12.0 copy to App Server `thread/start.developerInstructions`, with
effective native developer instructions retained. Cortex creates and verifies
that copy automatically on each engine launch from the upstream bytes; the
installed package and native account remain unchanged. The no-paid adapter
trace proves the `thread/start` payload; it does not prove a model received or
followed that developer text. The patched adapter delivers the role through
`developerInstructions` without duplicating the role block as user content;
native Codex compaction persistence remains unqualified. See [Codex adapter](docs/acp/codex-acp-adapter.md).

The same private adapter maps Codex App Server custom-tool response items,
including nested and replayed `rawResponseItem/completed` envelopes, to the
matching ACP tool lifecycle and preserves observed inputs, outputs, exit codes,
and failures. This prevents a real tool call from disappearing from the ACP
history, but does not synthesize missing output or establish live Paseo
rendering; that client observation remains unverified until retained.

The same private adapter sends each Codex App Server `turn/start` a verified
`workspaceWrite` policy with `writableRoots: []`, `networkAccess: false`,
`excludeTmpdirEnvVar: true`, and `excludeSlashTmp: true`. This preserves the
current project workspace while excluding `/tmp` and `$TMPDIR` from Codex's
own workspace policy. It is an engine policy request, not an operating-system
sandbox or a guarantee against arbitrary native code; the native-tool
limitation below still applies. The installed-bundle no-paid trace checks the
serialized request and the upstream package remains byte-for-byte unchanged.
The current temporary-tree exclusion patch has not yet received a fresh
real-client probe, so its live effect remains unverified; the request trace is
not a substitute for that observation. A subsequent clean real Codex worker
probe observed one `/tmp` sentinel attempt fail with `Read-only file system`,
and the sentinel was absent. This qualifies that observed `/tmp` outcome
only; it does not prove exclusion or OS-level isolation for `/var/tmp`,
`.cortex`, arbitrary same-user paths, or other same-user processes. See the
[retained boundary probe](docs/acp/evidence/paseo-2026-09-21/live-boundary-probe.md).

Codex ACP can stream terminal output in `_meta.terminal_output_delta` updates
instead of repeating it in the terminal completion's `rawOutput`. Cortex binds
the buffer to the exact `(wire session, tool call)` pair, merges it only into
that tool's completion, and caps it at 4 MiB. A capped buffer carries an
explicit truncation marker and `output_truncated=true`. No output, partial
output, or absent terminal metadata is promoted to a successful check: the
runtime preserves the distinction between observed, truncated and unavailable
output for replay and acceptance.

## Engine and workspace containment

Production assignment launch binds a mutating worker to the exact `project_root`
opened by the ACP client (Zed or Paseo). Cortex serializes mutating workers with a
writer lock and ownership epoch, captures the binding's before revision, and
rechecks it immediately before and after writes. A worker that cannot acquire
the current binding or observes stale state fails closed; it is never silently
redirected to a hidden snapshot or another directory. The worker normally runs
with the ordinary user rights of the client-launched process. Optional OS process
containment may be used when already available, but it is not a prerequisite,
and direct mode does not claim to isolate the project, `.cortex` store,
network, credentials, or other same-user processes. The coordinator also reads
the exact current project root, so it sees worker edits during the same engine
session. Its role policy and ACP permission refusals restrict the operations
Cortex can identify, but same-user native tools are not OS-confined to
read-only access. An ACP permission dialog applies only to the ACP request it
mediates and is not a general host sandbox.
The coordinator permission bridge allows one narrow bookkeeping action:
an edit whose declared path matches an active pipeline draft owned by that
task's coordinator thread. It checks the current session and attempt, the
registered draft path and ownership, and the regular file before returning
the engine's own `allow_once` option. It does not ask the host for that
draft write. Ordinary non-delete Bash and in-project coordinator edits are
auto-approved when observed targets stay inside the bound workspace and
outside `.cortex`; deletion, move, truncate and outside/private targets use a
fresh one-shot host review when available. An explicit denial, timeout,
missing or stale requester, or malformed response remains refused. A worker
write outside the bound workspace is a hard deny and is never forwarded to the
user. This classification depends on the engine issuing an ACP permission
request and accurately declaring its target; opaque native effects remain
outside this bridge. Engine-native private skill trees (`.codex`/`.claude`,
including home-directory skill paths) and named project checks (`pytest`,
`unittest`, Python `-m` forms, and explicit test/check scripts) are hard local
coordinator refusals and never become host permission cards: the coordinator
is told to delegate them to a worker. Inline Python assertions naming
rendered UI properties (contrast, focus, DOM/ARIA, responsive behavior) and
clearly asserted grep/rg queries follow the same rule, while ordinary public
source searches remain allowed.
The exact owner-bound native Claude edit of the coordinator's report draft is
auto-approved after its ownership and path checks. Ordinary non-delete Bash
and in-project coordinator source edits are refused locally with a notifier;
ordinary coordinator read operations remain auto-approved when observed targets
stay inside the bound workspace and outside `.cortex`; engine-private skill
paths and named project checks remain hard local refusals; deletion, move,
truncate and outside/private targets require fresh host review. Recognized
`/bin/bash -lc`, `bash -lc`, and `/bin/sh -c` wrappers apply the same rule to
literal nested non-delete commands; wrapped `rm` still prompts, while malformed
or opaque wrappers may prompt.
Declared Bash commands use a broad default-allow policy when targets stay
inside the bound workspace and outside `.cortex`. Destructive, private,
outside, symlink, or malformed commands remain reviewable or unclassifiable;
opaque effects cannot be guaranteed safe.
The unified execute/other classifier permits `2>&1` when the surrounding
command remains safe; true truncation, deletion, private, and outside targets
still require review.
The same boundary applies to native scripts: the exact
`PYTHONDONTWRITEBYTECODE=1 python3 -B - <<'PY'` form is auto-approved only
when its body is an AST-proven pure heredoc calculation. Unknown or mutating
scripts require host confirmation when available.
Worker Bash classification now uses the observed command text for
non-destructive commands, including read-only heredocs such as `from math
import pow`. Observed Bash `>` redirects auto-approve when every target is
inside the bound workspace and outside `.cortex`; `/tmp`, private targets and
deletion remain host-confirmed. Dynamic or opaque interpreter effects cannot
be completely classified, so this is not complete deletion enforcement.

Coordinator role refusals have an ACP protocol limitation. Claude ACP's
`reject_once` response carries no policy explanation, and its native adapter
can therefore produce the generic `User refused permission to run tool` text.
Cortex treats that outcome as a role event: it records the concrete refusal
kind, suppresses the misleading native prose from the user-facing feed, and
continues the same coordinator session with model-visible guidance to
delegate the work. Coordinator source edits, engine-private skill access and
coordinator project checks are handled this way and are never turned into a
host permission card. This proves only the runtime's recorded continuation;
the new path has offline/composed coverage but no fresh live Claude
qualification yet.

For the verified codex-acp 1.12.0 build, Cortex uses an owner-only private
copy of its adapter with the `agent` approval reviewer changed from the
engine's auto reviewer to the host user. The installed package and native
account are unchanged. This makes engine-issued approval requests visible to
Cortex's ACP permission bridge, where ordinary worker requests are allowed
and recognized `rm` requests go to the client. Native commands that execute
without an approval request remain outside that bridge; the previous live
sentinel deletion was auto-approved internally and is recorded as a failed
permission scenario. A subsequent disposable Paseo run showed an exact
`rm` permission card and explicit `allow_once` selection, but that task
did not finish because the coordinator kept retrying an acceptance decision
inside one turn. It is not end-to-end qualification.
An engine-issued `rm` approval is recorded only after the host response and
a fresh attempt-binding check; task acceptance rejects an observed `rm`
without an exact allow-once receipt. This checks the provenance of a host
selection, not whether a client actually required a human click.
For trusted-channel `read_report`, a successful receipt is bound to the exact
task, attempt, report reference, and request key. A pending ACP observation or
unmatched tool card is not evidence of a successful report read.

Cortex launches ACP engines with `PYTHONDONTWRITEBYTECODE=1` so ordinary Python
tests that inherit the engine environment do not create `__pycache__` in the
project. The project-revision backstop still rejects other observed writes by
the coordinator; this setting is not a filesystem boundary.

Linux operation requires no user-namespace setting, `bubblewrap`, `systemd`
sandboxing, or AppArmor rule. Workers execute directly in the exact client project
with the user's ordinary OS permissions. Runtime writer leases, revision
checks, dispatch authentication, and recovery evidence remain active, but they
do not restrict native engine tools at the OS level.
If a direct worker edits during the coordinator turn but publishes later,
Cortex durably retains the unresolved turn boundary. It allows acceptance
only after the worker's publication captures the exact current project tree;
missing, failed or mismatched publications remain blocking across a pause or
restart. This prevents an in-progress direct worker edit from being
automatically attributed to the coordinator. The digest is an observation,
not proof of which same-user process wrote the bytes; native coordinator
tools are not prevented from writing by this check.
For shared-root workers, provisioning also retains a pre-attempt manifest and
the runtime compares durable attempt deltas. Concurrent attempts with missing
evidence or overlapping changed paths are refused with a concrete
`shared_root_*` attribution reason; disjoint path changes remain admissible
when the manifests are complete.

The runtime bridges for filesystem and terminal requests validate their own
project-bound paths and deny private `.cortex` and `.git` access. Those bridges
do not govern arbitrary engine-native tools, which retain ordinary user-level
filesystem access in direct mode. Forwarded MCP servers and engine
configuration are additional authority sources; their behavior must be
qualified on the actual installed engine. The owner-only
`$XDG_CONFIG_HOME/cortex/mcp-servers.json` registry is loaded for `session/new`
and `session/load`; Paseo's empty `mcpServers` payload therefore does not
disable the globally registered `codebase-memory-mcp`, while Zed-forwarded
session entries remain supported. Registry environment values are held in
memory only and are not persisted in `.cortex` or diagnostics. Project-local
executable MCP configuration is deliberately unsupported, and the reserved
`cortex` name cannot be shadowed. See [permissions and containment](docs/acp/permissions-and-containment.md) and [engine adapters](docs/acp/index.md).

Native account MCP configuration is handled per child process. The pinned
Codex launcher checks that `features.plugins=false` is effective, inventories
the CLI's inherited native config MCP servers by name and enabled state, then
overrides every discovered name to `enabled=false` and verifies the result
before App Server starts. An unsupported or duplicate name, malformed inventory,
or failed check stops the child. The private session adapter also sets
`features.plugins=false` in new, resumed and forked session configuration and
allows Cortex's explicitly supplied `mcpServers` even when a name matches a
disabled native entry. For Claude, each Cortex-created session sends
`strictMcpConfig: true`; the installed SDK maps it to `--strict-mcp-config`,
whose declared contract excludes native user/project/plugin MCP discovery and
uses the session's explicit server list. These are scoped engine controls, not
OS isolation or proof of live enforcement. No fresh real-client observation
has qualified this native MCP boundary yet.

The cross-session project-document catalog is bounded and confined to
`docs/project/` and `docs/features/`: it scans at most 4096 entries, accepts at
most 128 files, reads no file larger than 256 KiB or more than 2 MiB in total,
and caps skip diagnostics. Symlinked roots and entries are rejected. A scan or
file limit produces explicit truncation metadata, so omitted documentation is
observable rather than silently treated as complete.
Each new worker attempt receives this catalog as lower-trust prompt data with
an explicit boundary. Delivery is observable; it does not prove the worker
read a document or followed the suggested search order.
The Cortex MCP relay keeps its channel secret in the broker process, never in
the engine's environment. A leading-letter relay token avoids command-line
option parsing failures. Broker diagnostics record only the relay connection,
MCP child PID/exit status and a bounded receipt for the actual relayed
`tools/list`: tool names, count, and input-schema digests, never schema bodies
or descriptions. They never copy MCP frames, child stderr, tokens, or
credentials. An observed list missing the base report tools fails closed;
this receipt proves delivery through the relay, not that a model used a tool.
A failed connection is
reported as failure, while lack of a connection before the first engine prompt
is only `not_observed` because the engine may initialize MCP lazily.

Workers produce changes directly in the project opened by the user. Each
mutating attempt records its authorized project binding, writer lease, input
revision, changed paths, output revision and observed outcome. The runtime
serializes writers, checks current editor state before writing, preserves a
recovery journal, and records conflicts or unknown outcomes instead of treating
them as success. The writer lock coordinates Cortex processes; it cannot
prevent an unrelated external editor from changing the project. See
[workspaces and integration](docs/acp/workspaces-and-integration.md).
Worker protocol prefers inline in-memory calculations, avoids `/tmp` scratch
scripts, and limits writes to assigned paths; a read-only QA script outside
that boundary still requires host permission.

Verification executes a direct argv command against the exact project revision
produced by the attempt and records command observation separately from the
coordinator's semantic assessment. A completed tool call alone is not a
passing check. Evidence retains the project revision, attempt, requirement
revisions, environment identity and check scope. Stale requirements, changed
files, incomplete or truncated output and unknown process outcomes cannot
establish a passing assessment. If the shared-root binding or its before/after
revision observation is missing, stale, mismatched or unavailable, the worker
result and acceptance path fail closed; the current acceptance capture must
be an observed revision, and an unavailable, capped or racy capture is
retained with its concrete reason. Model-authored report or pipeline prose
cannot repair that missing identity. Only the coordinator interprets the
observation against a criterion. See [test matrix](docs/acp/test-matrix-and-ci.md).

For `delegate`, an empty optional `plan_assignment_ref` has no identity and is
normalized to omission. Every nonempty plan reference is checked against the
current task before dispatch.

## Recovery and source identity

Task sources are captured from the current ACP session with origin and event identity; model-authored tool arguments cannot replace the user's request. Distinct turns remain distinct even when their text is identical. Published reports remain immutable, and pipeline editions keep the newest edition first with older editions below. Durable attempts, event claims, instruction deliveries, candidates and checks let recovery reconcile an uncertain operation before retrying. External effects are not assumed to be exactly-once unless the external system confirms them. See [ACP documentation](docs/acp/index.md) and [real-client runbook](docs/acp/real-client-runbook.md).

Outbound text and structured ACP display updates are recorded before transport
enqueue and replayed by `session/load` after reconnect. Recording proves a
runtime event was produced; it does not prove the client received or rendered
it. Old sessions that lack structured update rows do not acquire invented
worker/tool cards from task observations.
Coordinator stream chunks carry one `messageId` per logical assistant turn,
with a fresh id after a tool boundary and on a later turn. The same stored ids
are replayed, allowing clients to coalesce chunks without joining distinct
messages.

The native Paseo 0.8 provider has one client-specific display compatibility
transform: a tool update observed as `failed` with null output receives the
bounded non-null message `Tool call failed: <title>`, while its failed status
is preserved. This transformer does not infer an underlying cause, alter the
runtime result, or authorize a fallback attempt. The retained Paseo production
reproduction predates the transformer; the post-fix live agent observation is
still `NOT RUN`, so source-level and offline checks must not be reported as a
client pass. See [input history and replay](docs/acp/input-history-and-replay.md)
and the [Paseo observation record](docs/acp/evidence/paseo-2026-09-20/live-acp-regressions.md).

For Paseo reloads, the native provider buffers historical `timeline.item`
events emitted before `session.opened`, separately per session, and forwards
`session.opened` before the original item order. The buffer is bounded at
4,096 items and approximately 16 MiB of serialized JSON. Overflow fails
closed and remains an unverified/incomplete replay; no history is silently
dropped or treated as accepted.

Each coordinator project and MCP tool call gets its own top-level card with a
human-readable title and complete visible inputs, except for the normal
credential/secret redaction boundary; lifecycle updates coalesce within that
call. Read, Skill and Cortex system operations are compact and non-expandable;
Edit and Shell calls use the host's native expandable input/output fields.
Coordinator and worker cards share semantic operation labels. The coordinator's
`delegate` call is represented by the worker assignment card and is omitted from
the top-level stream. Worker actions remain bound to their assignment card and
exact task/session history. In Paseo 0.8, the
agent-scoped composer Workers pill opens that history in a normal Paseo
workspace editor tab. The direct worker card is the link to that tab; it
contains the worker's coalesced real message chunks and published result.
Selecting a worker passes
its assignment reference into the newly opened tab, including when the
previous tab was closed. Paseo's supported plugin API exposes only the
`workspace` (editor tab) and `explorer` (side panel) locations, so Cortex uses
`workspace` and cannot request the private native Claude subagent layout, a
specific tab title, split, width, or persistence policy. The popover is a
bounded flat list with separators only between worker rows.
The composer pill remains available when the current session has no workers and
then shows an empty state. Paseo project Read rows preserve the structured
project file path and genuine project Edit rows may show a line-numbered
diff; the installed plugin API exposes no public file/editor-open navigation,
so those details are not clickable file-opening links. Protocol-only and
coordinator-generated worker events are filtered; genuine worker message
chunks, including `maturity`/`state` updates, remain visible and are merged in
order. The worker timeline row pulses only while working and stops on terminal
status, including after replay. Procedural coordinator self-talk is suppressed
while delegated work is active, but a substantive coordinator answer remains
required after the worker result/report is available.
The panel reads redacted `worker_activity` rows from `task_events` with cursor
paging and refreshes active history incrementally about every 750 ms. Exact
tool lifecycle updates coalesce by logical tool identity, while adjacent
message chunks are appended in order as one stream segment. In Paseo's
coordinator and worker feeds, Cortex MCP calls and operations against the
private `.cortex` store are hidden in live and replayed presentation; ordinary
project operations and external MCP calls remain visible. Task events, tool
observations, reports and receipts remain durable, so this is a display
boundary rather than evidence deletion. Non-Paseo clients retain the full
lifecycle card. Codex and Claude `wait` polling, private thought, native
collaboration and scheduler calls are intentionally omitted as engine
bookkeeping. Titles and portable content pass through credential redaction
before display and replay;
engine-private payloads and downstream terminal IDs are not copied into the
client session. An accepted final answer can repeat an exact SHA-256 only when
it appears in the current published pipeline edition. This protects that
machine-checkable identifier, not every factual statement in model prose.
Paseo's ACP mode selector is mapped only to the coordinator language setting;
`session/set_mode` cannot change an engine permission mode or grant access.
Coordinator text deltas are delivered immediately through a bounded lexical
redaction buffer that holds only the current token-like suffix. This prevents a
credential split across adjacent deltas from reaching the client without
buffering the whole turn; accepted final text still receives the pipeline
digest guard.
The displayed model/effort options omit the engine's literal `default` alias,
while dispatch still validates the selected concrete route and effort.
Paseo's combined model selector exposes a model ID only when it identifies
one route. Duplicate model IDs use distinct route IDs, and a bare ambiguous
model ID is rejected. A displayed first option alone is not authorization to
create a task configuration revision; the ACP client must send a selection or
a saved user default must resolve to that route.

InstructionBundle records intended sources and the representation actually sent through the observed engine adapter. This proves delivery to the adapter, not model compliance. Automatically loaded AGENTS.md, CLAUDE.md, user settings, engine skills and forwarded MCP behavior may remain outside Cortex's visibility. An unavailable signal is reported as unverified, never imputed as success.

Coordinator and worker role instructions forbid engine-native subagents. For
the pinned claude-agent-acp 0.78.0, Cortex additionally sends
`_meta.claudeCode.options.disallowedTools` with `Agent` and `Task` for every
new session. The installed adapter forwards those names to the SDK, which
passes them to the Claude Code CLI as `--disallowedTools`; this is supported
by local source inspection and fake ACP wire tests, not yet a live execution
observation. For pinned Codex CLI 0.154.0, Cortex's per-process launcher sets
`agents.enabled=false`, `features.multi_agent=false`, and
`features.multi_agent_v2=false`; it checks both feature rows before starting
App Server. Neither `agents.enabled=false` alone nor both disabled feature
rows alone prevented native subagents in observed live workers. Pinned Codex
source resolves the combined settings to a disabled multi-agent version before
model selection, but a subsequent live worker still selected V2: codex-acp
passed a whole `features` table in `thread/start.config`, replacing the
process flags after `config/read`. The verified private adapter now forces
all three disabled settings in each session config (new, resume, fork),
preserving unrelated settings. A no-paid installed-adapter wire check confirms
the outgoing `thread/start` settings; a fresh real-client turn is still needed
to verify its effective multi-agent version and model-visible tools. Existing
running threads may ignore resume config overrides;
an instruction delivery receipt alone does not prevent either engine from
attempting one.

## Qualification status

Offline tests and the no-paid ACP startup smoke exercise source behavior and protocol surfaces. They cannot establish real Zed, Paseo, Codex or Claude end-to-end behavior. The [real-client runbook](docs/acp/real-client-runbook.md) and retained [Paseo observations](docs/acp/evidence/paseo-2026-09-20/live-acp-regressions.md) record only the scenarios actually seen; unobserved rows remain NOT RUN. Paid probes require explicit opt-in flags and an approval reference. The Zed qualification profile and project are disposable, while its Cortex child uses the user's existing production Codex and Claude native accounts. The Paseo observations use the user's already-open production Paseo at their explicit request, with the existing `test-ai` project for task work. Qualification does not copy credentials or create test engine accounts.
The retained Zed baseline includes Claude Sonnet worker activity, but its
selector/effective-route mismatch means it is not a Sonnet coordinator pass.
The fixed route/context and attribution behavior still requires a fresh
real-client observation.

The release version is 1.0.0. The exact payload hash is recorded in [package-identity.json](package-identity.json) after final stamping; this document does not pin a changing source hash. Historical Codex plugin, native-subagent, gateway and lifecycle-hook material is not part of the ACP product or its current security contract.
