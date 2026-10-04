# ACP documentation

Cortex is an incoming ACP v1 stdio agent at `src/cortex_acp.py`. It runs its own project-local runtime and starts Codex or Claude ACP engines, or the optional OpenCode v2 engine, as outgoing sessions. Models propose work and interpret evidence; the runtime owns lifecycle, dependency scheduling, rights, routing, independent review, verification and completion. The current security contract is [SECURITY.md](../../SECURITY.md).

## Start and configure

- [Paseo installation, onboarding and diagnostics](packaging-and-onboarding.md)
- [Paseo dependency audit and unpatched advisory disposition](./paseo-dependency-audit.md)
- [Configuration reference and Paseo settings UI](../configuration.md): supported management surfaces, JSON/SQLite storage, connections, defaults and policy ceilings, routing, MCP servers, file locations and precedence
- [Zed Custom Agent setup](zed-custom-agent.md)
- [codeg recipe and current limits](codeg-custom-agent.md)
- [Isolated dev Paseo environment and plugin safety](dev-paseo-environment.md)
- [Google Drive and Docs MCP setup](google-drive-mcp.md)
- [Figma MCP setup](figma-mcp.md)
- [Automated test matrix and CI](test-matrix-and-ci.md)
- [Registry metadata draft](registry-metadata-draft.md) — not submitted

The no-paid startup smoke in `scripts/cortex_acp_smoke.py` launches the production entry point in an isolated temporary project and host configuration. It checks ACP framing and schema without an engine call. Its optional `--fixture-connection` mode installs one isolated fake connection and an explicit version-2 routing entry for a placeholder identity; it does not exercise a paid engine or real configuration. The Zed setup helper in `scripts/cortex_acp_client_config.py` defaults to a dry run and requires explicit consent for a write.

## Orchestration and state

- [Current orchestration contract](orchestration.md)
- [Roles, delegation and instruction delivery](roles-and-delegation.md)
- [Commands and project skill management](skill-management.md)
- [Completion and resource lifecycle](completion-and-lifecycle.md)
- [Steering, stop and questions](steering-and-questions.md)
- [Failures, cancellation and disconnects](failures-and-cancellation.md)
- [Input history and replay](input-history-and-replay.md)
- [Recoverable publications](publications-and-consistency.md)
- [Parallel workspaces and integration](workspaces-and-integration.md)
- [Recovery and session replacement](recovery-and-replacement.md)
- [Recovery capsules](recovery-capsules.md)
- [Context generations](context-generations.md)
- [Summaries and budgets](summaries-and-budgets.md)
- [Mixed-team routing and fallback](routing-and-fallback.md)
- [Observability](observability.md)
- [Token accounting and provider plan usage](token-and-plan-usage.md)
- [Storage format 12 migration and rollback](format-migration.md)

## Engine and access boundaries

- [Codex ACP adapter](codex-acp-adapter.md)
- [Claude ACP adapter](claude-agent-acp-adapter.md)
- [OpenCode v2 adapter (optional)](opencode-adapter.md)
- [Unified system prompt](unified-system-prompt.md)
- [Permissions and containment](permissions-and-containment.md)
- [Security boundary audit](security-boundaries.md)

A client must explicitly select or launch Cortex as its agent. Finding a `.cortex` directory does not activate orchestration. Every project has one `.cortex` store. Paid live checks run only on an explicit owner request naming `cli` or `desktop`, through the isolated dev Paseo environment and a disposable project; see [the dev Paseo guide](dev-paseo-environment.md).
