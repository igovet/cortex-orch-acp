import { defineRpc } from "@getpaseo/plugin";
import { z } from "zod";

const event = z.object({
  id: z.number().int().nonnegative(),
  type: z.string(),
  createdAt: z.string(),
  text: z.string(),
  activity_ref: z.string().min(1).optional(),
  input: z.unknown().nullable().optional(),
  output: z.unknown().nullable().optional(),
  status: z.string().nullable().optional(),
  error: z.string().nullable().optional(),
  diff: z.string().nullable().optional(),
  tool_kind: z.string().nullable().optional(),
  title: z.string().nullable().optional(),
  locations: z.unknown().nullable().optional(),
  content: z.unknown().nullable().optional(),
  raw_input: z.unknown().nullable().optional(),
  raw_output: z.unknown().nullable().optional(),
});

const route = z.object({
  provider: z.string().min(1),
  model: z.string().min(1),
  effort: z.string().min(1).nullable(),
});

const attempt = z.object({
  id: z.string(),
  attemptNo: z.number().int().nonnegative(),
  status: z.string(),
  failureReason: z.string().nullable(),
  agentId: z.string().min(1).optional(),
  route: route.optional(),
  events: z.array(event),
});

export const readWorkerHistory = defineRpc({
  name: "cortex.worker-history.read",
  input: z.object({
    workspaceId: z.string().min(1),
    agentId: z.string().min(1),
    attemptId: z.string().min(1).optional(),
    assignmentRef: z.string().min(1).optional(),
    cursor: z.number().int().nonnegative().optional(),
    limit: z.number().int().min(1).max(200).optional(),
    assignmentCursor: z.string().optional(),
  }),
  output: z.object({
    assignments: z.array(z.object({
      taskId: z.string(),
      id: z.string(),
      assignmentRef: z.string().min(1).optional(),
      nodeKey: z.string(),
      title: z.string().min(1).optional(),
      // The coordinator card uses the resolved role label (for example
      // `explorer`). Keep that display identity separate from the private
      // assignmentRef, which is only used to select the worker history.
      role: z.string().min(1).optional(),
      route: route.optional(),
      status: z.string(),
      attempts: z.array(attempt),
    })),
    available: z.boolean(),
    truncated: z.boolean(),
    nextCursor: z.number().int().nonnegative().nullable(),
    nextAssignmentCursor: z.string().nullable(),
    message: z.string().optional(),
  }),
});

export type WorkerHistory = z.infer<typeof readWorkerHistory["output"]>;
