# Consistency and recovery

**Status: planned rules. The local desktop slice implements the rules below for its subset, with no-paid checks; no recovery claim is made beyond those checks.**

The authoritative service coordinates clients; local caches are not acceptance evidence. Commit runtime-owned state and corresponding public event atomically where supported. Use snapshot plus cursor, bounded replay and resnapshot on a gap. Reordered or duplicate events must not produce duplicate effects. Reject stale entity versions, old owner epochs and revoked device identities.

After service crash, reconnect and reconcile actual child and external state; do not replay prompts, terminal commands or ambiguous Git pushes. An unconfirmed external outcome remains unknown until independently inspected. Lost connections display age of last confirmed state. A changed result digest invalidates prior acceptance. Device races and crash points require executable tests, not UI-only assurances.

The service supervises each activated project's child beside its socket, so startup and a slow handshake never delay `service.status`. A child that exits or breaks the protocol is replaced with backoff; a registered folder that is away (a disk not mounted yet, a deleted folder) leaves that project waiting with its reason and comes back when the folder does, and it never stops the service. A child that answers late is not abandoned: that one request stays unknown, the late reply is dropped, and the journal poller resnapshots when the child answers. Nothing here replays a command or a prompt.

## Worktree Start and cleanup

Start records an intent before creating the checkout. Exact retries use the
recorded outcome; a changed payload conflicts. Once Git creation is verified,
Core commits the checkout binding with execution admission and the public event.
If Git may have run but the service cannot prove what it created, recovery keeps
the outcome uncertain; it does not recreate, adopt by name or remove a folder on
a guess. If a bound checkout later vanishes or no longer matches, execution and
checkout operations refuse rather than using main. Close, merge and purge record
cleanup separately: an unsafe or unconfirmed removal does not undo their outcome.

## Startup and project read isolation

The native bridge reloads the fixed channel unit, requests a nonblocking start
and waits up to 30 seconds for its authenticated local socket. That readiness
budget is separate from an action's request deadline (reads have none): no action bytes are sent until
connection succeeds. The first subscription reports connecting during its startup
grace period rather than a spurious offline state. Failure beyond that budget
remains visible and retries automatically; a real loss after connection is still
reported. This is a bounded readiness policy, not a promise of instantaneous
startup on an unavailable machine.

When a subscription that was connected ends, the bridge resubscribes and resyncs
silently and shows the machine offline only if that does not succeed within five
seconds; a subscription that keeps ending at once does not restart that grace.
Every end is written as one bounded line on the desktop's stderr
(`cortex-workspace-desktop: subscription ended reason=<code> after_ms=<n>`, one
line per distinct reason during a long outage), naming a fixed code such as
`service_disconnected:unexpectedeof` or `service_invalid_event:unknown_event`
and never frame content. A `native_delta` the window cannot validate is not a
connection problem: the bridge logs `event_refused reason=<code>` and turns it
into a `native_resync` of that project, which re-reads its authoritative state;
only a frame that names no valid project ends the subscription. The status
check takes the machine offline after two failed checks in a row while it was
connected (one, when it was not), so one lost answer is followed by the next
poll three seconds later.

The service serves at most 64 socket connections at once. A later caller waits
in the listen backlog until one ends instead of being accepted and dropped,
which a client read as a lost service. The desktop keeps at most 48 ordinary
requests in flight; the status check and the subscription's own reads never
wait for that bound, so the window's slow reads cannot crowd them out. A request
that waits longer than its deadline for one of those 48 is refused as busy
with nothing sent.

A subscription refresh reads the registry and invalidates project caches without
waiting for per-project controller RPCs. The service's existing independent
journal polling owns child resynchronization. This avoids accumulating one
five-second wait per activated project while the event socket goes undrained.

Registry identity is installed independently of task or product reads. In registry
mode, each project reads concurrently; the presentation waits at most 100 ms per
snapshot/facts stage before returning completed projects. Pending projects refresh
silently; normal startup is not a read failure. Three consecutive failed reads
produce a project-local issue, with automatic retries every three seconds. The
registered row and last confirmed records remain intact. It never turns another
project's data into a machine disconnect. Task snapshots are paired with owner
facts before publication so planning/purged rows do not leak onto the board.
Deferred results predating an owner command request a new read, not a replay of
the action. Existing schema, identity, generation and version checks remain.

The pending-read status stays out of layout flow and cannot intercept pointer input.
Finishing background reconciliation therefore does not move project or task controls;
read failures and connection failures retain their separate visible notices.

These behaviors have disposable fake-service/store tests; they do not by
themselves qualify a freshly installed or upgraded desktop archive.

## Confirmed board on launch

The desktop keeps a bounded display-only board snapshot in its channel's WebKit
profile. It contains validated task DTOs, selected owner titles/dependencies and
sprint membership, not credentials, capabilities, authoritative command-version
state, result acceptance, Git contents or conversation history. A cold window renders that
snapshot before waiting for the service, then reconciles registered identity
and live state. Corrupt, oversized, wrong-actor or incomplete snapshots are ignored;
removed projects and changed roots are discarded when the registry answers.
Storage denial or quota exhaustion falls back to live reads. A fresh profile with
no prior snapshot must obtain real data; no rows or success states are invented.

With `native.board`, one bounded `tasks.board` request reads up to 1,000 tasks in
one SQLite snapshot (768 KiB reply ceiling). Saved owner overlays and sprint
membership use the native `product.board` reader. Conversations, decisions,
results, workers and repository details enrich that board separately and cannot
hold task placement behind full conversation replay. Generation/watermark and
owner-command fences discard obsolete enrichment. A details read overtaken by
newer stream events (for example a Working task's activity) is not a failure: it
is discarded and reread against the newer snapshot. Persistent detail failures
remain visible and retry automatically without clearing the board.
A stream event, refresh or owner input marks a task's conversation for reread
without discarding it: the board update that precedes the details read keeps the
rows last confirmed, so a running task's page never empties and redraws between
the two.

Conversations are read per task, outside the details read and its watermark
fence. A conversation belongs to its execution, not to a board version, so a
moving watermark never discards it. Formerly every task's complete history had
to be read in one details read that no stream event could overtake. Under a
running task, or after a service restart left recovered runs working, that read
was overtaken every time, and the conversation never appeared. The page showed
the task description and the Working line only. Now each task's read is
published as soon as it finishes: never-read conversations first, then the most
recently active. Only a conversation whose task is on view (its page, or its
board preview) is read in the background; another is read when it is opened. A
window with no screen declaring what it shows (a headless store) reads every
conversation.

Streamed message chunks are display rows that the runtime does not commit one by
one. Each chunk's row waits in memory as a deferred write
(`StoreExecutor::write_deferred` in `crates/cortex-core/src/store/executor.rs`) and
commits in one transaction with the other waiting display rows within 250 ms, at
once when 64 are waiting, before any later ordinary write (a tool row, an owner
message, the turn's settle event, any runtime state change) and when the service
shuts down. Rows keep one row per chunk, the same seq order and the same message
identity a chunk committed at once would get, because each deferred row runs the
same statement in submission order; only the commit is shared. A streaming turn
therefore needs one commit per 250 ms window instead of one per chunk. The loss
bound applies to display text only: a service killed (not stopped) inside that
window loses the waiting chunks of the last 250 ms, never a tool row, a result, a
lifecycle change or anything another write depends on. A waiting row that fails
(for example a replaced owner's channel) is rolled back alone and never reported.

After its first complete read a conversation is only continued, never reread.
Each history page names a forward cursor (`tailCursor`, see the
[protocol](protocol.md#native-desktop-read-api)), and the task's stream events
(each written row moves the task's version) start a read of the rows written
after it. One read per project is in flight; events that arrive during it
start the next one right after it ends, no sooner than 120 ms after the previous
one began. New rows are appended to the rows held: a row already held (the same
row identity) is never added twice, and a read whose held rows were replaced by
a complete read meanwhile is dropped. Streamed chunks of one message join the
message already on screen, which keeps its identity, so its text grows in place;
a tool call's later update merges into its existing step. The forward cursor is
plain data (session binding, position and a check of the row at that position),
so it survives a service restart: the next read continues from it. When the
service refuses it, because the session's rows at or before the position were
removed or written again (`history_reset`), or the execution is bound to another
session (`invalid_cursor`), the window reads the whole conversation once and
replaces the held rows. Display rows are only appended (context compaction does
not rewrite them), so this happens only after retention removes a session or the
store is replaced. A service without forward cursors keeps the former behavior:
the whole conversation is reread after a short pause. Owner sends still reconcile
by request identity or text against the conversation (see `sends.ts`); question
cards and their answered state come from the task's decisions, not its history.
A conversation that is no longer on view keeps its rows and forward cursor and
continues from them when it is opened again.

A conversation read afresh (opened for the first time in this window, another
execution, or after `history_reset`) starts at its end. The window reads it
backwards with `history.earlier` (see the
[protocol](protocol.md#native-desktop-read-api)) until it holds about one screen
of the conversation: the entries the feed would show for the rows read, against
the number a screen of the window's height holds (an estimate of 40 px per entry,
between 12 and 60 entries). It stops there, once 150 rows are held, after four
pages, or at the first row. The newest page's `tailCursor` continues it forward
exactly as after a complete read. A 22,000-row conversation therefore shows its
latest part after one or two requests instead of 220; the rest is read as the
owner scrolls up to it.
While earlier rows remain, the boundary below the brief says "Earlier conversation
is not shown." with **Show earlier** as a fallback. The visible feed automatically
reads earlier parts on opening, resizing or scrolling if the boundary has not
passed at least one viewport height above the visible area. It does this even
while following the latest messages, one earlier read at a time, and checks the
boundary again after each prepend. Already held rows keep their identities and
reading position; a short partial feed aligns its tail at the bottom so a
prepend can use the spare space above it.
"Loading earlier…" remains until bounded pages yield visible conversation
content or reach the beginning. Empty pages, hidden tool folds and incomplete
leading messages do not end the load while an older cursor remains. A failed
read retains the current rows and offers **Retry**, without automatically
repeating the request. A refused position (`history_reset` or `invalid_cursor`)
refreshes the latest slice; the same unusable position still waits for **Retry**. Nothing derived from
a conversation read only in part speaks for the part not read. Its first message
or turn of steps may have begun before the rows held, so it is left out until
they are read; a cut message or a "Worked …" line counting only part of a turn is
never shown. A settled question asked before the first held row is not placed at
the top of the held rows, while an open one always shows. An answer given before
that row is not matched against the held owner messages, so a later message of
the same words still shows. An owner message sent before it is not appended at
the end as if just said. A send in progress is matched only against lines after
the one that was last when it was sent. Rows read above the screen are not
counted as new by the follow pill and do not pull the feed to the end. A service
that refuses `history.earlier` (`invalid_request` or `unsupported_operation`)
is read from the beginning as before. Other readers (the ACP host, the Paseo
tools, the wire-1 `history.page` query and a window without `native.query`)
read forward as before.
The display is the same in every execution state (running, paused, recovering,
waiting or ended) and does not need a live engine session: the service serves it
from the retained store rows. A conversation opened cold in a window (its first
read for that machine, project, task and execution) shows its skeleton until
every read its first screen needs has settled: that latest window of the
transcript (or that it is empty), the questions of its execution, and the first
facts read naming the task (its workers, messages and result). Then all of it
appears in one change, at its end, following. A question that arrives before
the transcript, or the reverse, is never shown on its own, and an empty list
before its read is not taken for "none". The questions of the task on view are
read on their own as soon as its identity is confirmed; the board's read of
every task's questions waits for that read (at most 5 s, `FIRST_VIEW_MS`) and
takes the task on view first. A slow first read is waited for, however long it
takes: the skeleton stays and nothing says it failed. Only a genuine failure (a
refused or unreadable read) or the machine going offline stops it: what was
confirmed is shown with one notice naming what is not ("Couldn’t finish loading
the conversation.") and **Retry**, and a later arrival clears it.
A view already shown in this window (a parked tab, a reconnect, a reread of the
same execution, a task the owner just started from its page) keeps what it
shows, its draft, focus and position, with no skeleton. Git settlement, older
history, report bodies and other tasks are never waited for. Until the first
transcript read finishes, the conversation says it is loading. A conversation page has no deadline: a slow page is
waited for. A read that genuinely fails (refused, unreadable, disconnected) shows its reason on that task with **Retry now**. Earlier confirmed rows stay
shown, and the read is retried automatically after 3 s, doubling up to 30 s. A
conversation failure is not a project read issue. The Working line comes from
the task's runtime state and running workers, never from missing history.
Queue status belongs to the service: incomplete window data never means the
queue paused. Missing-task warnings on both the board and sprint summary require a persistent project read issue.
The membership fence survives a rejected first board, not only an outstanding
promise: explicit facts reads and polls cannot publish a sprint before a successful
complete board. Membership is checked against the last successful presentation,
not an unpublished raw task read from a failed refresh. A poll observing newly added members requests a coherent reread
instead of publishing those IDs alone. Starting/recovering status without a failure
reason is not a project issue. The native reader retries a concurrent
`snapshot_expired` race at most three times within the same read; policy/storage
failures are not swallowed. These are reads only, never action retries.

## Slow reads are waited for

A read never fails for being slow, whatever it reads (the store, product records,
Git, files). The service runs every owner read (`machine::owner_read`, the same set
as `workspace_protocol::read::owner_read_op`: `product.read`, `product.facts`,
`product.board`, `workers` including coordinator history pages, `repo.snapshot`,
`repo.context`, `repo.file`, `repo.list`, `repo.locate`, `repo.skills`,
`repo.file.lease.status`, `command.output`, `overview`, `config.read`,
`project.config.read` and the sprint `live_graph`) until it completes: it has no
30-second helper deadline, never answers `machine_busy` while it waits for a lane
slot, and its Git commands have no 25-second `git_timeout`. Concurrency stays
bounded: saved-facts reads queue for their project's two `PROJECT_READS` slots,
and the other reads for their fixed lanes (reads, settings, repository
inspections), so a slow disk holds a bounded number of threads and never an
action's slot. Actions (writes, Git and engine actions) keep their deadlines.
The clients (`workspace_protocol::read::read_request`: board and history
projections, workspace queries, the project and owner-prompt lists and every owner
`machine.call` read) wait for such an answer without a request deadline; only a
lost connection (`service_disconnected`) fails it. The interface treats a timeout
or busy answer from an older service as still loading, never as an error.

## Native desktop reads

The service lazily retains one Rust Core read provider per logical project/channel.
A SQLite read-only connection sees committed WAL; no initialization, schema repair,
migration or writer hooks run. Every reply reads facts and its watermark in one
transaction. Task-page cursors bind generation, project, version, watermark and
page size; a concurrent write expires continuation with `snapshot_expired` rather
than mixing pages. These are expiring keyset pages, not a retained immutable
30-second projection-root implementation.

The service tails committed public outbox metadata every 250 ms for opened readers
and publishes selected task DTOs through Core's bounded, generation-local event
stream. The existing socket subscription forwards validated `native_delta` and
`native_resync` messages. Contiguous small task changes update the window cache;
only changed tasks reread related history/decisions/results, not the task list or
machine facts. History for a changed task is still read in bounded complete pages,
not append-only replication. Gaps, retention loss, oversized batches, changed
generations or conflicting entity bases request resnapshot. A native patch that
arrives during a coherent presentation read is queued and installed by version
after that read, preserving cached Git and owner overlays. Neither a read nor a
patch grants process, review or completion authority.

### Reads follow what is on screen

A window polls no project. A screen on view holds interest in what it shows: a
board, sprint or task page in its project's facts, a Git, worktrees or Code
page, a task's Changes or Code tab and the merge dialog in that project's Git
data too. Opening such a screen reads it once; its events, the owner's own
actions and a return to the window after a while read it again; a Git screen on
view and focused is also refreshed every 30 s, because files change outside the
app without any event, and its **Refresh Git status** control reads it at once.
Worker rows are read only for the task on view. A project no screen shows is
never read in the background: its board, sidebar row, badges, Inbox and question
notifications follow its runtime events. Scoped streaming facts and periodic Git
refreshes pause while the window is hidden. Valid native deltas can still apply,
and generic refresh notifications can still trigger a read, with a 15 s
coalescing gap while hidden. Returning to the window refreshes visible data
according to the 10 s freshness cutoff.

A display-only board restored from storage is labeled **Saved view; checking
live state** until the matching live project presentation replaces it. If live
state cannot be reached, the notice identifies the view as saved and the
existing failure notice still applies. A saved view does not confirm an action
or supply a command version.

Questions and results read by a details read are kept even when a newer
snapshot supersedes that read: they are at least as new as the snapshot they
were read for. A running task moves the snapshot all the time, so discarding
them left an answered question showing **Needs you** on the board, in the
sidebar and in the Inbox for as long as the task kept working. When only the
snapshot's watermark moved (the same generation), they are also shown at once,
with the board as it is now and at its current version, and another details
read follows; they no longer wait for a details read that no newer event
overtakes. A recorded
answer also marks its questions answered at once in the window, so the reread
that follows the answer shows the task working.

The service's sprint loop is driven the same way: it looks at a project when it
starts or activates, when an owner or loop change of a sprint or task record
could move a sprint on, and on that project's runtime task events while one of
its sprints is in motion (a queue running, planning, or a result waiting for an
automatic settle), with a 30 s safety tick only for such a project. A project
with no sprint in motion costs nothing between changes. Engine limits are
refreshed only when a settings read asks for them.

## Local desktop slice (built)

The adapter appends each public invalidation in the same SQLite transaction as the private write that caused it. Connection-local TEMP triggers on the adapter's source tables call one append helper on each supported Store connection, so a failed append rolls back the state change, including in autocommit. The append is coalesced per transaction: the first write in a transaction that touches a bound task or session bumps the execution, task and project versions once and appends one `task` and one `execution` event, and later writes to the same identity in the same transaction add nothing (a per-connection transaction epoch, advanced by the commit and rollback hooks, stamps a TEMP mark row that a statement or savepoint rollback also undoes). Readers only see committed state, so every commit that changes a bound identity still produces at least one event with a higher version, in sequence; a write of N rows costs one append instead of N. No persistent trigger requires independent or older SQLite writers to load Workspace code or retain its optional tables. Those external writes are outside the notification guarantee; the hooks are not a security boundary. Normal writable initialization removes the legacy persistent hooks in its schema transaction; non-initializing inspection never repairs the durable schema. Public project sequences are allocated only from the adapter's own outbox state, never from runtime event or transcript identities. Reconnecting clients receive a snapshot and then events after the cursor. A gap, a wrong project or a backwards version resynchronizes the client.

A task start is recorded as prepared, claimed, dispatched or unknown before the prompt pipeline runs. A restart after dispatch leaves the execution unknown and never replays the prompt. Answers to a single question or a complete batch are all-or-nothing. Only the winning request runs the existing continuation; if that handoff fails, the answer reports `answer_continuation_unconfirmed` rather than success.

The service owns the ACP child. Closing a client detaches its subscription and leaves the child running; explicit stop cancels the owned controller, closes the child's input, waits a bounded grace period and then kills only its own child. A restarted service creates fresh controllers and snapshots without replaying any start or prompt. Test evidence and its limits are recorded in [verification](../project/verification.md).

## Attachment files

Owner attachment bodies live once each in `<state>/attachments/<sha256>` (directory
0700, files 0600, named by the SHA-256 of the decoded bytes), beside the private
product state. A body is written to a private temporary file, synced, renamed and the
directory synced; an existing body of the same digest is kept and its time renewed.
The product state, every command and every reply hold only references
`{name, mimeType, size, sha256}`. A reference is accepted only when its body is
saved with that size; a read checks the body against its digest.

A product file saved by an earlier version with inline bodies is converted when it
is first loaded: the bodies are written first, then, only if the machine lock is free
at that moment, the file is re-read under the lock, converted and atomically
replaced. A crash between the two steps leaves the inline file, which the next load
converts again; a body that does not decode is left inline as it was. Converting is
idempotent, and a load that cannot write a body keeps the inline value it read. An
earlier service version reading a converted file does not fail to load it: its
replies list the references, its window shows each file as a name it cannot open,
its planning prompts leave those files out, and re-sending a saved file from it asks
to choose the file again. Its sprint note and patch actions refuse with
`invalid_attachments` on a sprint that has saved files, because they check every
saved file of the sprint as inline bytes.

Bodies are removed only under the machine lock, after a save that dropped a
reference (a message, comment or sprint removed or edited, an older copy given up),
after a sprint is deleted, on cleanup, and at most once an hour after an upload. A
removal pass reads every project's product state and keeps any body one of them
references; an unreadable product state stops the pass before anything is deleted.
A body no saved record references yet, such as a file attached to a draft that was
not sent, is kept for 24 hours after its upload or last renewal. A task's creation
and message files are also kept by Core in its own private store, so removing a
product-state body never touches what a task already received.

Files attached to an answer (a coordinator question, every member of a batch and a
sprint-planning question) are kept exactly like message files. Once an answer is
recorded, the product state references its files under `answerFiles.<task>`, taken
from Core's own record of the answer's stored files (each with its exact digest), so
the settled question card's chips open for as long as the task exists, archived
included. Deleting or purging the task, alone or with its sprint, removes those
references with its messages, and the bodies go once nothing else references them.
An answer recorded before this was kept is referenced on the next product read
(or cleanup) when its body is still saved here at its recorded size; a body already
removed stays unavailable and its chip says so. Nothing is attributed by guess: the
reference only keeps a body, and which question shows a file still follows
`answerAttachments`.
