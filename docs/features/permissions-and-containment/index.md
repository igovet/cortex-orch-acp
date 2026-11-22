# Permissions and execution containment

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed. The former Python runtime was removed on 2026-11-16: references to its modules and tests were dropped, and statements that cited them describe behavior carried from that implementation that has not been re-verified against the Rust runtime.


The runtime owns action authority. Engine processes are not sandboxed; the engine
session mode, its disabled native tools and the runtime's answer to each native
permission request bound native tools (see [Native engines](#native-engines)); drafts,
commands claimed as checks and external MCP calls go through a channel-bound gateway.
A plan proposes scopes; it cannot grant privileges, choose a model or create a
session. Coordinator and consultant are read-only. Reviewers cannot edit source.
Database-architect and performance-engineer are read-only specialists too.

Permission card command subjects JSON-quote empty arguments and arguments
containing whitespace or control characters, including newlines and tabs.
A subject longer than 600 characters has an explicit omitted-character count
in the card title; `rawInput.subject` retains the complete subject. This is a
display safeguard, not a change to the command or authorization rules.

Standalone extracted Dev qualification exercises the native read-only verifier
inside the candidate service process before desktop input. It fails on an
incomplete receipt rather than substituting a successful bare-host check. See
[extracted Dev readiness gates](../../cortex-workspace/testing-and-acceptance.md#extracted-dev-readiness-gates).

## Discovery and bounded read batches

[The additive read API](../tool-discovery-and-read-batch/index.md) requires a
live task and, for workers, a running resolved assignment. It intersects the
current offered catalogue with an explicit evidence-read allowlist; descriptive
read-only annotations grant nothing. Every inner read retains native task/owner
binding and schema validation. Authority loss or role/stage catalogue change
aborts delivery, not just the next read. Existing individual historical evidence
reads keep their existing behavior. No evaluator or new containment claim follows.

## Native engines

No engine process runs in an OS jail: Claude Code, codex-acp and Codex App Server
run as the owner with the launch environment's `HOME` and the connection's native
directory, can read what the owner can read, and share the host network. What
bounds a model's native tools is the engine session's mode and tool
configuration, set in `crates/workspace-service/src/native_engine_policy.rs`, and
the runtime's answer to each permission request the engine sends
(`classify` in `crates/cortex-engine/src/permission.rs`). Project changes and
commands are meant to go through the scoped Cortex tools (`command_run`,
`project_write`, `project_edit`, `project_delete`, `outside_write`,
`outside_command`), which check role, assignment, deny-write paths, deletion
consent and protected targets and record receipts. See
[SECURITY.md](../../../SECURITY.md#native-engine-tools).

**Engine session mode.** The mode depends on the channel's capability, never on
the access mode, and no session gets `bypassPermissions` or `agent-full-access`:

| Channel | Claude (claude-agent-acp) | Codex (codex-acp) | OpenCode |
| --- | --- | --- | --- |
| Coordinator, consultant, read-only profile, any review-stage session | `default` | `read-only` | `cortex` |
| Writing worker (any access mode) | `default` | `workspace-write` | `cortex` |

A read-only session whose adapter offers no such mode does not start; the
`NATIVE_SESSION_START_FAILED` event records `restricted_mode_unavailable`. The
chosen mode is recorded as `NATIVE_ACCESS_MODE_CONFIRMED`.

**Claude.** Every Claude session Cortex starts (coordinator, workers, reviewers,
sprint planning and short model texts) passes `session/new`
`_meta.claudeCode.options`: `disallowedTools` removes native `Bash`,
`BashOutput`, `KillShell`, `PowerShell`, `Edit`, `MultiEdit`, `Write`,
`NotebookEdit`, `Skill`, the subagent, scheduling, worktree, plan-mode,
task-stop and outward notification tools from the model's context;
`allowDangerouslySkipPermissions: false` makes bypass unavailable;
`strictMcpConfig: true` keeps only the MCP servers Cortex attaches (the `cortex`
relay and the owner-registered external servers attached to that channel);
`plugins` and `skills` are empty; hooks and auto-memory are off. Native `Read`,
`Grep`, `Glob`, `WebFetch` and `WebSearch` remain. Claude Code reads and searches
inside the project without asking; session deny rules keep `.cortex/**`,
`.cortex-dev/**` and the dev state files out of those reads. Every other native
request reaches the runtime.

**Codex.** The native Codex launcher forces `shell_tool` and `view_image` off,
with subagents, plugins, hooks and apps, verified with `codex features list`
before App Server starts, and codex-acp's thread config carries the same
switches, so Codex runs commands and reads images only through Cortex tools.
Native `apply_patch` stays on (Codex has no supported switch for it); Codex has
no native read tool. In `read-only` every `apply_patch` asks and is refused. In
`workspace-write` (a sandbox whose writable root is the project, with approvals
sent to Cortex rather than to Codex's automatic reviewer as in `agent`)
`apply_patch` edits inside the project without asking, and every escalation it
asks for (a write outside the writable root, an additional sandbox permission) is
answered by the runtime. codex-acp offers no way to exclude a path from the
writable root, so an in-project `apply_patch` into `.cortex`, `.cortex-dev`, a
credential file or a Git control file gets no runtime decision and is not
prevented. Codex's `unified_exec` feature stays listed as enabled: no supported
switch of Codex CLI 0.166.0 changes it, and whether it offers an exec path while
`shell_tool` is off is not established without a live check.

**Runtime answers.** Cortex gateway and attached external MCP tools pass for
every channel, because the gateway or relay re-checks the channel and operation.
Native subagent and question tools, and every non-Cortex tool of the consultant,
are refused. Every other native request of Claude and Codex is decided by this
table (`classify` in `crates/cortex-engine/src/permission.rs`, path classes in
`native_engine_policy.rs`):

| Target | Guarded | Development | Auto |
| --- | --- | --- | --- |
| Read or search inside the project | allow | allow | allow |
| Read or search outside the project | ask the owner | allow, recorded | allow, recorded |
| Write or edit inside the project (writing workers) | allow | allow | allow |
| Write or edit outside the project (writing workers) | ask the owner | ask the owner | allow, recorded |
| Deletion | ask the owner | ask the owner | allow, recorded |
| Credential and key material anywhere, read or write | ask the owner | ask the owner | allow, recorded (`auto_credential`) |
| Git control files, engine and Cortex configuration, write | ask the owner | ask the owner | allow, recorded (`auto_control`) |
| Cortex private state (`.cortex`, `.cortex-dev`, dev owner files, service state), read or write | refused | refused | refused |
| Web fetch | ask the owner | allow | allow |

Read-only roles and review-stage sessions get only the read rows and the web-fetch row:
their native write, delete and execute requests are refused in every mode under the
`read_only` rule, while a web fetch, which only reads, follows the web-fetch row like any
other channel's. The table narrows native engine tools only; a `command_run`
command can still read anywhere the owner can. "Ask the owner" opens a card on the
connected owner client titled `Owner decision: <role> wants to <operation>
<paths> because <reason>` with only Allow once and Deny; the engine's request
waits until the owner answers, and a denial goes back as the engine's reject
option. Each answer is a `NATIVE_PERMISSION_DECIDED` or
`NATIVE_PERMISSION_CANCELLED` event with its rule and decider (`owner`,
`mode:<access mode>`, `runtime`, `withdrawn:<reason>` or `no_owner_client`) and an
owner-decided one also leaves a `permission_receipts` row. A request for a channel-bound MCP tool (the `cortex` gateway or an attached external
server, rule `channel_mcp`), which the access mode allows and which re-checks the
channel inside the call, is recorded as one `NATIVE_PERMISSION_AUTO_ALLOWED` event
(request fields, outcome, rule and `mode:<access mode>` decider) written in one
transaction with the channel, session and access-mode checks; every other request
keeps its `NATIVE_PERMISSION_REQUESTED` record and its separate decision record.
These channel-tool allowances were more than half of all runtime events. An incoming ACP client
(Paseo, Zed) shows the card itself; in a native Workspace task it is an owner
decision of kind `permission` in the task's Conversation ("Asked by <role>", the
paths, why, Allow once and Deny), counted as needing the owner and raised as a
desktop question notification. A pause, stop or ended session withdraws it and
cancels the engine's request, and a service start withdraws any left open. Neither
engine adapter limits how long a permission request may wait. Cortex's own
`outside_write` and `outside_command` use the same table and card. OpenCode requests outside Auto that
the table does not settle keep the client route. Which cells each engine can
enforce is listed in [SECURITY.md](../../../SECURITY.md#native-engine-tools).
Display titles and raw argument text are never an authority. Channeled engine
sessions cannot enable ACP filesystem or terminal bridges.

The project is read-only through Cortex tools for the coordinator, reviewers and
read-only roles. Writing workers' declared scopes coordinate parallel work but do
not restrict the gateway's project write paths. A revoked or expired channel
terminates the process. The one bounded exception is a worker channel revoked by
acceptance of that attempt's own current (non-superseded) result: the 1 s
authority watchdog lets the in-flight turn end for at most 20 seconds, so its
final token report can arrive, before it terminates the engine. The post-result
settlement normally tears the engine down as soon as the turn ends and within 15
seconds (see
[token coverage](../token-and-plan-usage/index.md#coverage-and-interrupted-turns)).
Native tools can still act in that window; late project changes invalidate the
digest-bound review, verification or completion gate. Any other revocation, an
unprovable result cause and explicit cancellation, stop or replanning still
terminate at once.

The engine environment and each `command_run` receive the owner's own
ssh-agent (`SSH_AUTH_SOCK`, only a socket the owner owns), so ssh uses the
host's keys and `~/.ssh/config`. Root-owned ssh config includes that ssh
trusts on the host are mounted as read-only, owner-owned copies, because the
sandbox's user namespace shows root's files as `nobody`'s; see
[SECURITY.md](../../../SECURITY.md#owners-ssh-agent).

With the owner's opt-in `ssh-askpass.json`, the
engine environment and each `command_run` also receive the display session and
`SSH_ASKPASS` pointing at a Cortex-generated dialog script mounted read-only.
ssh prompts then appear on the owner's desktop and the answers bypass the model.
See [SECURITY.md](../../../SECURITY.md#optional-ssh-password-dialog).

The pinned Codex adapter is codex-acp 2.6.0 (`@openai/codex` `^0.166.0`);
codex-acp 1.12.0, 1.13.0, 2.0.1, 2.1.1 and 2.3.0 remain qualified. A session with
role instructions also receives the [unified system prompt](../unified-system-prompt/index.md).
The owner's account configuration and credentials are not rewritten.

The optional OpenCode v2 engine is described on the
[OpenCode adapter](../opencode-engine/index.md) page; its `cortex` agent asks for
every tool.

Cortex supplies its whole configuration: one `cortex` agent with
`"*": "ask"` and subagents, questions and skills denied, and OpenCode's built-in
`opencode.agent`/`opencode.plan` plugins removed (no plugin is loaded). From the hidden owner
configuration it carries only provider definitions and the provider on/off
lists, with configured credentials mounted read-only from memory rather than
placed in the environment. OpenCode opens its native `opencode.db` in place;
Cortex neither extracts database logins nor selects an alternate database.
The native database remains writable for OpenCode session state. With no
built-in primary agent, the installed 2.0.20 ACP catalog waits for `cortex`;
an observation without `cortex` is still rejected, never used to run a
`build`/`plan` turn.
Project configuration
and auto-update are off, and the data directory's `bin` is read-only. MCP tools,
including the Cortex gateway, are reached through its `execute` (Code Mode)
tool: one permission request per `execute` call, with each Cortex MCP call still
checked by the gateway. A refused permission ends OpenCode's turn as
`cancelled`; the runtime continues the session after its own refusal.

<a id="access-mode-guarded-and-auto"></a>
## Access mode: Development, Guarded and Auto

Fresh ACP sessions start **Development**, durably recorded with session creation.
Paseo receives ACP `modes` and `session/set_mode`; other clients receive the
`access_mode` config option. The existing plugin forwards unrecognized mode
ids unchanged; Development has no custom plugin visual in this release. No
rendered client claim follows from that source behavior.

Only client controls can select a mode. The additive
`acp_session_access_defaults` record does not backfill old sessions or rewrite
user-wide settings. Pre-task choices persist and win at atomic, idempotent task
binding. `access_mode_events` records initialization and later switches.
Load/reconnect preserves durable choices. Legacy sessions/tasks without a record,
non-ACP task creation, unknown values or unreadable configuration stay Guarded.
No model-facing mode or execution-profile setter exists.

The three modes are trust levels over two axes: where a writing worker may
work, and whether a deletion waits for the user.

| Behavior | Guarded | Development (fresh-session default) | Auto |
| --- | --- | --- | --- |
| Where a writing worker works | the project | the whole computer | the whole computer |
| Writing-worker `command_run` | restricted mounts; cwd inside the task tree | host-writable masks, any existing cwd, no card | same as Development |
| `outside_write` | consent card | consent card | runtime answers the card itself (Cortex private state refused in every mode) |
| `outside_command` (no sandbox) | consent card | consent card | runtime answers the card itself |
| Writing-worker `command_run` that needs the host (`sudo`, sandbox unavailable, launcher failure proven before release) | rerun through `outside_command`: consent card | same: consent card | same: runtime answers the card itself |
| `sudo` in a host command | owner password prompt on the desktop or a paired phone | same | same |
| Declared deletion (`project_delete`, `command_delete`, native delete) | consent card | consent card | runtime answers the card itself |
| Reads by any `command_run` command (every role, every mode) | anywhere the owner's account can read, no card (the sandbox masks only Cortex private state) | same | same |
| Read-only/reviewer `command_run` (every read-only role, every mode) | read-only profile (`read_only`); cwd inside the task tree | read-only profile (`read_only`); any existing cwd | same as Development |
| Automatic runtime verification | read-only profile (`read_only`); git-ignored directories writable, other project writes discarded with the check | same read-only profile | same read-only profile |
| Engine session mode (see [Native engines](#native-engines)) | by role, not by mode | by role, not by mode | by role, not by mode; no bypass or full access |
| Channel, role, revocation, owner deny, review and completion gates | retained | retained | retained |

Automatic runtime verification mounts the project as a private overlay (upper layer on disk in
the command's own scratch directory, never RAM-backed, removed with it) over its
pinned read-only source, with the git-ignored directories,
private-name masks and git metadata guards bound on top as before. A check may therefore run a
tool that rewrites a tracked file on every run (`next typegen` rewrites `next-env.d.ts`); the write
lands in the overlay and is gone when the check ends, so the reviewed sources and revision never
change. Observed on 2026-10-21: before this, such a check failed with `EROFS` in every access mode
and after the user's consent. Support is probed once per process (bubblewrap 0.10+ with
unprivileged overlayfs); without it the project mount stays plain read-only and the write is
refused. No access mode, permission or user consent changes the verification profile, and the
`verification_submit` contract tells the coordinator not to ask the user to authorize it.

The read-only profile (receipts name it `read_only`; the Guarded writer profile is `restricted`
and the host-writable Development/Auto profile is `development`) mounts the whole host read-only.
HOME is read-only except an explicit allowlist of tool caches (for example `.cache`, `.npm`,
and the Docker buildx activity records). Scratch is private, and the command runs in its own
PID namespace. Through these mounts a read-only role's command cannot change the project tree,
HOME startup files outside the cache allowlist, or other host files, in any access mode. Its
project writes (vitest's `node_modules/.vite-temp`, Playwright results) land in a private
disk-backed overlay discarded when the command ends (`discard_project_writes`; refused
where the host cannot overlay), so a reviewer's suite runs and the project keeps nothing.
AppArmor's world-writable query file `/sys/kernel/security/apparmor/.access` is bound
writable so a private `dbus-daemon` (`dbus-run-session`, a nested desktop shell) can accept
connections; read-only, it refused them and a desktop placement check failed only in
verification (2026-10-22). The
following paths are exceptions: they stay writable or reachable from a read-only command:

- the host `/tmp`, bound writable, so files and sibling projects under `/tmp` can be written;
- the session D-Bus and the systemd user manager under `/run/user` (audio, display and keyring
  sockets too), so `systemd-run --user` and similar calls start processes outside the sandbox;
- the host `/dev`, bound with its real devices;
- a Docker socket, if the host has one: a read-only mount does not stop mutating Docker requests;
- the owner's network, which the profile shares by owner decision.

Cache writes remain possible, so a read-only command can still poison a cache that a later run
reads. This narrows exposure; it is not hostile-code isolation. Automatic runtime verification
uses this same profile in every access mode (see [the decisions log](../../project/decisions.md#read-only-roles-and-the-command-host-profile)),
so its writes are limited to the git-ignored project directories, the tool-cache allowlist,
scratch and the host `/tmp`, subject to the reachable paths above. The profile limits writes, not
network access: as with every owner command, the owner's network is shared.

A missing, refused, cancelled or timed-out answer to a consent card blocks the
effect. Protected control-plane paths for `outside_write` are the
`control_plane_paths` locations (Cortex, Paseo, Codex, Claude and
OpenCode state and configuration, `~/.gitconfig`, `~/.git-credentials`, the
installed runtime, the project's `.cortex` and an external hooks directory),
existing or not, and anything beneath them, including through a symlinked parent. The
worktree's own git metadata directories (its gitdir and, for a linked worktree, the common
directory) are protected too. An outside-write target whose resolved real path lands inside
the project is refused as `outside_path_inside_project`, even when its literal path does not.

Development is trusted local development, **not a hostile-code sandbox**.
`development_command` uses `--bind / /`, real HOME and /var/tmp,
owner devices, groups and shared network. It retains task-private, Git and
denied-path overlays and protects known existing runtime/configuration/native
engine locations and the production Paseo home/profile. Parent protections precede
the bound task's grant, allowing host-created worktrees under those parents.
Missing or invalid explicitly configured engine-state targets fail closed.
Optional locations absent at preparation are not a promise about future files.

Docker resolves paths on the daemon host; existing socket access is already
possible with read-only client mounts. Daemon privilege, rootless/remote context,
host aliases, outside hardlinks, shared process access and arbitrary code can
circumvent direct masks. No privilege escalation, endpoint substitution,
credential copying or daemon installation is implicit. Docker login and
credential/configuration changes are separately authorized work.

Host capability never authorizes deletion, production/shared-system mutation,
publication, credential changes or paid checks. Declared `rm` commands receive
gateway consent via `command_delete`; denial starts no process. Read-only
roles cannot use a declared deletion command, even with owner approval.
The gateway preserves argv boundaries: literal `echo`/`printf` arguments containing
`;`, `|`, newlines or `rm` text are not executable declarations. It recognizes
`env`, `sudo`, `xargs` and `exec` argv wrappers, `find -exec`/`-execdir`, and explicit
`sh`/`bash`/`dash`/`zsh` command scripts, including leading script whitespace.
Shell option operands such as `-o pipefail` are skipped before locating `-c`.
Supported wrappers also apply inside explicit shell command positions, including
`sh -c 'env rm file'` and nested `env dash -c 'rm file'`. Simple unquoted
assignment prefixes and command-position control words (`if`, `then`, `elif`,
`else`, `while`, `until`, `do`, `!` and groups) are recognized before argv
dispatch. Examples include `sh -c 'LABEL=fixture rm fixture'` and
`sh -c 'if true; then env rm fixture; fi'`. Quoted/escaped prefix-like words
and ordinary arguments are not shell prefixes. Rejected consent precedes
working-directory preparation, command mounts and process launch.
Shell text remains lexical matching, not complete shell interpretation. The
classifier is not a semantic firewall for interpreters, scripts or Docker effects.
Auto uses the same host-writable `development_command` profile as
Development (receipts name it `development`) and keeps its automatic approval
and engine-mode confirmation. No access mode selects Claude `bypassPermissions` or
Codex `agent-full-access`, and no Cortex Claude or Codex session has a native
shell, so commands go through the brokered gateway tools.

Commands retain cwd/profile/policy provenance. An observed change of the task's access
policy (a mode switch) or of its write denials terminates a host-writable (Development or
Auto) writing-worker command's owned command group with incomplete
`execution_policy_changed` evidence. Read-only commands (every read-only role's command and
automatic verification, which has the `read_only` profile in every mode) and Guarded writer
commands are not stopped by such a change: a read-only command's profile does not depend on
the mode, so its mode switch leaves it running. Revocation/cancellation and timeouts still
stop the group. Neither termination nor switching to Guarded rolls back files,
detached processes, containers, volumes or daemon builds. No incomplete command
is automatically rerun. Earlier explicit Auto workers keep their confirmed
engine mode until they end.

Coverage: the `workspace-service` and `cortex-engine` Rust tests for access mode, development commands and gateway/permission/evidence, and the real-namespace cases. Test-only
unjailed launch replacement establishes receipts/cancellation, not containment
or real Docker operation. A namespace skip is an unverified containment result;
no-paid smoke and source tests do not qualify a rendered client or paid engine.


## Channel and filesystem authority

Before each operation the gateway checks task state, ownership epoch, channel
revocation/expiry and the current running assignment in SQLite. Rights are derived
from that assignment, never from model-provided role, task or capability fields.
Writing workers receive project-wide gateway write authority, except private and
user-protected paths; a declared assignment scope is a coordination work area,
not an enforced project-path boundary. Writers reserve additional paths with
`access_request` before changing them so parallel assignments remain coordinated. A directory
path with one trailing slash (`.next/`) names that directory, and `[`/`]` are literal path
characters (Next.js routes such as `[locale]/[slug]` were always refused before); `*` and `?`
and other non-canonical forms (`dir//`, `dir/./x`) remain `unsafe_path`. An absolute path or
one with `..` is `outside_project`. Every denial carries `guidance` on what to do next, so a
reason code is not read as "the work is forbidden". Generated build/test output (build and cache
directories, test reports, a regenerated type stub such as `next-env.d.ts`) needs no request:
a writer's `command_run` already writes it in Development and Auto, and runtime verification
discards its own generated writes. On 2026-10-21 a worker asked in advance for `.next/` and
`test-results/`, received `unsafe_path` for the trailing slash and stopped without running the build.
Owner deny-write entries come only from an explicit `deny-write: <path>` instruction (bare,
single-quoted, double-quoted or backtick-quoted). Natural-language requests such as "do not
modify X" create no entry, and a value containing wildcard or mask characters is skipped
when parsed. Entries are stored in normalized form and checked after the same normalization
for `project_write`, `project_edit`, `project_mkdir` and `project_delete`, both at dispatch and
at the filesystem effect. An entry stored by an earlier runtime that contains `*`, `?` or `[`
still guards writes: it is matched conservatively as a glob against both the normalized and
the raw path, and the command sandbox keeps every stored entry that exists when a command starts read-only, wildcard or not. An exact entry naming a path that does not yet exist is not bound for commands, so a command can create that path; the gateway file tools still refuse it (see the `.claude` and `.codex` paragraph in `SECURITY.md`).
Writer conflicts retry automatically on a bounded 1/2/4-second backoff, then
receive a runtime grant or denial without a user question. Unsafe paths, owner
deny-write policy and read-only roles receive immediate runtime denials. Retained
old access cards are cancelled; user text cannot grant or override access.
See [the access wait contract](../orchestration/index.md#worker-questions-and-additional-access).
Read-only roles and review stages have no project write grant. Steering revokes
old worker channels before its text is delivered to a model. Scope/replan/escalation
requests are proposals; the replacement plan is validated again.

`project_read` pages text by 1-based line. `offset` is a line number (default 1)
and `limit` is at most 2000 lines (the default and the maximum). `line_numbers`
defaults to true, which prefixes each line with its number and a tab; `false`
returns the raw text. Line numbers never belong in `project_edit` strings or in
`project_write` content. Each text result stays under 24,000 serialized bytes,
counting the duplicated structured copy. `start_line`, `end_line`, `total_lines`,
`complete`, `partial`, `next_offset`, `start_column`, `next_column` and
`truncated_lines` report what was returned. A page returns at most 2000
characters of one line (fewer when the line's serialized text exceeds the inline
budget). A longer line ends its page: it is listed in `truncated_lines`,
`next_offset` names that same line and `next_column` the first character not yet
returned. Passing both as `offset` and `column` continues the line, so following
the cursor returns every character exactly once. A line counts as read only after
all of its segments were returned in order from its first character; a segment
that starts past what was already returned adds nothing. Only the per-line count
of characters returned is stored, never text. UTF-8 regular files
return text and a SHA-256 that identifies the bytes read. A supported image (PNG,
JPEG, GIF or WEBP, sniffed by content, not extension), up to a 10 MiB cap,
returns a real MCP `image` content block plus its SHA-256, byte size, MIME type
and path as text (`gateway_image_too_large` above the cap). Any other non-UTF-8
regular file returns only its SHA-256, byte size and `binary: true`. A text file
above 16 MB is refused (`gateway_file_too_large`).

Read coverage is kept per channel. Ranges of completely returned lines accumulate
across pages only while the file's digest is unchanged. Only digests and line
ranges are stored, in the runtime request ledger (`project_read_state`), never
file bodies. Coverage is capped at 2048 merged ranges, so a fragmented read
cannot become complete. `full_content_sha256` is returned only when this
channel's coverage of the current bytes is complete. A partial page's `sha256`
is identity only and never authorizes an overwrite. A successful `project_write`
or `project_edit` replaces the path's coverage on that channel with the digest
of the bytes it just wrote, so the channel's next edit of its own result needs
no re-read (the engines' native edit tools behave the same way, and workers
edited twice in a row and were refused `gateway_read_required` before
2026-10-13). The new coverage is complete after a full write or after an edit
of a completely read file, never for `.gitignore` (the runtime adds protected
lines the model did not send); otherwise it is partial and does not authorize
`project_write`. The mutation receipt reports `read_coverage_complete` and
`full_content_sha256` for that coverage. A change by anyone else afterwards is
still `gateway_stale_read`. Copying a digest from another channel, or computing
one outside the runtime, does not bypass this rule.

`project_read` of a permitted missing path succeeds with `exists: false`,
`sha256: "absent"` and `parent_exists`. This is an observation, never permission:
all private-path and no-follow checks still apply, and real OS read failures
remain `gateway_read_failure`. Pass that SHA value directly to `project_write`
for creation; `parent_exists` is informational. A creation also creates every
missing parent directory (mode 0755): each component is created and reopened
through no-follow directory descriptors only after the private-path, write-grant
and user deny-write checks passed for the full path, and a file or symlink
component is refused with `gateway_parent_not_directory` naming it. A non-`absent`
`expected_sha256` under a missing parent is a `gateway_write_conflict` (field
`expected_sha256`, expected `absent`) and creates nothing. `project_mkdir` also
creates missing parents; an existing real directory returns `created: false`, a
file at the path or at a parent component is `gateway_path_exists`, and a symlink
component is `symlink_refused` (never followed).

`project_write` replaces the whole content of one file and never appends. An
existing file requires `expected_sha256` equal to `full_content_sha256` from
complete coverage on this channel. A partial read, a hand-supplied hash, or
`absent` for an existing file is refused with `gateway_complete_read_required`,
`gateway_stale_read` or `gateway_write_conflict`, and nothing changes. The
receipt reports `previous_size_bytes`, `size_bytes` (also as `byte_count`),
`previous_lines`, `new_lines`, and `drastic_shrink` when the new size is below
half the old one. A shrink is visible in the receipt and is not refused on its
own; complete coverage is still required.

`project_edit` applies one to 64 ordered exact replacements (`edits`, each with
`old_string`, `new_string` and an optional `replace_all`) atomically. It needs a
prior text read on this channel whose digest still matches the file; a partial
page is enough. With no prior read it refuses with `gateway_read_required`, and
with a stale read it refuses with `gateway_stale_read`. Each `old_string` must be
nonempty, must differ from its `new_string`, and must match exactly once unless
`replace_all` is true (`gateway_edit_not_found`, `gateway_edit_not_unique`,
`gateway_edit_no_change`). Matching is exact, including whitespace, and
overlapping occurrences count as ambiguous. Edits apply in order to the evolving
text. The file changes only if every edit succeeds, through the same atomic
writer `project_write` uses. An append replaces a unique tail anchor with itself
plus the added text. The result carries `edits_applied`, the new `sha256`, the
size and line receipt, and a bounded unified diff (`diff`, `diff_truncated`;
small before and after texts are included when they fit). Presentation uses the
exact committed content, including the final root `.gitignore` protection rule;
an edit whose removed rule is restored therefore reports no deletion. Before
calling the diff matcher, combined input is capped at 32,000 characters and the
before/after line-count product at 250,000. Above either budget the edit still
commits, but returns an empty `diff`, `diff_truncated: true` and
`diff_omitted: "input_budget"`; hash, size and line evidence remain available.
Presentation is computed before the filesystem effect. `project_edit` refuses
links, binary files and results above 16 MB.

### Read-precondition recovery

`gateway_read_required`, `gateway_complete_read_required` and `gateway_stale_read`
are recognized non-authorizing preconditions, with `write_state: rejected` on the
MCP error response. No file mutation occurred. They do not require an owner
question, access request or extra approval when the existing assignment and
authority already cover the change:

1. Read the current target with `project_read` on the same channel. For whole-file
   replacement, finish all non-truncated pages and use `full_content_sha256`.
2. Build a valid exact edit or complete replacement from that current content.
   Rebuild a stale patch; never blindly retry it or overwrite concurrent work.
3. Follow the live tool's delivery-key rules below: a keyless precondition failure
   has no replayable effect; a failed explicit key retains its rejection and
   needs a fresh key for the corrected proposal.

These corrections grant no authority. Real access, channel, role, scope,
private-path and policy refusals remain final; never reach the denied effect via
another tool, shell, channel or worker. A read can itself be denied; stop that
route rather than reclassifying it as a missing-read precondition.
The existing digest, coverage and exact-match guards remain authoritative.
`project_files.require_read` and `gateway._edit` enforce these guards; shared tool
discipline (for roles that edit files), the writing-worker prompt in both
unified-prompt editions and writer recovery capsules distinguish this workflow
from an authorization refusal. No new error taxonomy or retry
engine is introduced.

Focused regressions are in the `cortex-engine` gateway and `workspace-service` Rust tests. They exercise real channel dispatch
and stdio recovery without creating a question or consent request; their scripted
calls and instruction checks do **not** prove real-model compliance. Requested
live CLI qualification must use a fresh stamped runtime and exact session
bindings; checkout edits do not hot-upgrade an existing session.

### File-action delivery keys

`project_write`, `project_edit`, `project_mkdir` and `project_delete` accept an
omitted `request_key`: the runtime derives a stable operation/payload hash scoped
to the channel. Identical keyless retries of actual effects reuse the retained
result before checking CAS,
creating a directory or asking for deletion confirmation. Different payloads
are different actions; an intentional repeat of the same payload after other
changes needs a fresh explicit key. A pending/unknown receipt still refuses replay.
`project_delete` requires the exact existing SHA-256, supports binary files and
checks their raw-byte SHA-256 before unlinking, again after the owner's
confirmation. A refusal before any possible side effect keeps no replayable
auto-key receipt, so an identical keyless retry can succeed after correction. An
explicit key retains the refusal with its structured code, field and hint; use a
new explicit key for the corrected action. A declined or unavailable deletion
confirmation (`deletion_not_confirmed`) is retained under either key, so an
identical retry is refused again without asking twice. File operations use no-follow directory descriptors and
reject symlinks, hardlinks and private components `.cortex` and `.git`. The
project's `.claude` and `.codex` follow ordinary project file rules. A private path is refused before anything else, so a deletion there
never asks the user first. `draft_write` accepts only the exact owned, unpublished
draft ID and marker. Publication rechecks channel authority inside its
transaction; published reports are immutable.

### Project skill links

`project_skill_link` is the guarded way to expose
a shared skill to Claude. A writing worker can also change `.claude` with the
ordinary file tools and commands; the operation adds source verification and
collision protection. It creates or removes exactly one directory
symlink, `.claude/skills/<name>` -> `../../.agents/skills/<name>`, so Claude
Code (which loads project skills only from `.claude/skills`) sees a skill kept
in the shared `.agents/skills` directory.

- Who: any worker whose assignment may write; no particular declared scope is
  required because writers receive whole-project authority. The coordinator,
  consultants, reviewers and read-only roles are refused and are not offered the
  tool. A user-protected path matching the link refuses it.
- `name`: 1-64 lowercase letters, digits and single hyphens
  (`skill_link_name_invalid`); it is never a path.
- `link` requires `.agents`, `.agents/skills` and `.agents/skills/<name>` to be
  real directories in the project (no symlinked component) and `SKILL.md` a
  regular file (`skill_source_missing`). `.claude` and `.claude/skills` must be
  real directories and are created when absent; a symlinked or non-directory
  component is never followed (`skill_link_parent_unsafe`).
- Existing entry at the link path: the exact link is left unchanged
  (`changed: false`); a regular file, directory or other non-link is never
  replaced (`skill_link_collision`); a symlink with a different target (stale or
  foreign) is refused with its exact text (`skill_link_occupied`) and replaced
  only when the call passes that exact text as `replace_symlink_target` and the
  user confirms on a permission card (`skill_link_replace_not_confirmed`
  otherwise). A mismatched text never reaches the user (`skill_link_conflict`);
  the link is observed again after confirmation and replaced atomically.
- `unlink` removes only the exact managed link (`skill_link_not_managed`
  otherwise, and `changed: false` when absent); the skill in `.agents/skills`
  stays, so it is not a deletion and asks no confirmation.
- Git: the link text is relative and inside the project, so Git stores it as
  a symlink (mode 120000) that resolves in every clone. A `link` result carries
  `git.link_ignored`/`git.source_ignored` from a read-only
  `git check-ignore --no-index` run with a cleared environment and the owner's
  global and system Git configuration disabled (so it reads the project's own
  `.gitignore` files and `.git/info/exclude`, not a global `core.excludesFile`),
  or `null` outside a checkout or when Git cannot run. The operation never edits
  `.gitignore`, stages or commits; `project_git` status/diff show the link.
- Receipts: like every gateway mutation it has a durable started receipt; a
  failure after a directory or link may have been created is an uncertain
  outcome that is never re-executed automatically.

Covered by the `workspace-service` Rust tests.

`command_run` requires Linux bubblewrap and has no silent unsandboxed fallback.
Its effective mounts depend on [access mode](#access-mode-development-guarded-and-auto).
Writing workers' restricted commands (Guarded) use read-only real HOME with writable tool
caches; in Development and Auto their commands use real writable host resources. Read-only
roles always use the read-only profile described in the access-mode table. Both filter Cortex/credential environment
variables and retain validated owner SSH-agent access and shared network
and mounts the project read-only except for writing workers, who receive the
whole project writable regardless of their narrower declared coordination work
area; reviewers and read-only roles receive no writable project paths. `.cortex` is
masked, including nested ones; `.codex` and `.claude` are ordinary project paths;
the project's `.git` is visible
(see below). A command runs until it exits unless it sets `timeout_seconds`, and
is killed when authority is revoked. There is no unsandboxed fallback.

Sandbox preparation is bounded but tolerant of a busy host: the service waits up to 60
seconds for each step reported by the per-command supervisor, the namespace and overlay
probes up to 20 seconds and the git ignored-path query up to 30 seconds (each step is normally
well under a second). Exceeding a bound still refuses before launch with
`native_command_sandbox_unavailable`; the receipt is settled as never launched, and the
tool's hint tells a read-only role that nothing ran and the identical command may be retried once.
A writing worker's command is instead rerun on the host ([host rerouting](#host-rerouting-and-the-sudo-password-prompt)).
Such a never-launched call does not keep an earlier failed check open (see
[check history](../orchestration/index.md#check-history-and-current-outcome)).

A program inside a command cannot create its own namespace sandbox: Ubuntu's
`bwrap-userns-restrict` AppArmor profile confines everything bubblewrap starts so that it
cannot create another user namespace, whatever the mounts. Nested bubblewrap, flatpak,
`unshare`, rootless containers, Chromium/Electron sandboxes and a nested Cortex therefore
fail inside `command_run`. When the retained output (its first 64 KiB or its last 64 KiB)
shows one of these refusals, the settled receipt carries `nested_sandbox`
(`code: nested_sandbox_denied`, the matched signature, and whether `outside_command` is
offered to the command's profile). The sandboxed receipt's outcome, exit code and evidence
are unchanged. The receipt carries only this advisory hint: nothing is rerun automatically
on the host because of it ([host rerouting](#host-rerouting-and-the-sudo-password-prompt)).
The worker decides whether to ask for a host run, and a read-only role reports the check as
not run with that blocker. The signatures are bubblewrap's, `unshare`'s, a rootless
container runtime's `cannot clone` and Chromium's `No usable sandbox!` messages. A nested
Cortex reports its own `native_command_sandbox_unavailable` or
`native_verification_sandbox_unavailable` code instead and is not matched. Signatures in the
middle of a larger output are not scanned (`nested_sandbox_hint` in
`crates/workspace-service/src/native_command_handles.rs`).

### Host rerouting and the sudo password prompt

A writing worker's `command_run` that needs the host is rerouted in the cases below.
The runtime reruns the identical argv, cwd and timeout through `outside_command`
(request key `<key>.host`; Auto answers its consent card, Guarded and Development show
it with the reason in its title) and returns that host receipt with `rerouted`
(`from: command_run`, `reason` and the original request key) only when:

| Reason | When |
|---|---|
| `privilege` | `sudo`, `pkexec` or `doas` is at a command position of the argv: first, after wrappers such as `env`, `timeout` or `nice`, or in a `sh`/`bash`/`dash`/`zsh` script. Nothing runs in the sandbox first. |
| `sandbox_unavailable` | The sandbox could not be prepared (`native_command_sandbox_unavailable`); nothing ran. |
| Sandbox launcher failed before the payload was released | The private supervisor itself reports, over its control pipe before the final `go`, one of four sandbox-facility failures: Bubblewrap missing, launcher spawn failure, namespace or mount setup failure, or launcher probe timeout. The original row settles as not launched with dedicated supervisor evidence, and only then is the host rerun dispatched once. A supervisor timeout, a lost pipe, an authority failure or an unrelated preparation refusal is not such evidence. |

Once the sandboxed payload has started, the command is never replayed on the host
automatically, whatever its exit status or output. Output that looks like a refused nested
sandbox or a refused root (`nested_sandbox`, `sandbox_privilege`) is child-controlled text,
so it stays an advisory hint on the unchanged original receipt; the worker decides whether
to ask for a host run. Output text never selects the host route.

The sandboxed receipt is kept and records `rerouted_to` when a host rerun was dispatched, so the same request replayed
answers the same host receipt and nothing runs twice. Read-only roles, reviewers and
verification are never rerouted; their receipts carry the `sandbox_privilege` or
`nested_sandbox` hint telling them to report the check as not run. A long command whose
first reply is `running` is not rerouted when it later settles. A consent refusal on the
host route is the outside route's own refusal (`outside_action_not_confirmed`) with
`rerouted` attached; nothing ran.

`sudo` on the host asks the owner. Each host command gets a private askpass route
(`SUDO_ASKPASS`, a `sudo` shim first on `PATH` that adds `-A`, a per-command socket and
token). The service accepts only its own helper started by a root `sudo` and opens a
"Password for sudo" prompt naming sudo's command line, cwd, project, task and worker on
the desktop and on paired phones ([screens](../../cortex-workspace/SCREENS.md#sudo-password-prompt)).
Only that command waits. The password goes to sudo's pipe once and is never stored,
logged, put in an event, receipt or output, or shown to a model; a cancel, 10 minutes
without an answer, or the command ending makes sudo fail normally, and the receipt's
`sudo_prompts` lists each prompt's outcome. `sudo -n` never asks. A phone may send a
password only while the owner's "Passwords and tokens from phones" setting (formerly
"Sudo passwords from phones") is on (default on).
See [SECURITY.md](../../../SECURITY.md#owner-password-for-sudo-askpass-and-host-rerouting)
for the trust boundary (`native_askpass.rs`, `native_commands.rs`). When the task lacks
access instead (a GitHub, Codex or Claude login, or sudo), the owner gets an owner action
card (`native_owner_actions.rs`); see
[owner actions](../../../SECURITY.md#owner-actions-sign-ins-and-pasted-tokens).

A settled receipt whose retained output shows a program that could not be found
(bubblewrap's `execvp NAME: No such file or directory`, or exit 127 with a shell's
`not found`/`command not found` or `env`'s `No such file or directory`) carries
`missing_program` (`code: program_not_found`, the program when it is a plain short name,
the search path it was looked up on and what to do). It is advisory like
`nested_sandbox`: the outcome is unchanged and nothing is rerun
(`missing_program_hint` in the same file).

A launch that never released its payload (`failure: command_not_started`) names why
in the receipt's `cause`: `sandbox_unavailable:<reason>` with the sandbox's own fixed
reason (for example `setup_timeout`, `namespace_or_mount_setup_failed`,
`project_tree_limit`), `sandbox_preparation_timeout` when the supervisor did not answer
within 60 seconds, `supervisor_exited`, `supervisor_spawn_failed`, `store_busy` or
`store_unavailable` when the store refused the authorization or handle commit,
`refused:<code>` for another refusal (for example `refused:access_changed`), or
`call_ended_before_release`. Preparation stage durations (milliseconds) and the
scanned entry count are numbers only: they are recorded on the `NATIVE_COMMAND_PROCESS`
event of a launched command (`digest_ms`, `controls_ms`, `tree_walk_ms`,
`tree_entries`, `hardlink_policy_ms`, `git_policy_ms`, `overlay_probe_ms`, `probe_ms`,
`preparation_ms`, `authorize_commit_ms`, `bind_queued_ms`) and on the
`NATIVE_COMMAND_FINISHED` event's observation of a command that did not start; a
finished command's observation records `process_ms`. They are never part of the tool
reply.

A bare program name is looked up on the project's `.venv/bin`, `venv/bin` and
`node_modules/.bin`, then on the owner's
[tool search path](../../cortex-workspace/maintenance-and-controls.md#tool-search-path):
the `PATH` the Workspace service reads from the owner's login shell, folders
the owner added, then the existing usual per-user tool folders it still lacks
(`~/.local/bin`, `~/.cargo/bin`, the default nvm or fnm Node, Volta and the others
listed there), then the system folders, validated as [SECURITY.md](../../../SECURITY.md)
describes. An incoming ACP agent, which never reads a shell, uses its client's `PATH`
followed by the same usual and system folders. Any
other search entry inside the project is dropped. No mount is added for these
folders: every profile already shows the host filesystem (with private stores
masked), so a tool folder and its symlink targets are readable with exactly the
write access the profile already gives, read-only for the verifier and read-only
roles. The final verifier and the program-availability check use the same list.

An owner can also request stopping one exact running brokered worker command
with a reason, through `/cortex-kill`, the Paseo worker-panel control, or the
owner-local command CLI. This is a user-originated command-control path, not a
new model tool and not task cancellation: the command runner records and handles
the request, then reports a stopped command as a failed activity with the reason
while the worker continues. See [stopping one worker command](../steering-and-questions/index.md#stop-one-worker-command-with-a-reason).
`timeout_seconds` advertises only a minimum of 1 on `command_run` and
`outside_command`. Ordinary `command_run` is unbounded when timeout is omitted;
exceptional `outside_command` preserves its 600-second default. An enormous value is not refused on the field —
it is clamped internally to a ~68-year ceiling (`gateway.TIMEOUT_CEILING_SECONDS`,
`2**31-1` seconds) before it reaches the deadline/subprocess-timeout arithmetic,
so it can never overflow or crash; it behaves as effectively unbounded.
An invalid argument is refused on its exact field (`invalid_arguments`) before
anything starts. A failure once the command may have started (for example its
retained output could not be written), or an unexpected runtime exception, is
`storage_error` with `write_state: uncertain` and `field: cause` naming the
exception class (plus an `OSError`'s reason); the traceback goes to the
engine's stderr log, and the request key is fenced so it is never re-run.
Automatic verification runs in the read-only profile in every mode, so it may write only
the directories the project's git ignores (caches, builds, installed dependencies) inside
the project; the project's `.git` is read-only there, and the rest of the host is read-only
except the tool-cache allowlist, private scratch and the host `/tmp` (see the reachable paths
above). Each check records its profile (`read_only`). A mode switch does not stop a running
check: the check's profile does not depend on the mode, and a check cannot establish a pass if
it fails, its output is incomplete or the project digest changes. A reviewer reuses a runtime
check only when it ran in the `read_only` profile, which is the profile a reviewer's own command
gets. A check recorded under the Guarded writer name (`restricted`) or the host-writable name
(`development`) is never reused for a reviewer; it is run again instead. Gap closure accepts a
pass in `read_only`, in the task mode's own writer profile, or in `restricted` (a Guarded writer's
pass, which also covers read-only runs recorded before the read-only profile had its own name).
A host-writable pass never closes a gap once the task is Guarded.

### Git and tool state

Git run by a writing worker through `command_run` (no Cortex engine session has a
native shell) updates the project's own repository, so commit, branch, switch, stash,
merge and other operations that change only repository data work (the tests
cover commit, switch, stash and merge; fetch/push additionally need the owner's
network credentials and are not covered). This applies only when the project root
is itself a checkout (a `.git` directory, or a linked worktree's `.git` file) and
the whole project is granted (`.`, `**` or `**/*`, not `*` or a collection of
narrow file/directory grants). Development/Auto writing-worker `command_run`
now permits these repository-data writes consistently for ordinary repositories
and linked worktrees. Existing submodule repositories beneath `modules/` also
receive data grants; their containers and configuration remain protected.
A standalone submodule is not a supported logical project root; run its Git
commands from the superproject. Narrow grants, read-only roles, reviewers,
runtime verification and runtime-owned Git observations retain read-only metadata.

The table describes whole-project writing grants. Development/Auto additionally
protects `info/` (attributes, excludes and sparse-checkout affect later commands)
and supplies `core.hooksPath=/dev/null` and `core.fsmonitor=false` through inherited
Git command configuration. These are defaults, not protection against arbitrary
owner code or an explicit command-line override. Engine/Guarded behavior is unchanged:

| Path | `command_run` (writing worker) | `project_read` / `project_write` / `project_edit` / `project_delete` |
| --- | --- | --- |
| `.git` data: objects, refs, logs, index, HEAD, packed-refs, locks, merge/rebase/stash state | writable | denied |
| `info/` | read-only in Development/Auto; writable in engine/Guarded writing grants | denied |
| A linked worktree's own `<common>/worktrees/<name>/` and its common directory (even outside the project) | writable | denied |
| `config`, `config.worktree`, `hooks/`, `commondir`, a worktree's `gitdir`, `remotes/`, `branches/` in those directories | read-only | denied |
| `core.hooksPath` when it names a directory inside the project (e.g. husky) | read-only | ordinary project file rules |
| Other linked worktrees' metadata; `modules/` containers | read-only; existing submodule data writable only in Development/Auto | denied |
| The project's `.git` for reviewers and read-only roles | read-only | denied |
| `.cortex` | hidden | denied |
| The project's `.codex`, `.claude` | writable | allowed (ordinary project files; see also [Project skill links](#project-skill-links)) |
| Tool state in the project (`.venv`, `node_modules/.cache`, `.pytest_cache`, `__pycache__`, `.mypy_cache`, `target/`, `build/`, ...) | writable | allowed (ordinary project files) |

Those protected git entries configure or redirect git runs the owner makes later,
outside any sandbox (hooks, fsmonitor, editor, pager, filters, credential helpers,
includes). An existing one is mounted read-only. An absent one cannot be covered
by a mount without creating it in the owner's repository, so a guard
(`GitGuard`) checks every half second while the sandbox lives and once more when
it ends, removes any that appeared and, for `command_run`, reports them and fails
the command with `git_configuration_protected`. A process that deliberately
detaches from a `command_run` and outlives it is no longer swept (see
[SECURITY.md](../../../SECURITY.md#git-and-tool-state-in-the-project)). Changing git
configuration, installing hooks or adding worktrees/submodules is done by the
owner, or with `outside_command`, which the owner confirms. The read-only
`project_git` status/diff observation remains read-only.

Read-only git observations (`sandboxed_git` and `project_git`) and command/engine
sandboxes that expose git metadata without a repository write grant, plus
Development/Auto commands without a whole-repository write grant, redirect
`lfs.storage` to `<scratch>/lfs` through inherited Git command configuration.
Git LFS clean/process filters remain enabled: modified LFS files are compared
as valid LFS pointers, not as pointer-versus-binary content. Temporary files and
new LFS objects stay in disposable command/session scratch, never the real
`.git/lfs` by default; linked worktrees use the same redirect. The redirect
does not grant metadata writes and does not change local Git configuration.
Development/Auto repository writes described above are a separate permission decision.

This redirect is not a write permission or an enforcement boundary: explicit
`git -c` options can override it, but cannot change metadata mount permissions.
The scratch is not a persistent LFS cache and does not contain the repository's
existing objects; smudge/checkout, text conversion or downloads needing those
objects are not promised by this read-only observation support. Whole-project
engine/Guarded writing grants retain their existing repository-data behavior and persistent
LFS storage; protected config and hooks remain protected. Whole-project
Development/Auto writers also retain persistent repository LFS storage: staging
must not leave committed pointers whose objects disappear with command scratch.

Tool state needs no list: a writing worker's grant is the whole project, and only
the private names above are withheld. A regular file with more than one hard link
stays read-only (its other link may be outside the project), unless every one of
its links was found as a granted, non-deny-write path inside the project: esbuild
links its two binaries to each other, and pinning them made every `npm ci` fail
with `EBUSY`. Links are counted as distinct directory entries, so a tree seen
twice through a bind mount never makes up a missing outside link. Guarded and
engine launches retain directory-wide read-only demotion above 256 such files.

Every sandboxed command's preparation makes one complete pass over the project tree
(`policy::scan` in `crates/workspace-service/src/native_command_policy.rs`); it finds
both the private names above at any depth (they are never descended) and the
writable regular files with more than one link. Directory listings give each entry's
type, so only regular files inside the write set are `fstatat`ed, relative to their
open directory, and directories are listed by up to eight threads. The limit stays
2,000,000 entries (`project_tree_limit`). The scan is not cached between commands:
a new link to a project file made from outside the project changes no project
directory, so no cheap revalidation could notice it. On this checkout (about 490,000
scanned entries, a debug build, warm cache) the whole preparation fell from about
1.7 s to about 0.24 s, of which the scan is about 0.16 s; on a cold or busy disk it
is still proportional to the number of entries. What the scan guarantees is the tree
as it was listed: an entry created after its directory was listed and before the
command starts (a new private-named entry, a new outside hard link) is not covered
by that command's mounts and is seen by the next command's scan. A link count that
rises between the scan and the launch is the same window.

Development/Auto writing-worker commands instead detach granted external hardlinks
before granting writes. A uv `.venv` or pnpm `node_modules` can then be updated,
unlinked or recreated without writing through its project aliases into the shared
cache. Internal-only writable hardlinks stay linked; denied/private paths and
private symlink targets are not detached. Symlinks themselves are unchanged.
Each affected regular file gets an independent inode using a Linux reflink when
available, otherwise a bounded streaming copy. Ownership, modes (including
executability), timestamps and extended attributes are retained. This is persistent
project-local preparation, not a change to the cache, tool configuration or other
projects. Read-only roles, verification and engines never opt into it.

A git-ignored directory holding more than 100,000 such shared files is not detached
but bound read-only as a whole (one mount): retained verification workspace copies
(about 1.5 million pnpm links) made every writer command time out here
(`command_hardlink_materialization_timeout`, 2026-10-21). A materialization that still
exceeds its byte or time limit is refused with the directory holding most shared
files and what to do (remove or regenerate those copies, or git-ignore a disposable
directory), instead of a bare code.

Preparation preflights at most **2 GiB of logical file bytes per command** (counting
each alias separately, even for reflinks), with a **60 s cooperative copy deadline**.
Blocking filesystem calls can exceed that deadline. An INFO log records file count,
published bytes and elapsed seconds, without paths or content. Unsupported
reflinks fall back to copying; size/time limits, I/O/metadata errors and observed
concurrent source changes stop preparation before any command is launched, never
relax the shared-inode protection. Large environments may need owner-managed
independent copies; no automatic escalation is performed.

A complete copy is fsynced before each atomic file replacement, then the parent is
fsynced. No environment directory is removed or swapped. All original package paths
always contain complete files, even if interruption leaves some inodes detached and
others still shared; the next preparation rescans and resumes. This is **not a
whole-tree transaction** or rollback of inode identity. Normal errors remove the
current temporary file; a process/host crash can leave a hidden `.cortex-unlink-*`
temporary beside the unchanged original, never a half-written package file.
The gateway rechecks authority/policy before persistent preparation and fences it
with the command's durable started receipt. Copying or spawn failure after that
point retains an uncertain-effect receipt; an identical request key cannot replay
it. Resumption requires a fresh authorized command, not automatic retry.
Source checks detect observed concurrent updates but do not exclude a hostile
same-user process racing the final check/rename. Direct host aliases, Docker and
the already host-writable Development profile remain outside this project-alias
protection; this does not claim the shared cache is globally read-only.

Coverage: the `workspace-service` Rust tests exercise copying, metadata,
protected paths, bounded/error/interrupted preparation and shared-inode integrity;
the real-bubblewrap recreate case requires working user namespaces.

Finding these links and the private names takes a full scan of the project (one
`lstat` per granted file) before every sandboxed launch. On a large tree on a
cold HDD that scan took minutes, so the engine had not even started while the
chat showed nothing. A scan that runs longer than 3 s now opens a chat card,
`Preparing the sandbox (coordinator|worker)`, with the entries scanned and the
seconds spent, and closes it when the scan ends (`orchestration.sandbox
.SCAN_PROGRESS`). The scan itself is unchanged: the hard-link check is what
keeps a write through a link from reaching a file outside the project. A
sandboxed engine is never launched with the home directory or `/` as its
project (`engine_cwd_is_home`), because the scan would cover all of it. A
catalog probe of a session opened in the home directory took 283 s before it
was stopped.

Every side effect has a durable started receipt before execution. Identical retries
return the retained result. Conflicting keys fail. An interrupted operation with
an unknown outcome is never automatically executed again. Complete command bytes
are stored as an artifact; a passed check must reference a completed, successful,
nontrivial command receipt with complete retained output belonging to the exact
worker attempt (see [check evidence](../orchestration/index.md#check-evidence)). An MCP
success envelope, an engine-native tool call, model text or missing signal cannot
establish a passing check; such a claim is kept `unverified`.

## External MCP

Only the owner-global registry may grant external servers. Project files cannot
install a server. Each enabled entry must list exact `allowed_tools`; optional
`read_paths`, `env` and `network` are explicit administrator grants. Read-only roles
can invoke only the subset named in `read_only_tools`. MCP annotations never grant
rights. Coordinators receive no external execution servers. Their own tool
grants remain narrowed to `coordinator_tools` (or `read_only_tools` when unset).
The separate configured-worker inventory is a bounded discovery hint, not a
grant or proof of successful access. Delegation is appropriate for a capability
outside the coordinator role, never as a fallback around a denied action.
Built-in worker command/read-search discovery does not depend on external MCP
configuration. Its role-offer hints confer no access; project path reservations
are not outside grants, and safe host-history discovery must not resume an agent.
Read-only/Guarded command `/var/tmp` is scratch-backed, so a missing path in that
namespace is not proof of host absence. Authorized host-only reads/searches can
use the existing `outside_command` consent route offered only to writing workers;
a read-only investigator instead reports the capability gap. This does not
relax masks or permit retrying a denied operation. Executable no-paid fixtures
exercise allowed, denied and genuinely missing host reads without a sandbox;
namespace-dependent checks remain unverified wherever user namespaces are unavailable. See
[capability-based delegation](../roles-and-delegation/index.md#capability-based-delegation).

The engine receives a private broker endpoint, not the original executable or
credentials. The runtime proxy starts the server in the same scoped sandbox,
filters discovery and calls, rejects server-initiated sampling/elicitation, binds
request receipts to the actual channel and kills the child on revocation. A partial
JSON line has the same deadline as a complete response. An unresolved receipt
blocks replay. A read-path grant cannot overlay the project or its private masks.
A worker scope that does not exist yet is not granted to the server. An optional
server that cannot be attached safely, or that exits before the session is
created, is withheld from that session only, with a recorded reason
(`NATIVE_EXTERNAL_MCP_WITHHELD`) and a timeline note; the task keeps running.

The owner may opt a server out of that sandbox with `"containment": "none"` on
its entry, or for every entry without its own value as the registry default.
Such a server runs with the owner's full user authority: no Landlock, seccomp or
network restriction, and its path grants, `network` flag and runtime-directory
variables are not applied. Only its tool grants, the channel-bound relay and the
process-group stop at session end still apply; a daemon that detaches from that
group is outside Cortex's kill guarantee. Settings shows such a server as
**No sandbox** and offers the **Run without the sandbox** switch per server.
See [native external MCP boundaries](../../project/native-incoming-acp.md#external-mcp-boundary).

## Evidence limits

The no-paid suite exercises real Linux sandboxes, real MCP subprocesses, protocol
peers and installed startup/configuration surfaces. It cannot prove a GUI client
rendered a permission choice or that a paid model obeyed its advertised tools.
See [verification status](../../project/verification.md),
[security](../../../SECURITY.md).

## Runtime store ignore protection

Gateway `project_write` of the task root `.gitignore` preserves supplied entries
and appends a final `/.cortex/` rule unless it is already the last line. This
directory exclusion defeats preceding negations, including descendant rules.
Returned hashes describe the actual protected bytes; stale expected hashes
still refuse writes. A confirmed `project_delete` of the root ignore file
replaces it with only the required rule and returns `deleted: false`.

`command_run` and consent-backed `outside_command` compare that task root ignore
file before and after execution and repair changes before returning a receipt,
including nonzero command exits. Repair failure makes the receipt incomplete
with `gitignore_guard_failed`; revoked authority never authorizes a repair.
Before a fresh writable worker `result_submit` publishes its report, the runtime
reconciles observed root ignore-file changes, recreating removed files.
The revision manifest includes root `.gitignore` even if it ignores itself.
Without a complete start/current manifest the runtime conservatively ensures
the rule, creating `.gitignore` if missing; unchanged observed tasks do not
create an ignore file. Read-only roles and already recorded result replays do not mutate it.
This uses the current task root, not an arbitrary outside command target.

Repair snapshots are exact full UTF-8 text and its raw-byte digest, not public
`project_read` display pages. They retain every unrelated rule, including files
with more than 2000 lines or long lines, without numbering or truncation. Already
protected files are not rewritten. The internal reader keeps the same 16 MB,
no-follow and regular-single-link restrictions.

All repairs recheck live gateway authority and deny-write paths and use the
normal no-follow, regular-single-link, digest-checked atomic writer. An unsafe
file, incompatible encoding, denied path or conflicting write blocks repair;
no bypass grants access. Other entries and file permissions remain unchanged.

Installed hosts have no universal native pre-action hook. Native Edit/Write,
patches and shell edits are reconciled at result submission, not immediately
after each action. Interrupted turns without submission can leave an unsafe
ignore file. A command or native action can stage/commit before reconciliation;
Git ignores do not protect already tracked content or explicit `git add -f`.
The guard never deletes runtime data, untracks entries, stages or commits.

Focused no-paid checks:

```bash
umask 022 && timeout 900 cargo test --locked --offline -p workspace-service -p cortex-core
```
