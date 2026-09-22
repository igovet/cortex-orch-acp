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

// Durable message identities keep same-message chunks together while
// preventing an accidental merge when separate engine messages are adjacent.
const identified = groupWorkerEvents([
  event(40, { text: "in_progress · [worker message] before-", message_id: "turn:0:segment:0" }),
  event(41, { text: "in_progress · [worker message] tool", message_id: "turn:0:segment:0" }),
  event(42, { text: "in_progress · [worker message] after", message_id: "turn:0:segment:1" }),
  event(43, { text: "in_progress · [worker message] -tool", message_id: "turn:0:segment:1" }),
]);
assert.deepEqual(identified.map((row) => row.event.text), ["before-tool", "after-tool"]);

// Even if a malformed/replayed producer repeats an id across a visible tool,
// grouping remains timeline-local and never joins opposite sides.
const repeatedIdAroundTool = groupWorkerEvents([
  event(50, { text: "in_progress · [worker message] before", message_id: "same" }),
  event(51, { activity_ref: "tool:read", tool_kind: "read", text: "Read README" }),
  event(52, { text: "in_progress · [worker message] after", message_id: "same" }),
]);
assert.deepEqual(repeatedIdAroundTool.map((row) => row.event.text), ["before", "Read README", "after"]);

assert.equal(groupWorkerEvents([event(1, { type: "delegation_requested" }), event(2, { type: "assignment_ready" }), event(3, { type: "worker_result" })]).length, 3);
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
assert.deepEqual(protocolRows.map((row) => row.event.id), [20, 21, 22, 23, 24, 25, 26]);
assert.equal(protocolRows[4].event.text, "maturity/state ");
assert.equal(protocolRows[5].event.text, "[final response] assessment.");
assert.equal(protocolRows[6].event.text, "real worker message");
// The final-response event can restate every prior worker-message segment
// glued together with no separator; that duplicate must not render again.
const duplicated = groupWorkerEvents([
  event(60, { activity_ref: "message", text: "[worker message] Now let me look at an existing module. " }),
  event(61, { activity_ref: "tool:read", tool_kind: "read", text: "Read a.md" }),
  event(62, { activity_ref: "message", text: "[worker message] Good, vi.mock is the pattern." }),
  event(63, {
    activity_ref: "final-response",
    text: "[final response] Now let me look at an existing module. Good, vi.mock is the pattern.",
  }),
]);
assert.deepEqual(duplicated.map((row) => row.event.id), [60, 61, 62]);

// A final-response that adds real content past the shown prefix keeps only
// the new part.
const partialDuplicate = groupWorkerEvents([
  event(70, { activity_ref: "message", text: "[worker message] Read the file." }),
  event(71, { activity_ref: "final-response", text: "[final response] Read the file. Here is the summary." }),
]);
assert.equal(partialDuplicate[partialDuplicate.length - 1].event.text, "[final response] Here is the summary.");

// A final-response with genuinely new text (no prior chatter) is unaffected.
const freshFinal = groupWorkerEvents([
  event(80, { activity_ref: "final-response", text: "[final response] Answer." }),
]);
assert.equal(freshFinal[freshFinal.length - 1].event.text, "[final response] Answer.");

console.log("worker-panel event grouping tests passed");

const filtered = groupWorkerEvents([
  event(80, { tool_kind: "agent_thought_chunk", text: "internal thought" }),
  event(81, { content: { sessionUpdate: "agent_thought_chunk", content: { text: "thought" } } }),
  event(82, { title: "mcp__claude__wait" }),
  event(83, { activity_ref: "tool:read", title: "mcp.cortex.read_skill", tool_kind: "read" }),
  event(84, { type: "assignment_failed", text: "Real failure" }),
  event(85, { text: "[final response] Answer" }),
]);
assert.deepEqual(filtered.map(row => row.event.id), [83, 84, 85]);
