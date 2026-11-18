import { describe, expect, it } from "vitest";
import { applyFacts, emptyWorkspace, LOCAL_MACHINE_ID, type ProjectFacts, type Task, type Worker, type WorkerRoute, type WorkspaceState } from "../src";

const worker = (id: string, route: WorkerRoute | null): Worker => ({ id, taskId: "t", executionId: "e", role: id, status: "working", lane: "execution", goal: "",
  now: null, waitsFor: null, scope: [], route, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, activity: [], result: null });

describe("worker routes from the machine's facts", () => {
  it("keeps each worker's own recorded engine and model, apart from the coordinator's route and the task's settings", () => {
    const state: WorkspaceState = { ...emptyWorkspace(), projects: [{ id: "p", machineId: LOCAL_MACHINE_ID, name: "P", repository: "/p", openBranch: "", defaultBranch: "" }] };
    // The task runs its coordinator on Claude Fable; its workers were routed elsewhere.
    state.tasks = [{ id: "t", projectId: "p", status: "working", checkout: { kind: "main", branch: "main" }, settings: { model: "fable-5.2", effort: "high", mode: "auto", language: "auto" },
      coordinatorRoute: { model: "fable-5.2", effort: "high" } } as unknown as Task];
    const codex = { engine: "codex-acp", model: "gpt-6.1-sol", effort: "medium" }, claude = { engine: "claude-agent-acp", model: "sonnet", effort: "high" };
    const facts: ProjectFacts = { projectId: "p", openBranch: "main", defaultBranch: "main", repo: null, reviews: [], sprints: [], overlays: {},
      workers: [worker("backend-dev", codex), worker("code-reviewer", claude), worker("docs-writer", null)] };
    const next = applyFacts(state, facts);
    expect(next.workers.map(w => [w.id, w.route])).toEqual([["backend-dev", codex], ["code-reviewer", claude], ["docs-writer", null]]);
    // An unconfirmed route stays unknown: nothing fills it from the coordinator or the task.
    expect(next.workers.some(w => w.route?.model === "fable-5.2")).toBe(false);
    expect(next.tasks[0].coordinatorRoute).toEqual({ model: "fable-5.2", effort: "high" });
  });
});
