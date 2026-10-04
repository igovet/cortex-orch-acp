# Feature index

| Feature | Owner | Mechanism |
| --- | --- | --- |
| Task lifecycle and scheduling | Runtime | Transactional state machine and DAG; four concurrent workers per chat session across engines, with per-connection limits able only to lower that session's share |
| Worker questions and access waits | Runtime | Assignment-local waits; same healthy worker session continues after an answer or validated access decision |
| Project actions | Runtime gateway | Channel authority, scoped files, contained commands and external MCP |
| Reports and pipeline | Runtime store | Immutable reports, runtime-owned newest-first pipeline |
| Review and verification | Runtime | Fresh reviewer, bounded fix loop, exact command evidence |
| Knowledge routing | Models within assigned scope | Selective documents and bounded report cursors |
| Senior consultant | Runtime | Read-only escalation assignment with configured route |
| Informational questions | Runtime | Read-only answer before any plan/worker; exact answer receipt |
| Chat and tool history | ACP runtime and client | Readable tool rows; thinking and bookkeeping hidden in Paseo, evidence retained |

All 23 worker profiles remain available. Models have no delegation, route-selection
or acceptance API. The optional model-owned Rubber Duck lane has been removed.
[Implementation status](../acp/runtime-rewrite-status.md) records the available automated evidence.

Paseo uses the same tool presentation in coordinator chat and worker panels:
Read/Read skill retain only target/status in display history, never expand, and
never persist the returned body in new display events. Raw protocol data is hidden
everywhere; opaque-only calls are static rows. Command output and diffs still expand.
