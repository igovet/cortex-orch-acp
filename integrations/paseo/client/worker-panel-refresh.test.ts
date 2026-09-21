import { strict as assert } from "node:assert";
import { historyCursor, mergeWorkerHistory, shouldPollWorkerHistory, shouldShowWorkerHistoryLoader, WORKER_HISTORY_POLL_MS } from "./worker-panel-refresh.ts";

assert.equal(WORKER_HISTORY_POLL_MS, 750);
assert.equal(shouldShowWorkerHistoryLoader(null), true);
assert.equal(shouldShowWorkerHistoryLoader({ assignments: [], available: true, truncated: false, nextCursor: null, nextAssignmentCursor: null }), false);
assert.equal(shouldPollWorkerHistory(null), true);
assert.equal(shouldPollWorkerHistory({ assignments: [], available: false, truncated: false, nextCursor: null, nextAssignmentCursor: null }), true);
assert.equal(shouldPollWorkerHistory({ assignments: [], available: true, truncated: false, nextCursor: null, nextAssignmentCursor: null }), true);

const base = { taskId: "task-1", id: "assignment-1", nodeKey: "worker", status: "completed", attempts: [] };
assert.equal(shouldPollWorkerHistory({ assignments: [{ ...base, attempts: [{ id: "attempt-1", attemptNo: 1, status: "in_progress", failureReason: null, events: [] }] }], available: true, truncated: false, nextCursor: null, nextAssignmentCursor: null }), true);
assert.equal(shouldPollWorkerHistory({ assignments: [{ ...base, attempts: [{ id: "attempt-1", attemptNo: 1, status: "completed", failureReason: null, events: [] }] }], available: true, truncated: false, nextCursor: null, nextAssignmentCursor: null }), false);
assert.equal(shouldPollWorkerHistory({ assignments: [{ ...base, attempts: [{ id: "attempt-1", attemptNo: 1, status: "cancelled", failureReason: null, events: [] }] }], available: true, truncated: false, nextCursor: null, nextAssignmentCursor: null }), false);

const event = (id: number, text: string) => ({ id, type: "progress", createdAt: String(id), text });
const first = { assignments: [{ ...base, status: "in_progress", attempts: [{ id: "attempt-1", attemptNo: 1, status: "in_progress", failureReason: null, events: [event(1, "first")] }] }], available: true, truncated: false, nextCursor: null, nextAssignmentCursor: null };
const update = { assignments: [{ ...base, status: "completed", attempts: [{ id: "attempt-1", attemptNo: 1, status: "completed", failureReason: null, events: [event(2, "second")] }] }], available: true, truncated: false, nextCursor: null, nextAssignmentCursor: null };
const merged = mergeWorkerHistory(first, update);
assert.deepEqual(merged.assignments[0].attempts[0].events.map((item) => item.text), ["first", "second"]);
assert.equal(merged.assignments[0].attempts[0].status, "completed");
assert.equal(historyCursor(first), 1);
assert.equal(historyCursor(merged), 2);
assert.equal(mergeWorkerHistory(merged, update).assignments[0].attempts[0].events.length, 2);
const staleRunning = { ...update, assignments: [{ ...update.assignments[0], status: "running", attempts: [{ ...update.assignments[0].attempts[0], status: "running", events: [] }] }] };
assert.equal(mergeWorkerHistory(merged, staleRunning).assignments[0].attempts[0].status, "completed");
console.log("worker-panel refresh tests passed");

assert.equal(historyCursor(first, "unloaded-worker"), 0);
