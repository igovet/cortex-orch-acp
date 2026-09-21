import { presentToolSemantics, type ToolPresentation } from "../shared/tool-presentation.ts";

export type CoordinatorToolTimelineItem = {
  callId: string;
  name: string;
  detail?: unknown;
  metadata?: unknown;
};

export type CoordinatorToolRowProjection = {
  hidden: boolean;
  presentation?: ToolPresentation;
};

/** Project only compact coordinator/system rows for the main timeline. */
export function projectCoordinatorToolRow(
  item: CoordinatorToolTimelineItem,
): CoordinatorToolRowProjection | undefined {
  // Worker rows have their own direct-click transformer and panel renderer.
  if (item.callId.startsWith("worker:")) return undefined;
  const detail = item.detail;
  const input = detail && typeof detail === "object" && !Array.isArray(detail)
    && (detail as Record<string, unknown>).type === "unknown"
    ? (detail as Record<string, unknown>).input
    : undefined;
  const inputRecord = input && typeof input === "object" && !Array.isArray(input)
    ? input as Record<string, unknown>
    : undefined;
  // The ACP coordinator card often has a humanized title while its exact MCP
  // target remains in rawInput.server/tool/arguments.
  const semanticName = inputRecord?.server === "cortex" && typeof inputRecord.tool === "string"
    ? `mcp__cortex__${inputRecord.tool}`
    : item.name;
  const semanticInput = inputRecord?.server === "cortex" && inputRecord.arguments
    ? inputRecord.arguments
    : input;
  const presentation = presentToolSemantics({
    name: semanticName,
    title: item.name,
    input: semanticInput,
    metadata: item.metadata,
  });
  // Hidden rows must still produce an empty replacement even when the
  // operation is a native shell/edit/search rather than a compact system row;
  // otherwise the host's raw tool item remains visible in the timeline.
  if (!presentation.systemOperation && presentation.visible) return undefined;
  return { hidden: !presentation.visible, presentation };
}
