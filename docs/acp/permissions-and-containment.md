# Permissions, filesystem, and terminal bridging (T014)

This covers the outgoing ACP client's (`cortex_runtime/engine/`) handling of
agent-to-client requests from a downstream engine: `session/request_permission`,
`fs/read_text_file`/`fs/write_text_file`, and `terminal/*`. Implementation:
`resource_binding.py`, `resource_policy.py`, `permissions.py`, `fs_bridge.py`,
`terminal_bridge.py`, wired into `session_manager.EngineSessionManager`. The
incoming side's outbound-request support (`acp/host_requests.py`) is the seam
a future orchestration wiring uses to actually reach a real host/editor; this
task delivers the seam and its unit/wire-level tests, not full orchestration
wiring (there is none yet — see `acp/runtime.py`'s `UnavailableRuntime`).

## Defaults

`resource_policy.ResourcePolicy()` defaults to `fs_local=False`,
`terminal=False`: `EngineSessionManager.negotiate` advertises an **empty**
`clientCapabilities` object, exactly as before T014 (T012 regression tests
pass unmodified). `session/request_permission` is always nominally
serviced (there is no separate ACP capability flag for it) but, with no
`host_permission_requester` injected and/or no `ResourceBinding` registered
for the session, every request is answered `{"outcome": {"outcome":
"cancelled"}}` — never an error, never silently ignored.

## Execution-location table

| Operation | Where it executes | Notes |
|---|---|---|
| `session/request_permission` | Relayed to the host (if a `host_permission_requester` is injected and the session has a live, current `ResourceBinding`) | Never persists `allow_always`/`reject_always` as a standing rule |
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

## UX claims vs actual containment (must read)

**A downstream engine's own native tools (codex-acp's shell/file tools,
claude-agent-acp's native tools, etc.) read/write/exec directly against the
engine process's own OS-level access, independent of whatever ACP
permission dialog the host displays.** This bridge governs *ACP-mediated*
`fs/*`/`terminal/*` requests only — it has no way to intercept, sandbox, or
otherwise restrict a native tool call the engine makes through its own
process's file descriptors/syscalls. Cortex provides **no OS-level
sandbox** (no namespace/seccomp/chroot) around a launched engine process
anywhere in this codebase. Whether a given native tool actually stays
inside `workspace_root` is **UNKNOWN / not qualified by this task** — it
depends entirely on the specific engine build's own behavior. This is
explicitly deferred to live qualification (T025/T035); a passing test here
proves the *ACP protocol surface* is bound/contained correctly, not that a
real engine's native tools are.

## Known, deliberate simplifications (this task)

- The "own-draft area" carve-out for a binding's own `.cortex` storage
  (`project_storage.py`'s draft classification) is **not wired through**
  `fs_bridge.py`: every `.git`/`.cortex` path component is refused
  unconditionally, for every binding, with no exception. This is stricter
  than the task card's stretch goal, chosen because the lead design
  explicitly allowed "deny all `.cortex` and document" when the own-draft
  wiring is complex.
- `HostFsForwarder`/host-negotiated fs capability and the full
  `acp.host_requests.HostRequester` → `EngineSessionManager` production
  wiring (mapping a specific incoming ACP session to its own
  `host_permission_requester`) has no concrete production call site yet:
  there is no orchestration layer invoking `EngineSessionManager` from
  `acp.agent.Agent` in this repository as of T014. Both are implemented
  and unit/wire-tested with an injected callable; the actual "one incoming
  session owns one engine instance" wiring is out of this task's scope
  (see `acp/runtime.py`).
- MCP allowlisting (`mcp_bridge.allowed_mcp_servers`) is a fixed
  `("cortex",)` for both roles — there is no per-role differentiation to
  enforce yet, since no other MCP server is ever offered anywhere in this
  codebase.

See also [index.md](index.md) and `codex-acp-adapter.md`.
