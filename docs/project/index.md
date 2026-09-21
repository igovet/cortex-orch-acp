# Project overview

Cortex is an ACP v1 stdio agent. The runtime owns lifecycle, scheduling, routes,
permissions, independent review, verification and completion. Models propose plans,
perform bounded assignments and submit reports. The one project-local `.cortex`
store retains task metadata, immutable reports and newest-first pipeline editions.

- [Conventions](conventions.md)
- [Decisions](decisions.md)
- [Gotchas](gotchas.md)
- [Storage](storage.md)
- [Markdown reports](../features/markdown-reports/index.md)
- [Knowledge routing](../features/knowledge-routing/index.md)
- [Senior consultant](../features/senior-consultant/index.md)
- [ACP architecture](../acp/index.md)
- [Implementation and qualification status](../acp/runtime-rewrite-status.md)

Tool contracts live in `src/cortex_runtime/contracts.py` and
`src/cortex_runtime/orchestration/contracts.py`; the entry point is
`src/cortex_acp.py`. The runtime tool gateway is part of this ACP product.
Historical Codex-plugin, MITM gateway and native-subagent harnesses are removed.
