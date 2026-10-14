import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { runProcess, runPython, STORE_READER_CWD } from "./python-runner.ts";

// Basic correctness: argv passthrough, stdout capture, exit-code mapping.
{
  const result = await runPython(["-B", "-c", "import sys; print(sys.argv[1])", "hello"], { timeoutMs: 5000 });
  assert.equal(result.status, 0);
  assert.equal(result.stdout, "hello\n");
}
{
  const result = await runPython(["-B", "-c", "import sys; sys.exit(3)"], { timeoutMs: 5000 });
  assert.notEqual(result.status, 0);
}
// Store readers run isolated: a planted stdlib-shadowing module on
// PYTHONPATH or in the daemon's working directory is never imported, and
// the working directory is fixed.
{
  const planted = mkdtempSync(join(tmpdir(), "cortex-runner-planted-"));
  const previousPath = process.env.PYTHONPATH;
  const previousCwd = process.cwd();
  try {
    writeFileSync(join(planted, "json.py"), "raise SystemExit(42)\n");
    process.env.PYTHONPATH = planted;
    process.chdir(planted);
    const result = await runPython(["-B", "-c", "import json, os, sys; print(json.dumps([os.getcwd(), sys.flags.isolated]))"], { timeoutMs: 5000 });
    assert.equal(result.status, 0);
    assert.deepEqual(JSON.parse(result.stdout), [STORE_READER_CWD, 1]);
  } finally {
    process.chdir(previousCwd);
    if (previousPath === undefined) delete process.env.PYTHONPATH;
    else process.env.PYTHONPATH = previousPath;
    rmSync(planted, { recursive: true, force: true });
  }
}
{
  // A slow script exceeding the timeout is reported as a failure, not hung.
  const result = await runPython(["-B", "-c", "import time; time.sleep(2)"], { timeoutMs: 100 });
  assert.notEqual(result.status, 0);
}

// runProcess: stdin (kept out of argv), an explicit environment and working
// directory; a nonzero exit still returns what the child wrote to stdout.
{
  const result = await runProcess("python3", ["-B", "-c", "import os, sys; print(sys.stdin.read() + os.environ['CORTEX_RUNNER_TEST'] + os.getcwd())"], {
    timeoutMs: 5000, input: "in:", env: { ...process.env, CORTEX_RUNNER_TEST: "env:" }, cwd: "/",
  });
  assert.equal(result.status, 0);
  assert.equal(result.stdout, "in:env:/\n");
  const refused = await runProcess("python3", ["-B", "-c", "import sys; print('body'); sys.exit(1)"], { timeoutMs: 5000 });
  assert.equal(refused.status, 1);
  assert.equal(refused.stdout, "body\n");
}

// Reproducible load test: N "concurrent" calls (root cause r_6df0ca1ceeb1's
// bench.mjs/control.mjs shape) must overlap rather than serialize. A short,
// fixed-cost script run 20-way concurrently through the async runner must
// finish in well under 20x a single call's cost -- the signature that
// distinguishes this from the `spawnSync`-blocking mechanism the report
// measured (~52ms/call at every concurrency level, i.e. exactly linear).
{
  const script = "import time; time.sleep(0.05)";
  const singleStart = Date.now();
  await runPython(["-B", "-c", script], { timeoutMs: 5000 });
  const singleCostMs = Date.now() - singleStart;

  const concurrency = 20;
  const concurrentStart = Date.now();
  await Promise.all(Array.from({ length: concurrency }, () => runPython(["-B", "-c", script], { timeoutMs: 5000 })));
  const concurrentTotalMs = Date.now() - concurrentStart;

  // A fully serialized run would cost ~concurrency * singleCostMs (the
  // `spawnSync` mechanism this replaces). A bound well under that -- even
  // allowing generous scheduling noise and the concurrency cap -- proves
  // overlap rather than serialization.
  const serializedFloorMs = concurrency * singleCostMs;
  assert.ok(
    concurrentTotalMs < serializedFloorMs * 0.7,
    `expected overlap: ${concurrency} concurrent calls took ${concurrentTotalMs}ms, ` +
      `a fully serialized run would take ~${serializedFloorMs}ms (single call ${singleCostMs}ms)`,
  );
}

console.log("python-runner: ok");
