import { defineRpc } from "@getpaseo/plugin";
import { z } from "zod";

/** Config id Cortex reads a user's request to stop one running worker
 * command from (see orchestration/command_control.py). */
export const STOP_COMMAND_CONFIG_ID = "cortex_stop_command";
/** Cortex accepts at most this many Unicode characters of reason. */
export const STOP_REASON_MAX = 2000;
export const STOP_REQUEST_KEY_MAX = 256;
/** The exact gateway identity of one worker command; never command text. */
export const GATEWAY_COMMAND_REF = /^gateway:[0-9a-f]{32}$/;

const scope = { workspaceId: z.string().min(1), agentId: z.string().min(1) };

export const runningCommandSchema = z.object({
  toolCallRef: z.string().regex(GATEWAY_COMMAND_REF),
  operation: z.string(),
  assignmentRef: z.string().nullable(),
  attemptId: z.string().nullable(),
  startedAt: z.string(),
  // Bounded and scrubbed by the server; display only, never an identity.
  argvSummary: z.string(),
  // A stop already recorded for this command (from this panel, /cortex-kill
  // or the local owner command); the process may not have ended yet.
  stopRequest: z.object({ reason: z.string(), origin: z.string().nullable() }).nullable(),
});

export type RunningCommand = z.output<typeof runningCommandSchema>;

export const readRunningCommands = defineRpc({
  name: "cortex.commands.running",
  input: z.object({ ...scope, assignmentRef: z.string().min(1).optional() }),
  output: z.object({
    available: z.boolean(),
    commands: z.array(runningCommandSchema),
    message: z.string().optional(),
  }),
});

export const requestCommandStop = defineRpc({
  name: "cortex.commands.stop",
  input: z.object({
    ...scope,
    toolCallRef: z.string().regex(GATEWAY_COMMAND_REF),
    reason: z.string().max(STOP_REASON_MAX).refine((value) => value.trim().length > 0, "A reason is required."),
    requestKey: z.string().min(1).max(STOP_REQUEST_KEY_MAX).refine((value) => value.trim().length > 0),
  }),
  output: z.object({
    // requested: Cortex durably recorded the stop request (not yet proof the
    // process ended). refused: Cortex or this bridge did not accept it.
    // unconfirmed: no answer in time; retrying with the same key is safe.
    status: z.enum(["requested", "refused", "unconfirmed"]),
    message: z.string().optional(),
  }),
});

export type CommandStopResult = z.output<typeof requestCommandStop["output"]>;
