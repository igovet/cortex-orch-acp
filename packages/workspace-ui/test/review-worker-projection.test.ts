import { describe, expect, it } from "vitest";
import type { Worker } from "@cortex/workspace-domain";
import { reviewLine } from "../src/session-model";

const worker = (patch: Partial<Worker> = {}): Worker => ({
  id: "review", taskId: "t", executionId: "e", role: "code-reviewer", status: "completed", lane: "review", goal: "Review",
  now: null, waitsFor: null, scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null,
  activity: [], result: null, ...patch,
});

describe("review worker projection", () => {
  it("explains an incomplete review and its follow-up", () => {
    const previous = worker({ review: { verdict: "INCOMPLETE", notCovered: [{ item: "src/auth.ts", reason: "not read" }], rerun: { reason: "The previous review left part of the change uncovered.", previousWorkerId: "review", followUpWorkerId: "review_rerun1" } } });
    const followUp = worker({ id: "review_rerun1", status: "working", review: previous.review });
    expect(reviewLine(previous)).toBe("Review incomplete. Follow-up review started");
    expect(reviewLine(followUp)).toBe("Follow-up review: checking src/auth.ts");
  });
  it("keeps the old display contract when review facts are absent", () => {
    expect(reviewLine(worker())).toBeNull();
  });
});
