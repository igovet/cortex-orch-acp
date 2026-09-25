import { useRpc } from "@getpaseo/plugin/client";
import { useEffect, useState } from "react";
import { readWorkerResults } from "../shared/worker-results.ts";
import type { WorkerResult } from "../shared/worker-timeline-result.ts";
import { peekCachedWorkerResult, requestWorkerResult } from "./worker-result-batch.ts";

/**
 * The fallback for a worker card whose ACP item has no embedded result
 * (an older item, or a host that dropped the field): one batched RPC per
 * workspace+agent tick (`requestWorkerResult`), then the module cache for
 * every remount after. Disable while the card still has (or does not yet
 * need) a result -- a running card never shows one, and an embedded result
 * makes this hook redundant.
 */
export function useFallbackWorkerResult(
  workspaceId: string | null, agentId: string, assignmentRef: string, enabled: boolean,
): NonNullable<WorkerResult> | null {
  const rpc = useRpc(readWorkerResults);
  const [result, setResult] = useState<NonNullable<WorkerResult> | null>(
    () => (workspaceId ? peekCachedWorkerResult(workspaceId, agentId, assignmentRef) ?? null : null),
  );
  useEffect(() => {
    if (!workspaceId || !enabled) return;
    const cached = peekCachedWorkerResult(workspaceId, agentId, assignmentRef);
    if (cached) {
      setResult(cached);
      return;
    }
    return requestWorkerResult(rpc, workspaceId, agentId, assignmentRef, setResult);
  }, [agentId, assignmentRef, enabled, rpc, workspaceId]);
  return result;
}
