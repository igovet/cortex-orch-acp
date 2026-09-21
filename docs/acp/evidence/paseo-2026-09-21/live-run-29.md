# Twenty-ninth Paseo live qualification — failed

Date: 2026-09-21 UTC. This note records the observed failure without copying
private engine transcripts, credentials, or report bodies into the repository.

The run used a fresh disposable Git project at baseline `81f5fbc`, package
`1.0.0+sha256.446b55684a8aab46`, Paseo agent
`e97eaaa2-3ad5-4bca-b00c-e7e6441f5b22`, and Claude Sonnet medium as the
coordinator. Cortex delegated the first implementation attempt to Codex ACP
`gpt-5.6-luna` at medium effort. The built-in Cortex MCP catalogue reached
both engine sessions: the coordinator broker observed 24 tools and the worker
broker observed 13, including `create_draft`, `write_report`, and
`publish_result` for the worker.

The configured external MCP registry contained codebase-memory and Playwright;
Playwright was restricted to Claude workers. The real Codex worker session
recorded `external_mcp_inventory` with `server_count=0` and `server_names=[]`.
The coordinator Claude session also recorded zero external servers. The
production runtime wrapper did not forward `host_mcp_servers`, so the external
registry never reached the engine sessions. This is a **FAIL** for external MCP
delivery. The worker correctly lacked Playwright, but it also lacked the
configured codebase-memory server. Claude QA never started, so its Playwright
delivery remains **NOT RUN**.

The run was stopped at the first confirmed hard failure and its Paseo agent
was archived. No worker report, independent QA result, rendered/UI check,
pipeline decision, or task acceptance was produced. The first worker attempt
was cancelled by the stop. The `report_ref` to `read_report.report_id`
first-call behavior from run 28 was therefore not exercised again.

The retained sanitized observation and baseline checker output are outside
the repository at
`/var/tmp/cortex-paseo-landing-twenty-ninth-20260921-103646/evidence/failed-run.md`.
