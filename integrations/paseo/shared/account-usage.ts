import { defineRpc } from "@getpaseo/plugin";
import { z } from "zod";

/** Subscription windows reported by native Codex/Claude installations. */
export const readAccountUsage = defineRpc({
  name: "cortex.account-usage.read",
  input: z.object({}),
  output: z.object({
    available: z.boolean(),
    connections: z.array(z.object({
      engine: z.enum(["codex-acp", "claude-agent-acp"]),
      name: z.string(),
      windows: z.array(z.object({
        name: z.string(),
        usedPercent: z.number().min(0).max(100),
        resetsAt: z.number().nullable(),
      })),
    })),
  }),
});

export type AccountUsage = z.infer<typeof readAccountUsage["output"]>;
