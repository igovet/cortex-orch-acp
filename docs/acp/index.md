# ACP documentation

Cortex is an incoming ACP v1 stdio agent at src/cortex_acp.py. It runs its own project-local runtime and starts pinned Codex or Claude ACP engines as outgoing sessions. Models propose work and interpret evidence; the runtime owns lifecycle, dependency scheduling, rights, routing, independent review, verification and completion. The current security contract is [SECURITY.md](../../SECURITY.md).

## Start and qualify

- [Zed Custom Agent setup](zed-custom-agent.md)
- [codeg recipe and current limits](codeg-custom-agent.md)
- [Packaging, onboarding and diagnostics](packaging-and-onboarding.md)
- [Google Drive and Docs MCP setup](google-drive-mcp.md)
- [Figma MCP setup](figma-mcp.md)
- [No-paid test matrix and live-only cases](test-matrix-and-ci.md)
- [Real-client runbook](real-client-runbook.md) — live rows remain NOT RUN until retained observations exist
- [Registry metadata draft](registry-metadata-draft.md) — not submitted

The no-paid startup smoke in scripts/cortex_acp_smoke.py launches the production entry point in an isolated temporary project and host configuration. It checks ACP framing and schema without an engine call. The Zed setup helper in scripts/cortex_acp_client_config.py defaults to a dry run and requires explicit consent for a write. Neither substitutes for real-client qualification.

## Orchestration and state

- [Current orchestration contract](orchestration.md): requirements, assignments, attempts, direct project writers, checks, decisions and instruction delivery
- [Roles and delegation](roles-and-delegation.md)
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
- [Storage format 12 migration and rollback](format-migration.md)

## Engine and access boundaries

- [Codex ACP adapter](codex-acp-adapter.md)
- [Claude ACP adapter](claude-agent-acp-adapter.md)
- [Permissions and containment](permissions-and-containment.md)
- [Security boundary audit](security-boundaries.md)

A client must explicitly select or launch Cortex as its agent. Finding a .cortex directory does not activate orchestration. Every project has one .cortex store; work must be qualified in a separate disposable project. Real Zed, Codex and Claude end-to-end operation cannot be inferred from fake-engine tests or the no-paid smoke; use the runbook and mark unobserved scenarios unverified.
