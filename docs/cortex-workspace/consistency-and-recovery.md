# Consistency and recovery

**Status: planned rules. The local desktop slice implements the rules below for its subset, with no-paid checks; no recovery claim is made beyond those checks.**

The authoritative service coordinates clients; local caches are not acceptance evidence. Commit runtime-owned state and corresponding public event atomically where supported. Use snapshot plus cursor, bounded replay and resnapshot on a gap. Reordered or duplicate events must not produce duplicate effects. Reject stale entity versions, old owner epochs and revoked device identities.

After service crash, reconnect and reconcile actual child and external state; do not replay prompts, terminal commands or ambiguous Git pushes. An unconfirmed external outcome remains unknown until independently inspected. Lost connections display age of last confirmed state. A changed result digest invalidates prior acceptance. Device races and crash points require executable tests, not UI-only assurances.

The service supervises each activated project's child beside its socket, so startup and a slow handshake never delay `service.status`. A child that exits or breaks the protocol is replaced with backoff; a registered folder that is away (a disk not mounted yet, a deleted folder) leaves that project waiting with its reason and comes back when the folder does, and it never stops the service. A child that answers late is not abandoned: that one request stays unknown, the late reply is dropped, and the journal poller resnapshots when the child answers. Nothing here replays a command or a prompt.

## Startup and project read isolation

The native bridge reloads the fixed channel unit, requests a nonblocking start
and waits up to 30 seconds for its authenticated local socket. That readiness
budget is separate from the request deadline: no action bytes are sent until
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
the two. Only the details read replaces those rows.
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

## Local desktop slice (built)

The adapter appends each public invalidation in the same SQLite transaction as the private write that caused it. Connection-local TEMP triggers on the adapter's source tables call one append helper on each supported Store connection, so a failed append rolls back the state change, including in autocommit. No persistent trigger requires independent or older SQLite writers to load Workspace code or retain its optional tables. Those external writes are outside the notification guarantee; the hooks are not a security boundary. Normal writable initialization removes the legacy persistent hooks in its schema transaction; non-initializing inspection never repairs the durable schema. Public project sequences are allocated only from the adapter's own outbox state, never from runtime event or transcript identities. Reconnecting clients receive a snapshot and then events after the cursor. A gap, a wrong project or a backwards version resynchronizes the client.

A task start is recorded as prepared, claimed, dispatched or unknown before the prompt pipeline runs. A restart after dispatch leaves the execution unknown and never replays the prompt. Answers to a single question or a complete batch are all-or-nothing. Only the winning request runs the existing continuation; if that handoff fails, the answer reports `answer_continuation_unconfirmed` rather than success.

The service owns the ACP child. Closing a client detaches its subscription and leaves the child running; explicit stop cancels the owned controller, closes the child's input, waits a bounded grace period and then kills only its own child. A restarted service creates fresh controllers and snapshots without replaying any start or prompt. Test evidence and its limits are recorded in [verification](../project/verification.md).
