import type { PluginHandlerContext } from "@getpaseo/plugin/server";
import { readWorkerResults } from "../shared/worker-results.js";
import { resolveAgentTask } from "./worker-history.js";
import { runStoreReader } from "./store-reader.js";

// Deliberately narrow: only the terminal `worker_result` rows for the exact
// requested assignment refs, one spawn, no `getWorkerHistory` call. This is
// the fallback path (see `server/provider.ts`'s `presentTimeline`) for a
// worker card whose ACP item predates the embedded result projection.

function unavailable(assignmentRefs: readonly string[]) {
  return { available: false, results: Object.fromEntries(assignmentRefs.map((ref) => [ref, null])) };
}

export async function getWorkerResults(
  input: { workspaceId: string; agentId: string; assignmentRefs: string[] },
  context: PluginHandlerContext,
): Promise<{ available: boolean; results: Record<string, { summary: string | null; checks: { name: string; outcome: string; blocker?: string }[] } | null> }> {
  const resolved = await resolveAgentTask(input, context);
  if ("message" in resolved) return unavailable(input.assignmentRefs);
  const result = await runStoreReader("worker-results", [resolved.projectRoot, resolved.taskId, JSON.stringify(input.assignmentRefs)], {
    timeoutMs: 5000,
    maxBuffer: 4 * 1024 * 1024,
  });
  if (result.status !== 0 || !result.stdout) return unavailable(input.assignmentRefs);
  try {
    return readWorkerResults.output.parse(JSON.parse(result.stdout));
  } catch {
    return unavailable(input.assignmentRefs);
  }
}
