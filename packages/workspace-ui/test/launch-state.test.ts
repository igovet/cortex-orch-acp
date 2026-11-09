import { describe, expect, it } from "vitest";
import { emptyWorkspace, mapTask, type WorkspaceState } from "@cortex/workspace-domain";
import { buildIndex } from "../src/selectors";
import { wireTask, withTask } from "./support";

/** A started task whose engine has not opened yet, as the service projects it. */
function launching(patch: Parameters<typeof wireTask>[0]): WorkspaceState {
  const base = withTask(emptyWorkspace(), "Launch"), mapped = mapTask(wireTask({ executionId: "e", ...patch }), { projectId: "p" });
  return { ...base, machines: base.machines.map(m => ({ ...m, connection: "connected" as const })), tasks: [{ ...base.tasks[0], ...mapped, id: base.tasks[0].id, title: "Launch" }] };
}
const view = (state: WorkspaceState) => buildIndex(state, Date.now()).views[0];

describe("a task whose engine is still starting", () => {
  it("is a live, stoppable task with no retry to explain while it simply starts", () => {
    const v = view(launching({ phase: "planning", executionState: "starting" }));
    expect(v.unknown).toBe(false);
    expect(v.task.process).toBe("live");
    expect(v.fact.text).toBe("Planning");
  });

  it("shows the runtime's own retry notice, and its give-up as a failure that needs the owner", () => {
    const retry = "⟳ The engine did not start (EngineTransportClosed). Retry 3 in 20 s…";
    const retrying = view(launching({ phase: "planning", executionState: "starting", waitReason: retry, warnings: ["engine_launch_retrying"] }));
    expect(retrying.unknown).toBe(false);
    expect(retrying.fact).toMatchObject({ text: retry, tone: "st-warn" });
    const gaveUp = "✗ The engine kept failing for 8 min (EngineTransportClosed; 11 retries). Stopping.";
    const failed = view(launching({ phase: "blocked", executionState: "blocked", waitReason: gaveUp, warnings: ["engine_launch_failed"] }));
    expect(failed.task.status).toBe("needs_you");
    expect(failed.fact).toMatchObject({ text: gaveUp, tone: "st-bad" });
  });
});
