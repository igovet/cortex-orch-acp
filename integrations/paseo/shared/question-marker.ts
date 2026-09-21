/** Stable wire marker used because Paseo 0.8's ACP adapter drops ACP metadata. */
export const CORTEX_QUESTION_TOOL_NAME = "cortex.ask_user";
export const CORTEX_QUESTION_INPUT_MARKER = "__cortex_question";
// The runtime emits these IDs as cortex-question-1, cortex-question-2, ...;
// ACP preserves optionId while Paseo's adapter drops request _meta.
export const CORTEX_QUESTION_OPTION_PREFIX = "cortex-question-";

export function isCortexQuestionRequest(request: {
  input?: Record<string, unknown>;
  actions?: Array<{ id?: unknown }>;
}): boolean {
  const markedByInput =
    request.input?.[CORTEX_QUESTION_INPUT_MARKER] === true;
  const actionIds = request.actions?.map((action) => action.id);
  const markedByOptions =
    Array.isArray(actionIds) &&
    actionIds.length > 0 &&
    actionIds.every(
      (id) => typeof id === "string" && id.startsWith(CORTEX_QUESTION_OPTION_PREFIX),
    );
  return markedByInput && markedByOptions;
}
