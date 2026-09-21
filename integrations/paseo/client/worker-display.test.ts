import { strict as assert } from "node:assert";
import { workerAssignmentLine, workerCardMetadata, workerDisplayName } from "./worker-display.ts";

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
