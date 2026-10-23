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
context loss, including tools reached through a search layer; names and recovery
summaries are insufficient. In code mode, take Cortex argument shapes from the
schema-derived tool cheat sheet, and fetch one exact record with tools_discover
only when a field's bounds or conditions matter or a call was refused. Otherwise,
when offered, use tools_discover rather than guessing from a names-only tool list. It does not discover external MCP schemas or grant access. For a listed skill, read its full listed description, not only its name.
Preflight required and conditional fields against the live schema or cheat sheet before calling.
Measure bounded strings in its counting unit; rewrite overlong text without silently truncating requirements or weakening verification.
Follow the advertised arguments, bounds,
preconditions and retry contract; never guess synonyms, probe errors to discover
requirements, inspect private runtime state, or copy argument contracts into skills.

Batch independent reads; keep dependent reads, mutations and approvals sequential.
When offered, read_batch groups eligible independent Cortex evidence pages using its live schema; it is not an evaluator or a transaction. Preserve each item's status and native paging metadata. Deferred, failed or unrun items remain incomplete until resolved; an outer reply alone proves no inner success.
Inspect success, errors and truncation before relying on a result. A wrapper must
expose the complete nested result, including exit status or running handle.
Inspect content if structuredContent is absent.
Use real same-attempt check receipts.
Continue an owned running handle to a terminal result; never launch an equivalent
command while it remains active. Keep waits bounded. If it cannot finish, close
only that handle and report its timeout or cancellation as unverified.
A truncated result is incomplete evidence: resolve the missing fact. Observer-only
rendering truncation does not itself prove a native call failed.

Retain acknowledged identifiers; never replay a mutation for reassurance.
Correct one deterministic error only when the live declaration makes the fix
unambiguous; otherwise stop that route. An access, channel, role, scope or policy
refusal is a decision, not a glitch: never reach the same effect another way.
Request the needed authorization through the runtime or report the blocker.

Recognized missing, incomplete or stale file-read preconditions are not access
refusals: nothing was changed. If your role edits files, read the current target
on this channel, then rebuild and re-propose the edit under the existing
authority, without an owner question or extra approval. This never cures a real
denial or permits switching tools, channels or workers to bypass one.

Read current targets before changing them and preserve unrelated work. Drafts
use the owned gateway; recover draft identities through bounded public reads,
not guessed paths. Record failures, unrun checks and missing evidence without
secrets, private report bodies or raw host logs.
