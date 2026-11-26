# Security policy

Workspace task menus and the command palette expose the same existing controls. Opening them does not grant permission to run a command or bypass service checks. Actions that require confirmation still ask before dispatch.

Coordinator effort defaults are resolved against the levels advertised for the selected route. Removing the legacy effort sentinel does not change model eligibility, ceilings or provider authorization; explicitly unsupported concrete choices are still refused.

Workspace reserves a shared checkout for independent change review and host verification. Sprint plan and sprint revision reviews judge proposals, not checkout bytes, and neither wait for writers nor take a new reservation. A sprint revision review preserves the task's existing reservation after the current revision passes host verification. Other reviews run without that reservation only when there are no attributed project effects or unresolved change observations. Review of project changes requires and binds the task's own changes (gateway write receipts, and paths that changed while one of its commands or writing-engine turns ran); an edit of those files, by anyone, invalidates the reviewed digest, while other actors' uncommitted files are listed as context and neither required nor bound. Incomplete attribution retains the scheduling reservation without making unrelated files required reading. See "Review of the task's own changes" below. A worker continuation after a pending command reply or premature result submission requires a settled receipt for each named command on its own channel. Review packets identify earlier writes by task, revision and recorded event. Where available, the last write hash is compared with a fresh full-file content hash. Git-ignored files are not deliverable: their changes never make a review packet's change set incomplete. When the host cannot enumerate the change set, an approval still requires complete current reads of every listed changed file and carries a disclosed `unverified` runtime check for the unlisted part; it is never recorded as reviewed. A listed file whose current bytes have no reviewable text (binary, an image the reviewer did not read directly, or opaque encoded/minified text) is likewise never recorded as reviewed: an approval carries a disclosed `unverified` runtime check naming it. Past three rebinds, a review whose project keeps changing is refused until two submissions see the same state; it is then re-bound to that state and must be resubmitted, so it never certifies a state it did not bind. Missing receipts preserve the unknown-outcome barrier.

Execution baselines record the project checkout’s resolved Git HEAD, or null for an unborn branch or a plain folder. Ignored-file evidence is a separate metadata comparison, not a content check. If its bounded listing or metadata pass cannot finish, the ignored digest is absent and cannot support a review waiver; a complete deliverable manifest is still usable. An incomplete deliverable observation does not become complete as a result.

Workspace’s machine Board groups task data already loaded for the selected machine. Opening the Board does not send task commands or expand access; its inherited card actions use the existing task capability checks.

Workspace file-wait notices link only to a loaded task in the same project. The notice and link are read-only: they do not change leases, scheduling or task-control permissions.

Workspace desktop tooltips show text already available to the client. Showing or dismissing a tooltip does not call a service, change permissions, or enable an unavailable control. Disabled reasons remain available independently of tooltips.

Updating a task from its target advances only the Changes comparison baseline, not the task’s original integration base. If a retained update is committed outside Workspace, Finish merge verifies that HEAD has the saved task and target commits as its two parents before advancing the comparison base. This check does not modify Git. The updated tree still needs a fresh result and review before integration.

Returning closed work for rework requires an explicit owner action, current
task version, and matching execution and owner epoch where an execution exists.
It does not let an ordinary late message reopen closed work, bypass an
unconfirmed execution or side effect, or carry acceptance into the new cycle.
An Automatic close after an integrated restart uses only new-cycle write
records. If unmerged work was retained in place, a historical path alone does
not authorize committing it: its bytes must still match a retained write hash.
A confirmed PR on the previous branch is history once an integrated restart
creates a replacement owned branch. An unresolved PR attempt still blocks
another PR; rework is not permission to replay it.
See [rework admission](docs/cortex-workspace/local-task-execution.md#return-for-rework).

The desktop can offer a fresh close-and-delete confirmation after refresh only
for a retained dirty or occupied cleanup that passes the current read-only
preflight. This hint does not reopen the checkout or grant access to its files.
A new confirmed request can retry only after an earlier dirty or occupied result. It rechecks task quiescence, checkout
ownership and cleanliness before non-forcing Git removal. Uncertain outcomes
are not requeued, and a recorded removal is reported without deleting again.

Ordinary Workspace Git Commit does not rerun or replace verification. For an accepted native result in the project folder, the service can retain the original result identity only when the commit is a direct child on the accepted branch, its full Git tree matches the accepted deliverable paths, bytes and Git file modes, and the resulting index and checkout are still current. The proof compares raw file bytes rather than relying on Git status or running filters. Incomplete observations, changed files and every unproven checkout keep the existing stale-result checks. Creating a pull request still needs owner confirmation and a separate push. Commit and squash generation sends bounded Git diffs and file lists, and Push generation sends bounded outgoing commit prose and aggregate change counts, to the configured provider. Git drafts do not send task prose. Owner-entered text is not translated, and the confirmation boundary is unchanged.

The extracted Workspace Dev `smoke` and `drive` launchers check only the active GitHub CLI account on `github.com`, under a timeout. They retain the exit code, not authentication output. Broader host diagnostics, including other provider, Docker and sudo checks, run only when `probe` is explicitly invoked.

A skipped check in Workspace result facts records its reason in `detail`, including when paid qualification was not requested. The row is not execution evidence and grants no permission to rerun a prohibited command.

Automatic prefetch of earlier Workspace conversation uses the same authorized history read as **Show
earlier**. The task, session and page-size cursor bindings, service display filters and remote
operation allowlist are unchanged. Paging does not send messages or start an engine.

Coordinator language reminders are prompt guidance, not output filtering or authorization. They do not change who may ask or answer a decision or how the runtime validates an answer.

Conversation fallback text grants no decision authority. Answers still require the existing decision identity, capability and runtime validation. A draft kept visible in a read-only composer remains unsent; its explanation does not enable message delivery or change the capability check. For route consent, an option plus a comment must match exactly one recorded option in the stored answer format. The comment cannot grant another route or set effort, but recognized refusals still apply.

Workspace owner draft creation and active-task settings have reserved writer
slots, not a separate store or authorization path. Settings still validate the
selected route and recheck task ownership before committing. A sprint link
checks the saved task and sprint under the existing machine-state lock; an
already recorded link can be acknowledged without editing a started task.
The draft receipt alone does not confirm membership.

Workspace model catalogues distinguish connection readiness from policy permission:
engine/model denials make routes unselectable for new work. Displayed unknown
usage is Unavailable, not zero; it grants no quota or execution authority.

Engine limits refresh in the background with two subprocess reads that start no
model turn and cost nothing: Claude Code's local `/usage`
(`claude -p /usage --output-format json --restricted --strict-mcp-config
--no-session-persistence`) and the Codex app server's `account/rateLimits/read`
(`codex app-server`, sending only `initialize`, `initialized` and that request).
That App Server carries the same non-persistent feature layers as every
Cortex-launched one (plugins, hooks, apps and subagents off), so the owner's
plugin marketplaces are not synced, for example by a `git ls-remote` to a
configured remote source, each time the limits are read.
Each runs the verified native CLI of the qualified adapter, never a PATH lookup,
with a cleared environment: the owner's real `HOME`, absolute `PATH` entries,
locale, and the connection's own verified, private, unshared native directory as
`CLAUDE_CONFIG_DIR` or `CODEX_HOME`. No credential is read by Cortex, copied or
logged, and nothing logs in. The child runs from an empty private directory in
its own process group, killed after 20 seconds; its output is bounded to 1 MiB,
stderr is discarded and raw output is never stored or logged. A Claude result is
trusted only when its JSON proves a local command with zero turns, cost and
tokens; any other shape is discarded. Reads are rate-limited to once per
connection per five minutes under a nonblocking lock, only while settings are
being read, and write only the owner-only `<state>/cortex/quota` cache.
`CORTEX_WORKSPACE_USAGE_REFRESH=off` disables them.

Native project-file writes distinguish hash conflicts, missing/partial read
coverage and stale read coverage before replacement. Their structured errors
report rejected writes and correction guidance, not authorization refusals.
Existing-file replacement requires complete non-truncated same-channel coverage;
a supplied digest alone grants nothing. Deferred batch reads do not add coverage
for writes or independent review, even if their retained output is fetched, and
their page no longer reports the coverage it would have added. A binary read
(single or batched) is hash-bound coverage of its bytes; a batched image read,
which never delivers the image, grants none. Creation requires the absent-content
precondition. Exact edits may use a current partial text read but reject missing
or ambiguous anchors, including overlapping Unicode matches, atomically.
Actual private-path, role, channel and write-denial decisions remain final.

Native incoming ACP now persists client-owned Development/Guarded/Auto access
separately from display history. Worker engine modes must be actually offered
and confirmed after model selection; route parameters cannot override that
floor. Permission decisions recheck live channel/session identity and access.
The outside-write/command gateway requires a writing worker and checks
assignment waits and payload-bound durable identities. Guarded and Development
ask for each eligible action; Auto settles it automatically. These are
unsandboxed owner actions, not a containment fallback. No-follow file replacement
does not isolate hostile same-user code. Native command output is now retained
whole in private files and hash-checked when paged; a clipped inline
prefix alone is never complete evidence. A current channel may read its own
coordinator/assignment scope's older channel output, never a sibling assignment's.
Legacy output reads retain status/encoding but explicitly lack native integrity
certification and cannot manufacture a native check receipt. Ordinary passed worker checks require an
exact successful ordinary or confirmed outside-command receipt, intact native
retained output and either an unchanged before/after project digest matching
submission, or, for `command_run`, an unchanged before/after digest of the run's
work set (its assignment's declared scope plus the paths its own tools wrote before
the run) that still matches at submission, with no later write of that assignment
outside the set. The work-set binding proves only that the worker's own declared
work is what the check ran against: files outside the set, including other tasks'
concurrent changes the check may depend on, and changes the worker's own commands
made outside its scope, are not bound. Independent review and final runtime
verification bind the task's own changes (see "Review of the task's own changes"
below), or the exact whole checkout when attribution is incomplete.
The runtime derives evidence strength; duplicate, unknown, read/search/no-op and
unproved inline-shell passed claims are retained unverified, not certified. An
inline `sh`/`bash`/`dash -c` script is proved only as `umask`/`export`/`cd`/`set`
setup joined by `;` or `&&` and then one simple nontrivial command, with no other
operator or expansion; this is a bounded recognizer, not a shell security boundary.
Observed nonzero exits remain failures. Exact latest reruns can close failures
without rewriting history. Explicit exploratory withdrawals retain retracted
failures; supported source-bound standalone probe corrections and Python argparse
startup corrections retain retracted/superseded failures, never synthetic passes.
Complete output, same-attempt identity, ordering and unique correction links are
rechecked at completion; stored links alone grant nothing. Static probe analysis
never executes submitted source and refuses unsupported forms. General shell
status-flow equivalence remains unfinished.

### Review of the task's own changes

A shared checkout holds other actors' uncommitted work, so a review is required to
read, and binds, only the files this task changed. A file counts as the task's
change when one of its channels' gateway file tools recorded a write, move or
deletion of it, or when it changed while one of the task's commands ran (writable
`command_run`, owner-confirmed `outside_command`) or during a writing worker's turn
whose engine can change files without a receipt (codex-acp and OpenCode always;
Claude Code only when the runtime allowed a non-gateway, non-read request in that
turn). The host lists the checkout's dirty deliverable paths (Git status with
untracked files, plus size, mtime, ctime, mode and inode; never Git-ignored files;
a plain folder is walked) beside the launch of each such run (never delaying it;
an observation still running at the payload's release is kept only if an inode
change-time sweep from that instant shows the checkout untouched, and otherwise
attributes conservatively, as described in
[permissions and containment](docs/features/permissions-and-containment/index.md))
and after it, and records the paths whose entry changed on its finish record. Attribution is conservative: a path that
changed during the window is attributed even when another actor changed it at the
same time. Every other changed file stays in the packet as context
(`required: false`) and is never a coverage refusal. The review, its re-bindings,
verification and completion compare a digest of the required files' current
states (content hash and mode, link target or `absent`; metadata including ctime
past a 256 MiB hashing budget), so an edit of a task file by anyone still refuses
the verdict or invalidates the approval, and an edit elsewhere does not. When any
command or counted turn since the baseline lacks a complete change record (an
earlier build, a failed or oversized listing, a nested checkout, an unknown
outcome, a run still unsettled), the packet says so (`attribution_complete: false`
with its reason), every listed file is required and the review binds the whole
checkout, exactly as before. Limits: approval and verification certify the task's
files and that the plan's commands passed on the real checkout as it was when they
ran, not other actors' concurrent changes, which no review saw. A change restored
to the same status record and metadata within one run, changes inside a nested
checkout (which make the record incomplete) and an engine-native background
process that changes a file between two turns of its worker are not attributed.

Question-card delivery retains the prompt's cancellation generation, so cancelling
cannot be bypassed by a late card from that prompt. Fresh prompts retain their
independent permission lifecycle. Incoming Rust question cards and engine permission
requests have no human-response deadline; their bounded pending correlations are
removed on response, card retirement, session cancellation or connection shutdown.
A response after cancellation cannot authorize an action. This does not infer
process settlement. The Paseo shim's target environment allow-list includes only
HOME, XDG_CONFIG_HOME, XDG_STATE_HOME and the public CORTEX_ACP_RUNTIME_ROOT;
all target values must remain absolute paths.
A Paseo session's cancel is held for Python's five-second steering window: the
turn ends at once, a prompt on the same session meanwhile is steering, and a lone
cancel pauses the task when the window expires. A connection that closes during
the window applies no deferred pause; the task is left to the native host's
ordinary shutdown and recovery.

Workspace-service recovery may adopt legacy Python-owned format-12 tasks only
after acquiring the shared scheduler and Core lifetime fences. Adoption records a
separate host-transfer event, advances the task epoch, revokes old channels and
updates matching desktop bindings atomically; it never rewrites creation provenance.
Unsettled processes, legacy command/check starts and claimed answer deliveries
remain unknown and do not dispatch. Settled worker questions retain their answers
for a new assignment/session, never the revoked session. Older runtimes that ignore
the cooperative fence remain outside this guarantee.

Session history loading does not authorize task recovery or replay effects.
A new explicit owner prompt/queued message may resume a paused native task;
resumption and source admission commit atomically before engine dispatch.
Owner attachments (a draft's files at Start, a message's files, a question answer's files) are decoded and validated once by Core (at most 8 files and 4 MiB per submission, 10 MiB retained per task, simple names, strict base64), stored as private mode-0600 bodies with recorded digests, and never copied into events or request records; the native service advertises `attachments` only because it retains and delivers them. Worker guidance refuses files.
New-session startup may recover a settled coordinator using retained task data;
unknown processes, claimed deliveries and unfinished outside/control/verification
effects remain fenced. These changes do not implement the retained Python
engine jail, equivalent host-writable worker command profiles, cross-process process-identity
reconciliation or complete access/recovery parity. Do not infer those guarantees
from a mode string or from the no-paid fake-client tests.
Native context generations bind the exact attempt, wire session and owner epoch.
Only explicit adapter compaction metadata, not titles or chat, opens an observed
generation, once per completed signal. Capsules preserve recorded constraints
and answers or refuse on overflow; transport acknowledgements never establish
model understanding or measured context savings. Replacements require settlement
of the old owned process and revocation of its channel.

Coordinator reconciliation proposals cannot certify their own checks.
Synthesis/answers must disclose retained unverified checks; only matching runtime
verification can close a declared-command gap here. For an authenticated
coordinator, missing synthesis disclosures and duplicate disclosure sections
return correctable tool errors, not authority refusals. Missing-disclosure
feedback names only that task's current unresolved checks. It never matches
obsolete names loosely, records rejected synthesis, changes permissions or
weakens completion; channel, role and task-state checks still apply. A final result
must report every goal outcome of the plan; one not done is refused before anything
is recorded and again by completion (`completion_goal_not_met`), and only the owner's
own close-as-is answer lets a direct answer leave one undone, with the gap disclosed
by the runtime. `not_verified` cannot carry unfinished work (`not_verified_not_a_check`).
An approval whose `goal_coverage` reports an outcome undelivered runs no verification.
An INCOMPLETE review binds verification and completion only after the owner decided
to close its unreachable parts as is; a limit the runtime disclosed by itself no
longer binds. These gates check model-reported outcomes and the reviewer's
independent coverage; they do not prove a model's judgment. Completion still rechecks
report hashes, current project/review/verification bindings, pending input and
confirmed worker absence. Replan/scope/escalation share the retained three-request
budget since owner input; scope proposals never grant filesystem authority.
See [native access and startup](docs/project/native-incoming-acp.md#access-authority-and-startup)
and [native reconciliation](docs/project/native-incoming-acp.md#context-rollover-and-reconciliation).

The native MCP document gateway authenticates the current channel before and
after bounded reads and each publication phase. Drafts are restricted to their
creating principal; report references used as worker results must belong to that
same principal and task. Pipeline creation remains runtime-only. Pending draft
replacement receipts never replay an ambiguous file effect. Schema discovery
exposes only the effective Cortex channel catalogue with channel-bound cursors.
External MCP tools are advertised separately by the engine under their own
server names. Discovery and prompt guidance do not add tools or change grants.
Oversized authorized JSON replies are retained
whole for bounded output paging and marked incomplete until read. Native input
redaction is unavailable, so the document tools no longer advertise
`redact_values`; a call that sends it fails schema validation.
Native project writes additionally enforce current channel authority, read coverage,
whole-file hashes, no-follow parent descriptors and owner denials. Interrupted
file/command intents remain non-replayable. File intents bind both hashes and
the acknowledgement before replacement. Restart reconciliation observes the
current no-follow path: matching new bytes recover that acknowledgement;
matching old bytes settle an unchanged error, while different/aliased or legacy
unbound intents stay unknown. Recovery grants no channel read coverage.
Command process observations bind boot ID, PID and start ticks. Brokered worker
commands have independent Rust supervisors: preparation mutations and payload
launch require the inherited live authority pipe, and the exact supervisor is
durably bound before launch. Restart observers attach to private status/output
files and can request an owner stop through a launch-specific marker, never by
signalling a recovered PID. Output hashes, sizes and exit status settle together
in the existing receipt transaction; recovery supplies no project-revision binding.
Missing/mismatched supervisors, incomplete artifacts and legacy lost handles stay
unknown and non-replayable; once their process is provably gone (an earlier boot,
an empty recorded payload process group, an ended read-only PID namespace, or a
payload never released) the receipt is settled with an unknown or not-started
outcome, never a success, and is still never rerun. A lost worker's node is retried only when each of its receipts is complete or settled with an observed end (a final supervisor status, an owner stop, a launch that never started, ); a receipt settled with an unknown outcome still sends the work to its coordinator or owner. A command start of an earlier host with no recorded exit is settled only from its own complete or settled receipt (same task, channel and call reference) or from a later runtime event of another boot, and its exit is recorded as recovered with that evidence, never as a success the receipt does not show. Engine sessions left open by a service that died are
settled at the next start only when the process table shows no engine adapter,
Codex App Server or session relay of this user working in the project (an
unreadable entry counts as one); the task is then held for the owner, never
relaunched by itself. This is not detached unsandboxed descendant settlement
or engine-session recovery. Auto runs every role's command directly on the host as the owner (deletions wait
for the owner's permission card; an argv that deletes or overwrites a live store
file is refused). Brokered worker commands in Guarded and Development, read-only
roles and runtime verification run on the host at their real paths, with the
owner's tools, PATH, HOME and the shared network, under a Landlock ruleset the
per-command supervisor enters just before exec; no namespace, mount, overlay or
snapshot copy is created (see [Landlock command boundary](#landlock-command-boundary)).
A ruleset that cannot be prepared fails closed; there is no unsandboxed fallback
except the consent-backed host route.
A settled `command_run` receipt whose output shows a refused nested namespace sandbox
carries an advisory `nested_sandbox` hint naming whether the consent-backed
`outside_command` is offered to that profile; it changes no outcome, reruns nothing and
grants nothing.
Engine-jail and external-MCP containment remain separate from this command policy. Source reads
verify same-task input fingerprints and return images as real MCP content blocks;
answering-session provenance is committed with each native answer and legacy
runtime-authored provenance remains readable. Missing provenance is never inferred.
Project-wide command grants are independent of declared writer reservations.
Existing literal and expanded glob denials are overlaid after grants. Whole-project
writers can update Git data with config/hooks/pointers protected; Development adds
info/submodule guards and inherited hooks/fsmonitor disabling. Absent Git controls
are swept, not atomically excluded. Guarded HOME exposes non-sensitive tool state;
read-only HOME grants explicit caches and excludes sensitive relocation. Only
Development detaches external writable hardlinks, with bounded atomic replacement
and a durable intent/authority recheck; private/denied aliases stay untouched.
SSH config copies are trusted-root-only, and generated askpass requires owner opt-in.
These are direct masks/policy, not hostile-code isolation. See the
[native command contract](docs/project/native-incoming-acp.md#brokered-command-namespace-backend).
Ambiguous file effects and restarted running-command identities remain fenced.


Native coordinator skill proposals are consent records, not file-write authority.
The authenticated coordinator uses the retained question schema; action-matched
English/Russian labels and 2–5 distinct published project reports are required.
Questions persist before display, and decisions commit with task resumption.
Unknown answers cannot approve, and declined/unclear proposals write no project
files. Source-state inspection uses no-follow directory descriptors. Approval
only supplies a skill-management planning request; existing scope, independent
review and verification remain necessary. Native tool coverage is still partial.

The additive native [configuration read model](docs/project/native-config-read.md)
uses explicit host roots and bounded private JSON reads. It never creates/repairs
configuration, validates credential contents, invokes a keyring or launches an
engine. Login/version/cache markers are advisory, not authorization or install
qualification. Its pure settings compositor accepts only trusted, already
sanitized observations; display selectability is not dispatch authority. The
composed connection reader returns exact-byte revisions and credential kind only,
without rewriting authentication state. Advisory Codex rollout reads require the
expected private native directory and reject symlinked or ambiguous references;
Claude/context cache failures remain unknown with separate sanitized diagnostics.
The compositor itself runs no CLI and writes no quota lock, probe or cache (the
separate background limits refresh is described at the top). These checks
are not account validation or hostile same-user filesystem isolation. MCP
environment values and credential paths are not returned by that compositor.
Its MCP file reader also returns env presence only, including for invalid/disabled
entries; diagnostics never echo env values. It validates display problems without
resolving commands or opening declared read/write paths. Malformed documents retain
a safe per-surface error; unsafe storage refuses without repair. The additive routing
reader uses explicit host paths (including the host-resolved project/channel path),
never process-global settings. It observes existing catalogs without executing any
adapter or persisting connection state. Unsafe catalog directories refuse before
enumeration; malformed observations never grant model support. Structural routing
errors use bounded value-free diagnostics rather than echoing saved content. Defaults/profile management reads likewise never activate a profile, migrate a file,
validate credentials or approve a route. Cached coordinator binding is a display
observation, not dispatch authority; only an exact unique discovery-backed enabled
identity with observed parameters binds. Profile comparison excludes MCP commands,
args and environment values, and reports unavailable/interrupted state rather than
claiming synchronization when a required section cannot be captured. Invalid profile
metadata/capture diagnostics are bounded and value-free. The complete cached
catalog/selector display reader now composes native snapshot identities, exact
policy diagnostics, labels and efforts into settings. It uses stored connection
states only, never credential/keyring validation, and refuses malformed, unbound,
unsafe or ambiguous-mtime catalogs without publishing partial settings. It does
not grant dispatch authority, approve fingerprints or run refresh. The Workspace
composition additionally reads only mode/checkout from an explicit trusted channel
state root, never XDG engine state or a model-supplied path. Missing state stays
missing; unsafe roots/files or invalid JSON/object shape refuse the whole result.
It never opens a project database, repairs permissions or treats display defaults
as execution authority. These filesystem checks are not hostile same-user isolation.
The setup-composed reader additionally uses explicit host-selected bundle/data/node
roots for passive adapter/setup discovery. It inspects declared package entries,
version policy and login metadata without credential reads or any process launch.
Bundle file readability/stability and dependency-shape observations are not approved
pins, account verification or instruction-delivery qualification. Busy setup phases
use a noncreating, nonblocking probe of the existing private cooperative flock;
missing/free locks mean interrupted, not still running. Unsafe storage refuses the
whole composition. Optional OpenCode version discovery is explicitly unavailable
with a retained diagnostic; no version probe, absent-install or qualified-version
result is fabricated. No install, onboarding, refresh, ownership or replay is added.
Machine `config.read` now composes the native passive display reader on the
bounded inspection lane. Host HOME/XDG/channel roots, PATH/node and this runtime's
own bundle supply observations; socket parameters cannot select roots or refresh.
The result carries sanitized diagnostics, not fresh account validity. No Python
fallback, incidental connection rewrite or project owner is used. A read may start
the background limits refresh described at the top, but never waits for it.
Machine config.action selects Rust for connection rename, session-limit edits
and confirmed default-preset reset. Metadata edits preserve revision checks,
complete redacted replies, numeric override history and private ignore-file creation.
Reset uses the build-embedded public preset, re-reads the routing/defaults pair before
confirmation, checks that observation under the owner lock, budgets its reply before
writes and backs up existing bytes privately before either replacement. Ordinary
second-file failures roll back the first; uncertain replacement durability never
replays. A hard kill can leave the pair mixed; retained backups allow restoration.
Backup-name collisions refuse rather than overwrite a previous recovery copy.
Profile read/create/save/update/delete/deactivate also run natively. Mutations check
the exact profile revision under the shared owner lock; create/save additionally
check every section revision and capture usage policy, never MCP command/args/env.
Format-1 changes retain a private byte-identical migration backup and verify the
replacement; deletion verifies its replacement too. Failed verification restores
the original, while failed restoration is explicit and uncertain durability never
replays. Format-2 metadata changes replace only the profile file, as in Python;
qualified format-1/2 activation now journals before materializing all four sections.
MCP command/args/env remain installed definitions, never profile values. Known backing
failures restore earlier completed writes in reverse order; uncertain replacement,
failed restoration or finalization leaves an unknown/interrupted outcome, never replay. Native bounded
private-file and reply safety limits refuse rather than truncate preserved
settings. An ordered parser preserves wider integers/nonfinite/overflowing floats
only in ignored top-level profile-document metadata, without rounding or emitting
surrogate values in replies. Wider modeled fields, backing files and numeric tokens
over 4,300 bytes retain Python before admission. A new unqualified shape after
admission refuses rather than rounding, falling back or replaying. Registry and
ignore files retain no-follow, owner/single-link checks; replacement remains atomic
with an unknown-outcome fence after uncertain durability. Other actions select the
retained Python provider before any native admission or effect for custom preset
documents, launch-capable catalog/setup producers and unqualified profile activation shapes. Connection validate now
runs natively for filesystem references using canonical/collision checks and metadata
only; it never reads secret/marker bytes, approves a fingerprint or calls a keyring.
Non-secret removal atomically deletes registry/history and records the engine opt-out,
without touching native directories or keyring entries. Its default-route note is
prepared and revision-checked before commit. One shared owner lock now covers the
revision fence and registry effect, closing the retained wrapper's intervening race.
Canonical secret-file removal now uses no-follow directory descriptors and leaf
metadata, unlinking only an owned regular single-link id-derived file without
reading its bytes. Noncanonical references and leaf links/directories/hardlinks
remain untouched; unsafe parents refuse. Deletion precedes registry publication
as in Python and cannot be rolled back without reading the secret. Post-unlink
registry failure is unknown, never replayed. Qualified preset apply publishes
absent public-payload files/default profiles privately with no-replace hard links;
existing files are never overwritten and partial publication is not called atomic
or rolled back. Qualified profile activation journals pending before backing writes;
uncertain writes/finalization retain an interrupted outcome. After a known section
failure and confirmed rollback, an unconfirmed pending-journal clear is also
`machine_outcome_unknown` (Python reported only the section error). Native producers
contact no engine, network or keyring and do not change secret storage.
Valid keyring validation and unsupported removal defaults retain their existing Python
storage/error/cleanup behavior before admission. Stale-revision keyring refusals are native and never look up the backend; an unavailable provider refuses without changing credential state. Native API-key creation retains
canonical private file storage and known-failure removal/opt-out; it never moves
an OS-keyring reference or contacts a backend.
Seventy-one real dispatcher cases compare complete replies, configuration/home/state
bytes, symlink targets, modes and zero helper calls. Private bounded-file checks
still fail closed; changed credential kinds refuse after admission without replay. Explicit retained
actions can perform their established engine/keyring/network effects; native
metadata edits and passive config.read cannot. A provider error, timeout,
disconnect or unknown outcome is returned, never retried through another provider.
The read-only native catalog-refresh subset accepts only empty registries or
connections whose launch commands are null, empty or relative, or whose OpenCode
install is rejected before probing by the qualified metadata-only classifier, with a pre-existing
private shared owner lock and qualified cached projection. It acquires that lock
without creating it, checks exact registry bytes across acquisition/projection,
and never launches an engine, reads credential contents/keyrings or writes a
snapshot. Metadata-only read-only-directory classification is retained. Failed
attempts remain failures; cached routes never become new observations. Missing locks and unqualified projections retain the whole Python request before
effects, not a partially native batch. A separate qualified Codex/Claude producer performs
prompt-free initialize/session-new and Claude per-model effort selections in a
private temporary workspace, under the shared owner lock and exact-byte registry
fences. It publishes fresh immutable snapshots without replacing old ones.
Direct commands, qualified two-argument Codex/Claude installs and API-key secret-file
launches are supported. Only canonical private single-link secret files are read;
their values enter one engine-specific environment variable, not argv or replies.
Native-account marker contents are never read. Noncanonical references fail
without opening their target or starting an engine. Install bytes are
rechecked before spawn. Codex pins nested/hoisted bundled dependencies and creates
private source-guarded adapter/launcher artifacts without modifying the install.
For qualified catalog installs the launcher now selects the compiled Rust service
as a Linux shebang interpreter, with an exact JSON binary-path payload, not
Python or a shell. The parent environment allowlist is unchanged; the native
interpreter preserves the feature/MCP checks, locale coercion, timeout and final
exec contract. It reads only an owned private single-link no-follow artifact.
Safe legacy Python files migrate by verified atomic replacement; already-open
files remain intact. Failed/uncertain writes never replay through Python.
Unsupported interpreter paths retain the original provider before effects.
The separate Python engine/session launcher remains; no all-runtime cutover is claimed. Offline debug-model output is never retained; per-stream
4-MiB overflow/I/O uncertainty refuses publication/replay, while ordinary optional
debug failures keep the ACP-observed effort set. Unicode labels use retained Unicode 15 normalization
tables and the safety allowlist.

Sixty-seven Python captures (54 prior cases, eight OpenCode pre-probe failures
and five unverified Codex direct launches) compare complete replies/tree/mode
effects and exact prompt-free calls, including redacted noisy logs. The one
intentional content difference is checked explicitly: the exact oracle Python
launcher is replaced by the exact native artifact, with the same private mode.
39 additional actual launcher comparisons use allowlisted native fake peers,
including final PID/status/signals and real timeout/reaping; the native launcher
has no Python on PATH. Diagnostic
redaction precedes private log retention (400-character prefixes plus truncation
metadata, 256-KiB file cap).
Over-64-KiB lines, failed log I/O or a known launch secret echo fail closed,
never replay. This is finite parity evidence, not arbitrary transport equivalence.
Engine identities/clocks and independent EOF/reap races are explicitly bound and
normalized; failed outcomes and terminal exit codes are never assumed successful.

A Codex package candidate whose `@openai/codex` dependency (or, on Linux x86_64,
vendored binary) Python cannot resolve is launched directly like Python's
unverified launch: no private adapter/launcher artifacts, no `CODEX_PATH` or
`NO_BROWSER`, and the unchanged direct-launch environment allowlist and bounds.
It inherits Python's boundary: the adapter entry itself is not verified.

The exact-hash Codex 2.3.0 and 2.6.0 bundles are each patched natively only when their digest and
every patched call site match; the result is byte-identical to Python's adapter.
Verifier-dependent Codex candidates (including
unsafe artifact state or non-ASCII/control-bearing bundled-binary paths), potentially usable OpenCode installs,
OS-keyring launches, unqualified Claude candidates, unsafe names/roots, missing locks and unqualified
projections retain Python before effects. Claude same-name model choices without
observed effort still refuse after native observation without snapshot or replay.
Earlier batch publications can remain after later refusal. Uncertain publication
or final projection returns unknown. See the exact
[producer boundary](docs/configuration.md#native-launch-capable-catalog-producer).
Installer/onboarding requests outside the native pre-effect refusals (unsupported engine, OpenCode `install_engine`, missing opt-in, invalid revision or name) and valid OS-keyring validation still need the lazy Python
provider; this is not complete Settings migration. Missing provider/helper
infrastructure is not an observed keyring import failure: validation and
launch-capable catalog refresh refuse without changing credential state or
manufacturing engine logs/snapshots. Stale-revision refusals remain native.
OpenCode rejection admission reads only path metadata, never binary/credential contents;
it does not implement or qualify native version probing, provider import or a jail.
All successful OpenCode launch paths remain Python; no synthetic jail is treated
as isolation evidence. See [OpenCode pre-probe rejection](docs/configuration.md#opencode-pre-probe-rejection).
No new project-store or scheduler owner is introduced. Cooperative locking is not
hostile same-user isolation. See [action boundary](docs/configuration.md#native-machine-actions).
config.save uses the native all-surface writer without Python fallback. Claude and Codex
usage are refreshed by the background limits refresh described at the top.
The separate host-only `config_refresh` helper requires existing private roots,
a matching registry revision and unchanged approved fingerprints. Selected
filesystem references are checked for canonical paths/collisions before metadata
IO. It holds the Python-compatible registry lock through revalidation and atomic
replacement, preserving other owner fields. It never opens credential contents,
accesses a keyring or runs a process. Keyring selections refuse the entire batch.
The host-only config.read dispatch invokes this helper only for an
explicit selected-connection refresh, never display. It preflights display first,
re-reads after refresh, and fences a failed/superseded post-refresh projection as
machine_outcome_unknown without replay. Keyring display retains stored state plus
a value-free keyring_not_observed diagnostic; selected keyrings refuse the entire
refresh batch, never silently skip or become filesystem lookups. No native keyring
backend is supported. The machine socket uses only passive Display. Lock creation is an explicit-refresh effect;
no roots/modes/ignore files are repaired. Post-rename durability errors remain
`machine_outcome_unknown`, requiring a re-read rather than automatic replay.
This is cooperative same-user arbitration, not hostile-code isolation or account
validation, and it creates no scheduler/store owner.

The additive `cortex-core` Rust library is not yet the active orchestration
runtime. Its publication API requires transport-verified task/thread identity;
public in-process Rust methods are not authentication. Pipeline drafts have a
separate runtime-only entry point. Private report/draft files, SHA-256 checks,
request receipts and persisted publication intents coordinate SQLite commits
with filesystem replacement; unresolved conflicts fail closed. Offline restore
requires stopped access, explicit confirmation, a private verified backup and
exclusive cooperative locks, including a fence against live Rust connections.
These locks do not contain arbitrary same-user code or uncooperative legacy
writers. Inspection does not reconcile, migrate, checkpoint or create files.
The existing Python runtime and its safety boundaries remain in force until
service integration, independent review and release qualification establish a
cutover. See [Rust Core storage](docs/project/storage.md#rust-core-storage-domain).

Workspace native desktop reads use a service-owned, project/channel-scoped Rust
Core reader with SQLite read-only access to committed WAL. It does not initialize,
repair, migrate, install writer hooks or start a second scheduler. Strict immutable
inspection remains separate and refuses an uncheckpointed WAL. The service binds
registered project and same-user actor before querying; client payloads cannot
choose SQL, filesystem paths or actor authority. Native replies and small task
deltas are validated at the Rust and TypeScript bridges; generation, sequence or
entity-base conflicts require resnapshot, never guessed success. The window keeps
its last confirmed state during failed reads. Legacy mutation authorization and
result/decision evidence checks remain on the existing adapter.
A confirmed draft rename returns its committed version for the next Start;
older in-flight reads cannot replace that command version. This does not bypass
server-side version checks, retry a refused Start or authorize a started-task edit.
A started, finished or closed task's title may also be renamed: the Core rename
changes only the owner-visible `draftTitle` option, never the saved request,
settings, execution or lifecycle, and bumps the displayed task version in the same
transaction as its outbox entry, so a later resume must name the renamed version.
A cleared or deleted task refuses the rename.

Workspace draft creation without attachments now uses a reusable Rust Core writer.
The same-user service binds project and actor, validates the existing command
schema, and checks the activated store binding inside the write transaction.
Draft, saved options, immutable request receipt and public outbox entries commit
together; matching requests replay their original receipt and changed payloads
conflict. A native write failure never retries through Python. Byte-bearing
creates choose the retained adapter before any native effect. The service
acquires a cooperative project/channel owner lease before launching its retained
controller, including on recovery; a second participating service fails closed.
Standalone legacy ACP does not honor this lease, so it is not a universal
scheduler fence or hostile-code isolation. A trusted embedding can instead select
a native controller before scheduler construction. The service keeps legacy-owned
projects on their retained controller; it never adopts those tasks or opens a
competing native writer.

Native Start reaches Core through the ordinary authenticated workspace command.
Project/actor binding, draft version, owner epoch, saved-input snapshot, runtime
creation marker, display binding and request receipt are checked/committed together.
Admission and progress are owned independently of the requesting window. Replays
return the retained receipt without dispatch; payload changes conflict.
The live steering turn that handles a Send now message during execution has only
read tools and the two scoped steering tools; steering text never grants an
assignment, route, approval or write reservation, `steer_plan` is validated against
the retained plan, role/scope policy and `deny-write` rules, and targeted guidance
reaches only an exact live attempt. A live delivery is confirmed only for the
exact handoff's normal turn end and a committed nonblank owner transcript row.
On reply expiry the service revokes that channel before waiting for shutdown;
capacity is released only after process exit is observed.
Sprint draft creation and active-task model or effort changes share eight
reserved writer slots and preserve request-identity replay. Settings also use
a separate per-project lane. A full writer reserve returns `owner_queue_busy`
before enqueueing the write, rather than scheduling a late job. Engine effects
remain outside the transaction under channel/provisioning authority.
Only a host-supplied qualified route provider can authorize launch, never desktop
JSON alone. The production binary binds a read-only configured provider: it checks
approved connection fingerprints, genuine catalog identities, exact defaults/pins,
credential directory presence and executable access without launching an engine.
This is offline eligibility, not a fresh login, quota or paid-engine qualification.
The same binary serves the ancestry-authenticated MCP relay; no separate unshipped
relay executable is required. Worker routes are revalidated at dispatch, not
inferred from the coordinator picker. Native routing reads recorded owner input
and answered questions, never plan reasons or question wording as consent.
Project-root AGENTS.md/CLAUDE.md requirements require unchanged task-start file
identities; a changed file grants nothing for that task. Denied, unavailable or
exhausted explicitly requested routes refuse without substitution. Native menus
and plan hints are informational. The native Rust recognizer checks sentence-local
requests, role-purpose assignments, opaque identities and source-bound effort.
A selected option must choose one route, not merely mention its result, quota or
opinion; judgement questions cannot turn a bare model name into consent.
The automated case boundary is documented in [native incoming ACP](docs/project/native-incoming-acp.md);
this is not qualification of full legacy routing, installed clients or live accounts.
Service teardown joins its cancelled supervision and event tasks before the server
returns. The same-user endpoint lock stays held until its actual owners are gone;
requesting task cancellation alone is not evidence of released ownership.
Projects whose stores were last written by the former Python runtime are adopted automatically on Workspace activation through fenced Rust recovery: the format-11 store is upgraded with a verified private backup, a new ownership epoch is acquired, a separate host-transfer event is recorded without rewriting creation provenance, prior channels are revoked, and work with unsettled effects stays held and is never replayed. This does not add an ACP continuation route: old incoming ACP chats remain read-only history and new work starts in a new chat (the legacy controller and the `CORTEX_WORKSPACE_NATIVE=0` opt-out were removed on 2026-11-16). No socket option changes that decision. Unsupported creation-time settings and unqualified worktree routes refuse; no permission policy is widened. Desktop worktree support does not enable linked-checkout ingress through standalone ACP.
Native-owned send/steer, Stop/Pause/Resume, question answers, Retry and confirmed
owner resolution use the Rust host, not the retained mutation adapter. Each
control rechecks the activated project/actor, exact execution/session, persisted
native creation and current owner epoch. Resume and answers require current
versions; urgent Stop/Pause do not lose to stale display content. Intents precede
process effects, changed payloads conflict, and incomplete effects are not
replayed. Stop observes owned process waits before reply; continuation refuses
unsettled provisioning/capacity. Answers claim an exact wait/session/delivery
once; a bound result after that delivery can settle a reply lost to channel
revocation, but transport loss alone cannot. Confirmed owner resolution is a
non-certified closure, never a fabricated successful review or verification.
The no-paid command-path test uses actual disposable fake engine processes,
independent review and contained host verification, not production credentials.
Restart recovery acquires the same scheduler lease, revokes old channels and
continues only settled durable checkpoints. Unobserved process exits never
release capacity or certify work. The running service re-checks a session exit
it recorded as unconfirmed with the same process-table evidence as restart
recovery, only for attempts of the task's current owner epoch, with backoff; it
settles them only when no engine process works in the project, and never
signals, kills or replays anything. Unknown sessions, controls, claimed answer
deliveries and verification intents are not replayed. Paused state and retained
questions/waits survive, without granting replacement-session authority.
A final synthesis saved before a coordinator reply is lost remains a settled
checkpoint; it does not require replaying the turn or waive completion checks.
After the sixth series of identical interruptions to a running coordinator
without progress, the runtime records a durable owner question rather than
another timed retry. It reports the safe recorded reason, not engine output.
An answer authorizes continuation, not replay of uncertain effects or a waiver
of verification or process-settlement checks.
Installed desktop qualification remains a separate exact-archive gate.

Native worker guidance binds one live assignment/attempt, persists before queuing,
and cancels/settles the current ACP prompt before sending the next prompt to that
same session. Queued is not delivered; failed/abandoned delivery is unverified.
Worker stop revokes only the bound assignment before waiting for its owned
process. Independent siblings remain runnable; no-runnable-work handling leaves
a non-certified owner wait. The native pump currently runs sessions sequentially
per task; another task is not stopped. Command stop accepts only an exact
currently owned host-verifier reference, kills/reaps its process group and retains
an incomplete failed receipt. It cannot stop unobserved engine-native actions.
Check reruns take argv only from current-revision stored observations, require
matching result version/name/index and fresh project digest, use the unchanged
read-only sandbox, and never independently certify completion. Incomplete intents
are not retried. A separate stop lane keeps an owner rerun stoppable while waiting.
Native settings are task-local and next-use: exact eligible coordinator models
on the approved default connection, observed effort values and response language.
Only Guarded access is supported; Development/Auto refuse without widening policy.
Native presentation reads expose bounded worker/command/settings facts from the
same store, not a Python mutation or another scheduler.

When a worker's process exit and capacity release are confirmed, the scheduler
may claim another dependency-ready node while other workers in the former batch
remain active. The new claim repeats the current-revision ownership, connection,
scope and retry checks and cannot reuse an existing assignment. A failed final
verification in an Automatic sprint returns to bounded repair planning with each
failed command retained as a completion obligation. An omitted command is
refused, and only a later exact complete passing receipt can close it. Repair
continuation does not release unknown capacity, waive review or verification, or
convert incomplete evidence into success.

Two waits keep a worker from failing, and neither widens authority. A claimed
worker whose engine connection has no free slot waits locally
(`waiting_for_engine_capacity`); the authority check, slot check and reservation
are one store transaction, only a confirmed process exit releases a slot (an
ambiguous holder never does), and pause, stop, cancellation or shutdown ends the
wait without a launch. A typed provider rate/quota refusal that arrives before
any work began holds only that exact engine, connection and model in memory
(`waiting_for_provider_capacity`); the hold stores the limit class and timing,
never provider text, and the same route is probed once after expiry. Neither wait
substitutes a model, relaxes routing consent, owner pause, dependency order or
acceptance, or queues top-level Start: the 32-job limit still refuses with
`native_capacity_unavailable`, and authentication, billing, account-hold and
invalid-model errors remain hard refusals.

Every Hybrid or Automatic queue hold records one next actor (runtime, agent or
owner), decided by the scheduler alone. An agent repair (a repairable result or
a run that failed with every effect known) reopens the same task for its
coordinator under the machine lock, after the runtime confirms that the
previous processes and dispatched effects have settled; it is deduplicated per
reason (one repair per distinct failure fingerprint, no per-task cap). Each owner consent recorded on the hold
(`product.sprint` action `allow`, accepted only while the saved hold still names
that task, code and action) allows exactly one further repair. A repair grants no
permission, and never applies to an unknown effect or to a result whose only open
items are restricted checks. An owner hold always offers Close as is behind its
own confirmation.

Before a queue hold is saved, the service checks the task's dispatched actions
whose outcome is still unknown. While only command receipts are unsettled the
runtime acts next and the owner is offered nothing; a command stays the
runtime's to settle with no time cap, and any other unknown effect of a result
the queue would finish (one no continuation re-verified) is the owner's, and the hold
offers only actions the service accepts in that state (never an agent repair,
renewed attempts or an automatic finish). Close as is is checked without effect
(`product.task` `close` with `preflight: true`) before the run records its
resolution. Confirming Close as is on such an owner hold is an explicit owner
acknowledgement, like Retry: each unknown receipt is recorded as settled with an
unknown outcome (`settled_by: owner_close_as_is`) and an
`OWNER_ACKNOWLEDGED_UNKNOWN_EFFECTS` event, never as success, and nothing is
repeated. A worker that is still being stopped refuses the close, and while the
runtime can still settle a command the automatic-acceptance close stays refused.

A Workspace task whose retained execution records disagree has a separate,
confirmed **Close as is** action. Core checks the project owner, task version,
execution pointer and current contradiction in the transaction that records the
owner's disposition. The close does not use the disputed execution binding,
modify evidence, signal a process, release dependent tasks, or certify a
result. It does not bypass store-identity checks or make execution controls
available again. Ordinary task changes, including changing the target branch
and archiving, are refused while the records disagree.

The separately composed native session-host API acquires the same Core owner
fence before initialization. Its production channel provider verifies the stored
secret hash, expiry, owner epoch, principal, attempt, current assignment/profile,
lifecycle stage and bound engine session; MCP mutations repeat authority checks
inside their SQLite transaction. The broker supplies identity, never model JSON,
chat or `_meta`. Its private filesystem socket is addressed through a pinned
parent-directory fd under `/proc/<pid>/fd`, avoiding Unix socket path-length
limits without relocating the socket or changing token/ancestry checks. The fd
stays owned until broker shutdown; inaccessible proc-fd paths fail closed.
Only implemented role-catalogue tools are advertised (currently
role-scoped skill/reference reads, worker questions, execution/review inline results, coordinator plan intake and
informational coordinator answers). Inline publication uses the serial writer lane, a complete-payload
intent and the existing file-backed publication journal. Report ownership is
bound to the authenticated principal/attempt; the result event, node change and
receipt commit together after verifying the report file. A report left by a
failed result transaction is retained, not treated as accepted work. Native
passed-check claims and by-reference reports remain refused; worker command-tool
parity is incomplete. Review verdicts bind to fresh host-observed project bytes,
index and branch state, never a model-supplied digest. Drift requires resubmission.
A successful result revokes its worker channel without releasing capacity; the
trusted host can read its receipt after restart without replaying work or
certifying completion. Native engine permission
asks are cancelled and retained, not implicitly approved. Capacity is released
only after an owned process wait; ambiguous startup/termination is not replayed.
Pause revokes old channels, and resume cannot revive an old engine. This API is
selected only by the qualified native embedding and is not yet a complete execution host,
permission provider, universal legacy-owner fence or hostile-code sandbox.

Native instruction bodies and skill allowlists are generated from the canonical
portable sources and embedded into the Rust binary. Skill reads repeat ready-channel
identity and schema checks; a worker cannot read coordinator-only references or
another specialization's restricted design data. The Codex supplement lists only
served tools, not the entire oracle catalogue. Production route/account and native
instruction-delivery qualification remain required before desktop selection.
Trusted draft binding checks activated project/actor, persisted native creation,
project root and current owner epoch in one transaction. Its logical display
session is not a child engine session and cannot authorize a prompt. Native display
activity binds the real persisted attempt/session/current epoch, namespaces tool
ids by attempt, excludes thoughts, and explicitly marks clipped message/title previews.
From a tool call it keeps the kind, file locations, the engine's diff blocks, the
call's real input (every argument, at most 16,000 characters per text, 128 keys,
256 items and 8 levels) and its result text (at most 16,000 characters; terminal
handles and diff blocks are not result text). No call is hidden as a whole: a
Cortex tool (`read_batch`, `read_report`, `plan_submit`, the draft and result
tools) is shown like any other, including report, draft and plan bodies the call
carried, because the owner may read them in the Workspace anyway. Credentials
are the only redaction: text goes through the display redactor (bearer and basic
credentials, `NAME=value` assignments whose name contains KEY, TOKEN, SECRET,
PASSWORD, PASSWD or CREDENTIAL, private keys, and well-known token shapes such as
`sk-`, `sk-proj-`, `ghp_`, `github_pat_`, `xox[abp]-`, AWS `AKIA` and JWTs), and the
value of any argument whose name is a whole credential word (`password`, `passwd`,
`passphrase`, `secret`, `token`, `authorization`, `credential(s)`, `api_key`,
`access_key`, `private_key`, `client_secret`, `sudo_password`, in snake, kebab or
camel case) is replaced by `[REDACTED]` at every depth. Names are matched as whole
words, so `graphRevision` and token counts (`input_tokens`, `tokenCount`) are not
credentials, and hexadecimal digests and revisions (32, 40, 64 or 128 digits) are kept. Only the value
is masked. The same redactor covers a command line (each argument) and every text
page of command output the command-output reader returns, so a secret is never
stored or sent for display; a secret split across two output pages is masked again
by the Workspace, which also masks the same names and shapes when it draws a row. A file read or edit keeps only its path link and
diff; a command whose output is paged by the command-output reader keeps that
reader as the one source of its bytes. Titles are redacted the same way. Display
data never advances lifecycle authority.

The additive Rust route candidate binder is not dispatch authorization and is not
a substitute for runtime checks. It bounds JSON/catalog reads, checks private file
ownership/modes/link counts and parent directories, rejects seed/forged identities
and changed approved command fingerprints, and never reads credential contents.
Its exact coordinator/default and restricted worker-pin bindings do not enforce
project/task ceilings, quota, current account readiness or complete automatic
routing policy. Embedders must not treat them as permission to dispatch. The
Workspace new-task chooser's project filtering is not an authorization boundary,
and continuing on a saved task route still checks route eligibility against the
current routing rules. No production-account qualification is implied.

The native host driver alone claims graph nodes, schedules mandatory independent
review after observed changes, consumes verdict/fix loops and certifies completion.
Its final verifier launches argv directly with a filtered environment, a 20-minute
per-command child deadline and at most 1 MiB retained per output stream. The
launch budget accommodates a single bounded build or test step; it does not
cover an entire multi-step suite. Raw runtime receipts distinguish timeout,
cancellation, a signalled child and unavailable or truncated output. Incomplete,
signalled, nonzero or truncated output cannot pass. Landlock preparation is
mandatory and failure reports `native_verification_sandbox_unavailable`; plan
preflight preserves that reason rather than claiming the executable is missing.
Plan preflight checks for executable files in the same live read-only command
view used for verification. Availability does not say whether the check will pass.

<a id="host-effects-and-tool-search-path"></a>

**Tool search path.** The installed service is started by the systemd user manager
with a fixed minimal `PATH`, which lacks what the owner's terminal adds (nvm,
`~/.local/bin`, `~/.cargo/bin` and similar). At start, and when the owner chooses
Re-read from my shell, the service runs the owner's own login shell (from the
password database) once as an interactive login shell (`-l -i -c` for bash, zsh and
POSIX shells, `--login --interactive --command` for fish), so the same startup files
a terminal runs are executed. This is the owner's own code at the same trust as
their terminal. It runs with no terminal, in a new session, stdin closed, stderr
discarded, cwd = home, an environment of only `HOME`, `USER`, `LOGNAME`, `SHELL`,
`PATH`, `LANG`, `TERM=dumb` and the XDG base directories (no Cortex or credential
variables), and a 5-second limit, after which its whole process group is killed;
anything the startup files leave running in that group is killed as well. Only the
`PATH` value between per-run random markers is kept; banner output is ignored and
nothing else is retained or logged. If the shell cannot be read (timeout, error,
no markers, an unsupported shell), the service uses its inherited `PATH` plus the
existing well-known per-user tool directories (`~/.local/bin`, `~/.cargo/bin`,
`~/bin`, nvm's default version from `~/.nvm/alias/default`, fnm's default
version (`aliases/default/bin` in `FNM_DIR`, `~/.local/share/fnm` or `~/.fnm`), `~/.volta/bin`,
`~/.bun/bin`, `~/.deno/bin`, `~/go/bin`, pnpm's home, pyenv and asdf shims) and
records the reason, shown in Settings and Diagnostics. Service start never waits
for or fails on this read. Each entry is accepted only if it is an absolute,
existing directory without `..`, is not world-writable unless sticky, and is not
in Cortex's private state (any `.cortex`/`.cortex-dev` component, the service's
state directory, its configuration and state folders); entries are de-duplicated
by real directory in order and capped at 64 entries and 16 KiB. Owner-added extra
folders (at most 16, validated the same way and also refused inside a registered
project) follow, then those same usual per-user tool directories that exist and are
not already listed (filling gaps only: an earlier entry keeps precedence), then
`/usr/local/bin`, `/usr/bin` and `/bin`. A process that never read the shell (an
incoming ACP agent, whose `PATH` is its client's) launches commands with its
inherited `PATH`, then the usual directories, then the system ones, validated the
same way. A settled command receipt whose output shows a missing program carries an
advisory `missing_program` hint; it changes no outcome and grants nothing. Every launch
re-validates the list, and the sandbox's command path additionally drops any entry
inside the command's own project, so a project can never add a program directory
beyond its fixed `.venv/bin`, `venv/bin` and `node_modules/.bin`. Verification
and its program preflight use this same project-then-owner search order. The
owner's NVM directory and symlink targets are visible under the existing
read-only profile; the verifier does not stage a separate Node bundle.
The verifier disables networking and does not retry on the host. Ordinary worker
command routing is described under [the sudo password prompt and host rerouting](#owner-password-for-sudo-askpass-and-host-rerouting).
Pull request creation passes an explicit `--repo` derived
from the validated `origin`, drops `GH_REPO` and a conflicting `GH_HOST`, and refuses
to save or open an address for another host, owner or repository. Background outage
rechecks probe only the service's built-in HTTPS endpoints, refuse any non-public
resolved address and use a config-free, credential-free client.
An intent precedes command launch; an unsettled intent is never an automatic
rerun. Brokered worker-command supervisors can restore output/exit observations;
lost supervisors, outside-command and verification handles remain unknown (a
command receipt is settled `outcome_not_recorded` only once its process is
provably gone).
Receipt/output, revision/digest binding and pass/fail
are retained transactionally. Final completion rechecks reports, project digest,
open inputs/checks and confirmed worker exits. Unknown process capacity remains
reserved. These APIs are qualified with disposable stores/fake engines only.

Updated native executors and the production legacy ingress share a separate
`.scheduler-owner.lock` per logical project/channel. The guard is retained by
the writer lane/session references or the legacy process/runtime, including
ambiguous teardown; it is not SQLite's short access lock or the service locator
lease. Lock files must be private, same-user, regular, single-linked and opened
without following symlinks. Concurrent first-use legacy prompts construct only
one runtime. Active legacy tasks without an explicit native creation marker
refuse native startup; active marked native tasks refuse legacy startup, before
legacy reconciliation/engine construction. Terminal histories can coexist.
This is a cooperative fence for updated ingress only, not protection against
older installed runtimes, direct test adapters or arbitrary same-user writers.
No task adoption, effect-settlement inference or native desktop activation is
authorized by acquiring a lock.

Project settings reads now use the native machine lane and Core live read-only access. They return only
project routing and saved ceiling fields with existing revision tokens; invalid routing retains its byte
token and a resettable empty editor. Configuration files are bounded, regular and opened without following
the leaf link; owner files additionally require private same-user single-link metadata, with symlinked
configuration directories refused. Registry metadata is validated for compatibility, but credential
references, engine processes and catalog bodies are never opened by this reader. Missing stores are not
initialized. This path neither writes settings nor claims scheduler ownership, settles an unknown effect
or creates acceptance/check evidence; machine-wide setup still uses the retained provider. Project saves
now use native validation, the retained `connections.lock` arbitration and a Core immediate transaction
for ceilings, or an owner-only fsynced atomic replacement for routing. Ceiling changes may only narrow the
locked user ceiling and preserve project defaults; removal refuses defaults-bearing rows. Replies are
budgeted before effects. A post-replacement sync or uncertain database failure stays unknown and never
invokes Python. Cached catalog efforts in save replies are bounded display observations, never route
authorization. These cooperative locks do not isolate hostile same-user code. The native machine
user-routing transaction reuses this lock and atomic
replacement, holds admission through commit, and rechecks an observed revision change before writing. Dropping its prepared candidate saves nothing; uncertain post-rename sync remains unknown. Machine config.save now selects the native routing/defaults/MCP provider before effects; no save remains classified or admitted for Python delegation.

The native machine-defaults section also takes the shared owner lock plus
Workspace machine.lock, prepares both file candidates and its complete reply
before settings effects, and preserves the private configuration-root ignore file.
Unsafe Workspace storage refuses before saving owner defaults. The two files are
individually atomic, not transactionally atomic together: any failure after the
owner replacement stays unknown and cannot trigger replay or Python delegation.
Private directories, locks and the ignore file may be created before a refusal.
Unrepresentable preserved numbers/keys refuse instead of silently changing owner
data; invalid registry diagnostics remain native-parser diagnostics. The writer
starts no project owner and confers no account, route or credential authority.
Native socket regressions compare complete replies and exact file bytes/modes
against 224 offline Python observations, with a forbidden-helper tripwire.
Malformed AttributeError inputs preserve the observed ordinary RPC error envelope,
without process/thread crashes or diagnostic exception leakage.

The native machine-routing reply is now precomputed against candidate bytes under
the owner lock, including best-effort profile status and cached efforts. A damaged
profile never authorizes routing or prevents the save. The native MCP writer
reuses the same lock, private ignore-file helper and atomic replacement; it
preflights complete redacted replies and preserves explicit env keep values only
in the private file. It never discovers or launches the saved definition. Conflicts,
unsafe targets and budgets refuse before replacement; uncertain directory sync
stays unknown without replay. All three save surfaces run natively, including
their refusal paths; remaining config.action variants, legacy projection/evidence
paths and explicitly selected retained startup remain separate. Socket tests do not qualify an account,
an installed app or the complete zero-Python product.

Engine connection setup (`engine.connection`) selects native Rust for `login_command` install
verification/argv lookup and `recheck` status/reference registration. Recheck executes only the
host-selected native program and fixed status argv in the original HOME with the retained filtered child
environment, discarded stdin/stdout/stderr and an eight-second timeout. Timeout/cancellation kills the
direct child. Successful status precedes revision-checked create/reuse under the shared owner lock;
account directories and credential contents are never copied, opened or modified. `add_key` now uses
native create/write/validate transactions under the same shared registry lock, writing only the id-derived
private secret file (0600, private 0700 parent, no-follow/single-link/owner checks). Secret bytes never
enter registry metadata, replies, logs or argv, and no existing credential contents are read. Known
post-creation failures remove the new record/secret and record the engine opt-out; failed cleanup and
uncertain replacements return an unknown outcome without automatic rollback or replay. The steps are
individually atomic, not one transaction, and do not isolate hostile same-user races. OS-keyring-backed
validation is refused (the Python-selected backend was removed) and is not replaced with a native Secret
Service assumption. Native failures never retry through Python. Passive Rust setup observations are not
substituted for verified install resolution, native status or canonical
secret writes. Offline service coverage uses fake executable installs and disposable HOME/XDG directories; the provider fixtures reject network, real-keyring access and native-marker content reads. Only fixture-native status commands may execute. Complete replies/configuration effects and recorded fake-keyring lookup sequences are compared; native units inject private-write, validation, cleanup and unknown-commit failures. Two old provider-only post-effect exception cases retain their captured files but compare the native successful-action reply, not a fabricated native exception. No owner/account authority changed; native lookup does not execute the returned command. See [the retained setup boundary](docs/configuration.md#retained-engine-connection-setup).

Pre-save MCP discovery now executes in Rust, never through the Python migration port. It reads only the
channel's private MCP registry for explicit keep-env values, starts a process group under the existing
native Landlock/seccomp boundary (or, for a server the owner opted out with `"containment": "none"` on the
edited entry, the saved entry or the saved registry default, without that boundary and without grant
checks, as at session start), and sends only initialize, initialized and paged tools/list. No tool call,
registry write, task transition or acceptance is performed. Private/linked/missing grants and protected
descendants are refused before launch; grant trees are bounded to 50,000 entries. The 15-second protocol
deadline, 256-KiB buffered-frame limit, 32-message receive limit, 20-page limit and 1,000-tool limit fail
closed. Descriptions and configured environment values are scrubbed; stderr and raw protocol output are
never returned or logged. Success, timeout and cancelled futures kill the whole process group; a missing
containment capability refuses launch. This reuses the cooperative native external-MCP boundary, not the
Python mount-namespace implementation, and makes no hostile-code-isolation claim. Explicit retained
startup and unported configuration/setup operations still require Python; default native startup does not.

Owner `product.task` deletion actions `purge` and `clear_old` now use the native machine action lane and
shared Rust Core retention boundary. Explicit confirmation, finite numeric cutoff and finished/unarchived
preview checks precede deletion. Archive and unmerged-work checks retain their refusal order; discard
consent uses the exact legacy manifest digest, not the distinct native verification digest. No unavailable
observation establishes consent. Core holds its exclusive cooperative store-access fence across committed
deletion intent and teardown, rejects unknown starts, active/process-protected work and running worker
nodes, and emits the public removal in the deletion transaction. The running-node refusal is deliberately
stricter than the legacy Python purge. Worktrees without Cortex ownership evidence or with dirty contents
remain retained; removal is never forced. Adapter-only deletion also attempts space reclamation; its
failure is an explicit outcome of an already-committed deletion. Overlay-save failure never invokes Python
or recreates the deleted task on retry. These actions do not certify results, acquire scheduler ownership
or settle unknown effects. Legacy unmaterialized projections still delegate only before effects; explicit
retained startup and other retained operations still require Python.

Owner `product.task / accept` now shares Rust's native result-evidence validator and rechecks the current
completed execution, root/session/owner binding, unknown Start, digest, fresh-result marker, evidence
version and required literal acknowledgement. Its immediate Core writer transaction stays held through the
private overlay replacement; it does not complete the scheduler, authorize a different owner or
manufacture receipts. Pending review facts are for display only: an early check observation does not
establish final result identity or authorize acceptance. Acceptance captures the exact legacy checkout
digest. Archiving is not permission to discard, change Git, or certify work.
The Workspace asks before manually archiving an unfinished task and uses the
existing task-bound Pause control when work may still be live. An unconfirmed
stop remains visible even for a Done task and does not relax deletion fences.
A timed-out driver wait settles its admitted Pause as unknown, not as stopped.
Startup reconciles interrupted control and delivery evidence without relaunching
archived or restored-held work. Restore does not start work. An eligible
owner Start, Resume or Return for rework must pass the existing admission
checks before it can lift a restored hold. A task with no saved disposition whose bound, consistent runtime
record is the owner's force-finish (`CANCELLED` with `owner_resolved_without_certification`) is presented
as closed as is with `notVerifiedByCortex`, never as accepted or verified, and a saved disposition always
wins; `confirm_discard` on an archived task records digest-bound consent to delete its work at cleanup and
deletes nothing. Unobservable work establishes no consent, and purge independently rechecks source and
deletion fences. Both handlers preserve the existing reply/state shapes in offline Python comparisons and
refuse real overlay-write/store-exclusion failures without helper replay. Unsupported legacy
projection/evidence still delegates only before effects. Manual file-conflict choices (`resolve_block`)
now use the native machine lock, literal pathspecs and the descriptor-relative revision-checked writer;
private paths and linked file/parent writes refuse. A
durable intent precedes file/index effects; any later failure remains unknown, including overlay-save
failure, and never invokes Python. Supplied request identities replay the saved reply or unknown outcome
before legacy projection fallback; changed payloads conflict. No force, automatic restoration, result
certification or scheduler transition is performed. The native Git error envelope intentionally excludes
stderr. Merge finish/abort now also use the native machine lock and durable intent on the
materialized-checkout boundary. Finish validates unmerged entries and staged whitespace/markers before a
normal hook-enabled commit; abort invokes only Git's non-forcing merge abort. Success checks the observed
commit parents or unchanged abort HEAD and absence of MERGE_HEAD. Any failure after attempting Git or
overlay replacement stays unknown, never claims rollback, and cannot select Python or replay by key. These
are private task presentation updates, not runtime completion or new acceptance. The machine lock is
cooperative, not atomicity between Git and SQLite or protection against external same-user writers.
Unported configuration is refused natively; the explicit retained startup was removed. See
[native owner review decisions](docs/cortex-workspace/local-task-execution.md#native-owner-review-decisions).

Integration admission reads the private overlay, checks it again under the machine lock, and uses
Core live read-only state and private-index containment inspection. Unconfirmed or
stale requests, observed already-landed content and absent/non-result execution
bindings retain the Python refusal shape without a helper launch. A completed
binding alone is not proof of accepted source or successful checks. The narrow
native local squash path also validates reports/review/checks, the legacy source digest,
fresh-result and accepted-revision fences. It requires a clean, committed
project-folder source and an existing clean target worktree already ancestral to
the source, with no cleanup request. Git retains normal hooks. The machine lock
covers admission and each overlay save, and a durable intent precedes the first
effect; Git, hooks and GitHub run outside the lock, while the task's other owner
actions are refused with `operation_in_progress`. A known
failed commit restores the prior target index only if HEAD is unchanged. A real
squash content conflict is undone with `git reset --merge` and reported as a
conflict; the prior index is restored only when the target is then clean, and
Git output is never returned. Changed
HEAD, cancellation, timeout or an unsaved post-Git overlay remain uncertain, never
a reason to retry through Python. No runtime ownership or check receipt is
invented. Unsupported validation, staging, same-branch commits, target updates,
temporary targets, PRs, cleanup and legacy input shapes are refused before effects. These cooperative observations do
not freeze external same-user Git writers or provide hostile-code isolation.

Core's native project digest of a plain project folder (one in no Git repository)
is made by a no-follow walk instead of Git listings. The walk is selected when
fixed installed Git, run without inherited configuration, answers "not a git
repository" and the folder holds no `.git` entry. If the fixed executable is absent,
the walk instead requires that neither the folder nor any ancestor has a `.git` entry,
including broken links and worktree pointers. An unreadable or untrusted repository
is not observed as a plain folder. Acceptance and integration always observe fresh;
the metadata-only reuse signal is for presentation only. The walk skips the private stores,
hashes every other non-directory entry with the same descriptor-relative no-follow
reads and limits as a repository observation, refuses an unreadable directory, and
returns no partial listing. Its digest is domain-separated from every repository digest.

Unactivated machine task facts validate current legacy verification against a
fresh Rust observation for plain and Git task roots. This uses the legacy manifest
bytes, not Core's separate native digest. Descriptor-relative no-follow opens,
inode checks, nonblocking file classification and post-read/scan checks reject
observed races; file/time limits return no partial digest. The exact regular
engine lock and dependency/store exclusions are preserved, as is metadata-only
identification above the legacy content/total-byte budgets. No source cache is
used. Git file selection runs only fixed installed Git with a private HOME under a
Landlock ruleset (writes only to its scratch, excluded entries hidden), disabled
fsmonitor/hooks and no inherited Git configuration. Linked worktree metadata is
bound explicitly. An unavailable command uses the legacy whole-tree fallback;
a successful but oversized/non-UTF-8/malformed list retains the provider, never
a partial list. Private-mask enumeration limits also retain the provider.
Non-UTF-8 source names/targets and trees reaching 129 directory components
retain the provider rather than guessing source identity. Reads write no
acceptance, lifecycle or command receipt. Literal-gap rerun closures now revalidate
exact current/review/publication digests, the target identity and unique ordered
request/start/finish records in Rust. Retained output must be the exact private,
owned, single-link, regular task body, fully read with unchanged inode/metadata
and matching recorded length; missing, public, linked, truncated or failed output
never proves closure. Bodies above 8 MiB retain the provider rather than produce
a partial pass. Final-verification gap closures also validate exact unique runtime
starts and complete outputs, current reviewed source, the recorded disclosure and
its author's completion/publication. A later pending, failed or ambiguous rerun
supersedes a final closure. No derived closed_by field authorizes a pass. Acceptance
mutations still require the retained validator. These are cooperative same-user
observations, not a transactionally frozen filesystem or hostile-code isolation.

Workspace standalone machine operations now execute in-process Rust with bounded, separate
read/inspection/action lanes. Git uses fixed installed-Git argv, a narrowed environment and per-command
filter suppression for inspection; mutations retain hooks/filters and never force or auto-retry. Git owner
operations persist private fsync'd intent/receipt records containing identities, payload digests and
sanitized replies, not URLs, argv or stderr. Repeating a supplied request identity returns the retained
reply or an explicit unknown outcome; a changed payload conflicts. Legacy callers without a key receive a
new operation identity, not inferred retry safety. Native editor writes use descriptor-relative no-follow
opens, content-digest preconditions, atomic replacement and a second base check. Repository context and
saved attachment reads validate Core's live read-only store without initialization. A local Code
save requires a valid file lease and holds a Core immediate transaction across validation and
descriptor-relative replacement. The lease excludes overlapping Code saves and structured worker
file mutations in the same logical project and channel. It is not a filesystem lock: host commands,
outside writes from other projects, native engine writes, Git, external editors and terminals are not
covered. Revision checks still reject observed disk changes but cannot close the final race with
those writers. Code language tools are local desktop only; the service runs them in a sandbox with
read-only project access and rejects language-server requests to apply workspace edits. Linked-store
accounting uses Core's channel-specific logical
locator. Explicit workflow descriptions, maintenance and retention cleanup/sprint deletion now use native
Rust. Owner close/archive, automatic-ready/merged presentation updates and conflict handoff also run
natively; external closure validates session/root/epoch, launch uncertainty and live workers, then
rechecks settlement under an immediate transaction
through private overlay replacement. These updates never transition the runtime, certify checks, write
user files or perform a Git mutation; archive retains the unmerged-work question. Maintenance keeps
durable private identities, refuses effect replay and unknown installation outcomes, and fences native
desktop Start/resume/retry/check reruns while an installer is pending or unconfirmed. Downloads require
installed curl and fixed HTTPS release URLs; Git clones use explicit sanitized argv and retain partial
destinations. Core retention holds the exclusive cooperative store-access lock across committed deletion
intent, descriptor-checked draft removal, owned task-directory teardown and child-first row deletion.
Active, ambiguous or process-protected task evidence refuses deletion; space reclamation failure does not
undo committed deletion. Native saved draft edit/settings/rename serialize the execution-identity check,
option/description update, version increments and public outbox in one Core transaction. Sprint linking
holds the same immediate transaction across its quiet-task check and private JSON replacement.
Target/reopen/restore actions change private presentation only and certify no result. Unmaterialized
legacy checkout projections delegate before any effect; failed or uncertain native writes never retry
through Python. Other task state, review/integration, configuration and receipt-backed worker withdrawals
retain their existing provider; only explicit retained
startup requires the retained payload. Native traversal cancellation is cooperative: a thread blocked by a kernel filesystem call cannot be killed, retains its bounded lane permit and cannot hold the async reactor or shutdown join. This is trusted same-user development, not hostile-code isolation or completed zero-Python cutover.

Standalone task facts use validated live read-only Core access and the existing bounded native lane;
missing stores are not initialized. Result controls require matching project/session/owner bindings.
Completed-work controls validate report bytes through bounded no-follow reads and current-plan review
evidence; missing, stale or digest-less verification cannot authorize acceptance. Provisioning alone no
longer delegates a check disclosure; failed-check withdrawal claims and potentially authorized passes
still require the retained receipt interpreter. Obsolete-revision and non-runtime rerun records cannot
close current checks. Without a reviewed source digest, current reruns use the latest runtime event per
assignment/check index: pending, ambiguous, stale-owner and unbound successes remain unverified, complete
failures and timeouts remain failures, and same-owner technical blockers remain non-passes.
Runtime-verification presentation only applies attempts newer than its verification event. Source-bound
literal-gap reruns now use the same native interpretation plus exact source, command/output and
publication validation; missing-command/tool blockers carry across owners only when the exact literal
target and current source remain unchanged. Permission blockers, stale runs, passes and arbitrary
execution errors do not carry. Standalone worker history uses Core live read-only snapshots, exact
attempt/provisioning references and independent task/runtime cursor positions. It
selects display fields only, hides thoughts/read/report bodies and uses confirmed route predecessor chains rather than current defaults. Worker-result display claims never certify checks or grant acceptance. Coordinator detail history now reads only the registered task's retained session through Core, with session-bound high-watermark cursors, bounded selected display fields and confirmed route bindings; hidden updates are consumed without exposing their bodies. Neither history reader starts an engine, writes a task or claims scheduler ownership. Receipt-derived failed/passed rerun targets, receipt-backed worker withdrawals, alternate timestamp forms, unsupported coordinator display shapes and evidence beyond native bounds retain the existing provider rather than substituting weaker acceptance checks. Local containment uses a disposable index/object directory; no real index, branch, remote or task status is mutated by the read.

Native Workspace startup validates only its trusted runtime asset directory; there are no Python or helper files. Socket clients cannot select that root, an interpreter or another host. Requests that only the former one-shot Python helper could answer are refused natively without replay, fallback or service shutdown. Channel bundle discovery and voice use the trusted asset root. The package ships no Python. This is a same-user host boundary, not hostile-code isolation.

Workspace startup readiness and project reads are separate from action delivery. Socket readiness is
awaited before request bytes are sent; reconnect invalidates caches without serial child resynchronization
calls. Registry identities remain visible independently of store reads. Slow or failed reads retain
confirmed records and report project-local issues; malformed data still fails validation. Bounded
presentation waiting is not cancellation of SQLite work or proof of project readiness. Read retries never
initialize, repair or migrate stores or replay actions; same-user socket and mutation authority checks are
unchanged. Pending reads stay silent and repeated failures remain visible. A rejected first board cannot
release independent sprint facts or polls; a successful complete board is required. Native read retries
are limited to three concurrent snapshot-expiration attempts, not policy failures or mutations. The
bounded channel-profile board cache is display-only: validated task DTOs and selected sprint/owner fields
never supply capabilities, mutation versions, native stream generations or acceptance evidence. It omits
credentials, Git content and conversation bodies. Registry reconciliation discards removed/root-changed
identities. Native batched boards keep the same project/actor binding and explicit size bounds; background
enrichment is fenced by watermark and owner-command epoch. Installed dev startup checks use only
disposable container homes, a test-only delayed-start unit override and synthetic registered stores; no
owner service, package or project store is modified. These checks do not qualify the release-channel
upgrade or establish physical-GPU behavior.

Workspace action acknowledgement and projection synchronization are separate. Background reads never turn
a refusal or unknown outcome into success; no action is replayed automatically. Only acknowledged
rename/queue fields update locally, and reads begun before a newer reply are discarded and their scope is
requeued. Interrupted unscoped reads force a full project snapshot and matching facts, so consumed event
invalidations cannot silently drop another project's update. Facts notifications suppressed during a
coherent read retain their project scope for a fresh read, rather than replaying potentially stale
notification contents. Coalescing these scopes with an event refresh forces a full read so event narrowing
cannot omit them. The bundled `product.facts` helper uses the same project resolution, private-store
checks, frame limits and read lane as its component readers. Activity/operation indexes install only
through writable initialization, without a format change; inspection opens remain non-mutating. Validation
and mutation authorization are unchanged.

Workspace Delete sprint is an explicit confirmed destructive owner action, separate from archival Close sprint. It fences new member starts and continuation controls, invokes task-wide cancellation, and refuses to purge active, unknown or process-protected tasks. A bounded wait is not evidence of quiescence: incomplete deletion remains retryable. Worktree cleanup requires recorded app-creation provenance, matching Git checkout and a clean tree, and never uses force or deletes branches. Unproven or dirty worktrees are kept and their paths shown; the project folder is never removed. Bulk deletion does not run a VACUUM per task.

Workspace's saved-request editor arbitrates edits against Start in one SQLite write transaction using the original task version. Any execution identity, including a pending or unknown start, refuses an edit. Attachments use the existing validated inline-byte contract. Confirmed sprint closure saves the sprint and its member archives together. Started-but-not-Done members, contradictory completion evidence, open rework and running owner merge or pull-request effects block the write. Drafts remain drafts; closure neither cancels executions nor accepts results, runs Git, deletes worktrees or grants discard permission. Scheduled purge of a Done member still requires the ordinary auto-archive delay and retention period counted from its completion, as well as the existing purge safeguards. Age-based cleanup never purges an archived draft.

Workspace's Re-check state control rereads existing projections. Machine connectivity never confirms a task's process state; an unconfirmed task remains blocked. A bound execution reporting matching RECOVERING/recovery/recovering facts with the recorded turn_interrupted reason and no contradictory-evidence warning is an interrupted turn awaiting the owner's next message, not generic unknown recovery. Messaging remains available, but this never asserts process quiescence or enables purge as if stopped. Generic or contradictory recovery stays blocked. Re-check and the Diagnostics link do not resume, stop or replay an execution.

Workspace CPU reduction does not bypass history validation or weaken the public display boundary. Complete history pages are validated before returning; the window reuses them only for the same project, task version and execution identity. History writes/deletes still invalidate transactionally. Working status remains visible without continuous animation. See [performance measurements](docs/cortex-workspace/performance.md).

Workspace's sidebar unseen-change marks are client-local display state, not runtime facts. The app keeps them in its own local storage (`cw.seen-sidebar-changes`) as task, sprint and project ids with fixed-size 64-bit state codes, plus, for each project it has taken in, fixed-size 32-bit codes of the row ids its row bound dropped; no question wording, failure text, title or other task content is stored, and an earlier record that kept readable text is ignored. At most 1000 rows, 200 projects and 10000 dropped-row codes are kept. A record is removed when its task or sprint is gone from a project whose read confirmed what it holds. A mark grants, answers or accepts nothing. A missing record reads as a change only for a task or sprint that appeared in a project after the app took that project in; on the first run, for a project not taken in before, for a dropped row and whenever the codes did not fit, it is treated as already seen. See [Unseen changes](docs/cortex-workspace/SCREENS.md#responsive-layout).

Workspace start admission is task-local, not a project-wide writer lock or filesystem isolation boundary. Other active, waiting, recovering or unknown tasks do not refuse a new task's start. Pending, unknown or running Workspace starts likewise do not reserve the project against ordinary ACP first prompts. Each execution has its own ACP session and cancellation target; duplicate prevention, claim-before-dispatch and crash non-replay remain task-bound. Shared-checkout file/scope and owner-action safeguards are unchanged. Sprint queues check dependency order, but manual `tasks.start` has no server-side dependency check. See [the start contract](docs/cortex-workspace/protocol.md).

Workspace Git cache invalidation retains the last confirmed checkout for display; only helper-reported
repository absence enables the no-repository panel. Cached facts do not bypass service mutation checks.
Automatic Git-tab/focus refreshes are inspection-only and share in-flight repository reads. Untracked
dependency/cache files follow Git ignore rules rather than a built-in directory-name exclusion; reserved
private helper state remains excluded. Above 2,000 untracked files per checkout, wholly untracked
directories collapse to single entries that are never read for preview. Preview size and symlink
restrictions are unchanged. Push resolves the confirmed display URL against current Git remote
configuration and invokes Git with the configured name, never an arbitrary client URL; unknown or
ambiguous URLs are refused. Push remains non-forcing and is never retried automatically. Commit keeps the
helper’s existing staged-selection, non-helper-file and hook policy, without per-file UI staging.
Workspace Git inspection disables filters per command, including required filters, and never rewrites
repository configuration. Real writes retain normal hook and filter policy; task containment inspection
uses the same read-only options even with its temporary index.

Coordinator activity reads use the same project/task/session-bound display-history
projection through the public API and fixed machine helper. They do not restore
or launch an engine. Bookkeeping and thoughts stay hidden; read bodies, raw
protocol envelopes and private report contents are not added to this projection.
Route labels use recorded native confirmations (and confirmed coordinator
configuration), never machine-default selections or an unrelated retry's route.
See [recorded activity and routes](docs/cortex-workspace/local-task-execution.md#recorded-data-and-pending-questions).

The explicit `--local-release` desktop builder option creates non-official
release-channel packages for local owner testing, with a distinct
`local-owner-test` authorization and no owner-handoff eligibility. Rust compilation
and Debian packaging accept only that value or `github-main-clean` for release;
missing and unknown values fail. Local archive versions end in `.local`, rejected
by the GitHub publication version gate. These markers express build intent, not
cryptographic provenance or protection against a modified build script. It is refused in
GitHub Actions; the official `--release` clean-main workflow gate is unchanged.
The local prod wrapper does not install or publish, but its package uses production
identities and can replace production if installed: use disposable environments.
See [local Debian builds](docs/project/packaging.md#local-desktop-debian-builds). Native allocator/crash diagnostics block installed qualification even when the input driver returns zero; disappearance of the window is not proof of a clean process exit. The Linux task window does not disable compositing. Title-bar and window-manager closure pass through the unsaved-file guard and an outstanding-service-request barrier; new requests and user input are fenced while closing. Only then does the native bridge terminate this view’s web process through WebKit’s public API and wait for TerminatedByApi before destroying the window. The raw frontend destroy capability is not exposed. This avoids the affected child’s EGL/Skia exit-handler path without disabling WebKit sandboxing, stopping the independent service or filtering native diagnostics. It is not a general repair for WebKit crashes/recycling outside approved closure, a browser-storage durability guarantee, or real-GPU qualification.

Workspace maintenance uses explicit consent and a fixed pkexec/apt command in a
separate transient user unit. The release updater authenticates the exact SHA-256
sidecar bytes with a detached Ed25519 signature from the build-embedded public key.
Missing or invalid signatures or a missing key refuse the update before privilege
is requested. The updater rechecks the signature and archive digest before apt.
This does not add a root-owned staging helper or change cached-package rollback.
The signing seed stays outside the checkout in a 0600 file in a 0700 directory;
production CI uses a private temporary copy from its Actions secret.
See [key storage and rotation](docs/project/packaging.md#desktop-update-signing).
Publication is manual, limited to `main` (Latest) and `dev` (prerelease, never
Latest); Dev cannot query or install the production update feed.
The [2026-11-06 distribution-risk decision](docs/project/bundled-engine-licenses.md)
keeps native Claude login unchanged by owner choice, not Anthropic permission. API keys use the existing canonical private secret store through
helper stdin, never command history or logs; native login stays in the original
HOME and is not copied. The separate Rust `engine.connection` login-command lookup
checks qualified bundled/owned install paths and stable content before returning
executable/argv metadata. Lookup never executes login/status, reads account credentials or writes a connection.
The separate native recheck runs fixed status argv with discarded output and an
eight-second timeout, then creates/reuses the native reference with private registry
permissions and cooperative revision locking. API-key setup retains Python before
admission; native errors and uncertain writes never replay through the helper. This same-user
lookup is not account validation or a frozen authorization to execute a later command. Clone accepts credential-free GitHub URLs or an explicit
absolute local bare Git repository path outside private settings/runtime roots.
Local clones do not hard-link source objects, and no clone overwrites an existing folder. See [desktop maintenance limits](docs/cortex-workspace/maintenance-and-controls.md).

Workspace automatic naming sends only a bounded saved description to the configured
engine connection, from a disposable empty directory, with no Cortex channel or
external MCP servers and no advertised filesystem/terminal capability. The auxiliary
session requests read-only/plan mode, refuses unavailable configuration, and closes
at its 30-second deadline or output limit. This is not hostile-code isolation.
Owner renames and purge tombstones are checked under the same durable product lock
before applying a title. No task execution or owner-default update is implied.
Sprint planning runs as a read-only runtime task in the project folder, bound at Start to the sprint
planning policy, a trusted host option no command, plan or model tool can set. Its coordinator, reviewers
and read-only specialists run read-only as in every task; the runtime additionally refuses any plan with a
writing role and every project write of the task. Like any read-only role it can read what the project and
the sandbox allow, so this is the ordinary read-only channel boundary, not hostile-code containment. The
plan reaches the sprint only through the channel-verified `sprint_plan_submit` tool: the runtime assembles
its ordered parts (at most 50 items and 128 KiB per call) and validates the whole plan (keys,
dependencies, cycles, at most 512 KiB, no item count limit), sends it to a mandatory independent review
against the studied evidence, and accepts only the newest reviewed plan of a completed task; prose is
never parsed into tasks. Failure notices use service-owned explanations, not raw engine errors, prompts or
credentials. Only explicit materialization saves user-task drafts; planning never starts their executions.
Materialization is one service operation under the machine lock: every draft, its sprint membership and
dependencies, and the sprint's created state commit in one project-store transaction that also replaces
the saved sprint record, or none of them does; task identities derive from the sprint, plan version and
plan key, so a repeat never creates a
second set, and tasks an interrupted earlier client-side creation left are reported, never adopted. The owner selects Project folder or Separate worktree for the sprint; materialization copies that checkout kind to every task's saved create options, used by the existing start route. Manual and Hybrid can use separate worktrees; Automatic refuses them. A saved worktree Start without Git or committed HEAD is refused, never downgraded to the project folder. The refusal returns fixed recovery guidance, not Git output or local paths. Project-folder tasks still share the owner’s files. [Worktree execution boundaries](docs/cortex-workspace/security.md#separate-worktree-boundary) describe the stricter linked-checkout policy and its limits. Existing saved sprints and running checkouts are not moved.

Cross-task inspection is a coordinator-only read boundary. `sprint_graph` and `sprint_task_result` are bound to the authenticated task's materialized sprint. They expose bounded relationships, durable outcomes and immutable published reports, without private prompts, attachments, drafts or raw runtime logs. Code availability is evidence-based: matching write receipts and exact bytes can produce `confirmed`; absent or ambiguous evidence stays `unknown`. These reads grant no mutation, acceptance, merge or scheduler authority.

Sprint links and priority are changed only by the local Workspace owner. The `product.sprint` actions `live_graph`, `set_dependency` and `move_priority` are desktop-only: paired remote devices are refused all three, the generic sprint patch accepts no dependency, revision or priority field, and no model tool or ACP wire field reaches them. An edit names the graph revision it was made from, is validated as a whole graph under the machine lock, changes only a saved not-started draft's incoming links or the priority sequence, and never writes the approved plan, history or the materialization marker. A queued start rechecks its choice under the lock, so a stale scheduler pick cannot start a task after an edit. The model-facing `sprint_graph` contract and read scope are unchanged.

A sprint revision is the one model route that changes sprint drafts, and it is proposal-only. The coordinator-only, channel-verified `sprint_revision_submit` tool is accepted only for the authenticated task's own sprint, only while an owner-originated impact check is open for that task, and only from a coordinator turn with no worker running. Under the machine lock the service accepts changes of saved drafts with no execution identity at their expected versions, against the current graph revision, validated as a whole graph with the same ID-only validator as owner edits; it never changes a started, finished, archived or purged task, the approved plan, its history, `materializedVersion`, Pause or the roster beyond appended new drafts, and deletes nothing. Every revision has a mandatory independent review before it can take effect; the runtime applies an approved one only in an Automatic sprint (all draft changes, new drafts, the product record and the runtime event commit together or not at all), and in Hybrid or Manual only the local owner's `apply_revision` / `reject_revision` decides, refused to paired devices. A proposed cancellation is never executed. The `sprint_graph` `include: "pending_descriptions"` read returns full draft descriptions of the same sprint only, in bounded pages.

The service may restore the exact durable, settled COMPLETED session for owner actions. It does not adopt a database owner for a live task or alter generic ACP ownership checks. Retained technical blockers remain unpassed evidence, not fabricated passes; acknowledging them still requires explicit owner consent and current result binding. A real failed executable check is not an acknowledgement waiver.
Review follow-ups retain implementation content only while the observed source digest is unchanged; new
checks and runtime evidence still govern acceptance. Missing-command/tool blockers survive session-owner
renewal only for the exact unchanged check and source; permissions, stale attempts and passing evidence
are not carried forward by this rule. Git integration enumerates literal user paths, never
helper-directory pathspecs, and restores the previous index selection on failed staging/commit without
discarding user file contents. Held question-batch drafts rebind only across display-version changes with
identical question, option, owner, requirement and permission identities; all members still undergo the
normal server version checks. Multi-item sprint descriptions use the planner's per-item scope and relevant
plain-text owner answers, without service-appended sibling work or serialized conversation records.
Unanswered explicit owner decisions belong to the relevant item, never the implementing model. A sole item
additionally retains original requirements and plain-text follow-ups. Model-generated decomposition is not
filesystem access enforcement.
New managed checkout names use a bounded ASCII description slug and a stable suffix,
not a transport id. Names do not grant authority: Git common-directory and branch
validation still bind the saved checkout. Existing paths and branches are not moved
or renamed, including when the default worktree root changes. A worktree home may use a `.cortex` name outside projects; canonical paths inside Git checkouts or registered non-Git project folders are refused. Saving the root changes only the machine setting, not existing checkouts. Editor actions name registered checkout identities; the native bridge resolves their folders and invokes fixed launcher names, including per-user launcher folders, without shell evaluation. Diagnostics expose availability and recovery advice, not environment values or credentials.
Result cards and whole opened reports render Markdown as text nodes and known elements, never supplied HTML. Relative paths, HTTP(S) and mailto links do not navigate the desktop webview; executable and unknown URL schemes remain text. Link creation is not permission to access a file, execute a command or change report-read authorization. The local desktop confirms video and subtitle path opens separately before handing an existing regular file to a fixed system opener, with no shell. For a relative link on a worktree task, the popup offers a handoff only after resolving against the recorded task checkout, never by substituting the main project folder. The opener still checks the path independently and refuses executable files and mismatched canonical file types. Both the requested and resolved paths must be outside `.git` directories and marked Cortex stores. A `.cortex` or `.cortex-dev` name alone is not a store marker: the check looks for `cortex.sqlite3`, or for Dev ownership (`.cortex-dev/owner.json` or the legacy `.cortex-dev-owner.json` beside the Dev store). An unreadable marker check is refused. See [Result links](docs/cortex-workspace/local-task-execution.md#result-links).
Workspace review and worker overviews exclude withdrawn checks only when the runtime can rederive their
withdrawal from retained receipts and provisioning; presentation fields alone do not
suppress failures. Ready results that are not yet accepted retain explicit Accept controls in Conversation and diff review; those controls record acceptance only and never commit, merge or close the task. A result accepted by an Automatic sprint's queue shows no Accept control; that automatic acceptance uses the same validator, applies only while the sprint's current run mode is Automatic and likewise never commits, merges or closes the task (see [local task execution](docs/cortex-workspace/local-task-execution.md)). Accept sends the reviewed source version and digest, not its display sequence;
the service still revalidates both before recording acceptance. Accept and check reruns take that identity
from the result the screen displays (the delivered machine facts), using the raw wire result only when no
result fact is supplied; this changes no server authority, and server acceptance still revalidates current
evidence. Result versions follow runtime evidence events, not unrelated progress/transcript invalidations.
The UI respects service review blockers and source-drift warnings; a failed or unknown acceptance retains
a task-local reason and refresh action. Unverified reviewer claims with recorded commands receive real
automatic check attempts, never invented passes; commandless legacy gaps are separate manual items, not
acceptance blockers. Availability probes use the same read-only launch preparation and final argument
compaction as verification, never execute proposed programs, and distinguish sandbox/launch failure from
an observed missing executable. Policy/consent refusals are not technical acknowledgement authority.
External completion is an explicit owner-confirmed machine record, not runtime acceptance or integration.
The service refuses a live or unobservable execution; it never fabricates checks or performs Git
operations to close it. Content containment uses a disposable index/object directory and reverse-patch
checking against the local target, not commit ancestry. Unknown or oversized comparisons never assert
containment. Target push status is separate, last-fetched information and never close authority; no fetch
or push runs implicitly.
Accepting a result in a non-Git folder never initializes Git,
commits or pushes. Explicit Git initialization runs only `git init` in the registered
project, refuses an existing repository, and does not configure identity or remotes.

Paseo history replay checks the original assistant message identity even when
the host derives a different timeline item ID; ordering, text and manifest
checks remain required. Reasoning uses the exact host-derived ID tuple. See
[history recovery](integrations/paseo/HISTORY.md).

Fresh-install model settings use the owner's refreshed portable default-profile
routing policy, the portable `claude-agent-acp/opus` coordinator at medium effort,
and Russian message language. The bundled Sonnet ID is `sonnet`, including for
its seven-day quota window. Removing legacy deny entries does not enable those
models: exact enabled and automatic membership, provider policy, quotas and
other eligibility checks still apply. OpenCode remains explicitly denied.
Preset preferences do not grant model access or prove live availability. Existing owner
settings are never replaced. Account/connection
identities and machine-specific MCP paths/credentials are not cloned. Fresh installs
register disabled MCP examples with `/path/to/...` placeholders and empty environments,
not installed executables; the loader excludes disabled entries before launch. A named
applied `default` profile is created only for matching preset settings in an absent
profile registry. Existing profiles and MCP registries are never replaced; see
[the install snapshot boundary](docs/configuration.md#fresh-install-owner-default-snapshot).

Routing effort controls read cached per-model observations only; they do not start engines, grant model access or change authorization. Unknown/conflicting observations are labeled, and unsupported stored values are preserved rather than silently repaired. Runtime dispatch validation remains authoritative. See [settings](docs/configuration.md#manage-settings-in-paseo).

## Paseo dependency findings (2026-10-04)

The plugin lockfile retains GHSA-vfj7-8cjw-p6xm in `braces@3.0.3`,
propagated through React Native's Metro/Jest tooling. Its 15 high-severity npm
package entries describe one advisory, not 15 proven exploitable defects.
No patched braces release was published at this inspection. Installation and
regression passes do not clear a nonzero audit, and `devDependencies` is not
a security boundary. See the [dependency disposition](docs/project/paseo-dependency-audit.md)
for exact paths, artifact-test limits and the next upstream-fix decision.

## Current ACP boundary (1.0.6)

The additive `cortex-acp` Rust stdio binary uses the service's native host, not a
second orchestration implementation. Core acquires the project/channel scheduler
and owner fences before writable initialization; a busy owner refuses with
`scheduler_owner_busy`. An incoming agent process hands that lease over only
while it has no unfinished task and has been idle, so several agents can share a
project one after another; it never runs beside another holder. It does not
attach to an existing daemon. A project containing Python-era tasks opens through the same
fenced recovery/adoption as the Workspace service (unsettled effects are not replayed),
and a format-11 store is upgraded by the verified backup-first migration; continuing an
old task from incoming ACP is not implemented. Legacy format-12 session/load instead uses a query-only Core reader:
no native scheduler, recovery, permission grant or task adoption is triggered, and
`session/prompt` on such a session is refused with `legacy_session_read_only`. Storage
refusals return a human-readable message with a stable `reason`. A non-initializing
open that refuses format 11, or an open that refuses an unknown format, does not
migrate the store or create a migration backup. If schema validation rejects a
format-11 upgrade, the transaction rolls back without rewriting the main database,
but the private backup created before the transaction can remain. See
[format migration](docs/project/format-migration.md#opening-python-era-projects-through-incoming-acp).
The Rust entry is the default Paseo/Zed launch. Host setup (Zed registration, the
Paseo setup launch record) or a Paseo source-root launch selects the fixed
checkout path `target/debug/cortex-acp`, never model input; an explicit
`--rust-binary` or `CORTEX_ACP_RUST_BINARY` names another binary; the Python
entry and `CORTEX_ACP_ENTRY` were removed. The Git-install bootstrap
records the staged Rust entry. Whoever can write that checkout's build directory can
replace the launched binary: this is a same-user boundary.
Zed writes retain explicit settings-path/consent/replacement checks; Paseo retains
its private profile-local launch record and load shim. An unbuilt default and
invalid explicit binaries refuse and do not fall back to Python. The public runtime-root override is trusted host configuration,
not a permission grant. Rust ordinary startup refuses pinned-contract/Python-floor
mismatches before configuration/protocol output, observing only the local Python
version. First-run seeding covers absent public routing/default/MCP files and an
absent default profile. Approved-adapter following only advances existing
connections to a newer, exactly registered Cortex-owned or verified bundled adapter. It never
downgrades or executes an adapter. Under `connections.lock` it refuses a changed
registry, retains a private backup of the exact prior bytes, then replaces
atomically. Verified packaged bundles are followed through the same install checks. The Rust entry also runs the background engine installation, automatic registration and catalogue observation pass, except that a launch shape its verifier does not admit is not observed (`native_catalog_unavailable`). A pre-task
language choice is saved as the user default under the same lock, as in Python.
A pre-task model change validates the new route with empty dependent parameters,
so a saved effort the new model lacks is dropped, not refused or carried over.
The provider picker reads only an account identity claim for its label, like
Python: the `email` claim of the unverified `tokens.id_token` in a Codex
connection's own `auth.json` (owner-only regular single-link file, 4 MiB cap),
or `claude auth status` found through absolute `PATH` entries only (unlike
Python, never a relative or current-directory entry), run with only those
entries and the connection's own `CLAUDE_CONFIG_DIR`, closed stdin and one five-second
deadline for execution, stdout collection (1 MiB cap) and cleanup. A dedicated
process group is killed on completion or failure, including when a descendant
keeps stdout open after its parent exits. Pipe reads are cancellable; cleanup
never blocks waiting for EOF or process exit beyond that deadline. Tokio reaps
any direct child not yet waitable at expiry. Every failure omits the account;
no token, key or command output
is logged or returned. Offered routes stay limited to qualified, launchable
native-account connections. After `session/new` each registered connection gets
one prompt-free catalogue observation through the shared native producer; launch
shapes it does not admit are skipped. Unlike Python's detached refresh process,
an unfinished observation ends with the Rust process. Python-created sessions
reload with options computed from a read-only store view; the store is not
written.
Task commands use only existing native controls and Python's wording.
`/cortex-stop` pauses the bound task and revokes its channels; `/cortex-finish`
closes it uncertified through the confirmed owner close control; `/cortex-kill`
is the same owner command-stop control as `cortex_stop_command`, accepted even
while a turn runs. `/cortex-clear` only reads: it applies Python's selection and
soft protections (this connection's sessions, their tasks and live work) to a
preview, and a confirmed request that would delete anything refuses without a
write. `/cortex-new-task` checks the route first, then binds the new task to a
separate internal session row; the invoking session is never rebound. The internal
session persists the invoking client's access mode and language before task
admission, so a Guarded selection cannot silently become Development authority.
Prompt cancellation is handled by the follower of the actual executing session:
for a separate-task turn it pauses only the separate task, not the invoking
session's existing work. Idle cancellation does not stop a task; explicit stop
and finish commands still control the bound task. A refused
ordinary prompt records only its text row, never a task, source or attachment.
While a turn runs, a typed message can answer only a question already open for
that session's own task, through the same Core answer transaction and epoch/version
checks as `cortex_question_answer`; it never grants a permission. A coordinator
question is answerable this way only while its card is open and the launch
environment set Paseo's `CORTEX_PASEO_QUESTION_UI=1`. A batch member alone is refused;
batches still need one complete submission. Answering retires only that
question's card, never an engine permission request.
Efforts mode exits before ordinary setup, without launching engines; OS-keyring
connections refuse that native mode. For a Rust target (a launch argv of exactly
one absolute path named `cortex-acp`) the plugin's store readers run
`cortex-paseo-tools` from the same directory only: never a `PATH` lookup or an
environment-selected binary, with an empty environment, working directory `/` and
the Python readers' own argv. It opens `<project>/.cortex/cortex.sqlite3` only through
cortex-core's non-initializing live read-only mode (query-only, no migration, hooks or
writes; a format-11 store refuses with `legacy_storage_migration_required`) and applies
the same display scrubber and bounds as the Python readers. Paseo Settings requests
go to `cortex-paseo-tools config` from that same directory, with working directory `/`
and only `HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME`, `CORTEX_ROUTING`,
`CORTEX_WORKSPACE_CHANNEL`, `PATH`, `CORTEX_ACP_RUNTIME_ROOT` and
`CORTEX_OPENCODE_BINARY` from the launch environment. It answers reads through
workspace-service's offline configuration readers (no lock, write, repair, process,
keyring or credential read; MCP env values are returned as names only, as in Python).
Every change (defaults, routing, profiles, MCP servers, connections, project ceiling,
default preset) takes the same owner-configuration lock as Python, checks the caller's
revision under it and replaces files atomically with owner-only modes; a write whose
outcome is uncertain reports `machine_outcome_unknown` and is never replayed. Requests
never name a path, binary or command: catalogue refresh and onboarding start only a
verified adapter install the native verifier admits, never a prompt; engine download
installs only Cortex's manifest pin through npm; connection removal never reads a
secret. Non-finite input and integers outside the native exact range refuse rather
than being rounded or converted to removal markers. Validating an OS-keyring
connection, probing an installed or named OpenCode binary (it needs the OpenCode
namespace jail) and refreshing an engine without a qualified native catalogue producer
refuse with `config_api_unavailable` before any effect. Plugin activation runs
`cortex-paseo-tools setup` (cortex-acp's own first-run pass) with the full launch
environment. A missing binary refuses those reads and settings with
an explicit message and never falls back to Python. This is a
same-user trust boundary, not isolation from a planted binary in that directory.
The default switch followed a live Paseo `cli` qualification on Claude and Codex
native accounts; live Zed, rendered Paseo Desktop and installed-package
qualification of this entry were not run.
See [launch limits](docs/project/native-incoming-acp.md#default-launch).
A trusted incoming-client permission transport is optional: native Desktop still
cancels permission asks by default. Rust ingress forwards exact one-shot engine
options to its owner client, then Core rechecks channel/session/epoch authority;
unknown options, persistent grants, cancelled/late replies and transport loss
never become approval. EOF and host shutdown settle owned processes before
dropping the owner. NDJSON input/output and public history reads are bounded;
private engine thoughts and raw tool arguments/results are not public history.
Initial and follow-up image/resource prompts retain 0600 bodies in the private task source-block
directory with SHA-256, availability/reason and user_direct metadata; a follow-up's
attachments become the next source revision in the same transaction as its
steering event and transcript row, and its capture fingerprint is part of the
control request identity before replay, admission and settlement. An identical retry
returns only a settled receipt; changed capture bytes or metadata conflict, and an
unfinished control remains unknown without repeating effects. Fingerprints are
computed from trusted captures, never accepted from client request fields. Local links
are opened once without following the final symlink; the resolved real path
must also stay clear of `.git`, `.cortex`, `.cortex-dev` and the dev state files
(a symlinked directory inside the project pointing at one is refused as `private_path`);
fd-derived containment,
regular-file, owner and hardlink checks precede bounded reads. Network links are
not fetched. Only the launch environment can add the Paseo uploads root. Both
per-block and aggregate attachment budgets are 10 MiB, and all retained attachments
of one task stay within 10 MiB because each engine prompt carries them all; an
uncaptured image or an over-budget follow-up refuses before any write. Files are fsynced before metadata commits; a rolled-back
admission may leave unreferenced private files, never accepted/replayed input.
Outgoing prompts verify stored body hashes and carry native image/resource blocks.
Native engine-side ACP frames are bounded at 16 MiB to accommodate base64
expansion. Incoming client lines keep Python's 15 MiB bound; an oversized,
malformed or unterminated line is answered with a JSON-RPC error that never
echoes its content, and the connection continues.
Worker batches require all answers in one Core transaction; a single permission
choice or partial submission cannot resume the worker.
Only Guarded access and eligible configured native routes are offered. This is
not full legacy permission, skill-proposal or client-integration parity.
No Cortex session runs in an engine mode that skips the engine's permission
requests, in any access mode: every Claude session (coordinator, every worker,
reviewer and auxiliary session) runs in claude-agent-acp's `default` mode, a
read-only channel (the coordinator, a read-only profile or a review-stage
session) runs Codex in codex-acp's `read-only` mode, and a writing Codex worker
runs in the Cortex worker permission profile (else `workspace-write`; see [Native engine tools](#native-engine-tools)). A read-only
session whose adapter offers no such mode does not start
(`restricted_mode_unavailable`). claude-agent-acp asks for every tool Claude Code
does not run on its own, so the runtime answers its permission requests itself,
identifying tools only by the engine-declared name, kind, locations and command
text. Cortex gateway and attached external MCP tools pass for every channel,
because the gateway or relay re-checks the channel and operation. Native subagent
and native question tools, and every non-Cortex tool of the consultant, are refused
with an offered reject option (cancelled when none is offered). The runtime
answers codex-acp's requests (its `read-only`, worker-profile and
`workspace-write` sandbox escalations) the same way. Every other native request is decided by the access
mode's [decision table](#native-engine-tools): Cortex private state is allowed on
request (Auto allows, Guarded and Development ask the owner) and a write to a live
store file is refused in every mode; Auto allows everything else and records it; Development allows reads
outside the project and asks the owner for writes outside the project, credential
and key material, Git control and engine/Cortex configuration writes and
deletions; Guarded also asks before reads outside the project and native web
fetches. Read-only channels keep the read/search/fetch-only native fence in every mode; a web fetch
only reads and follows the mode's fetch rule (Guarded asks the owner).
An owner card for a declared deletion (`delete`/`move` kinds or a declared `rm`)
keeps its `Removal blocked pending confirmation: ...` title, other cards are titled
`Owner decision: ...`; only an offered one-shot option is accepted, and without a
connected owner client the request is refused. No engine process is
sandboxed as a whole, so Guarded never approves a native shell command without
a client. Nor does a jail mount the
owner's Codex `config.toml` read-only: the native Codex launcher instead starts
App Server with a non-persistent `-c` layer trusting its own working directory,
the project root codex-acp already marks trusted for every session thread, so
Codex does not record project trust in that file (its ephemeral title thread
once did). Every Codex session the Rust entry or desktop service starts
(coordinator, worker, review, verification, sprint planning), not only the
catalogue refresh, pins that launcher as `CODEX_PATH` with `NO_BROWSER=1`, as
Python does, so its App Server also gets the launcher's feature and native MCP
switches. Such a session's MCP relays reach App Server through the launcher
(as `-c mcp_servers.<name>` layers it verifies with `codex mcp list --json`), not as
ACP `mcpServers`: codex-acp would send those as one `mcp_servers` table that
replaces the layer turning the owner's own Codex MCP servers off, which then
started in every session. A relay named like an owner-configured server refuses
the launch. The hand-over variable is removed before App Server starts, so no
model-run command inherits it. codex-acp's own session thread config carries a
`features` table that replaces the launcher's `-c features.*` layers for that
thread (the owner's plugins and their MCP servers, such as a computer-use REPL,
then started in every session), so every launcher-backed launch also gives
codex-acp `CODEX_CONFIG` with the same features (subagents, plugins, hooks,
apps, native shell and image tools) set to `false`, which codex-acp spreads into
each thread's table; the launcher refuses a relay-carrying session whose
`CODEX_CONFIG` is missing or different. The catalogue observation does not set
it yet, so its brief observation session still starts the owner's plugin
servers. The launcher is written only when the engine starts; an
install-shaped codex-acp launch whose bundled Codex cannot be located then
refuses to start instead of running without it. Other engine state files the Python jail keeps read-only are not
protected this way. Codex and OpenCode ask only to
leave their own sandbox or configured rules; their requests keep the earlier
route: forwarded to the client outside Auto, or cancelled without one. Every
request and decision is journaled with its tool name, kind and decision rule.
External MCP sign-in. A server waiting for an OAuth sign-in raises one machine-wide owner card (`projectId` `machine`, kind `mcp_signin`) found by a no-model probe: the service starts the enabled entry with its configured containment but no channel and no tool grant, sends one `initialize` and reads stderr. The sign-in address (a plain `https` URL of at most 2048 bytes taken from the server's own stderr) lives only in service memory: `owner_actions.list` carries only `signIn.available`, the window's policy refuses `owner_actions.act open`, and only the desktop client asks the service for it (`native.open_mcp_signin`, which names the card id alone), checks it again as a plain `https` address and hands it to the fixed system opener with no shell; phones never receive it. The card cannot be dismissed (it closes when the server answers or is switched off in Settings), and nothing is logged, stored or reported from the probe's output. Tokens stay in the server's own directory and are never read.

Native external MCP attachment reads the owner-global registry plus the owner's
private per-project registry (`cortex/projects/<digest>/mcp-servers.json` under the
owner's Cortex configuration, never inside the project), never a project-local
executable configuration. Project entries pass the same validation, tool grants,
containment and relay checks as global ones; a malformed or non-private project
file refuses instead of falling back. Read-only worker grants are narrowed to
explicit read-only tools, writers keep their explicit tool set, and consultants
receive no server. Incoming configured names cannot override owner grants;
unconfigured stdio entries refuse. Entries no engine is handed (no stdio command,
such as an HTTP or SSE server, a reserved name or a repeated name) are accepted
and ignored; their URLs, headers and environment are not stored or forwarded.
Engines receive secret-free relay entries while the
host owns the contained process and private per-launch runtime directory.
Mandatory Landlock confines a contained server: it sees the real filesystem
(the owner's tools on the owner's search path), cannot read the owner's keys and
credential stores, the project's secret files or the live Cortex stores, and
writes only to its write grants, worker scopes and private runtime directory. The
network is shared (the `network` flag is accepted but not enforced); no namespace
or hostile-code isolation is claimed.
Writing workers' existing, non-root scopes are granted to the server; a scope
the worker has not created yet is not granted (never widened to a parent
directory, never created). Administrator paths must exist and must not overlap
the project. Administrator paths are checked without walking them: a granted tree
may hold symlinks and ordinary files (a Node install, a server's own `auth`
directory), while a path that is, contains or sits inside Cortex private state is
refused. A root, symlinked, protected or outside-project scope, an
overlapping or protected administrator path, unavailable containment, a failed
launch or relay, or a server that exits before the session is created withholds
only that server: the session starts without it, records
`NATIVE_EXTERNAL_MCP_WITHHELD` with the server name and a reason code, and shows
a timeline note. The server never runs with less containment than its registry
entry names. Core authenticates and
reserves calls before dispatch, retains complete private replies before return,
and never replays an unknown effect. Configuration values stay in host memory,
not task reports.

Uncontained external servers are an explicit owner opt-out. The owner sets
`"containment": "none"` on one entry or as the registry-wide default in the
owner-only `mcp-servers.json` (an entry's own `"contained"` overrides that
default; absent everywhere means contained, and any other value refuses the
registry). Such a server runs with the owner's full user authority: no Landlock
and no `no_new_privs`. Its `read_paths`,
`write_paths`, `network`, `runtime_dir_env` and worker scopes are not enforced or
applied, and the per-file project walk that builds the Landlock policy is
skipped. What still holds: it launches only the registry's own absolute (or
`/usr/bin`/`/bin`) executable, or a bare name found on the owner's tool search
path, with the registry environment plus the owner's
`HOME`, a built `PATH` (the executable's directory, the owner's tool search path,
`~/.local/bin`, `~/bin` and the system directories) and the Cortex process's
`XDG_RUNTIME_DIR`; channel secrets
are never passed; engines reach it only through the channel-bound relay, whose
explicit tool grants, role/consultant narrowing, reservation and no-replay rules
are unchanged. It runs in its own process group, which Cortex kills when the
session ends. A server that detaches (for example a daemon that calls
`setsid()`) leaves that group and is outside Cortex's kill guarantee. Pre-save
tool discovery follows the same choice. Opting out trusts that executable as
much as any program the owner runs directly. See [native incoming ACP](docs/project/native-incoming-acp.md)
for remaining compatibility limits.

Workspace local dictation uses a fixed packaged sherpa-onnx worker and default
PulseAudio capture, with closed native/service controls and a short read lease.
Audio is not uploaded or persisted. English recognition output is lowercased in the
local worker; only explicitly sent text enters the ordinary prompt path.
The native owner can select only RU or EN, and cannot change languages
during an active session. Each on-demand model download has a pinned size and content hashes;
no webview-supplied URL, path or executable is accepted. See [dictation authority](docs/cortex-workspace/security.md#local-dictation-built-local-linux).

Workspace attachment input carries bytes, not arbitrary client filesystem paths, and
only once: the `product.attachment` upload. The service validates base64, names, MIME
syntax, eight-file count and a 4 MiB combined decoded-byte cap, stores each body as a
private file `<state>/attachments/<sha256>` (directory 0700, file 0600, written
atomically, never through a symlink) and answers references `{name, mimeType, size,
sha256}`. Every other command (task create and edit, answers, prompts, queue, comments,
sprint notes) carries references and refuses inline bytes (`inline_attachment_refused`);
the desktop bridge refuses them too. One body is read back only by digest (64 lowercase
hex characters, no path from the client), at most 4 MiB and verified against its
digest. Unreferenced bodies are removed under the machine lock (unsent uploads after 24
hours). Queued messages keep references in owner-private service state; so do recorded
answers, whose file references come from Core's exact record and are released with
the task's deletion or purge;
task creation retains bytes in private SQLite options until Start; prompt, answer and direct-worker resources use the existing private source-block store.
Sprint creation and planning notes retain references in atomically replaced private product state; their bodies are read when the planning task is created and become that task's own draft attachments, never base64 inside prompt text. A sprint's cumulative planning resources share the eight-file/4-MiB budget. Product-state writes refuse before exceeding the bounded response budget; local frames are capped at 8 MiB. This is a bounded transport, not an unbounded upload service.
Only the sprint driver can set the draft's `planningSprintId`. The service commits it with the draft before publishing the task. The read-only board field does not grant execution authority, substitute for the sprint's planning-task link, or select the runtime policy at Start.
Sprint planning diagnostics are written by the service to private product state; owner-facing sprint patches cannot set them. The Workspace displays their text from live data and does not copy it into the offline board cache.
The sprint service checks recovery against the linked task's execution before accepting an amendment
or original-request replacement and starting fresh; these are not client-side start overrides.
When the service detaches a planner during recovery, it records the cancelled presentation (unless
already closed) and the read-only retirement flag in the same private product write as the changed
link. The flag only suppresses ordinary Board entries and notifications; it does not grant authority,
change the runtime execution state, or delete the task.
Question answers and their resource references commit together. The Linux bridge
reads clipboard image bytes (PNG, or another image GTK can decode, re-encoded as PNG)
only on an owner paste with no plain text, and reads no clipboard text or file path.
URI-only native drops use files observed by GTK, not paths supplied by JavaScript,
through an expiring one-use byte buffer. Sent conversation messages keep references to their
files in the same private product state. `native.save_attachment` accepts only a name and
bounded bytes from the webview and writes them to the path the owner chooses in the
system's save dialog; the webview never names a path. Files are never
executed or fetched by their resource URI; unavailable capture refuses delivery.
See [Workspace attachments](docs/cortex-workspace/local-task-execution.md#attachments).

Workspace task pause/resume is an authenticated owner control, not a model tool.
The hold, recorded resume state, request identity and public invalidations commit
transactionally. Paused tasks reject model mutations; Resume checks the displayed
version and current owner epoch. Cancellation of an engine turn is separate from
this durable hold. Pending worker question/access waits survive it, but old worker channels are fenced. A
recorded replacement receives the saved decision only after Resume; it must revalidate access, and unknown
action outcomes still block replacement. Resume is runtime continuation, never an invented user message or
answer. Worker history combines task and command/guidance namespaces under one attempt-bound continuation
and byte budget, with explicit clipping/refusal rather than silently omitted rows. Client page retention
is keyed to worker and attempt identity, not changing display facts; automatic reads of all available
worker-history pages do not bypass server cursor validation or page budgets. Failed continuation reads
keep loaded activity and offer Retry. Live review-action reads use validation-only shared Store access,
including normal SQLite WAL/SHM housekeeping, not immutable offline inspection. They neither initialize
nor migrate storage, and Accept still revalidates retained evidence and the current workspace. Public
worker report reads require a retained result reference
in the selected project, verify the published digest and use identity-bound cursors
with at most 4,000 characters per page. The webview receives neither private paths
nor arbitrary runtime files; report text is displayed without executing HTML.

Workspace check reruns resolve only retained commands on the bound task checkout;
clients cannot supply replacement argv or cwd. The native bridge accepts only the displayed check's bounded result identity and version before the runtime revalidates them. They use the same read-only runtime
verification boundary and retained-output checks as final verification. Durable
request reservations prevent silent execution replay. Automatic omitted-check
execution does not authorize paid probes or override recorded policy/consent
blockers. A technical waiver requires a runtime-recorded blocker and explicit
owner acknowledgement; it never changes failed or incomplete evidence into a pass.

Paseo Routing model choices exclude models disabled in the file being edited.
Stored pins and policy references remain intact and carry an inactive warning;
this presentation does not enable models or change runtime authorization.

Workspace Codex weekly usage displays only retained, unexpired windows explicitly reported as 10080 minutes; primary/secondary slot names alone do not establish duration. This display classification neither starts a model nor changes routing quota identities or authorization.

Workspace Debian ordering stamps are packaging metadata, not integrity or authorization evidence. The runtime content identity remains unchanged and embedded; build ordering does not bypass install verification, release gates or owner consent. See [build ordering](docs/cortex-workspace/platforms.md#debian-build-ordering-and-reproducibility).

Workspace Debian packages prefer their channel-local, root-owned engine tree and bundled Node; they do not require PATH Node or npm. The build uses the committed npm lock and Node archive checksums. Launch-time canonical-path, install verification and exact instruction-delivery registration remain mandatory. The Codex patched adapter and wrapper are written only into private connection state, never beside the read-only package. Same-version connections can relocate to the verified bundle without changing native account references. OpenCode stays optional. Public redistribution of the Anthropic SDK/native executables remains blocked pending terms review or permission; see the [licence inventory](docs/project/bundled-engine-licenses.md).

Startup and approved engine updates may advance existing Codex/Claude launch
commands to newer source-qualified, exactly registered owned installations.
This is an explicit runtime policy, not project-supplied command approval.
Only the launch command and its derived fingerprint change; account references,
endpoints, providers, routes and model settings remain intact. Selection remains
subject to launch-time integrity/private-adapter checks, never accepts an
unregistered release and never downgrades automatically. Registry writes are
locked, revision-checked and atomic with a private 0600 previous-config backup.
Each agent start also creates a missing `routing.json` or `defaults.json` from the bundled preset and never replaces an existing file; it registers a connection only for an engine that has none. There is no per-connection pin field; `CORTEX_ENGINE_FOLLOW=off` opts out for
rollback, separately from disabling downloads. See
[configuration and rollback](docs/configuration.md#approved-adapter-following-and-rollback).

Model-route consent requires an actual request in recorded user messages or
answers. Descriptive model remarks and coordinator `model_reason` text never
authorize a route. Flattened imported history is not trusted turn provenance;
restate requests outside it. Unterminated literal history tags cannot erase a
user refusal. See [request matching](docs/features/routing-and-fallback/index.md#owner-model-requests-in-plain-words).
The one other consent source is a model requirement in the project root's own
`AGENTS.md` or `CLAUDE.md` (regular, non-symlink files), which counts as the
owner's request for the role purpose it names. It is recorded as
`decided_by: project_instruction` with the file path, a newer user message
outranks it, a prohibition never grants, and hard owner deny, provider
authorization, unavailability and exhausted quota still refuse without
substitution. A file counts only when its inode change time and modification
time both precede the task's creation, so an edit made during the task,
including one by the task's own writing workers, grants nothing for that task;
the dispatch decision and plan preview then show `project_instruction` with
`status: changed_during_task` and the file path, which is never consent.
An owner edit made during a task applies from the next task. Anyone who could
edit those files before a task started, including an earlier task's workers
whose uncommitted edits remain, can still select an enabled model for the named
work, within owner policy, so review changes to them. A question about a
requirement is not one. See
[project instruction requirements](docs/features/routing-and-fallback/index.md#project-instruction-model-requirements).
The same requirement applies to runtime-created mandatory reviews without a plan
hint: dispatch resolves the required model, or refuses without substitution,
before reserving engine capacity or creating an engine manager. A reviewer does
not need to start on another model to discover the conflict. Chosen options that
request a recheck through a named model also count as user consent, including
commit-list/history objects that name no role purpose. They do not assign that
model to unrelated review roles; unselected options, past-work descriptions,
negations and ambiguous multi-model choices still grant nothing.

Non-initializing Store inspection (the default `initialize=False` mode) does not
create directories, locks, schema, rows or SQLite sidecars. Immutable inspection
is fenced by the exclusive access lock and refuses nonempty WAL data rather than
reading a stale snapshot. Live runtime callers, including configuration API project
reads, explicitly use `runtime_access=True` with `initialize=False`: they share the access lock and read committed WAL data,
without schema repair or migration, but permit normal SQLite sidecar/checkpoint
housekeeping. This runtime mode is not non-mutating inspection. Normal initializing
opens still install additive schema and perform the verified format-11 upgrade.
Local Code lease acquisition uses the shared access lock once the lease schema is
installed. Store bootstrap and schema installation still use the exclusive initialization
lock. The editor authority, checkout binding, and conflicting-writer checks still apply.
See [storage](docs/project/storage.md).

Declared writer reservations and denial comparisons use canonical lexical paths,
including older stored aliases; this does not resolve symlinks or create OS
isolation. Removed failed verification commands remain completion obligations
across repairs. Worker-local answers only reset replanning; they cannot approve
those removals. Consultant channels are not offered replan, scope or escalation
requests, and dispatch refuses them without changing task state. See
[orchestration](docs/features/orchestration/index.md) and
[consultant boundaries](docs/features/senior-consultant/index.md).

Portable MCP error translation exposes a bounded identifier code, category and
fixed corrective guidance, not exception-owned remediation, details or arbitrary
field values. Plan preflight and route-menu reads can establish rejection before
plan writes; an unlocated portable failure remains uncertain even when its category
is validation. Raw storage faults retain uncertainty and disclose only the class
and standard errno reason, never exception-owned reason text or filenames.
Accepted-plan reconciliation uses the same channel, key and complete payload;
it cannot bypass authority or create duplicate nodes. This is tested with
disposable stores, not evidence of a historical transaction's outcome.
See [MCP tool errors](docs/features/failures-and-cancellation/index.md#mcp-tool-errors).

Native tool failures are reported by their stable code without changing what is
enforced. After channel authentication a handler failure is a structured tool
error (`code`, the offending `field` when known, a fixed `hint`); a channel that
is no longer current, a tool not offered to the role or stage, a read-only role,
an owner-protected path and a missing owner confirmation stay JSON-RPC `-32001`
refusals that name the same code. Hints are fixed strings: no reply echoes
another task's data, a path outside the project or a credential. `project_mkdir`,
`project_delete` and `project_skill_link` hold exactly `project_write`'s
authority and `command_run`'s deletion consent; no new write authority exists.
`read_batch` grants nothing beyond its members: it accepts only the read-only
tools its catalogue lists (project file and directory reads, Git status/diff,
skills, reports, drafts, sources, retained output, capsule, discovery, command
status, and for the coordinator task context and route options) that the channel
is itself offered, runs each through the handler a direct call
uses, and rechecks the channel's exact identity between members, withholding
every body when authority is lost. The runtime-owned `pipeline.md` is published
under a runtime principal no channel holds; models cannot create or edit it.

Recognized `gateway_read_required`, `gateway_complete_read_required` and
`gateway_stale_read` errors reject before file mutation; they are read
preconditions, not authorization decisions. Guidance directs a current same-channel
read, required non-truncated coverage and a rebuilt valid edit under existing
authority, without an owner question or extra approval. It grants no access and
never cures a channel, role, scope, private-path or policy refusal. Hash,
exact-match and complete-read guards are unchanged. Tests establish the scripted
gateway/wire workflow, not real-model compliance; new guidance does not hot-upgrade
running sessions. See [read-precondition recovery](docs/features/permissions-and-containment/index.md#read-precondition-recovery).

Channel-scoped `tools_discover` exposes only the effective offered Cortex
catalogue, never external or other-role schemas. `read_batch` preflights every
member against an explicit read-only allowlist and its current schema.
It has no evaluator, command launches, questions, submissions, requests,
publishing or mutations.
Fresh task/channel/assignment authority and catalogue checks surround execution
and delivery; revocation or changed offers withhold all accumulated bodies.
Reads can still write runtime bookkeeping and owner-private retained outputs;
they are not filesystem isolation or zero-write transactions. Batches are
sequential, not snapshots, with cooperative authority-based cancellation, not
preemptible stdio execution. See [bounds and evidence semantics](docs/features/tool-discovery-and-read-batch/index.md).

Explicit worker effort comes only from the same retained user source that
authorizes the exact planned model, or an owner effort pin; model-authored reasons
and the coordinator's editor setting do not authorize it. Policy floors/ceilings,
observed support and native configuration confirmation still apply. Conflicts
refuse before a worker prompt, including after auth recovery or a live option
change; no policy file is rewritten to make a request pass. Dispatch intent is
not native confirmation. See [effort preservation](docs/features/routing-and-fallback/index.md#explicit-effort-preservation).

Coordinator capability guidance is instructional, not deterministic model
obedience. Its separate configured-worker MCP summary contains bounded, sanitized
server/tool identifiers grouped by engine filter and read-only/writing access
class, never launch arguments, filesystem grant paths, credentials or schemas.
It neither attaches worker-only tools to the coordinator nor grants access.
Actual proxy/channel checks and runtime-owned dispatch remain authoritative;
a refusal must not be retried through another tool, engine or worker. Configuration
and forwarding are not evidence of discovery, invocation or success.
A separate built-in discovery roster is derived from role tool offers, not
external MCP configuration. Worker commands can inspect authorized outside data;
`project_read` remains project-only and assignment-local `access_request` reserves
project work paths, not external grants; it is served to writing workers only, so
a read-only or review channel is not offered a route it could never be granted. Unknown source locations do not authorize
unbounded host searches. Host-history discovery must establish a supported
read-only route and exact session identity, without resumption, creation,
credential access or production mutation; unavailable or incomplete evidence stays
explicit. Read-only/Guarded commands mount private scratch at `/var/tmp`, so an in-command
missing-file error cannot establish host absence. The existing `outside_command`
consent route supports authorized bounded host-only reads, but only writing
workers receive it; read-only roles must report the capability gap. No mounts,
authorization checks or coordinator execution rights are widened. These are
instructions, not an additional enforcement boundary. See
[capability-based delegation](docs/features/roles-and-delegation/index.md#capability-based-delegation).

Native auth-loss recovery replaces only an owned engine process on its existing
account/model, after the normal observed-exit and replay-safety gates. Durable
one-shot fences survive runtime recovery; a repeated auth failure blocks for
manual `claude /login` or `codex login`, never automatic provider fallback or
model escalation. No token values are read/copied and no login/refresh/keepalive
command is run by the runtime itself. This can reload native account state, not
guarantee token renewal.
See [authentication recovery](docs/features/failures-and-cancellation/index.md#native-authentication-loss).

Separately, the former opt-in login keepalive tool (a Python script, removed on 2026-11-16; it was never invoked
by the runtime and its optional `systemd --user` templates were installed only by explicit owner action) ran, per engine, at most one
zero-cost refresh-forcing command a prior investigation proved does not make
a paid call; an engine without such a proven command is reported
`unsupported`, never guessed. It never reads, copies or prints a
credential/token value and never logs or persists a command's stdout/stderr,
only its bare exit code, as a sanitized `ok`/`needs-login`/`unsupported`/
`error` status. As of this writing both the installed Claude Code CLI and
the installed Codex CLI report `unsupported`, because no zero-cost refresh
command is proven for either (installed-CLI `--help` text describes
status/login display only, never a proven refresh or persistence). See
[login keepalive (removed)](docs/features/failures-and-cancellation/index.md#native-cli-login-keepalive-removed).

Localized final-answer disclosure descriptions replace raw check names only in
display. Original names remain unchanged in retained structured evidence and
gap matching; a translated description cannot satisfy a missing exact-name
disclosure. Coordinator-authored prose is not translated or language-validated.
Runtime-authored consent copy can use the effective response language; operation,
option identity and required authority do not depend on the translated text.
See [final disclosures](docs/features/task-lifecycle/index.md).

Paseo's history boundary distinguishes command output from native reads:
shell calls retain their output in the shell row, including split
execute/input/output updates, while native `read` and Cortex `project_read`/
`read_skill` bodies are metadata-only in display history. Edit/Write aliases map
native field names for the displayed edit row only; they do not perform a file
operation or add authority. This is presentation behavior, not model
enforcement. No-paid checks do not establish live model behavior or rendered
Desktop evidence; those claims require the separately authorized
surface-specific check.

Owner feed exposure of tool input and output. Workspace's Conversation, worker
pages and coordinator activity let the owner open any tool call except file
reads and edits into its recorded Input and Output, and Paseo's Workers tab
shows the retained input and output of its worker rows. Both display only what
the runtime already recorded; nothing new is retained, no permission, routing
or lifecycle rule changes, and private report, draft and result bodies stay
withheld. The runtime's own plumbing calls remain hidden in Workspace unless the
owner turns on Show every tool call; this changes visibility only. Recorded input
and output can contain secrets, file contents and personal data, so a long value
shows a preview with explicit Show all and Copy, and Copy places the text on the
owner's clipboard by the owner's own action. Output clipped or absent is labelled
Pending, Not recorded, Unavailable or clipped, never shown as complete.

Retained output of brokered commands (`command_run` and the approved
`outside_command` route) may hold sensitive stdout and stderr. The Workspace
reads it through the local owner-only machine operation `command.output`, keyed
by the exact project, Workspace task, attempt and call identity. A bare output ID
grants nothing; another project, task, attempt or a replacement channel is
refused; the file must belong to that task and channel and match its recorded
name, size and SHA-256, otherwise the state is `unavailable` and no bytes are
returned. Reads are paged (4,096-byte preview, pages of at most 16,384 bytes) and
history rows carry only a reference, never output. The desktop bridge allows the
operation; the phone policy and remote allowlist refuse it, and it is not a model
tool, so it grants no execution or file-read authority. An engine's own shell
call, host verification and `project_git` rows have no link. These are
local-presentation guarantees only; no-paid checks do not establish rendered
desktop or live Paseo behavior.

Always-included shared skills are required instruction components, not optional
references. A load failure or empty portable body blocks composition with
`required_skill_unavailable`, naming the skill, receiving role and underlying
cause. Worker provisioning records the existing failed step/error and reports
an `assignment_failed` diagnostic before opening an engine session; situational
catalog entries are still loaded only on demand. This validates instruction
availability, not model compliance or an additional permission boundary. See
[instruction delivery](docs/features/roles-and-delegation/index.md#instruction-delivery).

Frontend-design automatic routing is affinity-bound, including retries and review
fixes. Same-engine fallback, and cross-engine fallback when the affinity engine
has no eligible route at all, grants no authorization and does not bypass deny,
providers, task ceilings, tier/effort policy, unavailable routes or exhausted
quota. While the affinity engine has one eligible route, no other engine is
ever chosen. A cross-engine pin is refused, not silently substituted. Confirmed
explicit user model requests still take precedence under existing hard checks.
See [design routing](docs/features/routing-and-fallback/index.md#affinity-bound-frontend-design).

Requirements intake and design workflow are coordinator-only situational guidance,
not additional lifecycle or permission APIs. Bundled design skills are readable
only by their relevant worker roles; read-only designers/auditors return reports
while assigned writers persist design documents and capture images. Skill text,
reference tables and example sites never authorize tools, installs, network
actions or copied assets. Missing browser/evidence stays unverified; screenshot
artifacts use git-ignored project paths outside private storage. Bundled design
skills adapt Apache-2.0 and MIT material; their license and NOTICE files ship in their
directories, they contain no executable scripts, and the Taste Skill text is not bundled
because its license could not be established. See
[requirements intake](docs/features/requirements-intake/index.md) and
[UI/UX design workflow](docs/features/design-workflow/index.md).


Ordinary read-only worker commands use `read_only` in every mode. Automatic
verification uses the same host-visible profile with the host network (shared with every command view).
The profile restricts writes and uses a private PID namespace and private
`/var/tmp`. Host `/tmp`, allowed tool caches and the verifier's existing
writable ignored-directory grants are not private outputs. Neither route is
hostile-code isolation. The deletion classifier
is lexical: it recognises `rm` and the other common deletion commands, but not deletions
hidden inside scripts, interpreters or build tools.

A brokered MCP relay never receives its handshake token in argv or in its environment. The
broker writes the token to `relay.token`, a 0600 file beside its socket in a private 0700
directory, and the relay reads it there without following symlinks. The token is compared in
constant time and is bound to the connecting peer (same uid, and a descendant of the launched
engine). An unbound broker's token is single-use. The file is removed with the broker directory.
PID isolation applies only to the read-only profile, not to writing workers.

A natural-language request such as "do not modify X" creates no write denial. Only an
explicit `deny-write: <path>` in an owner message does, and it is matched after path normalization. A denial stored by
an earlier runtime with wildcard characters still guards writes (matched as a glob).
The denial is recorded in the same transaction as the message (`lifecycle::steering::parse_denials`,
stored in the steering event's `deny_write` field) and applies at once to every write not yet made,
whether the message waits for the coordinator's next boundary or restarts it: project file tools
refuse the path at each write (`write_denied`), a writing command already running is stopped
(`authority_changed`), plans, scope and access requests touching it are refused. A path outside the
project or a private path (`.git`, `.cortex`, ...) refuses the whole message (`gateway_path_denied`)
and nothing is recorded. Only the owner lifts a denial: an explicit `allow-write: <path>` in a message (parsed and confined exactly like `deny-write:`), or Remove on the task's Write denials list (from the desktop or from a paired phone, in the same exact request shape) (`allow_write` control, refused as `write_denial_unknown` for a path not denied). A lift removes only that exact recorded denial; it never widens a grant, the jail, a private path, a read-only role or another task, and natural language lifts nothing. A message that both denies and allows a path refuses (`write_rule_conflict`). An owner message to a
working task no longer cancels running workers, review or verification; stopping work stays the
explicit Stop, worker Stop and command stop.

Project agent and process-rule files (`AGENTS.md`, `CLAUDE.md`, `CLAUDE.local.md`, `GEMINI.md`,
`.agents/`, `.claude/` and `docs/project/verification.md`, `lifecycle::steering::RULE_FILES`) carry
no default write protection (owner decision 2026-11-23): a task edits them with the runtime's
approval like any other project file, and the owner is not asked. Only an explicit owner
`deny-write: <path>` protects one; `allow-write: <path>` (in a message or in an answer) or Remove
lifts that denial. The capsule's `rule_file_protection` list is empty.

At task start the service records an environment preflight (`ENVIRONMENT_PREFLIGHT`, carried in the
coordinator capsule as `environment`). It runs `gh auth status`, `codex login status`, `claude auth
status`, `sudo -n true` and `docker info` from the owner's tool search path with stdin, stdout and
stderr closed and prompts disabled, so only exit codes are recorded and no account, token or prompt
text is read; `sudo -n` never asks for a password and runs only `true`. `git --no-optional-locks
status` output is read (bounded to 64 KiB) for branch, upstream and change counts only, never file
names. Free space and display variables (short names such as `:0`) complete the record. Every probe
shares a 3-second budget and a probe that runs out of time is killed with its process group. An
engine CLI (`codex`, `claude`) runs only where that engine's native login files already exist, as
the passive settings marker checks them (`~/.codex/auth.json`, `~/.claude/.credentials.json` in a
private, owner-owned, non-symlinked directory); otherwise the fact is `not_logged_in` (or `unknown`
for an unusable native home) without a launch, because those CLIs create their native home on first
run and a probe must never create or alter one. The facts inform planning only; they grant nothing
and never stop a task.

Owner-triggered retention removes session access-default rows before deleted host
sessions, without changing surviving defaults or explicit access choices. The
foreign-key boundary and committed-deletion recovery remain in force; this grants
no model-callable deletion authority. Local no-paid binding tests require plugin
`npm ci` and type-check before Python execution; absent endpoint output is a
failure, never qualification evidence. See [verification sequencing](docs/project/verification.md#sequencing).

Terminal task cleanup, unexpected prompt exceptions and definitive host disconnects
stop only exactly task-owned opens and engines. Their coordinator/worker channels
are revoked before shutdown; a surviving process does not retain gateway authority.
Partial opens are included, and revoked coordinators cannot issue new worker channels.
Per-task admission/cleanup serialization and turn-generation fencing keep old EOF
or cleanup from revoking a replacement's channels or removing its host requesters.
Teardown does not alter verified
COMPLETED/FAILED outcomes or replay uncertain actions. Worker locks and capacity
remain quarantined until exit is observed, including active members of the
launched Linux process group after its leader exits. Unknown identity is never
signalled; no process-name matching or cleanup of user Docker resources is used.
Partial-launch locks survive unconfirmed shutdown. Orphan zombies are not active
writers; their OS parent owns reaping. Descendants that escape the owned group
are not covered by that group observation. The incoming ACP process still serves
its host connection until EOF/writer-close/SIGTERM, not just prompt end. See
[cleanup boundaries](docs/features/failures-and-cancellation/index.md#terminal-resource-cleanup).

Runtime-composed synthesis is a runtime-only completion path, not a model-facing
acceptance API. It requires completed nodes, first-pass independent APPROVE and
complete exact final-verification receipts on the current digest, no open
evidence gaps or pending decisions/input, and no repair or historical replan.
Existing report-integrity, project-drift and worker-session absence gates still
apply. Changed paths and escaped, attributed execution-summary quotations are the
only variable text sources; quoted prose is not promoted to verified semantic fact.
Backticks, control/format characters and Unicode line/paragraph separators in
filenames retain coordinator reconciliation rather than entering raw inline code.
Summary whitespace is flattened and Markdown punctuation escaped, including HTML
syntax. Missing/blank or overlong summaries and non-whitespace control/format
characters also retain coordinator reconciliation; outcomes are not truncated.
This is a text-composition boundary, not a qualification of every client renderer.
Composition and completion commit atomically. Registry termination bookkeeping reuses that
same SQLite writer; worker liveness and cancellation are rechecked before commit.
Before composition, the controller may await already-owned post-result cleanup
for at most ten seconds outside the SQLite transaction; it neither terminates
workers nor restores authority. Cancellation interrupts the wait; unknown
liveness or timeout cannot authorize completion. Every existing gate is retried
before commit. A missing transaction-safe engine gate retains coordinator reconciliation.
Recovery retries unfinished reconciliation under the same gates, and reuses
already committed synthesis without another completion event. No permission,
storage-format or host API expansion is introduced. See
[runtime-composed lifecycle](docs/features/task-lifecycle/index.md#runtime-composed-synthesis).

Reviewer command reuse is limited to the current review attempt, byte-identical
plan verification argv and the same project digest. The runtime verifies its own
execution events and complete retained output, issues an attempt-scoped output
alias, and labels the receipt and result evidence as runtime-produced, not as a
reviewer execution. Shutdown and revoked authority interrupt the bounded wait;
implementation workers execute normally. This does not credit speculative checks
as final verification or relax independent review. See
[reviewer verification reuse](docs/features/orchestration/index.md#reviewer-verification-reuse).

Post-result token settlement waits at most 15 seconds on an owned cleanup
thread for the exact prompt consumer to record usage; a turn that ends sooner
settles at once. It does not restore the
completed worker's channel authority, release capacity or the writer lock before
confirmed process exit, or postpone explicit cancellation. Cleanup is bound to
the original manager, not a replacement. Shutdown hands off to that exact owned
settlement once, with a bounded join; uncertain exit retains quarantine rather
than causing a second teardown in the same shutdown. Declared-command evidence
gaps require identical argv at the task-root cwd, in the read-only profile, the task mode's own writer
profile, or the Guarded writer profile (`restricted`); a different-cwd receipt cannot close them, and
a `development`-profile receipt cannot close one once the task is Guarded. Unknown consumption remains unknown;
see [usage settlement](docs/features/token-and-plan-usage/index.md#coverage-and-interrupted-turns).
The read-only synthesis aggregate keeps unfinished reconciliation explicit and
withholds its share; it never turns a pending outcome into a completed fallback
or acceptance evidence. This reporting change grants no completion authority.

Reviewer handoff adds only runtime-observed metadata, not new authority:
verified executor command receipts (not asserted to match the review digest),
planned verification argv explicitly labelled as not results, and changed-path
hashes/sizes against the execution baseline. The block excludes worker prose,
file contents and command output; missing or changed observations are diagnostic,
not an empty successful comparison. Reviewers publish inline through the existing
immutable-report path. Their independence, worker report access and mandatory
digest-bound verification remain unchanged. See
[reviewer handoff](docs/features/orchestration/index.md#reviewer-handoff).

Codex ACP **2.6.0** and Claude ACP **0.107.0** have separate exact-source
instruction-delivery registrations, alongside the retained older releases
(Codex 2.3.0, Claude 0.97.0 and earlier). Cortex still negotiates ACP v1. The
2.6.0 and 2.3.0 private adapters each check their complete
distribution SHA-256 and exact patch sites; it translates existing role metadata
to upstream append-style instructions without changing the installed package.
No-paid initialization and fake-App-Server traces are not paid/native-tool
enforcement or GUI evidence. Neither production nor dev connection files are
rewritten by these registrations. See
[Codex](docs/features/codex-engine/index.md#current-support-260-2026-11-16) and
[Claude](docs/features/claude-engine/index.md#current-support-01070-2026-11-16).

Claude ACP 0.88.0 is separately source-qualified for system instruction delivery;
0.78.0 and 0.87.0 remain independently registered. A Desktop check on 0.88.0 observed its resumed scenario complete without an instruction-delivery refusal; it does not establish native-tool enforcement (see [Claude adapter support](docs/features/claude-engine/index.md#previous-support-0880-2026-10-08)). Unknown installed Claude
versions cannot borrow that qualification or displace registered installs in
discovery; pruning preserves registered installs. Critical-role dispatch excludes
an unsupported or non-retryably unqualified engine/version across models and
connections, without weakening route pins or owner policy. Its existing
native-tool restrictions are retained through inspected SDK option forwarding,
not claimed as real-model enforcement evidence. Compaction detection uses the
engine's explicit `claudeCode.toolName=compact` metadata rather than display text.
No-prompt model catalog initialization does not prove execution or authorize a
model that is absent from the catalog. The bundled SDK 0.3.293 (also retained as an exact side-by-side 0.87.0
refresh) exposes `haiku` = Haiku 5.5 and accepts high/xhigh/max through ACP
controls. Its instruction and native-tool restriction forwarding was
independently inspected; this is not paid inference/enforcement evidence.
Old Haiku is denied by exact ID in the preset and deliberately adopted owner
settings; the original SDK 0.3.287 install is not overwritten. See
[adapter evidence](docs/features/claude-engine/index.md#previous-support-0880-2026-10-08).
The prompt-free catalog refresh a new ACP session starts runs in a child process
with its own session, so it can outlive the ACP process that requested it (Paseo
stops discovery processes about a second after `session/new`). It is still bounded
by the per-call refresh timeout, sends no prompt, runs one refresh per connection
at a time and only adds a catalog snapshot. See
[catalog refresh lifetime](docs/features/claude-engine/index.md#catalog-refresh-lifetime-and-deployed-runtimes).

Codex ACP 2.1.1 uses the same exact-source-guarded private patches as 2.0.1,
with a separate evidenced registration. Unknown versions remain unsupported;
installing a newer package does not bypass those checks. Role metadata is sent
for every qualified Codex version. Installed CLI 0.159.3 passed no-paid
wrapper/jail tests; native shell/view-image remain disabled and native
`apply_patch` remains jail-bounded. No paid/client enforcement or production
Paseo/plugin update is claimed. See [Codex evidence](docs/features/codex-engine/index.md#previous-support-211-2026-10-07).

Cortex is an incoming ACP stdio agent. It launches Codex and Claude ACP engines,
and the optional OpenCode v2 engine once the owner onboards it, as separate processes and stores task metadata and published Markdown under the
single project-local `.cortex` directory.

Owner input may reference uploaded files as plain-text absolute paths or file
URIs. Capture can read only the launching `PASEO_HOME/uploads` in addition to
the existing project attachment surface; it does not grant workers direct
outside-project filesystem access. Missing launch-home evidence grants nothing.
Production and dev upload homes are never both enabled. Realpath validation and
fixed-root opened-descriptor checks reject symlink escapes; a read-only,
non-following/non-blocking descriptor must be an owned single-hardlink regular
file within the existing 10 MiB file/prompt limits. No Paseo files are modified.
Only accepted owner prompt/steering text is scanned, never model output, reports
or embedded attachment contents. Rejections retain explicit source diagnostics;
accepted snapshots inherit the existing `user_direct` label and task-scoped
`list_sources`/`read_source` paging. See
[the upload boundary](docs/features/input-history-and-replay/index.md#paseo-uploaded-files).

The current working tree is undergoing
a runtime-owned orchestration rewrite and is **not release qualified**. See
[verification status](docs/project/verification.md).

The model proposes plans, writes reports and submits results. The runtime validates
plan structure, reserves dependency-ready assignments, selects configured routes,
runs independent review when required by the observed project state, runs
verification and checks completion invariants. After all planned nodes complete,
the runtime may omit review only when its observed project state equals the
execution baseline or a state independently approved by a reviewer. It records
that decision as `REVIEW_NOT_REQUIRED` and binds verification to the same digest.
Any project change, every fix cycle, or an unobservable project still requires
independent review. Review may start before completed workers' engine turns
end. Accepted results revoke that attempt's worker MCP channels in the same
transaction; capacity is not released until process exit is confirmed. Native
edits are not intercepted by channel revocation: late project changes still
invalidate the digest-bound review, verification or completion gate.
Before recording a follow-up answer that relies on an existing review, the
runtime checks the current review binding in the task's execution checkout.
A changed binding keeps the proposal in planning for a fresh review and
verification. Final certification repeats the evidence checks.
Verification may run speculatively in the existing ignored-path/scratch sandbox
while the reviewer runs. Candidate results grant no completion authority:
credit requires APPROVE on the exact plan/reviewer/digest binding and a fresh
unchanged-project observation. Stale, rejected and interrupted candidates are
discarded; stop joins the command sequence and kills its subprocess group.
Recovery never replays speculative diagnostic events as verification evidence.
Execution-start metadata is diagnostic only, not a verification lifecycle event.
A speculative run can close a deferred check only after authoritative credit on
the approved digest, with exact argv and a digest matching both the gap author's
runtime-observed publication or review binding and the current project. Execution
ordering does not substitute for or override those revision bindings.
See [speculative verification](docs/features/orchestration/index.md#speculative-verification-during-review).
No model hint can waive review. A model tool call cannot
select a task state, accept the task or directly create an engine session.
A model hint cannot grant a route or infer model family/quality. Eligibility, tier, effort and automatic membership come from the effective version-2 owner routing settings and current observed catalogs; the runtime also enforces pins, denies, provider policy, availability, quota and user authorization. New or renamed identities are not automatically eligible. New audits identify configured stretch policy with the neutral `stretch` key and effort basis; no historical audit is rewritten and this label grants no authorization. Policy refusals happen before worker capacity reservation or engine launch; the coordinator route-menu tool reports the named validation failure without claiming an uncertain storage write. See [settings-only routing](docs/features/routing-and-fallback/index.md#settings-only-routing).

Retained user-source model grants require the complete case-sensitive
`engine/model` token or a separately configured request label. Opaque identity
punctuation is not a substring boundary: another variant or provider path
cannot authorize a shorter flagship identity. Ambiguous unquoted punctuation
is refused; paired quotes explicitly delimit an identity from prose. Request
and negation grammar, source provenance, label ambiguity refusal and all route
hard authorization checks still apply. Confirmed user model requests take
priority over pins, preferences, automatic membership, role tiers and usage
ranking for covered exact-route nodes in every tier, including retries/fixes.
They never waive owner deny/disabled/provider policy, route/profile/recipient
ceilings, supported effort, engine availability or exhausted quota. Failure
fallback cannot silently switch that choice to another model identity.
Plan usage only breaks the last tie between otherwise equal candidates; unknown
usage is reported unavailable, never inferred above the 95% hot band.
No source or settings rewrite repairs a refusal.

ACP selection carries the owner/project ceiling into selected and active
revisions without passing the saved identity envelope as native parameters.
Route/effort changes cannot discard task narrowing or an explicit role profile.
Automatic dispatch filters candidates by route, connection and data-recipient
ceilings, then rechecks the actual chosen observed identity before capacity
reservation. Missing, stale, unmatched or ambiguous automatic identities fail
closed; the coordinator route is not an authorization proxy. This does not
establish paid-live or rendered-host qualification.

After a coordinator question commits `WAITING_USER`, further coordinator text
received in that turn is suppressed, not released by delayed tool callbacks.
This is a display boundary only: it does not cancel engine actions, rewrite
history, or grant authority. Ambiguous late pre-question text may also be omitted;
see [question display ordering](docs/features/orchestration/index.md#plan-submission-and-coordinator-polling).
The same display-only principle applies after coordinator synthesis commits:
later engine prose in that turn is withheld, while the stored synthesis and its
required disclosures remain the runtime's final output. Rejected submissions
do not mute text. This neither stops later tool actions nor changes completion
authority, replayed history, or the content of native engine transcripts. See
[synthesis display](docs/features/task-lifecycle/index.md#runtime-composed-synthesis).
With structured gap disclosures present, both final-answer paths refuse a second
authored disclosure heading, including common English/Russian aliases, before
persistence. The guard never silently deletes a section or changes evidence,
permissions, completion gates or existing replay history; arbitrary semantic
repetition is not an enforced security boundary.

Successful runtime recovery clears the exhausted planning-cycle counter, not
question identities or answer receipts. Only the normal exact-answer transition
can resume a waiting question; dismissal does not reset the cycle. The renewed
cycle still requires a valid structured proposal within three engine prompts.
A controller defect that previously reused the exhausted planning counter for
a recovery answer is fixed and has a focused no-paid regression. This repairs
the recovery transition only; it does not establish why a model produced no
proposal in a separate three-prompt run, nor qualify Desktop batch/worker UI.

The worker result schema requires `report` as one plain string: either the
exact `report_id` of an already published own report, or the complete inline
Markdown (optional `report_title`). When a draft was published through
`write_report`, its returned `report_id` alone is passed as `result_submit.report`;
any other identifier-shaped value is refused before anything is published, and a
unique own-report suggestion is only a
correction hint and does not bind a report. Check blockers and command evidence
are distinct. Rejected submissions retain only a safe error code and bounded
field-path diagnostics in observability events; rejected values and report bodies
are not retained there. Submission rejections contribute to the
`schema_validation_errors` diagnostic alongside argument-validation failures. The loop and write measurement records are metadata only: `project_read` rows keep the path, kind and file digest; gateway and external refusals keep the operation or server, tool and error code; write rows keep byte counts. External calls record an unkeyed SHA-256 of their arguments for repeat detection, which is not redaction. External reply content (`EXTERNAL_TOOL_RESULT`) and worker-activity raw fields remain content-bearing, as documented in [observability](docs/features/observability/index.md#4-redaction-observabilityredact).
Aggregate decisions read metadata only and retain the guarded checkpointed-store
boundary. Missing or ambiguous identities, phase boundaries and native-tool
classification are unavailable, not inferred success; ACP gateway echoes are not
native command receipts. These measurements do not relax authorization, independent
review or final verification.
A well-shaped `tool_call_ref` alone never verifies a check.
Copy the exact reference from the completed command response; do not infer its
prefix. The gateway rejects unknown fields such as `artifacts` in `result_submit`
and returns the bounded field path, not the field value.

Cortex's read-only `read_skill` route reads a Cortex shared/reference id within
the channel's role allowlist, or a project skill (`<dir>/<name>/SKILL.md` directly
below the project's `.agents/skills`, `.claude/skills` or `.codex/skills`) by
name, directory or file path. Only channels that are served `project_read` can
read project skills, so the route grants nothing beyond reading a project file
the same channel could already read; the senior consultant cannot. Cortex ids
always win: a same-named project file cannot replace a Cortex protocol skill's
body or lift its role restriction, and is read only by its path. Path forms
apply the `project_read` path rules (no `..`, empty or `.` parts, no `.git`,
`.cortex`, `.cortex-dev` or workspace owner files), absolute paths must lie
inside the project, symbolic links are never followed, and the file must be a
regular single-link UTF-8 file of at most 65,536 bytes. No glob, sibling file or
user-home path is read. Project skill text is returned labelled as project
content; it is model input and grants no tool access, scope or permission.
After channel authentication succeeds, an unreadable skill is a correctable
tool error naming the accepted forms, not an authority refusal. See
[the unified system prompt](docs/features/unified-system-prompt/index.md#project-skills).

Owner-initiated force-finish is a separate, non-acceptance outcome: an exact
`/cortex-finish` command captured from the ACP/Paseo host or an exact answered
task-closure question is required. The runtime records the source and unfinished
work, fences assignments and owned sessions, and uses terminal `CANCELLED`, not
verified `COMPLETED`. A model claim that the user wants to finish is not an
authorization or a passing review/verification receipt.
Paseo's structured question path is role-neutral across coordinator, worker and runtime fallback requests.
For grouped questions it uses one paginated form and submits the complete set together; incomplete
submissions are rejected. Unique, bounded topic headers are presentation keys mapped back to exact member
identities; a complete legacy id-keyed answer map is also accepted, but mixed or foreign key sets fail
closed. Numeric text-fallback selection uses the same current-open creation/batch order as the displayed
numbers, not random ids. None of these mappings grants access. The "Other..." field and a chat reply are
valid free-text answers. Dismissing or cancelling the form does not create an answer, and a declined or
timed-out host elicitation leaves the durable question pending. Where an ACP host lacks the structured UI,
Cortex can only fall back to a text prompt; it does not enforce that a model asks every question or that
every host renders options. A free-text legacy card's instructional action is not an answer. The live chat
echo after an acknowledged answer shows the question with the answer (`Question: … → Answer: …`); it is a
presentation of the already recorded answer and grants nothing.
Plan-node `work_kind` is a validated routing classification, not a permission grant. It may override role-derived affinity, preferences and stretch eligibility, but never changes role access, scope, tier or owner route authorization. Explicit classifications survive retries and review fixes. Legacy implementation settings expand only in memory on effective reads; editable source rules remain unexpanded, so an unrelated Settings save does not migrate their spelling. Reads do not write configuration; explicit saves still use the existing target authorization and revision checks.

Plan verification commands execute as literal argv without a shell. Submission rejects command substitution, backticks, redirection, pipes, shell operators and standalone parentheses, so a plan cannot defer that syntax error until verification. Structural validation does not prove semantic correctness of a plan, review or implementation.

Worker briefs preserve the assignment objective, scope and dependency report references. The original task goal and review git evidence are shortened only when needed to fit the provisioning limit, and every shortened field identifies the worker-readable capsule or git-output source. These references use tools already available to the worker and do not change its assignment authority. A recovery capsule whose critical sections exceed their byte budget is compacted deterministically, never refused: each truncated field carries its sha256, size, a fenced preview with its original trust label and the tool that returns the full text (`read_source` for user messages and answers). Each trimmed list names its omitted entries, so recovery never silently loses required context.

A worker `passed` check is accepted only with an exact same-attempt `command_run` or user-confirmed
`outside_command` receipt for one completed, nontrivial operation, complete untruncated output and exit
code 0. Engine-native shell calls and file reads do not verify checks. Insufficient evidence stays
`unverified`; observed nonzero exits stay `failed`. Earlier reports are not rewritten, and older evidence
is marked weak when projected under the current rule. Check run history is retained; a failure closes only
on a later exact verified rerun that remains the latest result for that command. Identical argv/cwd reruns
may cross between `command_run` and `outside_command` in either direction, preserving both receipts and
their operation/profile provenance. Same-operation profile changes do not match; this comparison never
grants execution permission. A supported syntax, regex or statically proven literal-expectation error in
the worker's own standalone Python probe can instead be retracted by a later verified passing correction;
the failed receipt stays as `retracted` history, never as a pass. Literal-data probes permit only
single-assignment literal bindings, equality assertions and a small pure-builtin allowlist, bounded to
8,192 source characters and 256 AST nodes, with no imports, IO, product calls or dynamic execution. Only
one expected literal changes; the original false comparison and all corrected true comparisons are
established without executing submitted source. Both
complete outputs, same operation/cwd/profile, unique correction binding and later-run checks remain mandatory. Project test/lint/build and runtime-verification failures are never retracted.\n\nTransport state is distinct from task outcome. A plugin disconnect records available session/task/attempt identities and history completeness; restart recovery reads persisted state and leaves actions with unknown outcomes unresolved. Until reload recovery is qualified for the deployed version, update the plugin between active tasks. A host SIGKILL without SIGTERM cannot be recorded, and native engine actions cannot be represented by the retained command receipt mechanism.

On stdin EOF, the ACP process joins its reader after EOF is observed and before
runtime teardown, so disconnect recording and cancellation are not abandoned
with the daemon reader thread. This does not provide a SIGKILL guarantee or
resolve unknown action outcomes.
On a failed stdout write, it likewise joins the writer's disconnect callback
before joining prompt workers or tearing down the runtime, without joining a
reader blocked on still-open stdin. SIGTERM records the disconnect synchronously.

For `result_submit`, a model-supplied `checks[].command` on a completed check
is descriptive, never evidence. Only the runtime derives `closed_by` and `retracted_by` from real
same-attempt command receipts for failed-check history; malformed fields get
path-specific corrections. A declared-command not_run/unverified gap can also
close by the runtime's own final-verification receipt with identical argv,
complete exit-0 execution and untruncated output on the current reviewed digest,
matching the gap author's runtime-observed publication or reviewed digest;
execution ordering is irrelevant on that unchanged binding. The durable closure
reference is rederived from runtime events, revision observations and output
metadata, never accepted from a model or trusted
merely because a stored result carries `closed_by`.
A new result with an open failure is refused with `open_failed_check` before
publication, request recording or channel revocation. Only an explicit
`assignment_outcome: "failed"` with a structured dead-end `blocker` (kind and
concrete detail) intentionally submits that failed assignment; without the
blocker it is refused with `assignment_blocker_required`. A review that leaves
something it can still do (an INCOMPLETE part without a dead-end blocker, an
unread changed file or required predecessor report of the review's round, a
BLOCK without findings) is refused
before publication with the outstanding items; the channel stays open, the
refusal is retained with its code and named items, and no second reviewer is
scheduled for the same revision. Fix and review rounds without progress
(unchanged reviewed bytes or repeated findings) stop with `review_no_progress`;
there is no changed-command supersession or waiver of review/verification.
An `open_failed_check` refusal names each open check with its `tool_call_ref`,
the gap that keeps it open and what closes it, and `omitted_failed_check` names
each omitted receipt; they name a program and receipt references only, never the
argv, a shell script or command output. Every failed (or still pending) gateway
receipt a submission cites is retained as a runtime event before the other
gates, refused submissions included, so a later result that omits or relabels
it is refused with `omitted_failed_check` and `open_failure` re-derives the
omission on recording; only runs actually cited as checks are retained. Inline
shell scripts verify only as `umask`/`export`/`cd`/`set` setup commands joined by
`;` or `&&` and then one simple nontrivial command, within a small literal word
grammar (plain `$NAME`/`${NAME}` expansions in arguments, a leading `$HOME/` in the
program); a pipe, `||`, `&`, redirection, substitution, any other expansion,
keyword or code-running builtin fails closed to trivial, including some harmless
forms, and a direct-argv check is the supported alternative. It is a bounded
classifier, not a shell parser or sandbox. Command receipts inline at most 64 KiB of output; evidence
always uses the complete retained output.
Accepted receipts disclose `node_status`, not task acceptance.

Missing permitted project reads return `sha256: "absent"` for the existing CAS
creation path, never an access grant. Private paths, links and real read errors
stay refused. Keyless project write/edit/mkdir/delete actions use operation/payload
hashes scoped to their channel; identical retries retain effect receipts and never
repeat a deletion or its confirmation. A pre-effect refusal keeps its rejection
event but no replayable derived-key receipt; an identical retry after fixing the
precondition can run. Explicit keys retain the full structured refusal. Unknown outcomes still fence retries.
Gateway situational-skill hints are advisory and do not replace the result gate.
Failures in the hint's connection, node lookup or observation do not turn an
already-committed write into a rejection; the retained success receipt records
`skill_hint_unavailable`. Result gating attributes only own reserved paths or
successful channel writes, excluding siblings' reserved paths (ambiguous native
edits yield no authorship assertion). This does not bypass mutation authorization.
`project_git` is context only, never check evidence; command receipt rules stay
unchanged. Worker recovery separates the task/coordinator snapshot from the exact
attempt's dispatch audit and native configuration confirmation.

ACP events sent to a host are bounded display projections. Worker cards omit
transcript bodies and raw inputs; timeline tool details are capped, and
oversized values carry truncation markers while the complete task record stays
in project history. Each worker card also has a fixed event budget of at most
13 updates, whatever the worker's tool count or duration. Coordinator and
engine tool cards have a per-task total budget: the first 150 distinct calls
get their own card, later calls are counted on one aggregate card with at most
20 updates, and failed calls past the budget keep their own card up to 50 more.
Session replay applies the same bound to older journals. Permission requests
and user questions are separate host requests and are never folded. These limits
constrain individual wire updates and the number of cards and updates per
task, and avoid retransmitting cumulative content; durable task history and
work remain proportional to the task. Coordinator repeat detection keeps at most
64 title/input/bounded-output digest entries, after redaction and truncation.
Intervening compact context receipts do not displace a matching projection;
references preserve terminal call status, while failed calls retain their output.
This is display deduplication, not suppression of complete MCP responses or
durable evidence.

The Paseo provider withholds only the ACP bridge's own turn-end settlement
(`failed`, `canceled` or `completed`) of a worker card that Cortex still reports
running, when the turn was cancelled or completed. The runtime keeps journaling
and sending only `worker:` card updates after their turn is cancelled; every
other update of a cancelled turn is still dropped. A committed worker result
outranks later cancellation cleanup, including when both map to ACP `failed`;
the provider retains its verdict/count against delayed cleanup frames. This does
not suppress process termination or rewrite the retained cancellation event.
A later non-terminal frame cannot reopen a settled card, and a
failed prompt keeps the bridge's failure. This is display integrity only: it does not
change task state, permissions, results or verification.

Asynchronous worker cards no longer rotate the coordinator's message identity.
The Paseo provider delays only their display events while that message streams,
then releases every event in arrival order at the next message/timeline boundary,
engine-message completion, turn end, load completion, cancellation, failure or close.
Engine-message completion uses an empty ACP text chunk with a unique reserved
`__cortex_message_boundary__:` message identity, also sent by the scheduler after
runtime notices written while workers run. The runtime also closes a complete
busy-steering acknowledgement before entering the controller, whose text tracker
cannot observe that earlier preamble. These markers are retained in ordered
display history and consumed by the provider without a display row. It is not worker completion
or a permission/result signal; it prevents the longer outer ACP prompt from
holding cards after the coordinator stopped writing. Text is neither buffered
nor rewritten. Durable journals keep the original event order and text; this
presentation ordering grants no execution authority and changes no evidence.
Previously recorded split identities and host-persisted rows are not repaired.

The Paseo provider also reduces a worker card's terminal result to its
open-issue count before the host persists the item, and the client reduces
older saved items the same way, so worker summaries, check names, blockers and
evidence gaps are not kept in the coordinator timeline. The selected worker's
summary and checks are read from the project store only by the opened Workers
tab, through the same read-only history reader and display scrubber (summary
clipped at 16,000 characters; an older store's `worker_result` checks pass the
same scrubber). The card never renders its failure text; opening it hands that
already host-redacted text to the Workers tab through the plugin's session-local
selection state, shown only when no attempt history explains the failure. A
failed history read shows a fixed message, never the transport error. This
changes presentation and what the host timeline stores, not task state,
permissions, evidence or verification; the full result remains in the project
store and published reports. The compact card poll also retains the attempt's
durable dispatch route and selection provenance; it does not fetch result bodies,
assignment prompts or transcripts to refresh route text.

The Workers tab's assignment prompt is read the same way, only for a read that
selects a worker (never by a compact card or catalogue read). It shows the
selected attempt's own `delegation_requested` brief, title and acceptance line
through the same display scrubber, not the runtime's worker framing, role
instructions, recovery capsule, documentation catalogue or report bodies
(dependency reports appear only as references). The runtime's
`worker_brief_sent` record adds only attempt identities and a SHA-256/length
of that text; it is non-meaningful `progress` that never wakes the
coordinator. A record that does not match the retained text withholds it. The
scrubber is pattern-based, so a brief can still contain whatever project or
user text the coordinator put in it; this is presentation of an existing
retained request, not a new disclosure boundary.

The Paseo client wraps its rendered panels, composer popover, and timeline card in per-surface error boundaries. If reading or coercing a thrown value fails while formatting the fallback, it uses `unknown error`. This is a UI recovery boundary, not an execution sandbox or a root-cause diagnosis.

Connection-registry owner-only reads retry a detached regular inode
(`st_nlink == 0`) by path for at most eight attempts after an atomic replace;
each open still checks no-follow, ownership and private modes. Hardlinks,
symlinks and unsafe replacements remain refused. Unchanged registry transactions
do not rewrite the file. This avoids validation-only inode churn, not a grant to
unsafe storage.

Recorded user requests may name an exact bare model ID unique across enabled
configured engines; shared bare IDs grant neither route and receive a plan
preview diagnostic. Terminal prose punctuation cannot truncate another enabled
configured ID. For enabled exact identities, direct `на/on/via/with` naming or a
request-form verb in the same user message supplies request grammar; negations
still apply. Configured labels retain conservative local grammar. A restrictive
user selection ("только Opus", "do the audit only on Opus") and a parenthesized
exact identity after a name are requests; remarks ("only Opus found the bug")
and negations ("только не Opus") are not. A real user answer that is exactly one
of its question's options grants the single model that option selects; the
asking model wrote the option, so an option that only mentions a model (its
result, quota, opinion or past work), a bare model name or a name after a lone
verb of choice ("Выбрать Opus", "Pick Opus") chosen under a question that does
not ask which model to use ("Чей результат аудита принять?", "Какой аудит
выбрать?"), the question text, unchosen options and ambiguous answers never
grant, and a sentence judging a result never scopes which nodes a choice pins.
Model prose and hints remain non-consent, and hard route
authorization checks are unchanged. See [routing](docs/features/routing-and-fallback/index.md#settings-only-routing).
Matching-pin chat notices compare audit-bound engine/model and any explicit effort;
silence is not consent or native configuration evidence. Missing/conflicting
bindings remain visible as unverifiable, and historical audits/notices are not
rewritten. See [notice boundaries](docs/features/routing-and-fallback/index.md#ignored-model-hint-notices).
Creating a project file creates its missing parent directories only after the
private-path, write-grant and user deny-write checks passed for the full path;
each component is created and reopened no-follow, and a file or symlink component
is refused (`gateway_parent_not_directory`) without following it.

Project file writes never accept a partial read as overwrite authority. `project_read` returns line pages
whose `sha256` identifies the bytes read; only a channel's complete, non-truncated coverage of the current
bytes yields `full_content_sha256`, and `project_write` of an existing file requires that value
(`gateway_complete_read_required` otherwise). A stale read, a hand-supplied hash and `absent` for an
existing file are refused before anything changes. A large shrink is flagged in the receipt
(`drastic_shrink`) rather than refused, but it still needs complete coverage. `project_edit` applies
ordered exact replacements atomically against the current bytes (no prior read is required); a missing or
ambiguous match refuses the whole edit and returns the current page as a read, while an edit already in the
requested state succeeds as `unchanged`. `project_write` accepts `encoding: "base64"` for binary content under the
same path policy, hashes and receipts. A hard-linked project file may be read, and a write replaces only the directory
entry (rename of a temporary file), so the shared inode, including a link outside the project, is never modified;
command `changed_paths` are attributed project writes. Coverage is per channel and per digest and stores no file bodies.
A successful `project_write` or `project_edit` replaces the path's coverage with the digest of the bytes
that channel just produced: complete after a full write or an edit of a completely read file (never for
`.gitignore`, where the runtime adds protected lines), otherwise partial. A later change by anyone else
therefore still fails as a stale read, and partial coverage never becomes overwrite authority. Coverage is
a runtime ledger, not a permission grant: private, link and deny-write checks still apply before any read
or write. `project_read` returns at most 2000 characters of one line per page; a longer line ends its page
with `next_column`, and it counts as read only once every segment was returned in order from its first
character, so a skipped segment never becomes complete coverage.

Gateway writes and edits of the root `.gitignore` add the active channel's rule only when no line already equals it. This is not post-command reconciliation. Edit presentation uses the protected bytes committed by the writer. Diff matching is skipped before computation when combined input exceeds 32,000 characters or the before/after line-count product exceeds 250,000; the receipt marks `diff_omitted: "input_budget"` and `diff_truncated: true`. Hash and size evidence remain available.

## Cortex Workspace local service (in development)

The Cortex Workspace desktop slice has a user-level machine service that owns the Cortex ACP child. It is not released, and its design lives in [Cortex Workspace](docs/cortex-workspace/index.md).

- The service listens only on a Unix domain socket in a private per-user runtime directory (directory mode 0700, socket mode 0600). Both ends compare the peer's effective user with their own. It opens no TCP, loopback HTTP or WebSocket listener. The one network listener it can open is the iroh UDP endpoint of remote access, described below, bound only while the owner has turned remote access on (off by default).
- The service's own private folders (runtime directory and, in the dev channel, the config, state, cache and data folders it gives started processes) must be real directories owned by the current user. One that is merely too permissive (for example 0775 under the user's own primary group from a 002 umask, or 0755) is made 0700 in place through the opened directory; a world-writable folder, a folder group-writable by another group or a foreign-owned folder stays refused, and the owner sees a refusal naming the folder and the problem instead of a generic filesystem failure.
- **Extracted Workspace Dev.** Only a Dev build honors the native startup opt-in
  `CORTEX_WORKSPACE_DEV_EXTRACTED_PREFIX`. It requires a user-owned 0700 prefix and private
  per-run XDG roots there. The service accepts only the Dev runtime in that prefix's Debian
  layout, with owned, non-group/world-writable payload entries and no symlink escape. Only
  direct relative links to files inside the bundled engines tree are accepted there. This is a
  layout and permissions check, not archive authentication; verify the trusted archive separately.
  A malformed request does not fall back to the installed service or profile. The desktop uses only
  the private socket and never asks systemd to start the installed unit; native notifications and
  their installed desktop activation path are disabled for the extracted run. This separates
  registries from the persistent installation, not authority from the same OS user. Use a separate
  disposable project: the Dev project-store name and the native account HOME do not change.
  See [configuration](docs/configuration.md#workspace-dev-extracted-package).
- **Stopped source merges.** Detection does not dispatch Cortex. Only an explicit owner handoff
  binds the continuation to the saved source and target commits. The service supplies a bounded,
  JSON-quoted Git path manifest with completeness and truncation flags, not file contents;
  paths are data, not instructions. Unconfirmed writers or Git outcomes block manual merge
  mutations; inspection does not replay the prompt. A resolved revision requires fresh
  independent review and runtime verification. Neither handoff nor a check request authorizes
  integration or push. For a legacy record without a saved merge identity, explicit owner
  confirmation can clear the record only after execution has settled and the service verifies
  the saved branch in the original checkout, with no remaining Git operation or unmerged index
  entries. Clearing neither changes Git nor restores earlier evidence. See [Integration and conflicts](docs/cortex-workspace/local-task-execution.md#integration-and-conflicts).
- **Opening a pull request in the browser.** **View PR** (on the merge card and in the Git menu) is the
  only action that opens a pull request address. The UI displays the recorded address as text and sends
  only the project and task identifiers; it never passes the displayed URL to the opener. The service
  returns the recorded address only if it is an `https` address on the host of the repository's `origin`
  remote, shaped `https://<host>/<owner>/<repo>/pull/<number>`, and checks it against the remote again at
  the press. The desktop checks it once more (an `https://` prefix, host characters limited to letters,
  digits, `.` and `-`, no whitespace or control characters, at most 2048 bytes). Outside extracted
  Dev, the desktop then starts the first
  of two fixed programs that is an existing regular file with an execute bit:
  `/usr/bin/xdg-open <address>`, or `/usr/bin/gio open <address>` when `/usr/bin/xdg-open` is not
  runnable. The address is the last argument, no shell is used, and the program's standard streams are
  discarded. A launcher that exits with a failure is reported as "The browser could not open this pull
  request."; one still running after 8 seconds counts as handed over. The item is offered only when the
  same search finds one of those two programs. Extracted Dev never reaches those programs from
  View PR. Only a desktop executable in a validated private Dev extraction can use the native
  qualification recorder; it appends the address digest to a private, existing 0600 file. A
  missing or invalid recorder never falls through to a system opener. The launcher also requires
  a complete native verifier result from the candidate service process before desktop input.
  See [the qualification gates](docs/cortex-workspace/testing-and-acceptance.md#extracted-dev-readiness-gates).
- **GitHub CLI (`gh`).** The service runs `gh pr create` in one place only: **Create PR…** in the Finish task
  dialog. It runs `gh pr create --repo <HOST/OWNER/REPO> --base <target> --head <task branch> --title <first line of the title> --body <description>`,
  with `--draft` when the owner chooses a draft, in the task's checkout, with `GIT_TERMINAL_PROMPT=0`,
  `LC_ALL=C`, an empty standard input, standard error discarded and a 25-second limit. The repository comes
  from the validated `origin`; `GH_REPO` and a conflicting `GH_HOST` are removed from the child. It runs only
  after the owner-confirmed Push witness check (repository, full commit and remote ref recorded by that push)
  and a read-only `gh api --method GET --hostname <HOST> repos/<OWNER>/<REPO>/git/ref/heads/<BRANCH>` lookup
  that must return exactly the witnessed commit; otherwise nothing is dispatched to `gh pr create` and the
  action is refused. After creation, a read-only `gh api` read of `repos/<OWNER>/<REPO>/pulls/<N>` must match
  the repository, head ref, full head SHA, base, open state and exact address before the address is saved.
  The service never pushes for it. Only the standard output of a successful run is considered. All of it is collected, but only its
  first 64 KiB is inspected: the last non-empty line of that part is the candidate, and it is recorded as
  the pull request address only if it passes the https and remote-host check above. The output of a failed
  or timed-out run is never read for an address. Any other output records the pull request without an
  address, and the pull request is still recorded as created. If `gh` cannot be started or exits with an
  error, the action is refused with "GitHub CLI could not create the pull request. Check gh login and
  repository access." If it does not
  finish within 25 seconds, it is refused with "GitHub did not answer. Check your pull requests before retrying." A recorded attempt is not repeated by the service, and a later Create PR… for the same task is refused with "A pull request was already attempted. Check GitHub before creating another." Before it starts, the service only looks for `gh` on its own search path (`ghAvailable`); when `gh` is not found, the action is refused with "Install and sign in to GitHub CLI (gh) to create a pull request." Besides `gh pr create`, the service runs `gh` only for the scoped read-only `gh api --method GET` lookups above and for `gh auth status` probes in preflight and owner-action rechecks.
- The actor of a Workspace request is the authenticated peer. Client-supplied actor values are replaced, and the ACP child receives a cleared environment.
- Cortex exposes the `_cortex/workspace` extension method alongside the unchanged standard ACP methods. Its operations are a closed set; the desktop shell has a closed bridge command forwarding that set and a separate bounded native notification command.
- **Show in file manager** is a local desktop action, not a phone or tunnel operation. The bridge requests an existing checkout file from the service, then passes its encoded URI to a fixed session-bus method without a shell. The fallback hands only the containing directory to the existing system opener, never the file as an executable or a command.
- Local desktop Code saves through the machine service. Each save requires a valid write lease for that file in the selected checkout and a matching disk revision; it does not require every writer in the checkout to stop. The service checks the revision again before replacing the file through a temporary file and rename. It does not follow links, refuses `.git`, `.cortex` and `.cortex-dev` at any depth, and limits saves to 512 KiB of UTF-8 text.
  The lease excludes overlapping Code saves and structured worker file mutations in the same logical project and Workspace channel. It is not a filesystem lock: host commands, outside writes from other projects, native engine writes, Git, external editors and terminals are outside the guarantee. Revision checks reject observed disk changes but cannot prevent races with those writers. See [editing files](docs/cortex-workspace/local-task-execution.md#editing-files).
- Git, file reads, Cortex settings and the owner's task records run through a fixed helper the service starts with a cleared environment. The desktop shell names an operation from a closed set, a registered project and parameters; the service supplies every root and directory. Paths stay inside the checkout and outside `.git`, `.cortex` and `.cortex-dev`, links cannot leave it, a push never forces, and the helper has no arbitrary-command operation. See [security design](docs/cortex-workspace/security.md).
- Code's file tree shows the owner every file and folder in the project folder, including dot-files, secret-named files such as `.env` and everything Git ignores (marked as ignored), and opens and saves them under the same rules as any other file. This is the owner's own window, not a disclosure to a model: it sends nothing to an engine or provider, and the commit message and pull request texts still leave out secret-named files and read only what Git reports, which never includes ignored files. Still never listed or opened: `.git` at any depth, `.cortex`, `.cortex-dev` and the other Cortex private names (`.cortex-dev-owner.json`, `.cortex-workspace-dev.json`, `.cortex-workspace.json`), anything outside the checkout, and any folder reached through a link (a link is listed, never followed). Folders are read one level at a time on demand (`repo.list`), at most 500 entries each, by descriptor and without following links.
- A composer's `/` list reads the project's own skills with `repo.skills`: the same read-only index the coordinator's instructions list and `read_skill` reads (`.agents/skills`, `.claude/skills`, `.codex/skills`, one level each, at most 64 skills, by descriptor and without following links, each SKILL.md a regular single-link file of at most 64 KiB, outside `.git` and the Cortex private names). It returns only each skill's folder name, project-relative path and front-matter description cut to 300 characters, never a skill body, and activates nothing. The composer's `@` list filters the file names the repository snapshot already returned (`git ls-files --cached` and `--others --exclude-standard`, at most 20,000), in the window; it adds no file read. Picking a file or a skill writes plain text into the owner's message; nothing is sent until the owner sends it.
- Folder search for adding a project takes only the text the owner typed, through closed Rust request types in both bridge and service. The service chooses the roots and limits and runs a fixed helper with a cleared environment. It reveals folder names and whether `.git` exists, never file contents, and it never offers `.cortex`, `.cortex-dev` or the service's private directories.
- The owner terminal is a separate, intentional same-user execution grant. Only
  terminal.open/read/write/resize/close are exposed, validated by shared closed Rust request types in both
  bridge and service. Initial cwd comes from the registered project, never the webview; the service
  selects the user's interactive shell. There is no one-shot exec, executable, argv, cwd or environment
  override API. Input is still full shell authority, including the user's files, accounts and network:
  compromised trusted webview content can exercise that authority. This is not model-gateway containment
  or protection from same-user code. At most eight PTYs retain 64 KiB each; read pages, writes,
  dimensions, UI scrollback and input queues are bounded. Output/input are not journaled or logged.
  Hide/window close detaches; Close shell, project removal and service stop send TERM then KILL to the
  dedicated session's job groups and reap the shell. Lost sessions and uncertain input are never replayed.
  Daemonized jobs that escape the session, abrupt service death and shell CPU/memory use remain outside
  cleanup guarantees. See [owner terminal reasoning](docs/cortex-workspace/security.md#owner-terminal-built-local-linux).
- The desktop window loads bundled local content only, under a content security policy that allows Tauri IPC connections alone. It has no shell, filesystem or process capability, and external navigation and new windows are refused.
- Links and file paths in messages are handed to the system only through one native desktop command (`apps/cortex-workspace-desktop/src-tauri/src/opener.rs`) that the local main window alone may call, after an explicit UI activation; it is not a model tool or a remote-machine command, and it checks the window origin and arguments, not an attested user gesture. It accepts typed file or URL targets only: a file must be an existing canonical regular absolute file, a URL must be http(s) or mailto. It runs fixed executables (a VS Code-family launcher or `xdg-open`/`gio open`) with an argv and no shell, and the system file handler refuses executables and `.desktop` launchers.
- Code previews checkout files inside that policy. Pictures, video and sound play from in-memory `blob:`
  URLs (`media-src 'self' blob:`, the only media source); their bytes come through the guarded file read,
  chosen by file name from a fixed list of picture, video and sound types, at most 4 MiB, never through a
  link and never for saving. SVG is shown only through an image element, which runs no script and loads
  nothing. HTML is parsed into an inert document and rebuilt from structural tags alone, dropping scripts,
  styles, forms, frames, links, event and style attributes and every non-inline image; links in a preview
  are never followed. Markdown uses the conversation renderer, which does not interpret HTML. PDF and
  other binary files are not rendered; frames and objects stay blocked. See
  [viewing files](docs/cortex-workspace/local-task-execution.md#viewing-files). It draws its own frame, so
  the main window may minimize, maximize, close and drag itself and read its maximized state, and nothing
  else.
- Default/release ACP uses `.cortex`; Workspace Dev uses `.cortex-dev` in the same project folder. Release data never prevents Dev registration and is never opened or migrated by Dev. The `.cortex-dev/owner.json` reservation binds the dev directory by device/inode at registration; an unowned, linked or replaced dev store is refused. A validated old `.cortex-dev-owner.json` is moved inside the dev store; a matching private `.cortex-workspace-dev.json` is retained there as `legacy-workspace.json`. Unknown or foreign legacy workspace files are not removed, and no old database is read. Missing sprint members can be restored as drafts only from an exactly matched saved plan and dependency mapping; the queue pauses, and no old execution, verification or acceptance is inferred. Private-path, Git and snapshot rules exclude both stores. This is accidental cross-channel protection, not isolation from a same-user shell. See [CW-S12](docs/cortex-workspace/backlog/CW-S12.md).
- Desktop notifications come from the machine service, which opens the owner's own session bus (the
  per-user `DBUS_SESSION_BUS_ADDRESS` `unix:path=` socket, else `$XDG_RUNTIME_DIR/bus`) with EXTERNAL
  authentication and makes only `Hello`, one `AddMatch` for the notification server's signals, `Notify`
  and `CloseNotification`, every call with NO_AUTO_START; it answers no method call and needs no display.
  A notification carries a bounded, markup-escaped work label and, for a stopped task or a paused sprint,
  the runtime's one-line reason, never question text, paths, commands or credentials. Notifications confer
  no execution or answering authority. Each raised notification keeps one target of a closed shape
  (`question`, `planning`, `result`, `task` or `sprint` with bounded project, task or sprint and decision
  ids, no other field) against the id the server returned. Only `ActionInvoked(id, "default")` for an id
  the service raised, from the server that returned that id, once, acts: the service sends the kept target
  to subscribed windows as `notification_open` with a random nonce (never the token), and the desktop
  shell checks the exact shape (`policy::notification_target`), claims the click with
  `notifications.opened` and only then receives the activation token, which it passes to GTK solely to
  raise its window. A click carries no data of its own, so another session-bus client can at most reopen a
  place the service itself named. If no window claims the click within two seconds, the
  service starts the app with `systemd-run --user` running the desktop entry's fixed binary path, with the
  token (ASCII graphic characters only, at most 512) as `DESKTOP_STARTUP_ID` and `XDG_ACTIVATION_TOKEN`
  and no other input; the target waits in memory for that window's `notifications.take`, once, for one
  minute. The window's `notifications.report` carries only the focused task or sprint ids and the enabled
  kinds (closed shapes in the desktop request allowlist and again in the service); the service keeps the
  kinds and the newest 1000 announced item states in `notifications.json` (0600) in its state folder.
  The desktop's `notifications.report`, `.opened` and `.take` are outside the remote allowlist and the
  phone's request policy, and the phone bridge drops `notification_open`. The one notification method a
  paired phone may call through the tunnel is `notifications.subscribe` with no parameters (see
  [phone notifications](#phone-notifications)); the phone's web view cannot send it either. The window's capability is still the same three commands; no URI scheme, MIME
  handler, D-Bus service or desktop-entry change is added. The service withdraws its notifications when it
  stops. A service that does not raise notifications leaves the window's own question notifications: only
  the bundled main window can submit them, through the same closed target shape, with clicks handled by
  the window's process alone, and the window withdraws them when it closes.
- Workspace invalidation hooks are connection-local to supported Store writers, not a security boundary on arbitrary SQLite clients. Independent/older writers do not need Workspace tables or callbacks; their writes are not guaranteed to publish notifications. Normal writable initialization retires legacy persistent outbox hooks; inspection leaves the durable schema untouched.
- Both channels ship a root-owned runtime under `/usr/lib/<package>/runtime`. Units ignore legacy `service.env` files and use fixed absolute launch paths. Login-start symlinks and minimal best-effort maintainer scripts start/restart/stop only the matching user unit; they never access Paseo or delete user data. The native service-start path runs absolute `/usr/bin/systemctl --user` with fixed reload/start arguments, not webview-supplied commands. The separate editor action accepts only a registered project identity and fixed VS Code arguments. Dev XDG isolation and unchanged native account references remain in force.
- [Standalone desktop qualification](docs/project/verification.md#standalone-cortex-workspace-desktop) runs a trusted, locally built Dev archive extracted without installation (`dpkg-deb -x`, no maintainer scripts) as the owner, in the owner's real environment: real HOME, session bus/keyring, native Codex/Claude and GitHub CLI logins (`GH_CONFIG_DIR` points at the owner's existing gh configuration; nothing is copied). Its five XDG roots and service socket are private to the run, so the persistent release and Dev installations, their settings, registrations and stores, production `~/.paseo` and the owner's projects are not used; the extracted app cannot fall back to either installed unit, and the launcher unsets inherited `CORTEX_ROUTING`. There is no nested namespace lab: this is the product's own channel separation, not a boundary for hostile same-user code. Smoke may use the owner's display. Drive requires a private, authenticated Xvfb (per-run X authority cookie, own Xvfb process) and refuses missing private display tools before archive handling. Before any program from the archive is extracted or run with the owner's HOME, gh configuration and session bus, the launcher requires the build's own manifest to be a regular, non-symlink, user-owned, not group/world-writable file whose package identity and SHA-256 match the archive; an absent, symlinked, unreadable or mismatching manifest refuses nonzero before extraction or launch. `drive` is bound to the exact archive digest recorded by a successful private-Xvfb smoke and refuses any other archive. A screenshot counts only when capture exits 0 and the file is a complete, decodable PNG; a failed or partial capture is refused and no digest is retained. Smoke checks logins by exit status, retains sanitized route/default readback and never account output or settings bodies; environment gaps (no display, missing tool, missing login) are recorded as unobserved steps and never count as a pass. Optional installation passes the verified private Dev archive snapshot to apt via sudo, not the mutable build path. Cleanup refuses an unlisted Debian archive anywhere under either named build directory. Paid use still requires explicit authorization for this surface and mode.
- The package does not depend on Bubblewrap and does not weaken AppArmor or kernel policy. Confined commands and contained MCP servers need a kernel with Landlock enabled (an active LSM); without it they fail closed while engine launches and the Auto host route are unaffected. See [platforms](docs/cortex-workspace/platforms.md).
- `history.page` exposes bounded display text, plans, tool titles/status and each call's selected target (path, command, query, skill or report reference) and non-private edit text, not raw tool arguments/results or private report bodies. An untitled update is judged private or not by the title retained for its call, and stays private when none was retained. A refused call keeps only a credential-redacted reason of at most 1,000 characters: the runtime refusal's `message`, or a Cortex tool's error text; engine tool output and report/draft refusals are not kept. A worker's edit lines come from the runtime's own record of that write (unclipped records only), credential-redacted and bounded per edit and per read. Its cursor binds the project, task, session, page size and frozen high-water mark. Owner queue and review drafts live in private service state, separate from immutable captured source history.
- The service-only `_cortex/workspace_control` method resolves the registered execution and live session/current owner epoch before accepting prompts, worker guidance, worker cancellation or a command-stop request. Stop/Close alone may cancel the exact bound launch before its runtime row exists even when retries have advanced the coordinator epoch; prompts and other controls retain current-epoch checks. It is not a model MCP tool. Worker guidance targets one live channel and is marked delivered when appended to a successful Cortex tool response, not when read or acted on. It grants no authority. The exact gateway command reference and a reason remain required for stopping a command; native engine actions cannot be stopped through this control.
- Task options are session-local, and verified linked worktrees share the logical project store. Worktree cleanup never forces dirty removal. Owner-confirmed integration checks current runtime evidence and checkout observation, refuses a dirty or incompatible target, and never hides a push. PR attempts use the installed GitHub CLI and retain ambiguous outcomes without copying credentials. Archive purge uses the existing exclusive-lock/journaled runtime cleanup, active-execution protection and explicit space-reclamation outcome. Project removal keeps files and history; the native editor action resolves only a registered project, never a webview-supplied command. See [task execution](docs/cortex-workspace/local-task-execution.md).
- Settings drafts retain their original file revision across background refreshes. A structured revision conflict requires explicit review and reapplication, never automatic retry or replacement with the newest write token. Refused defaults saves do not write Workspace-only access/checkout choices. Worktree reset uses the service-resolved channel default without moving existing worktrees. Sprint model preflight is a usability prerequisite, not a replacement for runtime routing and authorization.
- Retention uses the machine wall clock. Deterministic tests inject a callable into maintenance only; there is no wire field, environment override or packaged fixture for advancing it. The host clock and product timestamps are not rewritten.
- Pre-save MCP discovery is an explicit owner action, not a model tool. It uses the existing gateway sandbox with an empty project and the owner's draft read/write/network grants. It sends initialize and paginated tools/list only, never tools/call; a failed sandbox is not retried outside containment. The listing deadline is 15 seconds, with bounded frames/pages/tools, process-group termination and private runtime-directory cleanup. Stored environment secrets are resolved internally and never returned; stderr and exception messages are discarded. Tool descriptions are untrusted, bounded and redacted. Discovery neither saves nor widens tool access; the editable matrix retains configured grants.
- An engine session that fails to start keeps secret-free start diagnostics. The engine host retains the
  last 8 KiB of the engine process's stderr in memory (every byte is counted), its exit status and why the
  ACP transport closed (for example `eof`, `update_for_unknown_session`, `invalid_frame`,
  `authority_refused`). The failure event (`NATIVE_SESSION_START_RETRIED` or
  `NATIVE_SESSION_START_FAILED`) records the exit status, close reason and stderr size; the tail is
  redacted (values after credential-like keys such as `token=` or `Authorization: Bearer`, known secret
  prefixes such as `sk-`, `ghp_` or JWTs, long mixed letter/digit runs other than UUIDs, and control
  characters) and written only to `<store>/<task>/.native-diagnostics/<channel>-start-<attempt>.log`
  (directory 0700, file 0600, never replaced), which the event names with its SHA-256. Redaction is
  pattern-based and can miss a secret in an unusual format, so the file stays private and is never sent to
  a model, the timeline or a log. A file that cannot be written is reported in the event as `file_error`.
- Service status events are retained in the channel's private state directory under logs: directory 0700, files 0600, service.log and one rotated backup, about 64 KiB each. Records contain only a timestamp and a fixed category (helper success/refusal/failure, agent start/stop or scheduler failure), never prompt bodies, environment, command lines, credentials, stdout or stderr. Logging failure does not undo a committed action. Open logs asks the trusted service for that directory and invokes the fixed native folder opener; the webview cannot provide an arbitrary path.
- Shared-checkout tasks whose checkout and target branches match never use a merge or PR. Successful channel-bound write hashes are compared with bounded local branch content/history; unknown evidence never claims containment. A confirmed same-branch commit stages only matching task-recorded pending files, refuses ambiguous same-file staging/modes/links, and preserves unrelated staged files. Native/command-only changes without attributable file receipts require explicit external/as-is closure or manual Git work, never silent staging of the shared checkout. This is content evidence, not hostile-same-user isolation.
- **Automatic sprint policy.** The Automatic continuation policy is a trusted Rust option that the
  scheduler binds when it starts a task of an Automatic sprint. Commands, plans, model tool calls and the
  wire protocol cannot set, change or remove it, and the `automatic.finish` and `automatic.queue`
  operations are reachable only from the scheduler, never from ordinary RPC. The automatic finish admits
  only the project folder checkout, with saved automatic acceptance or explicit owner consent for that
  result version, digest, sprint and target, on the task's own branch. It makes a local commit of the
  task's recorded files under the repository lock and never pushes, fetches, creates a pull request or
  merges. A different target branch waits for owner confirmation through the existing integration path.
  The runtime verifies the approved plan before automatic acceptance. A worker’s skipped, unrequested paid
  qualification can be retained as a report note after verification of the current work. The note grants
  no permission and supplies no passing receipt. Required plan commands, attempted checks and other policy
  refusals retain their evidence requirements. Closing an unaccepted result as is records an owner
  disposition with a verification warning and creates no commit. A runtime stop of any task (manual or Automatic) is recorded as a continuation with its reason, evidence and
  strategy-ladder rung (the [no-stop policy](docs/features/task-lifecycle/index.md#no-stop-policy-and-the-strategy-ladder)),
  never as a question or hold for the owner, and with no attempt cap; only a route the host cannot
  qualify waits (`external_blocker`), resumed by the runtime itself. Access actions are a separate
  wait: authentication errors are evaluated only in recognized access-related command receipts
  or reported explicitly as missing access. A dotted service name is a display key, not a
  login command or probe target; only built-in service keys offer those actions.
  Escalated routes, a senior
  consultation, another role or another coordinator model are routing and planning changes only. The scheduler can recheck a known finish refusal
  only against the same saved hold under the existing Git and machine
  admission checks; no dispatched or unknown effect is replayed. Cancellation and archival never certify unfinished prerequisites or authorize discarding existing changes. A continuation resumes only once every owned session is confirmed gone; an effect with an unknown outcome is then settled as `unknown` (`settled_by: runtime_reverify`, `RUNTIME_UNCERTAINTY_RECORDED`) for the coordinator to re-verify from files, Git state and receipts, never replayed and never certified, and a command that may still run is waited for until the host settles it with process evidence. Neither the policy nor a continuation grants permission or changes review, verification or access rules. See [sprint run modes](docs/cortex-workspace/local-task-execution.md#sprint-run-modes).
- Post-merge cleanup uses Git without force only after the integration is recorded. A retained dirty/failed cleanup never masquerades as a failed merge. Source updates retain the previous result identity/version and require different, passing runtime evidence before renewed acceptance. Large and non-text conflicts retain bounded metadata and explicit manual Git instructions rather than writing placeholder text into a file.
- Task titles and Commit, squash and Push drafts may be written by a model. The task description
  (title only) or bounded Git draft context leaves the machine for the chosen connection's provider
  (Anthropic for Claude, OpenAI for Codex) under the owner's native account. The draft is written in a minimal ACP
  engine session started with the route's ordinary engine-session launch (cleared environment, real `HOME`
  and the connection's native directory; for Codex the native launcher that turns the owner's Codex MCP
  servers off and verifies that and the disabled shell, plugin, hook and app features before App Server
  starts; a Codex connection without that launcher is not used). The session gets an empty private scratch
  directory, no ACP MCP servers and the engine's read-only mode; every
  permission request is denied, and a permission request or tool call discards the answer. A Claude
  session gets the same native tool restriction as every Cortex Claude session (no native shell, edit,
  subagent, hook, plugin or owner-configured MCP server; see [Native engine tools](#native-engine-tools)).
  Denying permissions does not stop a tool the engine runs without asking (Claude's own read and search
  tools in its working directory); a tool call is detected and the answer discarded, but only after the
  tool has run, so the prompt asks for text only and this is not containment. The session has no runtime
  task, writes no project store and is not shown in any history; it ends after 25 seconds at most and its
  process group is reaped. Unicode format characters (bidirectional controls, zero-width characters, the
  byte order mark) are removed from generated text. Git reads for drafting do not fetch from a remote.
  Commit and squash prompts omit known private paths and binary content and list non-private untracked
  files by name only. Push prompts use local commit subjects, bodies and aggregate statistics, not
  patches or remote URLs. Known credential-like lines and private-key material are omitted before
  clipping prompt text. This filtering cannot recognize arbitrary secrets in repository text: generate
  only when the draft context may be sent to the provider. The answer stays editable. Without a
  matching Push summary, PR text uses local Git metadata, not a separate model call. Nothing is
  committed, merged or pushed without the owner's confirmation. A confirmed Push summary is stored privately after a successful Push
  and is not sent to the remote by Git. It can prefill the editable PR body and never authorizes PR
  creation or bypasses the push witness. Checkout-level drafts are not exposed to paired phones.
  `short-text.log` (file
  0600 in the private log folder) records route identity, reason and outcome codes only, never prompts,
  descriptions, diffs or answers. See
  [short model texts](docs/cortex-workspace/local-task-execution.md#short-model-texts).
- The task side chat (`product.task` `side_question`, see [task side chat](docs/features/side-chat/index.md))
  sends the owner's question with a bounded task context (title and description, runtime state and open
  obstacle facts, newest committed report summaries and one bounded report page, the latest coordinator
  messages, the task diff through the draft filter above, and the chat's own recent exchanges) to the
  provider of the task's coordinator route or the model the owner picked in the chat, under the owner's
  native account. It runs in the same hardened, MCP-free, read-only engine session as the drafts above
  (`short_text_engine::side_answer`), with one difference: on Claude the session works in the task's
  checkout so Claude Code's native Read, Grep and Glob can read it, which means file contents the model
  chooses to read also leave the machine. WebFetch and WebSearch are removed as well as every mutating and
  delegating tool; Cortex private state, credential files and paths outside the checkout ask
  and every permission request is answered no; any tool call other than `read` or `search` discards the
  answer, detected after it ran (the removed tools are the containment, not this check). Codex keeps its
  native shell off, works in a private scratch directory and gets exactly one MCP server through the
  hardened launcher: the side chat's read-only project files server (`side_chat_files`, started from the
  service binary, rooted at the checkout; `read_file`, `list_directory`, `search_files` only), which
  refuses absolute paths, `..`, symlink escapes, `.git`, Cortex private state and credential names, so
  the files Codex reads also leave the machine.
  The side chat never creates a runtime task or channel, opens the project store only `LiveReadOnly`,
  writes nothing to the task conversation and delivers nothing to the coordinator. Its history is kept in
  the service state directory (`side-chat/<project>/<task digest>.json`, 0600, newest 60 exchanges),
  never in the project; `short-text.log` gets route and outcome codes only. Paired phones may use the
  side-chat actions for a named task, like task drafts.
- Remote access (built, no-paid tests only, not qualified; nothing verified on a physical Android device
  or through a relay): off by default; when the owner turns it on, the service binds an iroh UDP endpoint
  that uses no n0 public relay and no n0 address lookup. Each remote stream goes to a fresh local-socket
  connection, and every request first passes a closed, fail-closed allowlist
  (`crates/workspace-service/src/remote_policy.rs`): every `remote.*` method, `service.stop`,
  `terminal.*`, `voice.*`, file saves, all Git write actions, native login, package install and rollback,
  and unknown methods are refused. A paired phone can still start and steer work and change machine and
  project settings, which is command execution authority on the machine. Identity, pairing, device
  registry, revocation, version model and relay limits are in
  [Remote access and device authorization](#remote-access-and-device-authorization); more detail is in
  [remote access security](docs/cortex-workspace/security.md#remote-access-built-not-qualified).
- The same OS user is local trust, not isolation from hostile code running as that user. The different-OS-user peer check is not yet verified.

Automatic result repair uses the assigned task and its existing access. A repair request grants no additional permission. Review and verification apply to the new result. Closing after workspace drift keeps the original acceptance attached to its earlier result version.

### Remote access and device authorization

Remote access is built and checked only by no-paid automated tests (loopback endpoints, no relay). It is not qualified: nothing was verified on a physical phone or through a relay.

- **Identity keys.** The machine has one 32-byte key in a 0600 file (`remote-host.key`) in the service's private state directory; each phone has its own key in a 0600 file in the app's private data with Android backup off. Private files are opened without following links and refused if owned by another user or readable by group or others. They are not wrapped by an OS keyring or the Android Keystore, so code running as the same user, or a rooted phone, can read them. Peers authenticate each other by endpoint identity in the iroh handshake, independently of any relay.
- **Pairing.** One offer is open at a time. The link (QR code or copied text) carries the machine identity, a 32-byte single-use secret, the machine name, the relay if any and direct addresses, and expires after 5 minutes. The secret is compared in constant time and consumed on use; the fifth wrong secret cancels the offer. After a correct secret both screens show a fingerprint derived from both identities and the secret, and the owner must press Allow on the computer. Before Allow the machine reveals nothing but the pairing status. There is no short pairing code. The link holds the secret while valid, so share it only with the phone being paired.
- **Device registry and failure behavior.** Allow stores the device in the registry file, and the phone is told `allowed` only after the save succeeded. If the save fails, nothing is added in memory, the owner's decision returns the error (`remote_state_unavailable`), the phone is told `cancelled` and the device gains no RPC access. Before the device is written to the registry, the host records a private grant-intent file (`remote-grant.json`, 0600) naming the provisional device, and removes it once the registry save is confirmed. If the save is uncertain (for example the rename or the directory sync fails after the new registry may already be on disk), the phone is told `cancelled`; on the next start the host removes that provisional device from the registry before binding any endpoint. If that rollback also fails, remote access stays off, with no endpoint bound, until storage is repaired. A decision after the offer expired grants nothing. A registry or key file that cannot be parsed (damaged JSON, wrong key length) is reported as damaged (`remote_state_corrupt`), not as a permissions problem: remote access stays off with the reason "Remote access could not start: its saved state is damaged. It stays off until the file is fixed.", no endpoint is bound and the file is not overwritten.
- **Accept-time rejection.** An application RPC connection from an identity not in the registry is refused with `device_rejected` before application data is exchanged. When remote access is enabled, unlisted identities can still use pairing and the constants-only version probe (`cortex-workspace/version/1`). The probe reveals no device or machine data, and neither exchange grants RPC access on its own.
- **Revoke and its limits.** Revoking removes the device from memory and closes all of its connections first, then rewrites the registry. If the write fails, the error is returned and access is not restored: the device cannot reconnect for RPC during this session, and it stays visible to the owner as a disconnected row that grants no access. Repair storage (for example the file permissions) and press Revoke again for the same device: that persists the removal and clears the row. Until a retry succeeds the registry file may still list the device, so restarting first can lose the revocation and the device can regain RPC access after the restart. Requests already handed to the local service are not rolled back or replayed, so an in-flight request of a revoked phone has an unknown outcome; frames buffered or sent after the revoke are not forwarded. Turning remote access off closes every connection and cancels pairing.
- **Version model.** The endpoint serves three ALPNs: `cortex-workspace/pair/1`, `cortex-workspace/rpc/1` and the version probe `cortex-workspace/version/1`. Over rpc the client and service negotiate the workspace wire version (currently 1.11, the generated wire constant): the major must match and the minor is the intersection of the two sides. `service.status` reports the real wire major and minor. The probe is not an authorization: one request byte is answered with one bounded JSON reply of protocol constants only (the supported ALPNs and the wire string, for example `{"supported":[...],"wire":"1.11"}`) and the stream closes. It reveals no machine name, identity-derived data, device list, pairing state or relay address; it holds one of the 32 handshake slots and obeys the 10-second handshake and exchange limits. A client uses it only after a failed pairing or rpc dial: if the probe answers and does not list the ALPN the client asked for, the client reports `remote_incompatible_version`; if the probe fails or still lists the ALPN, the machine is reported unreachable. A pairing link in an unknown format is refused locally as `unsupported_pairing_link_version`, distinct from `invalid_pairing_link`. The phone shows "update needed" for the first, "This pairing link is from another Cortex version" for the second and "no longer paired" (`device_rejected`) for a revoked phone; the desktop bridge shows reinstall guidance for `version_mismatch`, `service_protocol_mismatch`, `bridge_invalid_capabilities` and `unsupported_protocol`.
- **Relay.** There is either no relay or exactly one configured relay, never an n0 relay. Traffic is end-to-end encrypted between the two devices. A relay sees endpoint identities, timing, volume and client IP addresses; it cannot read content, impersonate a device or decide which device may control a machine. Device authorization rests on the computer's registry only. Anyone who can send UDP to the bound port can reach the handshake, the probe and pairing; these are bounded (32 handshakes, 4 pairing connections, 8 connected devices, 2 connections and 16 streams per device).

See [remote access security](docs/cortex-workspace/security.md#remote-access-built-not-qualified) for the allowlist and the honest limits.

### Phone notifications

The Android app raises its own notifications; no push service, push gateway or third-party sender is involved and no token leaves the phone. Built and checked by no-paid automated tests only: background and Doze behavior and a tap on a real phone are not verified.

- **What the machine sends.** A paired phone may call `notifications.subscribe` with exactly `{}` (the remote allowlist refuses any parameter and every other `notifications.*` method). It is read-only: the connection then receives the machine's open owner moments as one `notification_moments` snapshot at once and again after each change. Each moment is its kind (question, stop, access, planning, result, failed, paused, stalled), a 128-bit hex fingerprint (SHA-256 of the moment's internal state, so it reveals no decision or execution id), the task or sprint title (control characters removed, at most 120 characters; an untitled task's first description line) and the closed target the desktop opens (project, task or sprint, decision ids). It never carries a question's text, a stop's headline, a pause reason, a path, output or a credential. A snapshot holds at most 200 moments. The machine computes moments with or without a desktop session bus; without one it still raises nothing locally.
- **What the phone trusts.** Nothing as is. A moment of an unknown kind, a malformed fingerprint or a target outside the five closed shapes (`policy::notification_target`, the same exact shape and id bounds as the desktop shell's check) is dropped; a title is bounded again. The phone keeps the fingerprints it announced (at most 500 per machine) in `notified.json` in its private data, so a reconnect or an app restart announces nothing twice; the first snapshot a machine sends only records what is open.
- **Tap.** A notification's `PendingIntent` is immutable and opens the app's own launcher activity with the machine id, the target and a random key kept in the app's private preferences. The activity accepts a tap only with that key, removes the extras, and the Rust side accepts it only for a paired machine and a target of the closed shapes; the interface checks it again (`parseTarget`) and only navigates. Any app can start the exported launcher activity, but without the key its extras lead nowhere, and a valid target could at most open a place.
- **Foreground service.** While the owner keeps **Phone notifications** on (Settings on the phone; on by default once a machine is paired), a `specialUse` foreground service with one minimum-priority ongoing notification keeps the app's process alive so its existing end-to-end encrypted connections stay open. It is started only from the open app and is not restarted by the system (`START_NOT_STICKY`). The app asks for `POST_NOTIFICATIONS` when the owner turns notifications on; it never requests a battery-optimisation exemption itself and only opens Android's settings list when the owner asks. New permissions: `FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_SPECIAL_USE`, `POST_NOTIFICATIONS`.

## Tool access and containment

The supported Codex adapter versions are 1.12.0, 1.13.0 and 2.0.1. The
2.0.1 private copy retains exact-source checks for role delivery, manual
review, session configuration, per-turn permissions and MCP setup. Its new
typed renderer preserves Cortex's compaction metadata; missing or duplicated
patch sites block launch. This is explicit compatibility support for one
release, not permission to launch arbitrary future adapters. No native account
or production Paseo installation is changed by the compatibility patch.

### User-only stop control for brokered worker commands

Stopping a worker command is available only through explicit user-originated
paths: the bound ACP `cortex_stop_command` configuration input (including the
`/cortex-kill` command), the Paseo worker-panel control, or the owner-local
former `cortex_commands` utility (removed), or the local Workspace execution control. No model-facing MCP stop tool is exposed;
model-originated tool dispatch cannot claim the owner origin. Requests are bound
to an exact brokered gateway command and require a reason. This is not a hostile
same-user-code boundary: native Codex `apply_patch` and OpenCode `execute` actions
are not controlled by this command-stop feature, and local processes outside the
gateway are not claimed to be isolated by it. A request acknowledgement means
only that the durable request was accepted, not that the process has exited.
See [command-stop controls](docs/features/steering-and-questions/index.md#stop-one-worker-command-with-a-reason).

### Native engine tools

No engine process is sandboxed as a whole: Claude Code, codex-acp and Codex App
Server run as the owner with the launch environment's `HOME` and the
connection's native directory, can read what the owner can read, and share the
host network. This is an owner decision (2026-11-25: the engine must read and see
everything the owner can), not a deferred gap: no read or network isolation is
added for engines. What bounds a model's native tools is the engine session's own
mode and tool configuration plus the runtime's answer to each permission request
the engine sends. Model work that changes the project or runs commands is meant
to go through the scoped Cortex MCP tools (`command_run`, `project_write`,
`project_edit`, `project_delete`, `outside_write`, `outside_command`), which
check role, assignment, deny-write paths, deletion consent and protected targets
and record receipts.

**Claude (claude-agent-acp).** Every Claude session Cortex starts (coordinator,
every worker profile, reviewers, sprint planning and short model texts) passes
`session/new` `_meta.claudeCode.options` that the installed adapter merges into
its SDK options: `disallowedTools` removes native `Bash`, `BashOutput`,
`KillShell`, `PowerShell`, `Edit`, `MultiEdit`, `Write`, `NotebookEdit` and
`Skill`, native subagent and scheduling tools (`Agent`, `Task`, `Workflow`, team,
`SendMessage`, `ScheduleWakeup`, `Monitor`, cron and `RemoteTrigger`) and
lifecycle/outward tools (`EnterWorktree`, `ExitWorktree`, `EnterPlanMode`,
`ExitPlanMode`, `TaskStop`, `PushNotification`, `Artifact`, `SendFeedback`) from
the model's context; `allowDangerouslySkipPermissions: false` makes
`bypassPermissions` unavailable; `strictMcpConfig: true` limits MCP servers to
the ones Cortex attaches (its own `cortex` relay and the owner-registered external
servers the runtime attaches to that channel), so MCP servers from the owner's
Claude settings, project `.mcp.json` and plugins do not start; `plugins` and
`skills` are empty; hooks and auto-memory are off (`disableAllHooks`,
`autoMemoryEnabled: false`, in both the session's flag settings and its
SDK-parent policy settings, which an administrator's managed-settings tier can
override). `settingSources: ["project"]` keeps the owner's user-level Claude Code
settings and memory (`~/.claude/settings.json`, `~/.claude/CLAUDE.md`) and their
private per-project overrides (`.claude/settings.local.json`, `CLAUDE.local.md`)
out of every session; the project's shared settings and `CLAUDE.md` still load. The session runs in `default` mode in every access mode. Native
`Read`, `Grep`, `Glob`, `WebFetch` and `WebSearch` remain. Claude Code runs reads
and searches inside the project without asking. Cortex supplies four project-root
`ask` rules: `Read(./.cortex/**)`, `Read(./.cortex-dev/**)`,
`Read(./.cortex-dev-owner.json)` and `Read(./.cortex-workspace-dev.json)`, so a
native read of Cortex private state becomes a permission request the runtime
answers by the decision table (Auto allows, Guarded and Development ask the owner).
These are `Read` rules, not separate `Grep` or `Glob` rules. Credential-name
`Read` rules ask the same way; project allow rules can affect whether a request is
sent. Claude has no native write tool: its writes are the gateway's
`project_write`/`project_edit`, whose protected paths are decided as described under
**Gateway boundaries** below.

**Codex (codex-acp behind the native Codex launcher).** The launcher forces
Codex's `shell_tool` and `view_image` features off, with subagents, plugins, hooks
and apps, and verifies that with `codex features list` before App Server starts;
codex-acp's thread config carries the same switches. Codex then runs commands and
reads images only through Cortex tools, and has no native read tool. Native
`apply_patch` stays on (Codex has no supported switch for it). A read-only channel
runs in codex-acp's `read-only` mode (read-only sandbox, approval on request), so
every `apply_patch` asks and is refused. A writing worker runs in the Cortex
worker permission profile `permission-profile:cortex-workspace` in every access
mode when its App Server offers it (the native Codex launcher defines it for every
session), else in `workspace-write`. The profile is Codex's built-in `:workspace`
profile (the one `workspace-write` maps to: the project writable, `/tmp` writable,
no network) with these project-root paths kept read-only: `.cortex`,
`.cortex-dev`, `.cortex-dev-owner.json`, `.cortex-workspace-dev.json`, `.env`,
`.env.local`, `.env.development`, `.env.production`, `.env.test`, `.netrc`,
`.npmrc`, `.pypirc`, `.htpasswd`, `.git-credentials`, `.kube`, `credentials` and
`credentials.json` (`READ_ONLY_PROJECT_PATHS` in
`crates/workspace-service/src/native_codex_runtime.rs`); `:workspace` itself keeps
`.git`, `.codex` and `.agents` read-only. The launcher also pins
`approval_policy="on-request"` and `approvals_reviewer="user"` as non-persistent
`-c` layers, because codex-acp gives a profile mode the config's effective policy
and the owner's `config.toml` may set `never`, a granular policy without sandbox
approvals or the automatic reviewer. Either way the sandbox's writable root is the
project, with approval
requests (codex-acp's file-change approval with the changed paths, and its
additional-sandbox-permission request with the requested paths) going to Cortex
rather than to Codex's automatic reviewer as in `agent`. `agent` and
`agent-full-access` are never selected. Codex's `unified_exec` feature stays listed
as enabled: in the installed Codex CLI 0.166.0 neither
`-c features.unified_exec=false`, a `config.toml` entry nor `--disable unified_exec`
changes it. With the launcher's flags that CLI's local model-visible tool inventory
declares neither `exec_command` (start) nor `write_stdin` (continue); an ignored,
opt-in offline test with a `shell_tool=true` positive control checks this. It does
not cover a custom launch command, the thread config, a live session or the
timeline, which are unobserved. A native command row in the timeline is display
only, not an authorization or an execution receipt; only Cortex tool calls leave
broker receipts. Details: [Codex shell tool inventory and
`unified_exec`](docs/features/codex-engine/index.md#codex-shell-tool-inventory-and-unified_exec).

**OpenCode** sessions run in the `cortex` mode, whose permissions ask for every
tool (see [OpenCode](#opencode-optional-engine)).

**Decision table.** The runtime answers every native permission request Claude
and Codex send by this table, for every role allowed the operation kind:

| Target | Guarded | Development | Auto |
| --- | --- | --- | --- |
| Read or search inside the project | allow | allow | allow |
| Read or search outside the project | ask the owner | allow, recorded | allow, recorded |
| Write or edit inside the project (writing workers) | allow | allow | allow |
| Write or edit outside the project (writing workers) | ask the owner | ask the owner | allow, recorded |
| Deletion (`delete`/`move` kinds or a declared `rm`) | ask the owner | ask the owner | allow, recorded |
| Credential and key material anywhere, read or write | ask the owner | ask the owner | allow, recorded (`auto_credential`) |
| Git control files, engine and Cortex configuration, write | ask the owner | ask the owner | allow, recorded (`auto_control`) |
| Cortex private state, read or write | ask the owner | ask the owner | allow, recorded (`auto_private`) |
| Live store files (`cortex.sqlite3`, its `-wal`, `-shm`, `-journal`, the store's `.*.lock` files), write | refused (`cortex_live_store`) | refused | refused |
| Web fetch | ask the owner | allow | allow |

Credential and key material is a private file name anywhere (`.env*`, SSH key
names, `*.pem`, `*.key`, `*.p12`, `*.pfx`, keystores, `.netrc`, `.npmrc`,
`.pypirc`, `credentials*`, Terraform state and variables, kubeconfig, names
containing secret, credential, password or token) and the owner's home
credential locations (`~/.ssh`, `~/.gnupg`, `~/.aws`, `~/.azure`,
`~/.config/gcloud`, `~/.docker/config.json`, `~/.kube`, `~/.git-credentials`,
package-manager token files, keyrings and `~/.password-store`, shell and REPL
histories, the Codex and Claude credential files). The name rule
(`private_name` in `crates/workspace-service/src/machine/short_text.rs`) looks at
the file name only, case-insensitively, and matches well-known names exactly,
not anywhere inside a name: `.env` and SSH key names as a prefix (`id_rsa`,
`id_rsa.pub` and `id_rsa_backup` match, `old_id_rsa` and `fake_id_rsa` do not),
`.netrc`, `.npmrc`, `.pypirc`, `.htpasswd`, `credentials` and
`credentials.json` as the whole name, the key and state extensions as a suffix,
and only the four words secret, credential, password and token anywhere in the
name. A key copied to an unusual name is therefore an ordinary file to this
rule; broader substring matching would misclassify ordinary files such as
`rsa_test.py`. Git control files and
engine/Cortex configuration are the runtime's protected control-plane targets.
Cortex private state is the project's `.cortex`, `.cortex-dev` and dev owner
files and the service state directory. It is allowed on request like credentials
and Git controls (owner decision 2026-11-25: when a task needs something there
changed, the change is made once the owner allows it; it is never silent and never
forbidden outright). The one exception is a write to a live store file directly in
a `.cortex` or `.cortex-dev` directory (`live_store_file` in
`crates/workspace-service/src/native_engine_policy.rs`): the runtime holds POSIX
locks on those files and commits every state transition through the database and
its WAL, so a write from another process can corrupt the running store or drop
its locks, and no owner answer can make such a write safe. Reads of them follow
the private-state row.
**Auto is full, unsandboxed host access (owner decision 2026-11-26).** In Auto every
role and stage, the coordinator, consultant, reviewers, explorers, read-only and
worktree assignments included, runs `command_run` and `outside_command` directly on
the host as the owner: the owner's real HOME, PATH, environment, devices, network,
ssh-agent and docker, with no Landlock ruleset, sparse view or snapshot
copy; `sudo`, `pkexec` and `doas` go through the owner-password route. A read-only
role gets no read-only capture and no "writing worker required" refusal, and a
read-only task is never told that SSH or host commands are forbidden. The command's
working directory is its own checkout or worktree. Two guards remain in Auto: a
command whose argv deletes or overwrites a live Cortex store file
(`.cortex/*.sqlite3*`) is refused (`cortex_live_store`), and a deletion (the
recognizer for `rm`, `unlink`, `rmdir`, `shred`, `find -delete`, `git clean`/`rm`,
`rsync --delete`, also through wrappers and `sh -c`, and `project_delete`) always
waits for the owner's answer on a permission card and is never answered by Auto; a
denial is a normal refusal. Guarded and Development keep the behaviour described
below. Managed Codex scoped channels are unchanged.

Shell commands follow the same decision where a command can: in Auto, `command_run`
sees the real `.cortex`/`.cortex-dev` read-write, with every live store file that
exists at launch (`live_store_file`) bound read-only over it, so it cannot be
changed, deleted or renamed; a live file absent at launch is not pre-created, so a
command could create one by name, which the file tools refuse. In Guarded and
Development the directories stay read-only inside a shell command, because a running
command cannot pause for a per-write owner prompt; the failed command's result carries
a `cortex_store` hint to use `project_write`/`project_edit` instead, where the runtime
asks the owner. The access mode selects the view (`Profile::Auto`); the reported
command `profile` stays `development`.
 A path that cannot be resolved asks the owner outside Auto. Read-only roles (the coordinator, the consultant, architect,
ux-designer, explorer, planner, security-auditor, accessibility-auditor,
database-architect, performance-engineer, the runtime-reserved build-verification
profile, the code reviewer and every review-stage session) get only the read rows;
their native write, delete and execute requests are refused in every mode under the
`read_only` rule. Every native request of the consultant is refused. This table
applies to native engine tools only. Cortex gateway commands follow a separate
filesystem policy (see [Attack-surface review and limits](#attack-surface-review-and-limits)).

"Ask the owner" is a card titled `Owner decision: <role> wants to
<read|write|delete|run|fetch> <paths> because <reason>` with only Allow once and
Deny ("Allow for this task" is not offered: the consent model has no task-scoped
grant of a path class). An incoming ACP client (Paseo, Zed) shows it as its
permission card. A native Workspace task has no such client: its request becomes
an owner decision of kind `permission` in the task's decisions projection, kept as
one `pending_questions` row (options Allow once / Deny) and a
`NATIVE_PERMISSION_DECISION_OPENED` event naming the role, operation, exact paths,
reason (`outside_project`, `credential`, `control`, `deletion`,
`unresolvable_path`, `network`) and access mode, without a store format change.
The desktop shows it in the task's Conversation as "Asked by <role>", the task
reads as needing the owner, and the desktop notification for a waiting question
covers it. The engine's request stays pending while the decision is open; neither
claude-agent-acp nor codex-acp puts a time limit on a pending permission request,
and only a cancelled turn ends it. Allow once passes the engine's allow option;
Deny, or any other answer, its own reject option (ACP carries no reason text). A
pause, stop or ended worker session withdraws the decision with its reason and
cancels the engine's request; a decision a stopped service left open is withdrawn
when the service starts again (`service_restarted`), so it is never answered into
the void, and the worker's recovery follows the normal lost-worker rules. Every
answer is a `NATIVE_PERMISSION_DECIDED` (or `NATIVE_PERMISSION_CANCELLED`) event
with its rule and its decider: `owner`, `mode:<access mode>` for an allowance,
`runtime` for a policy refusal, `withdrawn:<reason>`, or `no_owner_client` when the
task has neither an ACP client nor a Workspace binding, in which case the request
is refused. An owner-decided request also leaves a `permission_receipts` row
(`allow_once`, `reject_once` or `cancelled`). An access-mode allowance of a channel-bound MCP tool (rule
`channel_mcp`) is the single event `NATIVE_PERMISSION_AUTO_ALLOWED`, committed in
the same transaction that re-authenticates the channel, matches the bound session
and reads the access mode; owner decisions, refusals and requests for native tools
keep the `NATIVE_PERMISSION_REQUESTED` record and their decision event. The [outside tools](docs/features/permissions-and-containment/index.md#access-mode-development-guarded-and-auto)
have their own target checks and consent decision, separate from the native
engine table.

**Where the table cannot be enforced.** A tool the engine runs without asking has
no runtime decision and no receipt:

| Cell | Claude (`default`) | Codex (worker profile) | Codex (`workspace-write`, fallback) | Codex (`read-only`) |
| --- | --- | --- | --- | --- |
| Read/search inside the project | runs without asking (allowed anyway) | no native read tool | no native read tool | no native read tool |
| Read/search outside the project | asks | no native read tool | no native read tool | no native read tool |
| Credential read inside the project | asks through the `ask` rules | no native read tool | no native read tool | no native read tool |
| Cortex private state read inside the project | asks through the four `ask` rules; `Grep`/`Glob` coverage not established | no native read tool | no native read tool | no native read tool |
| Write inside the project | no native write tool | `apply_patch` writes without asking | `apply_patch` writes without asking | asks; refused |
| Write outside the project | no native write tool | asks | asks | asks; refused |
| Cortex private state write inside the project | no native write tool | asks (observed) | written without asking | asks; refused |
| Git control write inside the project (`.git`, `.codex`, `.agents`) | no native write tool | asks (observed) | asks (observed) | asks; refused |
| Credential write inside the project | no native write tool | asks for the fixed root names above (observed for `.env`); any other credential name is written without asking | written without asking | asks; refused |

Observed without a model call (2026-11-25, Codex CLI 0.166.0, codex-acp 2.6.0, a
disposable project and a temporary `CODEX_HOME`): the real codex-acp and App Server
were driven over ACP stdio against a local mock Responses endpoint that returned
one `apply_patch` call, so no paid model was reached. Under the worker profile a
patch into `.cortex`, `.cortex-dev`, `.cortex-dev-owner.json`, `.env`, `.git` or
outside the project sent `session/request_permission` (kind `edit`, the exact path
as its location); Allow once wrote the file and Reject wrote nothing; an ordinary
project patch was written without a request; in `workspace-write` a patch into
`.cortex` was written without a request. This held with an owner-like
`config.toml` setting `sandbox_mode="danger-full-access"` and a granular approval
policy without sandbox approvals. Codex 0.166 accepts only exact paths (or a
trailing `/**`) as `read` entries; a glob entry accepts only `deny`, which would
also hide the file from reading, contrary to the owner decision above, so a
credential name pattern elsewhere in the project cannot be made read-only.
Also observed: after approval Codex writes the approved file but creates no missing
parent directory, so an allowed patch into a new directory below `.cortex` fails
with "Failed to create parent directories"; the gateway's `project_write` creates
missing parents and is the reliable route there. The ignored test
`real_codex_sandbox_keeps_profile_paths_read_only` repeats the sandbox part with
the bundled binary (`CORTEX_TEST_CODEX_CLI`). A full Cortex session with the real
engine (the launcher, a real task channel and the runtime's answer) was not run;
the runtime's answer to these requests is covered by the fake-engine scenario in
`tests/native_sessions.rs`.
That Claude Code asks before a read outside its working directory, honours the
`Read` ask rules, and that Codex asks for every write outside its writable
root follows from the installed adapters' code and documented behavior (for
Codex also from the probe above); the Claude part has not been observed in a live run.

**Gateway boundaries.** A writing worker's runtime-declared assignment work area
coordinates parallel writers; it is not a gateway write boundary. The gateway
enforces assignment/role authority, user-protected paths, protected-path decisions
and path safety, but does not compare ordinary writes with the
declared work area. A direct `project_read`, `project_write` or `project_edit`
of Cortex private state (`.cortex`, `.cortex-dev`, the dev owner files) and a
`project_write`/`project_edit` of Git control files (`.git`) or of a credential
file (`.env*`, private keys, `.netrc`, `.npmrc`, `.git-credentials`, `credentials`
files and key-store extensions, by name in any directory; `credential_file` in
`crates/workspace-service/src/native_engine_policy.rs`) are decided by the
runtime before the call runs (`admit_protected` in
`crates/workspace-service/src/native_access.rs`): Auto allows, Guarded and
Development ask the owner (Allow once / Deny; without an owner surface the call is
refused with `protected_path_not_confirmed`), and a write to a live store file is
refused in every mode (`cortex_live_store`). Each decision is one
`NATIVE_PROTECTED_PATH_DECIDED` event with the operation, path, rule, mode and
decider; the admission covers exactly that call. A `.git` or credential-file read
needs no gateway decision. Plan scopes, `scope_request` and `access_request` may
name these paths (owner decision 2026-11-25): a scope or reservation grants no
write there; each write is still decided as above.
Inside `read_batch` a protected path keeps its `private_or_invalid_path` refusal so
a batch never waits for the owner; `project_mkdir`, `project_delete` and the
skill-link tool still refuse protected paths. The same on-request rule applies to
another project's Cortex state reached through `outside_write`/`outside_command`,
except a write to its live store files. An `access_request` reserves additional work for
coordination. A directory path with one trailing slash (`.next/`) is that
directory, `[`/`]` are literal path characters, and an absolute or `..` path is
refused as `outside_project` with guidance; generated build/test output needs no
request. `command_run` defaults to a 30-minute limit; on a timeout or stop the
command and every process it started are killed, including children in their own
process group, and commands never inherit the MCP server's stdin. Writer
conflicts receive bounded delayed runtime rechecks (1/2/4 seconds), then a
runtime grant or denial without a user question. The `retry_at` and attempt index
are durable, and a live waiting worker rechecks them in its own session even when
no other node is runnable. Live-store, unsafe, deny-write and
read-only refusals are immediate runtime decisions. Retained old access cards are
cancelled; late user answers cannot grant access. Retry scheduling is retained
runtime state, not a synthesized user answer; authority loss fences the wait. The
advertised worker question tool is reserved for genuine user decisions, not
missing project access. Its question and options must use the selected chat
language while internal worker communication stays English; these are generation
instructions, not runtime translation or semantic enforcement of arbitrary prose.
A question recommendation is display metadata: it neither selects an option
nor constitutes an owner answer, permission grant or skill approval.
See [the access flow](docs/features/orchestration/index.md#worker-questions-and-additional-access).
The coordinator, reviewers and read-only roles get no Cortex project write tool.

A revoked or expired channel terminates the process. The one bounded exception is
a worker channel revoked by acceptance of that attempt's own current result: the
watchdog lets the in-flight turn end for at most 20 seconds so its final token
report can arrive, and the post-result settlement normally tears the engine down
when the turn ends, within 15. Native tools can still act in that window; such
late project changes invalidate the digest-bound review, verification or
completion gate as above. Explicit cancellation, stop and replanning terminate at
once. Independently of the engine's transport monitor, an active worker prompt
checks once a second, on the store's writer lane, whether its channel was revoked
or its owner epoch replaced; a turn still running 10 seconds after that is sent ACP `session/cancel` and, when the
engine has not replied within 25 seconds, ends the turn as outcome-unknown so
the session's ordinary shutdown terminates the process and releases its
capacity. This only stops work whose authority was already withdrawn; coordinator
sessions are not affected.

Unverified without a live run: that a real Claude turn lists no native `Bash`,
`Edit` or `Write` tool, that an attempted native edit is impossible, that the
attached MCP servers stay reachable under `strictMcpConfig`, that Claude Code asks
before a read outside the project, and which reads it performs on its own outside
the project (for example its own session files).

### OpenCode (optional engine)

The current Rust launch paths are described in
[Attack-surface review and limits](#attack-surface-review-and-limits) and
[OpenCode launch boundaries](docs/features/opencode-engine/index.md#current-rust-launch-boundaries).

The following bullets describe the **former Python engine jail**, not the
installed Rust task or short-text engine. They do not establish current
containment or account behavior.

- **Process isolation.** Each `opencode acp` starts a loopback control server
  whose password is in its environment. The OpenCode jail has its own user,
  PID and IPC namespaces. Any other bubblewrap sandbox is in a sibling user
  namespace, which the kernel denies `/proc/<pid>/environ` of the OpenCode
  processes. A setuid `bwrap` would not create those namespaces and is refused.
  Processes in the parent user namespace (the Cortex runtime, the owner's own
  shell) can still read it. Cortex never reads or uses the password.
- **Configuration is Cortex's.** The owner's `~/.config/opencode` is hidden
  behind an empty private tmpfs, project `opencode.json`/`.opencode/` is
  disabled (`OPENCODE_DISABLE_PROJECT_CONFIG=1`), and auto-update is off.
  Cortex supplies one `cortex` agent whose rules ask for every native tool
  call and deny subagents, questions and skills. Its `plugin` list only removes
  OpenCode's built-in `opencode.agent` and `opencode.plan` plugins (the source
  of `build`, `plan` and the helper agents); it loads no plugin. It names no model, so a
  session runs only on a model Cortex confirmed; OpenCode's own default is a
  free third-party model. Prompt-free refresh and session catalog capture do not
  save observations missing the required `cortex` mode; the previous snapshot is
  retained. Refresh reports `opencode_cortex_mode_not_advertised`.
- **Owner provider definitions.** From the hidden `~/.config/opencode`
  (`config.json`, `opencode.json`, `opencode.jsonc`) Cortex carries exactly the
  `provider` definitions and the `enabled_providers`/`disabled_providers`
  lists into its own configuration, because a custom provider (for example an
  OpenAI-compatible endpoint) exists only there. No agent, permission, MCP
  server, plugin, command or instruction is taken, and a provider that would
  load a package outside `@ai-sdk/` is left out. Recognized credential values (any
  API-key, token, secret, password, authorization or cookie field, and every
  `headers` value), and every expanded `{env:…}` or `{file:…}` string regardless
  of its field name, never enter the process environment: each is mounted
  read-only from memory into the jail, like the role prompt, and named with
  `{file:<path>}`. Inside that jail it is readable, the same trust class as
  the owner's own OpenCode logins. An `{env:NAME}`
  reference is resolved from the Cortex runtime's environment for exactly the
  named variable, and a `{file:path}` reference from the owner's home. The
  engine observations record provider names and skip reasons only. For arbitrary
  provider-specific credential fields, use substitutions rather than literal
  values: unknown literal fields cannot be classified as credentials.
- **Owner logins.** OpenCode itself opens the owner's native `opencode.db`
  in the existing data directory. Cortex does not read its credential rows,
  extract API keys, copy logins or select an alternate database. `OPENCODE_DB`
  overrides are not accepted. Native session state and logins share that
  database, as in the owner's own OpenCode. Because no built-in primary agent
  exists, the installed 2.0.20 ACP catalog waits for the configured `cortex`
  agent instead of snapshotting `build`/`plan` first. A catalog without `cortex`
  is still rejected; Cortex never falls back to `build` or `plan`.
- **Role prompt.** The prompt is mounted read-only into the jail from memory;
  the host holds only an empty mount point.
- **State.** The owner's `~/.local/share/opencode` is referenced in place,
  never copied, and must be owner-private. It stays writable in the jail for
  the native database, logs and tool output. The `bin` directory of downloaded
  helpers is read-only. Each process gets a private disposable `XDG_STATE_HOME`.
- **Code Mode.** Session MCP servers, the Cortex gateway included, are reached
  only through OpenCode's `execute` (Code Mode) tool. One `execute` call is one
  permission request, and the calls inside it are not asked individually. Every
  Cortex MCP call is still checked by the channel-bound gateway. Code Mode can
  call no shell or file tool, but it has `fetch` over the shared host network,
  like the engine's shell.
- **Refusals.** A declared shell `rm` asks the owner in Guarded mode, as for
  Codex. A refused permission ends OpenCode's whole turn as `cancelled`; Cortex
  treats its own refusal as a refusal and continues that session (at most three
  times per prompt), never a user cancellation.

The compatibility gate is deliberately major-version-wide: accepting a new 2.x
release does **not** mean its behavior was independently qualified. The
version-independent binary trust, jail and permission boundaries above remain
mandatory; model, effort and mode choices still require ACP confirmation.

`~/.cache` stays writable to every jailed process, as it is for other tools.
Whether OpenCode later loads executable content from its cache there was not
examined. No paid or live OpenCode run has been made.

Outside the jail, entirely unjailed, `connections.quota` lazily runs the
installed Claude Code CLI's own local `/usage` command (`claude -p /usage
--output-format json --restricted --strict-mcp-config`) to learn a
claude-agent-acp connection's plan-usage percentage. Legacy Claude `five_hour`
events, cached windows and CLI session lines do not contribute to quota decisions
or current window display; remaining reported windows still apply, and missing usage
stays unavailable rather than inferred hot. Codex windows and actual engine-refusal
handling are unchanged. The subprocess
environment comes from `connections.environment.build_process_env`, which
re-verifies the connection against the on-disk registry before trusting its
`credential_ref` and sets `CLAUDE_CONFIG_DIR` to that connection's own
credential directory (never the ambient one); `TZ=UTC` is then applied on
top. It never starts a model turn, and its result is only ever trusted when
the parsed JSON itself confirms `is_error: false`, `num_turns: 0` and
`total_cost_usd: 0` (see `docs/features/token-and-plan-usage/index.md`). This is the same
already-established pattern `connections.account_identity` uses for `claude
auth status` (a per-connection isolated CLI call, bounded by a short timeout,
that never raises and never logs a credential) -- both call
`build_process_env` rather than reading `credential_ref` directly.

Every supplied MCP endpoint is a runtime broker. Cortex operations check the
current task, assignment, channel and owner epoch. External servers require an
administrator-owned exact tool allowlist; their process gets only runtime-granted
scope mounts, explicit environment and optional explicit network/read grants. An
explicit network grant also exposes only the public system resolver/hosts/NSS
configuration and CA bundle as read-only files; without it those files remain
unmounted. The full `/etc` tree and user home are never added by that grant. A
read grant cannot overlay the project or unmask its private store. Native account
MCP entries are disabled. Server-initiated sampling/elicitation is refused.

Cortex gateway file writes check assignment and role authority (writing workers hold the whole-project grant), use no-follow descriptors, require complete-read or expected-content hashes, and replace files atomically. Commands run under
Landlock: the project is read-only by default and launch-time grants make the
whole project writable for a writing worker; that grant is broader than the
worker's runtime-declared assignment scope. Additional access requests update
only the declared-scope writer reservations after runtime validation, not the
Landlock ruleset. Private `.cortex` paths are masked (in Auto the store directories are writable except the live store files), the project's `.codex` and
`.claude` are ordinary project paths, and the project's `.git` is visible as described below. The restrictive command profile runs as the owner: their user and
group names resolve, their real HOME is mounted read-only (so its dotfiles and
credentials are readable by the command), the runtime's environment is passed on
except `CORTEX_*` and credential-like variables (token, secret, password, API key,
any name ending in `KEY`/`KEYS` or containing `_KEY_` other than Git's `GIT_CONFIG_*`
command-line configuration, auth, cookie, session; a commit author's `GIT_AUTHOR_NAME`/`_EMAIL`/`_DATE` are
identity and pass), and the host network, devices, supplementary groups and
session sockets remain visible. Read-only mounts
alone do not prevent mutating Docker socket requests; there is no implicit privilege escalation. External MCP
servers get no HOME, network or inherited environment unless explicitly granted;
automatic verification runs with the same owner environment and the command
profile of the task's access mode. Identified external hardlinks in the task tree
are mounted read-only, except Development/Auto writer commands first replace
granted aliases with independent copies; this is not a host-wide alias guarantee. Draft writes require the exact owned unpublished draft ID.
File actions and command starts recheck authority at the transaction boundary; running
commands and external servers are terminated on observed revocation.
Missing continuation-session metadata on a recovered channel does not revoke its
ordinary tool authority or bypass epoch, assignment, role or wait checks.
Questions without that binding use task-global recovery, never a fabricated
same-session wait; access continuation still requires the exact retained session. This does not
claim protection against a separate hostile process running as the same OS user and
racing filesystem changes.

### Destructive local retention maintenance

**Automatic Workspace retention.** The Workspace service archives Done tasks after `autoArchiveDays` (default 7) and, with `automaticArchivePurge` **on by default**, deletes archives older than `retentionDays` (default 30, from archive time; a Done task newly archived by sprint closure counts from its completion time plus `autoArchiveDays`, and an unknown completion time permits no age deletion) on project activation and then hourly. Deleted: eligible archived task records and their task-owned journal, report, evidence, output and snapshot data, a clean Workspace-created task worktree (non-forcing), and attachment files unreferenced for `unreferencedAttachmentRetentionDays` (default 1); the attachment sweep also honors answer references held by removed or detached projects and deletes nothing if any retained project's references cannot be read. Never touched: active, paused, blocked, awaiting-decision or unclosed tasks; tasks without a confirmed closure or without a verified linked execution in a terminal runtime state; dirty or unmerged worktrees (unmerged needs the existing exact discard authorization); branches and the root checkout; migration backups; qualification evidence; loaded sessions and busy stores. Eligibility, age and identity are rechecked at the deletion fence, the scheduled pass never forces, and the owner can switch background deletion off in Storage. The timer starts no task and calls no model. This does not change the ACP chat command below, which stays preview-only.

`/cortex-clear` is an explicit owner ACP chat command, not a model tool.
The runtime previews counts/cutoff unless the owner supplies trailing `confirm`;
confirmation authorizes deletion of matching tasks/artifacts only in the current
project's `.cortex` store and unambiguously task-linked drafts. The command
starts no coordinator/model turn. It never returns task contents or reports.
`/clear` is not registered because hosts reserve that name.

Chat retention normally protects tasks bound to this agent's sessions, every
non-terminal runtime task and unreleased/ambiguous capacity or unterminated
process records. Explicit `force` before `confirm` bypasses those protections
except the invoking session, its bound task and kept threads. Force assumes
stale processes are already dead: it releases reservation rows and marks process
records terminated with an owner-force assumption, but never kills OS processes.
Do not force-delete genuinely running work. Deleted sessions are evicted from
the agent's cache, including after partial committed teardown.
Selection and cleanup hold a nonblocking exclusive store access lock: competing
store access and pending intents conflicting with remaining keeps fail closed. In-flight
chat prompts cannot interpret this command as an answer or model work. Journaled
`pending_deletions` still permit recovery of committed deletions after partial
filesystem failure; an error does not claim successful cleanup or rollback of
already removed files. Dry runs do not replay intents. Preview counts include
already committed pending deletions; relative days recalculate on confirmation.

Old unbound host sessions (or sessions whose task is absent) are also removed,
with their `host_transcript`/`host_updates` and workspace bindings, only when
their creation/close/latest transcript/update activity is before the cutoff.
The invoking session is always kept; other loaded sessions are kept unless forced.
After deletion commits, SQLite `VACUUM` runs outside the transaction under the
same lock. Counts and DB byte sizes report reclamation; a vacuum failure does
not undo deletion and is reported as committed deletion with failed reclamation.

The owner-run `cortex_clear` CLI alternative was removed on 2026-11-16 without replacement, and the native host refuses a confirmed chat deletion for now.
Models/coordinators/workers must never run a deletion route on behalf of
the owner. See [retention maintenance](docs/project/packaging.md#retention-maintenance).

<a id="access-mode-guarded-and-auto"></a>
### Access mode (Development, Guarded and Auto)

Fresh ACP sessions durably start **Development**. **Auto** runs every role's
command on the host as the owner (see above). In **Development** and **Guarded**,
read-only roles and automatic verification, a brokered command runs on the host at
its real paths, with the owner's tools, PATH, HOME, devices, groups and shared
network, under a Landlock ruleset; see [Landlock command boundary](#landlock-command-boundary).
Docker/Compose, SDK/cache state and authorized outside-project cwd need no
routine permission card merely for crossing the project boundary. Automatic
verification uses the read-only profile with write grants for eligible ignored
directories, such as build outputs and dependency directories; other checkout
writes are refused. The verifier keeps the profile's private scratch and never
retries on the host. A check whose project digest changes is incomplete. A mode
switch does not stop a read-only command or a verification check. Only a writing
worker's command is stopped by a mode switch or a denial change. Engine processes
themselves are not sandboxed (see [Native engine tools](#native-engine-tools)).

<a id="landlock-command-boundary"></a>

#### Landlock command boundary

The per-command supervisor prepares a Landlock ruleset (`native_sandbox`) and the
payload enters it between fork and exec; no mount, PID, user or network namespace,
overlay or snapshot copy is created, and a ruleset that cannot be prepared fails
closed (`native_command_sandbox_unavailable`). Per profile:

| Profile | Writes | Unreadable |
|---|---|---|
| Read-only roles, verification | private scratch (TMPDIR), verification's ignored-directory grants, package/build caches, `/dev` | owner keys and credential stores, project secret files (`.env*`, `.npmrc` ...), live Cortex stores |
| Guarded writer | its checkout (or granted paths), scratch, the owner's non-sensitive HOME tool directories, `/dev` | the same set, except inside the writer's checkout |
| Development writer | as Guarded plus `/tmp` and `/var/tmp` | the live Cortex stores outside the writer's checkout |

Engine and Cortex configuration, the service executable and owner write denials
stay read-only. The owner's keys and credential stores are SSH private keys (not
`config`, `known_hosts` or `*.pub`), GnuPG private keys, cloud, Kubernetes and
Docker credentials, password stores and keyrings, `.netrc`, `.git-credentials`,
`.npmrc`, `.pypirc`, Cargo and GitHub CLI tokens, the engines' sign-in files,
Cortex and Paseo state, and browser profiles.

Landlock is allow-list only. A writer's checkout is an open write tree: an entry
inside it (`.cortex`, `.env`) cannot be hidden without making every file created
directly in the checkout unusable, so there the live store files are protected by
the argv seal (`cortex_live_store`) and deletion consent instead. An explicit
owner write denial inside the checkout is enforced; a directory that directly holds
it then keeps only listing and creation rights, so a file a command creates there
becomes usable from the next command on. Entry names below hidden directories stay
listable. Landlock does not restrict chmod/chown or timestamps, already open
descriptors, Unix-socket connections (the owner's ssh-agent and session bus stay
reachable) or the network. It is a write and secret boundary for trusted local
work, not hostile-code isolation.

Preparing a command never walks the project. Project secret files are looked up
only where they can be hidden (read-only roles, and Guarded commands without a
whole-checkout grant), from the service's per-checkout index: the last finished
walk plus a fresh read of the checkout's top level, with the walk refreshed in the
background. Only a checkout's first such command in a service process waits for a
walk. A secret file created below the top level after the last finished walk stays
readable to the next such command and is hidden from the one after it.

Devices and hardware are never a blocker for any command (Auto, Development,
Guarded and read-only roles): commands use the host `/dev` and `/sys` at their
real paths, and the owner's X11, Wayland, PipeWire/Pulse and session-bus sockets with
`DISPLAY`, `WAYLAND_DISPLAY`, `XAUTHORITY`, `XDG_RUNTIME_DIR`,
`DBUS_SESSION_BUS_ADDRESS`, `NVIDIA_*`, `CUDA_*` and driver variables. These
names are exempt from the credential-name filter; a device node grants hardware
access, never file writes. Every command shares the host network. Host reads are never a blocker for a linked-worktree task: the read-only worktree profile also
reads the main checkout (its `.cortex` and `.cortex-dev` stores hidden), `project_read`
accepts an absolute host path for every role (plain read-only paging, no write coverage, live store files
refused), and a writing worker's `outside_command` runs as in the main checkout under the same mode
consent; only `outside_write` and privilege escalation are refused for a linked checkout. File-write rules are
unchanged: read-only views stay read-only for files. The sparse views do not
mount the Docker socket.

This is **trusted local development, not hostile-code isolation**. Direct masks
hide the bound task's private runtime storage and protect Git configuration,
user-denied project paths, known existing Cortex configuration/runtime/engine
locations and the production Paseo home/profile. A task worktree beneath a
protected parent keeps its own grant. These mounts do not make arbitrary owner
code, alternate host aliases, hardlinks outside the tree, process access or Docker
unable to circumvent them. The daemon resolves bind paths in its own namespace;
rootful Docker may have host-root authority, rootless Docker still has owner
authority, and remote contexts have their own authority. No endpoint switching,
group change, daemon installation or credential copying is implicit; root comes
only from `sudo` on the host with the password the owner types
([askpass](#owner-password-for-sudo-askpass-and-host-rerouting)).

Capability is not authorization to delete data, modify production/shared systems,
change credentials, upload/push/deploy or run paid checks. Those effects still
need explicit task authorization. Declared `rm` commands use gateway consent
(`command_delete`), including recognized argv wrappers (`env`, `sudo`, `xargs`,
`exec`) and explicit `sh`/`bash`/`dash`/`zsh` command scripts with leading whitespace.
Recognized shell options with operands (for example `bash -o pipefail -c`) do
not hide the script. Supported wrappers inside shell command positions are
classified recursively, including `sh -c 'env dash -c "rm file"'`.
Simple unquoted assignment prefixes and command-position control words in
conditionals, loops, negation and groups are recognized before argv dispatch:
`sh -c 'LABEL=fixture rm fixture'` and
`sh -c 'if true; then env rm fixture; fi'` both require consent.
Quoted/escaped prefix-like words and ordinary argument positions do not acquire
shell-prefix meaning.
Literal non-shell argv arguments containing separators and `rm` text do not
request deletion consent. Refused, absent or malformed approval starts no process.
This lexical classifier
is not a semantic firewall: interpreters, scripts and daemon operations can hide
effects. `project_delete` consent remains. Both `outside_command` and `outside_write` are available to eligible writing
workers in the main checkout in all three modes. Guarded and Development ask
the owner for the specific action; Auto decides eligible requests automatically.
A missing or refused approval blocks the effect. Coordinators and read-only
roles (including reviewers and build-verification workers) do not receive these
routes. Workspace-linked writers can see the tools, but dispatch refuses them.

Cortex private targets remain refused. Outside writes to credential or
control-plane targets are not categorically blocked: eligible requests follow
the mode's decision. An outside command runs with the owner's host access, not
inside the Cortex command sandbox. Checking its working directory does not
contain the payload.
The access mode never changes an engine session's own mode.

**Guarded** remains the explicit restrictive command profile (host/HOME mostly
read-only, writable project and tool caches) with consent for every outside
write and outside command. **Auto** uses the Development host-writable command
profile for writing workers and runtime auto-approval of every consent card and
of every native permission request except a write to a live store file,
each recorded with its rule (see the [decision table](#native-engine-tools)); the
engine session mode stays the same as in the other modes, and read-only roles and
reviewers stay read-only. Development allows native reads outside the project and
asks the owner for native writes outside it, credential and key material, Git
control and engine/Cortex configuration writes and deletions; Guarded also asks
before native reads outside the project.
None of these modes lifts channel/role/epoch/revocation, owner denial, review,
completion, native-subagent or paid-probe gates.

Only ACP client controls select a mode; no model-facing setter exists.
`acp_session_access_defaults` is additive format-12 metadata, written with fresh
session creation; it never backfills old sessions or rewrites user settings.
An explicit pre-task client choice is durable and wins over the initial default.
Binding initializes one task event transactionally and idempotently. Load/reconnect
uses durable state. Existing sessions/tasks without initialization, unknown or
unreadable values and non-ACP task creation remain Guarded. Existing explicit
Guarded/Auto selections remain unchanged.

Development starts revalidate policy and authority; observed policy revision/denial
changes or revocation terminate the owned command group with incomplete evidence.
Desktop build snapshots resolve workspace members from the root Cargo manifest,
refuse outside-project member paths and linked member manifests, and keep the
hidden-file, symlink and dependency/build-cache exclusions. The container still
receives a source snapshot, not the owner’s checkout or home. This is source
selection, not hostile-code isolation.

Switching modes does not remount a process or undo outside writes, detached
children, Docker builds, containers or volumes. Receipts retain cwd, profile and
policy provenance; another cwd cannot close a failed check, nor can a profile change within one operation.
An approved `outside_command` rerun may close a `command_run` failure (and vice versa)
only at identical argv/cwd, despite their necessarily different profiles. Neither
no-paid fake-launch tests nor startup smoke establishes real containment or GUI
behavior. Paid CLI/desktop qualification still requires the owner's explicit
mode-specific request.

### Owner's ssh-agent

Every engine process and every worker `command_run` receives the owner's
`SSH_AUTH_SOCK` when it names a socket the owner owns
(`orchestration.ssh_askpass.host_agent_env`). The value comes from the
environment Cortex was started with. The rest of the credential filter still
applies. The owner's `~/.ssh/config` and `known_hosts` stay readable at their
real paths (private keys do not), so ssh inside the sandbox behaves as in the
owner's terminal with keys from the agent.
The agent never releases private keys, but any sandboxed process, and thus the
model, can ask it to sign. That means it can authenticate to every host the
unlocked keys open, within the task's network access. Lock or remove keys from
the agent (`ssh-add -D`, or a keyring lock) to withhold that. An agent
configured to confirm each use (`ssh-add -c`) still asks the owner on the
desktop.

Runtime verification uses the same host-visible read-only command profile with
the host network shared, not a separate SSH input policy. An eligible explicit host
command is not subject to the broker's Landlock ruleset.

### Optional ssh password dialog

This is off by default. With {"enabled": true} in
`$XDG_CONFIG_HOME/cortex/ssh-askpass.json`, an engine process started after that
point and worker commands on the existing host route additionally receive the owner's
`DISPLAY`, `WAYLAND_DISPLAY`, `XAUTHORITY`, `XDG_RUNTIME_DIR` and
`DBUS_SESSION_BUS_ADDRESS` (each checked for shape), plus `SSH_ASKPASS` and
`SSH_ASKPASS_REQUIRE=force`. ssh then shows its prompt in a `zenity` or
`kdialog` dialog, and the typed answer is written to ssh only. It never reaches
the model, a transcript, a log or the store. Cortex generates the dialog script
in the command's private scratch. The switch
lives in the owner's config directory, which the sandbox cannot write. Git also
uses `SSH_ASKPASS` for HTTPS credential prompts. Any process in the sandbox can
also draw windows on the owner's display, including a fake of this prompt, so
type a password only for a connection you expect. When no display session or
dialog program is available, nothing is added. `sudo` never works inside the
command sandbox: Landlock requires no_new_privs, which setuid programs refuse. A
writing worker's command that needs root runs on the host instead (next section).

Each supervised command also runs in its own transient systemd user scope (`systemd-run --user --scope`, bound to the service unit), a sibling of the service cgroup, with `MemoryHigh` at 60% and `MemoryMax` at 75% of RAM and `OOMPolicy=continue`. A runaway command is throttled or OOM-killed alone and cannot stall the service; the command settles as a normal failed command (`failure: out_of_memory`, with an "out of memory (N GB)" line in its output) and the task continues. The Landlock ruleset is unchanged. Without a usable `systemd-run` (no user bus) commands launch directly in the service cgroup as before.

<a id="owner-password-for-sudo-askpass-and-host-rerouting"></a>

### Owner password for sudo (askpass) and outside commands

An eligible writing worker's `command_run` goes through the `outside_command`
route before any sandboxed run if it requests recognized privilege escalation
(`sudo`, `pkexec` or `doas`). If the supervisor proves the sandbox failed before
launching the payload, the runtime can route it to the host once under a derived
request key. Started or ambiguous runs are not replayed; nested-sandbox or
privilege refusal text is only advisory. Read-only workers are never rerouted.
Direct and rerouted host commands use the same eligibility and private-target
checks: Auto decides automatically; Guarded and Development ask the owner.

**Askpass.** A host command gets one private askpass route: a 0700 directory in
`$XDG_RUNTIME_DIR` (or the temporary directory) with a Unix socket, an `askpass`
script and a `sudo` shim placed first on `PATH`, plus `SUDO_ASKPASS` and a
per-command random token in the command's environment. The shim runs the next
`sudo` on `PATH` with `-A` first, unless the arguments already choose `-n`, `-S`,
`-A` or only ask for help, the version or `-K`; an argv whose first word is `sudo`
or `…/sudo` gets the same `-A` (the receipt keeps the original argv). When sudo
needs the password it runs the helper (this service binary). The service accepts
the helper only if the connecting process is the same uid, is the very binary the
script names (same device and inode), is not traced, and its parent runs with
effective uid 0 for this user with the name `sudo`; the helper then makes itself
undumpable and refuses a stdout pipe that another readable process of the owner
holds, so the answer reaches only sudo's own pipe. Then the service opens the
prompt "Password for sudo", naming sudo's command line, cwd, project, task and
worker, on the desktop and on paired phones (`askpass.list`/`askpass.answer`).
Only the waiting command waits; the task's other work goes on.

The password is sent once in `askpass.answer`, moved into the waiting helper and
written to sudo's pipe. It is never written to the store, an event, a receipt, a
log, command output or a notification and never reaches a model. The service
overwrites its copy and the helper's with zeros after use; transient copies in the
socket transport buffers are freed, not zeroed. A prompt ends when it is answered,
cancelled, after 10 minutes, when its helper goes away or when its command ends;
then the helper exits non-zero and sudo fails as it would without a password. The
receipt's `sudo_prompts` lists how each prompt ended (`answered`, `cancelled`,
`expired`, `withdrawn`, `refused:<reason>`), never an answer. `sudo -n` never asks,
and sudo's own credential cache applies after a correct password. `pkexec` uses
the desktop's polkit agent on the machine's own screen, not this prompt.

**Phone.** A paired phone may list prompts and send a cancel; it may send a
password only while Settings, Remote access, "Passwords and tokens from phones"
(formerly "Sudo passwords from phones"; the stored setting is unchanged) is on
(the default, by owner decision). The password then crosses the paired,
end-to-end encrypted remote channel and, if one is used, the relay as ciphertext.
The setting is kept in `askpass.json` (0600) in the service's state directory and
changed only by the owner-only `remote.sudo.set`, which the remote policy refuses
to a phone like every `remote.*` method.

**Limits.** This is a same-user boundary. Code already running as the owner with
the host command's environment can start its own `sudo -A` (and so a genuine
prompt naming that command), and hostile same-user code can still observe input
through other means (for example an X11 keylogger, or a reader it hides from the
pipe check by making itself undumpable). Type the password only for a command you
expect. Checked by no-paid tests only, with a fake `sudo`: no real sudo, password or
device was used.

### Owner actions: sign-ins and pasted tokens

When the runtime records `OWNER_ACTION_REQUIRED` for a task (a GitHub, Codex or
Claude login is missing, or `sudo` needs the owner), the machine service lists it
(`owner_actions.list`) as a card on the desktop and on paired phones, and resolves it
only by recording `OWNER_ACTION_RESOLVED` (`how`: `terminal_login`, `token`,
`recheck` or `dismissed`), on which the runtime retries the blocked work. An action
resolves on evidence only: the service reruns the one preflight probe for that
service (`gh auth status`, `codex login status`, `claude auth status`, `sudo -n
true`; exit codes only, output discarded, never creating a native engine home) when
the owner asks and every 60 seconds while the action is open, and a `sudo` action
also resolves when a "Password for sudo" prompt of the same task is answered. The
owner can always dismiss a card; the runtime then retries anyway. The next
recheck time is stored in the task's event log, so a restart does not reset it,
and a card is closed (`by: task_terminal`) when its task is cancelled or
finishes. That closure grants no access and counts as no successful probe. For a
task still running, the service also raises a desktop notification for an open
card while no window is open.

**Terminal sign-in.** "Open sign-in in terminal" types the installed CLI's own
interactive login (`gh auth login`, `codex login`, `claude auth login`) into the
desktop's project terminal, in the project folder. The command comes from the
service, never from the model or the event. A phone never sends this action (both
allowlists refuse it); it shows the command to run on the computer.

**Pasted token.** Only a service whose CLI documents a non-interactive login reading
stdin offers a token field: GitHub (`gh auth login --with-token`). Codex's
`codex login --with-api-key` would replace the owner's native subscription login with
API-key billing, and Claude has no stdin login, so neither offers one. The token is
sent once in `owner_actions.act` and handled like a sudo password: the request's
string is moved into a buffer that is overwritten with zeros after use, written once
with a newline to the CLI's stdin (spawned with a cleared environment, the owner's
tool search path and `GH_PROMPT_DISABLED=1`), and the pipe is closed. The CLI's
stdout and stderr are discarded and only its exit code is used. The token is never
written to the store, an event, a receipt, a reply, a log, a notification or command
output, and never reaches a model; the client allowlist passes it through without
keeping it. Transient copies in socket and JSON transport buffers are freed, not
zeroed. `gh` itself stores the accepted token in its own credential store, as its
interactive login would. A paired phone may send a token only while "Passwords and
tokens from phones" is on; with it off the remote policy refuses the request before
it reaches the service. This is the same same-user boundary as the sudo prompt.
Checked by no-paid tests only, with a fake `gh` and a fake token: no real account,
token or CLI login was used.


Automatic verification has the same private-path isolation and a completely
read-only project except its scratch (`HOME`/`TMPDIR`/`TMP`/`TEMP`) and private
`/var/tmp`. Sandbox absence is a hard
failure. Complete command bytes, exit status, timeout/cancellation and project
digest are retained. A successful MCP envelope alone cannot certify a check: a
passed worker check requires its own complete successful gateway-command receipt of
the same attempt, with complete retained output and a nontrivial command; an
engine-native call or a missing, foreign or insufficient reference keeps it
`unverified` (see [check evidence](docs/features/orchestration/index.md#check-evidence)). A
recorded failure is never dropped or downgraded; the runtime treats it as closed
only after re-deriving, from its own receipts, an exact later passing rerun of the
same command in the same attempt
(see [check history](docs/features/orchestration/index.md#check-history-and-current-outcome)).

### Git and tool state in the project

With a whole-project grant, git run by a writing worker through `command_run` (no Cortex engine session
has a native shell) may update the project's own repository: the `.git`
directory, or a linked worktree's own `worktrees/<name>/` plus the shared common
directory even when that lies outside the project. Its data (objects, refs,
logs, index, HEAD, packed-refs, locks, merge/rebase/stash state, `info/`) is
writable. What configures or redirects git runs the owner makes later, outside
any sandbox, stays read-only in each of those directories: `config`,
`config.worktree`, `hooks/`, `commondir`, a worktree's `gitdir`, legacy
`remotes/` and `branches/`, plus other worktrees' metadata and a
`core.hooksPath` directory inside the project. Development/Auto writing-worker
commands additionally protect `info/` (attributes, excludes, sparse-checkout)
and allow data writes in existing submodule repositories beneath read-only
`modules/` containers, with the same configuration/hook/pointer protections.
Engine/Guarded commands keep submodule metadata read-only. Standalone submodules
remain unsupported as logical project roots; use the superproject.
Existing protected entries are mounted read-only. GitGuard attempts to remove
watched entries that were absent at launch every 500 ms and at settlement. On
removal, the supervisor adds a generic output notice and preserves the payload's
status. The sweep is best-effort, not atomic, and stops with the command.
Residual risk: a process that deliberately detaches from its `command_run` and
outlives it is no longer swept, and could still plant one of those absent entries
-- the same class as the racing same-user process this document already excludes.
A writing worker can also leave an interactive rebase or cherry-pick in progress;
git shows that state and runs its todo list only when the owner continues it.
Reviewers, read-only roles and automatic verification see `.git` read-only. `project_read`/`project_write`/`project_edit`/`project_delete` still refuse every
path through `.git`, `.cortex` or `.cortex-dev`.

Main-checkout `project_git` runs fixed read-only status, diff and history commands (log, show, cat-file, blame, ls-files) with a cleared
environment, global/system Git configuration disabled, optional locks off,
hooks and fsmonitor disabled, and no external diff or text conversion.
It probes repository filter keys and disables discovered drivers rather than
running LFS filters against scratch. Its output does not prove an LFS-aware diff.
For ordinary live commands without a whole-repository writer grant, Git metadata
stays read-only and `lfs.storage` defaults to disposable scratch. Whole-project
writer commands keep repository storage; configuration/hook overlays follow the
data grants. Development/Auto default to `core.hooksPath=/dev/null` and
`core.fsmonitor=false`; these defaults are not an enforcement barrier against
arbitrary owner code.

The project's own `.claude`, `.codex`, `.agents` and `.github` directories are
ordinary project paths. Writing workers may change permitted files through
file tools and eligible live writer commands. `project_git` status/diff and revision digests include
these paths where available. Residual risk: their settings, hooks, MCP server entries, agents, commands
and skills configure later Claude Code and Codex sessions opened in this project,
the owner's own interactive ones included, and a hook or MCP command written
there runs outside any Cortex jail when such a session starts. Review changes to
these directories before opening the project natively. To keep chosen entries
read-only, write one exact directive per path, for example
`deny-write: .claude/settings.json` and `deny-write: .claude/settings.local.json`,
or `deny-write: .claude` for the whole directory; a directory denial also refuses
`project_skill_link`. Wildcard values are not accepted and create no denial. The
file tools refuse a denied path whether or not it exists, but a command keeps it
read-only only if it exists when the command starts: to stop a command from
creating a missing settings file, deny its existing parent directory. The owner's
engine state directories in HOME stay protected as described above.

`project_skill_link` is the guarded way to expose a shared skill: a writing worker may create or
remove exactly the relative symlink `.claude/skills/<name>` ->
`../../.agents/skills/<name>` for a real in-project skill directory. It never
writes a file, never follows a symlinked `.claude`, `.claude/skills` or
`.agents` component, never replaces a file or directory, and replaces a
different existing link only after the user confirms (see
[Project skill links](docs/features/permissions-and-containment/index.md#project-skill-links)).

`/skill-management <request>` is the explicit coordinator command for creating, revising, or importing
project skills. Imported files are untrusted input; source URL, exact revision, and license information
are recorded, and the runtime retains independent review. Skill instructions do not expand assignment
scopes. Repeated work may be proposed to the user through a durable question (`user_question_request` with
`skill_proposal`). The runtime requires 2 to 5 retained reports as evidence, refuses duplicate proposals,
and accepts only explicit, action-matched English or Russian approval/decline option pairs before
persistence. A stored pair is checked again when an answer is interpreted, including after restart;
reversed, misleading or unknown labels cannot approve even through text option `1`. A decline or unclear
answer changes nothing in the project. Skills are never installed automatically or activated merely
because files exist. Once present in the supported project layout, a skill is listed by name, description
and path in the [project skills](docs/features/unified-system-prompt/index.md#project-skills) section of
every Cortex engine session whose role can read project files (owner-home skill directories are not
listed); Cortex does not guarantee selection or use. Skill text is model input and grants no additional
tool access, filesystem scope, or permission. Cortex's conditional `read_skill` trigger catalogue is
role-scoped guidance in the prompt and advertised tool schema; it likewise grants no access and does not
cause runtime-side skill loading. Tool state inside the project (`.venv`, `node_modules/.cache`,
`.pytest_cache`, `__pycache__`, `.mypy_cache`, `target/`, build caches) is
writable because a writing worker's grant is the whole project, with no list of
tools. Internal-only writable hardlinks remain linked. Guarded/engine launches
keep external aliases read-only and demote a directory above 256 such files.
Development/Auto writer commands instead materialize independent copies of granted
external aliases before granting writes, so uv/pnpm environments can be recreated
without project-alias writes changing shared-cache inodes. Denied/private paths
remain excluded; symlinks are unchanged. Reflinks or streaming copies preserve
ownership, modes, timestamps and extended attributes. A 2 GiB logical-byte
preflight and 60 s cooperative deadline fail closed; blocking filesystem calls
can exceed that deadline. Complete files are fsynced, atomically replaced and
their parent fsynced, never removing the environment directory. An interruption
may leave mixed inode identities but no partially overwritten package files;
a fresh scan resumes safely. This is not a whole-tree transaction; a crash can
leave a hidden temporary file. The inherited host-writable profile still permits
direct host aliases; this protects project aliases, not arbitrary same-user code.
See [hardlink preparation](docs/features/permissions-and-containment/index.md#git-and-tool-state)
for cost and error details. `.cortex` stays hidden.
Finding those links scans the whole project before each sandboxed launch, in the
same single parallel pass that finds every nested private entry; a slow
scan is shown in the chat, never skipped or cached, and a sandboxed engine is
never launched with the home directory or `/` as its project. A link count can change
without any project directory changing, so no cached scan could stay exact; the
guarantee is the tree as listed, and an entry or link created after its directory was
listed and before launch is first covered by the next command. A launch that never
released its payload records a fixed-vocabulary `cause` on its receipt, and stage
timings (numbers only) are kept on its runtime events, never in the tool reply.

### Sandboxed command umask

`command_run`/`outside_command` and automatic verification checks run under an
explicit `umask 022`, set by the sandboxed process itself right before it execs
the real command. This is a deliberate choice,
not an oversight: left alone, such a command would instead inherit whatever the
Cortex runtime process itself set for its own private store (`umask 0077`,
still used everywhere the runtime creates its own files/directories directly --
`.cortex/cortex.sqlite3`, engine state, recovery capsules), which made git,
npm, make and similar tools a worker or a check runs create files with
unexpectedly restrictive permissions with no security benefit: everything such
a command can write is already inside the project tree or its own scratch
directory, both already scoped to the running command. Engine processes are
not covered by command umask and may hold native account state.

## Configuration management UI and RPC boundary

The Paseo plugin's Cortex Settings screen edits only the closed surfaces exposed by the Cortex management
API: user defaults, project policy ceiling, routing, MCP servers, connection metadata and the first-run
actions described below. It does not edit Paseo's own plugin settings document or expose arbitrary paths,
commands, or executable selection. Project scope is resolved from Paseo's workspace; the owner
configuration root comes from the Cortex launch environment. For the Rust launch, Settings and automatic
setup apply the same compiled-channel resolver as `cortex-acp`: dev adds `cortex-workspace-dev` to the raw
XDG config root and `cortex-workspace-dev/state` to the raw XDG state root, while project configuration
uses `.cortex-dev`. Release roots and `.cortex` are unchanged. Runtime channel variables cannot redirect
native Settings into another channel. The plugin obtains its displayed binding from the tools binary's
read-free `config --host` metadata; a missing or invalid answer refuses before the requested operation.
HOME/native-account references and explicit owner routing overrides are preserved; this is cooperative
channel separation, not same-user process isolation.

That launch environment comes from the owner-private `paseo-plugin-launch.json` record that setup writes in the Cortex config root of its own environment (`$XDG_CONFIG_HOME/cortex/`, or `~/.config/cortex/`). A Paseo daemon with its own `XDG_CONFIG_HOME` reads its own record. It uses the `~/.config` record only if that record names the same `XDG_CONFIG_HOME`, so one Paseo instance never starts another instance's runtime against that instance's configuration and state. Setup replaces only a record it recognizes as Cortex's own: an older record for the same entrypoint, or a record with the complete Cortex-owned shape and the same storage roots. Any other existing file is refused, never overwritten or deleted. This separates records by environment. It is not an operating-system isolation boundary: every instance runs as the same user.

The first-run actions are: connection discovery, which reads installed adapter versions and checks whether
an engine's native login marker file exists, without creating anything; native-account onboarding of
`codex-acp`, `claude-agent-acp` or `opencode`, which needs an explicit opt-in, the current connections
revision, a qualified installed adapter and the login marker, and then references the owner's existing
native directory without copying it; Download engine, which runs npm to install only the manifest-pinned
`codex-acp` or `claude-agent-acp` package into Cortex's owned engine root
(`$XDG_DATA_HOME/cortex/engines`), bounded to 60 seconds in the former Python management API (native
Workspace npm uses the remaining 30-second request budget, reserving one second for kill/reap and reply),
with no caller-supplied package, version, path or binary and no npm output returned; and default-preset
read, apply (missing files only) and reset (confirmed, revision-checked, with private backups first). The
manual Download engine action starts network access on an explicit press when an npm
install is needed. Native Workspace prefers the verified channel bundle. Otherwise,
release Settings discovery, install and native-account recheck use the service's
`XDG_DATA_HOME` (falling back to `$HOME/.local/share` only when unset); Dev uses
its own state directory's `data` subtree. This selects the Cortex-managed adapter
store; it neither relocates native accounts nor confines npm or engine children to
those directories. Separately, plugin activation or
agent startup automatically downloads these same qualified manifest pins in a non-blocking background pass
(600-second npm bound per engine), registers a missing native-account connection only when its native
login marker exists, and observes models immediately through prompt-free discovery.
`CORTEX_ENGINE_UPDATE=off` disables this automatic pass. The Rust ACP entry runs the same
pass with the same 600-second npm bound and allowlisted progress files; its registration waits for
`connections.lock` without a bound and repairs the owner configuration root to 0700 (owner-only, current
user, no symlink component), catalogue publication is fenced on the unchanged observed record, and a
launch shape its verifier does not admit is never launched. Its registry reader accepts the shapes the
former Python registry parser accepted and refuses the rest; a registry that parser could read but not
re-encode (a lone UTF-16 surrogate) is never rewritten. Native markers are checked for existence only,
never opened or copied. Removal atomically persists an engine opt-out in the registry, preventing
automatic recreation even during concurrent startup; explicit Add connection remains available. Existing
connection records are not overwritten by registration. Model-discovery failure does not roll back a valid
connection or replace a prior catalogue. Progress files contain only allowlisted phase names, not npm
output, credentials or raw engine errors. No request field carries an API key, credential, path, binary,
command or package version, and API-key connections remain setup-command only. Responses carry no
credential value; preset results show configuration paths in display form only.

Native first-run cached catalogue entries are validated for field and evidence
shape and recomputed route identity before reporting readiness; invalid entries
retain `setup_failed` without launching or replacing the cache. New observations
recheck the registry record under `connections.lock` and hold it through durable
publication, excluding cooperative Settings removal/update during that window.
This is not protection against lockless same-user writers or route authorization.

The plugin RPC handlers rely on Paseo daemon authorization (the installed 0.10.0 implementation requires `daemon.manage` for RPC invocation); the handler itself receives no caller identity for an additional per-call check. Writes use revision preconditions and refuse stale revisions. File and project-configuration writes use the existing lock/SQLite transaction path; lockless external writers may still race the final instant. A timeout does not prove that a write failed; re-read state before retrying.

MCP environment values and credential values/paths are not returned. MCP secret values may be retained
with an explicit keep marker without being read back. Treat settings JSON fields as sensitive input: only
enter trusted configuration, and never paste secrets into support logs or reports. OpenCode onboarding is
explicit opt-in, references the owner's existing login rather than copying it, and does not enable
automatic routing. OpenCode models also stay off (routing `enabled`) until the owner switches each one on
under Settings > Models; a model left out of `enabled` is refused exactly like a denied one. The Models
section's **Refresh model lists** action (`cortex.config.catalog.refresh`, management action `refresh` on
`route_catalog`) starts each registered connection's engine for `initialize` and `session/new` only, in a
private empty temporary directory rather than the project, never sends a prompt, and only adds a catalog
snapshot; a failed refresh keeps the previous snapshot and changes no routing.

Connections background polling is read-only and retains the last successful view on read failure, visibly marked with retry controls. It does not retry writes or clear revision-conflict feedback. A passive read started before a local mutation cannot replace that mutation’s returned view.

See [configuration management](docs/configuration.md#manage-settings-in-paseo) for the supported scope and persistence map.

## Store and identity

Model-originated store calls require the authenticated task channel and current
ownership epoch. Role-specific schemas expose proposal and evidence operations;
the former model controls (`delegate`, `decide`, `cancel`, candidate/check
assessment and model-driven integration) are not advertised or dispatched.
Internal runtime services make their own transactional state changes and append
an event in the same transaction. A worker cannot submit a coordinator proposal
through its channel. Worker-panel activity envelopes are redacted before persistence. Those over 64 KiB are kept whole in the private large-output store under coordinator owner scope; the offload adds no channel read permission, and the preview event keeps only bounded fields and the reference.

Each result binds to the exact provisioned worker attempt and its own immutable
report publication. A report submitted inline with `result_submit` is published
through the same owned-draft and `write_report` checks under delivery keys derived
from the channel and the result's request key, before any result is recorded; it
grants no other authority. Delivery keys retain exact receipts; conflicting reuse fails.
Plan objective bounds are enforced before plan persistence and exposed in both
JSON Schema and the advertised property description. Length counts Unicode code
points, not encoded bytes or UTF-16 units. Preflight guidance never raises bounds,
silently truncates requirements or grants authority.
Submission guidance requires a complete report and valid argument serialization;
the owned-draft route is recommended up front for code blocks or reports larger
than about 3 KB, and is an alternative transport, not an acceptance bypass.
Decision-brief items are Markdown sections within `markdown`, never additional
report fields.
Missing reports remain rejected. An engine-side JSON parse failure is not a
runtime submission receipt and never establishes publication or completion.
In one retained Claude review session, some attempts omitted both report forms
and subsequent malformed JSON calls failed before Cortex dispatch; later valid
submissions were received. This session-specific evidence does not show a
general acceptance defect or guarantee future model compliance. Prepare the
complete report and structured arguments before submitting; correct input
serialization/schema errors rather than treating tool output as accepted.
Report hashing opens each component without following symlinks and verifies the
file identity before and after reading. Completion rechecks report digests,
completed nodes, the last independent review verdict, all configured verification
commands, the current project digest, synthesis identity and absence of live
sessions. Workers cannot choose the reviewed revision, weaken check contracts or
write outside their scope. Runtime and independent reviewer effects are separate
from the worker who proposes the implementation.

Project digest caching retains only bounded in-process content hashes.
Reuse requires unchanged device/inode/size/mode/owner/mtime/ctime, the same
no-follow open and final state checks, and timestamps older than the local
filesystem granularity guard. Restoring mtime alone cannot hide a same-size
write. Unknown, remote and overlay filesystems are not cached; no authorization,
persistent revision evidence or review/verification gate is bypassed. See
[project scope and digest caching](docs/features/workspaces-and-integration/index.md).

The runtime creates its project `.cortex` directory owner-only (`0700`) and
refuses a pre-existing directory with broader permissions. The refusal keeps the
path and expected mode and tells the local owner the exact `chmod 700` command;
it does not weaken the check or repair a pre-existing directory automatically.
A writable open of either channel's store (`.cortex` or `.cortex-dev`) adds an
owner-only `.gitignore` of `*` inside it when absent, created exclusively without
following a link and never replaced, so the store stays out of the project's Git
status without any edit to the project's tracked ignore files.

The former bound task inspector (a Python tool, removed on 2026-11-16; see [removed readers](docs/features/observability/index.md#removed-python-era-readers)) was
metadata-only and read-only, and refused busy or uncheckpointed storage rather than checkpointing it. The shipped `cortex-paseo-tools` readers keep that posture: they open the store through the non-initializing live read-only mode, never migrate schemas, initialize files, read report bodies or print tool arguments/output. Collection completeness is not acceptance; host identity, native resource liveness and
report-file digest verification require independent evidence.

## Attack-surface review and limits

The external attack-surface review below was performed during qualification.
System package contents are evaluated at install time, not trusted from source
pins.

- The inbound client chooses the project; it must be trusted to the user.
- ACP is stdio. Session ordering and ownership are runtime-managed.
- The model sees the user's project data and can attempt prompt injection.
- A subagent cannot bypass the runtime permissions because subagents are disabled.
- Command and MCP containment assumes a Linux kernel with Landlock enabled. Landlock restricts file access only: it does not restrict chmod/chown or timestamps, already open descriptors, Unix-socket connections, signals to processes outside the ruleset's own descendants, or the network.
- **Known exposure: ordinary command reads and engine processes.** Ordinary
  worker commands retain host visibility and profile-specific writable mounts.
  Checkout scope and mask rules are not a general host-read policy. A command
  can read owner-readable data outside the checkout where exposed by its view;
  network access can allow disclosure. Guarded and read-only worker profiles
  limit writes, not general host reads. Eligible Development/Auto writers can
  use a non-private canonical host working directory and write the live checkout.
- **Runtime verification limits writes, not general reads.** It uses the live
  read-only command profile with the host network (shared with every command view), private scratch and
  private-store masks. Eligible existing ignored checkout directories and
  permitted host cache paths can still receive writes. A setup refusal is not
  a pass and never triggers a host rerun. This does not establish hostile-code
  isolation or installed-package behavior.
- **No general engine filesystem jail.** Normal sessions and Claude/Codex
  catalogue and usage helpers do not inherit the verifier view. OpenCode keeps
  its separate [catalogue and version probe boundaries](docs/features/opencode-engine/index.md#current-rust-launch-boundaries).
  Experimental synthetic engine/account views do not establish production
  enforcement. Eligible outside actions, including authorized command reroutes,
  are host actions, not containment.
- **Declared work scope is not a filesystem write boundary.** Scope and path
  reservations coordinate concurrent writers; eligible Development/Auto commands
  can persist changes directly in the live checkout without using a project-file
  tool. Role, mode, protected paths and explicit denials still apply at the
  relevant operation boundaries.


Quota-window associations are exact owner settings, not model-family inference. Unmapped windows conservatively retain account-wide applicability, including exhaustion; missing numeric evidence is not zero. Settings validation precedes quota IO, and invalid reset policy cannot erase an engine-reported refusal. Associations cannot expand enabled/deny/provider/consent eligibility. See [the quota settings contract](docs/configuration.md#exact-quota-window-associations); install data is not a runtime overlay and existing profiles/configuration require explicit adoption.

Brokered Cortex MCP channel loss is indeterminate and does not grant route fallback or permission to
replay an uncertain mutation. The existing bounded recovery path requires a fresh attempt after confirming
the old one is absent. Replacement MCP servers must supply fresh, request-matched tool/schema evidence
before readiness; old-generation replies/exits cannot clear current failures. Incomplete or malformed tool
lists fail closed. Serialization failure is contained to the request; none of these protections establish
the cause of a historical production disconnect. See
[failures](docs/features/failures-and-cancellation/index.md). Captured steering remains immutable; native
host composer queue manipulation is not a grant to erase source history. Normal queue draining waits for
background continuation ownership instead of cancelling it. Explicit Stop/cancel still fences waiting
prompts and unfinished work; pending, unacknowledged prompts cannot authorize dispatch after interruption.
See [delivery ownership](docs/features/steering-and-questions/index.md#continuation-delivery-ownership).

The Paseo queued-message config channel now retains a runtime-owned canonical
content checksum with each source capture. A reused message identity with changed
content, or unverifiable legacy capture metadata, is refused rather than silently
acknowledged as an edit. This is a replay-content guard, not client authentication
or permission to mutate captured history. Native Edit remains pre-delivery only;
the plugin notice describes that supported workflow without changing host settings.
After capture-before-steering interruption, an identical retry recovers attachment
descriptors only from the same task and exact captured source revision. The content
checksum guard still runs first; recovery neither trusts replay staging metadata
nor rewrites an existing steering event. Image bodies remain behind `read_source`.

Steering that returns a verified revision to planning does not let the
coordinator skip gates: `synthesis_submit` stays refused outside `RECONCILING`,
and the coordinator's `answer_submit` there completes the task only after the
runtime re-checks completed nodes, report digests, the approved review, the
latest passing verification and the unchanged project digest; otherwise the task
ends `FAILED` (`answer_not_certified:<gap>`). Eligibility comes from runtime
events only and ends with any later plan, assignment, replan, verification
repair or worker provisioning. Planned-work answers share synthesis's unresolved
check assessment and disclosure guard: omitted `not_run` or `unverified` checks
are refused, and disclosed blockers are retained and rendered in the final answer.
An unrelated passing verification cannot silently close those gaps. See
[steering](docs/features/steering-and-questions/index.md).

## Removed dev qualification tooling

The Python dev qualification helpers were removed on 2026-11-16 without replacement: `paseo-dev binding` (the read-only exact-agent binding exporter), `paseo-dev route-preflight` (the routing-policy preflight), the bound task inspector, the outcome assertion script and the case observer. Their security properties no longer describe a shipped surface, and nothing in the product now projects bound inspection metadata. What remains of that design:

- The server still records bounded canonical argument/response fingerprints for the read API and its native reads (hashes, byte counts, explicit completeness flags, hashed typed request/channel/item identities and deferred output IDs) in the project store; unsupported or missing fingerprints are unavailable, and no report/output body is read for them. A fingerprint is not transport acknowledgement, model consumption, a command receipt, successful report delivery or acceptance, and deferred retrieval creates no report-read observation.
- The read-only `cortex-paseo-tools` readers (`project-root`, `task-id`, `worker-history`, `worker-results`, `running-commands`, `pipeline`) open the store only through the non-initializing live read-only mode and never initialize, repair or migrate it.
- A live qualification reads its outcome from the disposable project's store and the Paseo timeline; a missing binding, an unobserved confirmation or an unrun assertion is reported as unverified, never as a pass. No paid consent, model route, authority, hidden tool, batch limit or result-submission rule is relaxed by the absence of these tools.

## Deferred-check retained evidence

Deferred-check closure verifies the retained task-private output body, not just
its SQLite metadata. It opens every directory and the body without following
symlinks, checks owner-only storage and a regular single-link file, reads exactly
the recorded byte count, and rechecks file identity and metadata. Missing,
unreadable, shortened or unsafe bodies retain the disclosed gap. Validation is
read-only: it never creates storage or repairs permissions. No format migration,
new authority or relaxation of independent review is involved.

## Usage-record compatibility

Worker usage explanations use retained route/context evidence only. OpenCode's
missing context and provider plan usage remain unknown; no percentage or window
size is inferred from token totals. Plan percentages are labelled as snapshots
at route selection, not live account meters. These UI states add no quota reader,
credential access or network request.

Task-bound prompt accounting uses the existing immutable usage table and no
new storage format or authority. It retains task, attempt and session
identifiers, call sequence, outcome metadata, and validated numeric counters
from the observed final engine reply. It does not retain raw engine payloads,
read native account transcripts, or infer cost from context occupancy. A
missing report stays unknown, including after an accepted result. Legacy
Paseo reads still recognize previously marked post-result releases.
A separate best-effort write can log a sanitized error and omit the row
without changing result acceptance, termination, or replay policy. See
[usage attribution](docs/features/observability/index.md#2-usagecost-attribution).

Cortex supplies trusted role and route instructions through Claude's session system prompt and a per-session private Codex model instructions file. Task input, project skill listings and recovery context travel in user turns. A pending command reference is scoped to its issuing task and channel. It carries no acceptance authority. A completed command still requires a durable runtime result. Command diagnostics retain the failure class and registration stage without command arguments or paths.
