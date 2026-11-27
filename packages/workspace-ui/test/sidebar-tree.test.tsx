import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type Decision, type Sprint, type Task } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const world = () => { const s = withTask(emptyWorkspace(), "Work"); s.machines[0].connection = "connected"; return s; };
async function mount(state: ReturnType<typeof world>, path: string) {
  const s = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={s.port} />, root);
  await settle(); s.emit(projection("1", state)); await settle();
  return s;
}
const sprintOf = (id: string, runMode: Sprint["runMode"], taskIds: string[]): Sprint =>
  ({ id, projectId: "p", title: `${id} sprint`, description: "", runMode, targetBranch: null, planning: "created", conversation: [],
    plan: null, materializedVersion: 1, taskIds, queueState: "running", pauseReason: null, planningTaskId: "plan" });
const switchView = (name: string) => [...document.querySelectorAll<HTMLButtonElement>(".sb-view-head .seg button")].find(b => b.textContent === name)!;
const active = () => document.querySelector<HTMLElement>(".sb-work");
const showActive = async () => { switchView("Active work").click(); await settle(); };

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
  expect(block.querySelector(".sb-work-group")?.textContent).toBe("Sprints in progress");
  const rows = [...block.querySelectorAll<HTMLElement>(".task-row")];
  expect(rows.map(r => r.querySelector(".trunc")?.textContent)).toEqual(["Sprint: sp sprint", "First step"]);
  expect(rows[0].querySelector(".sb-row-project")?.textContent).toBe(s.projects[0].name);
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
  expect(document.querySelector(".sidebar .proj")).toBeNull();
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
  const groups = () => {
    const result: Record<string, string[]> = {};
    let current: string[] = [];
    for (const node of active()!.querySelectorAll(".sb-work-group, .task-row")) {
      if (node.matches(".sb-work-group")) current = result[node.textContent!] = [];
      else current.push(node.querySelector(".trunc")!.textContent!);
    }
    return result;
  };
  expect(Object.entries(groups())).toEqual([
    ["Needs you", ["idle-question", "standalone-question", "permission-only", "paused-by-you", "failed", "cancelled"]],
    ["Working", ["working"]], ["Review", ["review"]], ["Ready to merge", ["merge"]],
    ["Sprints in progress", ["Sprint: running sprint", "running-question", "Sprint: paused sprint", "continuation"]],
  ]);
  const row = (id: string) => active()!.querySelector(`.task-row[href$="/t/${id}"]`)!;
  for (const id of ["running-question", "continuation"]) {
    expect(row(id).classList.contains("in-sprint")).toBe(true);
    expect(row(id).querySelector(".sig")?.textContent).toBe("Needs you");
  }
  for (const id of ["idle-question", "standalone-question", "permission-only"]) {
    expect(row(id).classList.contains("in-sprint")).toBe(false);
    expect(row(id).querySelector(".sb-row-project")?.textContent).toBe(s.projects[0].name);
    expect(row(id).getAttribute("aria-label")).toContain("Needs you");
  }
  // Once a permission is answered, the still-working task moves back to Working, not both groups.
  port.emit(projection("2", { ...s, sessions: s.sessions.map(session => session.taskId !== "permission-only" ? session : {
    ...session, decisions: session.decisions.map(d => ({ ...d, answer: { text: "Allowed", at: 1 } })),
  }) })); await settle();
  expect(groups()["Needs you"]).not.toContain("permission-only");
  expect(groups()["Working"]).toEqual(["permission-only", "working"]);
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
