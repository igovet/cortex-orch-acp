# Observability

Runtime events are retained in the project's [channel-specific state store](../../project/storage.md#where-the-database-lives-and-how-it-moves). and resource attribution

The Rust runtime retains lifecycle events, route choices, activity and usage in the project store. Workspace and Paseo expose bounded views of those records.

> The usage, command and reader descriptions below are linked to the Rust source. Client-specific claims in the activity projection section are only verified where a source or a versioned observation is identified.

## Structured events

The runtime records task state transitions and related events through [Core lifecycle](../../../crates/cortex-core/src/lifecycle.rs). Workspace uses [Core projections](../../../crates/cortex-core/src/projections.rs) and the [machine worker reader](../../../crates/workspace-service/src/machine/workers.rs). Paseo uses its [read-only worker-history command](../../../crates/cortex-paseo-tools/src/worker_history.rs).

The runtime records elapsed times in `observability_events`. The [timing reader](#timing-diagnostics) combines these records with exact intervals derived from retained lifecycle events.

### Bounded ACP event projections

Worker activity is retained in the durable task event history, while the ACP
timeline receives a bounded display projection. Worker tool cards omit
transcripts and raw inputs. Paseo cards carry status and title, the host's
bounded failure line, and only the open-issue count of a terminal result;
generic clients receive a rolling tail of at most 20 action lines and 4 KiB.
Runtime worker prose is buffered by message block instead of being emitted per
token.

In Paseo, the coordinator conversation keeps worker cards compact. The
runtime sends `cortexWorkerOutcome: { status, openIssues }` in a panel card's
`rawOutput`, which survives the installed bridge's dropped update metadata.
The provider reduces legacy terminal `{summary, checks}` to the same count
(open issues are checks that did not pass and that no exact later rerun closed), and its timeline
transformer reduces an item saved by an older plugin, which still carries the
full `cortexWorkerResult`, to the same count. A replayed session therefore
never brings the summary, check names, blockers or evidence gaps back into the
coordinator stream (`integrations/paseo/shared/worker-card-outcome.ts`). The
card shows the role, status (icon and text), task title, the actual
engine · model · effort route, on its own muted line how that route was chosen
(tier, hint status, `by …` and the chosen plan's usage as `plan usage N%`, from
the runtime's route-decision summary; the worker page shows the
same as "Route choice: …" from the route audit's recorded decision), at most one
fixed attention category (for example "Failure details in the worker tab";
never any part of a failure or cancellation reason), labelled context/token
usage ("Usage unavailable" when nothing was reported), and the whole card is
the one action that opens the worker; it has no visible **Open worker** label
(its accessible name still ends with "Open worker"), and a disabled card says
"Worker unavailable". While the worker is executing (the host's `running`
until the compact read lands, then an executing durable attempt status), only
its marker and status icon pulse: a slow CSS opacity animation on web, a
native-driver loop elsewhere, static under a reduced-motion preference and
stopped once the card settles or unmounts; the selected attempt's status icon
in the Workers tab does the same (`integrations/paseo/client/working-pulse.tsx`).
A hidden polite live region outside the card's button
announces each real status or issue-count transition once with the worker's
role and title; mounting or replaying a card, its first reconciling read and
usage polls announce nothing. Its only
store read is a compact history read (`compact: true`: status, route, usage and the
open-issue count, no summary, check list or events), polled while the host
shows the card running; an unchanged poll keeps the same state and does not
re-render the card. The durable attempt route from the dispatch audit takes
precedence over the static timeline title, including its recorded selection
provenance. A route-only update re-renders the card on the next poll (normally
within three seconds); before dispatch supplies a route, active cards say
"Route pending". Settled cards with no known route say "Route unavailable".
Older title-carried routes remain a fallback. The per-card result fetch is retired.

One worker is one card. A worker's ACP tool call outlives the coordinator turn
that created it, and the installed Paseo client (0.12.0) merges a `tool_call`
item only into a row of the same turn, so a Stop or a later turn's outcome used
to add a second, identical card. The plugin's provider therefore emits each
worker card as a provider plugin item `worker-row:<callId>` of plugin
`cortex-acp`, which the host merges by that identity across turns and replays,
replacing its data in place (`integrations/paseo/shared/worker-card-item.ts`).
Paseo 0.12.0 flushes its text row even for an update to an existing plugin item.
The provider therefore holds worker-card display events while a logical
coordinator message streams and releases them, in arrival order, at the next
message/timeline boundary, engine-message completion, turn end, completed load,
cancellation, error or close (`integrations/paseo/server/worker-card-order.ts`).
Engine-message completion is distinct from the outer ACP prompt, which can remain
open while workers run. After its final text/redactor tail (also on an engine
exception), the controller emits an empty `agent_message_chunk` with a unique
`__cortex_message_boundary__:` message id. Runtime text written while workers run
(a route notice, a worker question or other notice) has no engine turn after it:
the next scheduler pass emits the same marker after its notices and before its
worker cards, at most one per pass and only when text was written since the last
marker. The busy-steering acknowledgement is emitted before the controller's
text tracker exists, so the runtime closes that complete message immediately,
before entering the worker continuation. Otherwise later dependent worker starts
and earlier outcomes remain queued even though their ACP cards were emitted.
The installed SDK preserves this empty chunk; the provider consumes it without displaying an empty row, clears its
streaming-message state and releases queued cards. Later workers then appear
immediately instead of waiting for another coordinator message or the whole task.
The marker is retained in ordered display replay, not engine input. Old journals
without it still rely on the other boundaries above. Text is never buffered,
trimmed or reconstructed. Worker-card creation no longer rotates the runtime
message id; real coordinator tools still do. This applies to new live output and
ACP replay, not repair of old split identities or already persisted host rows.
Focused no-paid tests include token-sized text, card updates, multiple workers,
flush paths and the installed client's stream reducer; they are not rendered
Desktop evidence.
The client transformer keeps the same compact projection for `tool_call`
worker items an older plugin persisted; such a legacy item and a new plugin
item for the same worker in one session can still show twice. A cancelled
worker reads **Cancelled**, not Failed: every runtime cancellation path
(event claim, `/cortex-stop`, Paseo Stop and a stop racing a recorded
cancellation) sends ACP `failed` with the `· Cancelled` title marker, and the
card lets a settled durable `cancelled`/`interrupted` status override a host
`failed` and lets any settled durable status override the host's own turn-end
`canceled` guess. A durable `failed` keeps a genuine failure failed, even when it published a
report and an older host item says Completed. Every result-emission path reads
the committed node verdict, including worker-turn cleanup and stop races.
Replan/Stop cancellation projection first resolves any already committed worker
result using the exact assignment/attempt binding; process cleanup and its
retained event do not relabel that result. The provider preserves the compact
result against later same-ACP-status cleanup (`failed` also represents
cancellation), and carries its explicit `outcomeStatus` into the saved plugin
row. That result verdict overrides obsolete title markers with no extra read.
For older saved rows without a verdict, the existing compact durable read can
repair a Cancelled marker when a failed result and its check count are known;
absent or legacy count-only metadata alone never proves a failure.
Failed assignments keep their issue count; successful assignments with unresolved
checks read Completed with issues; clean success stays Completed. The compact
history reader and full worker tab use the same verdict. Session/load repairs
older false-completion ACP cards from persisted nodes; old saved plugin rows are
reconciled by the compact durable read. No result body or check list is added to
the coordinator. Scripted runtime/bridge/client regressions establish these
projections, not paid-model obedience or rendered Desktop behavior.

Each coloured text on the card (role, route and usage in `foregroundMuted`,
status and attention line in their status token) uses the theme's own token when it reaches 4.5:1 on every card
background (surface0/1/2: normal, focus, hover, pressed), else the least blend
toward `foreground` that does (`client/card-contrast.ts`). The card's neutral
1-unit border is identical in every state: there is no focus, selected,
pressed or status outline or perimeter, and on web the browser's own focus
ring is suppressed on the card (`cardFrameStyle` in
`client/worker-card-model.ts`). Keyboard focus instead underlines the
**Open worker** label in the action colour. The installed light theme's muted text (4.40:1 on
hover) and default dark theme's accent (about 2.5:1) are lifted this way; the
host theme itself is never changed. Space on
the focused card opens the worker on release: in a recorded Paseo 0.12.0
Desktop run a focused card ignored Space (Enter and click worked), and the
installed bundle binds a bare Space outside text fields to a window-level
voice-mute shortcut, so the card handles Space for its own element only, and a shared
activation guard keeps Enter, click and Space to one navigation per gesture
(`client/card-keyboard.ts`). Opening a worker from its card moves focus to the
worker heading in the Workers tab once the attempt resolves.

The opened Workers tab reads the selected attempt's full history and shows its
terminal **Result** (the attempt's own retained runtime summary, else its own
latest `worker_result`, scrubbed and clipped at 16,000 characters with a
visible note) and **Checks** (each with its disposition, resolved history,
weak-evidence marker, blocker and evidence gap) above the retained activity,
with the existing bounded paging. When no runtime node row exists for the
attempt (an older store), the checks come from that attempt's own latest
`worker_result`, validated and scrubbed; a runtime row's checks are never
replaced. Active attempts and attempts that ended without a result show no
terminal sections; absent check data reads "Check details unavailable", a
known empty list "No checks reported". A rejected or unavailable history read
keeps the selection and any retained history, shows a safe reason (marking
retained content as possibly out of date) and a **Retry** action; it never
reads as an empty list or a failed worker. A worker opened from a failed card
whose history has no attempt shows the card's host-provided reason there.
An Activity row of that attempt's own `worker_result` whose text equals the
shown Result summary (whitespace aside) is not repeated below it; differing
result text, the final response and other prose stay, and stored history is
unchanged (`withoutShownResult` in `client/worker-panel-events.ts`).

The Paseo worker-history reader can show a retained assignment brief when the saved record belongs to the selected attempt. If there is no retained match, the reader cannot establish that a brief was sent. See the [Paseo history reader](../../../crates/cortex-paseo-tools/src/worker_history.rs) and [native worker admission](../../../crates/workspace-service/src/native_runtime.rs).

Coverage of the earlier card and tab behavior is
no-paid provider, history, model and client-bundle render-walk tests plus a
react-native-web fixture rendering; it is not Paseo Desktop evidence.

Each worker card also has a fixed event budget, because Paseo keeps every
update however small. Status transitions move forward only (create as
`pending`, start as `in_progress`, then `completed` or `failed`). Each is sent
at once, and a stale earlier status never moves a card back. Any other change,
such as a title or generic-client body, is a progress update. The first
progress update is sent at once. Later ones are sent at most once per 30
seconds and at most 8 times per card. A settled card may be refined at most
twice more, for example with a later outcome label or result, and these
refinements are not delayed. A card therefore sends at most 13 updates,
whatever its tool count, output or duration. A withheld change is not lost:
the latest state is recomputed on every call and sent with the next permitted
update, and always with the terminal one. Every worker-card title passes
through one normalizer (300 characters, outcome suffix kept), so two code
paths cannot send two spellings of the same title. A live qualification run found that alternating clipped and unclipped copies of one
long title caused 296 worker-card updates for three workers, against 9 on
the baseline. On Paseo, whose cards carry no body, a worker sends exactly its
three lifecycle updates. Workers' engine tool calls and context/usage updates
are recorded only in durable task history for the Workers panel. They are
never relayed to the host as separate events.

Coordinator tool-call fields such as raw input, raw output, locations and
content blocks are bounded to 16 KiB each. A serialized `session/update` has
a 32 KiB hard cap. Oversized values are shortened with explicit truncation
markers, leaving the full record in task history. These limits bound individual
host events and avoid resending cumulative worker transcripts; total durable
history still grows with the work performed.

Coordinator and engine tool cards are emitted as partial updates: unchanged
title, input and output fields are omitted, and a no-op update is suppressed.
When an MCP result's text is exactly a JSON mirror of its `structuredContent`,
the text copy is omitted because Paseo presents the structured value first;
non-mirror text is retained. If a completed call repeats an earlier call's
title, input and output exactly, the emitter sends a small
`repeated_output: true` reference with `same_as` pointing to the first card
instead of copying the output. The repeat cache retains at most 64
(title, input, bounded output) digest keys. A compact unchanged-context receipt
does not displace an earlier matching display projection: full contexts can
change only in fields that the display truncates. Each repeated projection
still carries its call's terminal status and references the earlier card;
changed projections and failed calls retain their bounded output. This cache
affects only the host display, not the complete authoritative MCP response or
durable history. A cache miss or eviction sends the full bounded result again.

The number of coordinator and engine tool cards is bounded per task, not only
per card (`wire_budget.COORDINATOR_*`). The first 150 distinct calls of a task
get their own card, unless those cards have already sent 4 MiB of serialized
updates. Each card sends at most 10 updates, create included. The last slot is
kept for its first terminal update, which also carries any change held back
past the cap. Later calls are not sent as separate cards. They are counted on
one aggregate card, `call:cortex-folded:<task>`, titled "N further tool calls
not shown individually". It is created, as `completed`, at the first folded
call. Updates follow at most once per 30 seconds, and the interval doubles
after each update. The count is always refreshed when a coordinator turn ends.
The card sends at most 20 updates per task, creates included, and its last
update says the count is no longer updated. A call that fails after the budget
still gets its own card, carrying its error, for up to 50 such failures.
Later failures are counted on the aggregate card. Permission requests and user
questions are `session/request_permission` host requests, not tool cards, so
the budget never folds or drops them.

The budget belongs to the task, not to an emitter, so it survives coordinator
turns, the per-prompt emitter, a reopened coordinator session and a Cortex
restart. A new runtime seeds the budget from the task's durable host journal
with a SQLite substring prefilter. Its bookkeeping is bounded: open-card
digests and per-call dispositions are 1,024-entry LRU maps. An evicted call
that later receives an update is treated as a new call and spends budget like
one. `session/load` applies the same bound. A journal written before the budget
existed replays the first 200 coordinator cards as recorded and counts the rest
on one aggregate card. That card's `_meta.cortexReplay` names no source
sequence and gives `foldedCards`. Durable task history (task events, the Workers
panel and reports) keeps every call. Only the live and replayed host stream is
bounded.

The regression scale test drives the real
Agent over a pipe transport with a Paseo client, the fake engine and
300-character worker titles. It records each repeated `task_get_context` poll
at 481–483 bytes after the first read. Each worker card sends 3 updates and
16,959 bytes for 10, 60 and 300 tools per worker, including with 20 KB
writes, and for a 60-tool worker on a simulated clock that advances 20 s per
card decision. The largest observed update was 16,108 bytes. An earlier run
of this fixture at 10, 200 and 1,000 tools, with the title normalizer and
event budget removed, sent about 60, 1,100 and 5,500 updates per worker. The
same file drives a coordinator making 10, 500 and 1,500 distinct calls over
four prompts: a question, its continuation, a follow-up turn and a follow-up
after a runtime restart with `session/load`. Card updates were 20, 312 and
324, and card bytes 11,380, 185,337 and 187,517. The aggregate card sent 8
and 10 updates, and the bookkeeping maps stayed at or under 1,024 entries;
the 1,500-call case fills them. An earlier 5,000-call run with the budget
disabled sent 9,563 card updates and 5,846,291 bytes. A journal holding 501
legacy cards replayed as 403 updates. These are test workload results, not a bound on the
whole daemon's memory or on durable history. Paseo 0.9.2 retains provider
events in a session's in-memory history until the session closes. Smaller and
non-repeated events reduce the bytes Cortex contributes to that retained
history. Tool cards are now bounded per task, but coordinator prose, relayed
thought and plan updates, and usage updates still grow with the number of turns
and the amount of text. None of this makes total host memory constant for other
session data.
Usage and thought updates can arrive between text chunks of one sentence;
they do not insert paragraph breaks into worker prose. Tool calls remain
observable message boundaries. Paseo compares the terminal response with complete
streamed prose blocks, allowing whitespace trimmed only at block boundaries;
it suppresses repeated text while preserving paragraph breaks inside the stream
and any new final content on both live display and history replay.
Meaningful Cortex MCP, native, external MCP and private-store operations remain
visible with readable action names. File and skill reads are static target/status
rows; their result bodies are excluded from new display history, including worker
activity, and projected out of old replay. The engine still receives the read result.
Raw data is never displayed. Other calls expand only for readable output, diffs or, for a shell call, its command; a shell call titled with its own command line is never treated as a read. Thinking and exact protocol
bookkeeping operations (including wait) are hidden by a shared presentation policy
in both the main chat and worker panels, including old replayed history. Other input/output and display updates are retained subject to
secret redaction. Missing, malformed or unavailable evidence is never converted
into success. The runtime rewrite and its qualification gaps are tracked in
[verification status](../../project/verification.md).

Paseo's worker history renders a retained native edit as a line diff only when
the durable ACP event contains an exact file path plus both `oldText` and
`newText` values (including new-file and delete-file null sides). It shows the
path above the numbered context/add/delete rows. A pending edit envelope with
no retained text stays a compact `Edit · Preparing file…` row and never turns
empty `{}`/`[]` transport placeholders into a raw JSON detail view; the panel
does not infer a diff from the current file or from a status-only event.

## 2. Usage/cost attribution

The Rust session driver records one immutable `usage_records` row per sent
task-bound ACP `session/prompt`, on a best-effort basis in the [native driver](../../../crates/workspace-service/src/native_driver.rs). The row names the
task, exact attempt and engine session, and its one-based sequence among
dispatched prompts on that session. Continuations and retries are separate
calls; neither launch nor undispatched requests are calls. Workspace resolves
engine and model from `NATIVE_SESSION_CONFIGURED` and correlates usage with
`NATIVE_TURN_SETTLED` without double-counting a settled refusal.

Only validated, recognized counters in the exact final `PromptResponse.usage`
are stored as `engine_reported`. If the adapter omits its total, the stored
total sums the recognized counters it provided. Cost is not collected, and context occupancy
does not stand in for consumption. Missing or invalid reports remain
`unknown`, with empty `tokens_json`. Losing the response after an accepted
worker result can leave token usage unknown without undoing that result.
See [coverage and interrupted turns](../token-and-plan-usage/index.md#coverage-and-interrupted-turns).

The older Paseo reader still omits legacy non-failed `released_after_result`
rows from known/unknown counts; the Rust writer does not generate that marker.
The existing table and its closed basis vocabulary are unchanged. No old rows
are rewritten.

Automatic worker routing writes its full choice into the existing
`attempt_route_audit.decision_detail_json` field. The `route_decision` object
identifies its ranking algorithm with `rank_version` and records model-hint
status/reason, quota state and plan usage (`usage`; rows before rank
version 15 also carry per-candidate `usage_band`), engine affinity,
`decided_by`, policy exclusions, and effort basis. Rows written before rank version 4 may also carry count-based
`balance` and `cross_tier` fields; they are kept and read, but no longer
written. These are decision inputs and outcomes,
not usage measurements. Unknown quota is retained as unknown, never reported as
zero usage. The recovery capsule exposes this detail only when the exact
attempt's audit row is available. Two optional fields are added to that detail. A pinned
effort that the route's policy does not permit is applied at the nearest permitted step, not
refused, and `route_decision.pin_effort_clamped` then records `configured` and `applied`.
A fallback hop's audit detail carries `failure_class`, the classified failure that triggered
the hop (from its `fallback_failure_class`), so a retry or fallback can be traced to its
cause.

Engine and model are not duplicated on each row. Workspace uses the exact
configured session reference for new rows; the older readers retain their
historical attribution rules. Route audit values are not token measurements.

## Redaction

Engine message streaming and launch error handling live in the [native session driver](../../../crates/workspace-service/src/native_sessions.rs). Paseo applies its own [display scrubber](../../../crates/cortex-paseo-tools/src/scrub.rs) when reading worker history. Treat retained evidence as private. The [external MCP broker](../../../crates/cortex-core/src/external.rs) records content-bearing results in the runtime event and request ledger.

## Paseo invoke latency and request coalescing

The plugin's GUI RPC handlers that previously used synchronous child-process calls now use asynchronous `execFile` with bounded concurrency. Worker-history identities are cached, and overlapping worker-history and pipeline poll requests are coalesced. `cortex-acp.invoke` timeout diagnostics identify the RPC method so a slow history read can be distinguished from other handlers. These measures remove the observed event-loop serialization cause and reduce duplicate work; they do not by themselves establish a timeout-free workload or a rendered GUI result. Use the plugin typecheck and unit-test commands in [verification guidance](../../project/verification.md), and inspect the actual Electron timeline for GUI claims.

## Timing diagnostics

Run `cortex-paseo-tools task-timings /absolute/project EXECUTION_ID` for the
retained execution's state, elapsed milliseconds by category and the source
intervals. Append `--dev` to select the separate Dev store. The command opens
one store read-only and returns a missing-execution error if the requested
execution is absent. Each interval carries its time basis and, where recorded,
the exact attempt, assignment, node and plan revision. The
review-cycle counts use the first review binding per node and revision, with
the assignment as the identity for older bindings. A binding without a
retained risk is counted as `unobserved`.

The runtime writes observed durations to the append-only
`observability_events` table. The payload contains a millisecond count.
Categories cover engine-session startup (each try), time parked in the local
capacity queue, preparation of an attempt's first worker prompt, sent
planning, worker, review and reconciliation prompts, and verification work
after any retry delay expires. Each elapsed measurement uses a monotonic
clock. The timestamp on an observed row is its recording time. An outcome
of `returned` describes a runtime operation returning. The lifecycle
events carry the verdict of a check, review or execution. A dropped future,
process exit or failed metrics write may leave an observation absent.

Derived intervals use retained wall-clock endpoints. They cover time spent in
each task phase, the local slot wait, the exact worker attempt from first
delivery to result, the final review binding to the review result, the audited
route selection to first delivery, and time from a completed graph
prerequisite to the dependent attempt's first delivery. A graph handoff
interval measures the recorded dependency boundary and elapsed dispatch
time. The receiving agent's report-reading evidence remains in its own history.
Unmatched, malformed or backwards time boundaries provide no interval.
Overlapping categories, concurrent nodes and multiple prerequisite edges can
cover the same elapsed time. Read each total on its own.

The scheduler's bounded history reader samples successful worker deliveries and
results by exact attempt and route audit, using the recorded role, work kind
and complexity. Older rows without those facts remain usable for any task
intervals their endpoints establish and do not enter the estimate. When
retention deletes an execution, its observed timings, runtime events and route
audit are deleted together. Duration estimates therefore use retained
executions only. See the [timing writer and reader](../../../crates/cortex-core/src/observability.rs)
and [retention policy](../../project/storage.md#automatic-workspace-retention).

## Removed Python-era readers

The bound inspector, phase timeline and developer wire trace were removed with
the former runtime on 2026-11-16. The read-only CLI provides
`project-root`, `task-id`, `worker-history`, `worker-results`,
`running-commands`, `pipeline` and `task-timings`.
See the [reader command registry](../../../crates/cortex-paseo-tools/src/lib.rs).
