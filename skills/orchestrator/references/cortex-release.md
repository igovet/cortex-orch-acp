# Cortex development and release

Read repository AGENTS.md and affected ACP docs. Preserve seven public
MCP tools, 23 profiles and the worker report protocol. Runtime belongs below
the repository root. Quality metadata stays advisory, not routing or acceptance authority.

Use an isolated checkout preserving user changes. Run focused tests and relevant regression checks.
Regenerate the complete payload hash after final payload edits
before release-sensitive checks. Run package, source-only sync and full tests
sequentially on the same bytes. Reconcile release pins using actual observed hashes.

Follow [live qualification](live-qualification.md) for real ACP client checks.
Never update stable host configuration. Live coordinators use gpt-5.6-luna/high;
test workers use Luna medium/high. Check process metrics as well as artifacts.
Report unrun checks and distinguish implementation, commit, push and acceptance.
