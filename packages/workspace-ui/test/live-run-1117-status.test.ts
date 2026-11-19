import { describe, expect, it } from "vitest";
import { emptyWorkspace, mapTask, type Task, type WorkspaceState } from "@cortex/workspace-domain";
import { buildIndex, decisionOf, humanStatus, inboxItems, interruptedTurn, nothingToCommit, readyLabel, readyStep, refusalReason, sideWord, stopReason } from "../src/selectors";
import { GENERIC_STATUS } from "../src/status-text";
import { acceptBlocked } from "../src/review-model";
import { stateChip, sessionModel } from "../src/session-model";
import { withTask, wireTask } from "./support";

/** Live run 2026-11-17: task states that read as waiting, running or mergeable when they were not. */
const state = (patch: Partial<Task>, project: Partial<WorkspaceState["projects"][number]> = {}): WorkspaceState => {
  const s = withTask(emptyWorkspace(), "Work");
  s.tasks[0] = { ...s.tasks[0], ...patch };
  s.projects[0] = { ...s.projects[0], ...project };
  s.machines[0] = { ...s.machines[0], connection: "connected" };
  return s;
};
const view = (s: WorkspaceState) => buildIndex(s, Date.now()).views[0];

describe("held and stopped tasks", () => {
  it("renders machine status reasons as human language without touching prose", () => {
    expect(humanStatus("review_incomplete")).toBe("The review is incomplete");
    expect(humanStatus("native_outside_outcome_unknown")).not.toContain("_");
    expect(humanStatus("Automatic continuation: check_this_file")).toBe("Automatic continuation: check this file");
    expect(humanStatus("review_needs_owner")).not.toMatch(/_|review_needs_owner/);
  });
  it("a recovery code reads in words, never as the raw code", () => {
    expect(stopReason("native_session_outcome_unknown")).toBe("The app stopped while this task ran, so how its engine session ended is not known");
    expect(stopReason("completion_worker_check_unverified")).toBe("The work could not be confirmed as complete");
    expect(stopReason("some_new_code")).toBe(GENERIC_STATUS);
    expect(stopReason("Задача остановлена: что дальше?")).toBe("Задача остановлена: что дальше?");
    const d = decisionOf(view(state({ status: "needs_you", issue: "native_session_outcome_unknown", process: "live", observedPhase: "recovery", executionId: "e" })));
    expect(d.ask).not.toContain("native_session_outcome_unknown");
  });

  it("a stop the runtime decided is not called an interrupted turn", () => {
    expect(interruptedTurn(null)).toBe(true);
    expect(interruptedTurn("turn_interrupted")).toBe(true);
    expect(interruptedTurn("native_control_outcome_unknown")).toBe(true);
    expect(interruptedTurn("completion_worker_check_unverified")).toBe(false);
    const s = state({ status: "needs_you", issue: "completion_worker_check_unverified", process: "live", observedPhase: "recovery", executionId: "e" });
    expect(view(s).fact.text).toBe("Stopped; waiting for your message");
    expect(stateChip(sessionModel(buildIndex(s, Date.now()), "t")!).text).toBe("Stopped");
  });

  it("an unfinished task's Accept waits for the task, not for missing evidence", () => {
    const result = { kind: "work" as const, version: 1, digest: "d", checks: [], summary: "s", acceptanceAvailable: false, sourceVersion: "1" };
    expect(acceptBlocked({ ...state({ status: "needs_you", result, workspaceDigest: "d" }).tasks[0] })).toBe("Available once this task finishes");
    expect(acceptBlocked({ ...state({ status: "review", result, workspaceDigest: "d" }).tasks[0] })).toBe("Result evidence is unavailable. Request a new result");
  });

  it("Finish chosen on the runtime's stop question closes the task as is, so no list asks the owner for it", () => {
    const mapped = mapTask(wireTask({ executionId: "e", phase: "stopped", runtimeState: "CANCELLED", executionState: "stopped", warnings: ["owner_resolved_without_certification"] }), { projectId: "p" });
    expect([mapped.status, mapped.closedAs, mapped.notVerifiedByCortex]).toEqual(["done", "as_is", true]);
    const s = state({ ...mapped, id: "t", projectId: "p" });
    expect(view(s).column).toBe("done");
    expect(inboxItems(buildIndex(s, Date.now()))).toEqual([]);
  });

  it("a cancelled or failed run reads as its outcome in Needs you, never as Waiting for you", () => {
    for (const status of ["cancelled", "failed"] as const) {
      const d = decisionOf(view(state({ status })));
      expect(d.ask).not.toBe("Waiting for you");
      expect(d.kind).toBe(status === "failed" ? "Failed" : "Cancelled");
    }
  });
});

describe("paused tasks", () => {
  it("never count as Working: they wait for the owner's Resume and say Paused", () => {
    const v = view(state({ status: "paused", process: "stopped", executionId: "e" }));
    expect(v.column).toBe("needs");
    expect(sideWord(v)).toBe("Paused");
    expect(decisionOf(v).kind).toBe("Paused");
    const summary = buildIndex(state({ status: "paused", process: "stopped", executionId: "e" }), Date.now()).summaries.get("p")!;
    expect(summary.counts.work).toBe(0);
  });
});

describe("finishing words", () => {
  it("a project-folder task whose record names no target is ready to finish, not to merge", () => {
    const task = state({ status: "review", checkout: { kind: "main", branch: "main" } }).tasks[0];
    expect(readyStep(task, "", true)).toBe("Ready to finish");
    expect(readyLabel({ ...task, status: "ready_to_merge" }, "")).toBe("Ready to finish");
    expect(readyStep({ ...task, checkout: { kind: "worktree", branch: "feature" } as Task["checkout"] }, "", true)).toBe("Ready to merge");
  });

  it("an answer that changed no files has nothing to commit and is closed", () => {
    const task = state({ status: "ready_to_merge", integrationFacts: { targetBranch: "main", alreadyInTarget: false, sameBranch: true, commitAvailable: false, noChanges: true, pushStatus: null } }).tasks[0];
    expect(nothingToCommit(task)).toBe(true);
    expect(readyLabel(task, "main")).toBe("Nothing to commit");
    expect(readyLabel({ ...task, integrationFacts: { ...task.integrationFacts!, noChanges: undefined } }, "main")).toBe("Review task");
  });
});

describe("refused worker results", () => {
  it("name why the runtime refused them", () => {
    expect(refusalReason("open_failed_check")).toBe("a check it cited failed and was still open");
    expect(refusalReason("brand_new")).toBe("the runtime refused it");
  });
});
