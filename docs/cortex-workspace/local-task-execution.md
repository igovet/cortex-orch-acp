# Local task execution

Conversation waits explain when decision details are unavailable and point to Answer in the header. Open decisions also render before their timeline reference arrives. Sent attachment chips tolerate the whitespace normalization used by runtime history.

**Status: implemented source slice, not full product or installed-package qualification.**
See the [protocol](protocol.md#local-desktop-slice-built) for the current local wire version, and
[IDE and Git](ide-and-git.md) for the wider contract.

For update, native/API-key connection, voice-settings and command-control behavior,
see [desktop maintenance and controls](maintenance-and-controls.md).

## Python-independent native service startup

The only host command is `workspace-service <allowed-root> <runtime-root>` (plus the one-argument MCP relay and the internal maintenance-worker mode). The absolute runtime root is checked as a source checkout or this channel's installed bundle, and no interpreter or helper script is required to bind the socket. The legacy ACP child, `agent_launcher.py`, `machine_tools.py`, the three- and five-argument forms, `--fixture-launcher`, `CORTEX_WORKSPACE_NATIVE=0` and `projects.resynchronize` were removed on 2026-11-16: the workspace service has only the native Rust execution host. `LaunchSpec::native` is the single launch specification. The trusted asset root supplies bundle/voice lookup; it cannot be supplied by socket JSON. Native machine capability stays advertised.

Native Git/files, task pages and saved history, Settings reads/saves/native actions, and engine login-command lookup/recheck/reference registration are native routes. Startup does not launch an engine or refresh credentials. Registered project identities are still listed independently of store readability or activation.

account or paid-model qualification.

## Native machine boundary

The new-task coordinator chooser uses the selected project's routing rules and
effective allowed-route list. If the machine default is not offered, the form
initially selects the first remaining model, when there is one, and saves that
choice with the draft. Manual planning, replanning and planning recovery use the
task's coordinator choice or its recorded route instead of resolving the machine
default first. The chooser does not authorize engine dispatch.

The reasoning-effort picker lists only concrete levels reported for the selected
model, ordered from Low to Max. If no effort was chosen, including on older
drafts saved with `default`, the coordinator uses the effort set in Settings,
or Medium if Settings has none. If the model does not offer that level, it
uses the nearest reported level (lower on a tie). A model with no reasoning-effort
control does not receive a reasoning-effort setting.

Save and Save-and-leave share one in-flight write per settings draft. A genuine
conflict from another write still requires reading and reviewing the current
settings; an unconfirmed write is not retried automatically.

`project.config.read` runs entirely in Rust (the former `PythonCoreMachinePort` was removed). It reads the registered project's routing file and
Core's live read-only project-ceiling row. The editor retains project-only rules,
all three engine entries, policy and quota fields, sorted ceiling sets, exact
SHA-256 tokens and numeric revisions; absent overrides stay absent, and malformed
routing keeps its byte revision with the existing empty editor. It does not
substitute effective user/environment rules or initialize a missing store. A
registered linked checkout with no root-local store retains the legacy missing-store
refusal rather than silently loading another checkout's settings.
Private owner defaults and connection-registry metadata retain their validation
refusals; no credential reference, catalog body or engine is opened. Reads use
bounded no-follow/nonblocking regular-file opens, and refuse oversized settings
rather than delegate or return partial data. Each configuration file is bounded
by the existing native frame budget; stored settings rows share that bound.

The 125 current-Python offline cases in
[`project-config-python.json`](../../crates/workspace-service/tests/fixtures/project-config-python.json)
are captured by
the former generator `generate_project_config_fixture` (removed with the Python runtime).
Service-boundary comparisons run in the private module tests with no helper or
engine configured, preserve project/configuration bytes (excluding SQLite's
shared-memory reader slots), and use short socket roots independently of TMPDIR.
Machine config.save now uses the [native writer](../configuration.md#native-machine-saves);
engine.connection login-command lookup, recheck status/reference registration and
API-key creation use native Rust. API keys retain canonical private secret-file
storage, separate registry transactions and known-failure removal/opt-out; uncertain
storage outcomes never switch providers or replay. Valid keyring validation and
installers/onboarding outside their pre-effect refusals are refused natively (the on-demand Python provider was removed); stale-revision and installer pre-effect refusals are native. Catalog refresh
preserves its read-only empty/null/empty-argv/relative subset and adds a working
native Codex/Claude observer for direct commands, qualified two-argument Codex/Claude
installs and native-account/API-key secret-file launches. Unicode labels and
scrubbed noisy-peer logs have recorded coverage. Sixty-two recorded captures of the former Python implementation
(54 prior cases and eight OpenCode pre-probe failures) compare full replies/files/modes
and exact prompt-free transcripts
with zero helper calls; startup still exercises native observation.
Qualified [OpenCode metadata-only pre-probe failures](../configuration.md#opencode-pre-probe-rejection)
now stay native without logs, binary probes, a jail or snapshots, and so does the
exact-hash Codex 2.3.0 bundle (its adapter is byte-identical to the recorded former patch). Potentially
usable OpenCode installs, keyrings, unqualified
Codex/Claude candidates, unsafe names/roots, missing locks and unqualified projections
are refused as a whole before effects. Unqualified post-observation
outcomes refuse without provider replay. Qualified Codex installs resolve
nested/hoisted npm dependencies, generate private source-guarded adapter/launcher
artifacts and merge offline debug-model efforts. Their catalog launcher now uses
the compiled Rust service as a Linux shebang interpreter, not Python or a shell;
safe legacy artifacts migrate atomically without replay. 39 offline
execution comparisons cover checks, argv/environment/cwd/stdio, timeout/reaping
and final PID/status/signals. Catalog comparisons explicitly validate this one
artifact-content change and preserve all other comparisons. Unsupported service
interpreter paths refuse before effects. See
[the exact producer limits](../configuration.md#native-launch-capable-catalog-producer)
and [prelaunch catalog refresh](../configuration.md#native-prelaunch-catalog-refresh).
See [per-action engine setup](../configuration.md#retained-engine-connection-setup)
for command, persistence and fallback boundaries.
Machine config.read is passive native display; config.action runs connection rename
and session-limit edits, confirmed default-preset reset and profile
read/create/save/update/delete/deactivate in Rust. Filesystem-reference validation
and connection removal now also use Rust, including canonical single-link owned
secret-file unlinking without reading contents. Qualified preset apply and format-1/2
four-section profile activation use Rust; wider ignored profile-document metadata is
also native; valid keyring validation, unsupported removal
defaults, custom preset documents and other activation/numeric shapes are refused natively.
Other unported producers refuse before admission. This is not an
installed-client qualification.

### Native project settings saves

`project.config.save` is native (the former `PythonCoreMachinePort` was removed).
Both routing and project ceilings use native validation and exact revision checks.
Routing retains supported-key ordering, unknown nested metadata, Unicode, numeric
formatting and owner-only atomic replacement. Ceilings retain the existing compact
JSON columns and timestamp-based revision material; the Core immediate transaction
preserves project defaults and refuses removing a defaults-bearing row.
The shared private `connections.lock` serializes with retained owner settings
writers; narrowing is checked against that locked user ceiling. Missing stores
are never initialized and no scheduler ownership is acquired.

The full reply is prepared and budgeted before effects. A failed post-rename
directory sync or uncertain database operation reports an unknown outcome,
not permission to repeat or delegate the write. No engine, credential file,
catalog refresh or configuration revalidation process is launched.
Routing-save replies include bounded cached model-effort display observations;
these do not establish route eligibility.

The 94 disposable-project cases in
[`project-config-save-python.json`](../../crates/workspace-service/tests/fixtures/project-config-save-python.json),
captured by
the former generator `generate_project_config_save_fixture` (removed with the Python runtime),
compare complete replies, routing bytes/modes and saved ceiling rows. Eleven
additional real Python captures compare observed/default-only/conflicting/invalid
catalog efforts. Native tests cover concurrent revision arbitration, unsafe lock
and target files, SQL rollback and uncertain post-replacement failure, without a
configured helper or engine. Independently produced ceiling clocks and their
derived revisions are validated against actual rows before clock-only normalization.

Native limits deliberately refuse oversized replies/files/rows, unsafe storage,
nonfinite or excessively nested JSON, and preserved integers outside the exact
64-bit range instead of writing rounded metadata. Invalid *other routing source*
diagnostics are sanitized native messages, not Python parser exception text.
The retained child environment does not forward `CORTEX_ROUTING`; save replies
therefore preserve the service's project/user precedence. Machine config.read has
its separate native boundary; remaining config.action variants, engine.connection
mutations outside the native gates are refused natively (the Python provider was removed).

`config.mcp.discover` runs completely in Rust. It retains the nested observation/refusal envelope, only sends
initialize/initialized/tools-list, and never persists configuration or calls a
tool. Keep-env markers use the private channel registry; configured values and
credential-shaped description text are scrubbed. The native external-MCP
Landlock boundary enforces explicit write grants, hides the owner's secrets and
Cortex stores, refuses protected descendants, and kills the process group on success, timeout or
cancellation. This is a cooperative same-user boundary, not hostile-code
isolation or the Python mount-namespace sandbox. Missing containment refuses
launch. Grant trees above 50,000 entries refuse; owner files retain the native
frame budget. Protocol budgets remain 15 seconds, 256 KiB buffered, 32 messages
per reply, 20 pages and 1,000 tools. Unix/runtime scratch roots remain short
independently of TMPDIR.

[`mcp-discovery-python.json`](../../crates/workspace-service/tests/fixtures/mcp-discovery-python.json)
contains 77 offline current-Python operation replies and 110 redaction cases,
captured by
the former generator `generate_mcp_discovery_fixture` (removed with the Python runtime).
Capture substitutes only sandbox argv construction for a disposable toy stdio
peer; it does not qualify the Python sandbox. Native tests separately exercise
real filesystem/network containment, explicit grants, and descendant cleanup
on success, timeout and cancellation, without a helper or model engine.
The fixture peer was a Python script when the captures were recorded; production discovery has no interpreter
dependency and can run any configured supported stdio executable. Native service
startup is independent of Python as described above; neither result qualifies an installed app.

`product.integrate` now performs read-only admission in Rust: exact confirmation,
saved readiness/acceptance digest, local already-in-target observation and the
bound completed-runtime prerequisite. The refusal order and envelopes are compared
with 57 offline Python cases in `crates/workspace-service/tests/integration_admission.rs`,
captured by the former generator `generate_integration_fixture` (removed with the Python runtime) beside it. Refusals preserve overlay,
SQL authority, real index, HEAD and user bytes; missing stores are not initialized.
The earlier project-folder squash comparison used the native result/source validator
for a narrow case: the source must be the clean, already-committed project
folder, the target an existing clean worktree and ancestor of that source, and
cleanup must not be requested. Report/review/check validation, current source,
fresh-result and accepted-revision fences precede every effect. The machine lock
is held only for admission (the saved task record is checked again under it) and
for each overlay save; the existing durable operation intent precedes Git, and Git,
hooks and GitHub run after the lock is released. Each save writes only the fields
integration owns (status, work, completion time, integration, fresh-result marker,
conflict, merge, acceptance, automatic-ready marker, pull-request attempt) into the
overlay as it is then, so other edits made meanwhile (a title, a target) are kept,
and it waits for the machine lock without a time limit, as does finishing the
intent: an effect that happened is always recorded. Until the integration
settles, another owner action on the same task and deletion of its sprint are
refused at once with `operation_in_progress`, and any other Git-changing owner
action of the same repository (another integration, a merge finish or abort, a
file-conflict choice, a Git action) is refused at once with
`integration_in_progress`: one runs per repository from admission to the
recorded outcome.
A known target-commit failure restores its exact previous index only if HEAD is
unchanged. A real target-side `git merge --squash` content conflict (unmerged index entries)
is reported as `integration_conflict`, not as a hook failure. It is not eligible for
Cortex handoff. Because a squash
writes no `MERGE_HEAD`, `git reset --merge` undoes the conflict markers, staged
task files and `SQUASH_MSG`, and only a then-clean target gets its exact previous
index back; otherwise the reply says the target may still hold the conflict. A
merge Git refuses before writing files restores the index and names likely
overwritten files. Unit cases in `src/machine/integration_squash.rs` force both
with real Git. Changed HEAD, cancellation, timeout or post-Git overlay-save failure
remain uncertain, with no helper fallback or blind repeat. Normal hooks remain
active. Success persists the existing Done/integration shape, not a runtime
completion or new scheduler. Seventeen additional offline Python reply/state/Git
cases in `fixtures/integration_squash_python.json` include success, real hook
failure, changed-HEAD uncertainty and result/source/review refusals; capture uses
the former generator `generate_integration_squash_fixture` (removed with the Python runtime) beside the existing tests. Socket roots
remain short even under a long TMPDIR. Unsupported receipt/closure evidence,
source staging, same-branch commits, target updates/conflicts, temporary target
creation, PRs and cleanup are refused before effects. Unported configuration/setup
is refused natively (the on-demand Python provider was removed).

Current legacy source-bound verification for plain and Git task roots now reads
natively in `product.read` and `product.facts`. Rust reproduces the legacy sorted
manifest bytes and SHA-256, including Unicode, symlink targets, nonregular
entries, dependency/store exclusions and the path-exact regular engine lock.
The recorded task workspace, not necessarily the logical store root, supplies
the source. Files above 1 MiB or the 512 MiB content budget retain the legacy
size/inode/mtime/ctime identity; smaller files are freshly hashed without a
content cache. No-follow descriptor-relative traversal and before/after state
checks fail closed on observed races. Without a Git list, the 50,000-entry/
20-second bounds never produce a partial digest. Git selection uses fixed
installed Git with a private HOME under Landlock (writes only to its scratch,
excluded entries hidden) and disabled hooks/fsmonitor. Linked-worktree pointers
and common metadata are supported. Unavailable Git/sandbox/metadata uses the
legacy whole-tree fallback; a successful list selects tracked and untracked,
not-ignored paths plus the root ignore boundary, with the legacy 300-second
scan budget and no entry cap. Non-UTF-8/malformed or over-64-MiB lists, private-mask
enumeration limits, non-UTF-8 source names/targets and depth of 129 or more
directory components retain the provider. Runtime event/check revision,
exact command, completeness, integer exit and digest rules, current review/report
integrity, open-check acknowledgement and fresh-result fences retain their DTOs.
A displayed Accept control is not an acceptance mutation or rerun closure.
the former generator `generate_source_verification_fixture` (removed with the Python runtime) captures the offline Python reference
in `fixtures/legacy-source.json` and `fixtures/task-source-verification.json`
under `crates/workspace-service/tests/`; the Rust tests need no helper or engine.
the former generator `generate_git_source_fixture` (removed with the Python runtime) additionally captures four Git manifests and
eleven exact control DTOs in `fixtures/legacy-git-source.json` and
`fixtures/task-git-source-verification.json`. The listed-walk interpreter uses
actual fixture-only Git file lists; this is separate from real sandbox success.
The capture host and Rust transport checks report unavailable sandbox listing
and exercise the bounded whole-tree fallback, not positive sandbox qualification.
Literal-gap rerun successes now also read natively: the latest finished attempt
must match the current owner's epoch, exact target hash/argv/cwd, reviewed and
current source digest, unique ordered request/start/finish records, complete
exit-zero check and the exact author's observed publication. Private retained
output is fully read without following links, with owner/mode/link-count,
length and before/after identity checks; bodies above 8 MiB retain the provider.
An unchanged exact literal worker or runtime-verification target can carry only
a missing-command/tool blocker across owners, never a pass or permission grant.
The 135 offline Python cases in `fixtures/task-rerun-closures.json` are captured
by the former generator `generate_rerun_closure_fixture` (removed with the Python runtime) under `crates/workspace-service/tests/`;
they compare both machine read DTOs without a helper or engine at test time.
These seeded stores test interpretation, not live check execution. Unknown,
replayed, stale, malformed, truncated or mismatched evidence never becomes a
pass. Final-verification gap closures also read natively: exact unique runtime
start/output receipts for every planned command must bind the reviewed current
source, and the recorded gap must match its author's completion and publication
(or review digest). Later pending, failed or ambiguous reruns supersede a closure;
derived closed_by fields do not prove one. The fixture includes 63 final-verification
cases, including duplicate commands, policy blockers, missing/changed reports and
private-output failures. Receipt-derived failed/passed targets still retain the
interpreter, as do existing size/materialization boundaries.
Native acceptance and owner-deletion boundaries are described below.

Saved draft `product.task` actions `edit`, `settings` and `rename`, plus private `target`, `reopen`, `restore` and `sprint` linking, now run natively when checkout options are materialized.
Draft edits preserve attachment bytes and unrelated options; expected-version and execution-identity checks arbitrate against Start in the same Core transaction as version/outbox updates.
`rename` alone also applies to a started, finished or closed task: it changes only the owner-visible title option, never the saved request, settings or lifecycle, and a cleared or deleted
task refuses it with `not_editable`. Sprint linking preserves dependency order, refuses started tasks and holds the Start arbitration transaction across the private JSON replacement. Private
target/reopen/restore changes neither start work nor certify a result. The unchanged response and persistence shapes are compared with offline Python fixtures in
`crates/workspace-service/tests/task_actions.rs`, captured by the former generator `generate_task_action_fixture` (removed with the Python runtime). Legacy unmaterialized checkout
projections, and structured or floating-point rename titles outside the native text-title surface, still delegate before any effect. Owner `close` (including external/already-in-target),
`archive` and `merged` run in Rust as private presentation actions. External closure preserves the confirmation/conflict/settlement/containment refusal order,
checks session/root/epoch, unknown launches and live workers, and rechecks settlement under the Core immediate transaction through overlay replacement. Archive keeps the saved work and checks as they were; for unfinished work it can request the existing runtime Pause without closing or certifying the result. See [Archive and service-owned work](#archive-and-service-owned-work).
The old unbound `resolution_started` action is refused; use the conflict-bound owner control described in [Integration and conflicts](#integration-and-conflicts).
The lifecycle fixture in `crates/workspace-service/tests/task_actions.rs` was captured by the former generator `generate_task_lifecycle_fixture` (removed with the Python runtime). Owner acceptance and archive-discard answers now also
run in Rust within the native evidence/materialized-checkout boundary described below. Manual file choices and merge finish/abort now use Rust as described below; integration
validation/effects outside the committed-source local squash slice are refused natively (the former Python provider was removed). An uncertain native mutation never falls back or
automatically replays.

### Native owner review decisions

`product.task` actions `accept` and `confirm_discard` now use Rust for
materialized checkouts and the existing native result-evidence surface.
Acceptance preserves the completed/current execution, project/session/owner,
unknown-Start, digest, fresh-result and evidence-version gates. Existing report,
independent-review, verification, source-drift and technical-blocker validation
is shared with native result controls, not inferred from model text. Open
automated checks require literal acknowledgement; manual checks do not become
automated blockers or outcome counts. The persisted acceptance retains the
owner's original version/digest values, exact open counts and a fresh legacy
checkout digest. Informational answers need no invented verification.

The machine lock protects the overlay. Acceptance revalidates under Core's
immediate writer transaction through the private JSON replacement, preventing
Start/owner-state changes between validation and save. It neither completes the
runtime nor creates a check receipt. Archiving never records discard consent. `confirm_discard` is allowed only on an
archived task and binds the owner's consent to delete its work at cleanup to the
exact legacy checkout digest; it deletes no files or task.
Unobservable source records no consent; later purge still rechecks the digest,
unmerged work and its independent deletion fences.

The 83 cases in
[`task-review-python.json`](../../crates/workspace-service/tests/fixtures/task-review-python.json),
captured offline by
the former generator `generate_task_review_fixture` (removed with the Python runtime),
compare replies, persisted JSON and SQLite rows through `Service::machine`
without a helper or engine. Cases include plain/Git/worktree sources, unknown
starts, foreign bindings, stale versions/digests, manual and policy-blocked
checks, failed reviews, drift, unreadable files.
Additional native tests exercise exclusive store access and real overlay-write
failure: neither invokes Python or changes authoritative runtime state.

Legacy unmaterialized options, unsupported/bounded-out result and source
evidence, and oversized legacy retention intervals are refused **before**
effects. No error after saving the overlay becomes a helper retry. The five
retained configuration/connection operation families remain unfinished; merge
finish/abort now use the native path below. Native startup is independent of
Python, but this is not installed-client or paid-model qualification.

### Native manual file resolution

`product.task / resolve_block` now runs in Rust for materialized checkouts and
string-identified file/block records. It preserves busy handoff, missing conflict,
stale file/block, invalid choice, unsafe mode/stage, symlink and binary refusals.
The selected ours/theirs/both lines replace the whole displayed file with a final
newline, preserving its executable mode. Git stages only that literal path.
Private paths are rejected; the shared descriptor-relative writer refuses linked
parents/files and revision drift. Native reads/writes are bounded to 512 KiB.
Malformed legacy shapes and oversized selected text are refused before effects.

The private machine lock spans inspection, the durable intent, file replacement,
staging and JSON save. A supplied request identity is checked before legacy
projection fallback: finished replies replay; dispatched/unknown intents refuse
with an unknown outcome; changed payloads conflict. After file replacement is
attempted, every failure stays unknown, even if Git or the JSON save failed.
Cancellation leaves a dispatched intent. Nothing forces Git, restores user files,
or retries through Python. Callers without a request identity retain the existing
new-operation semantics, not inferred replay safety. The service's registered-root
and single-owner fences remain unchanged; this action neither settles runtime
work nor certifies a result.

The 36 real-Git offline Python cases in
[`task-resolution-python.json`](../../crates/workspace-service/tests/fixtures/task-resolution-python.json),
captured by
the former generator `generate_task_resolution_fixture` (removed with the Python runtime),
cover all three choices, executable files, worktrees, unsafe add/delete/mode
conflicts, binary/empty lines, Unicode/space paths, stale inputs and missing stores.
Replies, saved JSON, SQLite rows, file bytes/mode and Git index/status/HEAD compare
through `Service::machine` with no Python helper or engine configured.
Native Git's existing sanitized failure message intentionally replaces the raw
Python stderr in the non-repository case; the refusal code/envelope are unchanged.
Native regressions exercise real index-lock and overlay-permission failures,
keyed success/unknown replay, changed payloads, exclusive store access,
private-path refusal and pre-effect legacy fallback. They do not qualify
process-kill crash timing or positive native sandbox capabilities.

### Native merge finish and abort

`product.task / finish_merge` and `abort_merge` require the saved checkout and merge identity. Active or unconfirmed writers block both. Finish refuses unmerged entries or a failed staged diff check (including markers/whitespace). It does not stage additional paths. It commits only the exact stopped merge; if Git already committed it, Finish requires HEAD to have exactly the saved source and target commits as parents, in that order. A confirmed finish compares Changes against the merged target commit and requires fresh evidence for the resolved revision.

Abort uses `git merge --abort` only for the exact stopped merge, or recognizes that it was already aborted. It refuses a replacement operation and does not advance the comparison base. The earlier freshness marker can be restored only if both the pre-merge deliverable and the staged-index snapshot match. Neither Finish nor Abort certifies a result or completes runtime work.

The machine lock spans reads and intent persistence, and each private JSON save,
which writes the task's merge-related fields (including the Changes base on a
confirmed update) and the overlay's `hasGit`/`ghAvailable` facts, and waits for the
lock without a time limit. The Git commit or abort and its hooks run after the lock is released, under
the repository's integration lock (another Git-changing owner action of the
project is refused at once with `integration_in_progress`); meanwhile other owner
actions on the task are refused at once with `operation_in_progress`.
A successful commit must have the observed HEAD and merge HEAD as parents;
a successful abort must leave HEAD unchanged. Both require MERGE_HEAD gone.
Normal hooks remain enabled, and no force, automatic restoration or retry is
added. Any error after attempting Git or overlay replacement remains unknown,
including hooks that commit then fail and successful Git followed by a failed
save. Supplied identities replay a finished reply or refuse dispatched/unknown
effects before any legacy projection fallback; changed payloads conflict.
Callers without keys retain the existing new-operation semantics.
The registered-root and single-owner fences are unchanged. External same-user
Git processes can race this cooperative lock; Git and overlay writes are not
one atomic transaction.

The 38 offline current-Python real-Git cases in
[`task-merge-python.json`](../../crates/workspace-service/tests/fixtures/task-merge-python.json),
captured by
the former generator `generate_task_merge_fixture` (removed with the Python runtime),
compare service replies, saved JSON/SQLite rows, file/index/status, MERGE_HEAD,
commit tree/parents/message, and same-attempt exact HEAD preservation on replay.
Fixtures cover resolved/unresolved/staged markers/whitespace, already manually
committed merges, worktrees, unrelated staged/untracked work, busy/stale inputs,
missing stores and actual hooks. Disposable paths, capability presence and
validated wall-clock timestamps are normalized; new commit timestamps/hashes
are not compared across runs. Native failures intentionally exclude Python
stderr. Failed attempted commits/aborts return an unknown-effect envelope
rather than Python's settled Git failure, even when a particular failure left
no change. Real index-lock and overlay-permission regressions test both actions,
unknown replay/payload conflicts, exclusive store access and invalid identities.
Tests need no Python helper or engine and use short socket roots independently
of TMPDIR. Process-kill crash timing and real-client GUI behavior are unqualified.

Shared unmaterialized legacy options/absent option-table projections still
are refused before effects, as are the existing unsupported evidence paths.
There is no merge-action-specific Python branch on the native boundary.
The machine operations that were directly delegated to the former Python provider, and are now refused natively, are `config.action` variants
other than connection rename/session limits, filesystem-reference validation,
removal with supported defaults (including canonical secret-file unlink), qualified
preset apply, default-preset reset, profile read/create/save/update/delete/deactivate
and qualified format-1/2 four-section activation, and `engine.connection` actions other
than native `login_command`/`recheck`; config.read
is passive native display and config.save uses the native all-surface writer without
fallback. Unqualified profile materialization documents and wider modeled/backing-file
numbers are refused natively; ignored wider profile-document metadata
is now native. Current routing activation requires model-policy version 2 in both
providers; missing/other-version refusal shapes are native; there is no additional MCP-management config.action verb to migrate:
MCP add/edit/remove/enable/validation already use native config.save, with explicit
discovery on native config.mcp.discover. See [per-action providers](../configuration.md#native-machine-actions).
Project saves use the native boundary above. Integration
outside the committed-source local squash slice, unsupported legacy facts,
receipt-backed worker actions still need migration. The retained mode and its Python launcher were removed.

### Native owner deletion

Two action-qualified operations, `product.task`/`purge` and `product.task`/`clear_old`, run natively for materialized checkout projections. This does not migrate the entire `product.task` surface. Purge requires literal confirmation and an archive. Clear-old rechecks literal confirmation, a nonnegative non-future numeric cutoff, finished/unarchived state, a numeric completion time at or before that cutoff, and absence of unmerged work. Booleans are not timestamps. Stale preview and unmerged-work refusals retain the existing shapes and private archive question.

The shared Core purge serializes against Start using the exclusive cooperative store-access lock, retains a durable deletion intent before runtime-file teardown, and rejects unknown/unbound launches, active work, process/capacity protection and running worker nodes. Running-node refusal is intentionally stricter than the legacy Python purge, which can delete a terminal row containing a running node. No owner state was used to establish this difference. No scheduler transition, acceptance or check evidence is inferred. A missing task in an existing store refuses rather than being treated as a successful replay. A missing store is not initialized. Discard consent is checked against the legacy manifest digest; unavailable or unsupported observations are not consent. Only clean, registered worktrees with Cortex ownership evidence can be removed, without force; other worktrees remain retained.

Replies, private JSON and task/options/project state plus nine table counts are compared against 117 offline Python captures in
[`task-deletion-python.json`](../../crates/workspace-service/tests/fixtures/task-deletion-python.json), produced by
the former generator `generate_task_deletion_fixture` (removed with the Python runtime).
Tests run with no helper or engine, using short socket roots. Reclaimed byte counts are validated as measured nonnegative integers before normalization because independent SQLite files have different physical sizes; success/failure statuses and every other reply field are compared unchanged. Tests also cover active/unknown launches, retained and removed worktrees, dirty/changed HEAD, valid/stale discard digests, cutoff boundaries, exact public-removal counts and foreign-key integrity. Separate native tests preserve the stronger running-worker and exclusive-access fences and simulate an overlay-save failure after committed deletion: no Python fallback or second deletion occurs. A blocked checkpoint reports failed reclamation while retaining the committed deletion. Main.rs's Python launcher and the retained config.action/engine.connection operations remain; native file resolution and merge finish/abort are described above.

Standalone machine calls run inside the Rust service: repository snapshots (status, changes, log, branches and worktrees), explicit Git mutations, folder search, plain-checkout file read/save/locate, machine settings/overview/diagnostics/logs, private message/comment queues and sprint-container JSON. Installed Git remains the subprocess boundary; these calls do not launch Python. Initial Git reads retain the existing frontend refresh/coalescing behavior and DTOs. Untracked directories collapse above 2,000 files; if more than 2,000 entries remain after collapsing, the service reports a bounded inspection refusal rather than silently omitting files or claiming repository absence.

Git mutations retain explicit confirmation, staging selection, normal hooks/filters, ff-only pull and non-forcing push by configured remote name. An empty index causes one automatic `git add -A -- <literal paths>`, not a call per file. A native Git write records a private intent before dispatch and its sanitized reply afterward. Git command identities supplied by the desktop prevent same-key re-dispatch; unresolved effects stay unknown and changed payloads conflict. Records contain digests rather than command text, URLs or raw diagnostics. Older callers without an identity cannot reconcile by the missing key; they must inspect Git instead of repeating an ambiguous action.

Machine configuration actions/engine connection setup, remaining retained task lifecycle/review actions and integration validation/effects outside the committed-source local squash slice and
receipt-backed worker withdrawals are refused natively (the former Python helper behind `CoreMachinePort` was removed). Explicit owner workflow descriptions, maintenance, retention cleanup
and destructive sprint deletion now run in Rust. Sprint deletion freezes membership and persists a start fence before cancellation; Core rejects a fresh Start for a deleting member. A
retained deletion intent and exclusive cooperative store-access lock cover per-task file/row teardown; active, unknown or process-protected executions refuse purge. Dirty or unproven
worktrees remain kept, never force-removed. Reclamation outcomes are explicit and do not turn a committed deletion into a failure. Qualified native-owned controls and their
worker/check/settings facts use the Rust path described below. Repository snapshots and `repo.context` read Core-owned task baseline/checkout options through a validated Rust Core live
read-only store; missing stores are not initialized for those reads. Overview resolves the channel-specific logical store through Core, including verified linked worktrees. Code save holds a
Core immediate transaction across lease validation and descriptor-relative replacement, serializing it with managed writer admission and structured file effects. Missing runtime bindings,
unreadable stores or unresolved conflicting writers do not permit a save. Native saves use descriptor-relative no-follow traversal, a required base digest, a second pre-replacement check
and atomic replacement; binary/non-UTF-8/oversized writes are refused. These machine
operations introduce no second scheduler. The same service now hosts a reusable Core draft writer and claims a cooperative project/channel lease before controller launch. Draft creation
without attachments bypasses ACP using the existing wire contract; other task mutations retain their handlers. See [native draft
writes](protocol.md#native-draft-writes-through-the-existing-desktop-contract) for the limits.

Directory search preserves ranking, allowed/private-root exclusions and count/depth/deadline bounds. Cancelling/replacing a search stops its wait and cooperatively stops traversal. A kernel-blocked filesystem thread cannot be killed in-process; it retains one of two permits, so it cannot create unbounded threads or block the async reactor/service shutdown. A hard deadline reports incomplete results, never successful completeness. These source checks are not installed-package, rendered-GUI or full all-Rust cutover qualification.

Standalone `product.read` and empty-worker `product.facts` requests read saved overlays/options, waiting controls and bound informational answers natively. Completed-work controls now
validate report digests, current-revision review verdicts/waivers, provisioned and unprovisioned check disclosures and absent, stale or digest-less verification. Missing reports, changed
content and unverified work cannot enable acceptance. Old-revision and non-runtime rerun records do not affect current controls. Current rerun controls with no possible source-bound closure
now use native latest-event interpretation per assignment/check index, including pending/ambiguous outcomes, complete failures, timeouts and same-owner technical blockers. Recorded successes
remain unverified, not closures; stale-owner outcomes cannot certify checks. Runtime-verification controls apply an attempt only when its event is newer than the verification. Source-bound
rerun claims failing necessary closure conditions (including failed, incomplete, stale, truncated and digest-mismatched observations) also read natively; no alternate native hash replaces
the exact source observer. Potentially valid rerun closures, cross-owner technical-blocker carry, provisioned passes with a matching gateway authorization and provisioned failed-check
withdrawal claims are refused natively (the former Python provider was removed). Model-supplied current-outcome and correction-display fields never establish a withdrawal. So do
unmaterialized legacy worktree options, stores without optional Workspace option tables, and evidence beyond native bounds (16,000 synthesis/result-JSON bytes, 1,000 nodes or 8 MiB per
report). The task-facts cutover is incomplete. Local integration presentation compares content with a private shadow index/object directory and never promotes a task status; missing or
failed containment evidence stays false. Offline Python-oracle DTOs and disposable-store checks live in `crates/workspace-service/tests/task_facts.rs`; the former generator
`generate_task_work_fixture` (removed with the Python runtime), the former generator `generate_task_provision_fixture` (removed with the Python runtime) and the former generator
`generate_task_attempt_fixture` (removed with the Python runtime) capture completed-work cases without engines or owner stores. Rerun evidence beyond 1,000 records or 16,000 payload bytes,
and malformed revision/target/result bindings, are likewise refused natively.

Standalone `workers` and worker bundles in `product.facts` now read retained assignment/attempt
identities, terminal status, selected tool/edit/command activity, guidance delivery and confirmed route
predecessor chains directly through Rust Core. History retains the existing 200-event and shared byte
budgets, canonical attempt-bound `wa2` cursors with independent task/runtime positions, sparse split-call
enrichment and explicit clipping notices. Later inserts cannot enter an older page. Read/report bodies,
thoughts and foreign-attempt/channel activity are not exposed. Displayed worker checks remain claims,
never acceptance evidence. Possible receipt-backed withdrawals, noncanonical timestamp forms, unsupported
request shapes and evidence beyond the native bounds (1,000 rows or 8 MiB per intermediate query) are
refused natively (the former Python provider was removed). Offline fixtures in
`fixtures/worker-facts.json` are captured by the former generator `generate_worker_fixture` (removed with
the Python runtime); source-bound nonclosure controls in `fixtures/task-source-nonpass-facts.json` by the
former generator `generate_source_nonpass_fixture` (removed with the Python runtime), all under
`crates/workspace-service/tests/`. Neither generator uses an engine or an owner store.

Coordinator detail requests (`workers` with `workerId=coordinator:<task>`) now read the registered task's
retained ACP session directly in Rust. The existing messages, plans, selected tools/edits/commands, status
and confirmed route DTOs retain the 100-row scan and 60,000-byte page budget. `wh1` continuations bind
project, task, session, page size and a fixed high watermark; later inserts cannot enter older pages, and
session replacement invalidates old cursors. Sparse tool updates enrich retained input across page
boundaries, with the original 100-update clipping notice. Hidden bookkeeping and thoughts stay out of the display; every other call keeps its bounded, credential-redacted input and
result (a file read or edit keeps its path link and diff only). Malformed retained updates retain an unavailable-content row;
unsupported request/display shapes, private report diff blocks, noncanonical timestamps and intermediate
reads above 1,000 rows or 8 MiB still use the retained interpreter. Offline Python DTOs in
`fixtures/coordinator-facts.json` are captured by the former generator `generate_coordinator_fixture`
(removed with the Python runtime) under `crates/workspace-service/tests/`; tests run with no Python helper
or engine. The full receipt/closure validator remains on the migration provider; native Git source
observations do not substitute for its receipt checks.

Native `product.read` attachment requests read the registered task's saved options
through Core, including before activation, returning a validated array of file
references (including a genuinely empty array; each body is saved privately on that
read and opened with `product.attachment` `read`), not the unrelated presentation
product object. A missing task or invalid saved
attachment array refuses. The same files start with the task (see [Attachments](#attachments)).

Completed native task facts display the retained synthesis alongside observed
checks. Their freshness comparison uses a fresh native project observation, not
an unrelated Git presentation digest; a subsequent file change still makes the
result stale. Owner check reruns remain observations, not task certification.

## Native service restart recovery

Native owner opening and service Stop share a settlement boundary: an in-flight
activation cannot install a new runtime after Stop returns. Project opens remain
concurrent and do not hold the machine-state mutex over recovery.

After acquiring the exclusive native scheduler lease, activation reconciles retained
tasks before publishing the project as ready. Fully settled checkpoints continue
execution, independent review and contained verification through the service-owned
pump, without an extra Start or an incoming ACP request.

A service death is not a process wait. Unsettled sessions/reservations, interrupted
controls and unfinished verification intents retain an explicit unknown-outcome
blocker; no old prompt, guidance, answer delivery or command is replayed. Old
channels are revoked and uncertain capacity stays occupied. Open questions and
wait records survive, with continuation disabled when their exact session is lost;
already-claimed answer deliveries become orphaned. Paused tasks remain paused.
This does not implement ACP session reattachment or automatic adoption of unknown
effects. Source checks use fake-engine SIGKILL journeys and settled checkpoint
reopens in `crates/workspace-service/tests/native_desktop.rs`.

## Native route candidate binding

`crates/workspace-service/src/native_routes.rs` is an additive read-only building
block used by the production service route provider. It reads the first existing routing file
from the project channel store, explicit `CORTEX_ROUTING`, then user configuration;
files are never merged or seeded. User defaults and approved connection records
bind only to genuine cached, fingerprint-checked catalog identities. Exact saved
connection/model defaults survive changed catalog route fingerprints. Enabled and
deny policy apply; providers restrict worker pins, not manual coordinators.

It supports a restricted explicit worker-pin subset, with observed parameters and
explicit refusals for missing effort selection, unsupported model policy or absent
automatic ranking. It does not compose live account/quota observations, task/project
ceilings or live account/quota qualification. The configured provider separately
checks executable access and builds native session routes for the desktop;
unsupported selection is refused before Start. Legacy-owned projects remain fenced. Focused no-paid checks: `cargo test --locked --offline -p
workspace-service --test native_routes`.

## Native session-host boundary

The additive `crates/workspace-service/src/native_sessions.rs` API holds the Core
project lease and uses one writer executor for durable channel issuance,
provisioning, capacity, task actions and question decisions. Coordinator/worker
processes use `cortex-engine`, the ancestry-bound MCP broker and a concrete Core
channel provider. Tool calls recheck authority inside the write transaction;
chat and turn termination never become lifecycle authority. Worker questions,
execution/review results, validated plan intake and informational coordinator
answers are advertised. Plan admission checks the bounded DAG, unique ids,
reserved/known roles, canonical private-safe scopes, ordered read-only outputs,
owner denials and literal verification argv with an availability probe. The
accepted revision, pending nodes, events and retry receipt commit together.
Execution results publish complete inline Markdown through Core's private draft
and immutable report journal. A persisted full-payload intent precedes file effects;
changed retries conflict, and a failure after publication reuses that report on an
exact retry. Report integrity is checked before the result event, node status and
receipt commit together. Full result summaries are retained without truncation.
Passed-check claims and by-reference report submissions remain refused until native
worker command evidence and explicit report-publication handlers are implemented.
Review results now bind a fresh host project observation and an independent
review attempt. Drift durably rebinds/refuses the verdict instead of accepting a
stale review. Native engine permission requests are cancelled, not approved by
a placeholder policy.

The trusted embedding calls `NativeHost::advance` after a settled turn/process wait
or a service reopen. It uses the existing Core scheduler and review/fix budgets,
claims durable assignment identities, and returns dispatch descriptions; it never
selects an unqualified production route. A pending, unprovisioned review can
resume with its same identity after reopen. An unfinished, provisioned node whose
owned process wait was retained as terminated uses Core's bounded retry/backoff
and a fresh assignment/session; the stopped-review test resumes through completion.
Reported workers still hold capacity until an owned wait. Ambiguous/live processes
are not automatically relaunched.

After approval (or Core's observed no-change waiver), the host runs each planned
argv directly, with no shell wrapper, in the read-only Landlock command profile:
the real host readable except the owner's secrets, the project's secret files and
the live Cortex stores; writes only to a private scratch, eligible ignored
directories and package/build caches; the network shared. It fails
closed on unsupported hosts with `native_verification_sandbox_unavailable`,
including failure to enter containment after preparation. Plan preflight preserves
that diagnostic; it does not misreport a missing executable or retry unsandboxed.
Tests exercise the real journey when containment works and assert plan refusal
without acceptance when the required capability is unavailable. Long project and
TMPDIR paths are covered: the broker retains its private filesystem socket but
addresses it through an owned parent-directory fd under `/proc/<pid>/fd`.
This is not parity with Python's ignored-cache overlay.
Each command has a 60-second deadline and a 1 MiB bound per output stream.
Truncation, timeout, signal or nonzero exit cannot pass. Intents precede effects;
unsettled verification after restart remains unknown rather than rerunning.
Receipts bind exact argv, owner epoch, revision and reviewed digest. Failed
verification returns through Core's bounded replan path. Reconciliation derives
synthesis from retained summaries and rechecks report integrity, open checks/input,
project identity and confirmed process exits before completion.

The API supports owner cancellation, pause and version-checked resume, and worker
question-answer delivery to the same bound session. A claimed answer is not
replayed after uncertainty. Pause revokes old channels; resume requires a new
continuation rather than reviving the old engine. Capacity remains held until
an owned wait confirms process exit; unknown effects remain ambiguous.

Native sessions compose the canonical coordinator and all 23 worker profiles from
an embedded generated bundle. Required shared skills are inlined, conditional
skills/references remain available through role-authorized `read_skill`, and only
Codex receives the schema-derived served-tool supplement. Check freshness with
`scripts/cortex-xtask.sh generate-instructions check`; regenerate with `write`
after canonical profile/skill edits. This is build-time data generation, not a
dependency of any kind during native session creation.

`NativeHost::bind_workspace` is a trusted hook for an already-created native task,
not a model tool or desktop Start implementation. It checks project/actor/root,
the native creation marker and owner epoch, and commits a unique execution binding,
a separate logical display session and public outbox entries atomically. Exact
rebinds return the existing identity; conflicting, legacy or stale bindings refuse.
Actual child ACP sessions stay bound independently in provisioning records. Native
messages and minimal tool title/status activity are copied into the logical
session's history with attempt-namespaced tool ids. Thoughts and raw tool
inputs/outputs are excluded; bounded message/title previews mark clipping.
Committed history changes emit the existing UI deltas. A settled reply drains prior
queued updates before returning, without turning chat into execution authority.
The fake-engine test observes these projections and their persistence after reopen;
it is not a desktop command-path or automatic restart-recovery qualification.

### Qualified native desktop Start

The ordinary `Service::workspace` command handler now routes `tasks.start` to
a service-owned native controller when a trusted embedding provides qualified
routes through `Service::bind_native`. Selection precedes scheduler construction;
existing format-12 Python-owned tasks are adopted by native recovery under the
single-owner fences, retaining their history and unknown-effect barriers.
Native errors never fall back to the retained mutation adapter.

Core validates the activated project/actor, draft version and null initial epoch,
then commits native runtime creation, owner epoch, saved input/options event,
logical execution/session, Start intent, immutable receipt and outbox together.
Engine provisioning happens afterward. Admission and the serialized progress pump
remain owned if the request/window disappears. An exact retry returns the same
receipt without another prompt; changed payloads conflict. The pump settles each
owned process before advancing through worker, independent review, contained host
verification and Core completion. Errors retain a diagnostic and stop rather than
inventing a result. The existing versioned native deltas carry the final task state.

The production service launcher binds `ConfiguredRoutes` through `Service::bind_native`
by default. It uses the same channel-isolated HOME/XDG settings roots as the retained
runtime, without discovery subprocesses, login or settings writes. Start preflights
coordinator, general execution and independent-review routes; other plan roles are
requalified when dispatched. Exact defaults and explicit role/alias/wildcard pins
are supported. Automatic ranking and nontrivial model policy remain explicit
refusals, not a first-catalog-entry fallback. Codex, Claude and OpenCode approved
commands are checked for executable access without a model call. Only absolute
PATH directories resolve approved bare names; project-relative commands refuse.
Selected observed parameters are applied and confirmed against live ACP session
options before prompting. Credential-directory presence is not fresh login evidence.
The service binary itself provides the authenticated MCP stdio relay.

The retained-controller fixtures, the emergency `CORTEX_WORKSPACE_NATIVE=0` retained mode and the legacy ACP child were removed on 2026-11-16; there is no opt-out, and socket/webview fields cannot select another host. The built-binary no-paid test launches the actual service with
private HOME/XDG/store and a PATH fake engine, then creates/starts through its socket
and observes native ownership, independent review, verification and completion.
This does not qualify real accounts, rendered desktop behavior or an installed archive.
Missing first-run config/state directories report a missing-policy refusal without
creating files. Machine-facts assembly keeps its own configuration snapshot across
project reads, so concurrent activation or settings-save cache invalidation cannot
turn that read into a null-configuration failure.
Candidate route binding alone is not live account authorization. A draft saved with a
model (route id or model name), effort, access mode and language starts like the Python
adapter's workspace Start: the host qualifies that choice with the per-task coordinator
route before admission, Core records it as the task's initial settings and switches the
task access mode, and coordinator turns use it. An unavailable model refuses with
`native_task_route_unavailable`, an effort the route does not advertise with
`unsupported_dependent_setting`, and malformed saved settings with
`native_task_settings_invalid`, all before any task is admitted. Sprint tasks inherit the
sprint's settings and start the same way. Creation-time attachments are retained with the task and delivered to its engine sessions. The desktop service advertises `taskWorktrees: true`. The composer and Edit task require explicit support to enable **Separate worktree**; the standalone incoming ACP host does not admit linked checkouts.

The no-paid `cargo test --locked --offline -p workspace-service --test native_desktop`
test enters the actual command handler with Rust fake coordinator/worker/reviewer
processes, a disposable store and real bounded host verification. It asserts
completion, execution identity, a versioned completion delta, transaction rollback,
exact replay, changed-payload conflict and unqualified-route refusal. Through the
production configured route provider it also starts a single task and a sprint task
saved with Claude `claude-sonnet-5` or Codex `gpt-6.1-sol` and a high effort, checking
that each coordinator is configured with that model under the engine's own effort
option id, and refuses an unavailable saved model and an unadvertised effort. Its retained
launcher is `/bin/false`: successful activation and zero children establish no
Python controller launch or mutation on that native path. This is source/protocol
evidence, not rendered GUI, installed archive or real-account qualification.

### Legacy task adoption

Native workspace-service activation opens existing format-12 stores under the
shared scheduler fence and the Core lifetime fence before any recovery writes.
For each nonterminal Python-owned task, one transaction advances the CAS owner
epoch, records `NATIVE_TASK_ADOPTED`, revokes old channels and moves matching
desktop execution/wait bindings to the new epoch. Original `TASK_CREATED`
provenance and logical session/execution identities remain unchanged. Reopening
does not adopt again; terminal histories and explicit pauses are not resumed.

A finished (completed, cancelled or failed) Python-run desktop task is adopted
the same way when its execution binding is otherwise valid, recording
`NATIVE_TASK_ADOPTED` with `finished: true` and replaying nothing. Without it
every owner action on such a task (Request changes, a message, Continue, Retry,
Close) was refused `native_execution_binding_invalid`. Its finished result is
presented read-only as a result from an earlier version of Cortex, with its own
result identity (the task's latest event) and a placeholder digest, never
acceptable, with Request changes as the way to a fresh result; a closed card
reads "This task is closed." and a cancelled task "This task stopped without a
result to review." instead of "Wait for Cortex to finish this result."

Activation never fails because of one old task. Recovery settles the retired
host's leftovers (process records without an end time, never-started worker
rows, rows of tasks that have no runtime record) only from durable evidence and
isolates each task's recovery, as described in
[recovery](../features/recovery-and-replacement/index.md#native-attempts-left-unconfirmed-by-an-earlier-host).

Settled planning, execution, review and verification checkpoints continue through
the native driver. Pending coordinator questions wait for the owner's answer;
settled worker questions continue on a fresh assignment/session with retained
answers. Retry backoff remains in force. Claimed deliveries are orphaned, never
redelivered. Unreleased capacity, nonterminated provisioning, unmatched legacy
command/check starts and unfinished native effects retain explicit unknown-outcome
recovery barriers. Acquiring a scheduler lock alone never proves an old process or
effect terminated. Older installed runtimes that ignore the cooperative fence
remain outside this guarantee; this is not engine-session reattachment.

No-paid tests seed Python-style format-12 tasks and exercise service activation,
owner-epoch rejection, question list/answer controls, independent review and
verification with fake engines on disposable stores. Namespace-capable execution
is required for positive command journeys; a namespace-refusal branch is not a
passing adoption journey. This does not qualify an installed archive or real clients.

### Native desktop task controls

For native-owned executions, the ordinary `executions.prompt`,
`executions.cancel`, `executions.control` and `decisions.answer` handlers now
reach the same NativeHost without a retained-child mutation. Adopted legacy tasks
use their existing desktop identities with the new owner epoch.
Native binding checks include the registered
project/actor/root, exact execution and logical session, native creation/adoption marker
and current owner epoch. No route supplied by desktop JSON authorizes dispatch.

Stop and Pause establish a resumable PAUSED hold and wait for the owned pump's
process shutdown before replying. Resume requires the current displayed task
version. Resume continues the retained task from the saved state: it supersedes
nothing, keeps open and ready worker waits and pending questions answerable, and
re-enters the driver (lost-worker retry for interrupted workers, coordinator
recovery for an interrupted planning turn). A Resume that refuses (for example
`native_shutdown_unconfirmed`) commits nothing and leaves the task paused, and a
recovery barrier recorded while paused (`native_session_outcome_unknown`) is
never erased silently: the resumed task continues as a continuation that carries
that uncertainty ([no-stop policy](../features/task-lifecycle/index.md#no-stop-policy-and-the-strategy-ladder)). Explicit `retry`, and an
owner message to a task whose coordinator waits for the owner, is held in
recovery or was reopened, settle the old process, retain its prior revision/results, supersede
obsolete questions and start a fresh coordinator/plan. None of them revive a
revoked channel. An owner message (Send now, or a queued message) to a working task,
also one whose worker asked the owner a question, settles, cancels and interrupts
nothing: the running driver, workers, review and verification continue, the worker's
question stays answerable and the task keeps its state, so its receipt does not wait
for any process. The message is recorded once and reaches the coordinator's next turn
at a safe boundary, in order, and is recorded delivered once the engine answered that
turn's prompt (see [steering](../features/steering-and-questions/index.md)). A queued
message stays in the machine's queue, editable, movable and removable, until the
service takes it at such a boundary (window open or closed); it then reads "Taken from
the queue for delivery" and refuses changes (`message_taken`). Send now on a queued row
records it at once. The task's Work details list the owner's write denials in force (path,
when set) with Remove, which lifts that one denial (`executions.control`
`allow_write`). An admitted control whose finish receipt never committed is
closed as an explicit `outcome: unknown` receipt carrying the task revision; the
same request keeps replaying as `native_control_outcome_unknown`, the task
continues as a continuation that records that outcome (a paused or waiting task
keeps it as its barrier), and later restarts do not raise it again.
A follow-up or incoming start uses the task's own coordinator route (explicit
choice, else the route pinned when the task started); only a task with neither
needs a resolvable owner default. A failed first prompt keeps its draft and a
retry of the same text reuses it. Steering text is retained
in the event stream and display history. The desktop shows an owner message, a
queued message, a worker message, a planner message and a question answer from
the press, before the service answers, and drops that local copy once the
service's record reads in its place: a saved, queued or worker message carries
the send's `requestId` as its id; a history `user` row carries no request
identity, so it is matched by text among the rows that arrived after the send.
A refused send keeps its text and files on its line with Retry; the local copy
is display only and never stands for a recorded message. Attachments of an owner message are validated before admission and retained with it (see Attachments); an invalid file refuses before mutation. Retry is allowed only for waiting/blocked/recovering work, not a
completed task. An ordinary owner message (Request changes) can reopen a
COMPLETED or FAILED result that is still open. A plain late message to a closed
or cancelled execution still refuses with `task_not_resumable`; use the separate
[Return for rework](#return-for-rework) action.

Questions are projected through bounded, execution-version-bound pages (up to
100 items per page, with an explicit refusal above 1,000 retained questions).
Answers validate the complete batch, question/options, versions, epoch and session
in one transaction. The existing wait claim delivers to the same waiting worker
once. Unknown delivery stays unknown, rather than automatically resending.
A durable authenticated result from that exact attempt after the claimed delivery
can settle an ACP reply lost because result publication revoked its channel.

Confirmed `mark_resolved` / `close_task` closes unfinished work without certifying
it: Core retains CANCELLED, and a durable owner-resolution marker maps the UI to
Done / Closed as is / Not verified by Cortex. It is not runtime-certified
COMPLETED. Closing an accepted result (or one Ready on its own) as is keeps it
checked: it reads Closed as is, accepted v<n> (or verified), and only an unchecked
result reads not verified. A task closed as is leaves its changes in place, so its
Changes tab keeps them: a worktree task's own diff, or, in the shared project
folder, only the files its workers recorded writing. Closing a COMPLETED result records a certified owner resolution. Stop,
steering, answers and owner resolution all invalidate the native outbox and feed
versioned UI deltas. As in the Python adapter, the UI then records the card's own
disposition: `product.task` `close` (Done, with the chosen close kind) after Close
task, and `reopen` after Request changes. These are native Rust overlay actions,
not retained-helper mutations.

A COMPLETED native result is presented with the same review actions as Python's
`result_projection`: `reviewActions` and `acceptanceAvailable` come from the
report, review and runtime-verification evidence, and the result identity
(`sourceVersion`, digest) is the one Accept and owner check reruns validate. Native
verification records the native verifier's digest of the project folder, so the
evidence is bound in that domain, never in the Python `capture_revision` domain.
Runtime verification and independent review decide acceptance: once the runtime's
own verification of the current version passed on the unchanged folder, a
worker's or reviewer's self-reported check that is only `unverified` (for example
`revision_mismatch`, run before the last edit, or `trivial_command`) or `not_run`
is shown on the result card as a note ("The runtime's own verification of this
version passed. N self-reported checks it does not cover are shown for information
and do not block acceptance:" followed by up to ten named checks). Such notes need
no acknowledgement and are not counted as open checks. A self-reported `failed`
check, a policy-blocked one, a failed or missing runtime verification and an
unapproved review still block Accept (`crates/workspace-service/src/machine/task_facts_work.rs`). A failed worker check whose withdrawal completion confirmed (a probe withdrawn after its run) is omitted again on the result card when the runtime re-derives that withdrawal from the run's recorded output: a native output is kept under the channel that ran the command, and its stored file must still hash to the digest completion recorded. On a native task a check that still blocks is named in the reason ("Worker check “…” failed. Rerun that check or request changes. Only a recorded technical blocker may be acknowledged."). A verification the native runtime recorded is compared with the native folder observation only; an earlier-format digest is taken only for a result verified before its task was adopted.
Changed files after the result disable Accept with the Request changes action.
For a task of a sprint that works in the project folder itself, later tasks write
the same folder, so its result is compared on the files it recorded writing: while
each of them (the latest Cortex write or reconciliation of that path before the
verification) still holds exactly that content and none of the task's commands is
left without its recorded end, the result keeps its verified identity and is not
stale. A change to one of its own files makes it stale; outside a sprint, or in a
worktree, the whole folder still counts. Commands' and engine-native writes that
Cortex did not record are not part of that comparison, just as they are not part
of the files the app commits for a project-folder task. A result the app itself
committed (`work: merged`, an `integration` of method `commit` whose digest is
the result's) is not stale merely because the commit moved HEAD.
The desktop's Accept and check-rerun commands use the result from the machine
facts shown on screen, ahead of the raw wire result. This also works when the
wire result adapter is absent or carries a different digest. An explicit empty
result clears the old identity rather than falling back to it. Accept still
requires a source version and matching displayed digest; the service revalidates
current evidence before saving the decision. Mock-transport screen tests exercise
the actual Accept button and its exact outgoing identity; these are not
installed-desktop or paid-engine qualification.

The same `native_desktop` no-paid test now covers each control through the real
service handler, same-session question continuation, Stop-to-Resume completion,
fresh Retry dispatch, non-certified closure, stale versions, exact replay and
versioned terminal deltas with an impossible retained launcher. Port tests cover
the close/reopen card dispositions after native controls, Core tests the reopen
and worktree refusals, and a service unit test accepts a seeded completed native
result and refuses it after the files change.
Complete report/tool parity, full permissions, production route/instruction
qualification, restart recovery and installed-package rebuild/real-input
qualification remain unfinished.

The five additional native owner controls now have real effects:
- `worker_message` binds a live assignment/attempt, records guidance and queues it
  to that worker. It waits for the current ACP prompt's cancellation reply before
  prompting the same session. A waiting-question worker can also receive guidance.
  The receipt says queued, not consumed; a completed prompt marks delivery, and an
  unknown or abandoned delivery stays unverified. Worker guidance carries no files: attachments refuse with `native_attachments_unavailable` and the worker composer says so.
- `worker_stop` revokes only the exact bound assignment before settling its process
  and capacity. Independent siblings continue; no-runnable-work handling leaves an
  owner wait, never successful completion or an automatic retry of the stopped work.
  Native task sessions currently run sequentially; other tasks are unaffected.
- `command_stop` requires the exact live host-verifier reference and a reason.
  The owned process group is killed/reaped and its incomplete receipt cannot pass.
  Unobserved engine-native commands remain unavailable. A separate stop lane also
  permits cancellation while an owner check-rerun handler is waiting.
- `rerun_check` requires the current stored check name/index, result version and
  observed project digest. Only retained current-revision argv can run, under the
  same bounded read-only sandbox. A fresh receipt replaces that displayed check
  observation; the rerun is not task certification or an acceptance bypass.
- `task_settings` validates and persists task-local next-use settings. Coordinator
  model choices stay on the approved default connection and existing eligible
  catalog; effort must be observed/supported. Response language applies to later
  sessions. Guarded is the only supported access mode; Development/Auto refuse.
  Existing live sessions are not silently reconfigured.

Native `product.read`, `product.facts` and `workers` presentation reads use the
same Rust store for these settings, guidance, worker states and host command
observations. They start from the private overlay merged with each task's
store-owned options (checkout with the checkout's current branch, settings, base
revision, saved draft title) and the local integration facts of tasks in Ready to
merge, exactly as the helper read does; a task created after the overlay was last
written is therefore presented with its checkout (a project-folder task stays
Ready to finish after a relaunch), never as a record without one. Host check
receipts are those of the current result only: after Request changes a task keeps
its plan revision until its next execution baseline, so receipts recorded before
its latest `TASK_REOPENED` are not presented, and a reopened task shows no result
(and no Accept) until its next run records one. Workers of an earlier version (a
node revision older than the plan's, or not newer than the latest reopen) carry
`superseded: true`, and while a reopened task has no result the Inspector counts
only the current version's worker checks. A worker's page lists, besides its
Cortex commands and edits with their receipts, its other tool steps (reads, skill
reads, searches, Git and engine tools) from the same retained display rows and
`Workspace step` selection the task timeline uses
(`cortex_core::projections::tool_step`), bound to the worker by its attempt; its
Cortex commands and writes are not repeated as steps unless refused. Rows carry
their recorded time and are ordered by it. A worker read returns the newest 200
rows; a cut sets `activityPartial` and an `activityCursor` (`na1.<time>.<sequence>`,
or `na2…` when the worker's queued messages continue separately), and a read of
that worker with the cursor returns the next older rows (and older queued
messages in the worker's `messages`), so every command, edit, tool step, note and
message stays reachable; narration, edit-diff and step-text byte budgets only
shorten a row's content. A reply over the frame moves the oldest rows of the
largest worker behind such a cursor; it is refused only when nothing more can be
moved. A task with many tasks, workers or guidance messages is never refused: the
reply names a cut under a top-level `partial` object (`tasks`, `workers`,
`size`). A coordinator page (`coordinator:<task>`) holds only the coordinator's own
session rows, paged by its own cursor; rows whose message or tool-call identity
names a worker attempt belong to that worker's page, and so does a command run on
a worker's channel (a command on the coordinator's channel appears on neither
worker page). The Conversation omits worker narration and worker tool calls: its
history read (`history.page`, `cortex_core::projections::history`) looks a row's
attempt up under the execution's runtime task (`workspace_executions.runtime_task_id`,
where `delegation_provisioning` records attempts), never under the workspace task
it is asked for. Its live line speaks for the coordinator: while workers run it
reads "Waiting for <role>…" (or "Waiting for N workers…"), and a worker's running
command, edit or read shows only on that worker's page. What needs the owner stays
in the Conversation whatever session it came from: questions, open or answered,
are read from the task's decisions (`decisions.page`, by the runtime task), not
from any session stream, and a worker's question carries `askedBy` (the worker
role recorded for its assignment) and reads "Asked by <role>". The result and
review entries come from the task's results, and the coordinator's own tool calls
that start, wait for or stop a worker stay coordinator rows.
A command run through the broker (`command_run` or the approved `outside_command` route) is
linked to its retained output without putting the output in history. The broker records, in the
Workspace-only display history, a small start and settlement marker holding status, identity and a
tool label (no argv, cwd, output or large-output ID); the Conversation row, the worker page and the
coordinator activity carry the resulting reference (`commandOutputRef`: attempt and call), and
history pages keep their existing budgets. The owner read is the local machine operation
`command.output` (port method `readCommandOutput`), keyed by project, Workspace task, attempt and
call (`native-command:…`); a bare output ID never selects output. The service resolves the
Workspace task to its runtime task and requires one broker start and one matching broker request on
the same channel and attempt, and an output file belonging to that task and channel. It opens the
file read-only and checks the expected name, size and SHA-256 before returning bytes. Omitting
`offset` and `limit` returns a 4,096-byte preview; `offset` (zero or more, end of file allowed) and
`limit` (1 to 16,384, default 16,384) page it. The reply carries the operation, status
(`running`, `passed`, `failed` or `stopped`), argv, cwd, exit code, signal and failure, and an
`output` of `pending` (still running), `unavailable` (never captured, removed or failing the
integrity check; no bytes are delivered) or `available` with the chunk, `utf-8` or `base64`
encoding, offset, byte length, total bytes, `nextOffset` (null at the end), and the `truncated`,
`captureComplete` and `complete` flags. Unknown request fields are refused. The read is registered
only on the local machine path: the desktop bridge allows it, the phone policy and the remote
allowlist refuse it, and it is not a model-facing tool. Host verification and `project_git` rows get
no link, and an engine's own shell call has none either. It adds no retention: it reads output the
broker already kept.
The coordinator's own message text is recorded in full around its question: every
chunk the engine emits, including chunks processed after `user_question_request`
committed `WAITING_USER`, is kept in order (`record` in
`crates/workspace-service/src/native_activity.rs`), so a message is never cut short
at the question.
Check receipts of the current result are read newest first up to 1000; beyond that
the result is marked `checksPartial` and Accept is refused (presentation and the
Accept command alike) with a readable reason, never decided from part of the
set. A worker last seen running under a task that has since ended is shown as not
confirmed and never counted as running; a failed refresh names the time of the
data still shown. Task deltas trigger the existing presentation enrichment. Check presentation does
not advertise acceptance. The `native_desktop` fixture positively exercises all
five handlers with real fake-engine processes, temp stores, UI deltas, exact
replay/conflict checks and an impossible retained launcher. Worker-stop coverage
uses an attempt identity, checks the stopped worker's confirmed exit/capacity release
at the stop receipt, and holds an independent sibling's ACP reply after its result
commit. Global cleanup assertions wait for that sibling's retained process-exit
observation, not merely its completed node. Service teardown separately joins its
cancelled supervision/event tasks before returning, allowing the endpoint lock to
be released when the caller drops the service. Command-stop coverage also stops an
owner rerun while that handler is waiting. Separate port/domain
tests cover native check forwarding and non-certifying presentation overlays.
For read-only/no-namespace fixture runs, build the test executable first and run it with a short `TMPDIR`
(the Python wrapper that selected one, `restricted_native_checks`, was removed). A short temporary prefix leaves room for the desktop service's Unix socket
namespace; it does not change the filesystem restrictions. Other
executables should keep the default long prefix for broker long-path coverage.
The native API must not open beside another owner.
Updated production legacy ingress and native executors share a separate
process-lifetime `.scheduler-owner.lock`. Conflicting owners refuse with
`scheduler_owner_busy`. Bare NativeHost opens still refuse unadopted legacy tasks
with `legacy_runtime_tasks_active`; service recovery is the explicit takeover path.
Marked or adopted unfinished native tasks refuse legacy startup with
`native_runtime_tasks_active`. Failed or cancelled/completed histories
are terminal for this admission check; their presence does not block opening the
other host. New native task creation records its host marker transactionally.
The legacy facade serializes first-use
runtime construction and retains its guard during ambiguous teardown; native
session references keep the writer lane and guard alive after the host handle
is dropped. Older installed runtimes and direct test adapters do not honor this
cooperative fence. Qualified native command composition uses it; production launch authorization remains gated.
The trusted host's result-receipt query supports reconciliation after worker
revocation or a service reopen. It checks task owner identity/epoch and does not
replay publication, relaunch a worker or certify the task. A reported worker's
capacity remains reserved until its owned process wait confirms exit.
The no-paid native-session tests use disposable stores and Rust fake ACP/relay
processes, not an installed app or a real engine account. They cover
create → plan → worker result → independent review → actual sandboxed verification
→ completed, pending-review reopen, a scoped review fix loop, failing verification
and truncated-output rejection. Observer bounds currently refuse symlink/special
files, files above 16 MiB, trees above 64 MiB, lists above 100,000 files or bounded
Git output overflow; such refusals are not successful observations.

## Return for rework

The owner can return closed work for rework, including accepted, closed-as-is
(including unverified), integrated, cancelled and failed work. Restore archived
items first. Restore does not run them; a successful Return for rework lifts the restored hold.
Closing without verification does not itself prevent rework, but
the service must confirm the exact execution and owner. A conflicting binding,
an unconfirmed worker termination or an unresolved external effect still blocks
the operation. A draft closed before its first run can start through this action.

Choose **Return for rework** and submit a nonblank owner message (up to 32,000
characters) with optional attachments. The service checks the current task
version and, for existing executions, the owner epoch. For a bound execution,
Core records `TASK_REOPENED` with origin `owner_rework`, retains the owner
message and returns to planning in the same transaction. This clears the old
active synthesis, blocker and hold. The task leaves Done and its old result and
acceptance are no longer current. Prior plans, results, acceptance and integration
remain in history. The new plan goes through the normal work, independent review,
runtime verification and owner acceptance rules; a direct answer cannot bypass
planning. The next result after v<n> is v<n+1>; a first result is v1.

For integrated work, rework starts from the current local head of the target
branch, not the previous task branch. A worktree task gets a fresh owned
worktree and branch; a project-folder task stays in place and requires that the
target branch already be checked out. Workspace does not switch that checkout
or remove its changes. The old integration is not undone: integrate the new
result separately after review and verification. The old owner close does not
prevent automatic acceptance of a freshly verified result. If the owner closes
the new result as is in an Automatic sprint, a project-folder commit includes
only paths attributed to this cycle; after unmerged rework, matching retained
write bytes may also be included. Previously confirmed pull requests remain in
history. A new owned branch needs its own confirmed Push and PR; an unresolved
previous PR attempt still blocks another PR.

## Recorded data and pending questions

An action receipt acknowledges the service reply, not completion of a later display refresh. Task and sprint actions reconcile their own project in the background and show Updating confirmed state until that read settles; a failed read retains the previous view and offers Retry. A newer command reply fences pre-reply reads. Confirmed renames and queue controls can display immediately, but a failed or unknown action never applies its requested change. New/activated projects still reread registry and capabilities. Product and worker facts share one helper call; Git inspection overlaps rather than serializing those reads.

A confirmed draft rename also returns the SQLite task version committed by that
write. The port keeps it for Start immediately, without awaiting the board refresh;
an older in-flight snapshot cannot replace it. The service still checks the actual
current version, so a competing edit may legitimately refuse Start without replay.

Machine connectivity and task process confirmation are separate. An unconfirmed task on a connected machine shows one notice with one icon, explains that the machine is connected but cannot confirm whether the task is running or stopped, and offers **Re-check state**. The composer stays read-only and points to that control, not a fictitious reconnect requirement. Re-check rereads the existing projections; a still-unknown reply stays blocked and points to Machine settings → Diagnostics. A failed read keeps the task and a retry explanation. No task is resumed, stopped or replayed by this read.

An unconfirmed task never leaves the owner without a decision. A native `RECOVERING` task with no recorded reason is unconfirmed only while one of its owned processes is not confirmed gone, and its projection names which case holds:

- `process_exit_pending`: a session of the task is still being stopped (its delegation or capacity reservation is still live). Its own shutdown observation settles it, and a settled reservation republishes the task (capacity reservations are hooked into the change outbox), so the task becomes an ended turn by itself. The conversation shows one quiet line and asks nothing; an Automatic or Hybrid queue waits for it without a hold. When a replan cancelled a live worker and no `LIVE_CANCELLATIONS_SETTLED` covers that cancellation yet, the projection also carries `cancelled_worker_stopping`, and `waitReason` names the still-live cancelled roles as `cancelled_worker_stopping:<roles>` (comma-separated, without `worker-`); the hold is unchanged, and the quiet line reads "Waiting for <role> to stop. The coordinator cancelled it to change the plan."
- `process_exit_unconfirmed`: the host recorded that the process did not confirm its exit (delegation and
  reservation `ambiguous`). The running service checks it again by itself on its scheduler passes (on the
  30-second safety tick and on task changes, in any project that has such an exit), with the same process
  evidence startup recovery uses: when no engine process works in the project or a task folder any more,
  it settles the attempt exactly as a restart would (delegation `terminated`, reservation `released` with
  observation `no_engine_process`, one `NATIVE_PROCESS_SETTLED` event with evidence
  `unconfirmed_exit_and_no_engine_process_in_project`) and the task is republished as an ordinary
  interrupted turn. It never signals or relaunches anything, only re-checks attempts bound to the task's
  current owner epoch whose every unsettled row is a recorded unconfirmed exit, and backs off while the
  evidence stays inconclusive (15 seconds, doubling to 10 minutes; a newly unconfirmed exit is checked at
  once). In Automatic, a task that only that exit held (stopped with no other reason, or paused only
  because a process exit was unconfirmed) continues by itself in the same pass, as an automatic
  continuation that never replays an unknown effect or waives review or verification; any other task waits
  for its owner as an interrupted turn. A service start still applies its own recovery rules (a later boot
  also settles it). While the evidence stays inconclusive the conversation pins a decision card: what
  happened in plain words, **Re-check state**, **Open workers** and **Open Diagnostics** to inspect it,
  and **Close as is…** with its confirmation. Closing commits the cancellation, stops what the service
  still runs for the task, records that a process was unconfirmed and repeats nothing. A message or **I
  checked it, continue** would be refused while the exit is unknown, so neither is offered, and the
  queue's hold for such a task offers only **Close as is…**.
- Records that disagree (`contradictory_evidence`, for example an execution bound to a different owner
  epoch or session) cannot be used to control the run. Use **Re-check state** or **Open Diagnostics** to
  inspect the disagreement, or confirm **Close as is…** to close only the Workspace task. The close
  neither stops nor resumes a process, repeats nothing, and keeps the conflicting records. The task reads
  **Closed by you · Not verified**, keeps a Diagnostics link, and no longer asks for a decision after
  restart. Ordinary task changes are refused; the Board and List group the task under **Closed, not
  verified**, outside the **Done** count. The close does not satisfy sprint dependencies. A generic
  unavailable read or an unconfirmed state without an explicit records conflict does not grant this
  action.

A matching bound `RECOVERING` / `recovery` / `recovering` projection with `waitReason: turn_interrupted` and no contradictory-evidence warning is different: the cancelled turn is waiting for an owner message. It shows **Interrupted** under Needs you, with **Send a message to continue from the recorded state**. Re-check installs that same recorded wait and restores messaging; it does not itself continue work. The generic Working indicator and Mark as handled card are absent. The task is not declared physically stopped, so process-quiescence guards remain in force. Missing bindings, mismatched runtime facts and other recovery states stay unconfirmed.

A bound `EXECUTING` or `REVIEWING` projection whose next step is queued behind another unfinished task's files carries the `waiting_for_files` warning and a `waitReason` sentence naming that task and the files. Its Working card and opened task status strip read "Waiting for the task “<title>” to release <files>". The title matches the holder's card, using its description label if it has no title. The strip links to the holder when its page is available in the same Workspace project; otherwise the recorded reason remains plain text. The step starts by itself when the holding ends, and Stop and Pause stay available. Nothing asks the owner.

Only a task that waits on the owner (Needs you, or a merge conflict) shows the **Waiting for you** card; a cancelled or failed run has no decision and shows none. **Mark as handled** appears only where the machine offers it. A task closed with **Close task** reads as closed (Done, not certified) from the cancellation the close committed, even when an earlier host refused its finish receipt because an engine exit was unconfirmed.

Coordinator messages and selected tool steps use the retained public `history.page`
sequence, not the coordinator's one-line status note. Available history rows
also carry worker-shaped `activity` (message, note, tool, command or edit) when
that convenience copy fits the page. Otherwise the readable text remains and
the helper reconstructs the activity. Long activity previews are explicitly clipped;
the public row retains its text within the existing wire limits.
Pages bind the project, task, session and page size, freeze their high-water mark,
and survive window reopen and helper restart. Runtime bookkeeping and thoughts
are hidden without deleting source history; hidden-only pages still advance.
Oversized or missing display content remains explicitly unavailable.
The window reuses complete conversation histories only while project, task version and execution identity match; transactional history invalidations cause a fresh read. Page construction validates each row and the final page without revalidating every growing prefix. Working indicators retain their icon and status text without continuous animation; slow Git completion publishes updated facts rather than triggering another full task read.

The machine `workers` helper accepts `workerId: "coordinator:<taskId>"` with that
task in `taskIds`, returning a coordinator detail with the same activity and
continuation fields as a worker. It reads the same public history projection.
Opening a worker activity view automatically reads each available continuation page, without a Load older control. A failed page keeps loaded rows and offers Retry; new live-tail rows remain retained for the same attempt. Existing keyed rows, collapsed tool groups and scroll anchoring are reused (the activity stream has no virtualizer). Project-folder checkout labels are separate from branch names; the project title chip uses the observed open branch rather than the Git default.
Ordinary worker lists stay worker-only. They retain each work-graph retry;
the latest keeps the assignment identity, and older attempts use their own
selection identities. `routeAttempts` additionally records each audited engine
fallback within a work attempt, oldest first.

Task list/detail `coordinatorRoute` and worker list/detail `route` describe recorded
confirmed runtime settings, separately from editable settings and machine defaults.
Worker routes resolve the provisioned engine attempt, not the work-graph attempt;
native confirmation takes precedence over dispatch intent, including narrowed or
unavailable effort. Coordinator routes require confirmed active configuration;
a pending/indeterminate application or mismatched current attempt stays unknown.
Missing confirmation gives a null route, and missing effort gives an empty string,
never an inherited guess. Recorded model names are preferred; a recorded model
without a name is explicitly marked as an alias. Transport, connection and
configuration identities are not display labels.

On the native runtime, every engine session start records `NATIVE_SESSION_CONFIGURED`
(`channel_id`, `session`, `engine`, `model`, `parameters`) only after the engine has
confirmed each `configure` value (`Engine::configure` refuses a reply whose
`currentValue` differs from the value sent), in the same transaction as
`NATIVE_ACCESS_MODE_CONFIRMED`. A native worker's `route` is
`{engine, model, effort}` from the newest such record of a worker channel bound to that
worker (`effort` from `thought_level`, `effort` or `reasoning_effort`, else empty), so
the route shown is the route its engine session runs on. It is never the
coordinator's route or the task's settings. A worker with no such record (not
started yet, or a store written before the record existed) has a null route.
The desktop labels a route that names its engine as `Engine · Model, Effort`, with
the model's name taken from that engine's catalogue on the machine. A worker
with a null route reads **Chosen when it starts** while it waits, and **Not
confirmed** otherwise. The worker card, the worker page and the inspector's
Selected worker section show only the worker's own route. The inspector's
Route and access section labels the task's route **Coordinator model**.

Task and project freshness uses recorded task/question/history timestamps, never the time a poll completed. Task freshness uses that task rather than an unrelated project refresh. Elapsed durations use one whole-second formatter: seconds, minutes plus seconds, then hours/minutes/seconds. An unreported time is omitted.

A worker can ask while the runtime remains EXECUTING for other workers. Its actionable pending question still appears in the task conversation, Inbox and sidebar; it does not imply that the entire runtime is paused. Questions asked together are answered in one form and sent by its one Send answers action as one batch answer, bound to the question versions on view; nothing waits in the app between questions. A batch that gained, lost or closed a question since is refused as a whole, and the form keeps every answer the owner gave with the reason. The answer form has no model or effort control, because the answer continues the existing session. The native window can notify outside the open task/sprint; see [notification behavior](maintenance-and-controls.md). All work lists sprint planning with an Open planning link before it has materialized tasks, rather than implying there is no work.

Weekly Claude/Codex usage comes from the runtime quota reader used by route selection. Packaged Claude discovery can use the verified SDK CLI when no global `claude` is on PATH. Claude uses named weekly windows; Codex uses the reported 10080-minute duration in either primary or secondary, never the slot name alone. Missing Codex duration stays unknown. Multiple scopes/connections use the highest reported percentage, not their sum or the last entry. Unknown usage/observation time remains unknown; OpenCode and five-hour windows are omitted. These source and fixture checks do not qualify live account availability.

## Sprint run modes

A sprint has one run mode, chosen when it is created and changed from its controls. The mode decides what starts the next task and what waits for the owner. The interface labels are **Manual**, **Hybrid** and **Automatic**, with these descriptions:

- Manual: "You start each ready task. Starting one never starts its neighbours."
- Hybrid: "The queue runs one task at a time in dependency order. You confirm each merge."
- Automatic: "Tasks go to Done on their own in the current branch. You answer work questions and permission requests, and review blocked results."

A sprint saved with the former `automatic` value (the queue and acceptance behavior without automatic finishing) opens as **Hybrid** with unchanged behavior. The first product write that persists this migration also records `sprintRunModeVersion: 1`. The three modes share the sprint's saved checkout choice and per-task options.

**Manual** starts a task only when you press Start. Each result is accepted and finished by you.

**Hybrid** runs the queue. When a task's result passes its checks, it is accepted by the automatic acceptance rules described below and waits in Ready to merge. The queue starts the next dependency-ready task, and you confirm each merge or pull request yourself. Hybrid holds its queue under the same next-actor rule as Automatic (see **Who acts next** below): a result that cannot be accepted, or a run that failed with every effect known, goes back to its agent first; cancelled and archived members no longer stop independent work; every hold names its task, and the hold clears by itself when its cause is gone. Only your own **Pause** stays until you continue the queue; a result that could not be accepted never replaces its reason or lifts it. A refused start also waits for **Try again** instead of being retried by itself. A Hybrid hold that needs you asks for acceptance with the recorded checks (**Accept with recorded checks…**), never for a merge.

**Automatic** runs the same queue and finishes each accepted result on its own:

- It runs in the project folder checkout, on its current branch, or in the one worktree a sprint shares (`checkout.shared`). A sprint saved with per-task worktrees does not start. Its queue pauses with the recorded reason "Automatic cannot start in a separate worktree. It runs only in the project folder." No fallback checkout, sprint branch or final sprint merge is created.
- A result is accepted automatically only under the rules of a manual Accept: the runtime verification has passed, no check is open or unpassed, no owner resolution is recorded, and the ordinary acceptance validator passes. An ordinary failed or missing check can return the task to its coordinator for repair. A fresh result goes through review, verification and acceptance again.
- The accepted result is committed on the same branch as a local commit of the task's recorded files, and then reads Done. The commit does not push, fetch, create a pull request or merge. A result whose recorded changes have already landed on the target reads Done without a second commit. In a folder without Git there is no commit to make: a verified, automatically accepted result reaches Done by itself, closed as is with its acceptance and checks kept (never Not verified), no repository is created, and the queue continues. Unrelated staged files stay in the index. Dependencies release only at Done, not at Ready to merge.
- If the workspace changed after automatic acceptance, the service can return the task for a fresh result. A Git failure or an unconfirmed effect can still pause the queue. The scheduler rechecks known preconditions and uses the same durable integration identity; a dispatched Git effect is never repeated. The queue never stages files the task did not record.
- A task whose target branch differs from the current branch is not committed. The queue waits for your confirmation through the existing merge or pull request action (recorded as `owner_integration`).

**Who acts next.** Every hold of a Hybrid or Automatic queue has exactly one next actor, decided in one place by the service and saved on the hold with what the owner is asked and the actions offered:

| Next actor | When | What happens |
| --- | --- | --- |
| Agent | A result that cannot be accepted but can be repaired (failed or missing ordinary checks, a changed workspace, changed task files, a result that must be refreshed), or a run that ended as failed with every effect known (for example an answer the runtime could not certify because a review or worker did not finish) | The service sends the task back to its coordinator by itself and keeps the queue running. Each distinct failure (identified by its fingerprint) is repaired once; there is no cap on the number of automatic repairs per task. Review and verification run again; a fresh result is accepted and finished by the ordinary rules. Changed task files are named to the coordinator, which must reconcile them with current files. While the repair runs no hold is saved. If the service is busy at that moment (another control, maintenance, or the service stopping), nothing is decided: the repair is tried again on the next pass, and a consent you already gave is kept. |
| Runtime | A route, quota or sign-in blocker, a conflict being resolved, a sprint task that could not be read, a session of a task that is still being stopped, a verified result in a folder without Git (it reaches Done by itself, like already landed work), or a held task with a command whose outcome is not yet confirmed (the hold reads "Waiting for an unconfirmed command of this task to settle"; the runtime settles such command receipts by itself once nothing can still run them, and the hold is decided again then; there is no time limit and the owner is not asked) | The service rechecks on every pass and continues by itself. The conversation shows one quiet line and asks nothing. |
| Owner | [Owner decisions](#owner-decisions) | The task's conversation pins a decision card above the message box as soon as the hold arrives, even before the result details load. It states the reason, lists in plain words what Cortex asks you to allow, and offers the primary action that grants it, then the separate **Close as is…** with its own confirmation. The task header's **See what is needed** brings the card into view. |

#### Owner decisions

The owner acts when the repairs are used up, a restricted check was not run, an effect's outcome is
unknown and the runtime cannot settle it (a write, an edit or a call, or a command still unsettled after
30 minutes; see below), an acceptance the finish cannot use (saved without the revision it accepted, or
made by hand), you paused or stopped the task, a process of the task did not confirm that it stopped and
Cortex cannot yet confirm it by itself (only **Close as is…**; Cortex keeps checking and the hold lifts by
itself once it confirms; see unconfirmed task state above), a prerequisite was cancelled, a different
target branch, a failed commit, or a finish precondition outside the task.

The owner actions are: **Allow Automatic to finish…** (accept the displayed version with its recorded
checks and let Automatic commit it on the named branch and close it), **Accept with recorded checks…**
(Hybrid), **Let the agent fix it** (one more bounded repair after the automatic ones, or after a failed
commit a fresh result that Automatic checks and commits again; the queue sends it by itself on its next
pass, and it applies only to the hold it was offered on), **Allow more attempts** (renews the continuation
budget), **I checked it, continue** (acknowledges actions whose outcome is unknown, without replaying
them, and lets the agent continue from the recorded state), **Resume** (a task you paused), **Try again**
(rechecks a refused start or a finish precondition), **Review and confirm integration…**, **Open the
conflict**, **Open sprint settings** and **Close as is…**. After an allow action the service continues and
brings the task to Done without further owner action when its checks pass. An outcome that is unknown is
never repeated automatically and is never handed to an agent; its card offers only **Close as is…** or,
for a stopped run, **I checked it, continue**. A normally completed Automatic task reaches Done without
any owner action.

**Unknown effects on a held task.** Before any hold is saved, the service checks the task's dispatched
actions whose outcome is still unknown (the same receipts that refuse an agent repair, more attempts and
an automatic close), for every hold code. While only commands are unsettled, the runtime acts next, as
above. Otherwise the hold states in plain words that an action never confirmed whether it finished and
that Cortex cannot find out by itself, and it offers only actions the service accepts in that state: **I
checked it, continue**, **Resume**, **Try again**, **Review and confirm integration…**, **Open the
conflict** and **Open sprint settings**, when the hold had them, and always **Close as is…**. **Let the
agent fix it**, **Allow more attempts**, **Allow Automatic to finish…** and **Accept with recorded
checks…** are not offered. The decision the hold replaced is kept on it; once the effect is settled, the
queue derives the condition again from the task, or the earlier decision returns as it was. Confirming
**Close as is…** on such a hold records each unknown receipt as settled with an unknown outcome
(`settled_by: owner_close_as_is`, never as success) and one `OWNER_ACKNOWLEDGED_UNKNOWN_EFFECTS` event, as
**I checked it, continue** does; a worker that is still being stopped refuses the close.

**Refusals on the card.** A refusal of a card action, or of the Close dialog, stays on the card or in the dialog in plain words until the hold changes or you try again; a click elsewhere does not dismiss it. The card does not offer a refused action again for the same hold; a new hold offers its actions again. Closing a task with a run first checks the saved task's close without changing anything (`product.task` `close` with `preflight: true`), then closes the run, then the task, so a refusal from that check never leaves the run closed and the task open. If the task close is still refused after the run closed, the message says so, and closing again finishes the task.

Outside a queue, a run that failed with every effect known shows the same card with **Ask the agent to try again** and **Close as is…**, and its message box stays open: a message reopens the task for its coordinator.

When a tool needs permission, its decision in the conversation names the operation and resource. **Allow once** resumes that request. A result that needs owner review appears in **Needs you**. **Open task** takes you to its conversation. Ordinary manual acceptance and integration into a different branch retain their separate finish controls.

The sidebar keeps the task title visible during recovery. A short status appears beside the title, with the detailed reason available in the task.

**Close as is** lets you end a blocked review in the task itself. The confirmation shows reported outstanding checks. Closing preserves the result and pending files in the shared folder, marks unaccepted work **Not verified**, and creates no commit. The scheduler rechecks the saved pause and can start the next ready member. A policy-restricted check cannot be launched from the result card. An unrequested paid qualification recorded only as a skipped report observation stays visible after current runtime verification. It does not add an obligation to the verification plan or count as a successful check.

**Continuations: the no-stop policy.** Every task, in any run mode, keeps working until it finishes: the runtime never pauses a task for you because of its own failures, and there are no attempt caps. When a process stop ends a task (a driver stop, a session that fails to start, a lost worker, a failed or unobserved check after its direct repairs, a blocking or stalled review, a degenerate model turn, an interrupted service), the runtime records a continuation with its reason and evidence and resumes planning without asking you, on the next rung of the strategy ladder ([no-stop policy](../features/task-lifecycle/index.md#no-stop-policy-and-the-strategy-ladder)):

- Rung 1 repairs the recorded failure with a fresh plan; rung 2 moves workers to escalated routes (a stronger model, preferably the other engine) and keeps them there; rung 3 asks the senior consultant, whose answer joins the next plan; rung 4 hands the failed work to another role or a different decomposition and moves the coordinator to another qualified model. Then the ladder starts again. The card's summary of what the attempts went to (each stop reason with how often) stays informational.
- A model that produced unusable output never runs a third time: the coordinator moves to another model and the task continues. A worker is replaced on another model or engine.
- A review whose reviewer cannot read some items with its tools, and that found nothing to fix, records those items as disclosed unverified limits; the task continues to verification and the final result names them. A review dead end that repeats on the same item takes rung 4.
- A route, connection, quota or authentication problem waits as `external_blocker`. The service resumes it by itself as soon as that route, or another qualified coordinator route, is usable. It does not log in or make a paid model call to do so.
- An effect whose outcome cannot be confirmed does not hold the task. Once every session of the task is confirmed gone, the runtime records the uncertainty (`RUNTIME_UNCERTAINTY_RECORDED`) and the coordinator re-verifies those effects from the files, Git state and receipts instead of repeating them. A command that may still run is waited for until the host settles it, however long that takes.
- A pause an earlier version recorded (`continuation_cap`, `owner_blocker`, `outcome_unknown`) is resumed by the runtime itself; **Allow more attempts** and **I checked it, continue** remain only for cards saved by those versions.

The scheduler rechecks blocked queues when state changes and on its safety tick. Cancelled and archived members no longer stop independent work. Skipping a member does not make it a completed prerequisite: if only blocked dependents remain, the hold names the task that depends on the prerequisite the queue will not finish, and that prerequisite, and that task's card offers **Close as is…** (whose dialog also offers Cancel). Holds that require you surface as paused-sprint notifications when enabled; a hold the service or an agent resolves, and your own **Pause**, do not notify. **Continue queue** is offered exactly when the hold offers **Try again**; no control overrides an unknown effect, failed verification or manual pause on a task. A hold saved by an earlier version is rewritten with its decision on the next scheduler pass.

A failed or not-run mandatory check is never waived. A repair plan that drops a failed required check is refused. Review and verification remain required, and a continuation grants no new permission.

**Changing the mode.** Switching to Manual stops the queue from starting further tasks; results already accepted stay accepted. Switching to Automatic accepts waiting results that pass. A mode change clears the queue's previous automatic pause. A task that has already started keeps the policy it was started with.

**Policy binding.** The continuation policy is a trusted runtime option. The scheduler binds it when it starts a task of an Automatic sprint. Commands, plans, model tool calls and wire fields cannot set, change or remove it, and the scheduler-only operations that finish and advance an Automatic queue are not reachable from ordinary requests.

**Limits.** Fixture-based WebKitGTK checks cover the acceptance hold, its task link and the close dialog. Full live desktop qualification of Automatic remains outstanding. Real provider authentication, quota recovery and a native GUI run are not observed here. Builds that predate Hybrid do not understand the mode marker, and mixed-version or rollback use is not qualified. Keep the service and the interface at the same version.

## Editing saved tasks and closing sprints

A task that has never started offers **Edit task…** in its title bar and board/list menu. The shared composer edits the original description and attachments, with title, model/effort/access/language and checkout choices. Saved files load before Save is enabled; a failed read offers Retry and never silently replaces them with an empty list. Save changes starts nothing. The original task version is checked transactionally against Start: an intervening edit refuses a stale save, and any execution identity (including an unknown or failed start) permanently makes the saved request read-only. Live messages and supported session-option changes remain separate actions.

**New task** on a created sprint adds a draft even while its queue is active. It inherits the sprint checkout choice. A new member joins the sprint's roster with no links and the lowest priority; existing tasks and dependencies are not rewritten, and its links and priority can then be edited (see [Live sprint graph and execution order](#live-sprint-graph-and-execution-order)). Editing a pending member changes only its request, not the original planning document or dependency graph. Adding after queue completion leaves the queue paused until the owner continues it. If saving succeeds but linking is unconfirmed, the UI opens the saved task and explains the partial effect rather than creating another copy.

**Close sprint…** is visible beside the sprint actions (in the sprint's actions menu when the title band is narrow) and asks for confirmation. It stops the queue and planner and moves the sprint and its member tasks to the Archive together. A task that has started must be Done; conflicting completion records or a merge or pull-request operation still running also block closure. The refusal names the blocking tasks, and nothing moves to the Archive until the blockers are resolved. Not-started tasks, including queued slots, are archived as drafts. Done tasks keep their completion, and already archived members stay archived. Closure does not accept, cancel, merge, delete or discard work.

The closed sprint leaves active lists and appears under **Archive → Closed sprints** with its retained Summary. Restore returns an archived member to the project without a sprint; restoring an unfinished task never starts it. A closed sprint cannot be reopened, restart its queue or accept new members. Sprints closed on older versions are not retroactively archived with their tasks.

Age-based cleanup of a Done task newly archived by sprint closure counts from that task's completion: add the normal auto-archive delay configured at closure, then the machine's archive retention period. Closing the sprint does not reset that clock or cut out the normal auto-archive delay. If completion is unknown, age-based cleanup is unavailable. Archived drafts have no age-based cleanup; explicit deletion still requires the existing checkout and unmerged-work safeguards. Archiving never removes a worktree or a branch.

A sprint cannot close while a task returned for rework is still open (`rework_pending`).
Returning a member for rework pauses a completed queue, or a queue whose automatic slot refers to that member.
If any direct or transitive dependent in the sprint already started, Workspace asks for confirmation
when it has that warning. Rework does not stop, change or roll back those dependent tasks.

**Delete sprint…** is a separate permanent action in the sprint menu, also available on a closed sprint. Its single confirmation names the task count and says running work will stop. The
service pauses the queue, fences new starts and continuation controls, cancels each bound execution through the existing task-wide stop path, and waits a bounded time for confirmed
runtime/process quiescence. It removes member tasks, their conversations/questions and the sprint from product lists; it never selects another sprint's tasks. A failed or unconfirmed stop
leaves the sprint in place, marked Deleting in the project's sidebar row and as a strip on its board ("Deleting, 1 task left: <title>, waiting for its stop"), with Retry delete… on the
sprint's summary; it is not listed under Archive's closed sprints, and it never hides or blocks other sprints. The confirmation dialog shows a refusal once, on the action's error line. A
deletion that started but still waits for a task closes the dialog (pressing again would change nothing) and its receipt line names each remaining task by its title with what to do (for
example, still waiting for Cortex to confirm that its engine processes stopped: stop or close it; reopening Cortex while no other agent app works in the project folder lets Cortex confirm
it), never by its internal id or a raw refusal code; the sprint's Deleting strip and Retry delete… carry it on. The service also saves that reason on the sprint
(`deleteReason`), so the strip says it after any later read ("Deleting, 1 task left: <title>. <reason>"). A stop that is refused or not confirmed does not end the pass early: every
member that is quiet is deleted in the same pass and each one that is not is named. When the bounded owner action answers before its deletion pass ends, the reply names the stage it
reached (or the last concrete reason), the pass keeps going, and when it ends the service announces the project as changed so every screen reads the sprint's remaining count again
instead of keeping the count from part-way through. A worker left running when the service was killed is settled when the
service restarts (see [native attempts left unconfirmed](../features/recovery-and-replacement/index.md#native-attempts-left-unconfirmed-by-an-earlier-host)), so a retried deletion completes.
Members already deleted (`purged`, or listed in the sprint's `deletedTaskIds`) leave the sprint's displayed membership and are never required on the task board; only the members that still
exist are. When a project's data read keeps failing, the board's strip keeps its owner wording and adds the read's precise cause (for example the sprint and the task identities a board did
not hold). Repeating a successful deletion returns its recorded result.

Only a clean worktree with recorded app-creation provenance and a matching Git checkout may be removed, using non-forcing Git removal. Dirty, unproven legacy or unavailable worktrees are retained and their paths are shown in a dismissible notice. The project folder and all Git branches (including unmerged commits) remain untouched. This does not grant permission to discard files. Successful deletion updates the window immediately from the service acknowledgement; a background refresh cannot resurrect deleted tasks. Bulk deletion skips per-task VACUUM, leaving physical space reclamation to ordinary cleanup.

## Live sprint graph and execution order

A created sprint has two graphs. The **approved plan** (`Plan vN`) is the plan version that Create tasks materialized; it is read-only history in the desktop and is not written by the live-graph actions. The pre-existing generic sprint patch can still write `plan`, `history` and `materializedVersion` unless the service is separately hardened. The **live sprint** is what the service actually runs: every task bound to the sprint, including drafts added later with **New task**, with the dependencies saved on each task (`product.tasks[id].dependencies`). After creation the sprint page opens on **Live sprint**. **Approved plan vN** and past versions stay one press away, read-only, and keep each plan row's task state, matched to its task by stable task ID. A line on the live page says how many tasks are not matched to the plan or whose links differ from it.

**Reading.** `product.sprint` action `live_graph {sprintId}` returns the ordered roster, each member's `readStatus` (`confirmed`, `missing`, `unreadable`, `purged`), its direct prerequisites (`dependencies`, null when the member could not be read, never an empty list), `editable`, `canBePredecessor`, `approvedPlanKey`, the effective `priority` order, a `graphRevision` token and the execution `order`. A missing or unreadable member makes `complete` false; the desktop then shows a placeholder and disables every edit. The read refuses rather than clipping above 1,000 roster entries or 10,000 stored dependency entries.

**Editing links.** `set_dependency {sprintId, dependentTaskId, predecessorTaskId, present, expectedGraphRevision}` adds or removes one direct link. Under the machine lock the service checks the revision and the bindings, validates the whole candidate graph with the same ID-only validator that materialization uses (self, missing, cross-sprint, duplicate and cycle are refused) and atomically replaces only the dependent's link set. Only a saved draft that has no execution identity, is not terminal or archived and belongs to an open, created, non-deleting sprint can change its incoming links. A new link to a cancelled, unconfirmed, purged or archived-unsettled prerequisite is refused; a retained bad link can still be removed. Asking for the state that already exists answers `changed: false`. This action never writes the approved plan, history or `materializedVersion`, never creates tasks again and never lifts the owner's Pause. In the desktop, **Edit links** on the graph (choose the prerequisite, then the dependent, read the sentence, confirm) and the Details panel of a task both do this; while the queue is running the app warns that the change can alter which task starts next and points to Pause.

**Priority among ready tasks.** `move_priority {sprintId, taskId, direction, expectedGraphRevision}` (`up` or `down`) swaps a not-started member with the nearest editable draft in that direction. The priority sequence is stored separately from the roster, so task numbers never change; with no saved preference it equals roster order, a new member is last and a removed member is dropped. It breaks ties only among tasks that are ready; dependencies still decide readiness. A move with no editable neighbour answers `changed: false`.

**Refusals and retries.** Refusals are `{ok:false, code, message}`; the codes are `unknown_sprint`, `project_store_unavailable`, `sprint_graph_too_large`, `sprint_membership_invalid`, `sprint_read_only`, `stale_graph_revision`, `task_outside_sprint`, `sprint_member_read_only`, `predecessor_unavailable`, `dependency_self`, `dependency_missing`, `dependency_cross_sprint`, `dependency_duplicate`, `dependency_cycle`, `invalid_request` and `graph_save_unconfirmed`. After a change, a stale refusal, a lost reply or an unconfirmed save the client reads again before sending another edit and never retries a move blindly. An older service that refuses the action, an unrecognized reply version and an offline cache leave the interface read-only.

**Start fence.** A queued start (Hybrid and Automatic, in the project folder and in a worktree) rechecks, under the machine lock and before the existing start claim, that the saved sprint, links, priority and the relevant task facts are what the scheduler chose from. A pick made from an older graph is dropped and reconsidered without recording a hold. Manual Start takes the unchanged admission path with no such check.

**Execution order.** The reply's `order {mode, queueState, working, notStarted}` is the single projection the desktop uses. Working tasks come first, in the durable order in which they started (`startOrder`; unknown order sorts last). Not-started tasks follow by `rank` (1-based) from the same readiness and priority rule the queue applies: Hybrid ranks behind a prerequisite that is Ready to merge or Done, Automatic only behind Done; picks that assume a running prerequisite settles are `conditional`. Unranked tasks follow in priority order with a `reason` (`task_missing`, `task_state_unknown`, `sprint_closed`, `queue_paused`, `queue_held`, `queue_complete`, `queue_blocked`, `dependencies_blocked`, `native_worktree_unavailable`). `next` is set on at most one task, only for a running queue with a live runtime; it is the current candidate, not a reservation. In Manual the ranks are suggestions and nothing is Next. Board, List and the queue bar of a sprint show this order (Working before Not started, then blocked, paused and unknown tasks with their reason); the status text reads "Next in", "Queued 2nd in" or, in Manual, "Suggested 2nd in". The All work view keeps its global grouping; only the tasks of one sprint are ordered within the places they already hold.

The live read and both edits are owner actions of the local desktop. Paired remote devices are refused all three, and the model-facing `sprint_graph` read keeps its contract and scope.

### Re-checking the sprint after an owner change

When the owner changes a running sprint task (an owner message delivered to its coordinator, an owner rework request, or an owner answer to the coordinator's or a worker's question) while other tasks of the sprint are still unstarted drafts, the runtime records `SPRINT_IMPACT_CHECK_REQUIRED` for that task. A worker's replan or scope request is not such a change. At each coordinator turn of a sprint member the service records which members are still drafts with no execution identity (`SPRINT_MEMBER_BOUND`); the check is required only while one of them is left. Until a verdict for the newest trigger is recorded, completion is refused (`sprint_impact_pending`, recorded once as `SPRINT_IMPACT_PENDING`) and the coordinator's next planning or reconciling turn names the trigger.

The coordinator reads the drafts with `sprint_graph` `include: "pending_descriptions"` (full descriptions, versions, links and the live `graphRevision`, in pages of about 48 KB with a cursor) and calls `sprint_revision_submit`. The tool's schema carries its contract. Verdict `unchanged` resolves the check. Verdict `revise` names the trigger, the `base_graph_revision` and up to 50 changes: `update` (an unstarted draft at its `expected_version`: title, description and/or its whole link set), `add` (a new draft with key, title, description and links to members or other new keys) and `propose_cancel`. Under the machine lock the service refuses a change of a started, finished, archived or purged task (`sprint_member_read_only`), a stale draft version (`stale_version`) or graph (`stale_graph_revision`), and runs the same ID-only validator as Create tasks and owner link edits (`dependency_self`, `dependency_missing`, `dependency_cross_sprint`, `dependency_duplicate`, `dependency_cycle`) plus the prerequisite rule (`predecessor_unavailable` for a new link to a cancelled or unavailable task). A refusal names the change and rule; the coordinator corrects it in the same turn.

A recorded revision gets a mandatory independent review (a fresh read-only reviewer, `_sprint_revision_review_N`), which never binds the task's own review or completion and neither waits for nor blocks writers. Findings return the task to the coordinator for a new revision; after three reviewed revisions for one trigger the last one goes to the owner with its findings. While a revision waits for its review or the owner's decision, the drafts it changes are not started: the queue shows them with the reason `sprint_revision_pending` and records no hold, and independent drafts continue. An approved revision of an **Automatic** sprint is applied by the runtime: every draft change, every new draft (appended to the roster, last in priority), the saved product record and `SPRINT_REVISION_APPLIED` (with digests of the graph before and after and the replaced titles, links and descriptions) commit together or not at all, after the versions and the whole graph are checked again. An owner-chosen title is kept. In a **Hybrid** or **Manual** sprint the owner sees the card **Sprint changes from task X** (Russian interface: «Изменения спринта от задачи X») on the Live sprint page and chooses **Apply** (`product.sprint` action `apply_revision {sprintId, revisionId, expectedGraphRevision}`) or **Reject** (`reject_revision {sprintId, revisionId}`); both are local desktop actions refused to paired devices. Proposed cancellations are only shown on that card, also in Automatic (after the other changes are applied the card offers **Dismiss**); the service never cancels a task for them. Before an owner decision the service checks the drafts and the graph again, and **Apply** also names the `graphRevision` the owner saw. A revision that cannot be applied any more is recorded as refused and the coordinator submits a new one.

The trigger task's conversation shows one runtime line in the task's language when the check resolves: "Проверены оставшиеся задачи спринта: без изменений", "…: изменено N, добавлено M" or that the changes wait for the owner (English: "Checked the sprint's remaining tasks: no changes" and so on). A changed task's Details on Live sprint say "changed by task X" (Russian «изменено задачей X»), held drafts show "Waits for a sprint revision" in the order, and the sprint Summary lists the revision history. The live read (`live_graph`) carries the newest 20 revisions as `revisions` (`revisionId`, proposing `taskId`, `status`, `applied`, `reason`, `changes`, `held`, `updated`, `added`, `previous`, `findings`). Previous titles, links and descriptions are kept in the revision record; there is no separate draft-version table. Nothing here writes the approved plan, its history or `materializedVersion`, lifts Pause or deletes anything.

## Choices and checkout

`tasks.create` saves the description and optional per-task model route, effort,
access mode, message language and checkout kind. Saving does not start work.
Explicit Start is task-local: another Workspace execution, pending/unknown start,
or ordinary ACP task in the project never refuses it. A task with an existing
execution still returns `busy`; refresh to see that execution instead of starting
it again. Request replay returns the original receipt without redispatch. A
crashed start remains unknown for that task only and is never replayed.
The reverse direction is independent too: pending, unknown or running Workspace
starts do not reserve the project against a new ordinary ACP first prompt.
Source-event replay still recovers the original ACP task without creating another.
Each execution has its own ACP session and cancellation target. Project-folder
tasks still share files; start admission is not a filesystem isolation boundary.
When execution finishes with recorded changes or incomplete change evidence, code review waits for active writers in that checkout to finish. New writers queue behind the checkout reservation through review, verification and final reconciliation. Reviews with no project changes to certify, including sprint plan and revision reviews, neither wait for writers nor take a new reservation. A sprint revision review preserves the task's existing reservation after the current revision passes host verification. Verification that runs host commands still needs a stable checkout; runtime-only sprint plan verification does not. Other checkouts have their own reservations.
Sprint queues retain their dependency checks and pause rules. Manual
`tasks.start` does not itself check sprint dependencies; dependency-aware
board/sprint controls are presentation guards, not server enforcement.

Selections are session-local: they do not rewrite machine defaults. Auto language
does not inherit a previously chosen explicit language. Composer changes require
an idle owning session; busy or unavailable sessions refuse them. A multi-setting failure explicitly names acknowledged and unconfirmed fields; it never reports a partial update as a successful whole. Refresh before retrying an unconfirmed step.

The Settings machine catalogue retains every connection-selectable route identity, including disabled models with their existing routing switch off and different connections for the same model, and cached observed
efforts for the chosen model. Unsupported or withdrawn routes are not silently
replaced. A failed dispatch remains unknown and is not replayed automatically.

A plain project folder opens and runs in place even when Git is not installed. Git-only actions such as initializing a repository still require Git. The **Separate worktree** choice is disabled with an explanation in a non-Git project. The service refuses Start for a saved worktree choice without a Git repository, without a first commit, or when Git is unavailable. It explains the problem and how to fix it or switch the saved task to **Project folder**. The socket reply carries the stable reason in `error.data.reason` (`worktree_git_required`, `worktree_head_unborn`, or `worktree_git_unavailable`). It neither initializes Git nor falls back to **Project folder**. Tasks, results, review and acceptance do not depend on Git. Code can read/save project files, while Changes explains that a Git diff is unavailable. Accepted work reaches Ready to merge; the owner can close it as-is without Git, or explicitly initialize Git before using Git integration. Closing as-is a result that already reached Ready to merge through acceptance (manual or automatic) keeps it checked, and so does a result that an earlier build marked `automaticReady` (`notVerifiedByCortex` stays false); closing any other result as-is still records the lasting Not verified warning.

### Separate worktrees

**Start** creates the worktree under the machine's configured root, at
`<root>/<repository-id>/<slug>-<12-character-suffix>`, with branch
`cw/<slug>-<12-character-suffix>`. It starts from the project's committed HEAD,
not its staged, unstaged or untracked files. Invalid roots and path or branch
collisions refuse Start instead of using another folder. Existing saved
branches and checkout paths remain unchanged, including legacy names and after a
default-root change. The default root is
`$XDG_DATA_HOME/cortex-workspace/worktrees`, or
`~/.local/share/cortex-workspace/worktrees` when the service has no XDG data override. The dev service sets XDG data to `<channel-state>/data`, so its default is `<channel-state>/data/cortex-workspace/worktrees`. Overview reports the absolute default separately from the saved root; the real reset control and example use that value. Changing or resetting the root never moves existing worktrees. Settings reject roots inside Git checkouts or registered non-Git projects, and locations occupied by files or lacking write/search permission. A writable external folder such as `/mnt/Big_Storage/.cortex/worktrees` is allowed; the `.cortex` name alone does not make it a project store. A refused save retains the draft and a persistent readable reason beside Save. The actual linked checkout is the ACP session cwd and permanent runtime
task workspace; it shares the main repository's **one store per channel**: `.cortex/cortex.sqlite3` for release/default ACP, `.cortex-dev/cortex.sqlite3` for Workspace Dev. Dev never opens the release store; see [channel ownership](backlog/CW-S12.md).
Native execution, review and verification use the bound checkout. Changes reads
its baseline-to-current diff, including its untracked files; Code and task Git
operations use its exact checkout ID. A missing or changed checkout leaves
history and recovery available where recorded, not an editable view of main.
The [worktree execution policy](security.md#separate-worktree-boundary) is stricter
than project-folder execution. The owner terminal still opens in the registered
project folder. Project-folder tasks share the owner's files;
pre-existing uncommitted work therefore belongs to that visible checkout.

Activation adds the exact Workspace helper paths to local Git excludes, without
editing tracked ignore rules. File projections and app staging exclude them too;
a commit refuses helper data already staged by another client. Separate-branch integration enumerates changed/untracked user paths and stages them literally, without adding or pathspec-matching ignored helper folders. Failed staging or commit restores the prior index selection; user file contents remain. Git identity/hook problems show recovery guidance instead of raw Git advice. Project-folder tasks still share their visible user changes; this is not authorship attribution.

## Local settings and Git drafts

Machine configuration is reread after activation and on machine-fact refreshes, including background first-run setup. Dirty settings retain their original revision token even if a newer read arrives. A structured revision conflict keeps the draft and exposes Review changes and explicit Reapply mine against the refreshed revision. Unknown save outcomes remain unknown and are never automatically retried. A refused defaults update does not write Workspace-only mode/checkout values.

Git commit-message drafts update reactive checkout state synchronously, survive fact refreshes, and clear only after a successful matching commit. A newer draft entered while Commit is in
flight is kept. Empty messages remain refused. Commit is local and requires no remote: an existing staged selection is kept, or, when the index is empty, the listed non-helper changes are
staged and committed. Amend uses the same visible message and explicit confirmation. The Finish dialog's commit message (Commit to this branch and squash merge) and its pull request title
and description use [short text drafts](#short-model-texts) (`product.task` `draft_integration_text` with `method` `commit`, `squash` or `pr`, `targetBranch`,
and for `commit` the pending `paths`). Commit and squash send a bounded file list and text diff to the configured model (pending paths against `HEAD` for commit;
the checkout against the target merge base for squash). Untracked files are listed by name only and private paths are omitted from the prompt. The PR title and its
fallback description use local Git metadata. A matching confirmed Push summary supplies the PR description instead. The dialog never replaces an owner edit. Nothing is committed, merged or pushed until
the owner confirms. Automatic close, branch updates, conflict completion and revert use fixed English messages. Owner-written messages are not translated. Revert requires an empty index before applying
its inverse and making the English-message commit; a failure leaves Git’s visible pending work for manual recovery. Push accepts a configured remote name or the confirmed
credential-redacted display URL and resolves it against current Git configuration before one non-forcing push. Unknown, stale or ambiguous URLs are refused; an unclear push result is never
retried automatically. While a fetch, pull, push or commit of a project runs, another Git action on that project is refused at once with `git_in_progress` and a message naming the running
action, instead of waiting; shorter Git actions are waited for. Pull refuses an absent tracking upstream with setup guidance. Fetch and Pull use Git's configured defaults/tracking directly,
not the Push dialog's remote field. When a remote exists, Pull is disabled with a reason when no upstream exists or the last fetched state has no incoming commits; Fetch checks for newer
remote state. Local-only repositories omit Fetch, Pull, Push, PR creation and push-status wording rather than showing dead network controls. Local commits, review, squash merge and confirmed
closure still work.

## Result links

On the local desktop, a Markdown video or subtitle file link opens a path popup.
Select **Open in default app** to hand the file to the operating system, or
**Copy path** to open it yourself. Relative paths use the task's checkout, not
the app's working directory. If the checkout path has not arrived, the popup
shows the relative path while it waits and updates in place when the checkout
is confirmed. If it cannot confirm the checkout promptly, it offers **Retry**.
A worktree task never falls back to the main project folder. The opener does
not load the file into Code or change the task's route. Missing files, refused targets and immediate opener
failures are shown in the popup; the task stays usable. The operating system
chooses the app, and a successful handoff does not confirm playback. A worktree
under an external `.cortex/worktrees` folder is not refused just because of
its name. Paths through `.git` or marked Cortex stores are refused, including
aliases into them. Ordinary source references still open in Code when the
file is in the checkout.

HTTP(S) and mail links hand off to the system on activation without replacing
the task window. Unsupported schemes such as `file:` and executable schemes
remain text. See the [desktop trust boundary](security.md#local-slice-trust-boundary-built).

## Integration and conflicts

Result cards retain headings, paragraphs, lists, quotes and code instead of collapsing
the runtime summary into one line. Only nonzero check outcomes have counters, and
an all-zero open-check record does not warn that checks remain open. Work awaiting
owner merge confirmation stays visibly actionable on the board, list and sidebar.

A complete, current runtime verification with valid workspace binding promotes
work to Ready to merge. Owner acceptance and integration remain separate.
New untracked UTF-8 text files appear as full additions within the shared diff-size budget; symlinks and
binary or oversized content are not read as text. A conversational no-file follow-up retains the
implementation summary when the observed source digest is unchanged; its reply remains in conversation,
and new runtime checks still govern acceptance. A recorded missing-command/tool blocker can survive
session-owner renewal only for the exact same check and source, never a policy refusal, stale attempt or
pass. Acceptance observes the checkout again. `results.get.acceptanceAvailable` reflects current evidence
rather than an always-false placeholder. `product.read.tasks[taskId].reviewActions` supplies
Accept/Request changes availability, a readable blocking reason and the unblocking action (`wait`,
`request_changes`, `rerun_checks` or `refresh`), plus current result identity/version/digest and whether
technical blockers require explicit acknowledgement. Check observations can arrive before
the final result facts: the card keeps Accept off and shows a waiting message, not an
evidence failure. A settled read can still report missing evidence or changed files.
The mutation rechecks these facts and the result version. Request changes uses the existing owning-execution prompt control; it does not require acceptance
or Git. For a settled completed result, the service restores its exact durable session automatically
before capturing owner guidance, including after restart; a live stale or differently bound session still
refuses. An informational answer with no checks can be accepted when its runtime evidence permits it; this
never certifies code for merging.

Changed files, stale results, unavailable
evidence and unsuccessful work verification refuse integration.

The owner confirms squash merge into a local target branch. Dirty target checkouts are refused; there is no automatic stash or discard of owner changes. The guarded [target-side squash rollback](#native-machine-boundary) is separate from resolving a source merge. A target predating the work’s baseline is refused rather than silently including earlier branch work. Bringing a newer target into the source branch invalidates the earlier acceptance. If that merge stops, the conflict stays in the source checkout; the target is not changed.

1. Open the conflict in **Changes**. To resolve it yourself, choose and stage an available text version, or edit and stage the resolution with Git. Add/delete, symlink, binary, mode and oversized conflicts may require Git or an editor. Use **Finish merge** after staging; it checks the full index and remains available for a confirmed stopped merge even if no paths are displayed. **Abort merge** is available only when the service can confirm it is safe to abort the saved operation.
2. To involve Cortex, choose **Hand to the app…** and confirm. Detection alone never dispatches Cortex. The service binds the handoff to the saved source and target commits and sends the coordinator a Git-index manifest: JSON-quoted paths as data, not instructions, with explicit completeness and truncation flags. The manifest contains no file contents. A recorded handoff does not prove delivery or progress. **Stop resolution** requests a stop. Once stop, refusal or failure is confirmed and Git still holds the saved stopped merge, manual Finish and Abort return. If the outcome is unclear, use **Inspect merge state**: it rechecks the saved state without sending another prompt or committing, and manual writes stay blocked while the outcome cannot be confirmed. On confirmed Cortex completion, the service can finish the exact saved merge when Git and the staged resolution validate it.
3. Whether you or Cortex finish, review the diff and select **Request fresh checks**. Independent review and runtime verification are not requested automatically, even in an automatic sprint. The request is bound to the resolved revision; if it cannot be confirmed, inspect rather than sending it again. **Requested** is not evidence that checks started or passed. Earlier checks cannot certify the resolution. Current passing evidence is required before integration, which still requires separate owner confirmation.

### Saved merges from an earlier version

If the conflict card says **Saved merge from an earlier version**, Workspace cannot verify the original merge commits and does not offer its normal Finish, Abort, inspection or Cortex handoff controls. Finish or abort the merge in Git in the original checkout on the saved branch. Then choose **Clear saved merge…** and confirm **Clear saved merge record**. Workspace refuses to clear the record if the owning execution has not settled, the branch does not match, or a Git operation or unmerged index entries remain. Resolve the stated condition before retrying.

Clearing changes only what Workspace has saved, not Git or the comparison base. It does not certify what happened in Git, restore earlier checks or request new ones. Obtain fresh review and verification for the current checkout before integration.

### Pull requests

PR creation uses the owner's installed `gh`, never copied credentials. Missing
`gh` is stated on the control. The branch must already have an upstream and no
unpushed commits: creating a PR never hides a push. A retained attempt prevents
blind retries after an ambiguous remote response. Confirm the outcome in GitHub
before another attempt. Real GitHub creation and paid-engine resolution are not
qualified by the no-paid fixtures.

Ordinary Close, Stop and Archive keep the folder. To close and request cleanup,
use **Close and delete worktree folder** and confirm. If the folder is retained
because it is dirty or busy, resolve the obstruction outside Workspace, refresh
the project and confirm the same action again. The task stays closed; Code,
Changes and regular Git actions do not reopen that checkout. Missing or uncertain
folders do not offer retry. In Finish, after a confirmed
local squash, **Delete worktree folder** is on by default when eligible and disabled
with a reason when unsafe. The merge is recorded before cleanup; if the folder
cannot be safely removed, it is retained without undoing the merge. Archive purge
includes the clean Cortex-created worktree, recognizes an already removed folder,
and never removes the main project. All three cleanup paths verify ownership and
cleanliness, use Git without force and keep the branch. Removing a merged checkout can make its old verification evidence unavailable; the scheduler preserves Done and the recorded integration instead of reopening the task. Only [Return for rework](#return-for-rework) clears the current closure and integration disposition. The previous integration stays in history.

Acceptance binds the runtime-evidence event version, not the execution display version: late transcript/progress writes cannot stale an unchanged result. New runtime evidence and changed source still invalidate old decisions. The UI uses current service review actions and source-drift warnings rather than inferring availability from check counts. An unsuccessful or unconfirmed Accept keeps its reason and Refresh result action beside the result after the transient toast closes. A paused queue identifies the blocking task and its review reason; review it, then use Continue queue.

Automatic sprint acceptance is recorded by the sprint service's `auto_ready` action, not by an owner click. The action decides from the sprint's current run mode at the moment the acceptance
would be written: it reads the run mode under the machine lock that a `product.sprint` `patch` of `runMode` also takes, in the same critical section that writes the acceptance. Switching a
sprint to Manual stops automatic acceptance of every result not yet accepted, including one that finished while the sprint was Automatic; such a result keeps Accept as an owner action.
Switching a sprint to Automatic settles its finished results that are still waiting, through the same validator. The background sprint pass runs when something can move the sprint on (an
owner or loop change of a sprint or task record, a runtime task event while a sprint is in motion, start or activation, and a 30-second safety tick while one is), at most every two seconds
per project, and offers `auto_ready` only for unaccepted finished members of sprints currently in Automatic, including in a paused queue; a Manual sprint costs that pass no task read, folder
observation or window refresh. The action then requires a completed runtime execution whose runtime verification passed and that the owner has not resolved, zero open failed, not-run and
unverified checks, no evidence warning, and the source version and digest a manual Accept would record, and it repeats the manual validator under the write transaction. A refusal from that
validator (stale result, required checks or review, required acknowledgement, unavailable result) returns `accepted: false` and leaves the task unaccepted; the queue then sends a repairable
result back to the agent or pauses with an owner decision (see [sprint run modes](#sprint-run-modes)). No open check is acknowledged on the owner's behalf. The action never waits for the
machine lock: while another operation holds it, the action returns `accepted: false` with `deferred: true`, the queue neither accepts nor pauses on that pass, and the next pass tries again.
A refusal is not evaluated again on every pass: it is kept while the runtime evidence of the task (its revision, state, latest event and latest node), its saved record and the project
folder's change signal are unchanged, for at most 30 seconds, and a run-mode change of any sprint of the project drops it. A folder without a usable change signal (no Git, no commit, a
nested checkout, over 100,000 paths, or a file changed in the last two seconds) keeps the refusal only while the same inputs hold, for at most 10 seconds. A settle that cannot complete (an
unreadable checkout, a Git timeout, a failed save) answers `accepted: false` with the failure and its reason instead of failing the pass, is kept for 10 seconds, and pauses only its own
sprint's queue with a reason that names the task and says the result could not be accepted automatically; a paused queue shows that reason, and every other task and sprint keeps running. The
kept refusal only skips a refusal; an acceptance always runs the full validator. Task completion does not read sprint state or take the machine lock, so a long operation holding that lock
never delays or fails a completion. An automatic record has the manual shape plus `"automatic": true`; a manual record keeps its previous shape.

#### Sprint planning limits

A sprint is planned by its own read-only planning task. Creating the sprint
saves it, then the service creates and starts that task in the project folder
and records it on the sprint (`planningTaskId`). The task is bound at Start to
the runtime's sprint planning policy (`SPRINT_PLANNING_BOUND`), a trusted host
option that commands, plans and model tools cannot set. It is an ordinary task
with its own coordinator, conversation and workers, but it is shown only inside
its sprint (see [Sprint planning conversation](#sprint-planning-conversation)).

The planning task studies the project: its coordinator reads the project and,
when the study needs it, plans read-only specialists (explorer, architect,
planner, auditors and the other read-only roles). The runtime refuses a plan
with a writing role (`read_only_task_writer_refused`) and refuses every project
write of the task. Its study plan names no verification commands (`[]`, refused
for every other plan); the runtime verifies the deliverable itself.

The deliverable is the sprint plan, submitted with the `sprint_plan_submit` tool,
whose schema states the item fields and limits. A plan may have any number of
tasks. One call carries at most 50 tasks and 128 KiB of UTF-8 JSON, so a larger
plan arrives in ordered parts; a first part starts a new plan and discards an
unfinished earlier one. The planning request tells the coordinator to submit a
large plan as small ordered parts while it works (`final: false` on each
intermediate part, `final: true` on the last), to keep each item compact without
dropping scope, dependencies, owner decisions or acceptance criteria, and not to
repeat the plan as prose. It points to the tool's advertised limits and states
no numbers itself. The runtime checks every part with the parts before it
and, at the final part, the whole assembled plan: unique keys, dependencies that
name only keys of the whole plan, no cycles, non-empty descriptions and at most
512 KiB of UTF-8 JSON for the whole plan. A refusal names the failing item
(1-based index over the whole plan, and key) and rule so the coordinator corrects
it in the same turn. A plan identical to the previous assembled plan is refused.
The review receives the whole assembled plan. Every recorded
plan gets a mandatory independent review against the sprint request and the
studied evidence (coverage of the description, self-contained items, sane
dependencies, nothing the evidence contradicts). This review does not wait for
active writers or reserve the checkout against new ones. Findings return the task to its
coordinator for a revised plan under the usual bounded review rounds and
coordinator returns; repeated findings or the round limit stop the task the
run-mode way. When the owner answers after a round-limit stop (the runtime's
own stop question, or the coordinator's question that the first such stop
asks for), the next submitted plan starts a fresh review budget. An approved plan passes the runtime verification
(`VERIFICATION_PASSED` with `runtime: sprint_plan`) only when it is the newest
plan and the one that review approved; the task then completes through the
ordinary completion gate.

When the planning task completes, the service turns its newest plan into the
sprint's proposal (a new plan version) and shows **Create tasks**. A completed
task without a new plan, after the owner asked something and the coordinator
answered, keeps the current proposal. A task that completes without any plan is
asked once, in its own conversation, to submit one; if it completes again
without a plan, planning is cancelled with a readable note. A planning task that
fails or is cancelled cancels planning with a readable note. **Stop** pauses the
planning task; **Resume planning** resumes it, asks the same task to continue
after a cancelled plan, or starts a new planning task when the previous one
ended. When a planning task can receive a prompt, a message in the sprint
conversation asks that task for changes and disables **Create tasks** until
its revised plan is ready. A failed revision keeps the previous proposal
visible for reference. Planning,
resuming and revising never create or run the sprint's tasks. Create tasks
materializes the plan, and the service then closes the planning task as done.
Closing or deleting the sprint stops a working planning task through
its execution control. A sprint saved by an earlier version whose
description-only planning was interrupted keeps its cancelled state and note;
an answer or Resume planning starts a planning task with the description and
the saved conversation.

A failure that cancels planning saves a reason, step, code and timestamp on
the sprint, with the same reason in the conversation. An owner stop is not a
failure. When the planning task reached a terminal state, the service also
classifies the outcome from recorded runtime evidence only and saves a `kind`:
`owner_stopped` (the owner stopped or finished the task), `limit` (an evidenced
plan-size refusal, `plan_part_too_large` or `plan_too_large`, or a recorded
scheduler, progress or no-progress limit) or `unknown` (the cause was not
recorded; the explanation says so and engine text is never guessed or shown).
The explanation says what Retry will do. After a `limit` failure, **Retry
planning** starts a new planning task whose request asks for small ordered parts
from the first submission. If that smaller-parts replacement meets the same
limit, the sprint records `recommendedAction: change_request`; Workspace then
offers **Change request** instead of Retry, and a plain Retry is refused with
`planning_change_request_required` until the owner replaces the description or
sends an amendment. An `owner_stopped` or `unknown` failure keeps the plain
Retry. The request, attachments, saved answers and conversation, and any
previously approved proposal are kept. Parts the stopped task had submitted but
not completed, and execution-only answers, are not carried into a replacement
task: the replacement starts its plan from the saved request. If Start is
refused or its reply is lost, the service checks for a committed execution.
A confirmed never-started draft is unlinked in the same write as the
failure; a committed execution keeps its link. If the check cannot be
completed, the service does not assume it can start a replacement.

**Retry planning** starts a fresh planning task when no execution was ever
started, including for older failed sprints that still link the draft.
**Change request** on a failed sprint with no live execution saves the
message and requests a fresh planning task in one operation. Both use the
current description, saved answers, messages and file references. If the
combined request exceeds 32,000 characters (including planning instructions
and saved conversation), the failure shows its length and limit. The owner
can replace the original description and restart planning without deleting
attachments, answers, the proposal or its history. These recovery actions
clear the active failure, not its conversation event. When the service
detaches a planner, it closes it as cancelled unless it was already closed.
The old task stays in history without appearing in ordinary Board lists or
counts, or raising its own notification. Recovery requires an owner action;
a failed start never launches a replacement by itself.

These are project-scoped `machine.call` operations with `op: "product.sprint"`:
`patch {sprintId, patch: {planning: "planning"}}` requests a retry,
`recover_planning_message {sprintId, text, attachments?}` saves a recovery
message (files are references), and `replace_planning_request {sprintId,
description}` replaces the text after `planning_request_invalid`. Send the
action name in `params.action`. Both recovery actions refuse if the sprint
is closed, deleting, materialized, no longer failed or still has a live
execution. A successful `{done: true}` records the owner change and requests
planning; it does not certify that the subsequent start succeeded.

Separately, if three consecutive service passes fail to check or advance
planning, the service saves one warning and adds one conversation event
without stopping planning. If saving that warning fails, each later failed
pass of the same streak tries again until it is saved, still with one event.
The service keeps checking and clears the active warning on the next
successful pass; the conversation event remains.

#### Reading relationships and prior results

Once a materialized sprint task starts, its coordinator receives a compact
summary of the task's direct predecessors and dependents. The coordinator can
use the read-only `sprint_graph` tool to page through the sprint graph or inspect
the selected task's neighbours. `sprint_task_result` opens a member's durable
state, the recorded synthesis, answered questions, write restrictions, the latest
recorded verification for its plan revision, and its immutable published report
catalogue. Answers and reports have separate continuations. A manual closure
retains its recorded verification warning. A report body is available through the same bounded cursor
pages as an ordinary report read. Both tools are coordinator-only and the
runtime derives the sprint from the authenticated task, so a task cannot select
another sprint or an arbitrary project task.

The result view reports whether recorded file writes are confirmed in the
recipient checkout, missing or unknown. A missing receipt,
ambiguous observation or unreadable file remains unknown. A queue dependency
still controls scheduling exactly as before. Hybrid acceptance does not by
itself mean that code from a predecessor is present in the recipient checkout.
Native execution that cannot establish a supported separate-worktree route
continues to refuse that start rather than claiming cross-worktree availability.

#### Creating a plan's tasks

**Create tasks** is one service operation (`product.sprint` action
`materialize {sprintId, planVersion}`), not a sequence of client requests. Under
the machine lock the service checks that the sprint is open, that its planning
is ready and that the named version is its current plan, validates the plan
(keys, descriptions, dependencies and cycles), and then creates every task
together with the sprint's transition to created: all drafts, with the sprint's
route and checkout kind (a sprint-wide checkout wins over a plan item's own),
the plan item's title, sprint membership, dependencies mapped from plan keys to
task identities and the inherited target branch, and the sprint's
`planning: created`, `materializedVersion` and `taskIds`. The drafts are written
in one project-store transaction and the saved sprint record is replaced inside
that same transaction. Any failure rolls the drafts back and leaves the sprint
ready with the same plan; if the saved record was already replaced, it is
restored to the copy read under the lock. A plan of any size is one transition,
so a closed window, a lost connection or a service restart never leaves part of
a plan created.

Each task's identity is derived from the sprint, the plan version and the plan
key, so the same plan always names the same tasks. A repeated **Create tasks**
(a double click, a retry after a lost reply, or a new request) for a version that
is already created answers with the same tasks and creates nothing; another
version of a created sprint, a stale version, a plan that is not ready, and an
unbound project store are refused before any effect.

Earlier versions created a plan's tasks one by one from the window and recorded
the sprint as created only at the end, so an interrupted Create tasks could leave
tasks that name the sprint but are not among its tasks. They are never adopted:
their set is unknown and they were created under other identities. Create tasks
creates the full plan beside them and its reply lists them; the window says how
many there are, and they can be deleted on the Board. **Delete sprint** also
removes them with the sprint.

#### Sprint planning conversation

**New Sprint** uses the same description editor as **New Task**: type `/`
for project skills or `@` for project files, and use the same attachment,
dictation and keyboard controls. Neither form offers commands that need an
active task. **Start planning** opens the sprint's **Conversation**.

Conversation uses the task layout from the first frame. It starts with the
sprint brief and shows whether planning is starting or its conversation is
still loading. For an older, stopped or failed sprint, saved notes, messages
and questions stay in that same feed, with Resume planning or the applicable
Retry planning and Change request controls. Text and attachments in the
composer stay in place as the planner becomes available.

Once the linked read-only planner is available, Conversation also shows its
coordinator messages, folded "Worked …" steps, questions, permission cards
and workers bar. The sprint brief takes the place of the planner's internal
instructions, and the proposed plan has an **Open plan** link. **Workers**
opens the linked planner's work details. **Plan**, **Board** and **Summary**
stay separate from the conversation.

A message from the composer asks for changes to the plan and continues the same
planning task; **Create tasks** waits for the revised plan. Answers, **Stop**,
**Resume** and the model choice go through the sprint's planning, so the
sprint's planning state changes with them. Sending closes once the tasks are
created. The plan is reviewed, edited and approved on the Plan tab as before.

The planning task has no sidebar row, board card, list row, count or search
result of its own. The service identifies new planning drafts before the
sprint links them, and the offline board preview preserves that identity and
any existing sprint link. No Git or commit action is offered for the planner.
A linked planner's open question or permission makes the sprint read **Needs
you** and gives the sprint one Inbox row. A notification about linked
planning opens the sprint's Conversation, and an address for the linked
planner opens its sprint (Workers for a worker address).

### Already in the branch and external completion

Ready to merge reads two independent facts. **Already in <branch>** means the task’s recorded content landed in the local target branch (or its separate-branch patch is contained there, including squash/cherry-picked content); it does not mean that branch was pushed. This view replaces merge/PR and acceptance controls with **Close task…**. The owner confirms closure; the app never closes automatically and records no merge or acceptance. Confirmation checks containment again. A changed target refuses closure and asks for refresh.

For separate branches, containment compares the saved task baseline with current tracked and untracked task content, excludes private helper paths, and reverse-checks the binary-capable patch against the local target in disposable Git index/object storage. The real index, objects and working files are unchanged. Missing baselines/worktrees, conflicts, unreadable or oversized content and failed patch comparisons never assert separate-branch containment. This context-sensitive comparison remains conservative for later overlapping edits.

When checkout and target are the same branch, there is no merge, squash, PR or same-branch from/to flow.
The service reads the task’s successful channel-bound file-write receipts, not the whole shared checkout.
For the native runtime these are its own file tools' records: each `NATIVE_PROJECT_WRITTEN` event of a
trusted channel of the task, confirmed by that channel's completed `project_write`/`project_edit` request
receipt with the same path and full-content SHA-256 (the bytes Cortex wrote). An event without such a
receipt stays unconfirmed. Exact recorded content in the target or its bounded local ancestry counts as
landed, even when another task later edits that file. A known Git baseline bounds that history; without
one, historical matches must not predate the write receipt. Missing/incomplete receipts, unsupported
deletions or links, content beyond the lookup budget and ambiguous edits remain unknown, not a positive
assertion. Older tasks are evaluated on refresh with the evidence already retained; no rerun or data
repair is needed.

Pending recorded files offer **Commit to <branch>…**, with owner confirmation and the existing
acceptance/evidence gates. Only those exact files are staged and committed; another staged file is
preserved. A different staged version of a task file, changed content, symlink path or unrecorded mode
change refuses this operation. The UI lists pending paths and warns when other uncommitted files share the
checkout. No reset, stash or push occurs. A successful local commit is labelled Committed, not Merged.
Unknown attribution explains its limit and offers **Close as is…** or **Done outside the app** after
finishing in Git, rather than a meaningless merge. The explanation names where the task works (the project
folder or its checkout) and says which limit applies: no file-write receipt at all (an engine that edits
files with its own tools leaves none, so there is no record of which files the task changed), or receipts
that could not be confirmed. A folder without Git keeps Close as is and Set up Git.

When a remote exists, a separate line reports pushed/unpushed from the target's last-fetched tracking reference, or unknown if none is available. It never fetches or pushes and never blocks closure. With no remote, no push-status line is shown.

Board cards are not draggable. **Done outside the app…** in the Board or List task menu, the task page menu, or the task's command palette opens confirmation; nothing changes until the owner confirms. Running/stopping/unknown tasks must be stopped and confirmed first; an active merge must be aborted. The service rechecks these conditions, persists `closedAs=external` and Done, clears active acceptance/integration labels, and retains files and historical evidence without inventing checks. A fresh window reads the same closure.

An item the owner closed **as is**, like an externally finished item, settles sprint dependencies and is never started again by the queue. Its Not verified warning remains; closure does not invent successful checks or a merge. An already-paused queue can recover through **Continue queue**, without repairing saved data; the owner decision does not silently undo a pause. Cancellation still blocks the queue. A live or unknown execution must settle before the queue can start a dependent task.

Shared styles give buttons, links, tabs, menus, cards, selectable rows and toggles a pointer; disabled/busy controls use the default cursor, while text inputs retain a text cursor. Cursor tests check the real shared cascade and inventory custom JSX click targets.

## Refresh responsiveness

Task refreshes keep their coherent runtime/product placement but do not wait indefinitely for repository
details or machine discovery. A repository inspection has a 50 ms foreground allowance and otherwise
finishes separately. Cached checkout facts render immediately while a stale snapshot refreshes in the
background, with a short reuse interval. A pending or unavailable snapshot never asserts that Git is
absent: the helper’s repository-presence fact, or the last confirmed value, governs the no-repository
panel. Review changes stays unavailable with an explanation until repository details arrive. Explicit
Git/file/task effects invalidate that reuse without dropping the cached value; superseded reads cannot
publish over the replacement. Commit sends one helper request and refreshes once; when nothing is staged,
the helper stages the listed non-helper changes in one call. The service still rechecks effect-specific
evidence before mutation. Machine discovery and disk accounting finish independently of the action that
requested a task refresh.

The service reserves separate bounded helper lanes for task reads, expensive inspections and owner actions. Reply logging uses a separate lock from product writes. Closed tasks do not reproject old result verification during product reads, and unrelated result projections are excluded from the product writer critical section. Known transient read failures keep the last coherent view and retry quietly twice; persistent failures and malformed facts remain visible. Mutations and ambiguous outcomes are never automatically replayed.

## Archive and service-owned work

When the machine is reachable, **Archive** is available for any retained task that
isn't already archived, even if its state is unconfirmed or a merge is in
progress. Done tasks move with one click. For other tasks, confirm **Archive**
to put away the work without checking its result, or choose **Cancel** to leave
it in place. If work may still be running, the service attempts the existing
Stop (Pause) action. It still archives the task if it can't confirm the stop,
and the Archive shows **Process stop was not confirmed**, even for Done work.
Done is a disposition, not evidence that the task’s processes exited.

The archive record keeps the pre-stop status. Archiving doesn't accept or close
an unfinished result, modify the retained merge or Git checkout, remove files,
or grant permission to discard anything. Hybrid and Automatic skip archived
and restored-held work for dispatch and background settlement without treating
unfinished work as a completed prerequisite. Restore
doesn't start or resume work: an unstarted task remains a draft; other unfinished
work returns stopped or held if its stop couldn't be confirmed. The queue does
not start it on restore. Use Start for a restored draft, Resume for a paused
run, or [Return for rework](#return-for-rework) for eligible ended work. If a
restored task's Resume reports that the machine is busy, the request has not
resumed it; try again when the machine is free. Automatic retention still
applies only to eligible Done tasks.

On activation, a saved materialized sprint whose members are missing from the selected channel store can restore those members as drafts. The saved plan version, original task identities, dependency order and task overlays must match exactly. Existing rows and explicitly purged members are not replaced. The queue pauses before restoration and explains that previous execution history is unavailable in this channel; nothing starts automatically, and no old execution/result is inferred or read from another channel. An ambiguous or missing plan stays paused for attention.

Retracted worker checks (withdrawn probes or invocation errors) are rederived from the persisted result, exact provisioning and command receipts, not the reviewer-only `current_outcome` field. Valid retractions are omitted from the active result checks and worker overview counts, as in the runtime's own completion gate. They neither block Accept/Ready to merge nor count as passes. Other failures, missing receipts and current runtime verification requirements remain blocking.

Sprint creation first checks that the chosen model exists in the current selectable catalogue. A missing/unavailable model refuses with a connect/select-model explanation, keeps the
description in the form and creates no sprint or planning task. A preset name alone is not a connected model. Sprint creation offers the same **Where it works** choice as a single task:
**Project folder** (the currently open local branch) or a **Worktree**, which is either **One worktree for the whole sprint** (the default, saved as `sprint.checkout = {"kind":"worktree","shared":true}`) or **Separate worktree per task** (`"shared":false`). The machine default fills the form until the owner chooses; the separate-worktree option is disabled with an explanation without service support, in non-Git projects or in Automatic mode. Creation saves its chosen checkout kind, route and target before starting the planning task, which always studies the project folder. Planning and Summary
show the saved checkout choice. Every task materialized from the plan inherits the sprint checkout kind, overriding per-item plan hints; in shared worktree mode every member is marked with its sprint, and the first Start provisions one worktree and branch (`cw/<sprint-title>-<suffix>`) that every later member's Start reuses (same checkout id, path and branch, after the same live witness check); in per-task mode each task gets its own worktree on explicit start. Sprints saved without `shared` keep their per-task worktrees. Separate worktrees per task run in Manual mode only: creating such a sprint saves Manual, the service refuses Hybrid/Automatic for it and switching a sprint to per-task worktrees switches it to Manual; the UI disables Hybrid and Automatic with an explanation. Closing or deleting a task of a shared worktree keeps the worktree (reason `worktree_shared`) until the last member releases it, then the normal clean, non-forcing removal applies. Members share the checkout the way tasks share the project folder. A member's target is the sprint branch, so its Finish is the same-branch **Commit**: only its own recorded files are committed onto the sprint branch and other members' files stay uncommitted. The sprint's real target is read from the sprint. When every member is done, **Merge sprint branch** (`product.integrate` with `method: "sprint_merge"`) squash-merges the sprint branch into the target once, in the checkout that has the target branch open (clean, no Git operation, no task using it), records `sprint.merge`, then releases the members so the clean shared worktree is removed (the branch stays). Automatic runs in a shared sprint worktree: its queue starts members there, commits each one's own files onto the sprint branch, and when the queue completes merges the sprint branch itself only when the target is the branch open in the project folder; any other target, or a refused merge, leaves **Merge sprint branch** to the owner. Manual, Hybrid and Automatic starts use those same saved task options; an Automatic sprint starts in the project folder or in its one shared worktree
(see [sprint run modes](#sprint-run-modes)). No checkout is created during planning, no existing checkout moves, and machine defaults are not rewritten. Older sprints without a saved choice
keep their per-item/default Project folder behavior. The choice is fixed for that sprint; task execution/recovery semantics are unchanged. Planning runs in the sprint's read-only planning
task (see [Sprint planning limits](#sprint-planning-limits)): it studies the project and proposes a validated, independently reviewed dependency plan; it never implements the description.
The planning task must ask the owner, not decide itself, when the description explicitly requires an owner question. For a multi-item plan, the planner supplies each task's self-contained
scope, acceptance criteria and relevant owner answers in plain text; only minimal shared context belongs there, never sibling work or serialized conversation records. An unanswered
owner-question requirement belongs only to its own item. The service does not append the whole sprint to every item. A sole planned task retains the original owner requirements and
plain-text follow-ups as a safeguard against a shortened summary; oversized combined descriptions refuse rather than truncate. Scope decomposition remains model-generated, not a filesystem
permission boundary. Create tasks materializes the plan as unstarted user tasks. Only Run sprint or an explicit task Start begins execution. The plan arrives only through the
runtime-validated `sprint_plan_submit` tool, never parsed from prose. A refused plan names the failing task (1-based index and key) and rule, for example "task 4 (t4): description is empty";
duplicate keys, unknown dependency keys and cycles are always errors. Planning failures add a service-owned readable explanation to the planning conversation; raw model output and engine
errors are not displayed. The cancelled banner offers Resume planning and the conversation retains the failure note; a classified failure names the limit or says the cause is unknown, and after a repeated limit offers Change request instead of Retry (see [Sprint planning limits](#sprint-planning-limits)). A failed or interrupted planning task never produces a ready state with a
null plan. Materialized tasks inherit its settings and target; changing a task target makes that target explicit. Sprint-target changes propagate to inherited task targets, not explicit
overrides. Once tasks are materialized, change their routes individually rather than silently rewriting them.

The activated service child advances Hybrid and Automatic sprints without a window.
It reads bounded runtime projections, pauses for questions, unknown outcomes,
conflicts or missing/unsuccessful checks, and uses a stable explicit start identity.
A question pause records the waiting execution identities and clears when those
same executions leave their answer wait. An explicit owner pause, a failed check,
a conflict or unknown outcome does not self-resume.
A lost active session after service restart remains unknown, not silently resumed.

Before synthesis, the runtime attempts each exact executable worker check reported
as `not_run` or `unverified` once on that observed source revision. An existing matching final
verification receipt avoids a duplicate run. Policy/paid-consent blockers remain
blocked and explicitly not run, never converted to failures or automatically executed. Plan submission and
verification repair first probe program availability in the real verification sandbox, using the same
final launch path and check directory. A bare program name is looked up on the command search path that
verifier and worker commands share: the project's `.venv/bin`, `venv/bin` and `node_modules/.bin`, then
the owner's [tool search path](maintenance-and-controls.md#tool-search-path) (the `PATH` read from the
owner's login shell plus folders the owner added), so a tool the owner's terminal finds (for example
`node` and `npm` from `~/.nvm`, or a `/usr/local/bin/node` link into it) is found by the plan check
exactly where workers and the final verifier run it. Other search entries inside the project are never
used. A changed tool search path applies from each task's next command or check, without a restart. An
unavailable probe environment does not establish that the check program is missing; an incomplete wrapper
command is refused rather than declared available. Workers declare checks early with the same preflight;
it proves availability only, not success. Missing tools require an isolated project-private installation
or an executable alternative, without sudo or system changes. If neither is possible, the responsible
worker asks the owner immediately through the existing question route. New unrunnable executable gap
declarations are refused before publication. A nonzero test result is a failure, not a technical waiver.

Human-only judgements and legacy gap items without a recorded command appear separately as **Check manually**. They keep their original evidence and explanation but do not enter passed/failed counters, offer Rerun, or block owner Accept. Legacy items with a recorded command remain rerunnable; an actual missing-program exit is a visible failed run, not a fabricated pass.

The check rerun control executes the retained literal command in its recorded task-checkout directory (including a subdirectory)
through the ordinary read-only verification runner, without a model turn. It does not widen the runner to the writing-worker `development` profile. It
binds to the current result version, check index, source digest and owning session. The native desktop bridge carries this same bounded identity, not executable arguments; the runtime still refuses a stale result or unavailable check.
Rerun is offered only for an item with a recorded command and no policy prohibition. A fresh run has a 60-second deadline and displays its actual outcome, exit code and a redacted bounded output excerpt, including successful output; complete output remains retained separately. Timeout/cancellation never earns a pass. An item without a command returns a plain-English manual-check explanation instead of a doomed rerun. The UI refreshes result evidence even after a technically blocked rerun, so a subsequent Accept uses its new source version. Check rows in working, question and review states show evidence availability and readable blockers, not report identifiers or internal codes. Conversation activity counts tools needing attention separately from actual failed checks; a refused tool submission is not itself a failed check. Identical delivery keys do not repeat execution; stale input refuses. Cancellation
uses the active session's existing stop path. New complete passing evidence can
close a check only on the matching published and reviewed source; later failed,
pending, incomplete or ambiguous attempts cannot reuse an earlier pass.
A rerun does not accept work, complete a failed assignment or resume a failed-check
sprint. For an open automated worker check, acceptance requires a retained technical or policy blocker
and the owner's explicit acknowledgement; manual items need no acknowledgement. Missing checks and real test failures
cannot be waived by that acknowledgement.

<a id="storage-retention"></a>
Maintenance runs once when a local project is activated and then every 60 minutes while the service runs (a service-owned loop, separate from sprint execution; passes are coalesced and never overlap). Done records are eligible for archive after the machine's
auto-archive interval `autoArchiveDays` (default seven days). A done record is archived without a question, with its work kept; a record saved with the retired archive-question fields loads as an ordinary Done or archived task and shows no question. Archive retention `retentionDays` defaults to 30 days, counted from the archive time (except a Done task newly archived by sprint closure, which counts from its completion time plus the auto-archive delay; see [closing sprints](#editing-saved-tasks-and-closing-sprints)), and is machine-configurable. The background purge is on by default (`automaticArchivePurge`: `true`) and can be switched off; the manual "Run cleanup now" is independent of that switch. Scheduled purge requires archived state (previously Done), an affirmative closure (merged, closed as-is by the owner, cancelled or closed externally) and a verified linked execution whose runtime state is COMPLETED, FAILED or CANCELLED. Paused, blocked, executing, awaiting-decision, unclosed, unlinked or ambiguous records, loaded sessions and unsettled attempts, processes or reservations are never purged, and eligibility and age are rechecked at the deletion fence. A scheduled pass never forces, and a transient refusal is retried at a later pass without writing a blocker. Unreferenced attachment files are swept by the same pass after `unreferencedAttachmentRetentionDays` (default 1). Runtime deletion reuses the existing exclusive-lock,
journaled retention cleanup and reports SQLite checkpoint/VACUUM outcomes. Storage shows a persistent count of purged records with failed space reclamation, including explicit purges, and a per-maintenance-run count.
Dirty worktrees explicitly allowed for discard can still be retained rather than
force-removed; their Git branch and files remain available. Owner queue/comment
records are removed with the purged item. No unrelated project is swept. Retention boundary tests inject a maintenance-only clock callable through an unshipped test launcher: neither the desktop protocol nor the packaged launch path accepts clock overrides. The host clock and existing task rows are never rewritten.

Remove project stops its service child and removes only the locator from the
visible registry. Files, branches, task store and product history remain.
Re-adding the same folder preserves its public project identity.
The project menu, the worktree menu and Code can open VS Code through a closed
native bridge action when its launcher is found, on `PATH` or in its usual
folders (`code`, `code-insiders`, `codium`). Without one the same control reads
Open in default app and hands the file or folder to `xdg-open` or `gio open`.
With neither, it is off and says what to install. The webview names a project,
checkout, file and line only; the service resolves the real path inside that
checkout, and the webview cannot supply a command or an arbitrary root.

The native window handles Alt+Left and Alt+Right explicitly for Back and Forward;
these shortcuts do not depend on browser-default navigation. A finished worker's
published report remains open when the same worker/report identity is refreshed.
Changing that identity closes the old report and discards its page cursor.

## Editing files

Local desktop Code opens files in the selected checkout read-only. Its
CodeMirror 6 editor supports tabs, dirty markers, find and replace, per-tab
undo and syntax highlighting. Ctrl+S or Save writes through the machine service; Autosave
in Settings, Appearance is off by default and saves about a second after typing.

Every save names the revision the owner last read, which is the SHA-256 of the
file's bytes. The service compares it in the lease-checked transaction, writes a
temporary file beside the target, syncs it, compares once more and renames it
over the target, keeping the file's mode. A save with an older revision writes
nothing and returns the current one. Reaching the file walks its folders one at
a time without following any link, so a link anywhere on the path is refused,
even one that stays inside the checkout. `.git`, `.cortex` and `.cortex-dev` are refused at any
depth. Files over 512 KiB, binary files, text that is not UTF-8 and files that
mix line endings open read-only with the reason; a file's own line endings are
kept. An uncooperative writer can still change the file between the final
comparison and replacement; the revision check is not a filesystem lock.

The open file is compared with the disk every three seconds and when the window
regains focus. A clean file follows the disk. A file with unsaved text shows the
conflict at once and keeps that text: Reload from disk takes the disk's version
and one undo brings the owner's text back; Keep mine saves it over exactly the
version that was shown, so a further change is another conflict. If the file was
deleted, Keep mine recreates it only while nothing else has.

Select **Edit** to obtain a file lease. **Ctrl+S**, **Save**, autosave and
**Keep mine** all require that lease, as well as the current base revision.
An unreadable store or unresolved writer binding does not establish that it
is safe to edit. For renewal, expiry, scope and recovery messages, see
[Code editing and write leases](ide-and-git.md#code-editing-and-write-leases).

Unsaved text lives above every screen, so moving between tabs, projects and
settings keeps it with its undo history. Closing a dirty tab asks first. Closing
the window by its button or through the window manager lists the unsaved files
and offers Save all and close, Close without saving or Keep working. A reply that
never comes leaves the tab at Save not confirmed: the disk is read once to see
whether the bytes landed, nothing is resent on its own, and Save asks again.
Unsaved text is held in memory, so a crash of the window loses it. To save a
parked draft, return to the file and reacquire the lease with **Edit**. Create,
rename, Save as and Compare are not built.

### The file tree

Code's tree shows every file and folder in the checkout's folder: what Git
tracks and the untracked files it reports, at once from the repository
snapshot, and everything else (dot-files, `.env`, build output and other files
Git ignores) from on-demand folder reads (`repo.list`). Ignored entries are
dimmed and named "ignored" to assistive technology. When Code opens, one read
names what the snapshot leaves out without walking into it: every ignored file,
the top-most ignored folders and the untracked folders, empty ones included. A
folder Git ignores (or, in a folder without Git, `node_modules`, `.venv` and
`__pycache__`) starts closed; opening it reads that one folder, one level, by
descriptor and without following links, and shows at most 500 entries with a
"N more not shown" row for the rest. Its subfolders are read when they are
opened. Opening Code or another checkout, coming back to it, Refresh files and
the owner's save there read again: the overview and every open folder, in one
call. Nothing reads on a timer. A folder inside a submodule lists its entries
without ignored marks, because Git cannot judge them. `.git`, `.cortex`,
`.cortex-dev` and the other Cortex private names are never listed, a link is
listed but not followed, and names that are not UTF-8 are counted, not shown.
Ignored files open, view and save exactly like any other file. Go to file finds
every file the tree has shown, listed ones included; Search in files searches the
Git files as before; the Git page and Changes
keep to what Git reports.

### Viewing files

Code shows a file by its name. Markdown, CSV, TSV and SVG open formatted, and
HTML and JSON open as source; a Preview and Source switch in the file bar moves
between the two in one click, each file keeping the last choice, and a jump to a
line shows the source. The preview shows the editor's text, unsaved text
included. Markdown uses the conversation's renderer, which never interprets
embedded HTML. CSV and TSV show a table of at most 1,000 rows; JSON is shown
indented, or says that it does not parse. SVG is shown only as an image, so its
scripts and external references never run or load. HTML is rebuilt from its
structural tags only: scripts, styles, forms, frames, links, event and style
attributes and remote images are dropped, and links are not followed.

PNG, JPEG, GIF, WebP, BMP, ICO and AVIF pictures are shown inline; MP4, M4V,
WebM, OGV and MOV video and MP3, WAV, Ogg, Opus, FLAC, M4A and AAC sound play
inline with their own controls. Their bytes come from the same guarded read as
text (`repo.file` with `media: true`): only these names are sent, as base64 of at
most 4 MiB, from inside the checkout, never through a link, and never for
saving. Whether a video or sound format plays depends on the media codecs
installed with the system webview; one that cannot be decoded says so. PDF files,
larger media and other binary files show a placeholder that names the reason and
offers Show as text, which opens the file read-only with its reason when it has
no text.

## Automatic task titles

New task requires a description, not a title. After the task is saved, the app
asks the machine to name it (`product.task` `generate_title`). The machine starts
a background job and answers at once with its id; saving and starting never wait
for naming. Until a title is saved, the description's first sentence (at most 60
characters) is shown. The title is written by the cheapest model of the
connection whose plan limit is the least used, as described in
[short model texts](#short-model-texts). The prompt asks for at most eight words in
the language of the task text; the stored title is one line of at most 80
characters.

When no route, hardened engine session or usable answer is available within the 25-second budget,
the job saves the description label with `titleState: "failed"`, and the task
shows "Title not generated" with Retry. Retry starts one new job. A job is never
repeated by itself, so a restart or a lost reply never repeats a possibly billed
call. The title is saved only under a free `machine.lock`: the job tries again
for up to 20 seconds in the background and then drops its title instead of
waiting or reporting busy. It never replaces a title the owner wrote (a rename
or a draft title, `titleState: "user"`) and never writes over a purged task.

A materialized sprint task is named by its plan item's own title: joining the
sprint (`product.task` `sprint` with `title`) records it as the task's title with
titleState=generated, not as an owner rename, and never over an owner's title. An
item without a title of its own keeps the description label. A user rename always records titleState=user, even for identical text.
No storage migration or wire-version change is required.

### Automatic sprint titles

A new sprint is created without a title (`title: null`) and shown by the label of
its description. Right after `product.sprint` `create` saves it, the service starts the
same kind of background job for the sprint and returns its id as `titleJobId` next to
`sprintId`; creating never waits for it, and every client (desktop, phone) gets the
result. The job uses the same route choice as task titles (cheapest model of the
connection whose plan limit is the least used, so Haiku or Luna by current usage) and a
sprint prompt: at most eight words summarizing the whole sprint text, in its language.
The title is saved with `titleState: "generated"` only while the sprint has no title
and no owner rename (`product.sprint` `patch` with `title` records `titleState: "user"`).
Without a route or a usable answer nothing is saved: the sprint keeps showing the label of its
first sentence, and a retry stays possible. `product.task` `generate_sprint_title`
(`{sprintId}`; workspace command `generate_sprint_title`) starts one new job; it answers
`{"jobId":null,"skipped":"owner_title"}` for a sprint that is already named. `short_text_poll`
reads a sprint job with `sprintId`. When the job ends the app rereads the project.

## Short model texts

Task titles and Commit, squash and Push drafts use one short text mechanism
(`crates/workspace-service/src/machine/short_text.rs`; route choice in
`crates/workspace-service/src/native_routes/short_text.rs`). Commit and squash
return an English subject and detailed body from a file list and up to 12,000
UTF-8 bytes of diff excerpts. Push uses subjects, bodies and aggregate diffstat
from at most 20 outgoing commits, with up to 12,000 UTF-8 bytes of that context.
Clipped excerpts are marked. Git inspection is local. Known private paths,
binary content and recognizable credentials are omitted where applicable; the
filter is not a guarantee against secrets in arbitrary source or commit prose.
The selected provider receives the resulting excerpt. A matching, previously
confirmed Push summary is reused as the editable PR description, not the title.
Without a matching summary the PR draft uses a local Git metadata template.
A generated Push draft is fitted to 8,000 UTF-8 bytes before the owner sees it.
The confirmed summary is saved whole: the Push dialog shows the limit, names
an over-limit byte count and disables Push, and the service refuses a longer
summary with `push_summary_too_long` before running Git, never shortening it.

- Route: among the routes the owner allows for automatic choice (enabled, not
  denied, listed in `auto`, placed in a tier) on Claude or Codex native
  connections, each connection offers its cheapest model by tier placement
  (`light`, then `standard`, `deep`, `flagship`; equal tiers by model id). The
  connection is the one whose plan limit is least used, read the same way route
  ranking reads it: the highest used percentage of its live windows. Connections
  with a reading below 95 % come first, lowest first; then connections without a
  reading (unknown, never zero); then those at or above 95 %. Equal keys keep the
  fixed order Claude, then Codex, then connection id. With the shipped tiers the
  Claude choice is `haiku` and the Codex choice is the `light` Codex model.
- Execution: one minimal ACP engine session on that route
  (`crates/workspace-service/src/short_text_engine.rs`). It is launched with the
  route's ordinary engine-session launch, so it carries the same hardening as
  every engine session: a cleared environment with the real `HOME` and the
  connection's own native directory, and for Codex Cortex's native launcher,
  which turns every MCP server configured in the owner's Codex account off,
  confirms that with `codex mcp list` and the disabled features (no shell tool,
  plugins, hooks, apps or subagents) with `codex features list` before App
  Server starts, and trusts only the session's own folder, without writing the
  owner's `config.toml`. A Codex connection without that launcher is not used
  for short texts. The session runs in an empty private scratch directory that is removed
  afterwards, is given no ACP MCP servers, and is set to the chosen model, `low`
  effort when the catalogue offers it, and the engine's read-only mode
  (`read-only` for Codex, `plan` for Claude; an engine offering neither is not
  used). It receives one text prompt. Every permission request is answered with
  the reject option (or cancelled when none is offered), and a permission
  request or any tool call makes the answer unusable. Only the agent's message
  text is kept. The whole run, engine start included, has a 25-second budget;
  the engine is then shut down and its process group reaped. The session has
  no runtime task or channel, writes nothing to a project store, takes no
  `machine.lock` and never appears in a task's history. At most two runs go at
  once; a job over that uses its fallback text.
- Jobs: `generate_title`, `generate_sprint_title`, `draft_integration_text`, `draft_checkout_commit` and
  `draft_push_summary` return `{"jobId"}`. `short_text_poll` takes the job ID and,
  for task-bound jobs only, the task ID. A finished job reports `source`
  (`model`, `fallback`, `template` for unmatched PR drafts, or `saved` for a
  PR body from Push), `reason` and `route` (when a model route was selected).
  Draft actions return without waiting for generation. An unavailable route,
  unreadable input, timeout, refusal or unusable answer leaves editable text
  and a reason; no alternate model is tried. Title persistence can take
  `machine.lock` in the background. Jobs live in memory for ten minutes; after
  a restart a poll is refused as `unknown_job` and no job is replayed.
- Diagnostics: `short-text.log` in the service log folder records
  `short_text_selected`, `short_text_generated`, `short_text_fallback` and `short_text_saved_push` with
  purpose, engine, model, connection id, tier, plan reading, choice reason,
  outcome code and duration. It never records a prompt, a description, a diff
  or an answer.

## File mentions, commands and skills in a message

The message fields offer inline suggestions. What the engine receives is the
owner's text exactly as it stands in the field when it is sent; no suggestion
adds a hidden content block, flag or instruction.

- `@` names a project file. The list filters the file names of the project's
  repository snapshot (tracked and untracked, not ignored, the names Go to file
  uses), plus the task's own changed files, in the window. Picking one writes
  `@<project-relative path>` (quoted, `@"a b.md"`, when the path holds a space).
  The engine receives that text inside the message and reads the file itself
  through its project tools, since it works in the project folder. Mentions are
  not sent as ACP `resource_link` or embedded resource blocks: the native prompt
  carries the owner's text inside Cortex's own instructions and recovery capsule
  as one text block, and only stored attachments become further blocks.
- `/` at the start of a line in the task's message field lists that screen's
  own controls: `/stop` (the title band's Stop or End run, while running),
  `/resume` (Resume, while paused), `/queue` and `/now` (the delivery choice),
  `/details`, `/changes` and `/code` (the task's areas, where they are offered)
  and `/new` (New task in this project). Picking one runs that control and sends
  nothing.
- The same `/` list names the project's own skills, read with `repo.skills` from
  the index `read_skill` uses. Picking one writes the sentence
  `Use the project skill <name> (<path>).` (or `Use the project skill at <path>.`
  when a Cortex skill has the same name). The coordinator receives that sentence
  as part of the message; its instructions already list the project skills and
  it reads one with `read_skill`. Nothing activates a skill on its own.
- Engine slash commands (`available_commands_update`) are not offered. The
  runtime does not keep them, and a message reaches the engine inside Cortex's
  prompt, not at its start, so an engine would not run `/name` typed there as a
  command.

## Attachments

Every form where the owner writes a message or creates work accepts a file picker,
dragged files and pasted clipboard images: new task and new sprint, the task
conversation (including paused resume and review discussion), queued messages,
every answer inside a question card, planning messages, Request
changes, diff comments and conflict-resolution guidance. A message to one worker (guidance) is the exception: the native service carries no files to a single worker, so its Attach button stays off with that reason, while a message about the task takes files. Text-only fields such as
names, branches, searches and commit messages take no attachments. Files appear as
removable chips with their name and, for images, a small thumbnail. A queued
message is the exception: its row in the queue shows no chips. The message keeps
its files; Edit returns them to the composer as chips, and once sent they open
from the conversation. A rejected file
(too large, too many, unreadable, an unusable name or type, or a machine without
attachments) shows its reason beside the form until the next successful action;
no part of that selection is silently sent.

Saved files stay available where they were sent. A task page reads its creation
files on demand, sprint descriptions and planning conversations show theirs, and a
sent conversation message keeps a reference to its files in the service's private
product state. A chip is drawn from the reference alone (name, type, size); its bytes
are read when the owner opens it. A chip that can be opened opens a preview of an image or, for any
file, offers **Save a copy** through the system's save dialog; the owner picks the
destination. A chip with only a name cannot be opened: worker-guidance records carry
names, and an older recorded copy may have kept only the name. A file whose saved
body is gone says so when opened. Answers to questions (single, batch and
sprint planning) keep their files with the delivered answer, and the answered card
shows them as chips. The service's product state references an answer's files as it
does a sent message's, so they stay openable while the task exists, archived
included, and go with the task's deletion (see [attachment files](consistency-and-recovery.md#attachment-files)). Each submission permits up to eight
files and 4 MiB of combined decoded bytes. A question batch shares that bound.
Text is required alongside attachments.

Queue saves the text and its file references in the service's private product
state. It survives window closure; **Edit** removes the queued item only after acknowledgement
and restores text and attachments to the composer. Existing composer text is
preserved before the restored text. A failed send retains the draft and files;
an ambiguous response is not automatically retried.

A file crosses the local wire as bytes once: when the owner attaches it (picker,
drop or paste), the form uploads it with `machine.call` `product.attachment`
`{action: "upload", files: [{name, mimeType, data}]}` (standard base64, at most
eight files and 4 MiB together), and the service answers one reference per file,
`{name, mimeType, size, sha256}`. Every later command and record carries only that
reference: `tasks.create`, `decisions.answer` (single and batch), `executions.prompt`,
the saved-task edit, `product.message` add/edit (queue), `product.comment` add/edit,
`product.sprint` create/note, and the window's pending and failed sends. Each of them
refuses inline `data` with `inline_attachment_refused`, and a reference whose body is
not saved with `attachment_unavailable`. The service reads each body when it hands
the files to Core (draft options at create, source blocks at Start, a message or an
answer) or builds a planning prompt, and nowhere else. Replies (`product.read`,
`product.facts`, `product.board`, a task's files) carry references only; one body is
read back by `product.attachment` `{action: "read", sha256}` (at most 4 MiB, by
digest only, checked against it). The service rejects path-like names, malformed
encoding and excessive input. Accepted resources are snapshotted in private `.cortex` source-block
storage and exposed through runtime prompt-resource references/`read_source`;
images use ACP image content, other files embedded resources. Answer text remains
unchanged; its identified answer, resources and wake-up references commit in the
same transaction. Stale or rejected answers capture nothing. Queue removal is not
a resource-delivery claim. No arbitrary URI is fetched and no attachment is
executed.

The native window disables Tauri's drag interception so WebKitGTK receives file
drops. On WebKitGTK versions that expose only URI text for native drops, a GTK
drop observer snapshots those exact files and offers a one-use, five-second buffer;
the webview cannot submit a filesystem path. WebKitGTK does not give the page a
clipboard image: it reports nothing, or only the copied page's `text/html`. A paste
with no plain text therefore asks the native clipboard for PNG, then any other image
GTK can decode, re-encoded as PNG within the same 4 MiB limit. Plain text is pasted
as usual, and no clipboard text or file path is read natively. Task creation retains
validated bytes in the private draft options until explicit Start snapshots them.
Native delivery: the draft's files are decoded and checked once more inside the Start admission and committed as the task's first source revision (private bodies in mode 0600 under the task's `.source_blocks`, SHA-256 digest and size recorded; the admission event carries no file bytes). An owner message (follow-up, Request changes, resume guidance) or a question answer retains its files as the next revision, in the same transaction as the message or answer, bound to the request by a fingerprint of the exact bodies, so a replay retains nothing twice. All of a task's retained bodies together stay within 10 MiB. Each engine session receives a body once, as the engine's advertised prompt capabilities allow (images as image blocks, text as resources, PDF only for Claude, anything else as a named marker), and later prompts refer to it by marker. Direct worker guidance carries no files (`native_attachments_unavailable`). Resource references,
not filesystem paths, are delivered. Neither saving a draft nor attaching a file
starts a model call.

The data contract also accepts optional attachments for sprint creation, planning answers/messages, review
comments and conflict-resolution guidance. Sprint descriptions and conversation entries list their saved
files by reference; a file's bytes are read when it is opened. The planning task receives them as its own
draft attachments, the same ACP image/resource representation as task inputs, and never places base64 in
prompt text or diagnostics. A sprint's initial files and all planning follow-ups share eight files/4 MiB
in total because a retry resends that input; excess is refused before saving. Omitted attachment fields on
older records remain valid. The private product-state response has a bounded aggregate size below the
8-MiB local frame cap; a full state refuses new content rather than saving an unreadable record. A task's
creation files are listed on demand through `readTaskAttachments` on the real port, backed by the
owner-only targeted `product.read` selector, as references (their bodies are saved privately on that
read), rather than copied into every board snapshot.

## Task Stop and Resume

Task Stop records `PAUSED` independently of ACP turn cancellation, from a running,
task-level or worker question/access wait, or between-turn state. It preserves pending
question identities, batches and assignment-local waits transactionally; questions
stay visible with answering disabled, and runtime access reconciliation is held.
Resume restores the saved state without interrupting or replanning; a question
wait still requires the original answer and stays answerable after Resume. Work
interrupted by Pause continues through the driver's own lost-worker retry or
coordinator recovery rather than replaying the interrupted prompt. The hold survives service restart, and only
an explicit owner Resume reconstructs the exact recorded session metadata. Resume
refuses a stale display version or changed owner. Stop is urgent intent for the
exact execution and does not require the background progress version to remain
unchanged. Close as is and Cancel task are separate confirmed terminal controls.

In Workspace the task composer is the one place that controls the run, as in other chat
clients: with an empty field its action button is Start on a draft, Stop while the task
runs and Resume while it is paused; as soon as the field holds text or files it is the
paper plane (Send now or Queue, or Send and resume), and clearing the field brings Stop
or Resume back. Enter never presses Start, Stop or Resume. The title band shows no state
chip and no Start, Stop or Resume; the task's "…" menu keeps Start, Stop, Resume and
Continue. A task waiting for the owner's answer, a held task and a result in review have
no Stop in the composer. Where a machine offers no message composer, Start, Stop and
Resume stay in the title band. A sprint's planning composer works the same way: Stop
cancels planning and Resume continues it.

One rule (`resumeState` in `packages/workspace-ui/src/session-model.ts`, read from the existing task projection only) decides the status text and the single action for the board card, task menu, command palette, title band, status strip and composer:

| Situation | Status shown | Action |
| --- | --- | --- |
| Owner Stop | Paused by you | Resume (versioned) |
| Interrupted turn or recovery hold waiting for the owner | Interrupted or Stopped, waits for you | Continue, or send a message |
| Safe automatic recovery (worker lost, service restart) | Recovering automatically | None; the service continues from the recorded state and replays nothing |
| Owned process exit or effect unconfirmed | Waiting for exit evidence, or for processes to stop | None; the reason is shown, and Resume and Continue stay off, also on a paused task |
| Cancelled or failed run that still needs closure | Run cancelled or Run failed | Close task; ordinary Request changes remains separate where available |
| Eligible closed task | Closed | Return for rework, not Resume or Continue |
| Archived task | Archived | Restore first; rework is unavailable while archived |
| Unknown or offline | Unavailable until the task's state is confirmed | None |

Stop keeps its behavior. Before the runtime exists (an engine launch still opening) there is nothing to pause, so Stop cancels the launch and the task is stopped. A paused task keeps its changes in its existing checkout, including a dedicated worktree if one was selected. A restart of the service does not discard or replay them.

Coverage, all no-paid: the `paused-checkpoint`, `hold` and `command-restart-lost` scenarios of `binary_journey` in `crates/workspace-service/tests/native_desktop.rs` start the compiled service with fake engines in a disposable Git project. The first pauses a task after worker progress, kills the service with SIGKILL, restarts the same binary and resumes with the current version: task, plan, node, report and attempt identities, history order and file contents are unchanged, the interrupted node gets a fresh attempt and the task completes without repeated side effects. `hold` continues without any owner click, and `command-restart-lost` keeps an unresolved command held and never relaunches it. `packages/workspace-ui/test/recovery-controls.test.tsx` covers each row of the table, including that terminal and unknown states offer neither Resume nor Continue. Rendered installed-desktop qualification of these controls was not run. A safe service-restart re-drive of a manual task cannot be told apart from other holds in the existing projection, so the UI does not label it separately.

A started task whose runtime row does not exist yet (its coordinator's engine is
still opening) projects as `starting`, not unknown, as long as its start, session
and owner facts agree. Retry recovery may advance the owner epoch beyond the original launch binding while keeping that same task/session; older epochs remain invalid. Stop and the Commit dialog's “Stop the task first” cancel this exact launch through retries; prompts and worker controls do not receive an epoch exception. Each automatic engine-launch retry shows the runtime's own
retry notice as the task status; a give-up projects as `blocked` with that notice
and needs the owner. There is no state to pause yet, so Stop cancels that launch
through the runtime's ordinary task stop and the task becomes stopped; a stopped
launch no longer holds owner checkout actions that require stopped tasks. A new service process still treats an
unconfirmed open launch as unknown.

This is not engine-process suspension. Stop fences old worker channels. Pending
worker waits retain their exact decision identity, including across service restart.
After Resume and the actual answer or runtime access decision, the runtime records
a replacement attempt for the same node and delivers the saved decision as source
data. This is explicitly a new session, not same-session recovery; completed work
must not be repeated and required access must be revalidated on the new channel.
Unknown action outcomes or unconfirmed process termination still block replacement.
Resume is runtime continuation, not a new user message, answer or requirement.
Stopping an individual worker or closing the task remains cancellation of its waits.
No promise of reviving a lost engine or undoing an external side effect is made.

## Worker reports and older activity

Open report reads only a published report referenced by a retained worker result
in this project. Pages have a verified immutable digest, identity-bound cursor,
4,000-character maximum and explicit partial/end labels. The view uses plain text,
not executable HTML, and never opens private runtime paths.

Older activity uses one continuation for both task events and command-control/guidance
rows, bound to the project, task, worker and attempt. Both event positions advance
together without repeating or skipping a namespace; newer arrivals stay in the live
tail. One read transaction supplies each page. Each worker DTO is at most 64 KiB,
with at most 200 combined events. A snapshot shares a 512-KiB history budget and has
a 768-KiB envelope ceiling: deferred history retains its own continuation rather
than disappearing. Oversized metadata or more than 100 workers for one task refuses
with a narrower-read message, rather than silently omitting workers. Text fields
are limited to 8,192 serialized UTF-8 bytes (command previews: 2,000), including an
explicit clipped-prefix/original-byte-count label. Original stored content is not
changed. Old task-only cursors require reopening history.

Older guidance pages carry delivery status and attachment names into the worker
view. Activity filters include recorded failures, and scrolling away from the end
stops following until the owner returns to it or presses Latest. Loading older
events does not count as new live activity.

## Local voice input

Ctrl+D or the microphone starts and stops on-device Russian or English dictation
in a composer. Settings → Voice input saves the language for that machine and
channel. Russian is the default; message language does not change recognition.
The setting cannot change during recording or download and applies to the next recording. Partial recognition appears live inside that composer; stopping leaves
a preview, never a message. Cancel discards only the recording. Insert appends the
final transcript to the editable draft. Insert and send is a separate explicit
action where that composer supports sending (including the answer inside a question card); new-task
dictation never creates or starts a task.

The service captures the default desktop source through PulseAudio's parec
(including PipeWire's PulseAudio compatibility server), not Web Speech or a cloud
speech service. The package contains sherpa-onnx-node 2.0.1 and uses bundled Node.
The fixed small Zipformer models are separate: Russian (109,906,654 bytes, about
105 MiB to download) and English (112,232,184 bytes, about 107 MiB). On first use
only the selected model downloads, with visible progress. Settings shows both
models' integrity status and can download the selected model without recording.
The worker checks the archive and model files against committed SHA-256 hashes
before installing the model in the channel-private cache. A failed or cancelled
download is not installed; retry starts a new verified download. No audio is
uploaded or written to disk.
Only explicitly sent transcript text enters the ordinary Cortex prompt path.

Recognition refreshes partial text using bounded overlapping offline inference;
it is not a token-by-token streaming model. Long utterances are segmented on
silence or at 20 seconds. A recording stops at 90 seconds and still requires
Insert or Insert and send. The service allows one microphone lease at a time;
a lost composer/window lease expires after ten seconds. Service stop cancels it.
Missing capture tools, audio-server/microphone access, unavailable local runtime,
failed download, failed integrity or a lost session are reported, not simulated.
There is no automatic language detection or mixed-language mode. Fixture checks
do not establish physical-device recognition quality. The optional rendered
English fixture uses `voice-model-en/` and `voice-fixture-en.wav` in the
`check-rendered` artifact directory alongside the Russian artifacts. Obtain
the English recording from `test_wavs/0.wav` in the pinned model archive.

## Explicit limits

- MCP Connect, Check tools and Retry list tools from the unsaved draft through the existing sandbox, without saving or calling them. Listings are bounded and redacted; configured grants remain editable in the matrix. Dismissing a check ignores its eventual reply; its server stops at the deadline. A sandbox refusal remains a refusal, never an uncontained retry. The real sandbox fixture requires working user namespaces and is not qualified on hosts that refuse them.
- Open logs opens private rotating status events: timestamps and fixed categories only, no raw service output. It uses `xdg-open`, then `gio open`; with neither, the control is off and says what to install. An opener's failure is shown as that failure, and one still running after eight seconds counts as handed over.
- The two-task, windowless real-service fake-engine sprint journey and forced failed/skipped-check rerun receipts are not yet qualified. Check-rerun remains a request, not proof that a check ran.
- Ctrl+J opens the local project shell; hiding or closing the window preserves it, while Close shell stops its jobs. The initial cwd is the registered project folder even in a task view, not the task worktree. See [terminal authority and limits](security.md#owner-terminal-built-local-linux). Remote-machine controls remain off; local voice input is described above.
- Native rendered qualification must run on a newly built binary. Source tests
  and a failed native build are not rendered evidence.
- This slice is not a new Debian package. Stamping, packaging and installation
  qualification remain separate release steps.

## Focused no-paid checks

Use sequential checks under `umask 022` with `~/.cargo/bin/cargo`; do not run the entire suite.

```sh
timeout 900 ~/.cargo/bin/cargo test --locked --offline -p workspace-service --test task_actions --test service --test machine
~/.cargo/bin/cargo test --locked -p workspace-service --test terminal
~/.cargo/bin/cargo test --locked -p cortex-workspace-desktop --no-default-features
npm --prefix packages/workspace-ui test -- test/terminal.test.tsx test/terminal-client.test.ts test/voice.test.tsx
# Voice: needs voice-model/ and voice-fixture.wav beside the built binaries, else the stage is recorded as not_applicable:
scripts/cortex-xtask.sh check-rendered --artifacts <build>/artifacts --only voice
# Against freshly built native binaries; disposable no-network container, no images:
scripts/cortex-xtask.sh check-rendered --artifacts <build>/artifacts --only terminal
npm --prefix packages/workspace-ui run typecheck
npm --prefix packages/workspace-ui test -- test/real-port.test.ts test/real-screens.test.tsx test/store.test.ts
```

The fake-gh fixture uses a local bare remote and a disposable executable; it proves invocation and duplicate-attempt guards, not GitHub behavior.

Git-rule fixtures inject the runtime-result validator; they establish Git safety,
not runtime verification. The service journey uses the real adapter/store with a
fake runtime, creates a real task worktree, confirms its own file/diff and single
store, and retains detach/restart behavior. No check calls a paid engine.
