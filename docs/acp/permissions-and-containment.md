# Permissions, filesystem, and terminal bridging (T014)

This covers the outgoing ACP client's (`cortex_runtime/engine/`) handling of
agent-to-client requests from a downstream engine: `session/request_permission`,
`fs/read_text_file`/`fs/write_text_file`, and `terminal/*`. Implementation:
`resource_binding.py`, `resource_policy.py`, `permissions.py`, `fs_bridge.py`,
`terminal_bridge.py`, wired into `session_manager.EngineSessionManager`. The incoming side's outbound-request support (`acp/host_requests.py`) is
wired into the production orchestrating runtime where the host advertises
the relevant capability. Live client behavior still needs observation.

## Defaults

`resource_policy.ResourcePolicy()` defaults to `fs_local=False`,
`terminal=False`: `EngineSessionManager.negotiate` advertises an **empty**
`clientCapabilities` object, exactly as before T014 (T012 regression tests
pass unmodified). `session/request_permission` is always nominally
serviced (there is no separate ACP capability flag for it) but, with no
`host_permission_requester` injected and/or no `ResourceBinding` registered
for the session, every request is answered `{"outcome": {"outcome":
"cancelled"}}` — never an error, never silently ignored.

For a worker, a declared native file read/search/fetch path into the
project's `.cortex` subtree is refused before the ordinary non-destructive
auto-approval. The same check resolves declared `cat` operands, so a
workspace symlink targeting `.cortex` is refused too. Private store reads
must use the authenticated Cortex MCP tools. This covers only paths the
engine actually declares in `session/request_permission`; a native tool that
never emits that request, or arbitrary code hidden inside a terminal command,
remains outside ACP's observable enforcement boundary.

Codex ACP turns also carry the engine's `workspaceWrite` sandbox policy with
an empty `writableRoots` list, disabled network access, and explicit
exclusions for `/tmp` and the environment-selected temporary directory. That
policy keeps the current project cwd writable without adding another root, but
it is enforced by Codex App Server rather than by Cortex or the operating
system. It does not change the limitation above for native code that bypasses
an observable ACP request.

A retained clean real-client probe observed one Codex worker's `/tmp`
exclusion: the exact policy was sent, the single sentinel creation attempt
failed with `Read-only file system`, and the sentinel was absent. This is
evidence only for that observed `/tmp` outcome. It does not establish
`/var/tmp`, `.cortex`, arbitrary same-user, or OS-level process isolation;
those remain unverified. See the [probe record](evidence/paseo-2026-09-21/live-boundary-probe.md).

## Execution-location table

| Operation | Where it executes | Notes |
|---|---|---|
| `session/request_permission` | Cortex auto-approves recognized non-destructive worker actions and the exact active coordinator-owned pipeline draft edit; worker writes outside their bound workspace remain a hard deny. Other unsafe or unclassifiable coordinator actions are shown to the host user as a fresh one-shot permission request when a live binding and requester exist; an explicit denial, timeout, missing host, or stale binding remains a refusal. | Cortex stores no standing permission rule. For a harmless request, the bridge prefers the offered `allow_once` option and accepts the engine's offered `allow_always` only when it is the sole allow option (Claude ACP can use that shape); deletion review still requires a one-shot host decision. |

Coordinator ordinary non-delete Bash read operations are auto-approved when
observed targets stay inside the bound workspace and outside `.cortex`;
coordinator source edits are refused locally with a notifier.
The installed Claude ACP 0.78.0 turns its ACP `reject_once` result into the
generic model text `User refused permission to run tool`; the ACP response
cannot carry a policy reason. Cortex therefore records its own
`coordinator_role_refusal` event when it denies a coordinator source edit,
engine-private access, or a coordinator-run project check. That event requires
an immediate continuation in the same host turn. The continuation explains
that the restriction belongs to Cortex and directs the coordinator to
delegate the work. A locally refused source edit is never sent to the host
for a permission choice. This path has a composed no-paid test; its behavior
with the real Claude model still needs a fresh live observation.
Engine-native private skill trees (`.codex` and `.claude`, including
`~/.codex/skills` and `$HOME/.claude/skills`) are a hard local refusal for
coordinator requests and never become a Paseo host permission card. Named
project checks (`pytest`, `unittest`, Python `-m` forms, repository
`test`/`check`/`lint` scripts, and explicit `tests/` or `test_*.py` scripts)
are worker operations: Cortex refuses them locally and tells the coordinator
to delegate verification. These checks inspect only the declared ACP
path/command text; inline Python assertions that name rendered UI properties
(contrast, focus, DOM/ARIA or responsive checks) and clearly asserted grep/rg
queries are covered as well. They are not a general shell parser, and normal
public source searches remain allowed.
Recognized `/bin/bash -lc`, `bash -lc`, and `/bin/sh -c` wrappers apply the
same rule to literal nested non-delete commands; wrapped `rm` still prompts.
Malformed or opaque wrappers may prompt. Deletion, move, truncate and
outside/private targets require fresh host review; the classifier makes lexical
observed-command decisions and cannot guarantee the absence of effects hidden
behind opaque interpreters.
Redundant verification attempts, including repeated npm or Chrome checks, may
trigger a bounded verification prompt rather than an unbounded retry loop.
Declared Bash commands now use a broad default-allow policy when observed
targets remain inside the bound workspace and outside `.cortex`. Destructive,
private, outside, symlink, or malformed commands remain reviewable or refused;
opaque interpreter effects cannot be guaranteed safe.
The independent permission matrix is clean with 159 focused tests; the later
fake-wire matrix passed both execute and `other` shapes in 58 focused checks.
These checks do not qualify the pending fifteenth real-client landing.

The narrow calculation exception is the exact
`PYTHONDONTWRITEBYTECODE=1 python3 -B - <<'PY'` form with an AST-proven,
side-effect-free heredoc body. Cortex may auto-approve that exact class;
an unknown or mutating script is sent to the host for confirmation when a live
requester exists. This classification does not authorize arbitrary Python or
turn an unobserved native action into a trusted result.

Worker Bash classification now uses the observed command text: non-destructive
commands, including a read-only heredoc such as `from math import pow`, may be
auto-approved. Observed Bash `>` redirects auto-approve when every target is
inside the bound workspace and outside `.cortex`; `/tmp`, private targets and
deletion remain host-confirmed. Dynamic or opaque interpreter effects cannot
be completely classified, so this is not a guarantee of complete deletion
enforcement.

The pipeline draft exception is checked against the current coordinator
session and attempt, the task's root thread binding, and an unpublished
`pipeline` draft row. The Claude Write request observed on 2026-09-20 declared
an absolute `rawInput.file_path` and one matching `locations.path`; both must
match the registered path. The target must still be an owner-owned regular
file below that project's `.cortex/pipeline-drafts`. This permission decision
allows the native engine tool to edit only that draft according to its declared
request; it does not open ACP `fs/*` access to `.cortex` or establish an OS
filesystem boundary. For this exact coordinator-owned draft path, the
recognized native Claude edit is auto-approved after the ownership and path
checks; the draft remains private until publication.

The installed codex-acp 1.12.0 `agent` mode sends approval decisions to its
own `auto_review` reviewer. In a disposable Paseo run this reviewer approved
an exact `rm -- <sentinel>` without issuing `session/request_permission`; the
Paseo auto-accept feature was off. Cortex now starts an owner-only, verified
private copy of the pinned adapter that changes the `agent` reviewer to
`user` and forwards Cortex-owned instructions to App Server (see
[codex-acp-adapter.md](codex-acp-adapter.md)), preserving `on-request`,
the workspace policy, native account, and original installed package.
Unknown bundle layouts, reviewer definitions, or instruction call sites
fail before launch. An `rm` request actually emitted by this adapter is
classified and forwarded to the host; a missing requester or invalid reply
is refused. This remains a request-path rule, not proof that every native
deletion will generate a request. Fresh real-client evidence is required
before calling deletion confirmation verified.
Host permission round trips have a bounded 180-second window, independent of
the 30-second default for ordinary control RPCs; cancellation and session
invalidation still discard late replies.

The host relay is task/connection scoped, not coordinator-turn scoped. A
coordinator `session/prompt` may return while an already delegated worker is
still running; a later unsafe or ambiguous worker action is therefore sent to
the same connected host for a fresh one-shot decision instead of silently
falling back to `reject_once`. The relay is replaced when a task is resumed
from another connection and is released on definitive connection close,
explicit task stop, refusal, or acceptance. A disconnected or stale binding
still fails closed, and a worker write outside its bound workspace remains a
hard deny that is never forwarded to the user.

For an engine-issued `rm` request, Cortex stores the actual host response
after rechecking the session binding, keyed by task, attempt, tool call and
exact command hash. A host-selected `allow_once` receipt must match the
observed completed `rm` tool action before task-level acceptance; deletion is
treated as host-confirmed only when both signals exist. A worker report's
assertion is not a receipt. A missing, rejected, stale or conflicting response blocks
acceptance. The receipt records a host selection, not proof that a person
clicked the client UI; that distinction requires retained real-client
observation. This gate cannot prevent native actions that never request ACP
permission, though an observed `rm` without a receipt cannot be accepted.
| `fs/read_text_file`, `fs/write_text_file` (opt-in, `policy.fs_local`) | Local disk, inside the bound `workspace_root` | `.git`/`.cortex` refused unconditionally (see limitation below) |
| `fs/*` via `HostFsForwarder` | Host's own unsaved buffer, never local disk | Only when host negotiated fs AND `binding.role == "coordinator"` AND `workspace_root` equals the host-visible project root |
| `terminal/*` (opt-in, `policy.terminal`) | A locally spawned subprocess, cwd contained in `workspace_root`, least-privilege env | Every terminal is scoped to the `engine_session_id` that created it |
| Anything else, or fs/terminal while the policy is off | Refused, `-32601` | `stub_handlers.unsupported_client_request_error` |

## Binding and invalidation

Every resource operation is checked against a `ResourceBinding` (engine
instance/session/context-generation/assignment/attempt/workspace-binding/
ownership-epoch/role — all required, fail-closed) registered via
`EngineSessionManager.bind_resource`. `BindingRegistry` tracks live
invalidation state (`invalidate_session`, `invalidate_epoch`,
`invalidate_generation`, `invalidate_action`, and automatic invalidation on
`release()`/`shutdown()`); `ResourceBinding.is_current(snapshot)` is
re-checked **twice** for a permission request — once at receipt, once
again immediately before relaying a host decision — so a session
replacement/epoch bump/generation bump/cancellation that happens *during*
a slow host round-trip still wins over a stale approval.

## Revision capture

Every `fs/*` operation records a `Revision(source, sha256, path)` into an
in-memory `RevisionLog`, keyed by `workspace_binding_id`. `source="disk"`
(local worktree) and `source="host_buffer"` (unsaved host buffer via
`HostFsForwarder`) are never compared or equated — a later safe-integration
precondition check is expected to treat them as distinct provenance, not
this task's scope.

## Process containment and its limits

A downstream engine's native tools execute with its process's OS access,
independent of ACP permission dialogs. Production operation uses the exact
project root opened by Zed and the ordinary user rights of the Zed-launched
process. No `bubblewrap`, user-namespace setting, `systemd` sandboxing, or
AppArmor rule is required. Cortex starts the engine directly. There is no OS
filesystem boundary hiding the private `.cortex` store from native tools.
The coordinator uses the exact current project root as its working directory;
otherwise a long-lived coordinator session can read a startup snapshot after a
worker has changed the real Zed project and draw a false conclusion. The
coordinator's role policy and ACP permission bridge are not OS read-only
boundaries for native tools. Cortex detects observed project revision changes,
but it cannot prevent arbitrary same-user writes without process containment.
The engine launch pins `PYTHONDONTWRITEBYTECODE=1` so a normal Python test
inheriting that environment does not create bytecode files during a coordinator
turn. Other test side effects remain subject to the revision backstop.

The authenticated Cortex MCP server still uses the channel-protected runtime
dispatch path. Its bridge validates project-bound `fs/*` and `terminal/*`
requests and refuses `.git`/`.cortex` paths, but that bridge does not govern
arbitrary engine-native tools, forwarded MCP servers, network access,
credentials, or other same-user processes. Runtime writer leases, ownership
epochs, revision checks, and recovery evidence remain active regardless of
execution mode. Real Zed, Codex and Claude behavior still requires the live
runbook; offline tests cannot qualify it.

In the isolated 2026-09-20 Zed run, Cortex recorded automatic permission
approval for ordinary downstream actions while Zed still displayed `Run
Command` confirmation cards. Configuring the disposable Zed profile's own
`agent.tool_permissions` removed those ordinary prompts in the next run.
That setting is client-wide and was not applied to the user's stable Zed.
Neither the ACP permission classifier nor command-pattern rules in Zed can
prove that arbitrary native code has no deletion side effect.

## Known, deliberate simplifications (this task)

- The "own-draft area" carve-out for a binding's own `.cortex` storage
  (`project_storage.py`'s draft classification) is **not wired through**
  `fs_bridge.py`: every `.git`/`.cortex` path component is refused
  unconditionally, for every binding, with no exception. This is stricter
  than the task card's stretch goal, chosen because the lead design
  explicitly allowed "deny all `.cortex` and document" when the own-draft
  wiring is complex.
- Host buffer checks depend on a host-negotiated `fs` capability. Without
  it, the buffer state is recorded as unknown; a disk check cannot prove
  that an unsaved editor buffer does not exist.
- The Cortex MCP relay is channel-authenticated. It does not make other
  engine-native tools obey ACP permission prompts or provide network
  isolation.

See also [index.md](index.md) and `codex-acp-adapter.md`.
