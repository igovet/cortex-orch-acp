import { strict as assert } from "node:assert";
import { workerPopoverStatus } from "./worker-popover-status.ts";

const assignment = (status: string, events: Array<{ type: string }>) => ({
  taskId: "task", id: "assignment", assignmentRef: "ref", nodeKey: "worker", status,
  attempts: [{ id: "attempt", attemptNo: 1, status, failureReason: null, events: events.map((event, id) => ({ id, ...event, createdAt: "now", text: event.type })) }],
});

assert.equal(workerPopoverStatus(assignment("running", [{ type: "assignment_cancelled" }])), "cancelled");
assert.equal(workerPopoverStatus(assignment("running", [{ type: "assignment_failed" }])), "failed");
assert.equal(workerPopoverStatus(assignment("running", [{ type: "worker_result" }])), "completed");
assert.equal(workerPopoverStatus(assignment("completed_with_issues", [{ type: "worker_result" }])), "completed_with_issues");
assert.equal(workerPopoverStatus(assignment("cancelled", [{ type: "worker_result" }])), "cancelled");
assert.equal(workerPopoverStatus(assignment("running", [])), "running");
console.log("worker popover status tests passed");
