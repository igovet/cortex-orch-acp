import type { PluginHandlerContext } from "@getpaseo/plugin/server";
import { readPipeline, type Pipeline } from "../shared/pipeline.js";
import { getWorkerHistory, resolveAgentTask } from "./worker-history.js";
import { runStoreReader } from "./store-reader.js";

// Read-only projection of one task's durable lifecycle: its orchestration
// state, the user's messages, the coordinator's requirements, and per
// delegation the worker's reported result and the coordinator's verdict.
// Workers themselves come from the worker-history reader, which already
// resolves their names, routes and runtime-verified status.
function unavailable(message: string): Pipeline {
  return { available: false, message, tokens: { total: null, coordinator: null }, task: null, messages: [], requirements: [], delegations: {}, plan: null, workers: [] };
}

export async function getPipeline(input: { workspaceId: string; agentId: string }, context: PluginHandlerContext): Promise<Pipeline> {
  const resolved = await resolveAgentTask(input, context);
  if ("message" in resolved) return unavailable(resolved.message);
  const result = await runStoreReader("pipeline", [resolved.projectRoot, resolved.taskId], {
    timeoutMs: 5000,
    maxBuffer: 16 * 1024 * 1024,
  });
  if (result.status !== 0 || !result.stdout) return unavailable(result.refusal ?? "The pipeline could not be read.");
  // Only lifecycle status is needed here: a cursor past every event id keeps
  // the transcript out while terminal status is still read independently.
  const workers = await getWorkerHistory({ ...input, cursor: Number.MAX_SAFE_INTEGER, limit: 1 }, context);
  try {
    return readPipeline.output.parse({ ...JSON.parse(result.stdout), workers: workers.assignments });
  } catch {
    return unavailable("The pipeline returned an invalid response.");
  }
}
