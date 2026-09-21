# Project overview

Cortex is an ACP v1 stdio agent. The runtime stores task state, reports and
advisory governance in the project-local `.cortex` store; the model owns
planning, delegation, evidence interpretation and acceptance. The current ACP
architecture and client qualification procedure live under [`docs/acp/`](../acp/index.md).

- [Conventions](conventions.md)
- [Decisions](decisions.md)
- [Gotchas](gotchas.md)
- [Storage](storage.md)
- [Markdown reports](../features/markdown-reports/index.md)
- [Knowledge routing](../features/knowledge-routing/index.md)
- [Advisory lane](../features/advisory-lane/index.md)
- [Senior consultant](../features/senior-consultant/index.md)

The source of truth for public tool contracts is [`src/cortex_runtime/contracts.py`](../../src/cortex_runtime/contracts.py).
The ACP server entry point is [`src/cortex_acp.py`](../../src/cortex_acp.py).
Tests and offline smoke checks are described in [`docs/acp/test-matrix-and-ci.md`](../acp/test-matrix-and-ci.md).

Historical Codex-plugin, native-subagent, gateway and lifecycle-hook material
was removed from this directory. It must not be used to describe current
runtime behavior.
