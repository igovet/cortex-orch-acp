# Project overview

Cortex is an ACP v1 stdio agent. The runtime owns lifecycle, scheduling, routes,
permissions, independent review, verification and completion. Models propose plans,
perform bounded assignments and submit reports. The one project-local `.cortex`
store retains task metadata, immutable reports and newest-first pipeline editions.

Feature behavior is documented one page per feature in the [feature index](../features/index.md).

## Project pages

- [Architecture overview](architecture.md)
- [Conventions](conventions.md)
- [Decisions](decisions.md)
- [Gotchas](gotchas.md)
- [Storage and recovery](storage.md)
- [Storage format 12 migration and rollback](format-migration.md)
- [Security boundary audit](security.md)
- [Verification](verification.md)
- [Automated test matrix and CI](test-matrix.md)
- [Packaging, onboarding and diagnostics](packaging.md)
- [Paseo plugin dependency audit](paseo-dependency-audit.md)
- [Isolated dev Paseo environment](dev-environment.md)
- [Configuration reference](../configuration.md)

Release readiness is decided by the checks in [verification](verification.md).

Tool contracts live in `src/cortex_runtime/contracts.py` and
`src/cortex_runtime/orchestration/contracts.py`; the entry point is
`src/cortex_acp.py`. The runtime tool gateway is part of this ACP product.
Historical Codex-plugin, MITM gateway and native-subagent harnesses are removed.
