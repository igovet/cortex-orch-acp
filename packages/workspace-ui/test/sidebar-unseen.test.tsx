import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type AutomaticQueue, type Decision, type Sprint, type Task, type TaskSession, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { SEEN_CHANGES_KEY, SEEN_CHANGES_LIMIT, stateCode } from "../src/unseen";
import { draftTask, flush, projection, scriptedPort, withTask } from "./support";

/** Sidebar rows that changed since the owner last opened them read as new until opened, across reloads. */
let dispose: (() => void) | undefined;
let version = 1;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); version = 1; });
afterEach(() => unmount());
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const unmount = () => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; };
const HOME = `/m/${LOCAL_MACHINE_ID}`;

const task = (id: string, title: string, patch: Partial<Task> = {}): Task => ({ ...draftTask(), id, title, status: "working", ...patch });
const sprint = (patch: Partial<Sprint> = {}): Sprint => ({ id: "sp", projectId: "p", description: "Launch", title: "Launch", runMode: "manual", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["w"], queueState: "running", pauseReason: null, ...patch } as Sprint);
const world = (tasks: Task[], sprints: Sprint[] = [], sessions: TaskSession[] = []): WorkspaceState => {
  const s = withTask(emptyWorkspace(), "unused");
  s.machines[0].connection = "connected";
  return { ...s, tasks, sprints, sessions };
};
/** A second project, `q`, beside `p`. */
const withSecond = (state: WorkspaceState): WorkspaceState => ({ ...state, projects: [...state.projects, { ...state.projects[0], id: "q", name: "Second", repository: "/q" }] });
const question = (id: string, text: string, answered = false): Decision => ({ id, kind: "question", from: "coordinator", workerId: null, at: null, text, options: [], answer: answered ? { text: "Red", at: null } : null, withdrawn: false });
const session = (taskId: string, ...decisions: Decision[]): TaskSession => ({ ...emptySession(taskId), decisions });

async function mount(state: WorkspaceState, path = HOME) {
  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={s.port} />, root);
  await settle(); s.emit(projection(String(version++), state)); await settle();
  return { push: async (next: WorkspaceState) => { s.emit(projection(String(version++), next)); await settle(); } };
}
/** Every visible sidebar row whose title is `title`. */
const pickView = async (name: string) => {
  [...document.querySelectorAll<HTMLButtonElement>(".sb-view-head .seg button")].find(b => b.textContent === name)!.click(); await settle();
};
const rows = (title: string, within = ".sidebar") => [...document.querySelectorAll<HTMLElement>(`${within} .task-row`)].filter(r => r.querySelector(".trunc")?.textContent === title);
const fresh = (title: string) => rows(title).some(r => r.classList.contains("unseen"));
const open = async (title: string) => { rows(title)[0].click(); await settle(); };
const goHome = async () => { [...document.querySelectorAll<HTMLElement>(".sidebar .nav")].find(a => a.textContent?.trim() === "Home")!.click(); await settle(); };
const asks = (text: string): Partial<Task> => ({ status: "needs_you", issue: `question: ${text}` });
const saved = () => JSON.parse(localStorage.getItem(SEEN_CHANGES_KEY) ?? "{}") as { v?: number; seen?: string[][]; projects?: [string, string, number][] };
const ids = () => (saved().seen ?? []).map(m => m[0]);

describe("sidebar unseen changes", () => {
  it("a task that comes to need the owner reads as new, with a mark and words, until it is opened", async () => {
    const app = await mount(world([task("t", "Alpha"), task("u", "Beta")]));
    expect(fresh("Alpha")).toBe(false);
    await app.push(world([task("t", "Alpha", asks("Which colour?")), task("u", "Beta")]));
    expect(fresh("Alpha")).toBe(true);
    expect(fresh("Beta")).toBe(false);
    const row = rows("Alpha").find(r => r.classList.contains("unseen"))!;
    expect(row.getAttribute("aria-label")).toMatch(/new update$/);
    expect(row.querySelector(".sb-new")?.getAttribute("aria-hidden")).toBe("true");
    await open("Alpha");
    expect(fresh("Alpha")).toBe(false);
    await goHome();
    expect(fresh("Alpha")).toBe(false);
    expect(rows("Alpha")[0].getAttribute("aria-label")).not.toMatch(/new update/);
  });

  it("a later change after opening reads as new again: a new question, or the same one after the task worked again", async () => {
    const app = await mount(world([task("t", "Alpha")]));
    await app.push(world([task("t", "Alpha", asks("Which colour?"))]));
    await open("Alpha"); await goHome();
    expect(fresh("Alpha")).toBe(false);
    await app.push(world([task("t", "Alpha", asks("Which size?"))]));
    expect(fresh("Alpha")).toBe(true);
    await open("Alpha"); await goHome();
    await app.push(world([task("t", "Alpha")]));
    expect(fresh("Alpha")).toBe(false);
    await app.push(world([task("t", "Alpha", asks("Which size?"))]));
    expect(fresh("Alpha")).toBe(true);
  });

  it("another question in place of the one opened reads as new, even with the same words and no working read between", async () => {
    const app = await mount(world([task("t", "Alpha")]));
    await app.push(world([task("t", "Alpha", asks("Which colour?"))], [], [session("t", question("d1", "Which colour?"))]));
    expect(fresh("Alpha")).toBe(true);
    await open("Alpha"); await goHome();
    expect(fresh("Alpha")).toBe(false);
    // The same read again changes nothing.
    await app.push(world([task("t", "Alpha", asks("Which colour?"))], [], [session("t", question("d1", "Which colour?"))]));
    expect(fresh("Alpha")).toBe(false);
    await app.push(world([task("t", "Alpha", asks("Which colour?"))], [], [session("t", question("d1", "Which colour?", true), question("d2", "Which colour?"))]));
    expect(fresh("Alpha")).toBe(true);
    // What is kept names rows and projects and holds fixed-size codes, never a question's words.
    const text = localStorage.getItem(SEEN_CHANGES_KEY) ?? "";
    expect(text).not.toMatch(/colour/i);
    expect(saved().v).toBe(3);
    expect((saved().seen ?? []).every(([, key]) => /^([0-9a-f]{16})?$/.test(key))).toBe(true);
    expect(saved().projects).toEqual([["p", "", 0]]);
  });

  it("another queue hold on the same task reads as new", async () => {
    const hold = (holdId: string): AutomaticQueue => ({ kind: "settlement_blocked", taskId: "w", code: "stale_result", reason: "stale_result", needsOwner: true, holdId });
    const app = await mount(world([task("w", "Member")], [sprint({ runMode: "hybrid" })]));
    await app.push(world([task("w", "Member", { status: "review" })], [sprint({ runMode: "hybrid", queueState: "paused", automaticQueue: hold("h1") })]));
    expect(fresh("Member")).toBe(true);
    await open("Member"); await goHome();
    expect(fresh("Member")).toBe(false);
    await app.push(world([task("w", "Member", { status: "review" })], [sprint({ runMode: "hybrid", queueState: "paused", automaticQueue: hold("h2") })]));
    expect(fresh("Member")).toBe(true);
  });

  it("a task that finishes on its own stays listed as new until opened, and leaves once the owner moves on", async () => {
    const app = await mount(world([task("t", "Alpha")]));
    await app.push(world([task("t", "Alpha", { status: "done", doneAt: 1_700_000_000_000 })]));
    expect(rows("Alpha").length).toBeGreaterThan(0);
    expect(fresh("Alpha")).toBe(true);
    expect(rows("Alpha")[0].querySelector(".sig")?.textContent).toBe("Done");
    await pickView("Active work");
    expect(document.querySelector(".sb-work")?.textContent).toContain("Finished");
    await open("Alpha");
    expect(rows("Alpha").length).toBeGreaterThan(0);
    expect(fresh("Alpha")).toBe(false);
    await goHome();
    expect(rows("Alpha")).toHaveLength(0);
  });

  it("a task closed in another window or client, never opened here, reads as new until opened", async () => {
    const app = await mount(world([task("t", "Alpha")]));
    await app.push(world([task("t", "Alpha", { status: "done", closedAs: "as_is", doneAt: 1_700_000_000_000 })]));
    expect(fresh("Alpha")).toBe(true);
    await pickView("Active work");
    expect(rows("Alpha", ".sb-work")).toHaveLength(1);
    await open("Alpha"); await goHome();
    expect(rows("Alpha")).toHaveLength(0);
  });

  it("a run of an Automatic sprint that fails or is cancelled reads as new, though it sits with the working tasks", async () => {
    const auto = (patch: Partial<Sprint> = {}) => sprint({ runMode: "automatic", taskIds: ["w", "v"], ...patch });
    const app = await mount(world([task("w", "Wren"), task("v", "Vole")], [auto()]));
    expect(fresh("Wren")).toBe(false);
    await app.push(world([task("w", "Wren", { status: "failed", executionId: "e1" }), task("v", "Vole", { status: "cancelled", executionId: "e2" })], [auto()]));
    expect(rows("Wren").length).toBeGreaterThan(0);
    expect(fresh("Wren")).toBe(true);
    expect(fresh("Vole")).toBe(true);
    await open("Wren"); await goHome();
    expect(fresh("Wren")).toBe(false);
    expect(fresh("Vole")).toBe(true);
  });

  it("the task open on screen counts as seen as it changes", async () => {
    const app = await mount(world([task("t", "Alpha")]), `${HOME}/t/t`);
    await app.push(world([task("t", "Alpha", asks("Which colour?"))]));
    expect(fresh("Alpha")).toBe(false);
    await goHome();
    expect(fresh("Alpha")).toBe(false);
  });

  it("a finished sprint marks its row until the sprint is opened", async () => {
    const app = await mount(world([task("w", "Member")], [sprint()]));
    expect(fresh("Sprint: Launch")).toBe(false);
    await app.push(world([task("w", "Member", { status: "done", doneAt: 1_700_000_000_000 })], [sprint({ queueState: "complete" })]));
    expect(fresh("Sprint: Launch")).toBe(true);
    const row = rows("Sprint: Launch").find(r => r.classList.contains("unseen"))!;
    expect(row.querySelector(".ws-sr")?.textContent).toContain("new update");
    await open("Sprint: Launch");
    expect(fresh("Sprint: Launch")).toBe(false);
    await goHome();
    expect(fresh("Sprint: Launch")).toBe(false);
  });

  it("a finished Hybrid sprint with a task still ready to merge lists that task once in Active work", async () => {
    const hybrid = (patch: Partial<Sprint> = {}) => sprint({ runMode: "hybrid", taskIds: ["w", "x"], ...patch });
    const app = await mount(world([task("w", "Wren"), task("x", "Lark")], [hybrid()]));
    await app.push(world([task("w", "Wren", { status: "ready_to_merge" }), task("x", "Lark", { status: "done", doneAt: 1 })], [hybrid({ queueState: "complete" })]));
    expect(fresh("Sprint: Launch")).toBe(true);
    await pickView("Active work");
    expect(rows("Sprint: Launch", ".sb-work")).toHaveLength(1);
    expect(rows("Wren", ".sb-work")).toHaveLength(1);
    expect(rows("Lark", ".sb-work")).toHaveLength(1);
    // Once the sprint is seen it leaves, and its task waiting to merge is listed on its own again, still once.
    await open("Sprint: Launch"); await goHome();
    expect(rows("Sprint: Launch", ".sb-work")).toHaveLength(0);
    expect(rows("Wren", ".sb-work")).toHaveLength(1);
  });

  it("a folded project speaks for its new rows", async () => {
    const app = await mount(world([task("t", "Alpha")]));
    const project = () => document.querySelector<HTMLElement>(".sidebar .proj")!;
    document.querySelector<HTMLElement>(".sidebar .proj-x")!.click(); await settle();
    expect(document.querySelector(".sidebar .tasks")).toBeNull();
    await app.push(world([task("t", "Alpha", asks("Which colour?"))]));
    expect(project().classList.contains("unseen")).toBe(true);
    expect(project().textContent).toContain("new updates");
    document.querySelector<HTMLElement>(".sidebar .proj-x")!.click(); await settle();
    expect(project().classList.contains("unseen")).toBe(false);
    expect(fresh("Alpha")).toBe(true);
  });

  it("states already present on the first run are taken as seen", async () => {
    const before = world([task("t", "Alpha", asks("Which colour?")), task("u", "Beta", { status: "review" }), task("v", "Gamma", { status: "failed" }), task("w", "Member", { status: "done", doneAt: 1 })], [sprint({ queueState: "complete" })]);
    await mount(before);
    expect(fresh("Alpha")).toBe(false);
    expect(fresh("Beta")).toBe(false);
    expect(fresh("Gamma")).toBe(false);
    expect(fresh("Sprint: Launch")).toBe(false);
    expect(rows("Member")).toHaveLength(0);
    expect(document.querySelector(".sidebar .unseen")).toBeNull();
    expect(ids().sort()).toEqual(["s:sp", "t:t", "t:u", "t:v", "t:w"]);
  });

  it("a record kept by an earlier version is not read as a change", async () => {
    localStorage.setItem(SEEN_CHANGES_KEY, JSON.stringify({ projects: ["p"], seen: [["t:t", "needs|needs_you|question: Old", "p"]] }));
    await mount(world([task("t", "Alpha", asks("Which colour?"))]));
    expect(fresh("Alpha")).toBe(false);
    expect(saved().v).toBe(3);
  });

  it("a record of the previous layout keeps its rows, and takes each project in afresh", async () => {
    localStorage.setItem(SEEN_CHANGES_KEY, JSON.stringify({ v: 2, seen: [["t:t", "", "p"]] }));
    // Alpha was seen asking nothing, so its question is new; Beta has no record and the layout names no project taken in.
    await mount(world([task("t", "Alpha", asks("Which colour?")), task("u", "Beta", asks("Which size?"))]));
    expect(fresh("Alpha")).toBe(true);
    expect(fresh("Beta")).toBe(false);
    expect(saved().v).toBe(3);
    expect(saved().projects).toEqual([["p", "", 0]]);
  });

  it("a task created while the app was closed reads as new on the next launch, waiting on the owner or finished", async () => {
    await mount(world([task("t", "Alpha")]));
    unmount();
    const later = world([task("t", "Alpha"), task("u", "Beta", asks("Which size?")), task("v", "Gamma", { status: "done", doneAt: 1, executionId: "e1" }), task("w", "Delta", { status: "failed", executionId: "e2" }), task("x", "Quiet")]);
    await mount(later);
    expect(fresh("Alpha")).toBe(false);
    expect(fresh("Beta")).toBe(true);
    expect(fresh("Gamma")).toBe(true);
    await pickView("Active work");
    expect(rows("Gamma", ".sb-work")).toHaveLength(1);
    await pickView("Projects");
    expect(fresh("Delta")).toBe(true);
    expect(fresh("Quiet")).toBe(false);
    await open("Beta"); await goHome();
    unmount();
    // Opened is remembered; what was not opened stays new across another launch.
    await mount(later);
    expect(fresh("Beta")).toBe(false);
    expect(fresh("Gamma")).toBe(true);
    expect(fresh("Delta")).toBe(true);
  });

  it("a task created while the app was closed in a project that was empty reads as new", async () => {
    await mount(world([]));
    unmount();
    await mount(world([task("u", "Beta", asks("Which size?"))]));
    expect(fresh("Beta")).toBe(true);
  });

  it("a finished sprint created while the app was closed reads as new", async () => {
    await mount(world([]));
    unmount();
    await mount(world([task("w", "Member", { status: "done", doneAt: 1 })], [sprint({ queueState: "complete" })]));
    expect(fresh("Sprint: Launch")).toBe(true);
  });

  it("a project taken in for the first time takes its present rows as seen, though another was taken in before", async () => {
    await mount(world([task("t", "Alpha")]));
    unmount();
    await mount(withSecond(world([task("t", "Alpha"), task("r", "Robin", { projectId: "q", ...asks("Which size?") })])));
    expect(fresh("Robin")).toBe(false);
    expect(saved().projects?.map(p => p[0]).sort()).toEqual(["p", "q"]);
  });

  it("rows the bound dropped stay seen after a reload, while a task created beside them while closed reads as new", async () => {
    const done = Array.from({ length: SEEN_CHANGES_LIMIT + 100 }, (_, i) => task(`d${i}`, `Done ${i}`, { status: "done", doneAt: 1, executionId: `e${i}` }));
    await mount(world(done));
    expect(ids()).toHaveLength(SEEN_CHANGES_LIMIT);
    // The dropped rows leave short codes of their ids, never the ids.
    const [[, codes, lossy]] = saved().projects!;
    expect(lossy).toBe(0);
    expect(codes).toBe(Array.from({ length: 100 }, (_, i) => stateCode(["row", `t:d${i}`]).slice(0, 8)).join(""));
    expect(ids()).not.toContain("t:d0");
    expect(localStorage.getItem(SEEN_CHANGES_KEY)).not.toContain("\"t:d0\"");
    unmount();
    await mount(world([...done, task("n", "Newcomer", asks("Which size?"))]));
    expect(fresh("Newcomer")).toBe(true);
    expect(rows("Done 0")).toHaveLength(0);
    expect(document.querySelectorAll(".sidebar .task-row.unseen").length).toBe(rows("Newcomer").length);
  });

  it("a project whose dropped codes did not fit takes its unremembered rows as seen, and is exact again once taken in", async () => {
    localStorage.setItem(SEEN_CHANGES_KEY, JSON.stringify({ v: 3, seen: [], projects: [["p", "", 1]] }));
    await mount(world([task("u", "Beta", asks("Which size?"))]));
    expect(fresh("Beta")).toBe(false);
    expect(saved().projects).toEqual([["p", "", 0]]);
    unmount();
    await mount(world([task("u", "Beta", asks("Which size?")), task("v", "Gamma", asks("Which colour?"))]));
    expect(fresh("Beta")).toBe(false);
    expect(fresh("Gamma")).toBe(true);
  });

  it("a dropped row's code keeps it seen, while an unreadable project entry is not read", async () => {
    const code = stateCode(["row", "t:u"]).slice(0, 8);
    localStorage.setItem(SEEN_CHANGES_KEY, JSON.stringify({ v: 3, seen: [], projects: [["p", code, 0], ["q", "not-a-code", 0]] }));
    await mount(withSecond(world([task("u", "Beta", asks("Which size?")), task("v", "Gamma", asks("Which colour?")), task("r", "Robin", { projectId: "q", ...asks("Which size?") })])));
    expect(fresh("Beta")).toBe(false);
    expect(fresh("Gamma")).toBe(true);
    expect(fresh("Robin")).toBe(false);
  });

  it("more finished rows than the bound on the first run, and after a reload, read as nothing new", async () => {
    const many = world(Array.from({ length: SEEN_CHANGES_LIMIT + 100 }, (_, i) => task(`d${i}`, `Done ${i}`, { status: "done", doneAt: 1, executionId: `e${i}` })));
    await mount(many);
    expect(document.querySelector(".sidebar .unseen")).toBeNull();
    expect(rows("Done 0")).toHaveLength(0);
    expect(ids()).toHaveLength(SEEN_CHANGES_LIMIT);
    unmount();
    // The rows the bound dropped are not remembered, so they are taken as seen again, never as new.
    await mount(many);
    expect(document.querySelector(".sidebar .unseen")).toBeNull();
    expect(rows("Done 0")).toHaveLength(0);
  });

  it("an opened row stays seen after later rows push older records out", async () => {
    const quiet = (n: number) => Array.from({ length: n }, (_, i) => task(`q${i}`, `Quiet ${i}`, { status: "draft" }));
    let app = await mount(world([task("t", "Alpha"), ...quiet(10)]));
    await app.push(world([task("t", "Alpha", asks("Which colour?")), ...quiet(10)]));
    await open("Alpha"); await goHome();
    await app.push(world([task("t", "Alpha", asks("Which colour?")), ...quiet(SEEN_CHANGES_LIMIT + 50)]));
    expect(ids()).toHaveLength(SEEN_CHANGES_LIMIT);
    unmount();
    app = await mount(world([task("t", "Alpha", asks("Which colour?")), ...quiet(SEEN_CHANGES_LIMIT + 50)]));
    expect(fresh("Alpha")).toBe(false);
  });

  it("marks persist across a reload, and opening is remembered too", async () => {
    let app = await mount(world([task("t", "Alpha"), task("u", "Beta")]));
    const after = world([task("t", "Alpha", asks("Which colour?")), task("u", "Beta", asks("Which size?"))]);
    await app.push(after);
    expect(fresh("Alpha")).toBe(true);
    await open("Beta"); await goHome();
    unmount();
    app = await mount(after);
    expect(fresh("Alpha")).toBe(true);
    expect(fresh("Beta")).toBe(false);
    await open("Alpha");
    unmount();
    await mount(after);
    expect(fresh("Alpha")).toBe(false);
    expect(fresh("Beta")).toBe(false);
  });

  it("a row that changed while the app was closed reads as new on the next launch", async () => {
    await mount(world([task("t", "Alpha")]));
    unmount();
    await mount(world([task("t", "Alpha", asks("Which colour?"))]));
    expect(fresh("Alpha")).toBe(true);
  });

  it("a deleted task takes its record along, the last one of its project too", async () => {
    const app = await mount(world([task("t", "Alpha"), task("u", "Beta")]));
    await app.push(world([task("t", "Alpha", asks("Which colour?")), task("u", "Beta", asks("Which size?"))]));
    await open("Alpha"); await goHome();
    expect(ids()).toContain("t:t");
    await app.push(world([task("u", "Beta", asks("Which size?"))]));
    expect(ids()).not.toContain("t:t");
    expect(ids()).toContain("t:u");
    await app.push(world([]));
    expect(ids()).toEqual([]);
  });

  it("an unreadable or unlisted project keeps its records, within the bound", async () => {
    const gone = Array.from({ length: SEEN_CHANGES_LIMIT + 200 }, (_, i) => [`t:x${i}`, "", "gone"]);
    localStorage.setItem(SEEN_CHANGES_KEY, JSON.stringify({ v: 2, seen: [...gone, ["t:u", "", "p"]] }));
    const state = withSecond(world([task("r", "Robin", { projectId: "q" })]));
    state.projects[0].issue = "The project could not be read.";
    // Project p cannot be read for now (its task may come back), project `gone` is not listed: both keep what is kept of them.
    await mount(state);
    expect(ids()).toHaveLength(SEEN_CHANGES_LIMIT);
    expect(ids()).toContain("t:r");
    expect(ids()).toContain("t:u");
    expect(ids()).toContain(`t:x${SEEN_CHANGES_LIMIT + 199}`);
    expect(ids()).not.toContain("t:x0");
  });
});
