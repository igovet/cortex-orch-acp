import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, parseLiveSprintGraph, type LiveSprintGraph, type PlanItem, type Project, type Sprint, type SprintGraphEdit, type SprintGraphWrite, type SprintOrder, type Task, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { liveModel } from "../src/live-graph";
import { buildIndex, columnsOf, sprintView } from "../src/selectors";
import { planRows } from "../src/sprint-model";
import { draftTask, flush, projection, scriptedPort } from "./support";

/*
 * A created sprint's live graph: every bound task, later drafts included, with the links it waits on now, beside the approved
 * plan, which stays as approved. Links and priority are edited only through the service's owner actions, each with the
 * revision shown; Board, List and the queue bar show the service's execution order.
 */

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });

const settled = async () => { for (let i = 0; i < 12; i++) await flush(); };
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const button = (text: string, root: ParentNode = document) => [...root.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent?.trim() === text);
const labelled = (label: string) => $<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
const press = (el: Element) => { el.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, button: 0 })); el.dispatchEvent(new MouseEvent("pointerup", { bubbles: true, button: 0 })); };
const node = (id: string) => $<SVGGElement>(`.gsv .n[data-id="t:${id}"]`)!;
const escape = (target: Element = document.body) => target.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));

const REV = "a".repeat(64), REV2 = "b".repeat(64);
const project: Project = { id: "p", machineId: LOCAL_MACHINE_ID, name: "Cedar", repository: "/cedar", openBranch: "main", defaultBranch: "main" };
/** Plan v1: one task, then two that wait for it. */
const items: PlanItem[] = [
  { key: "k1", title: "Lay the schema", description: "Schema", dependsOn: [] },
  { key: "k2", title: "Write the importer", description: "Importer", dependsOn: ["k1"] },
  { key: "k3", title: "Write the exporter", description: "Exporter", dependsOn: ["k1"] },
];
/** The plan's three tasks, the first running, and two drafts the owner added later with New task. */
function tasks(): Task[] {
  const t = (id: string, title: string, patch: Partial<Task> = {}): Task => ({ ...draftTask(), id, title, description: title, sprintId: "sp", ...patch });
  return [
    t("t1", "Lay the schema", { status: "working", process: "live", executionId: "e1" }),
    t("t2", "Write the importer", { dependencies: ["t1"] }), t("t3", "Write the exporter", { dependencies: ["t1"] }),
    t("t4", "Add the audit log"), t("t5", "Document the format"),
  ];
}
const sprint = (patch: Partial<Sprint> = {}): Sprint => ({ id: "sp", projectId: "p", description: "Data", title: "Data", runMode: "hybrid", targetBranch: null, planning: "created",
  conversation: [], plan: { version: 1, items }, materializedVersion: 1, taskIds: ["t1", "t2", "t3", "t4", "t5"], queueState: "running", pauseReason: null, ...patch });
const order = (patch: Partial<SprintOrder> = {}): SprintOrder => ({ mode: "hybrid", queueState: "running", working: [{ taskId: "t1", startOrder: "1" }],
  notStarted: [{ taskId: "t4", rank: 1, reason: null, next: true, conditional: false }, { taskId: "t5", rank: 2, reason: null, next: false, conditional: true },
    { taskId: "t2", rank: null, reason: "dependencies_blocked", next: false, conditional: false }, { taskId: "t3", rank: null, reason: "dependencies_blocked", next: false, conditional: false }], ...patch });
function graph(patch: Partial<LiveSprintGraph> = {}, deps: Record<string, string[]> = {}): LiveSprintGraph {
  const base: Record<string, string[]> = { t1: [], t2: ["t1"], t3: ["t1"], t4: [], t5: [], ...deps };
  return { contractVersion: 1, sprintId: "sp", graphRevision: REV, complete: true, priority: ["t1", "t2", "t3", "t4", "t5"], order: order(),
    members: ["t1", "t2", "t3", "t4", "t5"].map((id, i) => ({ taskId: id, readStatus: "confirmed" as const, dependencies: base[id], editable: id !== "t1", canBePredecessor: true, approvedPlanKey: i < 3 ? `k${i + 1}` : null })), ...patch };
}
function world(s: Sprint, list = tasks()): WorkspaceState {
  const state = emptyWorkspace();
  state.machines[0].connection = "connected";
  state.projects.push(project);
  state.tasks.push(...list);
  state.sprints.push(s);
  return state;
}

/** A Workspace on a port whose service has the live graph. Each read answers `answer()`; each edit is recorded. */
async function open(path: string, s = sprint(), answer: () => LiveSprintGraph | "unsupported" = () => graph(), write: () => SprintGraphWrite = () => ({ outcome: "changed" })) {
  const port = scriptedPort({ capabilities: { everything: true, operations: [] } });
  const reads: string[] = [], edits: SprintGraphEdit[] = [];
  port.port.sprintGraph = {
    read: async sprintId => { reads.push(sprintId); const g = answer(); return g === "unsupported" ? { kind: "unsupported" } : { kind: "graph", graph: g }; },
    edit: async edit => { edits.push(edit); return write(); },
  };
  history.replaceState(null, "", path);
  const root = document.createElement("div");
  document.body.append(root);
  dispose = render(() => <Workspace port={port.port} />, root);
  await settled();
  port.emit(projection("1", world(s)));
  await settled();
  return { port, reads, edits };
}
const PLAN = `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/plan`;

describe("the live graph contract", () => {
  it("reads only contract version 1, and never takes an unread member's links for none", () => {
    expect(parseLiveSprintGraph(graph())).toEqual(graph());
    expect(parseLiveSprintGraph({ ...graph(), contractVersion: 2 })).toBeNull();
    expect(parseLiveSprintGraph({ ...graph(), graphRevision: "1700000000" })).toBeNull();
    // At most one current candidate, and never an unranked one.
    expect(parseLiveSprintGraph({ ...graph(), order: order({ notStarted: [{ taskId: "t4", rank: 1, reason: null, next: true, conditional: false }, { taskId: "t5", rank: 2, reason: null, next: true, conditional: false }] }) })).toBeNull();
    expect(parseLiveSprintGraph({ ...graph(), order: order({ notStarted: [{ taskId: "t4", rank: null, reason: "queue_paused", next: true, conditional: false }] }) })).toBeNull();
    const unread = graph({ complete: false });
    unread.members[4] = { ...unread.members[4], readStatus: "missing", dependencies: null, editable: false, canBePredecessor: false };
    expect(parseLiveSprintGraph(unread)?.members[4].dependencies).toBeNull();
    const model = liveModel(buildIndex(world(sprint()), 0), sprint(), { status: "ready", graph: unread, message: null, key: null });
    expect(model.unread).toBe(1);
    expect(model.items[4]).toMatchObject({ key: "t5", title: "Task not loaded", dependsOn: [] });
    expect(model.why).toBe("1 task could not be read, so links cannot be changed until the sprint reads completely.");
  });

  it("keeps the approved plan's task states when drafts are added later, pairing by task id", () => {
    const index = buildIndex(world(sprint()), 0), s = sprint();
    // Without the service's proof, the roster's first tasks are paired in run order, once their links agree with the plan.
    expect(planRows(index, s, items).map(r => [r.view?.id ?? null, r.backed])).toEqual([["t1", true], ["t2", true], ["t3", true]]);
    expect(planRows(index, s, items)[0].word?.text).toBe("Working");
    // The service's proof wins over roster positions.
    expect(planRows(index, s, items, undefined, new Map([["k1", "t1"], ["k2", "t3"], ["k3", "t2"]])).map(r => r.view?.id)).toEqual(["t1", "t3", "t2"]);
    // A relinked roster proves nothing, so no row borrows another task's state.
    const relinked = tasks().map(t => t.id === "t2" ? { ...t, dependencies: ["t4"] } : t);
    expect(planRows(buildIndex(world(s, relinked), 0), s, items).every(r => !r.backed && r.view === null)).toBe(true);
  });

  it("orders each sprint's tasks by the service's projection, in the places they hold, and ignores a stale order", () => {
    const loose: Task = { ...draftTask(), id: "solo", title: "Standalone" };
    const state = world(sprint(), [loose, ...tasks().reverse()]);
    const orders = new Map([["sp", order({ working: [{ taskId: "t1", startOrder: "1" }] })]]);
    const index = buildIndex(state, 0, orders);
    const ids = (list: { id: string }[]) => list.map(v => v.id);
    const columns = columnsOf(index.views, index);
    expect(ids(columns.todo)).toEqual(["solo", "t4", "t5", "t2", "t3"]);
    expect(ids(columns.work)).toEqual(["t1"]);
    // Next only for the service's candidate; a place for the ranked; the reason for the rest.
    const fact = (id: string) => index.byId.get(id)!.fact.text;
    expect([fact("t4"), fact("t5"), fact("t2")]).toEqual(["Next in Data", "Queued 2nd in Data", "Waits for Lay the schema"]);
    expect(sprintView(index, sprint())).toMatchObject({ next: { id: "t4" }, now: { id: "t1" }, ordered: true });
    // An order read under another run mode describes another queue: nothing is ranked by it.
    const stale = buildIndex(world(sprint({ runMode: "manual" }), [loose, ...tasks().reverse()]), 0, orders);
    expect(ids(columnsOf(stale.views, stale).todo)).toEqual(["solo", "t5", "t4", "t3", "t2"]);
    // Manual's order is a suggestion: no Next, a suggested first task.
    const manual = buildIndex(world(sprint({ runMode: "manual", queueState: "idle" })), 0, new Map([["sp", order({ mode: "manual", queueState: "idle",
      notStarted: [{ taskId: "t5", rank: 1, reason: null, next: false, conditional: false }, { taskId: "t4", rank: 2, reason: null, next: false, conditional: false }] })]]));
    expect(sprintView(manual, sprint({ runMode: "manual", queueState: "idle" }))).toMatchObject({ next: null, suggested: { id: "t5" } });
    expect(manual.byId.get("t4")!.fact.text).toBe("Suggested 2nd in Data");
  });
});

describe("the sprint's Live sprint view", () => {
  it("opens on every bound task with live links, says how it differs from the approved plan, and keeps the plan as approved", async () => {
    const { reads } = await open(PLAN);
    expect(reads).toEqual(["sp"]);
    expect($(".sp-plan")?.getAttribute("aria-label")).toBe("Live sprint");
    expect(all(".sp-plan .pt-row:not(.th)").map(r => r.querySelector(".tt")?.textContent)).toEqual(["Lay the schema", "Write the importer", "Write the exporter", "Add the audit log", "Document the format"]);
    expect(document.body.textContent).toContain("2 tasks not matched to Plan v1. The approved plan stays as approved.");
    button("Approved plan v1")!.click(); await settled();
    expect($(".sp-plan")?.getAttribute("aria-label")).toBe("Plan v1");
    expect($(".sp-plan-h")?.textContent).toContain("Read only");
    const rows = all(".sp-plan .pt-row:not(.th)");
    expect(rows).toHaveLength(3);
    // The plan's rows keep their tasks' states although the sprint now has more tasks than the plan.
    expect(rows[0].querySelector(".st")?.textContent).toContain("Working");
  });

  it("adds a prerequisite from Details with the revision shown, says what it changes, and reads the graph again", async () => {
    const { reads, edits } = await open(PLAN);
    labelled("Links and details of task 4").click(); await settled();
    const panel = $(".ws-read")!;
    const pick = panel.querySelector<HTMLSelectElement>('select[aria-label="Task 4 waits for"]')!;
    pick.value = "t5"; pick.dispatchEvent(new Event("change", { bubbles: true })); await settled();
    expect(panel.textContent).toContain("Add a link: Task 4 “Add the audit log” will wait for task 5 “Document the format” to finish first.");
    expect(panel.textContent).toContain("The queue is running, so this can change which task starts next. Pause the queue first");
    button("Add link", panel)!.click(); await settled();
    expect(edits).toEqual([{ action: "set_dependency", sprintId: "sp", dependentTaskId: "t4", predecessorTaskId: "t5", present: true, expectedGraphRevision: REV }]);
    expect(reads.length).toBe(2);
    // A link that would close a loop is named before anything is sent.
    labelled("Links and details of task 1").click(); await settled();
    expect($(".ws-read")?.textContent).toContain("Only a saved draft that has not started can change its links.");
  });

  it("moves a ready draft's priority without renumbering it, and says a stale refusal in words", async () => {
    let answer = graph();
    const { edits } = await open(PLAN, sprint(), () => answer, () => ({ outcome: "refused", code: "stale_graph_revision", message: "The sprint changed." }));
    expect(labelled("Move task 1 up")).toBeNull();
    // Meanwhile the sprint changed: the refused move is followed by a read of the new revision, never by a resend.
    answer = graph({ graphRevision: REV2, priority: ["t1", "t2", "t3", "t5", "t4"] });
    labelled("Move task 5 up").click(); await settled();
    expect(edits).toEqual([{ action: "move_priority", sprintId: "sp", taskId: "t5", direction: "up", expectedGraphRevision: REV }]);
    // The list is in the saved priority; each task keeps its roster number.
    expect(all(".sp-plan .pt-row:not(.th) .no").map(n => n.textContent)).toEqual(["1", "2", "3", "5", "4"]);
    labelled("Links and details of task 5").click(); await settled();
    button("Move up", $(".ws-read")!)!.click(); await settled();
    expect(edits).toHaveLength(2);
    expect(edits.at(-1)).toMatchObject({ taskId: "t5", expectedGraphRevision: REV2 });
    expect($(".ws-read")?.textContent).toContain("The sprint changed since it was shown. It was read again; check the links and try again.");
  });

  it("edits links on the graph: prerequisite, then dependent, a preview in words, and Escape steps back first", async () => {
    localStorage.setItem("cw.plan.view", JSON.stringify("graph"));
    const { edits } = await open(PLAN);
    const toggle = button("Edit links")!;
    toggle.click(); await settled();
    expect(toggle.getAttribute("aria-pressed")).toBe("true");
    expect($(".gv-band")?.textContent).toContain("Choose the prerequisite");
    press(node("t5")); await settled();
    expect($(".gv-band")?.textContent).toContain("Prerequisite: task 5. Now choose the task that will wait for it.");
    press(node("t4")); await settled();
    expect($(".gv-band")?.textContent).toContain("Add a link: Task 4 “Add the audit log” will wait for task 5 “Document the format” to finish first.");
    button("Add link", $(".gv-band")!)!.click(); await settled();
    expect(edits.at(-1)).toEqual({ action: "set_dependency", sprintId: "sp", dependentTaskId: "t4", predecessorTaskId: "t5", present: true, expectedGraphRevision: REV });
    // Both ends of a direct link offer its removal.
    press(node("t1")); press(node("t2")); await settled();
    expect($(".gv-band")?.textContent).toContain("Remove the link: Task 2 “Write the importer” will no longer wait for task 1 “Lay the schema”.");
    expect(button("Remove link", $(".gv-band")!)).toBeDefined();
    escape(); await settled();
    expect($(".gv-band")?.textContent).toContain("Prerequisite: task 1.");
    escape(); await settled();
    expect($(".gv-band")?.textContent).toContain("Choose the prerequisite");
    escape(); await settled();
    expect(toggle.getAttribute("aria-pressed")).toBe("false");
    expect(edits).toHaveLength(1);
  });

  it("turns editing off while a member could not be read, and stays read-only on a service without the live graph", async () => {
    const unread = graph({ complete: false });
    unread.members[4] = { ...unread.members[4], readStatus: "unreadable", dependencies: null, editable: false, canBePredecessor: false };
    await open(PLAN, sprint(), () => unread);
    expect(document.body.textContent).toContain("Live graph read incomplete: 1 task could not be read.");
    expect(all(".sp-plan .pt-row:not(.th)").map(r => r.querySelector(".tt")?.textContent)).toContain("Task not loaded");
    expect(labelled("Move task 4 up")).toBeNull();
    dispose?.(); document.body.innerHTML = "";
    localStorage.setItem("cw.plan.view", JSON.stringify("graph"));
    await open(PLAN, sprint(), () => "unsupported");
    expect(all(".gsv .n")).toHaveLength(5);
    expect(button("Edit links")).toBeUndefined();
  });
});

describe("a sprint's Board and List", () => {
  it("put Working before Not started, in start order and by the service's rank, with Next only where it says", async () => {
    await open(`/m/${LOCAL_MACHINE_ID}/p/p/s/sp/board`);
    const columns = all<HTMLElement>(".board-wrap .col").map(c => c.dataset.col);
    expect(columns.indexOf("work")).toBeLessThan(columns.indexOf("todo"));
    const todo = all<HTMLElement>('.col[data-col="todo"] .card').map(c => c.dataset.task);
    expect(todo).toEqual(["t4", "t5", "t2", "t3"]);
    expect(document.body.textContent).toContain("Next in Data");
    dispose?.(); document.body.innerHTML = "";
    await open(`/m/${LOCAL_MACHINE_ID}/p/p/s/sp/list`);
    const heads = all(".ws-list-h").map(h => h.textContent ?? "");
    expect(heads.findIndex(h => h.startsWith("Working"))).toBeLessThan(heads.findIndex(h => h.startsWith("Not started")));
    expect(all(".ws-list .lrow .w500").map(t => t.textContent)).toEqual(["Lay the schema", "Add the audit log", "Document the format", "Write the importer", "Write the exporter"]);
  });
});
