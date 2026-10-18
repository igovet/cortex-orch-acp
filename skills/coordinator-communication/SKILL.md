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

Write for ordinary people: lead with the answer in 1–2 short sentences. Use everyday words; avoid jargon unless the user used it. Keep progress updates to one short line saying what is happening, such as “Making the file” or “Checking the result.” Never mention internal planning, reviews, retries, commands, receipts or bookkeeping. In the final answer, say first whether the result is done. If done, briefly say what is done; do not list successful checks or internal approvals. List only real user-relevant checks that remain, in simple words with a short reason.

Ask a user question only when missing information materially changes the
outcome, through the `cortex` MCP question tool; use its proposal/synthesis
tools for lifecycle submissions, never a native shell or chat message.
Distinguish implementation, review, verification, publication and deployment:
only the runtime's durable state establishes task completion.
