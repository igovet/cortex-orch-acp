import assert from "node:assert/strict";
import test from "node:test";
import { groupWorkerEvents, type WorkerEvent } from "./worker-panel-events.ts";

const event = (id: number, fields: Partial<WorkerEvent>): WorkerEvent => ({
  id, type: "worker_activity", createdAt: String(id), text: "", ...fields,
});

// Real desktop reproduction: the stream preserves paragraph separators, but
// the final-response repeats the same segments with those separators trimmed.
// Synthetic prose preserves the observed shape without publishing live bodies.
test("final response does not repeat streamed paragraphs with trimmed boundaries", () => {
  const events = [
    event(10, { activity_ref: "message", message_id: "turn:0:segment:0",
      text: "in_progress · [worker message] Inspecting the fixture." }),
    event(20, { activity_ref: "tool:read", tool_kind: "read", text: "Read fixture.txt" }),
    event(30, { activity_ref: "message", message_id: "turn:0:segment:1",
      text: "in_progress · [worker message] \n\nValidating the result." }),
    event(40, { activity_ref: "final-response", message_id: "turn:0:segment:2",
      text: "in_progress · [final response] Inspecting the fixture.Validating the result.Finished successfully." }),
  ];
  for (const input of [events, [...events].reverse()]) {
    const rows = groupWorkerEvents(input);
    assert.deepEqual(rows.map(({ event }) => event.text), [
      "Inspecting the fixture.",
      "Read fixture.txt",
      "\n\nValidating the result.",
      "in_progress · [final response] Finished successfully.",
    ]);
  }
});

test("new final content and interior whitespace remain intact", () => {
  const rows = groupWorkerEvents([
    event(10, { activity_ref: "message", message_id: "turn:0:segment:0",
      text: "in_progress · [worker message] Inspecting the fixture." }),
    event(20, { activity_ref: "final-response", message_id: "turn:0:segment:1",
      text: "in_progress · [final response] Inspecting a different fixture.\n\nKeep  two spaces." }),
  ]);
  assert.equal(rows.at(-1)?.event.text,
    "in_progress · [final response] Inspecting a different fixture.\n\nKeep  two spaces.");
});

test("deduplication preserves whitespace inside streamed blocks and new final prose", () => {
  const rows = groupWorkerEvents([
    event(10, { activity_ref: "message", message_id: "turn:0:segment:0",
      text: "in_progress · [worker message] First  line.\nStill here." }),
    event(20, { activity_ref: "tool:read", tool_kind: "read", text: "Read fixture.txt" }),
    event(30, { activity_ref: "message", message_id: "turn:0:segment:1",
      text: "in_progress · [worker message] \n\nSecond paragraph." }),
    event(40, { activity_ref: "final-response",
      text: "in_progress · [final response] First  line.\nStill here.Second paragraph.\n\nNew  content." }),
  ]);
  assert.deepEqual(rows.map(({ event }) => event.text), [
    "First  line.\nStill here.", "Read fixture.txt", "\n\nSecond paragraph.",
    "in_progress · [final response] New  content.",
  ]);
});

test("changed text inside a streamed block remains a new final response", () => {
  const rows = groupWorkerEvents([
    event(10, { activity_ref: "message", text: "[worker message] Keep  two spaces." }),
    event(20, { activity_ref: "final-response",
      text: "[final response] Keep one space.\n\nNew content." }),
  ]);
  assert.equal(rows.at(-1)?.event.text,
    "[final response] Keep one space.\n\nNew content.");
});
