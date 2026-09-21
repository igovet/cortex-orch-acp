# Feature index

| Feature | Runtime owner | Entry point | Verification |
| --- | --- | --- | --- |
| Markdown reports and task state | ACP runtime | MCP report and pipeline operations | `tests/test_source_reports.py`, `tests/test_pipeline_bootstrap.py` |
| Knowledge routing | Coordinator and bundled skills | Explicit orchestration or harvest route | profile/package checks |
| Advisory lane | Coordinator | Optional configured advisory provider | advisory unit tests |
| Senior consultant | Coordinator and consultant profile | Risk-triggered optional assignment | consultant contract tests |

The current runtime has no plugin manifest, native Codex harness, gateway,
lifecycle hook dispatcher or host-side profile registration. Those historical
surfaces are excluded from this index.

Real ACP client behavior is tracked in [`docs/acp/real-client-runbook.md`](../acp/real-client-runbook.md).
Rows remain `NOT RUN` until a real isolated client observation is retained.
