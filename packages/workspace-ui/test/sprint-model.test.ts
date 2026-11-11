import { describe, expect, it } from "vitest";
import type { PlanItem, Sprint, Task } from "@cortex/workspace-domain";
import { planCaption, planGraph, planRows, sprintSummary } from "../src/sprint-model";
import { statusFilterOf } from "../src/plan-graph";
import type { TaskView, WorkspaceIndex } from "../src/selectors";

const task = (id: string, patch: Partial<Task> = {}) => ({
  id, title: id, description: id, status: "draft", dependencies: [], conflict: "none", integration: null, work: "unmerged", closedAs: null,
  checkout: { kind: "main", branch: "" }, result: null, ...patch,
}) as unknown as Task;
const view = (t: Task, unknown = false) => ({ id: t.id, task: t, unknown, fact: { text: "Process state unknown", tone: "st-idle", icon: "help" }, target: "main" }) as unknown as TaskView;
const indexOf = (views: TaskView[]) => ({ byId: new Map(views.map(v => [v.id, v])) }) as unknown as WorkspaceIndex;
const items = (keys: string[], deps: Record<string, string[]> = {}): PlanItem[] => keys.map(k => ({ key: k, title: `Title ${k}`, description: k, dependsOn: deps[k] ?? [] }));

const v1 = items(["x", "y"]), v2 = items(["a", "b"], { b: ["a"] });
const sprint = (patch: Partial<Sprint> = {}): Sprint => ({
  id: "sp", projectId: "p", description: "S", runMode: "manual", targetBranch: null, planning: "created", conversation: [],
  plan: { version: 2, items: v2 }, history: [{ version: 1, items: v1 }, { version: 2, items: v2 }], materializedVersion: 2,
  taskIds: ["ta", "tb"], queueState: "idle", pauseReason: null, ...patch,
});
const index = indexOf([view(task("ta", { status: "working" })), view(task("tb", { dependencies: ["ta"] }))]);

describe("planRows", () => {
  it("gives the materialized version its tasks' states", () => {
    const s = sprint(), rows = planRows(index, s, s.plan!.items);
    expect(rows.map(r => [r.no, r.view?.id, r.word?.text, r.waits, r.backed])).toEqual([[1, "ta", "Working", [], true], [2, "tb", "Blocked", [1], true]]);
    expect(planRows(index, s, s.plan!.items, 2).map(r => r.word?.text)).toEqual(["Working", "Blocked"]);
  });

  it("never gives a past version the current tasks' states, even with as many items", () => {
    const s = sprint();
    for (const rows of [planRows(index, s, s.history![0].items), planRows(index, s, v1, 1)]) {
      expect(rows.map(r => [r.view, r.word, r.backed])).toEqual([[null, null, false], [null, null, false]]);
      expect(rows.map(statusFilterOf)).toEqual([null, null]);
    }
  });

  it("gives a draft copy no states", () => {
    const s = sprint();
    expect(planRows(index, s, structuredClone(v2)).every(r => !r.backed && r.word === null)).toBe(true);
    expect(planRows(index, s, v2, null).every(r => !r.backed)).toBe(true);
  });

  it("pairs tasks by run order, the order they were created in", () => {
    const shuffled = items(["late", "early"], { late: ["early"] });
    const s = sprint({ plan: { version: 2, items: shuffled }, history: [] });
    const rows = planRows(index, s, shuffled);
    expect(rows.map(r => r.view?.id)).toEqual(["tb", "ta"]);
  });

  it("shows a created task that is not loaded as Unknown", () => {
    const s = sprint({ taskIds: ["ta", "missing"] });
    const rows = planRows(index, s, s.plan!.items);
    expect(rows[1]).toMatchObject({ view: null, word: null, backed: true });
    expect(rows.map(statusFilterOf)).toEqual(["working", "unknown"]);
  });

  it("keeps the sprint summary's rows keyed by task", () => {
    const s = sprint({ taskIds: ["ta", "tb", "missing"] });
    const summary = sprintSummary(index, s);
    expect(summary.planRows.map(r => [r.item.key, r.word?.text, r.deps])).toEqual([["ta", "Working", []], ["tb", "Blocked", [1]]]);
    expect(summary.working).toBe(1);
  });
});

describe("the small inline graph", () => {
  const plan = items(["a", "b", "c"], { b: ["a"], c: ["a"] });

  it("keeps today's geometry and wording", () => {
    const g = planGraph(plan, null);
    expect([g.width, g.height]).toEqual([320, 104]);
    expect(g.nodes.map(n => [n.key, n.x, n.y, n.label, n.on])).toEqual([["a", 12, 35, "1 Title a", false], ["b", 188, 12, "2 Title b", false], ["c", 188, 58, "3 Title c", false]]);
    expect(g.edges.map(e => e.on)).toEqual([false, false]);
    expect(g.label).toBe("Task 1 leads to 2, 3.");
    expect(planCaption(plan, null)).toBe("Automatic run order, one at a time: 1, 2, 3.");
  });

  it("lights only the explicit pick's links", () => {
    const g = planGraph(plan, "b");
    expect(g.nodes.map(n => n.on)).toEqual([false, true, false]);
    expect(g.edges.map(e => [e.key, e.on])).toEqual([["a>b", true], ["a>c", false]]);
    expect(planCaption(plan, "a")).toBe("Task 1 starts first and unblocks 2, 3. Automatic run order, one at a time: 1, 2, 3.");
    expect(planGraph(plan, "gone").nodes.some(n => n.on)).toBe(false);
  });
});
