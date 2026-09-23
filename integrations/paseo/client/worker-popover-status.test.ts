import { strict as assert } from "node:assert";
import { workerPopoverStatus, orderPopoverWorkers } from "./worker-popover-status.ts";

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

const dated = (id: string, status: string, completedAt?: string) => {
  const row = assignment(status, []);
  return { ...row, id, attempts: [{ ...row.attempts[0], completedAt }] };
};
const rows = [dated("old", "completed", "2026-09-21T12:00:00Z"), dated("running", "running"), dated("new", "completed", "2026-09-21T14:00:00Z"), dated("unknown-date", "failed")];
assert.deepEqual(orderPopoverWorkers(rows).map(a => a.id), ["running", "new", "old", "unknown-date"]);
assert.equal(rows[0].id, "old");
