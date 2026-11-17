import { describe, expect, it } from "vitest";
import { emptyWorkspace, type Task, type Worker, type WorkspaceState } from "@cortex/workspace-domain";
import { buildIndex } from "../src/selectors";
import { sessionModel, workingNow } from "../src/session-model";
import { withTask } from "./support";

const worker = (patch: Partial<Worker> = {}): Worker => ({ id: "w1", taskId: "t", executionId: "e", role: "technical-writer", status: "working", lane: "execution", goal: "", now: "Writing", waitsFor: null, scope: [], route: null, attempt: null, elapsedMs: null, runningSince: 1, context: null, usage: null, activity: [], result: null, ...patch });
const state = (task: Partial<Task>, workers: Worker[]): WorkspaceState => {
  const s = withTask(emptyWorkspace(), "Work");
  s.tasks[0] = { ...s.tasks[0], ...task };
  s.machines[0] = { ...s.machines[0], connection: "connected" };
  s.workers = workers;
  return s;
};

/** Live run 2026-11-17: the worker facts of a large task could no longer be read, so the last successful read (a worker running)
 *  stayed on screen after the task itself had finished. */
describe("workers kept from a read that could not be refreshed", () => {
  it("a running worker is shown running while its task runs, then not confirmed once the task has ended", () => {
    const before = sessionModel(buildIndex(state({ status: "working", executionId: "e" }, [worker()]), Date.now()), "t")!;
    expect(before.running.map(w => w.id)).toEqual(["w1"]);
    expect(workingNow(before)).toBe(true);
    // The next worker read is refused; the task read still moves on to its finished state.
    const after = sessionModel(buildIndex(state({ status: "review", executionId: "e" }, [worker()]), Date.now()), "t")!;
    expect(after.running).toEqual([]);
    expect(after.active).toEqual([]);
    expect(workingNow(after)).toBe(false);
    expect(after.workers[0]).toMatchObject({ status: "unknown", stale: true, now: null, runningSince: null });
    expect(buildIndex(state({ status: "review", executionId: "e" }, [worker()]), Date.now()).views[0].running).toBe(0);
  });
  it("a finished worker keeps its recorded state under an ended task", () => {
    const m = sessionModel(buildIndex(state({ status: "review", executionId: "e" }, [worker({ status: "completed" })]), Date.now()), "t")!;
    expect(m.workers[0].status).toBe("completed");
    expect(m.workers[0].stale).toBeUndefined();
  });
});
