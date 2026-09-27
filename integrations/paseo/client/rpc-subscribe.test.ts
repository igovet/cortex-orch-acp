import assert from "node:assert/strict";
import { createSharedPoller } from "./rpc-subscribe.ts";

const tick = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

// Two subscribers with the identical key share one timer and one RPC per
// tick, not one each -- the exact popover+panel / tab+popover duplication
// this module exists to remove.
{
  let calls = 0;
  const rpc = async (input: { workspaceId: string; agentId: string }) => { calls += 1; return { seen: calls, ...input }; };
  const poller = createSharedPoller(rpc, (input) => `${input.workspaceId}\u0000${input.agentId}`);
  const results: unknown[] = [];
  const unsubA = poller.subscribe({ workspaceId: "ws", agentId: "agent" }, 20, (r) => results.push(["a", r]));
  const unsubB = poller.subscribe({ workspaceId: "ws", agentId: "agent" }, 20, (r) => results.push(["b", r]));
  await tick(10);
  // The immediate refresh on group creation fires once, delivered to both.
  assert.equal(calls, 1);
  assert.equal(results.length, 2);
  unsubA();
  unsubB();
}

// Distinct keys never share a poll.
{
  const seenKeys: string[] = [];
  const rpc = async (input: { id: string }) => { seenKeys.push(input.id); return input; };
  const poller = createSharedPoller(rpc, (input) => input.id);
  const unsubA = poller.subscribe({ id: "one" }, 1000, () => {});
  const unsubB = poller.subscribe({ id: "two" }, 1000, () => {});
  await tick(10);
  assert.deepEqual(seenKeys.sort(), ["one", "two"]);
  unsubA();
  unsubB();
}

// The shared timer runs at the fastest currently-subscribed interval.
{
  let calls = 0;
  const rpc = async () => { calls += 1; return { calls }; };
  const poller = createSharedPoller(rpc, () => "same-key");
  const unsubSlow = poller.subscribe({}, 1000, () => {});
  await tick(5);
  assert.equal(calls, 1, "the initial immediate refresh");
  const unsubFast = poller.subscribe({}, 20, () => {});
  await tick(60);
  assert.ok(calls >= 3, `expected the fast subscriber's interval to drive the shared timer, got ${calls} calls`);
  unsubSlow();
  unsubFast();
}

// A failed RPC delivers null to every listener, never a stuck/undelivered
// subscriber; and it does not hang the shared timer.
{
  let attempts = 0;
  const rpc = async () => { attempts += 1; throw new Error("network"); };
  const poller = createSharedPoller(rpc, () => "err-key");
  const results: unknown[] = [];
  const unsub = poller.subscribe({}, 20, (r) => results.push(r));
  await tick(10);
  assert.deepEqual(results, [null]);
  assert.equal(attempts, 1);
  unsub();
}

// Unsubscribing the last listener stops the shared timer (no further calls).
{
  let calls = 0;
  const rpc = async () => { calls += 1; return {}; };
  const poller = createSharedPoller(rpc, () => "stop-key");
  const unsub = poller.subscribe({}, 15, () => {});
  await tick(10);
  const callsAtUnsub = calls;
  unsub();
  await tick(80);
  assert.equal(calls, callsAtUnsub, "no further RPCs after the last listener unsubscribes");
}

console.log("rpc-subscribe: ok");
