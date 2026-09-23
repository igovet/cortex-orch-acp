# Permissions and execution containment

The runtime owns action authority. Each engine process runs in an OS jail that
bounds its native tools; drafts, commands claimed as checks and external MCP calls
go through a channel-bound gateway.
A plan proposes scopes; it cannot grant privileges, choose a model or create a
session. Coordinator and consultant are read-only. Reviewers cannot edit source.

## Native engines

Every channel-bound engine process runs in the engine jail
(`orchestration.sandbox.engine_jail`, Linux `bwrap`). The host is read-only with
`/run/user` hidden; `/tmp` stays the host's real, writable `/tmp` (an
interop path the owner and the process both use, deliberately not
private), while `/var/tmp` is a private, writable, per-run directory instead
of the host's (which can be read-only or absent under the read-only host
bind); `HOME` and `TMPDIR`/`TMP`/`TEMP` are a private per-process scratch. See
[SECURITY.md](../../SECURITY.md#tool-access-and-containment). The project is
read-only except the
assignment's scopes (none for the coordinator, reviewers and read-only roles);
`.cortex` is hidden and `.codex`, `.claude` are read-only; `.git` is
read-only except what git itself needs for a writing worker (see
[Git and tool state](#git-and-tool-state)); HOME is read-only apart from its
tool caches. The engine's own state directory stays writable except its session
configuration (settings, hooks, instructions, skills, agents, commands, rules),
which is read-only and created neutral when absent; installed plugins are hidden.
A revoked or expired channel terminates the process. Engines therefore keep their
native tools, `AGENTS.md`/`CLAUDE.md` loading and skills.

With the owner's opt-in `ssh-askpass.json` (`orchestration.ssh_askpass`), the
engine environment and each `command_run` also receive the display session and
`SSH_ASKPASS` pointing at a Cortex-generated dialog script mounted read-only.
ssh prompts then appear on the owner's desktop and the answers bypass the model.
See [SECURITY.md](../../SECURITY.md#optional-ssh-password-dialog).

The supported Codex CLI is 0.155.1 with codex-acp 1.12.0. A verified private
adapter and per-process launcher disable native agents, multi-agent, plugin,
hook and app features and each inherited MCP server; only session-specific
brokered servers are re-enabled. Every turn runs with `dangerFullAccess`
(Codex's own sandbox cannot start in the jail's user namespace, and the jail is
the boundary) and approval `untrusted`, pinned by the adapter patch in
`engine.process`, so each command still reaches the permission bridge. The user's account configuration and credentials
are not rewritten.

Claude ACP 0.78.0 keeps its native tool preset and `settingSources` default and
receives `skills: "all"`, `strictMcpConfig: true`, empty `plugins`, disabled
hooks, and disallowed `Agent`, `Task`, `Workflow` and agent-team tools. Every role
receives the same options; roles differ only in their jail scopes.

ACP permission requests for native tools are allowed, since the jail bounds them,
except a deletion, which the user confirms in Guarded mode (see
[Access mode](#access-mode-guarded-and-auto)); a native subagent request is
refused. Offered MCP allow options are used and the
gateway then checks the actual channel and operation. Display titles and raw
argument text are never an authority. Channeled engine sessions cannot enable ACP
filesystem or terminal bridges, which would act outside the jail. The trusted
engine binary runs as the local user and can read what that user can read.

## Access mode: Guarded and Auto

`configuration.access_mode` holds one mode per task. The user sets it from the
client: Paseo gets ACP `modes` (`guarded`, `auto`) and switches with
`session/set_mode`, which its composer shows as the shield, decorated by the
plugin (`withAccessModeVisuals`: Guarded `ShieldCheck`/safe, Auto
`ShieldOff`/dangerous, neither marked unattended); Zed and other clients get the
`access_mode` config option (category `mode`) and `session/set_config_option`.
No MCP tool reaches it. A choice made before the first prompt waits in the ACP
session and becomes the task's first revision; afterwards each switch is the
task's next append-only revision in `access_mode_events` (origin
`acp_config_option`, `acp_set_mode` or `acp_pre_task_selection`, plus the ACP
session id). No revision, an unknown value or an unreadable table all read as
Guarded, and nothing is stored in user defaults, so every new session starts
Guarded.

| | Guarded (default) | Auto |
| --- | --- | --- |
| Native deletion reported to the bridge | permission card | runtime selects the offered allow option |
| `project_delete`, `outside_write`, `outside_command` (`orchestration.consent`) | permission card | runtime allows |
| No client connected | refused | allowed |
| Worker engine mode | floor: Claude `default`, Codex `agent` | most permissive offered: Claude `bypassPermissions` → `acceptEdits` → `default`; Codex `agent-full-access` → `agent` |
| Coordinator engine mode | floor | floor |
| Jail, `.cortex`/`.git` protection, gateway scopes and role rules, subagent ban, stale-binding refusal, external MCP allowlist | enforced | enforced |

Both approval paths reach one choke point,
`OrchestratingRuntime.permission_requester_for_task`; in Auto it returns
`access_mode.auto_approve` instead of the connection's host requester. Every
refusal that is not the user's to give is decided before that point and is
unchanged. The worker mode is chosen in
`EngineSessionManagerAdapter._apply_worker_mode_floor` and confirmed over the
wire exactly like the floor (confirm-or-fail). With Claude
`bypassPermissions`, the engine no longer sends permission requests, so the
bridge no longer sees that worker's native calls; subagent tools are still
removed at launch. Codex keeps sending requests because its approval policy is
pinned. A worker keeps the engine mode it was confirmed in: switching back to
Guarded affects new cards and new workers, not an already running permissive
worker. Automatic answers are recorded by tool-call id and kind only.

Covered by `tests/test_access_mode.py` (default Guarded, recorded switches,
refusal of non-user origins, auto-approval with no host, consent in both modes,
subagent/stale-binding/gateway refusals in Auto, worker mode confirmation) and
`integrations/paseo/server/provider-access-mode.test.ts`. Paseo's actual shield
rendering and an Auto run against a paid engine have not been observed live.

## Channel and filesystem authority

Before each operation the gateway checks task state, ownership epoch, channel
revocation/expiry and the current running assignment in SQLite. Rights are derived
from that assignment, never from model-provided role, task or capability fields.
An operation cannot expand its scope. Steering revokes old worker channels before
its text is delivered to a model. Scope/replan/escalation requests are proposals;
the replacement plan is validated again.

`project_read` is cursor-bounded. `project_write` and `project_delete` require the
exact existing SHA-256; creation requires `absent`. File operations use no-follow
directory descriptors and reject symlinks, hardlinks and private components
`.cortex`, `.git`, `.codex`, `.claude`. `draft_write` accepts only the exact owned,
unpublished draft ID and marker. Publication rechecks channel authority inside its
transaction; published reports are immutable.

`command_run` requires Linux bubblewrap. It runs as the owner (read-only real
HOME with its tool caches writable, credential-free environment, shared network)
and mounts the project read-only except the grant: the whole project for a
writing worker, nothing for reviewers and read-only roles. `.cortex`, `.codex`
and `.claude` are masked, including nested ones; the project's `.git` is visible
(see below). A command runs until it exits unless it sets `timeout_seconds`, and
is killed when authority is revoked. There is no unsandboxed fallback.
`timeout_seconds` advertises only a minimum of 1 on `command_run` and
`outside_command`; the runtime stays unbounded by default (omit the field to
run until the command exits). An enormous value is not refused on the field —
it is clamped internally to a ~68-year ceiling (`gateway.TIMEOUT_CEILING_SECONDS`,
`2**31-1` seconds) before it reaches the deadline/subprocess-timeout arithmetic,
so it can never overflow or crash; it behaves as effectively unbounded.
An invalid argument is refused on its exact field (`invalid_arguments`) before
anything starts. A failure once the command may have started (for example its
retained output could not be written), or an unexpected runtime exception, is
`storage_error` with `write_state: uncertain` and `field: cause` naming the
exception class (plus an `OSError`'s reason); the traceback goes to the
engine's stderr log, and the request key is fenced so it is never re-run.
Automatic verification may write only the directories the project's git ignores
(caches, builds, installed dependencies); the project's `.git` is masked there.

### Git and tool state

Git run by a writing worker -- through `command_run` or its native shell in the
engine jail -- updates the project's own repository
(`orchestration.sandbox.git_metadata_mounts`), so commit, branch, switch, stash,
merge and other operations that change only repository data work (the tests
cover commit, switch, stash and merge; fetch/push additionally need the owner's
network credentials and are not covered). This applies only when the project root
is itself a checkout (a `.git` directory, or a linked worktree's `.git` file) and
the whole project is granted:

| Path | `command_run` / native shell (writing worker) | `project_write` / `project_delete` / `project_read` |
| --- | --- | --- |
| `.git` data: objects, refs, logs, index, HEAD, packed-refs, locks, merge/rebase/stash state, `info/` | writable | denied |
| A linked worktree's own `<common>/worktrees/<name>/` and its common directory (even outside the project) | writable | denied |
| `config`, `config.worktree`, `hooks/`, `commondir`, a worktree's `gitdir`, `remotes/`, `branches/` in those directories | read-only | denied |
| `core.hooksPath` when it names a directory inside the project (e.g. husky) | read-only | ordinary project file rules |
| Other linked worktrees' metadata, `modules/` (submodule repositories) | read-only | denied |
| The project's `.git` for reviewers and read-only roles | read-only | denied |
| `.cortex` | hidden | denied |
| `.codex`, `.claude` | hidden in `command_run`, read-only in the native shell | denied |
| Tool state in the project (`.venv`, `node_modules/.cache`, `.pytest_cache`, `__pycache__`, `.mypy_cache`, `target/`, `build/`, ...) | writable | allowed (ordinary project files) |

Those protected git entries configure or redirect git runs the owner makes later,
outside any sandbox (hooks, fsmonitor, editor, pager, filters, credential helpers,
includes). An existing one is mounted read-only. An absent one cannot be covered
by a mount without creating it in the owner's repository, so a guard
(`GitGuard`) checks every half second while the sandbox lives and once more when
it ends, removes any that appeared and, for `command_run`, reports them and fails
the command with `git_configuration_protected`. A process that deliberately
detaches from a `command_run` and outlives it is no longer swept (see
[SECURITY.md](../../SECURITY.md#git-and-tool-state-in-the-project)). Changing git
configuration, installing hooks or adding worktrees/submodules is done by the
owner, or with `outside_command`, which the owner confirms. The read-only
`project_git` status/diff observation is unchanged.

Tool state needs no list: a writing worker's grant is the whole project, and only
the private names above are withheld. A regular file with more than one hard link
stays read-only (its other link may be outside the project), and a directory with
more than 256 of them (a pnpm `node_modules`, a hardlinked uv `.venv`) is
read-only as a whole; reinstall such a tree from the owner's shell or with
`outside_command`.

Every side effect has a durable started receipt before execution. Identical retries
return the retained result. Conflicting keys fail. An interrupted operation with
an unknown outcome is never automatically executed again. Complete command bytes
are stored as an artifact; a passed check must reference a completed, successful
command receipt belonging to the exact worker attempt. An MCP success envelope,
model text or missing signal cannot establish a passing check.

## External MCP

Only the owner-global registry may grant external servers. Project files cannot
install a server. Each enabled entry must list exact `allowed_tools`; optional
`read_paths`, `env` and `network` are explicit administrator grants. Read-only roles
can invoke only the subset named in `read_only_tools`. MCP annotations never grant
rights. Coordinators receive no external execution servers.

The engine receives a private broker endpoint, not the original executable or
credentials. The runtime proxy starts the server in the same scoped sandbox,
filters discovery and calls, rejects server-initiated sampling/elicitation, binds
request receipts to the actual channel and kills the child on revocation. A partial
JSON line has the same deadline as a complete response. An unresolved receipt
blocks replay. A read-path grant cannot overlay the project or its private masks.

## Evidence limits

The no-paid suite exercises real Linux sandboxes, real MCP subprocesses, protocol
peers and installed startup/configuration surfaces. It cannot prove a GUI client
rendered a permission choice or that a paid model obeyed its advertised tools.
See [implementation status](runtime-rewrite-status.md),
[security](../../SECURITY.md), and the [real-client runbook](real-client-runbook.md).
