import { defineRpc } from "@getpaseo/plugin";
import { z } from "zod";

export const workerEventSchema = z.object({
  id: z.number().int().nonnegative(),
  type: z.string(),
  createdAt: z.string(),
  text: z.string(),
  activity_ref: z.string().min(1).optional(),
  // Stable identity for one streamed worker message segment. It changes at
  // every tool boundary, including hidden runtime tools, so replay cannot
  // accidentally join prose from opposite sides of a tool call.
  message_id: z.string().min(1).optional(),
  input: z.unknown().nullable().optional(),
  output: z.unknown().nullable().optional(),
  status: z.string().nullable().optional(),
  error: z.unknown().nullable().optional(),
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
  // The routing id/alias (e.g. `opus[1m]`) -- used for pin/deny and as the
  // record identity; never itself presented as the concrete model.
  model: z.string().min(1),
  // The real, engine-observed model name (e.g. `Opus 5.5 (1M)`) resolved
  // from this route's cached catalog evidence -- absent when the engine
  // never gave evidence more concrete than the bare alias/id itself; the
  // client then shows `model` explicitly marked as an alias.
  modelName: z.string().min(1).nullable().optional(),
  effort: z.string().min(1).nullable(),
});

export const tokenTotals = z.object({
  input: z.number().int().nonnegative(),
  output: z.number().int().nonnegative(),
  cachedRead: z.number().int().nonnegative(),
  cachedWrite: z.number().int().nonnegative(),
  total: z.number().int().nonnegative(),
  knownCalls: z.number().int().nonnegative(),
  unknownCalls: z.number().int().nonnegative(),
});

const attempt = z.object({
  id: z.string(),
  attemptNo: z.number().int().nonnegative(),
  status: z.string(),
  failureReason: z.string().nullable(),
  completedAt: z.string().nullable().optional(),
  agentId: z.string().min(1).optional(),
  route: route.optional(),
  // Runtime-retained result checks let the worker tab mirror the chat card
  // and Pipeline's terminal issue count without parsing an error string.
  checks: z.array(z.object({
    name: z.string(), outcome: z.string(), blocker: z.string().optional(),
    // A failed check an exact later rerun closed: kept as run history, not a
    // current failure (see delegation.evidence.accept_checks).
    closedBy: z.string().optional(),
    // The runtime downgraded a claimed "passed" to unverified/failed for
    // insufficient evidence; evidenceGap names the unmet condition.
    claimedOutcome: z.string().optional(),
    evidenceGap: z.string().optional(),
  })).optional(),
  // The worker's own terminal summary for this exact attempt (the runtime's
  // retained result, else this attempt's latest worker_result), scrubbed and
  // bounded server-side; summaryTruncated says the bound clipped it. Shown
  // only in the worker tab: coordinator cards never carry it.
  summary: z.string().optional(),
  summaryTruncated: z.boolean().optional(),
  // Compact reads (`compact: true`, the coordinator card's usage poll)
  // replace the check list with this open-issue count: checks that are not
  // passed and not closed by an exact later rerun.
  openIssues: z.number().int().nonnegative().optional(),
  // Latest ACP context fill of this attempt's engine session.
  context: z.object({ used: z.number().int().nonnegative(), size: z.number().int().positive() }).optional(),
  // Engine-reported token totals of this attempt's calls.
  tokens: tokenTotals.optional(),
  events: z.array(workerEventSchema),
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
    // Card-sized read: no summary, no check list (an openIssues count
    // instead). Coordinator cards use it; the worker tab never does.
    compact: z.boolean().optional(),
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
