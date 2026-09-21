import { strict as assert } from "node:assert";
import { workerDisplayName } from "./worker-display.ts";

assert.equal(workerDisplayName({ role: "explorer", title: "delegate:b2c3d4e5-f6a7-4b8c-9d0e-234567890abc", nodeKey: "worker" }), "explorer");
assert.equal(workerDisplayName({ role: "worker-explorer", title: "Explore the project", nodeKey: "worker" }), "explorer");
assert.equal(workerDisplayName({ title: "delegate:b2c3d4e5-f6a7-4b8c-9d0e-234567890abc", nodeKey: "explorer" }), "explorer");
assert.equal(workerDisplayName({ title: "Explore the project", nodeKey: "worker" }), "Explore the project");
console.log("worker display tests passed");
