# Security policy

Fresh-install model settings use the owner's refreshed portable default-profile
routing policy, `codex-acp/gpt-6.1-sol` coordinator at medium effort, and Russian
message language. Existing owner settings are never replaced. Account/connection
identities and machine-specific MCP definitions/paths are not cloned; see
[the install snapshot boundary](docs/configuration.md#fresh-install-owner-default-snapshot).

## Paseo dependency findings (2026-10-04)

The plugin lockfile retains GHSA-vfj7-8cjw-p6xm in `braces@3.0.3`,
propagated through React Native's Metro/Jest tooling. Its 15 high-severity npm
package entries describe one advisory, not 15 proven exploitable defects.
No patched braces release was published at this inspection. Installation and
regression passes do not clear a nonzero audit, and `devDependencies` is not
a security boundary. See the [dependency disposition](docs/project/paseo-dependency-audit.md)
for exact paths, artifact-test limits and the next upstream-fix decision.

## Current ACP boundary (1.0.0)

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
project work paths, not external grants. Unknown source locations do not authorize
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

Separately, the opt-in `scripts/cortex_auth_keepalive.py` tool (never invoked
by the runtime; optional `systemd --user` templates under `scripts/systemd/`
are installed only by explicit owner action) runs, per engine, at most one
zero-cost refresh-forcing command a prior investigation proved does not make
a paid call; an engine without such a proven command is reported
`unsupported`, never guessed. It never reads, copies or prints a
credential/token value and never logs or persists a command's stdout/stderr,
only its bare exit code, as a sanitized `ok`/`needs-login`/`unsupported`/
`error` status. As of this writing both the installed Claude Code CLI and
the installed Codex CLI report `unsupported`, because no zero-cost refresh
command is proven for either (installed-CLI `--help` text describes
status/login display only, never a proven refresh or persistence). See
[opt-in login keepalive](docs/features/failures-and-cancellation/index.md#opt-in-native-cli-login-keepalive-outside-the-runtime).

Localized final-answer disclosure descriptions replace raw check names only in
display. Original names remain unchanged in retained structured evidence and
gap matching; a translated description cannot satisfy a missing exact-name
disclosure. Descriptions and blockers are coordinator-authored text, not runtime
translation or language enforcement. See [final disclosures](docs/features/task-lifecycle/index.md).

Paseo's history boundary distinguishes command output from native reads:
shell calls retain their output in the shell row, including split
execute/input/output updates, while native `read` and Cortex `project_read`/
`read_skill` bodies are metadata-only in display history. Edit/Write aliases map
native field names for the displayed edit row only; they do not perform a file
operation or add authority. This is presentation behavior, not model
enforcement. No-paid checks do not establish live model behavior or rendered
Desktop evidence; those claims require the separately authorized
surface-specific check.

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


Read-only roles (reviewer, auditor, explorer, architect and the other read-only
profiles) run `command_run` in the read-only profile (receipts name it `read_only`) in every
access mode: HOME is read-only except an explicit tool-cache allowlist, and the command runs in
a private PID namespace. Automatic runtime verification uses the same profile in every mode, and
inside a check only the project's git-ignored directories are writable. Writing workers keep
the access-mode profile. Cache writes are allowed, so a read-only command can poison a cache
that a later run reads. The read-only profile does not mask the host `/tmp`, the session D-Bus and
systemd user manager under `/run/user`, the host `/dev`, or a Docker socket if one exists, so a
read-only command can still write `/tmp` and reach those services. The profile shares the owner's
network, as every owner command does, so it limits writes, not egress. Read-only commands keep
running across a mode switch. This is not hostile-code isolation. The deletion classifier
is lexical: it recognises `rm` and the other common deletion commands, but not deletions
hidden inside scripts, interpreters or build tools.

A brokered MCP relay never receives its handshake token in argv or in its environment. The
broker writes the token to `relay.token`, a 0600 file beside its socket in a private 0700
directory, and the relay reads it there without following symlinks. The token is compared in
constant time and is bound to the connecting peer (same uid, and a descendant of the launched
engine). An unbound broker's token is single-use. The file is removed with the broker directory.
PID isolation applies only to the read-only profile, not to writing workers.

A natural-language request such as "do not modify X" creates no write denial. Only an
explicit `deny-write: <path>` does, and it is matched after path normalization. A denial stored by
an earlier runtime with wildcard characters still guards writes (matched as a glob).

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
a host-writable receipt cannot close one once the task is Guarded. Unknown consumption remains unknown;
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

Claude ACP 0.88.0 is separately source-qualified for system instruction delivery;
0.78.0 and 0.87.0 remain independently registered. A Desktop check on 0.88.0 observed its resumed scenario complete without an instruction-delivery refusal; it does not establish native-tool enforcement (see [Claude adapter support](docs/features/claude-engine/index.md#current-support-0880-2026-10-08)). Unknown installed Claude
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
[adapter evidence](docs/features/claude-engine/index.md#current-support-0880-2026-10-08).
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
Paseo/plugin update is claimed. See [Codex evidence](docs/features/codex-engine/index.md#current-support-211-2026-10-07).

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
Plan usage ranks candidates whenever every candidate's usage is known; unknown
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

Cortex's read-only `read_skill` route can read shared skill/reference IDs and
exact `SKILL.md` paths listed in that engine session's sent prompt, bound to the
session and attempt for both coordinator and worker channels. Binding occurs
after the engine session opens, and reads consult the current persisted channel
binding rather than a cached pre-launch context. Compaction advances only that
same session's generation; older persisted generations, revoked/expired channels
and terminated session snapshots cannot authorize reads. A channel cannot move
to another engine session: fallback and recovery require a fresh channel.
Only observed sent metadata (or OpenCode's observed launch prompt), not a
reconstructed payload, can grant paths. It accepts no glob or arbitrary path, and does not
grant general file access. Listed paths are read from their original canonical
files with bounded UTF-8 regular-file checks and symlink-safe traversal; changed
targets and unlisted paths fail closed. The route is not available as a
session-path grant when that session did not receive the corresponding skill
catalog. See [the unified system prompt](docs/features/unified-system-prompt/index.md).

Owner-initiated force-finish is a separate, non-acceptance outcome: an exact
`/cortex-finish` command captured from the ACP/Paseo host or an exact answered
task-closure question is required. The runtime records the source and unfinished
work, fences assignments and owned sessions, and uses terminal `CANCELLED`, not
verified `COMPLETED`. A model claim that the user wants to finish is not an
authorization or a passing review/verification receipt.
Paseo's structured question path is role-neutral across coordinator, worker and runtime fallback requests. For grouped questions it uses one paginated form and submits the complete set together; incomplete submissions are rejected. Unique, bounded topic headers are presentation keys mapped back to exact member identities; a complete legacy id-keyed answer map is also accepted, but mixed or foreign key sets fail closed. Numeric text-fallback selection uses the same current-open creation/batch order as the displayed numbers, not random ids. None of these mappings grants access. The "Other..." field and a chat reply are valid free-text answers. Dismissing or cancelling the form does not create an answer, and a declined or timed-out host elicitation leaves the durable question pending. Where an ACP host lacks the structured UI, Cortex can only fall back to a text prompt; it does not enforce that a model asks every question or that every host renders options. A free-text legacy card's instructional action is not an answer. The live chat echo after an acknowledged answer shows the question with the answer (`Question: … → Answer: …`); it is a presentation of the already recorded answer and grants nothing.
Plan-node `work_kind` is a validated routing classification, not a permission grant. It may override role-derived affinity, preferences and stretch eligibility, but never changes role access, scope, tier or owner route authorization. Explicit classifications survive retries and review fixes. Legacy implementation settings expand only in memory on effective reads; editable source rules remain unexpanded, so an unrelated Settings save does not migrate their spelling. Reads do not write configuration; explicit saves still use the existing target authorization and revision checks.

Plan verification commands execute as literal argv without a shell. Submission rejects command substitution, backticks, redirection, pipes, shell operators and standalone parentheses, so a plan cannot defer that syntax error until verification. Structural validation does not prove semantic correctness of a plan, review or implementation.

Worker briefs preserve the assignment objective, scope and dependency report references. The original task goal and review git evidence are shortened only when needed to fit the provisioning limit, and every shortened field identifies the worker-readable capsule or git-output source. These references use tools already available to the worker and do not change its assignment authority. A recovery capsule whose critical sections exceed their byte budget is compacted deterministically, never refused: each truncated field carries its sha256, size, a fenced preview with its original trust label and the tool that returns the full text (`read_source` for user messages and answers). Each trimmed list names its omitted entries, so recovery never silently loses required context.

A worker `passed` check is accepted only with an exact same-attempt `command_run` or user-confirmed `outside_command` receipt for one completed, nontrivial operation, complete untruncated output and exit code 0. Engine-native shell calls and file reads do not verify checks. Insufficient evidence stays `unverified`; observed nonzero exits stay `failed`. Earlier reports are not rewritten, and older evidence is marked weak when projected under the current rule. Check run history is retained; a failure closes only on a later exact verified rerun that remains the latest result for that command. Identical argv/cwd reruns may cross between `command_run` and `outside_command` in either direction, preserving both receipts and their operation/profile provenance. Same-operation profile changes do not match; this comparison never grants execution permission. A supported syntax, regex or statically proven literal-expectation error in the worker's own standalone Python probe can instead be retracted by a later verified passing correction; the failed receipt stays as `retracted` history, never as a pass. Literal-data probes permit only single-assignment literal bindings, equality assertions and a small pure-builtin allowlist, bounded to 8,192 source characters and 256 AST nodes, with no imports, IO, product calls or dynamic execution. Only one expected literal changes; the original false comparison and all corrected true comparisons are established without executing submitted source. Both complete outputs, same operation/cwd/profile, unique correction binding and later-run checks remain mandatory. Project test/lint/build and runtime-verification failures are never retracted.\n\nTransport state is distinct from task outcome. A plugin disconnect records available session/task/attempt identities and history completeness; restart recovery reads persisted state and leaves actions with unknown outcomes unresolved. Until reload recovery is qualified for the deployed version, update the plugin between active tasks. A host SIGKILL without SIGTERM cannot be recorded, and native engine actions cannot be represented by the retained command receipt mechanism.

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
`assignment_outcome: "failed"` intentionally submits that failed assignment;
there is no changed-command supersession or waiver of review/verification.
The refusal names receipts, evidence gaps and a bounded command identity (program,
argv word count and argv SHA-256 prefix, cwd, profile); it never echoes the argv,
a shell script or command output. Every failed (or still pending) gateway
receipt a submission cites is retained as a runtime event before the other
gates, refused submissions included, so a later result that omits or relabels
it is refused with `omitted_failed_check` and `open_failure` re-derives the
omission on recording; only runs actually cited as checks are retained. The
status-capture recognizer for `... ; code=$?; ...; exit $code` scripts proves
only a narrow simple-command form, requiring an unquoted capture identifier and
`=`; quoted whole assignment words or identifiers are commands, not captures.
Double quotes around the value alone (`rc="$?"`) are supported. Assignment-prefixed
commands are rejected rather than treating the assignment as the command head;
otherwise a hidden `trap` could override the captured failure at shell exit.
This also excludes Bash append-assignment prefixes and leading numeric words,
including file descriptors before redirections, which can hide the same builtin.
Every word must also fit a small literal grammar checked with its quotes:
literal command heads and redirection targets, and only plain `$NAME`/`${NAME}`
expansions in other arguments. Bash parameter expansions such as `${x[code=0]}`
or `${x:code=0}` evaluate arithmetic that can reassign the captured variable
inside an innocuous printer argument, so every other `${...}` form, `$(...)`,
unquoted glob/brace/tilde characters and unbalanced quotes fail closed,
including some harmless forms; a direct-argv check is the supported alternative.
`printf` is excluded between capture and exit, even with a harmless literal format:
Bash `%n` and `-v` can assign the captured variable, and plain variable arguments
can supply those formats/options without using forbidden expansion syntax.
Safe `echo`/`tail` wrappers remain supported.
Other variable exits stay unverified (`status_variable_unproven`); it is a bounded classifier, not a shell
parser or sandbox. Command receipts inline at most 64 KiB of output; evidence
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
store and published reports.

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
still apply. Configured labels retain conservative local grammar.
Model prose and hints remain non-consent, and hard route
authorization checks are unchanged. See [routing](docs/features/routing-and-fallback/index.md#settings-only-routing).
Creating a project file creates its missing parent directories only after the
private-path, write-grant and user deny-write checks passed for the full path;
each component is created and reopened no-follow, and a file or symlink component
is refused (`gateway_parent_not_directory`) without following it.

Project file writes never accept a partial read as overwrite authority. `project_read` returns line pages whose `sha256` identifies the bytes read; only a channel's complete, non-truncated coverage of the current bytes yields `full_content_sha256`, and `project_write` of an existing file requires that value (`gateway_complete_read_required` otherwise). A stale read, a hand-supplied hash and `absent` for an existing file are refused before anything changes. A large shrink is flagged in the receipt (`drastic_shrink`) rather than refused, but it still needs complete coverage. `project_edit` applies ordered exact replacements atomically after any prior text read of the current bytes; a missing or ambiguous match refuses the whole edit. Coverage is per channel and per digest and stores no file bodies. A successful `project_write` or `project_edit` replaces the path's coverage with the digest of the bytes that channel just produced: complete after a full write or an edit of a completely read file (never for `.gitignore`, where the runtime adds protected lines), otherwise partial. A later change by anyone else therefore still fails as a stale read, and partial coverage never becomes overwrite authority. Coverage is a runtime ledger, not a permission grant: private, link and deny-write checks still apply before any read or write. A line longer than 2000 characters is never counted as read, so such a file can be changed only with `project_edit`.

Internal `.gitignore` reconciliation uses an exact, unnumbered, non-truncated snapshot through the same no-follow, single-link and 16 MB reader; public read pages are never repair input. Edit presentation uses the protected bytes committed by the writer. Diff matching is skipped before computation when combined input exceeds 32,000 characters or the before/after line-count product exceeds 250,000; the receipt marks `diff_omitted: "input_budget"` and `diff_truncated: true`. Hash and size evidence remain available.

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
`scripts/cortex_commands.py` utility. No model-facing MCP stop tool is exposed;
model-originated tool dispatch cannot claim the owner origin. Requests are bound
to an exact brokered gateway command and require a reason. This is not a hostile
same-user-code boundary: native Codex `apply_patch` and OpenCode `execute` actions
are not controlled by this command-stop feature, and local processes outside the
gateway are not claimed to be isolated by it. A request acknowledgement means
only that the durable request was accepted, not that the process has exited.
See [command-stop controls](docs/features/steering-and-questions/index.md#stop-one-worker-command-with-a-reason).

Each channel-bound engine process runs in a Linux `bwrap` engine jail. Codex's
remaining native tools stay jail-bounded: the per-process launcher and the
verified private adapter's session-config patch both force Codex's own
`shell_tool` and `view_image` features off (`-c features.shell_tool=false -c
features.view_image=false`, re-forced in every generated session config so
App Server's own thread/start config cannot restore them), verified against
the installed Codex CLI 0.154.0's `features list`; Codex then runs shell commands
and reads images only through the scoped Cortex `command_run`/`project_read`/
`read_source` tools. Native `apply_patch` remains on for file edits -- the installed Codex
CLI exposes no supported off switch for it, and it is not even listed by
`features list` -- and a separate `unified_exec` feature was found still
enabled after this change, with neither `-c features.unified_exec=false` nor
`--disable unified_exec` having any observed effect; whether it still offers a
residual native exec/read path once `shell_tool` is off is not established
without a live qualification check (not requested for this change). In Claude
Cortex sessions, adapter-level `disallowedTools` also disallows native Bash,
file/search/edit, notebook edit and Skill alternatives so scoped Cortex MCP
tools provide the corresponding workflow and command receipts; native
subagents remain excluded. The jail remains a backstop for tools the engine
still exposes. Effective exclusion in a real
Claude 0.81.2 turn still requires a new owner-authorized live check. The host is mounted read-only, but `/run/user` remains visible inside the
engine jail (including session-bus, audio, display and keyring sockets); this
is an accepted exposure, not a permission boundary. `/tmp` is explicitly bound
as the host's real, writable path (a screenshot the owner points at, a file the
process leaves for the owner both need the same real path on both sides of the
jail), while `/var/tmp` -- which has no such
interop use, and can be a read-only mount or absent entirely once the whole
host is bound read-only -- is instead a private, writable, per-run directory
backed by the process's own scratch, never the host's shared one; `HOME` and
`TMPDIR`/`TMP`/`TEMP` are that same private scratch. Cortex creates that scratch
and its other temporary files under the host's `/var/tmp`
(`cortex_runtime.tempdirs`) rather than a RAM-backed `/tmp`. The private
`/var/tmp` is mounted before the scratch, so a sandbox sees its own scratch but
not the other Cortex scratch directories there, which previously sat in the
shared `/tmp`. The project mount follows
the engine's existing launch-time jail grant: writing workers can have the
whole project writable even when their runtime-declared assignment work area is
narrower. That declared area coordinates parallel writers; it is not a gateway
write boundary. The gateway enforces assignment/role authority, user-protected
paths, private-path restrictions and path safety, but does not compare ordinary
writes with the declared work area. An `access_request` reserves additional
work for coordination; it does not remount or widen the OS jail. Writer conflicts
receive bounded delayed runtime rechecks (1/2/4 seconds), then a runtime grant or
denial without a user question. Private, unsafe, deny-write and read-only refusals
are immediate runtime decisions. Retained old access cards are cancelled; late
user answers cannot grant access. Retry scheduling is retained runtime state,
not a synthesized user answer; authority loss fences the wait.
The advertised worker question tool is reserved for genuine user decisions,
not missing project access. Its question and options must use the selected chat
language while internal worker communication stays English; these are generation
instructions, not runtime translation or semantic enforcement of arbitrary prose. See
[the access flow](docs/features/orchestration/index.md#worker-questions-and-additional-access). The engine
jail also leaves the host's real `/tmp` writable and the engine state directory
writable apart from protected session-configuration files, so these locations
are residual write surfaces, not denied by the project gateway. `.cortex` is hidden (for a task bound to a linked worktree, the jail root is that worktree
and the main repository's shared `.cortex` is masked too); the project's
`.codex` and `.claude` are ordinary project paths; `.git` is read-only except the
repository data git needs for a writing worker (see
[Git and tool state in the project](#git-and-tool-state-in-the-project)). The coordinator, reviewers and
read-only roles get no project write grant. Database-architect and
performance-engineer are also read-only roles, as is the runtime-reserved
build-verification profile (not dispatched; the runtime executes verification).
The engine's state directory (`CODEX_HOME`
or `CLAUDE_CONFIG_DIR`, the owner's real one for a native account) stays writable
except the files and directories that configure later sessions (settings, hooks,
instructions, skills, agents, commands, rules); absent ones are first created
neutral on the host so none can be planted, and installed plugins are hidden. A
revoked or expired channel terminates the process. The one bounded exception is a
worker channel revoked by acceptance of that attempt's own current result: the
watchdog lets the in-flight turn end for at most 20 seconds so its final token
report can arrive, and the post-result settlement normally tears the engine down
when the turn ends, within 15. Native tools can still act in that window; such late project changes
invalidate the digest-bound review, verification or completion gate as above.
Explicit cancellation, stop and replanning terminate at once. Inside the jail Codex runs with
full access, since its own sandbox cannot start there; its approval policy is
pinned to `untrusted` so each command still reaches the permission bridge, which
allows it unless it deletes (see [Access mode](#access-mode-development-guarded-and-auto)).

Excluded regardless of the jail: native subagents (Claude `Agent`, `Task`,
`Workflow` and team tools are disallowed and refused at the permission bridge;
Codex agents and multi-agent features are forced off), user and project MCP
servers (strict MCP config; native Codex MCP entries disabled), hooks, plugins,
and (Codex only) the `shell_tool` and `view_image` features described above.
The host network is shared, which also exposes abstract Unix sockets of the host
network namespace. The engine binary itself is trusted, runs as the owner and can
read what the owner can read.

### OpenCode (optional engine)

OpenCode v2 is launched only after the owner's explicit
`add-opencode-connection` step, only from a trusted binary whose sandboxed
`--version` and `initialize` `agentInfo.version` are both in the `2.x.x` range, and
only inside the engine jail. Anything else fails before a session. Its additional
boundaries, checked by no-paid tests against the installed binary
([OpenCode adapter](docs/features/opencode-engine/index.md)):

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
claude-agent-acp connection's plan-usage percentage. The subprocess
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

Cortex gateway file writes check assignment and role authority (writing workers hold the whole-project grant), use no-follow descriptors, require complete-read or expected-content hashes, and replace files atomically. Commands run under Linux
`bwrap`: the project is read-only by default and launch-time grants make the
whole project writable for a writing worker; that jail grant is broader than
the worker's runtime-declared assignment scope. Additional access requests
update only the declared-scope writer reservations after runtime validation,
not the OS jail. Private `.cortex` paths are masked, the project's `.codex` and
`.claude` are ordinary project paths, and the project's `.git` is visible as described below. The restrictive command profile runs as the owner: their user and
group names resolve, their real HOME is mounted read-only (so its dotfiles and
credentials are readable by the command), the runtime's environment is passed on
except `CORTEX_*` and credential-like variables (token, secret, password, API key,
auth, cookie, session; a commit author's `GIT_AUTHOR_NAME`/`_EMAIL`/`_DATE` are
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

The owner-run `src/cortex_clear.py --project-root PATH --days N` alternative
retains its direct manual semantics and repeatable `--keep-thread` protection;
it is not automatically active-task-aware. Optional `--force` explicitly marks
stale live-state records released/terminated; `--dry-run` previews without deletion.
Run it only with the runtime idle.
Models/coordinators/workers must never run either deletion route on behalf of
the owner. See [retention maintenance](docs/project/packaging.md#retention-maintenance).

<a id="access-mode-guarded-and-auto"></a>
### Access mode (Development, Guarded and Auto)

Fresh ACP sessions durably start **Development**. A writing worker's ordinary
`command_run` uses host-writable mounts, real HOME, real /var/tmp, owner devices,
groups and shared network through the channel-bound host broker. Docker/Compose,
SDK/cache state and authorized outside-project cwd need no routine permission
card merely for crossing the project boundary. The mode decides
the host for writing workers: Development and Auto give them the host-writable profile (a
`docker build` check can record its builder state in `~/.docker/buildx`), and Guarded keeps
them restricted. Read-only roles, reviewers and automatic verification always get the
read-only profile (`read_only`), while their project stays read-only -- verification may write only
git-ignored directories, and a check whose project digest changes is incomplete. A mode switch
does not stop a read-only command or a verification check. Only a host-writable (Development or
Auto) writing-worker command is stopped by a mode switch or a denial change. Engines themselves stay jailed.

This is **trusted local development, not hostile-code isolation**. Direct masks
hide the bound task's private runtime storage and protect Git configuration,
user-denied project paths, known existing Cortex configuration/runtime/engine
locations and the production Paseo home/profile. A task worktree beneath a
protected parent keeps its own grant. These mounts do not make arbitrary owner
code, alternate host aliases, hardlinks outside the tree, process access or Docker
unable to circumvent them. The daemon resolves bind paths in its own namespace;
rootful Docker may have host-root authority, rootless Docker still has owner
authority, and remote contexts have their own authority. No endpoint switching,
sudo, group change, daemon installation or credential copying is implicit.

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
effects. `project_delete` and `outside_command` consent remains. In Development
an `outside_write` needs no card unless its target, as named or through a
symlinked parent, is or lies beneath a protected control-plane location
(`sandbox.control_plane_paths`, existing or not); those still ask.
Development keeps the worker engine floor.

**Guarded** remains the explicit restrictive command profile (host/HOME mostly
read-only, writable project and tool caches) with consent for every outside
write and outside command. **Auto** uses the Development host-writable command
profile for writing workers, runtime auto-approval of every consent card and the
most permissive offered worker engine mode.
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
applies. With HOME read-only, the owner's `~/.ssh/config` and `known_hosts`
apply unchanged, so ssh inside the sandbox behaves as in the owner's terminal.
The agent never releases private keys, but any sandboxed process, and thus the
model, can ask it to sign. That means it can authenticate to every host the
unlocked keys open, within the task's network access. Lock or remove keys from
the agent (`ssh-add -D`, or a keyring lock) to withhold that. An agent
configured to confirm each use (`ssh-add -c`) still asks the owner on the
desktop.

Unprivileged bubblewrap maps only the owner's uid into the sandbox, so a file
root owns on the host appears owned by `nobody` there. ssh rejects an
`Include`d config file (for example `/etc/ssh/ssh_config.d/*.conf`) owned by
anyone but root or the user. Each sandbox that shows the whole host
therefore mounts, at the real path of every `~/.ssh/config` or `Include`d
file that root owns and no group or other user can write, a read-only copy of
the same bytes owned by the owner
(`orchestration.ssh_askpass.mount_trusted_configs`). These are exactly the
files ssh accepts on the host. A file ssh rejects on the host stays as it is
and is rejected the same way. The copies live in the command's own scratch
directory, mounted read-only.

### Optional ssh password dialog

This is off by default. With {\"enabled\": true} in
`$XDG_CONFIG_HOME/cortex/ssh-askpass.json`, an engine process started after
that point and every worker `command_run` additionally receive the owner's
`DISPLAY`, `WAYLAND_DISPLAY`, `XAUTHORITY`, `XDG_RUNTIME_DIR` and
`DBUS_SESSION_BUS_ADDRESS` (each checked for shape), plus `SSH_ASKPASS` and
`SSH_ASKPASS_REQUIRE=force`. ssh then shows its prompt in a `zenity` or
`kdialog` dialog, and the typed answer is written to ssh only. It never reaches
the model, a transcript, a log or the store. Cortex generates the dialog script
and mounts it read-only, so a sandboxed process cannot replace it. The switch
lives in the owner's config directory, which the sandbox cannot write. Git also
uses `SSH_ASKPASS` for HTTPS credential prompts. Any process in the sandbox can
also draw windows on the owner's display, including a fake of this prompt, so
type a password only for a connection you expect. When no display session or
dialog program is available, nothing is added. `sudo` still fails: bubblewrap
sets no_new_privs, which setuid programs refuse.

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

With a whole-project grant, git run by a worker (`command_run`, or its native
shell in the engine jail) may update the project's own repository: the `.git`
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
Such an entry that does not exist yet cannot be mounted over without creating it
in the owner's repository; instead a guard checks every half second while the
sandbox lives and once more when it ends, and removes any that appeared
(`command_run` then fails with `git_configuration_protected`, naming them).
Residual risk: a process that deliberately detaches from its `command_run` and
outlives it is no longer swept, and could still plant one of those absent entries
-- the same class as the racing same-user process this document already excludes.
A writing worker can also leave an interactive rebase or cherry-pick in progress;
git shows that state and runs its todo list only when the owner continues it.
Reviewers, read-only roles and automatic verification see `.git` read-only. `project_read`/`project_write`/`project_edit`/`project_delete` still refuse every
path through `.git` or `.cortex`, and plan scopes cannot name them.

For read-only git observations and command/engine sandboxes without a repository
write grant (including narrow Development/Auto commands), Git command configuration
redirects `lfs.storage` to disposable
`<scratch>/lfs`. LFS clean/process filters stay enabled; their temporary files
and new objects default to scratch rather than real git metadata, including a linked
worktree's common directory. The redirect itself grants no real `.git` writes.
Inherited Git configuration is preserved; explicit `git -c` can override the
redirect but cannot change mount permissions. Scratch is not a persistent LFS
cache: smudge, text conversion and downloads needing existing objects are
outside this read-only diff/status support. Separately, Development/Auto writers
with a whole-project grant (`.`, `**`, `**/*`) may update index, objects, refs,
logs and merge/rebase/stash data in ordinary repositories and linked worktrees.
Narrower grants (even `*`) leave metadata read-only. Configuration/hook/info
overlays follow data grants; binding an existing config file read-only also
prevents Git's lockfile+rename replacement. The real-bubblewrap regression checks
both direct writes and rename-over, but namespace skips do not verify enforcement.
Development/Auto commands default to `core.hooksPath=/dev/null` and
`core.fsmonitor=false` without replacing inherited Git settings. Explicit
command-line configuration and arbitrary owner code are not prevented by these defaults.
Whole-project engine/Guarded writing grants keep the repository-data permissions
and persistent LFS storage described above. Whole-project Development/Auto
writers also use persistent LFS storage, so staging/committing cannot discard new
LFS objects with disposable command scratch.

The project's own `.claude` and `.codex` are ordinary project paths: a writing
worker may read, write and delete them with the file tools and commands like any
other project file, and `project_git` status/diff and revision digests include
them. Residual risk: their settings, hooks, MCP server entries, agents, commands
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

`/skill-management <request>` is the explicit coordinator command for creating, revising, or importing project skills. Imported files are untrusted input; source URL, exact revision, and license information are recorded, and the runtime retains independent review. Skill instructions do not expand assignment scopes. Repeated work may be proposed to the user through a durable question (`user_question_request` with `skill_proposal`). The runtime requires 2 to 5 retained reports as evidence, refuses duplicate proposals, and accepts only explicit, action-matched English or Russian approval/decline option pairs before persistence. A stored pair is checked again when an answer is interpreted, including after restart; reversed, misleading or unknown labels cannot approve even through text option `1`. A decline or unclear answer changes nothing in the project. Skills are never installed automatically or activated merely because files exist. Once present in the supported project layout, a skill is listed by name, description and path in the [unified system prompt](docs/features/unified-system-prompt/index.md) of every Cortex engine session, which replaces the engines' own skill catalogs with the union of the project and owner skill directories either engine reads; Cortex does not guarantee selection or use. Skill text is model input and grants no additional tool access, filesystem scope, or permission. Cortex's conditional `read_skill` trigger catalogue is role-scoped guidance in the prompt and advertised tool schema; it likewise grants no access and does not cause runtime-side skill loading. Tool state inside the project (`.venv`, `node_modules/.cache`,
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
Finding those links scans the whole project before each sandboxed launch; a slow
scan is shown in the chat, never skipped or cached, and a sandboxed engine is
never launched with the home directory or `/` as its project.

### Sandboxed command umask

`command_run`/`outside_command` and automatic verification checks run under an
explicit `umask 022`, set by the sandboxed process itself right before it execs
the real command (`bwrap` has no `--umask` option). This is a deliberate choice,
not an oversight: left alone, such a command would instead inherit whatever the
Cortex runtime process itself set for its own private store (`umask 0077`,
still used everywhere the runtime creates its own files/directories directly --
`.cortex/cortex.sqlite3`, engine state, recovery capsules), which made git,
npm, make and similar tools a worker or a check runs create files with
unexpectedly restrictive permissions with no security benefit: everything such
a command can write is already inside the project tree or its own scratch
directory, both already scoped to the running command. A whole engine process
(`engine_jail`) is not covered by this and keeps the runtime's stricter
default, since its own state directory can hold real credentials (a native
account's session tokens) that should stay owner-only.

## Configuration management UI and RPC boundary

The Paseo plugin's Cortex Settings screen edits only the closed surfaces exposed by the Cortex management API: user defaults, project policy ceiling, routing, MCP servers, and connection metadata. It does not edit Paseo's own plugin settings document or expose arbitrary paths, commands, or executable selection. Project scope is resolved from Paseo's workspace; the owner configuration root comes from the Cortex launch environment.

The plugin RPC handlers rely on Paseo daemon authorization (the installed 0.10.0 implementation requires `daemon.manage` for RPC invocation); the handler itself receives no caller identity for an additional per-call check. Writes use revision preconditions and refuse stale revisions. File and project-configuration writes use the existing lock/SQLite transaction path; lockless external writers may still race the final instant. A timeout does not prove that a write failed; re-read state before retrying.

MCP environment values and credential values/paths are not returned. MCP secret values may be retained with an explicit keep marker without being read back. Treat settings JSON fields as sensitive input: only enter trusted configuration, and never paste secrets into support logs or reports. OpenCode onboarding is explicit opt-in, references the owner's existing login rather than copying it, and does not enable automatic routing. OpenCode models also stay off (routing `enabled`) until the owner switches each one on under Settings > Models; a model left out of `enabled` is refused exactly like a denied one. The Models section's **Refresh model lists** action (`cortex.config.catalog.refresh`, management action `refresh` on `route_catalog`) starts each registered connection's engine for `initialize` and `session/new` only, in a private empty temporary directory rather than the project, never sends a prompt, and only adds a catalog snapshot; a failed refresh keeps the previous snapshot and changes no routing.

See [configuration management](docs/configuration.md#manage-settings-in-paseo) for the supported scope and persistence map.

## Store and identity

Model-originated store calls require the authenticated task channel and current
ownership epoch. Role-specific schemas expose proposal and evidence operations;
the former model controls (`delegate`, `decide`, `cancel`, candidate/check
assessment and model-driven integration) are not advertised or dispatched.
Internal runtime services make their own transactional state changes and append
an event in the same transaction. A worker cannot submit a coordinator proposal
through its channel.

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

The [bound task inspector](docs/features/observability/index.md#bound-task-inspection) is
metadata-only. It requires an explicit project/session/task/current owner-epoch
binding, validates owned non-symlink storage, and takes the existing access lock
exclusively without creating it. SQLite is read-only/immutable only after a
nonempty WAL has been ruled out under that lock; busy or uncheckpointed storage
is refused, never checkpointed or silently read without its WAL. It does not
construct a writable/non-initializing Store, migrate schemas, initialize files,
read report bodies, print tool arguments/output or probe processes. Pagination
is digest-bound and refuses changed or oversized snapshots. Collection
completeness is not acceptance; host identity, native resource liveness and
report-file digest verification require independent evidence.

## Attack-surface review and limits

The external attack-surface review below was performed during qualification.
System package contents are evaluated at install time, not trusted from source
pins.

- The inbound client chooses the project; it must be trusted to the user.
- ACP is stdio. Session ordering and ownership are runtime-managed.
- The model sees the user's project data and can attempt prompt injection.
- A subagent cannot bypass the runtime permissions because subagents are disabled.
- Sandboxing assumes Linux user/mount/PID/network namespaces and bubblewrap.
- Scope strings are not treated as proof of authorization; they are validated and
  checked at each operation boundary.


Quota-window associations are exact owner settings, not model-family inference. Unmapped windows conservatively retain account-wide applicability, including exhaustion; missing numeric evidence is not zero. Settings validation precedes quota IO, and invalid reset policy cannot erase an engine-reported refusal. Associations cannot expand enabled/deny/provider/consent eligibility. See [the quota settings contract](docs/configuration.md#exact-quota-window-associations); install data is not a runtime overlay and existing profiles/configuration require explicit adoption.

Brokered Cortex MCP channel loss is indeterminate and does not grant route fallback or permission to replay an uncertain mutation. The existing bounded recovery path requires a fresh attempt after confirming the old one is absent. Replacement MCP servers must supply fresh, request-matched tool/schema evidence before readiness; old-generation replies/exits cannot clear current failures. Incomplete or malformed tool lists fail closed. Serialization failure is contained to the request; none of these protections establish the cause of a historical production disconnect. See [failures](docs/features/failures-and-cancellation/index.md). Captured steering remains immutable; native host composer queue manipulation is not a grant to erase source history. Normal queue draining waits for background continuation ownership instead of cancelling it. Explicit Stop/cancel still fences waiting prompts and unfinished work; pending, unacknowledged prompts cannot authorize dispatch after interruption. See [delivery ownership](docs/features/steering-and-questions/index.md#continuation-delivery-ownership).

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

## Dev qualification binding boundary

`scripts/cortex_paseo_dev.py binding` is an internal read-only dev CLI helper,
not a plugin RPC or a new host capability. It accepts only an exact owned agent
UUID, workspace and canonical disposable project outside the checkout and both
Paseo homes. It checks private, bounded, owned, non-linked dev/production PID
records for distinct endpoint/PID/server identities, and fences read races and
endpoint/server drift. Reading those external records requires explicit permission;
this command does not grant it. It removes inherited PASEO overrides and uses
only the fixed dev loopback endpoint, with no daemon auto-start or credentials.
Authentication-required hosts refuse rather than opening an account/login route.

Only the exact agent is fetched/refetched: provider, workspace, cwd and the
strict existing runtime wrapper must match. Guarded immutable storage discovery
rechecks the exact session/task/current durable owner and binding digest. No
latest-task fallback, unrelated enumeration, report-body reads, initialization,
migration, checkpoint or lifecycle writes occur. Observation is not an ownership
lease, engine confirmation, disconnect/resource-release receipt or acceptance.
Timeout/output/identity/storage/auth refusals expose safe codes, not exception
text or raw logs. Failures may expose only fixed allowlisted diagnostic stages/reasons:
local missing metadata, spawn, deadline/output and subprocess observations, or a
complete canonical exit-1 task-inspector reason at `stage=task-discovery`.
Unknown, malformed, duplicate/escaped, truncated or terminated output supplies no
child-authored reason; local process observations remain distinguishable without
copying the output. Unknown exception codes are redacted, not accepted by prefix.
Zero exit requires a complete canonical exact-identity/digest binding envelope;
missing evidence or extra fields never become success. Diagnostics never return a binding or authorize another read, retry,
checkpoint or migra
## Inspection projection and route preflight

The bound task inspector projects runtime-created outcome metadata and nothing
else:

- receipt refs, exit codes and completeness;
- retained-output size and truncation flags, without reading the body;
- check outcomes with their `closed_by`/`retracted_by` refs;
- allowlisted route-decision fields;
- rejection codes with bounded field paths;
- question kinds and blockers.

Check names, question text and options, argv, output, candidates and reasons
stay hidden or hashed. A model-authored reference that is not an exact gateway
receipt is shown as `unrecognized`. Bindings, cursors, refusals and the
no-initialize boundary are unchanged.

`scripts/cortex_qualification_assert.py` evaluates expectations over the same
bound, complete inspection snapshot. It adds no reader, binding route or write;
an inspection refusal exits 2 with a safe code, and an unmet expectation exits 1.
A zero exit is an assertion over metadata, not acceptance, rendered-GUI evidence
or proof that native processes ended.

`cortex_paseo_dev.py route-preflight` reads one owner routing policy file and
reports its hash. It refuses the production Paseo home. It never reads
credentials, refreshes quota, writes settings or grants model consent. A ready
result is not an authorization: dispatch still enforces every owner policy and
availability check.

## Deferred-check retained evidence

Deferred-check closure verifies the retained task-private output body, not just
its SQLite metadata. It opens every directory and the body without following
symlinks, checks owner-only storage and a regular single-link file, reads exactly
the recorded byte count, and rechecks file identity and metadata. Missing,
unreadable, shortened or unsafe bodies retain the disclosed gap. Validation is
read-only: it never creates storage or repairs permissions. No format migration,
new authority or relaxation of independent review is involved.

## Usage-record compatibility

Post-result releases retain the original closed SQLite basis vocabulary and
use existing `failed`/`stop_reason` columns; no table rebuild or format change
is needed. Paseo hides only the token alarm for non-failed, explicitly marked
release calls. It does not infer missing usage as zero/free or suppress genuine
unknown/failed calls. Interrupted-turn accounting retains only recognized
nonnegative integer token fields from the exact final response observed within
the existing cleanup bound; it never reads native account transcripts or treats
context occupancy as cost. Engine/role coverage remains metadata-only and does
not change permission, acceptance, termination or storage-format contracts. See
[usage attribution](docs/features/observability/index.md#2-usagecost-attribution-observabilityusage).
