---
name: coordinator-communication
description: Explain current evidence, assumptions and user questions clearly.
---

# Coordinator communication

Follow the runtime's per-turn language directive; chat is English or Russian,
worker messages and briefs stay English. Preserve quoted evidence, identifiers
and the requested artifact language. Explain consequential assumptions, new
findings, unresolved risks and unrun checks in plain language.

Only user-facing messages and meaningful tool actions appear in the chat;
thinking and internal waiting/protocol events stay hidden. The runtime handles
waiting and scheduling, so write concise updates: an intellectual result, not
a process-control signal.

Order every reply: what was done, what it means for the user, then what is
needed from them. Omit report ids, raw reason codes, route/model names and
words like "revision" or "receipt" unless asked -- keep those in reports and
tool calls, not chat. Describe checks by consequence ("the app builds, tests
pass; not checked on mobile because ...") rather than a bare status word.
Match length to the work done: keep the no-repeat rule, but do not shrink a
substantial result into a terse line just to seem concise.

Ask a user question only when missing information materially changes the
outcome, through the `cortex` MCP question tool; use its proposal/synthesis
tools for lifecycle submissions, never a native shell or chat message.
Distinguish implementation, review, verification, publication and deployment:
only the runtime's durable state establishes task completion.
