# Security policy

Fresh-install model settings use the saved 2026-10-05 owner default's portable
routing policy plus the owner-requested 2026-10-07 Haiku 5.5 addition, `claude-agent-acp/opus[1m]` coordinator at medium effort, and Russian
message language. Existing owner settings are never replaced. Account/connection
identities and machine-specific MCP definitions/paths are not cloned; see
[the install snapshot boundary](docs/configuration.md#fresh-install-owner-default-snapshot).

## Paseo dependency findings (2026-10-04)

The plugin lockfile retains GHSA-vfj7-8cjw-p6xm in `braces@3.0.3`,
propagated through React Native's Metro/Jest tooling. Its 15 high-severity npm
package entries describe one advisory, not 15 proven exploitable defects.
No patched braces release was published at this inspection. Installation and
regression passes do not clear a nonzero audit, and `devDependencies` is not
a security boundary. See the [dependency disposition](docs/acp/paseo-dependency-audit.md)
for exact paths, artifact-test limits and the next upstream-fix decision.

## Current ACP boundary (1.0.0)

Claude ACP 0.88.0 is separately source-qualified for system instruction delivery;
0.78.0 and 0.87.0 remain independently registered. One owner-requested Desktop check on 0.88.0 (2026-10-08) observed its resumed scenario complete without an instruction-delivery refusal; it does not establish native-tool enforcement (see [Claude adapter support](docs/acp/claude-agent-acp-adapter.md#current-support-0880-2026-10-08) and [the Desktop evidence](docs/acp/evidence/claude-088-live-desktop-2026-10-08.md)). Unknown installed Claude
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
[adapter evidence](docs/acp/claude-agent-acp-adapter.md#current-support-0880-2026-10-08).
The prompt-free catalog refresh a new ACP session starts runs in a child process
with its own session, so it can outlive the ACP process that requested it (Paseo
stops discovery processes about a second after `session/new`). It is still bounded
by the per-call refresh timeout, sends no prompt, runs one refresh per connection
at a time and only adds a catalog snapshot. See
[catalog refresh lifetime](docs/acp/claude-agent-acp-adapter.md#catalog-refresh-lifetime-and-deployed-runtimes).

Codex ACP 2.1.1 uses the same exact-source-guarded private patches as 2.0.1,
with a separate evidenced registration. Unknown versions remain unsupported;
installing a newer package does not bypass those checks. Role metadata is sent
for every qualified Codex version. Installed CLI 0.159.3 passed no-paid
wrapper/jail tests; native shell/view-image remain disabled and native
`apply_patch` remains jail-bounded. No paid/client enforcement or production
Paseo/plugin update is claimed. See [Codex evidence](docs/acp/codex-acp-adapter.md#current-support-211-2026-10-07).

Cortex is an incoming ACP stdio agent. It launches Codex and Claude ACP engines,
and the optional OpenCode v2 engine once the owner onboards it, as separate processes and stores task metadata and published Markdown under the
single project-local `.cortex` directory. The current working tree is undergoing
a runtime-owned orchestration rewrite and is **not release qualified**. See
[implementation status](docs/acp/runtime-rewrite-status.md).

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
See [speculative verification](docs/acp/orchestration.md#speculative-verification-during-review).
No model hint can waive review. A model tool call cannot
select a task state, accept the task or directly create an engine session.
A model hint cannot grant a route or infer model family/quality. Eligibility, tier, effort and automatic membership come from the effective version-2 owner routing settings and current observed catalogs; the runtime also enforces pins, denies, provider policy, availability, quota and user authorization. New or renamed identities are not automatically eligible. New audits identify configured stretch policy with the neutral `stretch` key and effort basis; no historical audit is rewritten and this label grants no authorization. Policy refusals happen before worker capacity reservation or engine launch; the coordinator route-menu tool reports the named validation failure without claiming an uncertain storage write. See [settings-only routing](docs/acp/routing-and-fallback.md#settings-only-routing).

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
Usage switching starts only at observed usage >=95%; unknown usage is reported
unavailable, never inferred above the threshold.
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
see [question display ordering](docs/acp/orchestration.md#plan-submission-and-coordinator-polling).

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
`schema_validation_errors` diagnostic alongside argument-validation failures.
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
catalog. See [the unified system prompt](docs/acp/unified-system-prompt.md).

Owner-initiated force-finish is a separate, non-acceptance outcome: an exact
`/cortex-finish` command captured from the ACP/Paseo host or an exact answered
task-closure question is required. The runtime records the source and unfinished
work, fences assignments and owned sessions, and uses terminal `CANCELLED`, not
verified `COMPLETED`. A model claim that the user wants to finish is not an
authorization or a passing review/verification receipt.
Paseo's structured question path is role-neutral across coordinator, worker and runtime fallback requests. For grouped questions it uses one paginated form and submits the complete set together; incomplete submissions are rejected. Unique, bounded topic headers are presentation keys mapped back to exact member identities; a complete legacy id-keyed answer map is also accepted, but mixed or foreign key sets fail closed. Numeric text-fallback selection uses the same current-open creation/batch order as the displayed numbers, not random ids. None of these mappings grants access. The "Other..." field and a chat reply are valid free-text answers. Dismissing or cancelling the form does not create an answer, and a declined or timed-out host elicitation leaves the durable question pending. Where an ACP host lacks the structured UI, Cortex can only fall back to a text prompt; it does not enforce that a model asks every question or that every host renders options. A free-text legacy card's instructional action is not an answer. The live chat echo after an acknowledged answer shows the question with the answer (`Question: … → Answer: …`); it is a presentation of the already recorded answer and grants nothing.
Plan verification commands execute as literal argv without a shell. Submission rejects command substitution, backticks, redirection, pipes, shell operators and standalone parentheses, so a plan cannot defer that syntax error until verification. Structural validation does not prove semantic correctness of a plan, review or implementation.

Worker briefs preserve the assignment objective, scope and dependency report references. The original task goal and review git evidence are shortened only when needed to fit the provisioning limit, and every shortened field identifies the worker-readable capsule or git-output source. These references use tools already available to the worker and do not change its assignment authority. A recovery capsule whose critical sections exceed their byte budget is compacted deterministically, never refused: each truncated field carries its sha256, size, a fenced preview with its original trust label and the tool that returns the full text (`read_source` for user messages and answers). Each trimmed list names its omitted entries, so recovery never silently loses required context.

A worker `passed` check is accepted only with an exact same-attempt `command_run` or user-confirmed `outside_command` receipt for one completed, nontrivial operation, complete untruncated output and exit code 0. Engine-native shell calls and file reads do not verify checks. Insufficient evidence stays `unverified`; observed nonzero exits stay `failed`. Earlier reports are not rewritten, and older evidence is marked weak when projected under the current rule. Check run history is retained; a failure closes only on a later exact verified rerun that remains the latest result for that command.\n\nTransport state is distinct from task outcome. A plugin disconnect records available session/task/attempt identities and history completeness; restart recovery reads persisted state and leaves actions with unknown outcomes unresolved. Until reload recovery is qualified for the deployed version, update the plugin between active tasks. A host SIGKILL without SIGTERM cannot be recorded, and native engine actions cannot be represented by the retained command receipt mechanism.

On stdin EOF, the ACP process joins its reader after EOF is observed and before
runtime teardown, so disconnect recording and cancellation are not abandoned
with the daemon reader thread. This does not provide a SIGKILL guarantee or
resolve unknown action outcomes.
On a failed stdout write, it likewise joins the writer's disconnect callback
before joining prompt workers or tearing down the runtime, without joining a
reader blocked on still-open stdin. SIGTERM records the disconnect synchronously.

For `result_submit`, a model-supplied `checks[].command` on a completed check
is descriptive, never evidence. Only the runtime derives `closed_by` from real
same-attempt command receipts; malformed fields get path-specific corrections.
A new result with an open failure is refused with `open_failed_check` before
publication, request recording or channel revocation. Only an explicit
`assignment_outcome: "failed"` intentionally submits that failed assignment;
there is no changed-command supersession or waiver of review/verification.
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
work remain proportional to the task.

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
authorization checks are unchanged. See [routing](docs/acp/routing-and-fallback.md#settings-only-routing).
Creating a project file creates its missing parent directories only after the
private-path, write-grant and user deny-write checks passed for the full path;
each component is created and reopened no-follow, and a file or symlink component
is refused (`gateway_parent_not_directory`) without following it.

Project file writes never accept a partial read as overwrite authority. `project_read` returns line pages whose `sha256` identifies the bytes read; only a channel's complete, non-truncated coverage of the current bytes yields `full_content_sha256`, and `project_write` of an existing file requires that value (`gateway_complete_read_required` otherwise). A stale read, a hand-supplied hash and `absent` for an existing file are refused before anything changes. A large shrink is flagged in the receipt (`drastic_shrink`) rather than refused, but it still needs complete coverage. `project_edit` applies ordered exact replacements atomically after any prior text read of the current bytes; a missing or ambiguous match refuses the whole edit. Coverage is per channel and per digest, stores no file bodies, and is cleared by every successful mutation. Coverage is a runtime ledger, not a permission grant: private, link and deny-write checks still apply before any read or write. A line longer than 2000 characters is never counted as read, so such a file can be changed only with `project_edit`.

Internal `.gitignore` reconciliation uses an exact, unnumbered, non-truncated snapshot through the same no-follow, single-link and 16 MB reader; public read pages are never repair input. Edit presentation uses the protected bytes committed by the writer. Diff matching is skipped before computation when combined input exceeds 32,000 characters or the before/after line-count product exceeds 250,000; the receipt marks `diff_omitted: "input_budget"` and `diff_truncated: true`. Hash and size evidence remain available.

## Tool access and containment

The supported Codex adapter versions are 1.12.0, 1.13.0 and 2.0.1. The
2.0.1 private copy retains exact-source checks for role delivery, manual
review, session configuration, per-turn permissions and MCP setup. Its new
typed renderer preserves Cortex's compaction metadata; missing or duplicated
patch sites block launch. This is explicit compatibility support for one
release, not permission to launch arbitrary future adapters. No native account
or production Paseo installation is changed by the compatibility patch.

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
`TMPDIR`/`TMP`/`TEMP` are that same private scratch. The project mount follows
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
[the access flow](docs/acp/orchestration.md#worker-questions-and-additional-access). The engine
jail also leaves the host's real `/tmp` writable and the engine state directory
writable apart from protected session-configuration files, so these locations
are residual write surfaces, not denied by the project gateway. `.cortex` is hidden (for a task bound to a linked worktree, the jail root is that worktree
and the main repository's shared `.cortex` is masked too) and `.codex`,
`.claude` are read-only; `.git` is read-only except the
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
revoked or expired channel terminates the process. Inside the jail Codex runs with
full access, since its own sandbox cannot start there; its approval policy is
pinned to `untrusted` so each command still reaches the permission bridge, which
allows it unless it deletes (see [Access mode](#access-mode-guarded-and-auto)).

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
([OpenCode adapter](docs/acp/opencode-adapter.md)):

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
`total_cost_usd: 0` (see `docs/acp/token-and-plan-usage.md`). This is the same
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
not the OS jail. Private `.cortex`, `.codex` and `.claude` paths are masked,
and the project's `.git` is visible as described below. A worker's `command_run` runs as the owner: their user and
group names resolve, their real HOME is mounted read-only (so its dotfiles and
credentials are readable by the command), the runtime's environment is passed on
except `CORTEX_*` and credential-like variables (token, secret, password, API key,
auth, cookie, session; a commit author's `GIT_AUTHOR_NAME`/`_EMAIL`/`_DATE` are
identity and pass), and the host network is shared. Supplementary groups show
as `nogroup` inside the user namespace, `/run` (and so the Docker socket) is not
mounted, and `sudo` does not work. External MCP
servers and automatic verification get no HOME, network or inherited environment
unless explicitly granted. Hardlinked writable
files are rejected. Draft writes require the exact owned unpublished draft ID.
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
the owner. See [retention maintenance](docs/acp/packaging-and-onboarding.md#retention-maintenance).

### Access mode (Guarded and Auto)

Each task has an access mode (`configuration.access_mode`). **Guarded** is the
default and the behavior described above: the runtime asks the user on a
permission card before native deletion of declared `rm` invocations (including
direct `rm`, `sudo rm`, `xargs rm`, and similar forms), before `project_delete`,
and before every `outside_write` and `outside_command`; no answer, a timeout, a
disconnect or a refusal means the action does not happen. In **Auto**, these
consent-gated operations are auto-approved without a per-action user card (even
when no client is connected). The owner-selected Auto behavior is to permit outside-project effects without
a permission card. Guarded mode continues to require explicit accepted consent
and refuses when consent is absent or fails. Non-`rm` commands such
as `git clean`, `git commit`, `docker`, `python`, and `pytest` continue without
a permission card in Guarded mode. Workers run in the engine mode that routes
each mutating tool call through `session/request_permission` (Claude `default`,
Codex `agent`).

**Auto** changes exactly two things:

- Those user-approval prompts are answered by the runtime instead of the user:
  it selects the request's own offered allow option (one-shot preferred), for
  the coordinator and for every worker, whether or not a client is connected.
  A request that offers no allow option is still cancelled. Each automatic
  answer is recorded in the task's `access_mode_events` table by tool-call id
  and kind only, never the command or path text.
- A worker opened while the task is in Auto is confirmed in the most permissive
  engine mode its own session offers: Claude `bypassPermissions` (else
  `acceptEdits`, else `default`), Codex `agent-full-access` (else `agent`).
  The mode is confirmed over the wire or the worker does not run. With Claude
  `bypassPermissions` the engine stops sending permission requests at all, so
  the permission bridge no longer sees that worker's native tool calls. Codex's
  per-turn approval policy stays pinned by Cortex's adapter patch, so a Codex
  worker still sends its requests, which Auto answers.

Auto does **not** bypass anything that is not a user approval. In both modes:
the engine jail and `command_run` sandbox (read-only host, scope-bound project
writes, hidden `.cortex`, read-only `.codex`/`.claude`, read-only `.git`
config/hooks/pointers with the `GitGuard` sweep); the channel-verified MCP
gateway (task state, ownership epoch, channel revocation, assignment scopes,
write denials, role rules such as no `command_run` for a coordinator or
consultant, no-follow file operations, expected-content hashes, receipts); the
native-subagent ban (removed at launch, and refused at the bridge whenever
a request reaches it); a stale engine binding is refused; the coordinator's own
engine mode floor; the external MCP allowlist; the paid probes' explicit
`--i-authorize-paid-call`/`--approval-ref` flags; credential-free command
environments (the owner's ssh-agent socket excepted, below) and display/log
redaction. The ssh password dialog is a secret
entry, not an approval, and is not answered by Auto. The ssh password dialog is a secret entry, not an approval, and is not answered by Auto.

Only the user changes the mode, through their ACP client: Paseo's composer mode
control (ACP `modes`/`session/set_mode`) or the `access_mode` config option
(Zed and others). No MCP tool reads or writes it, so a model cannot switch it.
Every switch is an append-only revision in `access_mode_events` with its origin
and ACP session; anything missing or unreadable reads as Guarded. The mode is
never stored as a user default: every new ACP session starts Guarded, and a
choice made before the first prompt becomes the new task's first revision.
Switching back to Guarded applies at once to new prompts and new workers; a
worker already confirmed in a permissive engine mode keeps it until it ends. The
client's rendering of the shield and the Auto path against a paid engine have not
been exercised in the applicable live environment; retain only the evidence for the exact observed surface. The no-paid test suite and startup smoke do not establish GUI behavior.

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
`unverified` (see [check evidence](docs/acp/orchestration.md#check-evidence)). A
recorded failure is never dropped or downgraded; the runtime treats it as closed
only after re-deriving, from its own receipts, an exact later passing rerun of the
same command in the same attempt
(see [check history](docs/acp/orchestration.md#check-history-and-current-outcome)).

### Git and tool state in the project

With a whole-project grant, git run by a worker (`command_run`, or its native
shell in the engine jail) may update the project's own repository: the `.git`
directory, or a linked worktree's own `worktrees/<name>/` plus the shared common
directory even when that lies outside the project. Its data (objects, refs,
logs, index, HEAD, packed-refs, locks, merge/rebase/stash state, `info/`) is
writable. What configures or redirects git runs the owner makes later, outside
any sandbox, stays read-only in each of those directories: `config`,
`config.worktree`, `hooks/`, `commondir`, a worktree's `gitdir`, legacy
`remotes/` and `branches/`, plus other worktrees' metadata, `modules/`
(submodule repositories) and a `core.hooksPath` directory inside the project.
Such an entry that does not exist yet cannot be mounted over without creating it
in the owner's repository; instead a guard checks every half second while the
sandbox lives and once more when it ends, and removes any that appeared
(`command_run` then fails with `git_configuration_protected`, naming them).
Residual risk: a process that deliberately detaches from its `command_run` and
outlives it is no longer swept, and could still plant one of those absent entries
-- the same class as the racing same-user process this document already excludes.
A writing worker can also leave an interactive rebase or cherry-pick in progress;
git shows that state and runs its todo list only when the owner continues it.
Reviewers and read-only roles see `.git` read-only; automatic verification does
not see it. `project_read`/`project_write`/`project_edit`/`project_delete` still refuse every
path through `.git`, `.cortex`, `.codex` or `.claude`, and plan scopes cannot name
them. The single exception is `project_skill_link`: a writing worker may create or
remove exactly the relative symlink `.claude/skills/<name>` ->
`../../.agents/skills/<name>` for a real in-project skill directory. It never
writes a file, never follows a symlinked `.claude`, `.claude/skills` or
`.agents` component, never replaces a file or directory, and replaces a
different existing link only after the user confirms (see
[Project skill links](docs/acp/permissions-and-containment.md#project-skill-links)).

`/skill-management <request>` is the explicit coordinator command for creating, revising, or importing project skills. Imported files are untrusted input; source URL, exact revision, and license information are recorded, and the runtime retains independent review. Skill instructions do not expand assignment scopes. Repeated work may be proposed to the user through a durable question (`user_question_request` with `skill_proposal`). The runtime requires 2 to 5 retained reports as evidence, refuses duplicate proposals, and accepts only explicit, action-matched English or Russian approval/decline option pairs before persistence. A stored pair is checked again when an answer is interpreted, including after restart; reversed, misleading or unknown labels cannot approve even through text option `1`. A decline or unclear answer changes nothing in the project. Skills are never installed automatically or activated merely because files exist. Once present in the supported project layout, a skill is listed by name, description and path in the [unified system prompt](docs/acp/unified-system-prompt.md) of every Cortex engine session, which replaces the engines' own skill catalogs with the union of the project and owner skill directories either engine reads; Cortex does not guarantee selection or use. Skill text is model input and grants no additional tool access, filesystem scope, or permission. Cortex's conditional `read_skill` trigger catalogue is role-scoped guidance in the prompt and advertised tool schema; it likewise grants no access and does not cause runtime-side skill loading. Tool state inside the project (`.venv`, `node_modules/.cache`,
`.pytest_cache`, `__pycache__`, `.mypy_cache`, `target/`, build caches) is
writable because a writing worker's grant is the whole project, with no list of
tools; multiply hard-linked files stay read-only unless every one of their links
is itself a writable path of the project (esbuild links its two binaries to each
other), and a directory with more than 256 read-only ones stays read-only as a
whole (pnpm `node_modules`, a hardlinked uv `.venv`). `.cortex` stays hidden.

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
[project scope and digest caching](docs/acp/workspaces-and-integration.md).

The runtime creates its project `.cortex` directory owner-only (`0700`) and
refuses a pre-existing directory with broader permissions. The refusal keeps the
path and expected mode and tells the local owner the exact `chmod 700` command;
it does not weaken the check or repair a pre-existing directory automatically.

The [bound task inspector](docs/acp/observability.md#bound-task-inspection) is
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

Brokered Cortex MCP channel loss is indeterminate and does not grant route fallback or permission to replay an uncertain mutation. The existing bounded recovery path requires a fresh attempt after confirming the old one is absent. Replacement MCP servers must supply fresh, request-matched tool/schema evidence before readiness; old-generation replies/exits cannot clear current failures. Incomplete or malformed tool lists fail closed. Serialization failure is contained to the request; none of these protections establish the cause of a historical production disconnect. See [failures](docs/acp/failures-and-cancellation.md). Captured steering remains immutable; native host composer queue manipulation is not a grant to erase source history. Normal queue draining waits for background continuation ownership instead of cancelling it. Explicit Stop/cancel still fences waiting prompts and unfinished work; pending, unacknowledged prompts cannot authorize dispatch after interruption. See [delivery ownership](docs/acp/steering-and-questions.md#continuation-delivery-ownership).

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
## Usage-record compatibility

Post-result releases retain the original closed SQLite basis vocabulary and
use existing `failed`/`stop_reason` columns; no table rebuild or format change
is needed. Paseo hides only the token alarm for non-failed, explicitly marked
release calls. It does not infer missing usage as zero/free or suppress genuine
unknown/failed calls. Diagnostic token availability remains honest. See
[usage attribution](docs/acp/observability.md#2-usagecost-attribution-observabilityusage).
