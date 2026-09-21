/** Presentation policy only. Durable runtime evidence is not deleted. */
const HIDDEN = new Set([
  "agent_thought_chunk", "thought", "thinking", "reasoning", "wait", "wait_agent",
  "task_notification", "usage_update", "current_mode_update", "config_option_update",
  "available_commands_update", "session_info_update", "turn_started", "turn_complete",
]);

export function isHiddenOperation(value: unknown): boolean {
  if (typeof value !== "string") return false;
  const name = value.trim().replace(/^mcp(?:__|\.)[^.]+?(?:__|\.)/i, "").toLowerCase();
  return HIDDEN.has(name);
}

export function isHiddenWorkerEvent(event: {
  type?: unknown; tool_kind?: unknown; title?: unknown; content?: unknown;
}): boolean {
  const content = event.content && typeof event.content === "object" && !Array.isArray(event.content)
    ? event.content as Record<string, unknown> : {};
  return [event.type, event.tool_kind, event.title, content.sessionUpdate].some(isHiddenOperation);
}
