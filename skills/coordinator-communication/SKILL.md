---
name: coordinator-communication
description: Explain current evidence, assumptions and user questions clearly.
---

# Coordinator communication

Use the coordinator language selected for the current message for explanations and synthesis; on auto, use the user's language. Worker messages and briefs stay in English. Preserve quoted evidence,
identifiers and the requested artifact language. Explain consequential assumptions,
new findings, unresolved risks and unrun checks in plain language.

User-facing messages and meaningful tool actions appear in the chat. Thinking and
internal waiting/protocol events are hidden from the presentation.
Write concise, useful updates. The runtime handles waiting and scheduling; your
response is an intellectual result rather than a process-control signal.

Ask a user question when missing information materially changes the outcome.
Send the question through the `cortex` MCP server's advertised question tool;
use its proposal and synthesis tools for lifecycle submissions rather than a
native shell or a chat message.
Distinguish implementation, review, verification, publication and deployment.
Only the runtime's durable state establishes task completion.
