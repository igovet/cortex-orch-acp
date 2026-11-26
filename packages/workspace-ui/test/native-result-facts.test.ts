import { describe, expect, it } from "vitest";
import { acceptBlocked, stale } from "../src/review-model";
import { editedFiles, reviewLine, verificationFace } from "../src/session-model";
import type { Task, Worker } from "@cortex/workspace-domain";

const digest = "f".repeat(64);
const verified = (over: Record<string, unknown> = {}) => ({
  id: "t", status: "review", conflict: "none", workspaceDigest: digest,
  reviewActions: { accept: true, reason: null, unblockAction: null },
  result: { kind: "work", acceptanceAvailable: true, version: 201, digest, summary: "Done.", checks: [{ name: "awk", outcome: "passed", detail: "", source: "runtime", kind: "automated" }], warnings: ["The reviewer runs on the same model as the work it reviews."] },
  ...over,
}) as unknown as Task;

describe("native result and worker facts", () => {
  it("accepts a verified, reviewed result whose worktree is unchanged, even with a same-model review note", () => {
    expect(stale(verified())).toBe(false);
    expect(acceptBlocked(verified())).toBeNull();
  });
  it("still blocks Accept with the runtime's reason when evidence is unavailable or the files changed", () => {
    const reason = "Result evidence is unavailable or the files changed. Request changes and run fresh checks.";
    expect(acceptBlocked(verified({ reviewActions: { accept: false, reason, unblockAction: "request_changes" } }))).toBe(reason);
    expect(acceptBlocked(verified({ workspaceDigest: "0".repeat(64) }))).toMatch(/no longer matches the worktree/);
  });
  it("reports runtime verification from the task's runtime-recorded checks", () => {
    const model = (source?: "runtime") => ({ view: { task: { observedPhase: null, result: { checks: [{ name: "awk", outcome: "passed", detail: "", ...(source ? { source } : {}) }] } } } }) as never;
    expect(verificationFace(model("runtime")).label).toBe("1 passed");
    expect(verificationFace(model()).label).toBe("Not run");
  });
  it("says when an approved stage missed the goal and that no verification follows", () => {
    const reviewer = (review: Record<string, unknown>) => ({ id: "rv", review, superseded: false }) as unknown as Worker;
    expect(reviewLine(reviewer({ verdict: "APPROVE" }))).toBe("Review approved");
    expect(reviewLine(reviewer({ verdict: "APPROVE", goalNotMet: true }))).toBe("Stage approved, goal not met");
    const model = (workers: Worker[]) => ({ workers, view: { task: { observedPhase: null, result: null } } }) as never;
    expect(verificationFace(model([])).label).toBe("Not run");
    expect(verificationFace(model([reviewer({ verdict: "APPROVE" })])).label).toBe("Not run");
    for (const review of [{ verdict: "APPROVE", goalNotMet: true }, { verdict: "REQUEST_CHANGES" }, { verdict: "INCOMPLETE" }])
      expect(verificationFace(model([reviewer({ verdict: "APPROVE" }), reviewer(review)])).label).toBe("Will not run: changes needed");
  });
  it("counts the edits a worker recorded", () => {
    const edit = (code: string, add: number, del: number, sequence: number) => ({ sequence, kind: "edit", text: "Edited", code, at: null, diff: { add, del, lines: [] } });
    const worker = { activity: [edit("a.md", 3, 1, 1), edit("a.md", 2, 0, 2), edit("b.md", 1, 1, 3), { sequence: 4, kind: "command", text: "ls", at: null }] } as unknown as Worker;
    expect(editedFiles(worker)).toEqual({ files: 2, add: 6, del: 2 });
  });
});
