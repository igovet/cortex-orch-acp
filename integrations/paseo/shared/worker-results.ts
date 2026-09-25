import { defineRpc } from "@getpaseo/plugin";
import { z } from "zod";
import { workerResult } from "./pipeline.ts";

// Fallback for a worker card whose ACP item predates the embedded result
// projection (see ``server/provider.ts``'s ``presentTimeline``): one lightweight,
// narrowly-scoped batched read instead of a per-card ``readPipeline`` RPC.
export const readWorkerResults = defineRpc({
  name: "cortex.worker.results",
  input: z.object({
    workspaceId: z.string().min(1),
    agentId: z.string().min(1),
    assignmentRefs: z.array(z.string().min(1)).min(1).max(200),
  }),
  output: z.object({
    available: z.boolean(),
    // Keyed by the requested `assignmentRef`; `null` when that worker has
    // no durable result yet (still running, or never published one).
    results: z.record(z.string(), workerResult.nullable()),
  }),
});
