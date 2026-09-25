import type { WorkerResult } from "../shared/worker-timeline-result.ts";

type TerminalResult = NonNullable<WorkerResult>;
type ResultsRpc = (input: { workspaceId: string; agentId: string; assignmentRefs: string[] }) =>
  Promise<{ available: boolean; results: Record<string, TerminalResult | null> }>;

// Immutable once set: a completed worker's result never changes, so a cache
// hit here means the caller skips the RPC entirely, even across remounts
// (e.g. reopening a session the user visited earlier this run).
const resultCache = new Map<string, TerminalResult>();

function cacheKey(workspaceId: string, agentId: string, assignmentRef: string): string {
  return `${workspaceId}\u0000${agentId}\u0000${assignmentRef}`;
}

/** A completed worker's result, if the fallback path has already fetched
 * one for this exact assignment ref (e.g. an earlier mount of this card). */
export function peekCachedWorkerResult(workspaceId: string, agentId: string, assignmentRef: string): TerminalResult | undefined {
  return resultCache.get(cacheKey(workspaceId, agentId, assignmentRef));
}

type Group = {
  refs: Set<string>;
  listeners: Map<string, Set<(result: TerminalResult | null) => void>>;
  timer: ReturnType<typeof setTimeout> | null;
};
const groups = new Map<string, Group>();

// Coalesce every card that mounts within one short tick into a single RPC
// instead of one per card -- this is what makes the fallback "one
// lightweight batched RPC", not N of them, for an N-card timeline.
const BATCH_WINDOW_MS = 25;

function flush(groupKey: string, workspaceId: string, agentId: string, rpc: ResultsRpc): void {
  const group = groups.get(groupKey);
  if (!group) return;
  groups.delete(groupKey);
  const refs = [...group.refs];
  rpc({ workspaceId, agentId, assignmentRefs: refs })
    .then((response) => {
      for (const ref of refs) {
        const result = response.results[ref] ?? null;
        if (result) resultCache.set(cacheKey(workspaceId, agentId, ref), result);
        for (const listener of group.listeners.get(ref) ?? []) listener(result);
      }
    })
    .catch(() => {
      // A result is supplementary card content; a failed batched read
      // simply leaves those cards without one, never a stuck spinner.
      for (const ref of refs) for (const listener of group.listeners.get(ref) ?? []) listener(null);
    });
}

/** Register this card's assignment ref into the next batched fallback read
 * for its workspace+agent. A cache hit calls back synchronously and issues
 * no RPC. Returns an unsubscribe for the caller's cleanup. */
export function requestWorkerResult(
  rpc: ResultsRpc, workspaceId: string, agentId: string, assignmentRef: string,
  onResult: (result: TerminalResult | null) => void,
): () => void {
  const cached = resultCache.get(cacheKey(workspaceId, agentId, assignmentRef));
  if (cached) {
    onResult(cached);
    return () => {};
  }
  const groupKey = `${workspaceId}\u0000${agentId}`;
  let group = groups.get(groupKey);
  if (!group) {
    group = { refs: new Set(), listeners: new Map(), timer: null };
    groups.set(groupKey, group);
  }
  group.refs.add(assignmentRef);
  let listeners = group.listeners.get(assignmentRef);
  if (!listeners) {
    listeners = new Set();
    group.listeners.set(assignmentRef, listeners);
  }
  listeners.add(onResult);
  if (!group.timer) {
    group.timer = setTimeout(() => flush(groupKey, workspaceId, agentId, rpc), BATCH_WINDOW_MS);
  }
  return () => {
    listeners.delete(onResult);
  };
}
