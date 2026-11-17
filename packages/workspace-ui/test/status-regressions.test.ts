import { describe, expect, it } from "vitest";
import { emptyWorkspace, type Worker } from "@cortex/workspace-domain";
import { buildIndex, sideWord } from "../src/selectors";
import { sessionModel, workersOutcome } from "../src/session-model";
import { withTask } from "./support";

const worker = (id: string, status: Worker["status"]): Worker => ({ id, taskId: "t", executionId: "e", role: "technical-writer", status, lane: "execution", goal: "g", now: null, waitsFor: null, scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, activity: [], result: null, messages: [] } as unknown as Worker);

describe("sidebar task row", () => {
  it("a paused task reads Paused, a stopping one Stopping, a running one Working", () => {
    for (const [status, word] of [["paused", "Paused"], ["stopping", "Stopping"], ["working", "Working"]] as const) {
      const s = withTask(emptyWorkspace(), "Work"); s.tasks[0].status = status;
      expect(sideWord(buildIndex(s, Date.now()).views[0])).toBe(word);
    }
  });
});

describe("worker counts", () => {
  it("cancelled and failed workers are never counted as finished", () => {
    const s = withTask(emptyWorkspace(), "Work"); s.tasks[0].status = "paused";
    s.workers = [worker("a", "cancelled"), worker("b", "cancelled")];
    const m = sessionModel(buildIndex(s, Date.now()), "t")!;
    expect(workersOutcome(m.workers, m.active.length)).toBe("0 active, 0 finished, 2 cancelled");
    expect(workersOutcome([worker("a", "completed"), worker("b", "failed"), worker("c", "working")], 1)).toBe("1 active, 1 finished, 1 failed");
  });
});
