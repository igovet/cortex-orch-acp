import { defineRpc } from "@getpaseo/plugin";
import { z } from "zod";
import { readWorkerHistory, tokenTotals } from "./worker-history.js";

const check = z.object({ name: z.string(), outcome: z.string(), blocker: z.string().optional() });

export const readPipeline = defineRpc({
  name: "cortex.pipeline.read",
  input: z.object({
    workspaceId: z.string().min(1),
    agentId: z.string().min(1),
  }),
  output: z.object({
    available: z.boolean(),
    message: z.string().optional(),
    // Engine-reported tokens: the whole task, and the coordinator's share.
    tokens: z.object({ total: tokenTotals.nullable(), coordinator: tokenTotals.nullable() }),
    task: z.object({
      id: z.string(),
      createdAt: z.string(),
      state: z.string().nullable(),
      waitReason: z.string().nullable(),
      waitDetail: z.string().nullable(),
    }).nullable(),
    // The first user message is the request; later ones steer it.
    messages: z.array(z.object({ text: z.string(), createdAt: z.string() })),
    requirements: z.array(z.object({ id: z.string(), statement: z.string(), criteria: z.array(z.string()) })),
    // Keyed by the public delegation ref (a worker's `assignmentRef`).
    delegations: z.record(z.string(), z.object({
      order: z.number(),
      result: z.object({ summary: z.string().nullable(), checks: z.array(check) }).nullable(),
      decision: z.object({ decision: z.string(), rationale: z.string().nullable() }).nullable(),
    })),
    // The runtime's accepted plan (current revision), including nodes no
    // worker has started for yet. Null for a task the runtime never planned.
    plan: z.object({
      state: z.string(),
      blocker: z.string().nullable(),
      nodes: z.array(z.object({
        id: z.string(),
        role: z.string(),
        objective: z.string(),
        stage: z.string(),
        status: z.string(),
        assignmentRef: z.string().nullable(),
        waiting: z.string().nullable(),
        result: z.object({ summary: z.string().nullable(), checks: z.array(check) }).nullable(),
      })),
    }).nullable().default(null),
    workers: readWorkerHistory.output.shape.assignments,
  }),
});

export type Pipeline = z.infer<typeof readPipeline["output"]>;
