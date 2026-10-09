# Security boundary audit

> Status: migrated from the former ACP reference. Statements were not re-verified line by line against source in this pass; claims without a source path are unverified until reviewed.


This retained audit describes the earlier implementation. Its old delegation and
completion test outcomes do not qualify the replacement runtime. Current enforcement
and its native-tool gaps are stated in [SECURITY.md](../../SECURITY.md) and the
[verification status](verification.md).

This is the consolidated write-up of an independent adversarial pass
across both ACP directions and recovery. It indexes the actual enforcement
points, the real attacks attempted against them (with outcomes), and the
containment limitations that remain -- accurately stated, not implied away.
Detailed rationale for pre-existing mechanisms lives in the tasks that built
them (linked inline); this page does not restate their design docs.

## Enforced boundaries and where they live

| Boundary | Enforcement point |
|---|---|
| Channel identity (who is calling) | `trusted_channel.resolve_channel` -- unknown id and wrong secret share one error (`channel_unknown`), `hmac.compare_digest` on the secret hash, revoked/expired checked before any use |
| One mutating owner per task | `ownership.acquire_task_owner_tx` (atomic CAS under `BEGIN IMMEDIATE`) |
| Stale channel cannot mutate | `server.Server._dispatch_channeled` (built-in tools) **and**, since its introduction, `server.Server.dispatch`'s delegation-operation branch (see Finding 1 below) -- both compare `channel.owner_epoch` against `ownership.current_owner_epoch` before any mutating call |
| Cross-task/cross-role data scoping | every delegation handler in `delegation.mcp_tools` derives task/recipient/owner scope only from the already-resolved `ChannelContext`; no operation accepts a task id, recipient selector, or owner scope as an argument |
| Least-privilege process environments | `connections.environment.build_process_env` -- fixed base-env allowlist, per-connection isolation dir, single fresh-read secret variable, denylist defense-in-depth, no `extra_env` |
| Filesystem containment | `engine.fs_bridge` -- fd-based, TOCTOU-safe (`O_NOFOLLOW` + `/proc/self/fd` real-path check), `.git`/`.cortex` refused, non-regular files refused |
| Workspace isolation between workers | `delegation.workspace_isolation` -- disjoint `workspace_root`s per binding |
| Cortex MCP identity | `engine.mcp_bridge.build_mcp_servers` always emits the authenticated `cortex` entry first; client and Cortex-owned stdio entries cannot shadow that name |
| Forwarded MCP configuration | `configuration.mcp_servers.load_mcp_servers` accepts only owner-only registries, rejects malformed entries, and keeps env/header values in memory; engine forwarding preserves the role's authenticated Cortex channel |
| Redaction choke point | `observability.redact` -- single scrub point for logs/diagnostics (`engine.logs`, `gateway.diagnostics`) |
| Fallback/routing consent | `routing.authorization` -- deny-by-default, explicit consent required, privacy/credential scope included (not just cost) |

## Attacks attempted and outcomes

### 1. Credentials and recipients
- Two connections of the same engine, one worker/coordinator env each:
  `connections.environment.build_process_env` re-reads the registry fresh
  per call and only ever injects the calling connection's own isolation
  dir and secret; existing coverage in
  `tests/test_connections_environment_isolation.py`. No change needed.
- Secret leakage into logs/reports/capsules: `observability.redact` is a
  single choke point; `trusted_channel`'s secret is never persisted,
  only its hash.
- **Finding (fixed): a hook-recorded internal principal thread id leaked to
  any channel on the same task** via `list_reports`' discovery feed
  (`Store._change_metadata`'s `actor_thread_id`), even though
  `trusted_channel`'s own principal is deliberately masked everywhere else
  specifically because it is a usable impersonation credential on the
  unfenced legacy Codex `_meta` path. Fixed: for a channel-managed
  (`trusted`) caller, `actor_thread_id` is now always `null`; a new boolean
  `actor_is_self` tells the caller only whether an observation was its own
  action. Untrusted/legacy callers were unaffected (their own thread id was
  already directly visible on that path). Regression:
  `tests/test_security_boundaries.py::test_hook_actor_thread_id_never_leaks_a_principal_to_a_channel_peer`.
  The unfenced legacy Codex `_meta` path no longer exists (`server.py`
  requires a trusted channel for every call), so this specific
  impersonation route is closed; the regression test above guards it.

### 2. Runtime binding
- **Finding (fixed): stale owner epoch bypassed delegation MCP
  operations.** `recovery.coordinator_recovery.recover` bumps the task's
  owner epoch and revokes the old coordinator channel in two *separate*
  transactions. `server.Server._dispatch_channeled` (the 7 built-in tools)
  already checked the caller's epoch before a mutation, but the delegation
  dispatch branch (`delegate`/`clarify`/`cancel`/`decide`/`integrate`/
  `ask_question`/`publish_result`) never did -- it relied solely on
  revocation. A crash/restart between the two recovery transactions left a
  stale, un-revoked coordinator channel able to call every delegation
  operation, silently broadening access past a completed recovery
  (violates INVARIANTS.md: "Recovery does not broaden access", "stale
  approvals cannot authorize a new action"). Fixed: the same epoch check
  now gates every non-read delegation operation. The read-only ones
  (`list_events`/`list_sources`/`read_source`/`read_capsule`/
  `read_large_output`/`read_diagnostics`) intentionally still don't
  require a live epoch, matching `list_reports`/`read_report` -- a stale
  channel reading data never sees more than its own already-resolved
  identity could always see (no broadening). The legitimate epoch-forward
  migration for still-owned worker channels
  (`ownership.reassign_worker_channels_tx`) is unaffected. Regressions:
  `tests/test_security_boundaries.py::test_stale_coordinator_channel_cannot_delegate_after_ownership_transfer`,
  `::test_stale_epoch_read_operations_still_see_only_their_own_data`,
  `::test_worker_channel_migrated_forward_by_recovery_still_delegates`.
- Forged/cross-task ids in tool arguments: every delegation handler scopes
  by `channel.task_id`/`channel.assignment_id`/`channel.attempt_id` only
  (see table above); attempted forged `assignment_ref`/`report_ref` values
  are rejected (`report_not_own`, `report_ref_already_published`,
  `no_result_to_accept`) -- see
  `tests/test_security_boundaries.py::test_worker_cannot_claim_another_workers_or_tasks_report_as_its_own_result`
  and pre-existing `tests/test_trusted_channel.py`.
- A second coordinator / duplicate mutating owner: rejected by the atomic
  CAS in `ownership.acquire_task_owner_tx` (`tests/test_task_ownership.py`).

### 3. Injection
- Instruction-shaped text (fake system headers, fake tool-call JSON, a fake
  `<cortex:status>` delimiter, Unicode lookalikes) planted in a worker
  report: stored and returned as opaque Markdown text; never parsed as an
  instruction, never changes governance/ids/routing. See
  `tests/test_security_boundaries.py::test_injected_instruction_shaped_text_in_a_report_stays_inert_data`.
  Capsule/summary/attachment trust labeling and provenance separation are
  implemented in `acp/replay_store` and `recovery/capsule.py`;
  no gap was found in this pass.

### 4. Project-controlled trust
- **Finding (fixed): a worker channel could call `set_governance`.**
  Governance is task-wide, coordinator-owned advisory state (keyed by
  task, not by thread), but `Store.call` never rejected a worker
  (`parent_thread_id is not None`) the way it already did for
  `create_draft(template="pipeline")`. Fixed with the same
  before-transaction guard (`coordinator_governance_only`). Regression:
  `tests/test_security_boundaries.py::test_worker_channel_cannot_set_task_governance`.
- Path traversal / symlink escape in the fs bridge: covered by
  `engine.fs_bridge`'s fd-based, TOCTOU-safe containment; existing
  coverage in `tests/test_workspace_fs_bridge_containment.py`, no new gap
  found.
- Direct `.cortex` SQLite/WAL/lock access and another worker's draft: drafts
  are scoped by `owner_thread_id`; a worker's own MCP channel never accepts
  another worker's draft id.

### 5. Native-tool containment
No new claim is made here. `docs/features/permissions-and-containment/index.md`
("UX claims vs actual containment") already states plainly that a
downstream engine's own native tools are not sandboxed by this project and
the actual containment depends on that engine's own implementation --
confirmed accurate on inspection; unchanged.

The permission bridge now refuses a worker's declared native read/search/fetch
path into `.cortex`, including a declared `cat` operand whose symlink resolves
there. This is a request-path guard only. It does not turn native tools into
an OS sandbox, and a direct read that the engine performs without an ACP
permission request remains unobservable and therefore cannot be claimed
prevented.

### 6. Main ACP route needs no MITM
The main incoming-ACP/outgoing-delegation path (`server`, `store`,
`trusted_channel`, `ownership`, `delegation.mcp_tools`, `acp.agent`,
`acp.orchestrating_runtime`, `acp.runtime`) requires no proxy, MITM, or
root certificate to operate; none of those modules import the gateway
package. Regression:
`tests/test_security_boundaries.py::test_acp_and_delegation_runtime_never_import_the_gateway_package`.

## Remaining limitations (accurate, not overbroad)

- **Hard-link-outside detection**: a hard link inside a workspace to a file
  whose other name lives outside the workspace root is not detectable from
  an open file descriptor alone (documented in `engine/fs_bridge.py`'s
  module docstring).
- **Native-tool containment is genuinely unknown**: see §5 above and
  `docs/features/permissions-and-containment/index.md`.
- **Terminal `cwd` realpath-to-`Popen` TOCTOU**: a directory swapped
  between the containment check and process launch is not closed (a known limitation).
- **Multi-file apply is not atomic**: `delegation.integration`'s
  multi-file integration is conflict-checked but not atomic across files.
- **Calls/tokens remain unmetered** ceilings; only
  monetary claims that are actually enforceable are ever surfaced.
- **`StoreError.received` may hold argument text** in some validation
  paths; not reached by any path in the runtime
  attacked, left as a tracked, non-blocking informational item.
- **Restart/recovery limits**: recovery is derived from durable state, not
  from provider-side memory; a genuinely unrecoverable engine-side context
  is reported as such, never silently fabricated.
