# Thirtieth Paseo live qualification — failed

Date: 2026-09-21 UTC. This note summarizes the retained, sanitized observation
without copying private engine transcripts, credentials, or report bodies.

The run used a disposable Git project at baseline `81f5fbc`, package
`1.0.0+sha256.20c9767314c6351e`, Paseo 0.8 agent
`711c91d5-e6e2-4928-8291-67cef3b021ab`, task `t_d6065f001efe`,
Claude Sonnet medium coordinator, and a Codex ACP `gpt-5.6-luna` medium
implementation worker. The agent was stopped and archived at
2026-09-21 11:07:03 UTC after a hard qualification failure.

External MCP delivery worked for the roles that ran. The coordinator engine
inventory `eirt_2078f9a0dfae` and worker inventory `eirt_a5b94cab9d70`
each contained exactly one external server, `codebase-memory-mcp`. The
worker correctly had neither Playwright nor Google. The built-in Cortex
catalogue exposed 24 coordinator tools and 13 worker tools. Claude QA did
not start, so its Playwright inventory remains **NOT RUN**. These observed
inventories correct the empty-inventory failure from run 29 for these two
roles only.

The Codex worker called native `mcp__cua_repl.js` `getState()` at 11:05:52
UTC, then tried to open a Chrome `file://` tab at 11:05:57 with
`visible:false`. That call failed with `Capability is not available:
visibility` (tool observation `tc_9b516e949167`). A second Chrome `file://`
attempt at 11:06:00 was blocked by browser URL policy (observation
`tc_025391473cd6`). Neither failed call establishes that a page opened or
rendered. The attempted native main-browser route violated this run's
CLI-only/no-main-GUI boundary, and the unavailable path was retried. The run
was stopped before publication. Its rendered and accessibility checks remain
**UNVERIFIED**.

Earlier, one native `apply_patch` call attempted Delete File and Add File on
the same `index.html` path. The native tool rejected it as multiple operations
on one path; the worker corrected the edit. This was a separate native tool
error, not a Cortex MCP `invalid_arguments` call.

An independent static project checker run after the stop exited 0 with all
eight checks passing. That result covers only the files at stop. There was
no `worker_result` or report, no independent QA worker or report, no pipeline
decision, and no task-level decision or acceptance. The implementation
attempt ended failed due to the urgent stop. The earlier `report_ref` to
`read_report.report_id` first-call defect was not exercised because no report
was published. The overall qualification is **FAIL**.

The retained sanitized source observation, including event counts, tool
observation IDs, exact checker output, and file digests, is outside the
repository at
`/var/tmp/cortex-paseo-landing-thirtieth-20260921-34a5zh_x/evidence/failed-run.md`.
