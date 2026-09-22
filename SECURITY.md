# Security policy

## Current ACP boundary (1.0.0)

Cortex is an incoming ACP stdio agent. It launches Codex and Claude ACP engines
as separate processes and stores task metadata and published Markdown under the
single project-local `.cortex` directory. The current working tree is undergoing
a runtime-owned orchestration rewrite and is **not release qualified**. See
[implementation status](docs/acp/runtime-rewrite-status.md).

The model proposes plans, writes reports and submits results. The runtime validates
plan structure, reserves dependency-ready assignments, selects configured routes,
requires independent review, runs verification and checks completion invariants.
A model tool call cannot select a task state, accept the task or directly create
an engine session. Structural validation does not prove semantic correctness of
a plan, review or implementation.

## Tool access and containment

Each channel-bound engine process runs in a Linux `bwrap` engine jail, so its own
native tools (shell, file edit, patch, search, skills) keep working and are bounded
by the operating system rather than disabled. The host is mounted read-only with
`/run/user` hidden (its session bus could start unconfined processes) and a private
`/tmp`; the project is read-only except the assignment's scopes, `.cortex` is
hidden and `.git`, `.codex`, `.claude` are read-only. The coordinator, reviewers and
read-only roles get no writable scope. The engine's state directory (`CODEX_HOME`
or `CLAUDE_CONFIG_DIR`, the owner's real one for a native account) stays writable
except the files and directories that configure later sessions (settings, hooks,
instructions, skills, agents, commands, rules); absent ones are first created
neutral on the host so none can be planted, and installed plugins are hidden. A
revoked or expired channel terminates the process. Inside the jail Codex runs with
full access and no approvals, since its own sandbox cannot start there, and
permission requests are allowed.

Excluded regardless of the jail: native subagents (Claude `Agent`, `Task`,
`Workflow` and team tools are disallowed and refused at the permission bridge;
Codex agents and multi-agent features are forced off), user and project MCP
servers (strict MCP config; native Codex MCP entries disabled), hooks and plugins.
The host network is shared, which also exposes abstract Unix sockets of the host
network namespace. The engine binary itself is trusted, runs as the owner and can
read what the owner can read.

Every supplied MCP endpoint is a runtime broker. Cortex operations check the
current task, assignment, channel and owner epoch. External servers require an
administrator-owned exact tool allowlist; their process gets only runtime-granted
scope mounts, explicit environment and optional explicit network/read grants.
An explicit network grant also exposes only the public system resolver/hosts/NSS
configuration and CA bundle as read-only files; without it those files remain
unmounted. The full `/etc` tree and user home are never added by that grant.
A read grant cannot overlay the project or unmask its private store. Native account
MCP entries are disabled. Server-initiated sampling/elicitation is refused.

File writes use scope checks, no-follow descriptors, expected-content hashes and
atomic replacement. Commands run under Linux `bwrap`: project read-only by default,
only granted files/directories writable, and private `.cortex`, `.git`, `.codex` and
`.claude` paths masked. A worker's `command_run` runs as the owner: their user and
group names resolve, their real HOME is mounted read-only (so its dotfiles and
credentials are readable by the command), the runtime's environment is passed on
except `CORTEX_*` and credential-like variables (token, secret, password, API key,
auth, cookie, session), and the host network is shared. Supplementary groups show
as `nogroup` inside the user namespace, `/run` (and so the Docker socket) is not
mounted, and `sudo` does not work. External MCP
servers and automatic verification get no HOME, network or inherited environment
unless explicitly granted. Hardlinked writable
files are rejected. Draft writes require the exact owned unpublished draft ID.
File actions and command starts recheck authority at the transaction boundary;
running commands and external servers are terminated on observed revocation.
This does not claim protection against a separate hostile process running as the
same OS user and racing filesystem changes.

### Optional ssh password dialog

This is off by default. With `{"enabled": true}` in
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
read-only project. Only its private `/tmp` is writable. Sandbox absence is a hard
failure. Complete command bytes, exit status, timeout/cancellation and project
digest are retained. A successful MCP envelope alone cannot certify a check: a
passed worker check requires its own complete successful gateway-command receipt.

## Store and identity

Model-originated store calls require the authenticated task channel and current
ownership epoch. Role-specific schemas expose proposal and evidence operations;
the former model controls (`delegate`, `decide`, `cancel`, candidate/check
assessment and model-driven integration) are not advertised or dispatched.
Internal runtime services make their own transactional state changes and append
an event in the same transaction. A worker cannot submit a coordinator proposal
through its channel.

Each result binds to the exact provisioned worker attempt and its own immutable
report publication. Delivery keys retain exact receipts; conflicting reuse fails.
Report hashing opens each component without following symlinks and verifies the
file identity before and after reading. Completion rechecks report digests,
completed nodes, the last independent review verdict, all configured verification
commands, the current project digest, synthesis and worker-session absence.
Unknown live-session state blocks completion. A report's prose alone is not an
observed check result.

A question and its durable identity commit before elicitation. Missing, timed-out
or cancelled answers cannot resume work. Worker questions revoke old grants and
resume planning after the actual answer. User steering and replan/scope/escalation
proposals revoke old worker channels before a replacement plan is delivered.
Explicit path denials are enforced on new scopes. Natural-language policy parsing
supports documented path-denial forms; arbitrary prose is passed to replanning,
not claimed to be a complete semantic permission parser.

Lost assignments receive at most two retries after confirmed session absence;
ambiguous unfinished action receipts prevent replay. Review/fix cycles and model
replan requests are bounded. Coordinator, worker, review and command deadlines
are runtime-owned. Recovery capsules include the current durable graph and policy;
a compacted model summary cannot change either. Live replacement-session and
compaction qualification still require retained real-client observations.

The writable store-open path migrates verified format 11 to format 12 under the
exclusive lock, retaining a private backup under `.migration-backups/`. An
inspection open with `initialize=False` stays non-mutating and reports
`legacy_storage_migration_required`; unknown formats remain fail-closed. Storage
migration is distinct from the removed model-owned orchestration API.

## Display, secrets and evidence

Every observed native and Cortex tool lifecycle is forwarded to the ACP transcript,
including titleless/status-only updates, input/output and locations. Worker display
updates and coordinator display updates are forwarded through the production
wrapper. Paseo hides thinking and protocol bookkeeping, including waits, at the presentation
boundary. Meaningful tool calls keep readable labels, inputs, outputs and failures.
Durable redacted evidence is retained for audit. Secrets continue to be redacted;
transparency is not credential disclosure.

Before any plan or worker exists, the read-only coordinator can submit an
informational answer. Runtime completion checks an exact retained answer receipt,
no plan/revision, no worker provisioning, and no cancellation or blocker. This
path does not certify changes or tests and cannot replace planned work or bypass
its mandatory review and verification.

Outbound updates are retained for replay before transport delivery. Retention
proves an event was produced, not that a real client rendered it. No missing or
unsupported host observation is counted as success. The focused replacement
controller tests use fake model sessions with real storage and sandboxed check
processes. The installed Codex native-dispatch test additionally runs the actual
CLI inside the engine jail with a loopback fake provider and observes native
`apply_patch` writing its granted scope and failing outside it.
It is not a paid model or GUI qualification. See [current test evidence](docs/acp/runtime-rewrite-status.md);
[real-client qualification](docs/acp/real-client-runbook.md) remains unverified for
this rewrite. Read-only Git review observations use fixed sandboxed commands with
hooks, fsmonitor, external diff and text conversion disabled. Private store and
engine configuration paths are excluded from their pathspecs.

A passing claim without a complete successful command receipt is rejected before
result publication or consumption of its delivery key. This permits evidence
correction; it never converts an absent, failed or unresolved check into success.

Paseo display history excludes returned file/skill read bodies before host persistence
and worker activity persistence. The model's MCP response is unaffected. Existing
read events are projected to target/status on replay without rewriting old storage.
Raw data is never rendered in either chat or worker panels; opaque-only calls are
nonexpandable. Execution receipts and independent-review evidence remain intact.
