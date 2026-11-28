import type { PluginHandlerContext } from "@getpaseo/plugin/server";
import { readWorkerHistory } from "../shared/worker-history.ts";
import { runStoreReader, storeReaderRefusal } from "./store-reader.ts";

/** The runtime keys its one store by the logical project root of the session
 * cwd: a verified linked Git worktree shares its main repository's store, so
 * a worktree workspace directory has no store of its own. `cortex-paseo-tools
 * project-root` mirrors that resolution read-only; any ambiguous, forged or
 * unsafe layout exits nonzero instead of guessing. */
/** The runtime's logical project root for one workspace directory, or null. */
export async function resolveProjectRoot(directory: string): Promise<string | null> {
  const result = await runStoreReader("project-root", [directory], { timeoutMs: 5000 });
  const root = result.status === 0 && result.stdout.endsWith("\n") ? result.stdout.slice(0, -1) : "";
  return root.startsWith("/") && !root.includes("\0") && !root.includes("\n") ? root : null;
}

const MAX_RUNTIME_SESSION_WRAPPER_LENGTH = 512;
const MAX_RUNTIME_SESSION_PAYLOAD_LENGTH = 480;
const ACP_SESSION_ID = /^acps_[0-9a-f]{12}$/;

/**
 * Paseo 0.8 stores the ACP identity in a versioned plugin-owned wrapper.
 * Keep this boundary deliberately narrow: runtimeInfo is client supplied and
 * must never become a source of project roots, task ids, or arbitrary SQL
 * identities. Unknown versions/shapes and conflicting identities fail closed.
 */
export function parsePaseoRuntimeSessionId(runtimeInfo: unknown): string | null {
  if (!isRecord(runtimeInfo) || !Object.prototype.hasOwnProperty.call(runtimeInfo, "sessionId")) {
    return null;
  }
  const wrapped = runtimeInfo.sessionId;
  if (typeof wrapped !== "string" || wrapped.length === 0 || wrapped.length > MAX_RUNTIME_SESSION_WRAPPER_LENGTH) {
    return null;
  }
  const prefix = "plugin:";
  if (!wrapped.startsWith(prefix)) return null;
  const payloadText = wrapped.slice(prefix.length);
  if (payloadText.length === 0 || payloadText.length > MAX_RUNTIME_SESSION_PAYLOAD_LENGTH) return null;
  // Paseo's v1 wrapper is emitted as plain JSON with these literal keys.
  // Reject escapes before JSON.parse: otherwise `\u0073essionId` can become
  // a second logical key after parsing while evading raw duplicate checks.
  if (payloadText.includes("\\")) return null;
  // JSON.parse keeps the last value for duplicate object keys. Reject those
  // known identity keys before parsing so an ambiguous wrapper cannot choose
  // a different session by ordering.
  for (const key of ["version", "data", "sessionId"]) {
    const occurrences = payloadText.match(new RegExp(`\\"${key}\\"\\s*:`, "g"))?.length ?? 0;
    if (occurrences !== 1) return null;
  }

  let payload: unknown;
  try {
    payload = JSON.parse(payloadText);
  } catch {
    return null;
  }
  if (!isRecord(payload) || payload.version !== 1 || !isRecord(payload.data)) return null;

  const payloadKeys = Object.keys(payload);
  const dataKeys = Object.keys(payload.data);
  if (payloadKeys.length !== 2 || !payloadKeys.includes("version") || !payloadKeys.includes("data")) return null;
  if (dataKeys.length !== 1 || dataKeys[0] !== "sessionId") return null;

  const sessionId = payload.data.sessionId;
  return typeof sessionId === "string" && ACP_SESSION_ID.test(sessionId) ? sessionId : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

// `resolveAgentTask`'s two identity spawns (`resolveProjectRoot`,
// `readTaskId`) are invariant for a given (workspaceId, agentId) for the
// life of the agent's bound session (measured: ~60% of a single call's
// cost). Cached here, keyed by that pair and invalidated whenever the
// workspace directory or the agent's bound session id changes underneath
// it, so a stale or reassigned agent never reads another session's task.
type AgentTaskCacheEntry = { directory: string; sessionId: string; projectRoot: string; taskId: string };
const agentTaskCache = new Map<string, AgentTaskCacheEntry>();

function agentTaskCacheKey(workspaceId: string, agentId: string): string {
  return `${workspaceId}\u0000${agentId}`;
}

/** Resolve the exact Cortex task bound to this Paseo agent, or say why not. */
export async function resolveAgentTask(input: { workspaceId: string; agentId: string }, context: PluginHandlerContext): Promise<{ projectRoot: string; taskId: string } | { message: string }> {
  const workspaceHandle = context.paseo.workspaces.ref(input.workspaceId);
  await workspaceHandle.refresh();
  const directory = workspaceHandle.directory;
  if (!directory || !directory.startsWith("/") || directory.includes("\0")) return { message: "Worker history is unavailable for this workspace." };
  // The Rust launch without its store-reader binary: say so, never fall back.
  const refusal = storeReaderRefusal();
  if (refusal) return { message: refusal };
  const agentHandle = context.paseo.agents.ref(input.agentId);
  await agentHandle.refresh();
  const agent = agentHandle.current();
  const sessionId = parsePaseoRuntimeSessionId(agent?.runtimeInfo);
  if (!sessionId || agent?.workspaceId !== input.workspaceId) return { message: "Worker history is unavailable for this agent." };

  const cacheKey = agentTaskCacheKey(input.workspaceId, input.agentId);
  const cached = agentTaskCache.get(cacheKey);
  if (cached && cached.directory === directory && cached.sessionId === sessionId) {
    return { projectRoot: cached.projectRoot, taskId: cached.taskId };
  }

  const projectRoot = await resolveProjectRoot(directory);
  if (!projectRoot) return { message: "Worker history is unavailable for this workspace." };
  const binding = await runStoreReader("task-id", [projectRoot, sessionId], { timeoutMs: 5000 });
  if (binding.status !== 0) return { message: binding.refusal ?? "The Cortex project store could not be read." };
  const taskId = binding.stdout.trim();
  if (!taskId) return { message: "No durable Cortex task is bound to this agent." };
  agentTaskCache.set(cacheKey, { directory, sessionId, projectRoot, taskId });
  return { projectRoot, taskId };
}

export async function getWorkerHistory(input: { workspaceId: string; agentId: string; attemptId?: string; assignmentRef?: string; cursor?: number; limit?: number; assignmentCursor?: string; compact?: boolean }, context: PluginHandlerContext) {
  const resolved = await resolveAgentTask(input, context);
  if ("message" in resolved) return { assignments: [], available: false, truncated: false, nextCursor: null, nextAssignmentCursor: null, message: resolved.message };
  const { projectRoot, taskId } = resolved;
  const result = await runStoreReader("worker-history", [projectRoot, taskId, input.attemptId ?? "", String(input.cursor ?? 0), String(input.limit ?? 200), input.assignmentCursor ?? "", input.assignmentRef ?? "", input.compact ? "1" : ""], {
    timeoutMs: 5000,
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.status !== 0 || !result.stdout) {
    return { assignments: [], available: false, truncated: false, nextCursor: null, nextAssignmentCursor: null, message: result.refusal ?? "Worker history could not be read." };
  }
  try {
    return readWorkerHistory.output.parse(JSON.parse(result.stdout));
  } catch {
    return { assignments: [], available: false, truncated: false, nextCursor: null, nextAssignmentCursor: null, message: "Worker history returned an invalid response." };
  }
}
