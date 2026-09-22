# Permissions and execution containment

The runtime owns action authority. Each engine process runs in an OS jail that
bounds its native tools; drafts, commands claimed as checks and external MCP calls
go through a channel-bound gateway.
A plan proposes scopes; it cannot grant privileges, choose a model or create a
session. Coordinator and consultant are read-only. Reviewers cannot edit source.

## Native engines

Every channel-bound engine process runs in the engine jail
(`orchestration.sandbox.engine_jail`, Linux `bwrap`). The host is read-only with
`/run/user` hidden and a private `/tmp`; the project is read-only except the
assignment's scopes (none for the coordinator, reviewers and read-only roles);
`.cortex` is hidden and `.git`, `.codex`, `.claude` are read-only; HOME is
read-only. The engine's own state directory stays writable except its session
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
brokered servers are re-enabled. Every turn runs with `dangerFullAccess` and
approval `never`: Codex's own sandbox cannot start in the jail's user namespace,
and the jail is the boundary. The user's account configuration and credentials
are not rewritten.

Claude ACP 0.78.0 keeps its native tool preset and `settingSources` default and
receives `skills: "all"`, `strictMcpConfig: true`, empty `plugins`, disabled
hooks, and disallowed `Agent`, `Task`, `Workflow` and agent-team tools. Every role
receives the same options; roles differ only in their jail scopes.

ACP permission requests for native tools are allowed, since the jail bounds them;
a native subagent request is refused. Offered MCP allow options are used and the
gateway then checks the actual channel and operation. Display titles and raw
argument text are never an authority. Channeled engine sessions cannot enable ACP
filesystem or terminal bridges, which would act outside the jail. The trusted
engine binary runs as the local user and can read what that user can read.

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

`command_run` requires Linux bubblewrap. It mounts the project read-only and grants
write access only to existing paths within the assignment scope. Private paths
are masked, including nested private directories. User home, inherited credentials
and network are unavailable. New root files/directories use the file gateway.
Commands have a 300-second ceiling and are killed when authority is revoked.
There is no unsandboxed fallback. Verification uses a wholly read-only project.

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
