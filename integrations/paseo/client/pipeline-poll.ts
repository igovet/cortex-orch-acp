import type { Pipeline } from "../shared/pipeline.ts";
import { SharedPoller } from "./rpc-subscribe.ts";

type PipelineInput = { workspaceId: string; agentId: string };
type PipelineRpc = (input: PipelineInput) => Promise<Pipeline>;

function keyOf(input: PipelineInput): string {
  return `${input.workspaceId}\u0000${input.agentId}`;
}

// `PipelinePanel` is mounted both as the composer-pill popover content and
// as the agent panel tab at once; both request the identical
// `{workspaceId, agentId}` pipeline. One shared poller means the pair share
// one timer and one RPC per tick instead of two independent ones.
let poller: SharedPoller<PipelineInput, Pipeline> | null = null;

function sharedPoller(rpc: PipelineRpc): SharedPoller<PipelineInput, Pipeline> {
  if (!poller) poller = new SharedPoller(rpc, keyOf);
  return poller;
}

/** Subscribe to a shared, deduplicated poll of `readPipeline` for this
 * (workspaceId, agentId). Concurrent callers share one timer, ticking at the
 * fastest currently-requested `intervalMs`. Returns the effect cleanup. */
export function subscribePipelinePoll(
  rpc: PipelineRpc, input: PipelineInput, intervalMs: number, onResult: (result: Pipeline | null) => void,
): () => void {
  return sharedPoller(rpc).subscribe(input, intervalMs, onResult);
}

/** Test-only: drop the module-level singleton so tests get an isolated poller. */
export function resetPipelinePollerForTests(): void {
  poller = null;
}
