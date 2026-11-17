import assert from "node:assert/strict";
import { runProcess, STORE_READER_CWD } from "./process-runner.ts";

// argv passthrough, stdin (kept out of argv), an explicit environment and
// working directory; a nonzero exit still returns what the child wrote.
{
  const echo = await runProcess("/bin/sh", ["-c", "printf '%s' \"$1\"", "sh", "hello"], { timeoutMs: 5000 });
  assert.deepEqual(echo, { status: 0, stdout: "hello" });
  const piped = await runProcess("/bin/sh", ["-c", "cat; printf '%s%s' \"$CORTEX_RUNNER_TEST\" \"$(pwd)\""], {
    timeoutMs: 5000, input: "in:", env: { CORTEX_RUNNER_TEST: "env:" }, cwd: STORE_READER_CWD,
  });
  assert.deepEqual(piped, { status: 0, stdout: "in:env:/" });
  const refused = await runProcess("/bin/sh", ["-c", "echo body; exit 1"], { timeoutMs: 5000 });
  assert.deepEqual(refused, { status: 1, stdout: "body\n" });
  const missing = await runProcess("/nonexistent/cortex-tool", [], { timeoutMs: 5000 });
  assert.equal(missing.status, 1);
}
{
  // A slow child exceeding the timeout is reported as a failure, not hung.
  const result = await runProcess("/bin/sh", ["-c", "sleep 2"], { timeoutMs: 100 });
  assert.notEqual(result.status, 0);
}

// N concurrent calls must overlap rather than serialize: 20 calls of a fixed
// cost finish in well under 20x a single call.
{
  const run = () => runProcess("/bin/sh", ["-c", "sleep 0.05"], { timeoutMs: 5000 });
  const singleStart = Date.now();
  await run();
  const singleCostMs = Date.now() - singleStart;
  const concurrency = 20;
  const concurrentStart = Date.now();
  await Promise.all(Array.from({ length: concurrency }, run));
  const concurrentTotalMs = Date.now() - concurrentStart;
  const serializedFloorMs = concurrency * singleCostMs;
  assert.ok(
    concurrentTotalMs < serializedFloorMs * 0.7,
    `expected overlap: ${concurrency} concurrent calls took ${concurrentTotalMs}ms, a serialized run would take ~${serializedFloorMs}ms`,
  );
}

console.log("process-runner: ok");
