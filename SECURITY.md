# Security policy

## Current ACP boundary (1.0.0)

Cortex is an incoming ACP stdio agent. It launches Codex and Claude ACP engines
as separate processes and stores task metadata and published Markdown under the
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
independent review. No model hint can waive review. A model tool call cannot
select a task state, accept the task or directly create an engine session.
Plan verification commands execute as literal argv without a shell. Submission rejects command substitution, backticks, redirection, pipes, shell operators and standalone parentheses, so a plan cannot defer that syntax error until verification. Structural validation does not prove semantic correctness of a plan, review or implementation.\n\nA worker `passed` check is accepted only with an exact same-attempt `command_run` or user-confirmed `outside_command` receipt for one completed, nontrivial operation, complete untruncated output and exit code 0. Engine-native shell calls and file reads do not verify checks. Insufficient evidence stays `unverified`; observed nonzero exits stay `failed`. Earlier reports are not rewritten, and older evidence is marked weak when projected under the current rule. Check run history is retained; a failure closes only on a later exact verified rerun that remains the latest result for that command.\n\nTransport state is distinct from task outcome. A plugin disconnect records available session/task/attempt identities and history completeness; restart recovery reads persisted state and leaves actions with unknown outcomes unresolved. Until reload recovery is qualified for the deployed version, update the plugin between active tasks. A host SIGKILL without SIGTERM cannot be recorded, and native engine actions cannot be represented by the retained command receipt mechanism.

The Paseo client wraps its rendered panels, composer popover, and timeline card in per-surface error boundaries. If reading or coercing a thrown value fails while formatting the fallback, it uses `unknown error`. This is a UI recovery boundary, not an execution sandbox or a root-cause diagnosis.

## Tool access and containment

Each channel-bound engine process runs in a Linux `bwrap` engine jail, so its own
native tools (shell, file edit, patch, search, skills) keep working and are bounded
by the operating system rather than disabled. The host is mounted read-only with
`/run/user` hidden (its session bus could start unconfined processes); `/tmp`
stays the host's real, writable, real path deliberately (a screenshot the owner
points at, a file the process leaves for the owner both need the same real
path on both sides of the jail), while `/var/tmp` -- which has no such
interop use, and can be a read-only mount or absent entirely once the whole
host is bound read-only -- is instead a private, writable, per-run directory
backed by the process's own scratch, never the host's shared one; `HOME` and
`TMPDIR`/`TMP`/`TEMP` are that same private scratch. The project is read-only
except the assignment's scopes, `.cortex` is
hidden (for a task bound to a linked worktree, the jail root is that worktree
and the main repository's shared `.cortex` is masked too) and `.codex`,
`.claude` are read-only; `.git` is read-only except the
repository data git needs for a writing worker (see
[Git and tool state in the project](#git-and-tool-state-in-the-project)). The coordinator, reviewers and
read-only roles get no writable scope. The engine's state directory (`CODEX_HOME`
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
servers (strict MCP config; native Codex MCP entries disabled), hooks and plugins.
The host network is shared, which also exposes abstract Unix sockets of the host
network namespace. The engine binary itself is trusted, runs as the owner and can
read what the owner can read.

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

File writes use scope checks, no-follow descriptors, expected-content hashes and
atomic replacement. Commands run under Linux `bwrap`: project read-only by default,
only granted files/directories writable (the whole project for a writing worker),
private `.cortex`, `.codex` and `.claude` paths masked, and the project's `.git`
visible as described below. A worker's `command_run` runs as the owner: their user and
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
commands and external servers are terminated on observed revocation. This does not
claim protection against a separate hostile process running as the same OS user and
racing filesystem changes.

### Access mode (Guarded and Auto)

Each task has an access mode (`configuration.access_mode`). **Guarded** is the
default and the behaviour described above: the runtime asks the user on a
permission card before native deletion of declared `rm` invocations (including
direct `rm`, `sudo rm`, `xargs rm`, and similar forms), before `project_delete`,
and before every `outside_write` and `outside_command`; no answer, a timeout, a
disconnect or a refusal means the action does not happen. Non-`rm` commands such
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
entry, not an approval, and is not answered by Auto. The ssh password dialog is a secret
entry, not an approval, and is not answered by Auto.

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
not see it. `project_read`/`project_write`/`project_delete` still refuse every
path through `.git`, `.cortex`, `.codex` or `.claude`, and plan scopes cannot name
them. Tool state inside the project (`.venv`, `node_modules/.cache`,
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
commands, the current project digest, synthesis identity and absence of live
sessions. Workers cannot choose the reviewed revision, weaken check contracts or
write outside their scope. Runtime and independent reviewer effects are separate
from the worker who proposes the implementation.

The runtime creates its project `.cortex` directory owner-only (`0700`) and
refuses a pre-existing directory with broader permissions. The refusal keeps the
path and expected mode and tells the local owner the exact `chmod 700` command;
it does not weaken the check or repair a pre-existing directory automatically.

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
