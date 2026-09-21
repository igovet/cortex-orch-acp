# Isolated Zed observation — 2026-09-19

Client: Zed 1.20.2 on Linux/Xvfb. Cortex was launched from this checkout as an
ACP stdio custom agent, using only a disposable project and separate
`HOME`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME`/`XDG_CACHE_HOME` and Zed
`--user-data-dir`. No existing Zed session, credentials, or stable configuration
were used. Final source identity after the command repair was
`1.0.0+sha256.300106ba5c91de30`.

Installed adapter files were verified by package version and SHA-256 before
registration: `codex-acp` 1.12.0 (bundled Codex 0.154.0) and
`claude-agent-acp` 0.78.0. Both isolated native-account connections reported
`authorization_required`; no model prompt or paid probe was sent. A prompt
against an unconfigured session returned `orchestration_unavailable` with
`no_configuration_revision`, and did not claim task completion.

| Observation | Retained evidence | Result |
| --- | --- | --- |
| Explicitly selected Cortex appears under External Agents, while an existing `.cortex` alone does not activate it | [agent menu](final-zed-agent-menu.png) | PASS |
| Zed ACP log showed valid `initialize` and `session/new` request/response pairs as the first frames; a direct EOF launch emitted zero stdout/stderr bytes | Raw GUI log screenshot retained outside the repository at `/tmp/cortex-acp-live-ypkwriti/evidence/final-zed-acp-init.png` (SHA-256 `333e0ff438b06945f1637f31d6d0131d21a99d09e5d637cca2f6b64c1c9d526a`); [loaded session](final-zed-cortex-loaded.png) | PASS for startup; no model execution |
| With zero connections, no empty Model or Thought-level selector appears | [loaded session](final-zed-cortex-loaded.png) | PASS |
| `/cortex-status`, `/cortex-setup`, `/cortex-model` and `/cortex-effort` dispatch as commands; unavailable model/effort selections return guidance instead of an internal `configId` error | [status and setup](release2-zed-status-setup.png), [model](release2-zed-model-fixed.png), [effort](release2-zed-effort-fixed.png) | PASS for fallback; route selection remains unverified |
| Zero-connection and authorization-required setup messages are legible and disclose no credential | [zero connection](release-zed-setup-final.png), [authorization required](release2-zed-status-setup.png) | PASS |
| Ordinary user task with no route does not start an engine and reports the missing configuration | [task result](release-zed-no-connection-final.png) | PASS for failure handling only |
| Removing only the isolated Zed `agent_servers.Cortex` entry hid Cortex; the isolated connection registry SHA-256 stayed `aebcfa409e379a000a8f0ec472a8bc522fcd3108f0da8af2f276a10b7cd2200f` | [menu after removal](release2-zed-uninstalled.png) | PASS |

The first GUI pass exposed two onboarding defects: setup text named internal
functions instead of public CLI commands, and `/cortex-effort high` exposed
`Unknown configId 'thought_level'` when no route existed. Both were changed
and re-observed in a fresh Cortex process; the current screenshots above show
the corrected behavior. The no-paid ACP smoke harness and full automated suite
are separate protocol/unit evidence, not substitutes for the blocked live rows.
After the repair, `2090` automated tests passed; both production-entry ACP
smoke variants passed all `17/17` checks. Package validation and source-only
sync check reported the exact release identity above.

Still unverified in a real model run: selectable route and thought-level
rendering, streamed progress and cancellation, delegated worker launch,
instruction delivery, sandbox/MCP broker, candidate review and integration,
revision continuation, and recovery. codeg was not run.
