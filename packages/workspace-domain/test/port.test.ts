import { describe, expect, it } from "vitest";
import { applyFacts, applyProjection, compareVersions, emptyWorkspace, LOCAL_MACHINE_ID, overlayTask, projectionKey, type Project, type ProjectFacts, type ProjectionUpdate, type Sprint, type Task } from "../src";
import { canRun, capabilityOf, noCapabilities, WIRE_OPERATIONS } from "../src/capabilities";

const project: Project = { id: "p", machineId: LOCAL_MACHINE_ID, name: "P", repository: "/p", openBranch: "", defaultBranch: "" };

describe("versions", () => {
  it("orders decimal strings beyond the safe integer range", () => {
    expect(compareVersions("9007199254740993", "9007199254740992")).toBe(1);
    expect(compareVersions("2", "10")).toBe(-1);
    expect(compareVersions("7", "7")).toBe(0);
  });
  it("refuses anything that is not a decimal", () => {
    expect(() => compareVersions("1.5", "2")).toThrow();
    expect(() => compareVersions("", "2")).toThrow();
  });
});

describe("projections", () => {
  it("a reopened runtime task cannot retain acceptance or Ready to merge", () => {
    const task = {id:"t",status:"working",process:"live",observedPhase:"planning",checkout:{kind:"main",branch:"main"},acceptance:null,result:null} as Task;
    const next = overlayTask(task, {status:"ready_to_merge",automaticReady:true,acceptance:{version:1,digest:"old",at:1,open:null}}, "main");
    expect(next.status).toBe("working");
    expect(next.acceptance).toBeNull();
    expect(next.observedPhase).toBe("planning");
  });
  it("a project slice replaces only that project", () => {
    const start = emptyWorkspace();
    start.projects.push({ ...project, id: "other" });
    const task = { id: "t", projectId: "p" } as Task;
    const update: ProjectionUpdate = { kind: "projection", version: "1", scope: "project", slice: { project, tasks: [task], sessions: [] } };
    const next = applyProjection(start, update);
    expect(next.projects.map(p => p.id).sort()).toEqual(["other", "p"]);
    expect(next.tasks).toEqual([task]);
    expect(start.tasks).toEqual([]);
    expect(projectionKey(update)).toBe("project:p");
  });
  it("a later slice drops a task the project no longer reports", () => {
    const first: ProjectionUpdate = { kind: "projection", version: "1", scope: "project", slice: { project, tasks: [{ id: "t", projectId: "p" } as Task], sessions: [] } };
    const second: ProjectionUpdate = { kind: "projection", version: "2", scope: "project", slice: { project, tasks: [], sessions: [] } };
    expect(applyProjection(applyProjection(emptyWorkspace(), first), second).tasks).toEqual([]);
  });
  it("a whole-workspace update replaces everything", () => {
    const state = emptyWorkspace();
    expect(applyProjection(emptyWorkspace(), { kind: "projection", version: "1", scope: "workspace", state })).toBe(state);
    expect(projectionKey({ kind: "projection", version: "1", scope: "workspace", state })).toBe("workspace");
  });
  // The screens compare lists by position: a reordered list is a changed one, and redraws everything that shows it.
  it("reading a project's unchanged facts again leaves every list in its order", () => {
    const sprint = (id: string, projectId: string) => ({ id, projectId }) as Sprint;
    const facts = (projectId: string, sprints = [sprint(`s-${projectId}`, projectId)]) => ({ projectId, openBranch: "main", defaultBranch: "main", repo: null, workers: [], reviews: [], overlays: {}, sprints }) as unknown as ProjectFacts;
    let state = emptyWorkspace();
    for (const id of ["a", "b", "c"]) state = applyFacts(state, facts(id));
    expect(applyFacts(state, facts("a")).sprints.map(s => s.id)).toEqual(["s-a", "s-b", "s-c"]);
    // Records that changed still arrive at the end.
    expect(applyFacts(state, facts("a", [sprint("s-a", "a"), sprint("s-a2", "a")])).sprints.map(s => s.id)).toEqual(["s-b", "s-c", "s-a", "s-a2"]);
  });
});

describe("capabilities", () => {
  it("names a contract operation for the commands the contract has, and a local one for the rest", () => {
    expect(capabilityOf("create_task")).toBe("tasks.create");
    expect(capabilityOf("start_task")).toBe("tasks.start");
    expect(capabilityOf("answer_decision")).toBe("decisions.answer");
    expect(capabilityOf("run_sprint")).toBe("workspace.run_sprint");
    expect(WIRE_OPERATIONS).toEqual(["tasks.create", "tasks.start", "decisions.answer"]);
  });
  it("fails closed: nothing is allowed before the machine says so", () => {
    expect(canRun(noCapabilities(), "create_task")).toBe(false);
    expect(canRun({ everything: false, operations: ["tasks.create"] }, "create_task")).toBe(true);
    expect(canRun({ everything: false, operations: ["tasks.create"] }, "stop_task")).toBe(false);
  });
});

describe("empty workspace", () => {
  it("starts with one local machine and no project, task, engine or option", () => {
    const state = emptyWorkspace();
    expect(state.machines).toHaveLength(1);
    expect(state.machines[0]).toMatchObject({ id: LOCAL_MACHINE_ID, kind: "local", connection: "disconnected", version: null, connections: [] });
    expect([state.projects, state.tasks, state.sprints, state.engineUsage, state.options.models, state.options.efforts, state.options.modes, state.options.languages].every(list => list.length === 0)).toBe(true);
    expect(JSON.stringify(state)).not.toMatch(/mock/i);
  });
});
