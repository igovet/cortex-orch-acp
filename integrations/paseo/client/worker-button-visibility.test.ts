import assert from "node:assert/strict";
import { hasCurrentSessionWorkers } from "./worker-button-visibility.ts";

assert.equal(hasCurrentSessionWorkers(undefined), false);
assert.equal(hasCurrentSessionWorkers(null), false);
assert.equal(hasCurrentSessionWorkers({ available: false, assignments: [] }), false);
assert.equal(hasCurrentSessionWorkers({ available: true, assignments: [] }), false);
assert.equal(hasCurrentSessionWorkers({ available: true, assignments: [{ taskId: "task", id: "assignment", nodeKey: "worker", status: "completed", attempts: [] }] }), true);
console.log("worker button visibility tests passed");
