# Project overview

Cortex is an ACP v1 stdio agent. The runtime owns lifecycle, scheduling, routes,
permissions, independent review, verification and completion. Models propose plans,
perform bounded assignments and submit reports. The one project-local store per channel (`.cortex` by default, `.cortex-dev` for Workspace Dev)
retains task metadata, immutable reports and newest-first pipeline editions.

Feature behavior is documented one page per feature in the [feature index](../features/index.md).

## Project pages

- [Planned: Cortex Workspace](../cortex-workspace/index.md)
- [Architecture overview](architecture.md)
- [Conventions](conventions.md)
- [Decisions](decisions.md)
- [Gotchas](gotchas.md)
- [Known orchestration issues (open)](known-issues.md)
- [Storage and recovery](storage.md)
- [Storage format 12 migration and rollback](format-migration.md)
- [Security boundary audit](security.md)
- [Verification](verification.md)
- [Automated test matrix and CI](test-matrix.md)
- [Packaging, onboarding and diagnostics](packaging.md)
- [Bundled engine licence inventory](bundled-engine-licenses.md)
- [Paseo plugin dependency audit](paseo-dependency-audit.md)
- [Isolated dev Paseo environment](dev-environment.md)
- [Configuration reference](../configuration.md)
- [Native configuration observations and read effects](native-config-read.md)

Release readiness is decided by the checks in [verification](verification.md).

Tool contracts live in `crates/cortex-engine/src/schema.rs`; the entry point is
the Rust binary `cortex-acp` (`crates/cortex-acp`). The product contains no Python. The runtime tool gateway is part of this ACP product.
Historical Codex-plugin, MITM gateway and native-subagent harnesses are removed.
