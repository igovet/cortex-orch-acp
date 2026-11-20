/** Presentation policy only. Durable runtime evidence is not deleted. */
const HIDDEN = new Set([
  "agent_thought_chunk", "thought", "thinking", "reasoning", "wait", "wait_agent",
  "task_notification", "usage_update", "current_mode_update", "config_option_update",
  "available_commands_update", "session_info_update", "turn_started", "turn_complete",
]);

/**
 * The runtime's own plumbing, which a person reading the work does not need to see: polling a command, paging its
 * retained output, discovering tools, loading skills and task context, drafting and publishing reports, the protocol
 * calls whose outcome the page already shows as its own card (a submitted result, plan, synthesis, answer, question,
 * replan or access request), and code-index upkeep. What the agents did to the project stays visible: reading,
 * searching and changing files, commands, Git, browser actions and reading other workers' reports.
 */
const PLUMBING = new Set([
  "command_status", "read_large_output", "tools_discover", "read_skill", "read_capsule", "task_get_context",
  "list_reports", "list_sources", "list_route_options", "check_preflight", "create_draft", "draft_write", "read_draft",
  "write_report", "result_submit", "plan_submit", "synthesis_submit", "answer_submit", "verification_submit",
  "sprint_plan_submit", "escalation_request", "replan_request", "scope_request", "user_question_request",
  "ask_question", "access_request", "list_projects", "index_status", "check_index_coverage", "index_repository",
  "toolsearch", "todowrite",
]);

/** `showPlumbing` is the reader's choice to see every tool call; the engine's own status updates stay hidden either way. */
export type VisibilityOptions = { showPlumbing?: boolean };

const operationOf = (value: unknown): string | null =>
  typeof value === "string" ? value.trim().replace(/^mcp(?:__|\.)[^.]+?(?:__|\.)/i, "").toLowerCase() : null;

/** Whether this operation is one of the runtime's own steps, which the simplified view leaves out. */
export function isPlumbingOperation(value: unknown): boolean {
  const name = operationOf(value);
  return name !== null && PLUMBING.has(name);
}

export function isHiddenOperation(value: unknown, options: VisibilityOptions = {}): boolean {
  const name = operationOf(value);
  if (name === null) return false;
  return HIDDEN.has(name) || (PLUMBING.has(name) && options.showPlumbing !== true);
}

export function isHiddenWorkerEvent(event: {
  type?: unknown; tool_kind?: unknown; title?: unknown; content?: unknown;
}, options: VisibilityOptions = {}): boolean {
  const content = event.content && typeof event.content === "object" && !Array.isArray(event.content)
    ? event.content as Record<string, unknown> : {};
  return [event.type, event.tool_kind, event.title, content.sessionUpdate].some(value => isHiddenOperation(value, options));
}
