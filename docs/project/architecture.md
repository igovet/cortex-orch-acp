# Architecture overview

Cortex is an incoming ACP v1 stdio agent at `src/cortex_acp.py`. It runs its own project-local runtime and starts Codex or Claude ACP engines, or the optional OpenCode v2 engine, as outgoing sessions. Models propose work and interpret evidence; the runtime owns lifecycle, dependency scheduling, rights, routing, independent review, verification and completion. The current security contract is [SECURITY.md](../../SECURITY.md).

Source map (the paths below exist in the checkout; the descriptions were taken from the previous ACP reference and are not re-verified line by line in this pass):

- `src/cortex_acp.py` and `src/cortex_runtime/acp/`: the ACP agent surface a client connects to.
- `src/cortex_runtime/delegation/`: assignments, worker profiles, workspaces, integration and the MCP tool registration (`mcp_tools.py`).
- `src/cortex_runtime/engine/`: outgoing engine adapters for Codex, Claude and the optional OpenCode v2 engine.
- `src/cortex_runtime/contracts.py` and `src/cortex_runtime/orchestration/contracts.py`: tool contracts.

Fresh ACP sessions durably initialize Development in the project store. The
host-side MCP broker gives a writing worker's brokered `command_run` in a Development or
Auto task host-writable mounts, with retained cwd/profile/policy receipts. Read-only roles and
automatic verification always get the read-only host profile, so their project stays read-only
(verification may write only git-ignored directories). Engine
processes keep their existing jails. This is a
trusted-local-development boundary, not adversarial containment from Docker or
arbitrary owner code. Legacy/missing/unreadable state stays Guarded; explicit
client choices are preserved. See [access modes](../features/permissions-and-containment/index.md#access-mode-development-guarded-and-auto).

Feature behavior is described in the [feature index](../features/index.md); project-wide rules are in [conventions](conventions.md), [storage](storage.md), [security](security.md) and [verification](verification.md).

The no-paid startup smoke in `scripts/cortex_acp_smoke.py` launches the production entry point in an isolated temporary project and host configuration. It checks ACP framing and schema without an engine call. Its optional `--fixture-connection` mode installs one isolated fake connection and an explicit version-2 routing entry for a placeholder identity; it does not exercise a paid engine or real configuration. The Zed setup helper in `scripts/cortex_acp_client_config.py` defaults to a dry run and requires explicit consent for a write.

A client must explicitly select or launch Cortex as its agent. Finding a `.cortex` directory does not activate orchestration. Every project has one `.cortex` store. Paid live checks run only on an explicit owner request naming `cli` or `desktop`, through the isolated dev Paseo environment and a disposable project; see [the dev Paseo guide](dev-environment.md).
