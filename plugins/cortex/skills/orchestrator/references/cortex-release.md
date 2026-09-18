# Cortex development and release

Read repository AGENTS.md and affected project/feature docs. Preserve seven public
MCP tools, 23 profiles and the worker report protocol. Runtime belongs below
plugins/cortex. Quality metadata stays advisory, not routing or acceptance authority.

Use an isolated checkout preserving user changes. Run focused tests and the incident
regression evaluation. Regenerate the complete payload hash after final payload edits
before release-sensitive checks. Run package, source-only sync and full tests
sequentially on the same bytes. Reconcile release pins using actual observed hashes.

The Codex-plugin-harness-era isolated dev candidate and CLI-before-Desktop
requirements are historical, superseded by the T033 ACP migration (`scripts/cortex-dev`
was deleted as genuinely dead; see [live qualification](live-qualification.md)). Never
update the stable plugin or host configuration. Live coordinators use gpt-5.6-luna/high;
test workers use Luna medium/high. Check process metrics as well as artifacts.
Report unrun checks and distinguish implementation, commit, push and acceptance.
