import type { PluginHandlerContext } from "@getpaseo/plugin/server";
import { readRunningCommands, type CommandStopResult } from "../shared/command-stop.ts";
import { commandStopSender, isUnconfirmedStop, type CommandStopSend } from "./command-stop-registry.ts";
import { parsePaseoRuntimeSessionId, resolveAgentTask } from "./worker-history.ts";
import { runStoreReader } from "./store-reader.ts";

type RunningCommands = ReturnType<typeof readRunningCommands["output"]["parse"]>;

// Read by `cortex-paseo-tools running-commands`: a started gateway
// command of a live worker channel, current owner epoch and non-terminal
// task, with no exit recorded. "Running" means running or unresolved; only
// Cortex's runner can confirm a stop.
function unavailable(message: string): RunningCommands {
  return { available: false, commands: [], message };
}

/** The running or unresolved worker commands of one task, read-only. */
export async function readRunningCommandsInStore(projectRoot: string, taskId: string, assignmentRef?: string): Promise<RunningCommands> {
  const result = await runStoreReader("running-commands", [projectRoot, taskId, assignmentRef ?? ""], { timeoutMs: 5000, maxBuffer: 4 * 1024 * 1024 });
  if (result.status !== 0 || !result.stdout) return unavailable(result.refusal ?? "Running commands could not be read.");
  try {
    return readRunningCommands.output.parse(JSON.parse(result.stdout));
  } catch {
    return unavailable("Running commands returned an invalid response.");
  }
}

export async function getRunningCommands(input: { workspaceId: string; agentId: string; assignmentRef?: string }, context: PluginHandlerContext): Promise<RunningCommands> {
  const resolved = await resolveAgentTask(input, context);
  if ("message" in resolved) return unavailable(resolved.message);
  return readRunningCommandsInStore(resolved.projectRoot, resolved.taskId, input.assignmentRef);
}

/** The native ACP session bound to this exact Paseo agent, or null. */
async function agentSession(input: { workspaceId: string; agentId: string }, context: PluginHandlerContext): Promise<string | null> {
  const handle = context.paseo.agents.ref(input.agentId);
  await handle.refresh();
  const agent = handle.current();
  if (!agent || agent.workspaceId !== input.workspaceId) return null;
  return parsePaseoRuntimeSessionId(agent.runtimeInfo);
}

/** A readable reason for a stop request Cortex did not accept. */
export function stopRefusalMessage(detail: string): string {
  const text = detail.trim().slice(0, 300);
  if (/command_stop_not_running/.test(text)) return "This command has already finished or is no longer a running worker command of this task. Nothing was stopped.";
  if (/command_stop_session_not_owner/.test(text)) return "This chat no longer owns the Cortex task. Nothing was stopped.";
  if (/request_key_conflict/.test(text)) return "This stop request conflicts with an earlier one. Edit the reason and try again.";
  return `Cortex did not accept the stop request${text ? ` (${text})` : ""}.`;
}

export async function stopCommandHandler(
  input: { workspaceId: string; agentId: string; toolCallRef: string; reason: string; requestKey: string },
  context: PluginHandlerContext,
  lookup: (sessionId: string) => CommandStopSend | undefined = commandStopSender,
): Promise<CommandStopResult> {
  const sessionId = await agentSession(input, context);
  if (!sessionId) return { status: "refused", message: "This chat is not bound to a Cortex session. Nothing was stopped." };
  const send = lookup(sessionId);
  if (!send) {
    return { status: "refused", message: "This chat has no live Cortex connection. Nothing was stopped. "
      + "Reopen the chat and try again, or use /cortex-kill with the command reference and a reason." };
  }
  try {
    await send({ toolCallRef: input.toolCallRef, reason: input.reason.trim(), requestKey: input.requestKey });
    return { status: "requested" };
  } catch (error) {
    if (isUnconfirmedStop(error)) {
      return { status: "unconfirmed", message: "Cortex did not confirm the stop request in time. It may still apply; retry to check." };
    }
    return { status: "refused", message: stopRefusalMessage(error instanceof Error ? error.message : "") };
  }
}
