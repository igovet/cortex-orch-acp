// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, describe, expect, it } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, mapDecision, type LiveSprintGraph, type Sprint, type Task } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { graphKey } from "../src/live-graph";
import { buildIndex } from "../src/selectors";
import { sprintBlockers } from "../src/sprint-blockers-model";
import { draftTask, flush, projection, scriptedPort, wireDecision } from "./support";

const task = (id: string, title: string, dependencies: string[] = [], patch: Partial<Task> = {}): Task =>
  ({ ...draftTask(), id, title, description: title, sprintId: "sp", dependencies, ...patch });
const tasks = () => [
  task("done", "Measure delays", [], { status: "done", closedAs: "as_is" }),
  task("paused", "Wake the window", ["done"], { status: "paused", executionId: "run" }),
  task("next", "Batch publication", ["paused"]),
  task("later", "Reduce redraws", ["paused", "next"]),
  task("free", "Rewrite the queries", ["done"]),
  task("both", "Move project databases", ["free", "paused"]),
];
const sprint = (patch: Partial<Sprint> = {}): Sprint => ({
  id: "sp", projectId: "p", title: "Update latency", description: "Update latency", runMode: "automatic", planning: "created",
  targetBranch: null, conversation: [], plan: null, materializedVersion: null, taskIds: tasks().map(t => t.id), queueState: "paused", pauseReason: null, ...patch,
});
const state = (s = sprint(), members = tasks()) => {
  const w = emptyWorkspace();
  w.machines[0].connection = "connected";
  w.projects.push({ id: "p", machineId: LOCAL_MACHINE_ID, repository: "/p", name: "Project", openBranch: "main", defaultBranch: "main" });
  w.tasks = members;
  w.sprints = [s];
  return w;
};
const graph = (s = sprint(), members = tasks()): LiveSprintGraph => ({
  sprintId: s.id, contractVersion: 1, graphRevision: "a".repeat(64), complete: true, priority: s.taskIds,
  order: { mode: s.runMode, queueState: s.queueState, notStarted: [], working: [] },
  members: members.map(t => ({ taskId: t.id, readStatus: "confirmed", dependencies: t.dependencies, editable: t.status === "draft", canBePredecessor: true, approvedPlanKey: null })),
});
const settled = async () => { for (let i = 0; i < 10; i++) await flush(); };

describe("current sprint blockers", () => {
  it("uses the current run mode, separates a pause, and counts a multi-prerequisite task once", () => {
    const s = sprint();
    const m = sprintBlockers(buildIndex(state(s), 0), s);
    expect(m.kind).toBe("ready");
    if (m.kind !== "ready") return;
    expect([m.blocked, m.clear]).toEqual([3, 1]);
    expect(m.blockers).toMatchObject([
      { view: { id: "paused" }, immediate: ["next"], later: ["later", "both"], chain: ["paused", "next", "later"] },
      { view: { id: "free" }, immediate: [], later: ["both"], chain: ["free", "both"] },
    ]);
    expect(m.waiting.get("both")).toEqual(["free", "paused"]);
  });

  it("keeps an output-blocked draft out of the clear count after the producer is done", () => {
    const reason = "This task requires report v1 from Measure delays. The matching reviewed, verified output is unavailable.";
    const s = sprint(), order = graph(s).order;
    order.notStarted.push({ taskId: "free", rank: null, next: false, conditional: false, reason: "deliverable_missing" });
    const orders = new Map([[s.id, order]]);
    const index = buildIndex(state(s), 0, orders);
    expect(index.byId.get("free")?.fact.text).toBe("Waiting for the required output");
    const analysis = sprintBlockers(index, s);
    expect(analysis).toMatchObject({ blocked: 4, clear: 0, outputs: new Map([["free", "Waiting for the required output"]]) });
    s.automaticQueue = { taskId: "free", kind: "settlement_blocked", code: "deliverable_missing", reason, needsOwner: false };
    const held = buildIndex(state(s), 0, orders);
    expect(held.byId.get("free")?.fact.text).toBe(reason);
    expect(sprintBlockers(held, s)).toMatchObject({ blocked: 4, clear: 0, outputs: new Map([["free", reason]]) });
    delete s.automaticQueue;
    s.queueState = "running";
    expect(sprintBlockers(buildIndex(state(s), 0, orders), s)).toMatchObject({ clear: 1, outputs: new Map() });
  });

  it("does not infer counts from missing, stale, incomplete, or cyclic data", () => {
    const s = sprint(), index = buildIndex(state(s), 0);
    const cached = { status: "ready" as const, key: graphKey(index, s), graph: graph(s), message: null };
    expect(sprintBlockers(index, s, { ...cached, key: "old" })).toEqual({ kind: "incomplete" });
    expect(sprintBlockers(index, s, { ...cached, graph: { ...cached.graph, complete: false } })).toEqual({ kind: "incomplete" });
    expect(sprintBlockers(buildIndex(state(s, tasks().slice(1)), 0), s)).toEqual({ kind: "incomplete" });
    const cyclic = tasks().map(t => t.id === "next" ? { ...t, dependencies: ["later"] } : t);
    expect(sprintBlockers(buildIndex(state(s, cyclic), 0), s)).toEqual({ kind: "cycle" });
    const w = state(s);
    w.machines[0].connection = "disconnected";
    expect(sprintBlockers(buildIndex(w, 0), s)).toEqual({ kind: "offline" });
  });

  it("uses current live links and distinguishes Automatic from Hybrid readiness", () => {
    const s = sprint();
    const members = tasks().map(t => t.id === "done" ? {
      ...t, status: "ready_to_merge" as const, closedAs: null, work: "merged" as const,
    } : t);
    const index = buildIndex(state(s, members), 0);
    const g = graph(s, members.map(t => t.id === "both" ? { ...t, dependencies: [] } : t));
    const cached = { status: "ready" as const, key: graphKey(index, s), graph: g, message: null };
    expect(sprintBlockers(index, s, cached)).toMatchObject({ kind: "ready", blocked: 3, clear: 1 });
    const h = sprint({ runMode: "hybrid" });
    expect(sprintBlockers(buildIndex(state(h, members), 0), h)).toMatchObject({ kind: "ready", blocked: 3, clear: 1 });
    expect(sprintBlockers(index, s)).toMatchObject({ kind: "ready", blocked: 4, clear: 0 });
  });
});

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); document.body.replaceChildren(); localStorage.clear(); sessionStorage.clear(); });
it.each(["pause", "question"] as const)("opens from the sprint board and navigates to a %s without a command or extra graph read", async kind => {
  const s = sprint(), harness = scriptedPort({ capabilities: { everything: true, operations: [] } });
  const reads: string[] = [];
  harness.port.sprintGraph = { async read(id) { reads.push(id); return { kind: "graph", graph: graph(s) }; }, async edit() { throw Error("unexpected edit"); } };
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/p/p/s/sp/board`);
  const root = document.body.appendChild(document.createElement("div"));
  dispose = render(() => <Workspace port={harness.port} />, root);
  await settled();
  const fixture = state(s);
  if (kind === "question") {
    const task = fixture.tasks.find(t => t.id === "paused")!;
    task.status = "needs_you";
    task.process = "live";
    const question = mapDecision(wireDecision({ executionId: task.executionId!, prompt: "Which cache should be shared?" }));
    const session = emptySession(task.id);
    session.decisions = [question];
    session.timeline = [{ id: "question-entry", kind: "decision", decisionId: question.id }];
    fixture.sessions = [session];
  }
  harness.emit(projection("1", fixture));
  await settled();
  const trigger = document.querySelector<HTMLButtonElement>(".sprint-blockers-trigger");
  expect(trigger).not.toBeNull();
  const readCount = reads.length;
  trigger!.focus();
  trigger!.click();
  await settled();
  const dialog = document.querySelector<HTMLDialogElement>(".sprint-blockers-dialog")!;
  expect(dialog.open).toBe(true);
  expect(dialog.textContent).toContain(kind === "pause" ? "Resuming alone does not satisfy" : "Decision needed");
  expect(document.activeElement?.textContent).toBe("Close");
  dialog.dispatchEvent(new Event("cancel", { cancelable: true }));
  await settled();
  expect(document.activeElement).toBe(trigger);
  trigger!.click();
  await settled();
  expect(reads).toHaveLength(readCount);
  dialog.querySelector<HTMLAnchorElement>("h3 a")!.click();
  await settled();
  expect(location.pathname).toContain("/t/paused");
  if (kind === "question") expect(document.querySelector(".task-panel")?.textContent).toContain("Which cache should be shared?");
  expect(harness.commands).toEqual([]);
});

it("renders the queue's named output refusal on the draft card and counts that draft as blocked", async () => {
  const reason = "This task requires report v1 from Measure delays. The matching reviewed, verified output is unavailable.";
  const members = tasks().filter(t => t.id === "done" || t.id === "free");
  const s = sprint({ taskIds: members.map(t => t.id), automaticQueue: {
    taskId: "free", kind: "settlement_blocked", code: "deliverable_unverified", reason, needsOwner: false,
  } });
  const harness = scriptedPort({ capabilities: { everything: true, operations: [] } });
  harness.port.sprintGraph = { async read() { return { kind: "graph", graph: graph(s, members) }; }, async edit() { throw Error("unexpected edit"); } };
  history.replaceState(null, "", "/m/" + LOCAL_MACHINE_ID + "/p/p/s/sp/board");
  const root = document.body.appendChild(document.createElement("div"));
  dispose = render(() => <Workspace port={harness.port} />, root);
  await settled();
  harness.emit(projection("1", state(s, members)));
  await settled();
  expect(document.querySelector('.card[data-task="free"] .card-s')?.textContent).toContain(reason);
  document.querySelector<HTMLButtonElement>(".sprint-blockers-trigger")!.click();
  await settled();
  const dialog = document.querySelector(".sprint-blockers-dialog")!;
  expect(dialog.textContent).toContain(reason);
  expect([...dialog.querySelectorAll("dd")].map(n => n.textContent)).toEqual(["1", "0"]);
  expect(dialog.textContent).not.toContain("No confirmed draft");
  expect(harness.commands).toEqual([]);
});
