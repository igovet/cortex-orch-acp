# Twenty-seventh Paseo live qualification — failed

Date: 2026-09-21 UTC. This note records the retained qualification result
without copying private client logs, project contents, credentials, or report
bodies.

The run used a disposable Git project, package
`1.0.0+sha256.336dbc79e27c92c6`, Paseo agent
`cac42f1c-ad1d-470c-9cb2-42d137b74b8d`, and Claude Sonnet medium as the
coordinator. Governance setup and the first Codex Luna high frontend
delegation succeeded without an MCP schema error.

The Codex worker created `index.html` and `styles.css`, and its static check
passed 4/4. Two ordinary Python one-liner syntax errors were corrected during
the work. The native rendered check remained unverified in that environment;
no rendered pass was inferred.

The worker then ended its native Codex turn normally with a final natural
language response, but it did not call `create_draft`, `write_report`, or
`publish_result`. Cortex therefore recorded the assignment as
`worker_ended_without_result`. No immutable implementation report or
structured worker checks existed for that attempt.

This is a **FAIL**, not a transport-crash diagnostic. The coordinator started
another frontend assignment, but qualification stopped at the first hard
worker report-protocol failure. The second worker was cancelled before it
produced a result. No independent Claude QA, candidate acceptance, final
pipeline decision, or task acceptance was recorded.

The same-session corrective continuation requested after this run was not
present in the observed run and is therefore **UNVERIFIED live**. A future
qualification must show that one bounded continuation is sent to the same
worker session after a normal turn with no durable result, that the worker
remains active while the coordinator waits, and that a second missing result
still fails truthfully without inferring evidence. This note does not claim
that the continuation has since been implemented or live-qualified.

The disposable Paseo agent was stopped and archived. No product or plugin
source was edited during this run. The retained private source observation is
outside the repository at
`/var/tmp/cortex-paseo-landing-twenty-seventh-20260921-122321/evidence/failed-run.md`.
