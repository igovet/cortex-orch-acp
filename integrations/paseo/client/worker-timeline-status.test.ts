import assert from "node:assert/strict";
import test from "node:test";
import { isWorkerTimelineRunning } from "./worker-timeline-status.ts";

test("worker timeline activity runs only for a live worker", () => {
  assert.equal(isWorkerTimelineRunning("running"), true);
  assert.equal(isWorkerTimelineRunning("working"), true);
  assert.equal(isWorkerTimelineRunning("completed"), false);
  assert.equal(isWorkerTimelineRunning("failed"), false);
  assert.equal(isWorkerTimelineRunning("canceled"), false);
  assert.equal(isWorkerTimelineRunning(undefined), false);
});

test("terminal replay state stops activity after a restart or stop", () => {
  for (const status of ["completed", "failed", "canceled", "cancelled", "stopped"]) {
    assert.equal(isWorkerTimelineRunning(status), false, status);
  }
});
