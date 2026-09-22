import { strict as assert } from "node:assert";
import { contextSummary, workerAssignmentLine, workerCardMetadata, workerDisplayName } from "./worker-display.ts";

assert.equal(workerDisplayName({ role: "explorer", title: "delegate:b2c3d4e5-f6a7-4b8c-9d0e-234567890abc", nodeKey: "worker" }), "explorer");
assert.equal(workerDisplayName({ role: "worker-explorer", title: "Explore the project", nodeKey: "worker" }), "explorer");
assert.equal(workerDisplayName({ title: "delegate:b2c3d4e5-f6a7-4b8c-9d0e-234567890abc", nodeKey: "explorer" }), "explorer");
assert.equal(workerDisplayName({ title: "Explore the project", nodeKey: "worker" }), "Explore the project");
// The Workers popover shows exactly the chat row's name and grey line.
const chat = workerCardMetadata("Worker: debugger · codex · gpt-5.6-sol · high — Map the trailing-stop lifecycle", "running");
assert.deepEqual(chat, { workerName: "Map the trailing-stop lifecycle", subtitle: "debugger · codex · gpt-5.6-sol · high · working" });
assert.deepEqual(workerAssignmentLine({
  id: "a", nodeKey: "code_path_audit", role: "worker-debugger", title: "Map the trailing-stop lifecycle",
  route: { provider: "codex", model: "gpt-5.6-sol", effort: "high" }, attempts: [],
} as never, "running"), chat);
assert.equal(workerCardMetadata("Worker: debugger — Audit", "canceled").subtitle, "debugger · cancelled");
console.log("worker display tests passed");

assert.equal(contextSummary(undefined), null);
assert.deepEqual(contextSummary({ used: 84_300, size: 200_000 }), { percent: 42, detail: "84k / 200k", level: "normal" });
assert.deepEqual(contextSummary({ used: 950_000, size: 1_000_000 }), { percent: 95, detail: "950k / 1M", level: "danger" });
assert.equal(contextSummary({ used: 300_000, size: 200_000 })?.percent, 100);
