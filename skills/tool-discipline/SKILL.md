---
name: tool-discipline
description: Prevent guessed, incomplete and replayed tool calls during explicitly selected Cortex coordinator or worker work.
---

# Tool discipline

Before a call, name the concrete fact, state change or acceptance condition it must
resolve. Reuse retained results while relevant state is unchanged. A timeout, quiet
worker or desire to reconfirm does not justify another call.

Runtime owns all delegation; native subagents are disabled. Follow the runtime's
role-specific access and tool offerings. For writing workers, declared scope is
the agreed work area coordinating parallel writers, not an enforced project
write boundary. Reserve additional project paths through `access_request` before
changing them; private storage and user-protected paths remain unavailable.
Outside-project work requires a permission request accepted by the user.
Use the `cortex` MCP server for plans, results, questions, retained reports, drafts,
conditional `read_skill` calls, and scoped project operations. A worker runs a
reportable check with Cortex MCP `command_run` (or its advertised approved
outside-command route); a passed `result_submit` check cites that command receipt.
Native shell/file tools can help inspect the workspace where permitted, but their
output is not a Cortex command receipt. External MCP servers such as
`codebase-memory-mcp` supply domain tools, not Cortex lifecycle operations.

Use only tools available to this host and role. Read the complete live declaration
before first use and reload it after context loss; a name or recovery summary is
insufficient. Use minimal requests with declared defaults and observed identifiers.
Preflight keys, required and conditional fields against the live schema and
property descriptions. Measure bounded strings in its counting unit; rewrite
overlong text without silently truncating requirements or weakening verification.
Do not copy argument contracts into
skills, infer synonymous fields, discover requirements by causing errors, or inspect
runtime internals instead of advertised operations.

Batch independent reads when useful and inspect every result. Keep dependent reads,
mutations, approvals and retries sequential. A wrapper must expose the full nested
result, including exit status or a running handle. Returning stdout alone is
unverified. If a wrapper yields an active cell or command session, continue with its
matching wait or input operation until a terminal receipt before dependent work.
Retain and poll or resume that same handle; while it remains active, never launch the
same or an equivalent command. Keep waits bounded. If it cannot reach a terminal
result within the assignment constraints, cancel or close only the owned handle,
retain the timeout or cancellation receipt, and report the check unverified.

For file changes, read the current target and preserve unrelated work. Use the
observed digest when a tool requires a write precondition. Draft content goes
through the owned-draft gateway, preserving the returned marker. Recover draft
identities through bounded public reads rather than guessing paths.

Inspect success, error and truncation signals before making claims. For discovery,
use complete schemas already attached when available; otherwise select only needed
tool names and complete schemas. Open selected full declarations individually. A truncated result is
incomplete evidence: resolve the missing fact before relying on it. Observer-only
rendering truncation does not by itself prove the native call failed. Retain
acknowledged identifiers and never replay a mutation for reassurance. Correct one
deterministic error from the live declaration when the fix is unambiguous; otherwise
stop that route. Follow the tool's advertised retry contract for uncertain delivery.

Record failures and unrun checks without secrets, private report bodies or raw host
logs. Recover ownership and acknowledged references before resuming.
