import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type Decision, type Sprint, type Task } from "@cortex/workspace-domain";
import { Workspace, type PhoneFrame } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); document.body.innerHTML = ""; vi.restoreAllMocks(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const world = () => { const s = withTask(emptyWorkspace(), "Work"); s.machines[0].connection = "connected"; return s; };
async function mount(state: ReturnType<typeof world>, path: string, shell?: PhoneFrame) {
  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={s.port} shell={shell} />, root);
  await settle(); s.emit(projection("1", state)); await settle();
  return s;
}
const sprintOf = (id: string, runMode: Sprint["runMode"], taskIds: string[]): Sprint =>
  ({ id, projectId: "p", title: `${id} sprint`, description: "", runMode, targetBranch: null, planning: "created", conversation: [],
    plan: null, materializedVersion: 1, taskIds, queueState: "running", pauseReason: null, planningTaskId: "plan" });
const switchView = (name: string) => [...document.querySelectorAll<HTMLButtonElement>(".sb-view-head .seg button")].find(b => b.textContent === name)!;
const active = () => document.querySelector<HTMLElement>(".sb-work");
const showActive = async () => { switchView("Active work").click(); await settle(); };
const sprintRow = () => document.querySelector<HTMLAnchorElement>(".sidebar .task-row:has(.sb-sprint-progress)")!;

it.each([
  { width: 260, locale: "en", done: "Done 1 of 14", working: "In progress: 0", waiting: "Waiting for you: 1", drafts: "Not started: 12", unverified: "Of Done, closed without verification: 1" },
  { width: 220, locale: "ru-RU", done: "Готово 1 из 14", working: "В работе: 0", waiting: "Ожидают вас: 1", drafts: "Ещё не начаты: 12", unverified: "Из готовых — закрыты без проверки: 1" },
])("counts the board's Closed as is card in Done at sidebar width $width", async ({ width, locale, done, working, waiting, drafts, unverified }) => {
  vi.spyOn(navigator, "language", "get").mockReturnValue(locale);
  localStorage.setItem("cw.ws.sidebarWidth", JSON.stringify(width));
  const s = world(), base = s.tasks[0];
  s.tasks = [
    { ...base, id: "done", title: "Finished member", status: "done", closedAs: "as_is", notVerifiedByCortex: true, sprintId: "sp" },
    { ...base, id: "paused", title: "Paused member", status: "paused", sprintId: "sp" },
    ...Array.from({ length: 12 }, (_, i): Task => ({ ...base, id: `draft-${i}`, title: `Draft ${i}`, status: "draft", sprintId: "sp" })),
  ];
  s.sprints = [sprintOf("sp", "manual", s.tasks.map(t => t.id))];
  await mount(s, `/m/${LOCAL_MACHINE_ID}`);
  const check = () => {
    const row = sprintRow();
    expect(row.querySelector(".sb-sprint-progress")?.textContent).toBe(done);
    expect(row.querySelector(".trunc")?.textContent).toBe(width === 220 ? "sp sprint" : "Sprint: sp sprint");
    expect(row.querySelector("[data-tooltip-cut]")).toBeNull();
    expect(row.getAttribute("data-tooltip")).toContain("sp sprint");
    for (const field of [done, working, waiting, drafts, unverified]) {
      expect(row.getAttribute("aria-label")).toContain(field);
      expect(row.getAttribute("data-tooltip")).toContain(field);
    }
  };
  check();
  await showActive();
  check();
});

it("discloses unknown, unverified, and external sprint statuses without counting a waiting task twice", async () => {
  const s = world(), base = s.tasks[0];
  const member = (id: string, status: Task["status"], patch: Partial<Task> = {}): Task => ({ ...base, id, title: id, status, sprintId: "sp", ...patch });
  s.tasks = [member("done", "done"), member("working", "working"), member("question", "working"),
    member("review", "review"), member("merge", "ready_to_merge"), member("draft", "draft"),
    member("outside", "done", { closedAs: "external" }), member("as-is", "done", { closedAs: "as_is", notVerifiedByCortex: true }),
    member("unverified", "closed_unverified", { closedAs: "unconfirmed" }),
    member("unknown", "working", { process: "unknown" })];
  s.sessions = [{ ...emptySession("question"), decisions: [{ id: "q", kind: "question", from: "coordinator", workerId: null,
    at: null, text: "Waiting for input", options: [], answer: null, withdrawn: false }] }];
  s.sprints = [sprintOf("sp", "manual", [...s.tasks.map(t => t.id), "unread"])];
  const port = await mount(s, `/m/${LOCAL_MACHINE_ID}`);
  expect(sprintRow().querySelector(".sb-sprint-progress")?.textContent).toBe("Done ≥3 of 11");
  for (const field of ["In progress: 1", "Waiting for you: 3", "Not started: 1", "Of Done, outside the app: 1",
    "Of Done, closed without verification: 1", "Closed, not verified: 1", "Status unavailable: 2"]) {
    expect(sprintRow().getAttribute("aria-label")).toContain(field);
  }
  port.emit(projection("2", { ...s, sprints: [{ ...s.sprints[0], taskIds: ["unread"] }] })); await settle();
  expect(sprintRow().querySelector(".sb-sprint-progress")?.textContent).toBe("Progress unavailable");
  expect(sprintRow().getAttribute("data-tooltip")).toContain("Status unavailable: 1");
  expect(sprintRow().getAttribute("data-tooltip")).not.toContain("In progress: 0");
});

it("distinguishes Hybrid automatic acceptance and active conflict resolution from owner action", async () => {
  const s = world(), base = s.tasks[0];
  s.tasks = [
    { ...base, id: "review", status: "review", sprintId: "sp" },
    { ...base, id: "merge", status: "ready_to_merge", sprintId: "sp" },
    { ...base, id: "resolving", status: "ready_to_merge", conflict: "resolving", sprintId: "sp" },
  ];
  const sprint = sprintOf("sp", "hybrid", s.tasks.map(t => t.id));
  s.sprints = [sprint];
  const port = await mount(s, `/m/${LOCAL_MACHINE_ID}`);
  expect(sprintRow().getAttribute("aria-label")).toContain("In progress: 2 · Waiting for you: 1");
  port.emit(projection("2", { ...s, sprints: [{ ...sprint, queueState: "paused", automaticQueue: {
    kind: "settlement_blocked", taskId: "review", code: "automatic_acceptance_blocked", reason: "Review required", needsOwner: true,
  } }] })); await settle();
  expect(sprintRow().getAttribute("aria-label")).toContain("In progress: 1 · Waiting for you: 2");
});

it.each(["hybrid", "automatic"] as const)("lists a running %s sprint and its tasks as an indented tree in Active work", async mode => {
  const s = world(), base = s.tasks[0];
  s.tasks = [
    { ...base, id: "plan", title: "Planning run", status: "closed" as never, sprintId: null },
    { ...base, id: "a", title: "First step", status: "working", sprintId: "sp" },
    { ...base, id: "b", title: "Second step", status: "draft", sprintId: "sp" },
  ];
  s.sprints = [sprintOf("sp", mode, ["a", "b"])];
  await mount(s, `/m/${LOCAL_MACHINE_ID}`);
  await showActive();
  const block = active()!;
  expect(block).not.toBeNull();
  expect(block.querySelector(".proj")?.textContent).toContain(s.projects[0].name);
  const rows = [...block.querySelectorAll<HTMLElement>(".task-row:not(.all)")];
  expect(rows.map(r => r.querySelector(".trunc")?.textContent)).toEqual(["Sprint: sp sprint", "First step"]);
  expect(block.querySelector(".proj")).toBeTruthy();
  expect(rows[1].classList.contains("in-sprint")).toBe(true);
  expect(rows[0].classList.contains("in-sprint")).toBe(false);
});

it("switches between exclusive views and remembers the selection across remounts", async () => {
  const s = world();
  s.tasks[0].status = "working";
  const path = `/m/${LOCAL_MACHINE_ID}`;
  await mount(s, path);
  expect(switchView("Projects").getAttribute("aria-pressed")).toBe("true");
  expect(active()).toBeNull();
  expect(document.querySelector(".sidebar .proj")?.textContent).toContain(s.projects[0].name);
  expect(document.querySelector(".sidebar .tasks .task-row")?.textContent).toContain(s.tasks[0].title);
  await showActive();
  expect(switchView("Active work").getAttribute("aria-pressed")).toBe("true");
  expect(document.querySelector(".sidebar .proj")).toBeTruthy();
  expect(active()?.textContent).toContain(s.tasks[0].title);
  dispose?.();
  document.body.innerHTML = "";
  await mount(s, path);
  expect(switchView("Active work").getAttribute("aria-pressed")).toBe("true");
  switchView("Projects").click(); await settle();
  expect(active()).toBeNull();
  expect(document.querySelector(".sidebar .tasks .task-row")?.textContent).toContain(s.tasks[0].title);
});

it("lists owner-action rows once across standalone work and sprint modes", async () => {
  const s = world(), base = s.tasks[0];
  const task = (id: string, patch: Partial<Task> = {}): Task => ({ ...base, id, title: id, status: "working", ...patch });
  const decision = (id: string, patch: Partial<Decision> = {}) => ({
    ...emptySession(id), decisions: [{ id: `decision-${id}`, kind: "question", from: "coordinator", workerId: null,
      at: null, text: "What next?", options: [], answer: null, withdrawn: false, ...patch } satisfies Decision],
  });
  s.tasks = [
    task("running-question", { sprintId: "running" }),
    task("idle-question", { sprintId: "idle" }),
    task("standalone-question"),
    task("permission-only"),
    task("continuation", { sprintId: "paused", status: "paused", automaticContinuation: {
      policy: "automatic_continuation", continuationCount: 3, lastContinuationReason: null,
      pause: { eventId: "cap", kind: "continuation_cap", reason: "continuation_cap" },
    } }),
    task("paused-by-you", { status: "paused" }),
    task("failed", { status: "failed" }),
    task("cancelled", { status: "cancelled", sprintId: "idle" }),
    task("archived", { archive: { at: 1, previousStatus: "working" } }),
    task("plan"),
    task("suspended", { status: "needs_you" }),
    task("unavailable", { status: "needs_you" }),
    task("withdrawn", { status: "needs_you" }),
    task("unconfirmed", { status: "needs_you", process: "unknown" }),
    task("working"),
    task("review", { status: "review" }),
    task("merge", { status: "ready_to_merge" }),
  ];
  s.sprints = [
    sprintOf("running", "manual", ["running-question"]),
    { ...sprintOf("idle", "hybrid", ["idle-question", "cancelled"]), queueState: "idle" },
    { ...sprintOf("paused", "automatic", ["continuation"]), queueState: "paused", automaticQueue: {
      kind: "continuation", taskId: "continuation", code: "continuation_cap", reason: "continuation_cap", needsOwner: true,
    } },
  ];
  s.sessions = [decision("running-question"), decision("idle-question"), decision("standalone-question"),
    decision("permission-only", { kind: "permission" }), decision("archived"), decision("plan"),
    decision("suspended", { suspended: true }), decision("unavailable", { kind: "permission", unavailable: "Stale" }),
    decision("withdrawn", { withdrawn: true }), decision("unconfirmed")];
  const port = await mount(s, `/m/${LOCAL_MACHINE_ID}`);
  await showActive();
  const shown = () => [...active()!.querySelectorAll<HTMLAnchorElement>('.task-row[href*="/t/"]')].map(a => a.href.split("/").pop()).sort();
  const expected = ["running-question", "idle-question", "standalone-question", "permission-only", "continuation", "paused-by-you", "working", "review", "merge"].sort();
  expect(shown()).toEqual(expected);
  expect(active()!.querySelector('[href$="/t/idle-question"]')?.classList.contains("in-sprint")).toBe(true);
  // Once a permission is answered, the still-working task moves back to Working, not both groups.
  port.emit(projection("2", { ...s, sessions: s.sessions.map(session => session.taskId !== "permission-only" ? session : {
    ...session, decisions: session.decisions.map(d => ({ ...d, answer: { text: "Allowed", at: 1 } })),
  }) })); await settle();
  expect(shown()).toEqual(expected);
});

it("filters the phone drawer tree and removes its last project when work ends", async () => {
  const s = world();
  s.tasks[0].status = "stopping";
  s.sprints = [sprintOf("current", "manual", ["t"]), sprintOf("empty", "manual", [])];
  const phone: PhoneFrame = { kind: "phone", machine: () => ({ name: "Desk", state: "connected", path: "direct", lastConnectedAt: 1 }), openMachines() {}, openSettings() {}, retry() {}, attach() {} };
  const port = await mount(s, `/m/${LOCAL_MACHINE_ID}`, phone);
  document.querySelector<HTMLButtonElement>('.ws-phonebar [aria-label="Menu"]')!.click();
  await showActive();
  const drawer = document.querySelector(".ws-drawer")!;
  expect(drawer.querySelector('[aria-label="Sort projects"]')).toBeTruthy();
  expect(drawer.querySelector('[aria-label="Actions for sprint current sprint"]')).toBeTruthy();
  expect(drawer.querySelector('[aria-label="Actions for sprint empty sprint"]')).toBeNull();
  expect(drawer.querySelector(".in-sprint .sig")?.textContent).toBe("Stopping");
  port.emit(projection("2", { ...s, tasks: [{ ...s.tasks[0], status: "done" }] })); await settle();
  expect(active()!.querySelector(".proj")).toBeNull();
  switchView("Projects").click(); await settle();
  expect(drawer.querySelector(".proj")).toBeTruthy();
});

it("expands and folds a project's tree from its chevron, and remembers it", async () => {
  const s = world(), base = s.tasks[0];
  s.tasks = [{ ...base, id: "a", title: "First step", status: "working", sprintId: "sp" }];
  s.sprints = [sprintOf("sp", "hybrid", ["a"])];
  await mount(s, `/m/${LOCAL_MACHINE_ID}`);
  const tree = () => document.querySelector(".sidebar .proj-wrap + .tasks");
  const toggle = () => document.querySelector<HTMLButtonElement>(".sidebar .proj-x")!;
  expect(toggle().getAttribute("aria-expanded")).toBe("true");
  expect(tree()?.querySelectorAll(".task-row:not(.all)").length).toBe(2);
  toggle().click(); await settle();
  expect(toggle().getAttribute("aria-expanded")).toBe("false");
  expect(tree()).toBeNull();
  toggle().click(); await settle();
  expect(toggle().getAttribute("aria-expanded")).toBe("true");
  expect(tree()?.querySelectorAll(".task-row:not(.all)").length).toBe(2);
});

it("opens a collapsed project tree when its name is clicked as well as navigating", async () => {
  const s = world(), base = s.tasks[0];
  s.tasks = [{ ...base, id: "a", title: "First step", status: "working" }];
  localStorage.setItem("cw.ws.expanded", JSON.stringify({ p: false }));
  await mount(s, `/m/${LOCAL_MACHINE_ID}`);
  const tree = () => document.querySelector(".sidebar .proj-wrap + .tasks");
  expect(tree()).toBeNull();
  document.querySelector<HTMLAnchorElement>(".sidebar .proj")!.click(); await settle();
  expect(tree()).not.toBeNull();
});
