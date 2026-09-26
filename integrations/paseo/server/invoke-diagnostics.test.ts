import assert from "node:assert/strict";
import { instrumentHandler } from "./invoke-diagnostics.ts";

const context = {} as any;
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

// A fast call passes through its result and logs nothing.
{
  const lines: string[] = [];
  const handler = instrumentHandler("cortex.example.read", async (input: { workspaceId: string; agentId: string }) => ({ ok: true }),
    { thresholdMs: 10_000, log: (line) => lines.push(line) });
  const result = await handler({ workspaceId: "ws", agentId: "agent" }, context);
  assert.deepEqual(result, { ok: true });
  assert.deepEqual(lines, []);
}

// A slow call (>= threshold) logs the exact method name, a duration, and a
// hashed (never raw) identity -- but never the input/output content.
{
  const lines: string[] = [];
  const handler = instrumentHandler("cortex.worker-history.read", async () => { await wait(15); return { secretBody: "do not log me" }; },
    { thresholdMs: 10, log: (line) => lines.push(line) });
  const result = await handler({ workspaceId: "workspace-1", agentId: "agent-1" }, context);
  assert.deepEqual(result, { secretBody: "do not log me" });
  assert.equal(lines.length, 1);
  assert.match(lines[0], /method=cortex\.worker-history\.read/);
  assert.match(lines[0], /durationMs=\d+/);
  assert.doesNotMatch(lines[0], /workspace-1/);
  assert.doesNotMatch(lines[0], /agent-1/);
  assert.doesNotMatch(lines[0], /secretBody|do not log me/);
}

// The same identity always hashes to the same value, so repeated slow calls
// for one session can be correlated across a log window.
{
  const lines: string[] = [];
  const handler = instrumentHandler("cortex.pipeline.read", async () => { await wait(15); return {}; },
    { thresholdMs: 10, log: (line) => lines.push(line) });
  await handler({ workspaceId: "ws-x", agentId: "agent-x" }, context);
  await handler({ workspaceId: "ws-x", agentId: "agent-x" }, context);
  assert.equal(lines.length, 2);
  const [, hashPart1] = lines[0].match(/workspace=(\S+)/)!;
  const [, hashPart2] = lines[1].match(/workspace=(\S+)/)!;
  assert.equal(hashPart1, hashPart2);
}

// A rejected handler still propagates, and a slow-but-failed call is still
// diagnosed (never silently swallowed by the instrumentation).
{
  const lines: string[] = [];
  const handler = instrumentHandler("cortex.worker-results.read", async () => { await wait(15); throw new Error("boom"); },
    { thresholdMs: 10, log: (line) => lines.push(line) });
  await assert.rejects(() => handler({ workspaceId: "ws", agentId: "agent" }, context), /boom/);
  assert.equal(lines.length, 1);
  assert.match(lines[0], /method=cortex\.worker-results\.read/);
}

console.log("invoke-diagnostics: ok");
