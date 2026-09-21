import assert from "node:assert/strict";
import { groupWorkerEvents, type WorkerEvent } from "./worker-panel-events.ts";

const event = (id: number, changes: Partial<WorkerEvent>): WorkerEvent => ({ id, type: "progress", createdAt: String(id), text: `event ${id}`, ...changes });
const rows = groupWorkerEvents([
  event(6, { activity_ref: "a", status: "completed", output: { done: true } }),
  event(1, { activity_ref: "a", status: "pending", raw_input: { command: "test" } }),
  event(2, { type: "worker_message", text: "prose boundary" }),
  event(3, { activity_ref: "b", status: "in_progress" }),
  event(4, { activity_ref: "a", status: "in_progress" }),
  event(5, { activity_ref: "b", status: "failed", error: "error" }),
]);
assert.deepEqual(rows.map((row) => row.event.id), [1, 2, 3]);
assert.deepEqual(rows.map((row) => row.updates.map((update) => update.id)), [[1, 4, 6], [2], [3, 5]]);
assert.equal(rows[0].event.status, "completed");
assert.deepEqual(rows[0].event.raw_input, { command: "test" });
assert.deepEqual(rows[0].event.output, { done: true });
assert.equal(rows[2].event.status, "failed");
assert.equal(rows[2].event.error, "error");
assert.equal(groupWorkerEvents([event(1, { title: "same" }), event(2, { title: "same" })]).length, 2);
const chunks = groupWorkerEvents([
  event(10, { text: "in_progress · [worker message] Read" }),
  event(11, { text: "in_progress · [worker message]  README" }),
  event(12, { activity_ref: "tool", tool_kind: "read", text: "Read README" }),
  event(13, { text: "in_progress · [worker message] Now" }),
]);
assert.deepEqual(chunks.map((row) => row.event.text), ["Read README", "Read README", "Now"]);
assert.deepEqual(chunks.map((row) => row.updates.length), [2, 1, 1]);

// Provider streamed prose uses one activity ref for the whole message. Every
// chunk must survive projection and only adjacent prose may be coalesced.
const streamedMessage = groupWorkerEvents([
  event(30, { activity_ref: "message", text: "[worker message] evidence-" }),
  event(31, { activity_ref: "message", text: "[worker message] backed " }),
  event(32, { activity_ref: "tool:read", tool_kind: "read", text: "Read README" }),
  event(33, { activity_ref: "message", text: "[worker message] report." }),
  event(34, { activity_ref: "final-response", text: "[final response] complete." }),
]);
assert.deepEqual(streamedMessage.map((row) => row.event.text), [
  "evidence-backed ", "Read README", "report.", "[final response] complete.",
]);
assert.deepEqual(streamedMessage.map((row) => row.updates.map((update) => update.id)), [[30, 31], [32], [33], [34]]);

assert.equal(groupWorkerEvents([event(1, { type: "delegation_requested" }), event(2, { type: "assignment_ready" }), event(3, { type: "worker_result" })]).length, 1);
const failedTool = groupWorkerEvents([
  event(1, { activity_ref: "tool:tc1", tool_kind: "read", status: "in_progress" }),
  event(2, { activity_ref: "tool-error:tc1", status: "failed", error: "denied" }),
]);
assert.equal(failedTool.length, 1);
assert.deepEqual(failedTool[0].updates.map((update) => update.id), [1, 2]);
assert.equal(failedTool[0].event.error, "denied");
const protocolRows = groupWorkerEvents([
  event(20, { type: "maturity", text: "maturity" }),
  event(21, { type: "state", text: "state" }),
  event(22, { type: "maturity/state", text: "maturity/state" }),
  event(23, { type: "coordinator_decision", text: "assessment." }),
  event(24, { type: "progress", text: "[worker message] maturity/state " }),
  event(25, { type: "worker_result", text: "[final response] assessment." }),
  event(26, { type: "worker_message", text: "real worker message" }),
]);
assert.deepEqual(protocolRows.map((row) => row.event.id), [24, 25, 26]);
assert.equal(protocolRows[0].event.text, "maturity/state ");
assert.equal(protocolRows[1].event.text, "[final response] assessment.");
assert.equal(protocolRows[2].event.text, "real worker message");
console.log("worker-panel event grouping tests passed");
