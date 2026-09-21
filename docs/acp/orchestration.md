# ACP orchestration

Cortex is the incoming ACP agent. Its main runtime owns the coordinator and
worker engine sessions. Each engine gets a channel-verified Cortex MCP
subprocess; that subprocess validates the caller's role and records operations
in the project-local `.cortex` store. The main runtime launches engines,
serializes mutating writer leases, runs checks against the exact project root,
and records the resulting evidence. A model selects strategy and makes
semantic decisions; the runtime validates identities, transitions and
observable boundaries.

On a new Paseo session, Cortex advertises a unique engine model ID as the
combined model option value so Paseo's CLI `--model` can send a real ACP
selection before the first prompt. If several connections advertise the
same model ID, the options retain distinct route IDs and an ambiguous bare
model ID cannot choose an account. The selected route and effort are proposed
as a task configuration revision before an engine launches. Merely displaying
the first catalog option does not create that revision.

## Plan and attempts

The coordinator may use update_plan to record versioned requirements, their criteria and a graph of assignments. An assignment binds the requirement revisions and selected predecessor candidate it consumes. delegate starts a durable attempt for a planned assignment; a revision or retry creates another attempt while retaining earlier reports and outcomes. A simple task can still use one worker. The runtime does not insert mandatory researchers, reviewers or writers.

If a worker's engine reports a normal `end_turn` before it has left a durable
result, question, or owned publication, the runtime gives that same worker
session one bounded corrective continuation and keeps the assignment in its
working state while the coordinator waits. This is a recovery of an incomplete
worker turn, not a second delegation and not a substitute for a report or
evidence. A second no-result `end_turn` becomes
`assignment_failed(kind="worker_ended_without_result")`; engine failures,
refusals, and cancellation use their own terminal paths without this
continuation.

Workers are bound to the exact `project_root` opened by the ACP client. A
mutating attempt acquires a serialized Cortex writer lease and records the
project revision before and after its work; failure to acquire the lease,
stale state, or an ambiguous write is an assignment failure. There is no
hidden snapshot used as the destination of worker changes. A completed worker
publishes a Markdown report and calls `publish_result` with that report;
publication alone does not finish the attempt. The coordinator reads selected
reports by reference. ContextPacket binds current requirements, attempt
identity, selected reports, the exact project revision, feedback and open
questions, with provenance labels. Report and external-document text remains
lower-trust data.
Worker handoff prompts name the exact predecessor `report_ref` and require its
trusted `read_report` receipt before the coordinator relies on that report.
Worker protocol prefers inline in-memory calculations, avoids `/tmp` scratch
scripts, and writes only assigned paths. A read-only QA script outside that
boundary remains subject to host permission. Worker Bash classification uses
the observed command text for non-destructive commands. Bash `>` redirects are
auto-approved when every target is inside the bound workspace and outside
`.cortex`; `/tmp`, private targets and deletion remain host-confirmed. Opaque
interpreter effects are not treated as fully classifiable.
Each new worker attempt's engine prompt includes a current bounded catalog of
`docs/project/` and `docs/features/` references with source digests and an
untrusted-data boundary. The worker is advised to read the relevant present
index before broad source searches; missing indexes do not block the attempt.
This records prompt delivery, not the worker's tool order or document reads.

## Candidate and verification

Checks bind to the exact project revision produced by the worker attempt.
The first `request_check` for a candidate must copy the applicable current
requirement revisions from `read_plan` into `requirement_bindings`; a later
requirement revision invalidates only evidence bound to that requirement.
The request also stores the direct command, environment and covered files
before execution. The runtime runs the command in a bounded check process and
records the observed exit status and output state. `read_check` exposes that
observation. `assess_check` records the coordinator's separate judgment; a
completed tool call or exit code is not automatically a semantic pass.
Candidate/snapshot records found in older task stores are migration and audit
data, not a second production destination.
In direct-user mode the check runner sets `PYTHONDONTWRITEBYTECODE=1`, so
ordinary Python tests do not create `__pycache__` inside the staged candidate.
It compares the candidate manifest before and after execution and records a
bounded failure code if the candidate still changes. This detects drift; it
does not contain arbitrary native commands at the OS boundary.

Requirement changes invalidate affected evidence. A check requested before a relevant revision cannot later receive a passing assessment, including after a crash between revision and invalidation writes. Unscoped evidence is invalidated conservatively; evidence scoped only to unrelated requirements remains available. A changed candidate or unknown, incomplete or truncated command outcome cannot establish a passing check. The old evidence remains available for audit.

An exact, current candidate-check observation can satisfy verification in place
of a worker's `not_run` or `unverified` rendered-check result. This exception
is narrow: the candidate must contain that exact attempt, its digest, literal
argv, checked-file scope, environment identity and current requirement
bindings must match, the complete observation must pass, and the coordinator
must record an explicit `assess_check(pass)` for it. Cortex may reuse that
exact still-applicable assessed pass instead of rerunning an identical check.
An explicit worker `failed` result is contradictory evidence and is never
overwritten by a later candidate pass. Any changed candidate, requirement,
command, scope, environment, truncated output or invalidated evidence requires
a new check.

The worker's direct write path checks every affected editor buffer and current
project revision before mutation, uses the Cortex writer lock and a recovery
journal, and records conflicts or unknown results without deleting pending
changes. The lock does not prevent an external editor from writing; external
drift is a conflict to reconcile. The project opened in Zed changes during the
authorized worker attempt, under serialized ownership. A separate candidate
integration step is not required for the current direct-project contract.
When a direct worker publishes while the coordinator is still in its turn,
the MCP dispatch records the observed project tree before making the result
event visible. The end-of-turn check accepts that same observed tree as a
worker result; an unavailable observation or later unmatched change blocks
the turn. This is tree-state reconciliation, not OS-level writer attribution.
For a shared-root worker publication, the matcher retains the worker capture's
bounded manifest so it can identify a benign generated-only delta such as a
new `__pycache__` bytecode file. Any non-generated path change, missing
manifest, malformed evidence, or concurrent worker edit remains unattributed
and blocks the turn; the completion gate continues to require an exact digest.

## Decisions, events and recovery

Each MCP operation reaches the same dispatched, channel-verified path. Durable task events carry requests and outcomes; a request is not proof that its background operation succeeded. The runtime coalesces meaningful worker, candidate, check, integration and user events before continuing the coordinator. Once observed successful `delegate` calls in the current tool batch finish, Cortex ends that coordinator engine turn and waits locally; it does not spend model calls merely to poll or repeatedly read a capsule. A question or stop is delivered promptly. The turn boundary is based on observed tool lifecycle and a durable delegation request; an engine that never reports that tool lifecycle cannot establish this early boundary.
`list_events` is a read-only cursor page. Call it with `{}` to start at the
retained beginning, or pass the exact returned `after` cursor and an optional
`limit` from 1 through 200; it has no delivery key. Publication through
`write_report` requires replacing every exact server-returned line in
`replaceable_markers`, including guidance comments, while preserving
`required_first_line` and satisfying `required_replacement_count`. Do not
invent ids, cursors, marker text, or report metadata; reuse values from the
immediately preceding tool result.
The advertised `write_report.report_id` maps to the worker publication
argument `publish_result.report_ref`; a first-call argument mismatch remains a
tool error and is not silently treated as publication.
For an assignment whose brief explicitly requires rendered or UI evidence,
`publish_result` fails fast unless `checks` is a non-empty structured array.
The worker must name the criterion, provide its outcome, and link the exact
observed tool call when one produced the evidence. Informational or review
assignments without an explicit rendered requirement may still publish without
checks. Report prose never substitutes for these structured checks.
Only a successful trusted-channel `read_report` receipt proves a report read;
the receipt binds the exact task, attempt, report reference, and request key.
An ACP observation that is still pending or lacks that binding remains
unverified.

The coordinator records acceptance or revision through decide. decide(revise) creates a linked continuation with feedback, prior result and current requirements; decide(accept) records a decision about the available result rather than inferring success from report prose. A stopped task is fenced from stale continuation writes and worker prompts. Cancellation releases resources owned by that task, while uncertain external effects require reconciliation before retry.
Assignment acceptance and task completion are distinct decisions. A successful
assignment-level `decide(accept)` receipt reminds the coordinator to record
an overall task decision after reconciling the remaining assignments and
evidence. An assignment decision alone never marks the task accepted.
If the coordinator records the explicit overall accept in that same turn,
the task decision atomically discharges only its own matching assignment
reminder; pending worker or user events still prevent acceptance. The
reminder stays in the event log as a superseded audit record.
Before task-level acceptance, the coordinator publishes its current
`pipeline.md` edition with the decision rationale and relevant evidence. The
completion gate refuses acceptance when the current coordinator edition is
missing. It binds the decision to that edition's report ID, sequence and
digest, then checks the binding again before terminal completion. This guards
the durable task state without authoring the coordinator's decision or
asserting that the published prose is semantically correct. The task decision
event and stored task state are authoritative for whether acceptance occurred;
the pipeline records the model's decision basis without a transient pending
status that would become stale immediately afterward.
After a refused acceptance, coordinator instructions require repairing the
stated missing durable evidence or state before another decision attempt;
an unchanged refused `decide(accept)` must not be repeated. Pending events
are delivered in a later continuation before a retry. This is model guidance,
not an additional completion gate.
The coordinator recovery capsule exposes each provisioned worker's exact
`attempt_route_audit` engine, model and effort beside its assignment, or an
explicit unavailable marker when no audit exists. It also exposes task
acceptance separately from assignment decisions, requiring both the retained
task-level accept event and the stored accepted state. These are context facts
for the coordinator; they do not parse pipeline prose or change the completion gate.

## Client display and final-answer integrity

Cortex shows project and external MCP tool calls as compact lines with a
short input preview. Each coordinator call has its own ACP card. Lifecycle
updates for that logical call coalesce in place, so completion does not add a
second entry, and adjacent calls never share a card. Read, Skill and Cortex
system operations use compact, non-expandable cards. Edit and Shell calls use
the host's native expandable card fields for their structured input and
output. The same semantic operation labels are used for coordinator and
worker cards. The coordinator's `delegate` call is hidden from this stream
because the worker assignment card is its visible representation. Each worker
assignment has one durable lifecycle card for its own project and external MCP
calls. Its final model message appears when observed; otherwise a bounded,
worker-authored result summary appears when one was published.
Tool output, host-side computer/UI-control runtime calls (such as the CUA
bridge), and engine activity such as waiting, scheduling and thinking do not
appear in those cards. In Paseo's coordinator and worker feeds, Cortex MCP
operations and operations against the private `.cortex` store are hidden,
including streamed and `session/load` replay presentation. Ordinary project
operations and external MCP calls remain visible. The corresponding task
events, tool observations, reports and receipts remain durably stored; the UI
filter does not remove evidence. Cards include the worker
assignment in their IDs and labels, so concurrent workers cannot overwrite one another.
Once a worker card receives `completed`, `failed`, or `canceled`, that first terminal
outcome is authoritative: later activity snapshots and contradictory terminal
updates from the worker executor are discarded. This prevents a stale
`in_progress` update from reopening the card, or a late `completed`/`failed`
update from contradicting the durable outcome, in clients such as Paseo 0.8.
Display updates are stored before transport enqueue and replayed by
`session/load`. The client may render these cards differently; generic ACP
does not have a nested-subagent entity.
The durable `worker_result` closes the corresponding graph attempt and takes
precedence over a stale running snapshot or lower-priority terminal observation
for that attempt. This status projection does not remove the underlying event
history. Paseo presents a genuine project Read with its structured path, while
a compound shell remains a Shell operation with its command/output rather than
being relabeled as a Read or Edit.
The installed Paseo 0.8 plugin provides a Workers popover through its
agent-scoped composer Workers pill, which remains available for every agent
session. With no current-session workers it shows an empty state. The popover
is a bounded, read-only flat
list with separators only between worker rows; its compact layout uses a
bounded height and an inner scroll view. It polls
the current session about every 750 ms while an attempt is active. The initial
load shows a loader, while later cursor polls keep the existing transcript
mounted so the panel does not jump or collapse as worker events arrive. The
visible name comes from the resolved worker role (such as `explorer`) in both
the coordinator card and Workers view; `delegate:` IDs remain internal. The
composer pill is agent-scoped. Paseo 0.8 can place that
pill close to the message footer on mobile, so mobile spacing/placement remains
a host limitation; the plugin does not claim to have fixed that placement.

Selecting a worker stores its assignment reference before opening the Workers
workspace editor tab for the current ACP session and task. The coordinator
worker card is also a direct link. If the tab is closed, the next click creates
the tab and resolves that stored reference before loading history, rather than
landing on the unfiltered worker list. Paseo's plugin API does not expose a
more specific tab title, split, width, or persistence control. Cortex records
redacted worker activity as durable `task_events` rows (`worker_activity`),
bound to the exact task, assignment and worker attempt. Exact tool lifecycle
updates coalesce to the same logical tool row, while adjacent worker message
chunks are appended as one stream segment in order. Protocol-only and
coordinator-generated events are filtered from that worker feed, while genuine
worker message chunks, including `maturity`/`state` updates, are preserved and
merged in order. Reload and long transcripts remain bounded by cursor paging.
Paseo project Read rows preserve the structured project file path, and
genuine project Edit rows may render a line-numbered diff. The installed Paseo
plugin API has no public file/editor-open navigation capability, so these paths
and diffs are display details rather than clickable file-opening links.

When the ACP session identifies itself as Paseo, the worker chat preserves and
coalesces all genuine worker message chunks in order, including the published
`worker_result`, and tells the user `Open the Workers pill for full history`.
The pill is the navigation affordance; worker activity does not become a
top-level coordinator stream. Clients that do not identify as Paseo
retain the full worker lifecycle card because ACP has no portable nested-panel
capability. The generic ACP provider cannot populate Paseo's provider-native
panel without the installed Paseo plugin.
The native Claude `Explore` block seen in Paseo is a private provider-subagent
view: its compact, colored tool rows and nested expansion are styled by Paseo,
not by ACP. Cortex can supply typed action cards and a worker lifecycle card,
but cannot request that exact native layout through the installed generic ACP
adapter. The worker's message stream and published result appear in its own
card, and all emitted cards are replayed by `session/load`. The timeline row
subtly pulses only while the worker is working; it stops on a terminal status
and remains stopped after replay.
For Paseo 0.8 compatibility, the bounded, redacted worker mirror is also
sent as plain-text `rawInput` on the assignment card. Paseo's bridge drops
`content` and renders object input as JSON, so this preserves the final result
and the Workers-panel instruction without a separate output block; the
ordinary ACP `content` field remains available to clients that support it.
For choice questions, the optional native Paseo `cortex-acp` provider presents
marked Cortex permission requests as native question choices; see
[question handling](steering-and-questions.md#questions-structured-elicitation-with-a-text-fallback).

The coordinator is silent in the Paseo feed while delegated work is active:
procedural self-talk and intermediate narration are suppressed. After a worker
result is delivered and its report is read, the coordinator must still emit a
substantive answer; suppression does not discard the requested conclusion.
Accepted final text remains subject to the durable pipeline digest guard. Tool
lifecycle and worker cards continue independently of that prose. This
suppresses visible narration but does not refund model tokens
already generated before the handoff boundary.

Paseo renders ACP model names and descriptions as separate lines. Cortex keeps
the engine-provided display name as the primary label, with only `codex` or
`claude` as the grey provider label below. A new ACP session
refreshes each configured engine catalog in the background; failures leave the
last good selector intact. The literal `default` alias is hidden from model and
effort choices. Efforts are ordered `low`, `medium`, `high`, `xhigh`, `max`,
`ultra` where the selected model supports them. The default route policy
allows every model-supported coordinator effort while retaining separate
worker limits. Paseo ignores generic ACP options in category `other`, so Cortex
exposes coordinator language through Paseo's supported mode selector and maps
`session/set_mode` to the same durable language choice that Zed sets with
`session/set_config_option`.

Coordinator project and external MCP tool calls remain top-level cards. Their
titles are converted to human-readable operation names and their visible
inputs remain complete except for the ordinary credential/secret redaction
boundary. Worker actions stay in the assignment card or its exact
session/task-bound Workers history. Engine-private tool payloads and
unresolvable downstream terminal IDs are not presented as if they belonged to
the client's session. The engine may expose thought
chunks, but Cortex cannot reveal hidden reasoning that the engine never sent.
For an accepted task, the final prose is held until the durable decision is
known. Fingerprint-shaped strings in it must match SHA-256 values in the
current published pipeline edition; unmatched values are withheld and marked
unverified. This guard does not certify the rest of the model's prose.

InstructionBundle records the role, specialization, applicable project instructions, skill/reference catalogue and actual engine route. It retains source versions, the intended bundle hash and the per-session `session/new` metadata representation, including replacement sessions. The separate generation-delivery ledger tracks what the adapter sent; native Codex compaction persistence remains unqualified. Codex and Claude use their actual session adapters. Delivery records prove what Cortex sent to the adapter; they do not prove model compliance or complete control of automatically loaded engine instructions, settings and forwarded MCP servers.

SQLite holds the durable plan, attempts, event claims, delivery receipts and recovery state; process memory is a cache. A crashed check request left running becomes unknown until examined. An uncertain integration keeps its journal and source workspace. The project has one .cortex store, immutable worker reports and a pipeline.md whose newest edition is first. See [storage migration](format-migration.md), [workspaces and integration](workspaces-and-integration.md), and [publications and consistency](publications-and-consistency.md).

## Verification boundary

The offline suite checks dispatch, persistence, writer serialization, direct
project binding, instruction delivery, report templates and failure recovery
with fake engines. Snapshot/candidate tests remain migration coverage for old
task stores. The no-paid ACP smoke checks startup and protocol shape. Neither
establishes actual Zed or paid engine behavior. The [real-client runbook](real-client-runbook.md)
retains those scenarios as NOT RUN until observed on a disposable project
with isolated host state and separately authorized paid calls.
