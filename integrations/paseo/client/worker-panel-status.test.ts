import assert from "node:assert/strict";
import { workerSection, workerStatus } from "./worker-panel-status.ts";

assert.equal(workerSection("planned", 0), "active");
assert.equal(workerSection("running", 0), "active");
assert.equal(workerStatus("provisioning", 0), "running");
assert.equal(workerSection("reported", 0), "completed");
assert.equal(workerStatus("reported", 0), "completed");
assert.equal(workerSection("failed", 2), "completed");
assert.equal(workerSection("unknown", 1), "active");
console.log("worker-panel status tests passed");
