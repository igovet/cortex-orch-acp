import assert from "node:assert/strict";
import { peekCachedWorkerResult, requestWorkerResult } from "./worker-result-batch.ts";

const tick = (ms = 40) => new Promise((resolve) => setTimeout(resolve, ms));

// Every card mounting inside the batch window is coalesced into one RPC
// call, not one per card -- the whole point of the fallback path.
{
  let calls = 0;
  const rpc = async (input: { workspaceId: string; agentId: string; assignmentRefs: string[] }) => {
    calls += 1;
    return {
      available: true,
      results: Object.fromEntries(input.assignmentRefs.map((ref) => [ref, { summary: `s-${ref}`, checks: [] }])),
    };
  };
  const seen: Record<string, unknown> = {};
  requestWorkerResult(rpc, "ws", "agent", "a1", (r) => { seen.a1 = r; });
  requestWorkerResult(rpc, "ws", "agent", "a2", (r) => { seen.a2 = r; });
  requestWorkerResult(rpc, "ws", "agent", "a3", (r) => { seen.a3 = r; });
  await tick();
  assert.equal(calls, 1, "three cards in one workspace+agent must share one RPC call");
  assert.deepEqual(seen.a1, { summary: "s-a1", checks: [] });
  assert.deepEqual(seen.a2, { summary: "s-a2", checks: [] });
  assert.deepEqual(seen.a3, { summary: "s-a3", checks: [] });
}

// The cache makes a remount of an already-resolved card free: no RPC at all.
{
  let calls = 0;
  const rpc = async (input: { workspaceId: string; agentId: string; assignmentRefs: string[] }) => {
    calls += 1;
    return { available: true, results: Object.fromEntries(input.assignmentRefs.map((ref) => [ref, { summary: "s", checks: [] }])) };
  };
  let first: unknown;
  requestWorkerResult(rpc, "ws2", "agent", "cached-ref", (r) => { first = r; });
  await tick();
  assert.equal(calls, 1);
  assert.deepEqual(peekCachedWorkerResult("ws2", "agent", "cached-ref"), { summary: "s", checks: [] });

  let second: unknown;
  const unsubscribe = requestWorkerResult(rpc, "ws2", "agent", "cached-ref", (r) => { second = r; });
  assert.equal(calls, 1, "a cache hit must not issue a second RPC");
  assert.deepEqual(second, { summary: "s", checks: [] });
  unsubscribe();
}

// Different workspace+agent groups never share a batch.
{
  const groupsSeen: string[][] = [];
  const rpc = async (input: { workspaceId: string; agentId: string; assignmentRefs: string[] }) => {
    groupsSeen.push([input.workspaceId, input.agentId]);
    return { available: true, results: {} };
  };
  requestWorkerResult(rpc, "ws-x", "agent-1", "r1", () => {});
  requestWorkerResult(rpc, "ws-y", "agent-2", "r2", () => {});
  await tick();
  assert.equal(groupsSeen.length, 2, "distinct workspace+agent pairs must not be batched together");
}

// A failed batched read resolves every pending card with null, never a stuck spinner.
{
  const rpc = async () => { throw new Error("network"); };
  let resolved: unknown = "not-called";
  requestWorkerResult(rpc, "ws3", "agent", "r3", (r) => { resolved = r; });
  await tick();
  assert.equal(resolved, null);
  assert.equal(peekCachedWorkerResult("ws3", "agent", "r3"), undefined);
}

console.log("worker-result-batch: ok");
