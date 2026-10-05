---
name: tool-discipline
description: Shared discipline for available tools, complete evidence and safe delivery.
---

# Tool discipline

Before a call, identify the concrete fact or state change it must resolve.
Reuse retained results while relevant state is unchanged. A timeout, quiet worker
or desire to reconfirm does not justify another call.

Runtime owns all delegation; native subagents are disabled. Use only tools
available to this host and role. Cortex MCP owns lifecycle operations, reports,
questions and skill reads; external MCP servers supply their domain tools.
Read each tool's complete live description and schema before first use and after
context loss, including tools reached through a search or code layer. Names and
recovery summaries are insufficient. For a listed skill, read its full listed description, not only its name.
Preflight required and conditional fields against the live schema before calling.
Measure bounded strings in its counting unit; rewrite overlong text without silently truncating requirements or weakening verification.
Follow the advertised arguments, bounds,
preconditions and retry contract; never guess synonyms, probe errors to discover
requirements, inspect private runtime state, or copy argument contracts into skills.

Batch independent reads; keep dependent reads, mutations and approvals sequential.
Inspect success, errors and truncation before relying on a result. A wrapper must
expose the complete nested result, including exit status or running handle.
Continue an owned running handle to a terminal result; never launch an equivalent
command while it remains active. Keep waits bounded. If it cannot finish, close
only that handle and report its timeout or cancellation as unverified.
A truncated result is incomplete evidence: resolve the missing fact. Observer-only
rendering truncation does not itself prove a native call failed.

Retain acknowledged identifiers; never replay a mutation for reassurance.
Correct one deterministic error only when the live declaration makes the fix
unambiguous; otherwise stop that route. A refusal is a decision, not a glitch:
never reach the same effect another way. Request the needed authorization through
the runtime or report the blocker.

Read current targets before changing them and preserve unrelated work. Drafts
use the owned gateway; recover draft identities through bounded public reads,
not guessed paths. Record failures, unrun checks and missing evidence without
secrets, private report bodies or raw host logs.
