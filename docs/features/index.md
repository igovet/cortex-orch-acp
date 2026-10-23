# Feature index

Every product feature has one page below. Each page names its source area; project-wide rules are in the [project overview](../project/index.md).

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

## Pages

Runtime lifecycle and coordination:

- [Orchestration](orchestration/index.md)
- [Task lifecycle and scheduling](task-lifecycle/index.md)
- [Roles and delegation](roles-and-delegation/index.md)
- [Requirements intake](requirements-intake/index.md)
- [UI/UX design workflow](design-workflow/index.md)
- [Automatic routing, mixed teams and authorized fallback](routing-and-fallback/index.md)
- [Summaries, weak models and budgets](summaries-and-budgets/index.md)
- [Steering, questions and cancellation](steering-and-questions/index.md)
- [Failures, cancellation and disconnects](failures-and-cancellation/index.md)
- [Durable input history and replay](input-history-and-replay/index.md)
- [Publications and consistency](publications-and-consistency/index.md)
- [Markdown reports](markdown-reports/index.md)
- [Tool discovery and read batches](tool-discovery-and-read-batch/index.md)
- [Knowledge routing](knowledge-routing/index.md)
- [Senior consultant](senior-consultant/index.md)
- [Retired advisory lane](advisory-lane/index.md)

Workspaces, recovery and context:

- [Project scope and filesystem effects](workspaces-and-integration/index.md)
- [Recovery races and session replacement](recovery-and-replacement/index.md)
- [Deterministic recovery capsules](recovery-capsules/index.md)
- [Context generations and qualified recovery delivery](context-generations/index.md)
- [Unified system prompt](unified-system-prompt/index.md)

Access, commands and observability:

- [Permissions and execution containment](permissions-and-containment/index.md)
- [Commands and project skill management](skill-management/index.md)
- [Observability and resource attribution](observability/index.md)
- [Token and provider plan usage](token-and-plan-usage/index.md)

Engines and client integrations:

- [Codex ACP adapter](codex-engine/index.md)
- [Claude ACP adapter](claude-engine/index.md)
- [OpenCode v2 adapter (optional)](opencode-engine/index.md)
- [Zed Custom Agent setup](zed-agent/index.md)
- [codeg Custom Agent recipe](codeg-agent/index.md)
- [Google Drive and Docs MCP setup](google-drive-mcp/index.md)
- [Figma MCP setup](figma-mcp/index.md)

Packaging and project operations (project pages): [packaging](../project/packaging.md), [dev Paseo environment](../project/dev-environment.md), [verification](../project/verification.md), [test matrix](../project/test-matrix.md).

All 23 worker profiles remain available. Models have no delegation, route-selection or acceptance API. The optional model-owned Rubber Duck lane has been removed. [Verification status](../project/verification.md) records the available automated evidence.

Paseo uses the same tool presentation in coordinator chat and worker panels: Read/Read skill retain only target/status in display history, never expand, and never persist the returned body in new display events. Raw protocol data is hidden everywhere; opaque-only calls are static rows. Command output and diffs still expand; a shell row always opens to its full command, even without output. Explicit `execute`/`terminal` kind classifies shell calls even before command input arrives, so split updates do not turn a shell into a read; a shell call titled with its own command line (Claude's Bash, e.g. `cat a | grep b`) is a shell, not a read; only the engine's own `read` kind makes a command a read. Claude Edit/Write `file_path`/`old_string`/`new_string`/`content` input is aliased for the host's edit row so its diff opens in chat; the alias affects presentation only.
