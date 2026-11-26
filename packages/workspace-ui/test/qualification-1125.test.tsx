// @vitest-environment jsdom
import { createRoot } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emptyWorkspace, LOCAL_MACHINE_ID, parseLiveSprintGraph, type LiveSprintGraph, type ProjectFacts, type Sprint, type Task, type Worker, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createRealPort, usableProduct } from "../src/port/real";
import { hrefOf } from "../src/router";
import { buildIndex, REVISION_WAIT, revisionHeld } from "../src/selectors";
import { workerState } from "../src/session-model";
import { createWorkspaceStore } from "../src/store";
import { taskActionsFor } from "../src/task-actions-model";
import { capabilitiesWire, draftTask, flush, projection, scriptedPort, scriptedTransport, snapshot, wireTask, withTask } from "./support";

/*
 * Regressions for the defects the Dev qualification of Cortex Workspace found on 2026-11-25: a task record the service
 * answers as null froze every later read of its project; a sprint revision's owner card was clamped to one line and a draft
 * it holds back still offered Start; the context menu closed on release; an Automatic pause had no Resume on its page; the
 * inspector showed the machine's defaults as a task's route; a bare file name in a description was no link; and one worker
 * read Cancelled, Failed and "Could not run here" in different places.
 */

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); vi.useRealTimers(); });
const settle = async () => { for (let i = 0; i < 12; i++) await flush(); };
const machine = LOCAL_MACHINE_ID;

async function mount(path: string, state: WorkspaceState, extra?: (p: ReturnType<typeof scriptedPort>) => void) {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  extra?.(p);
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", state)); await settle();
  return p;
}
const world = (patch: Partial<Task> = {}) => {
  const s = withTask(emptyWorkspace(), "Qualified");
  s.machines[0].connection = "connected";
  Object.assign(s.tasks[0], patch);
  return s;
};
const facts = (overlays: ProjectFacts["overlays"], workers: Worker[] = []): ProjectFacts =>
  ({ projectId: "p", openBranch: "main", defaultBranch: "main", repo: null, workers, reviews: [], sprints: [], overlays });

describe("1. a task record answered as null", () => {
  it("is left off the board instead of failing the project's read", () => {
    const product = usableProduct({ tasks: { kept: { status: "done" }, gone: null } as Record<string, unknown>, sprints: [null, { id: "s" }] as unknown[] });
    expect(Object.keys(product.tasks as object)).toEqual(["kept"]);
    expect(product.sprints).toEqual([{ id: "s" }]);
  });

  it("keeps every later board and facts read of the project working", async () => {
    let reopened = false;
    // The service of the board test in real-port.test.ts, with a deleted task the product answers as null.
    const s = scriptedTransport({
      "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot"]),
      "project.snapshot": () => snapshot("4", [wireTask()]),
    });
    s.answers["projects.list"] = () => [{ projectId: "p", displayName: "Project", canonicalRoot: "/p", activation: {} }];
    s.answers["service.capabilities"] = () => ({ methods: ["native.board", "native.query", "machine.call"], machine: true });
    s.answers["native.board"] = () => {
      const version = reopened ? "5" : "4";
      const item = wireTask({ executionId: "e", version, ...(reopened ? { phase: "planning", executionState: "executing", runtimeState: "PLANNING" }
        : { phase: "result", executionState: "result", runtimeState: "COMPLETED" }) });
      return { protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "g", version, highWatermark: version,
        data: { type: "board", value: { items: [item], nextCursor: null, highWatermark: version } } };
    };
    s.answers["native.query"] = () => { throw Error("No detail read in this test"); };
    s.answers["machine.call"] = raw => {
      const { op } = raw as { op: string };
      if (op === "repo.snapshot") return { ok: true, result: null };
      if (op === "workers") return { ok: true, result: [] };
      return { ok: true, result: { tasks: { "task-1": reopened ? { reworkEvent: "22", title: "Reopened" } : { status: "done", title: "Closed" }, "deleted-member": null }, sprints: [] } };
    };
    const p = createRealPort({ transport: s.transport, actorId: "local-peer", registry: true });
    await p.query("capabilities.get", {});
    await p.query("project.snapshot", {});
    const [first] = await p.query("project.facts", {});
    expect(first.overlays["task-1"]).toMatchObject({ title: "Closed" });
    expect("deleted-member" in first.overlays).toBe(false);
    // The read after it (where the previous facts are carried over) used to fail on the null record, every time.
    reopened = true;
    await p.query("project.snapshot", {});
    const [later] = await p.query("project.facts", {});
    expect(later.overlays["task-1"]).toMatchObject({ title: "Reopened", reworkEvent: "22" });
  });

  it("one update the store cannot apply never stops the updates after it", async () => {
    const s = scriptedPort();
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const store = createRoot(d => { dispose = d; return createWorkspaceStore(s.port); });
    await flush();
    s.emit(projection("1", world({ title: "Before" })));
    // Malformed facts (no worker list) throw while they are applied: they are left out, and nothing else is.
    expect(() => s.emit({ kind: "facts", facts: { ...facts({}), workers: undefined as unknown as Worker[] } })).not.toThrow();
    expect(error).toHaveBeenCalled();
    s.emit(projection("2", world({ title: "After" })));
    expect(store.state().tasks.map(t => t.title)).toEqual(["After"]);
    s.emit({ kind: "facts", facts: facts({ t: { title: "From facts" } }) });
    expect(store.state().tasks.map(t => t.title)).toEqual(["From facts"]);
  });
});

const REV = "b".repeat(64);
function sprintWorld(): WorkspaceState {
  const s = emptyWorkspace();
  s.machines[0].connection = "connected";
  s.projects.push({ id: "p", machineId: machine, name: "Cedar", repository: "/cedar", openBranch: "main", defaultBranch: "main" });
  const t = (id: string, title: string, patch: Partial<Task> = {}): Task => ({ ...draftTask(), id, title, description: `${title} as planned`, sprintId: "sp", ...patch });
  s.tasks.push(t("t1", "Lay the schema", { status: "working", process: "live", executionId: "e1" }), t("t2", "Write the importer"), t("t3", "Write the exporter"));
  const sprint: Sprint = { id: "sp", projectId: "p", description: "Data", title: "Data", runMode: "hybrid", targetBranch: null, planning: "created",
    conversation: [], plan: { version: 1, items: [] }, materializedVersion: 1, taskIds: ["t1", "t2", "t3"], queueState: "running", pauseReason: null };
  s.sprints.push(sprint);
  return s;
}
/** An order read before the revision (it still ranks the held draft first) and the revision that holds it back. */
const heldGraph = (): LiveSprintGraph => parseLiveSprintGraph({ contractVersion: 1, sprintId: "sp", graphRevision: REV, complete: true,
  priority: ["t1", "t2", "t3"], order: { mode: "hybrid", queueState: "running", working: [{ taskId: "t1", startOrder: "1" }],
    notStarted: [{ taskId: "t2", rank: 1, reason: null, next: false, conditional: false }, { taskId: "t3", rank: 2, reason: null, next: false, conditional: false }] },
  members: ["t1", "t2", "t3"].map(id => ({ taskId: id, readStatus: "confirmed", dependencies: [], editable: id !== "t1", canBePredecessor: true, approvedPlanKey: null })),
  revisions: [{ revisionId: "sr_1", taskId: "t1", status: "awaiting_owner", applied: false, reason: "The owner switched to CSV",
    changes: [{ op: "update", member_id: "t2", expected_version: "2", title: "Import CSV", description: "Import the CSV file" }],
    held: ["t2"], updated: [], added: [] }] })!;

describe("2. a Hybrid sprint revision waiting for the owner", () => {
  it("is a whole card: who proposed it, why, the change as before and after, and visible Apply and Reject", async () => {
    await mount(`/m/${machine}/p/p/s/sp/plan`, sprintWorld(), port => {
      port.port.sprintGraph = { read: async () => ({ kind: "graph", graph: heldGraph() }), edit: async () => ({ outcome: "changed" }) };
    });
    const card = document.querySelector<HTMLElement>(".sp-revision")!;
    expect(card).not.toBeNull();
    // Not clamped into a two-line strip: the strip that holds it is readable whole.
    expect(card.closest(".l-strip")?.classList.contains("l-strip-readable")).toBe(true);
    expect(card.classList.contains("note-panel")).toBe(false);
    expect(card.querySelector(".sp-rev-head")?.textContent).toContain("Sprint changes from task Lay the schema");
    expect(card.querySelector(".sp-rev-reason")?.textContent).toBe("The owner switched to CSV");
    expect([...card.querySelectorAll(".sp-rev-old")].map(e => e.textContent)).toEqual(["Write the importer", "Write the importer as planned"]);
    expect([...card.querySelectorAll(".sp-rev-new")].map(e => e.textContent)).toEqual(["Import CSV", "Import the CSV file"]);
    const actions = [...card.querySelectorAll<HTMLButtonElement>(".sp-rev-actions button")].map(b => b.textContent);
    expect(actions).toEqual(["Apply", "Reject"]);
  });

  it("holds its draft back: no queue place and no Start, even when the order was read before the revision", () => {
    const s = sprintWorld(), graph = heldGraph();
    expect(revisionHeld(graph)).toEqual(["t2"]);
    const index = buildIndex(s, 0, new Map([["sp", graph.order]]), new Set(revisionHeld(graph)));
    const held = index.byId.get("t2")!, free = index.byId.get("t3")!;
    expect(held.fact.text).toBe(REVISION_WAIT);
    expect(free.fact.text).toBe("Queued 2nd in Data");
    const noop = vi.fn();
    const ws = { store: { index: () => index, state: () => s, clock: () => 0, pendingTask: () => false }, router: { route: () => ({ name: "home", machine }) },
      panels: { pins: () => [], togglePin: noop }, run: noop, go: noop, select: noop, inspectable: () => true, gate: () => undefined, can: () => true,
      t: (key: string) => key, open: noop, stopTask: noop, closeTask: noop, archiveTask: noop, returnForRework: noop, noOpener: () => undefined, externalLabel: () => "Open" };
    const start = taskActionsFor(held, ws as unknown as Parameters<typeof taskActionsFor>[1]).find(a => a.id === "start")!;
    expect(start.enabled).toBe(false);
    expect(start.why).toMatch(/sprint revision/);
    expect(taskActionsFor(free, ws as unknown as Parameters<typeof taskActionsFor>[1]).find(a => a.id === "start")?.enabled).toBe(true);
    // Applied or rejected, it holds nothing back.
    expect(revisionHeld({ revisions: [{ ...graph.revisions![0], status: "rejected" }] })).toEqual([]);
    expect(revisionHeld({ revisions: [{ ...graph.revisions![0], status: "applied", applied: true }] })).toEqual([]);
  });
});

describe("3. the context menu opened by a right press", () => {
  it("stays open after the button is released", async () => {
    await mount(hrefOf({ name: "project", machine, project: "p", view: "list", scope: { kind: "all" } }), world());
    const link = document.querySelector(".lrow-shell a")!, menu = document.querySelector<HTMLElement>("#m-task")!;
    menu.style.left = "";
    // Linux sends the context menu on the press, with the button still held: the menu waits for the release.
    expect(link.dispatchEvent(new MouseEvent("contextmenu", { button: 2, buttons: 2, bubbles: true, cancelable: true, clientX: 210, clientY: 150 }))).toBe(false);
    expect(menu.style.left).toBe("");
    window.dispatchEvent(new MouseEvent("pointerup", { button: 2, bubbles: true }));
    await settle();
    expect(menu.style.left).toBe("210px");
    expect(menu.style.top).toBe("150px");
    expect(menu.querySelector("[data-task-action='start']")).not.toBeNull();
    // A context menu sent on the release opens at once.
    menu.style.left = "";
    link.dispatchEvent(new MouseEvent("contextmenu", { button: 2, buttons: 0, bubbles: true, cancelable: true, clientX: 220, clientY: 160 }));
    expect(menu.style.left).toBe("220px");
  });
});

describe("6. the inspector's route of a task", () => {
  const route = () => [...document.querySelectorAll(".ins-sec")].find(e => e.querySelector(".ins-h")?.textContent === "Route and access")!;
  /** The project's facts are read, then a task is created: the board shows it before a facts read has its saved record. */
  async function created() {
    const before = world({ status: "done", process: "stopped" });
    before.options.models = [{ id: "route-x", label: "Model X", engine: "Claude" }];
    const p = await mount(`/m/${machine}/t/n`, before);
    p.emit({ kind: "facts", facts: facts({ t: {} }) });
    await settle();
    const after = structuredClone(before);
    after.tasks.push({ ...after.tasks[0], id: "n", title: "Just created", status: "working", process: "live", executionId: "e" });
    p.emit(projection("2", after));
    await settle();
    return p;
  }

  it("is a placeholder until the task's saved route is read, never the machine's defaults", async () => {
    const p = await created();
    expect(route().querySelector(".ws-skel-strip")).not.toBeNull();
    expect(route().textContent).not.toContain("What this machine sets");
    p.emit({ kind: "facts", facts: facts({ t: {}, n: { settings: { model: "route-x", effort: "high", mode: "guarded", language: "auto" } } }) });
    await settle();
    expect(route().querySelector(".ws-skel-strip")).toBeNull();
    expect(route().textContent).toContain("Model X");
  });

  it("is the machine's route once a read made while the task is on the board has no route of its own", async () => {
    const p = await created();
    expect(route().querySelector(".ws-skel-strip")).not.toBeNull();
    // The same facts again, read now: still no record of it, so it runs on the machine's route, and that is shown.
    p.emit({ kind: "facts", facts: facts({ t: {} }) });
    await settle();
    expect(route().querySelector(".ws-skel-strip")).toBeNull();
  });

  it("shows the machine's route at once for a task that was on the board when its project's facts were read", async () => {
    const p = await mount(`/m/${machine}/t/t`, world({ status: "working", process: "live", executionId: "e" }));
    expect(route().querySelector(".ws-skel-strip")).toBeNull();
    p.emit({ kind: "facts", facts: facts({}) });
    await settle();
    expect(route().querySelector(".ws-skel-strip")).toBeNull();
  });
});

describe("7. a task description", () => {
  it("links a bare file name the checkout has, as questions and results do", async () => {
    const s = world({ status: "working", process: "live", executionId: "e", description: "Append a line to README.md and leave NOTES.md alone." });
    s.projects[0].repository = "/fixture";
    s.repos = [{ projectId: "p", remote: "", tree: ["README.md", "src/app.ts"], sources: {}, commits: [], branches: [], fetchedAt: null,
      checkouts: [{ id: "p:main", projectId: "p", kind: "main", path: "/fixture", branch: "main", files: [], sources: {}, taskId: null, label: null, base: "main", ahead: 0, behind: 0, behindOf: {}, draft: "", updateConflict: null }] }];
    await mount(`/m/${machine}/t/t`, s);
    const links = [...document.querySelectorAll<HTMLAnchorElement>(".brief .ws-result-copy a")].map(a => a.textContent);
    expect(links).toEqual(["README.md"]);
  });
});

describe("9. one worker's state", () => {
  const worker = (patch: Partial<Worker>): Worker => ({ id: "w1", taskId: "t", executionId: "e", role: "general", status: "failed", lane: "execution", goal: "Run it",
    now: null, waitsFor: null, scope: [], route: null, attempt: null, elapsedMs: 1000, runningSince: null, context: null, usage: null, activity: [], result: null, ...patch });

  it("reads the same everywhere: a failed worker that could not run here never also reads Failed or Cancelled", async () => {
    const w = worker({ blockerKind: "tool_unavailable" } as Partial<Worker>);
    expect(workerState(w).label).toBe("Could not run here");
    const s = world({ status: "needs_you", process: "live", executionId: "e" });
    s.workers = [w];
    await mount(`/m/${machine}/t/t`, s);
    const inspector = [...document.querySelectorAll(".ins-sec")].find(e => e.querySelector(".ins-h")?.textContent?.startsWith("Workers"))!;
    expect(inspector.textContent).toContain("Could not run here");
    expect(inspector.textContent).not.toMatch(/Failed|Cancelled/);
  });
});
