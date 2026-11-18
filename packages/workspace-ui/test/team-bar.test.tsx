import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type Worker, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

// The task's team stands in one bar pinned above the composer, outside the scrolling feed.
let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; localStorage.clear(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const worker = (patch: Partial<Worker> = {}): Worker => ({ id: "w", taskId: "t", executionId: "e", role: "frontend-dev", status: "working", lane: "execution", goal: "Build the page", now: null, waitsFor: null, scope: [],
  route: { model: "opus-5.5", effort: "high" }, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, activity: [], result: null, ...patch } as Worker);
function world(workers: Worker[]): WorkspaceState {
  const state = withTask(emptyWorkspace(), "Team task");
  state.machines[0].connection = "connected";
  state.tasks[0].status = "working"; state.tasks[0].process = "live";
  state.workers = workers;
  const session = emptySession("t");
  session.timeline = [{ id: "c1", kind: "cortex", text: "Planning." }, ...(workers.length ? [{ id: "facts:workers", kind: "workers" as const, title: "Workers", workerIds: workers.map(w => w.id) }] : [])];
  state.sessions = [session];
  return state;
}
async function mount(state: WorkspaceState) {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", state)); await settle();
  return p;
}
const bar = () => document.querySelector<HTMLElement>(".ws-team");
const menu = () => document.getElementById(document.querySelector(".ws-team-all")!.getAttribute("popovertarget")!)!;

it("shows no team bar and no workers block in the feed while a task has no workers", async () => {
  await mount(world([]));
  expect(document.querySelector(".timeline .ai")).not.toBeNull();
  expect(bar()).toBeNull();
  expect(document.querySelector(".timeline .wrow, .timeline .ws-team")).toBeNull();
});

it("pins one worker's bar above the composer, outside the feed's scroller, naming who works now", async () => {
  await mount(world([worker({ now: "Editing the header" })]));
  const b = bar()!;
  expect(b).not.toBeNull();
  // Not part of the feed: the scroller holds no part of it, and it stands in the dock just above the composer.
  expect(document.querySelector(".timeline")!.contains(b)).toBe(false);
  expect(b.closest(".task-panel > .dock")).not.toBeNull();
  // What follows it in the dock's flow (its menu and stop dialog live in the top layer) is the composer.
  const after = [...b.parentElement!.children].slice([...b.parentElement!.children].indexOf(b) + 1).find(e => !e.matches("[popover], dialog"));
  expect(after?.matches(".composer, .mc-fold")).toBe(true);
  const lead = b.querySelector(".ws-team-lead")!;
  expect(lead.querySelector(".ws-team-role")?.textContent).toBe("frontend-dev");
  expect(lead.querySelector(".ws-team-now")?.textContent).toBe("Editing the header");
  expect(lead.querySelector(".i.spin")).not.toBeNull();
  expect(b.querySelector(".ws-team-all")?.textContent).toBe("Workers: 1 of 1 working");
  // The feed holds no workers block, and its rows are in the menu.
  expect(document.querySelector(".timeline .wrow")).toBeNull();
  expect(menu().querySelectorAll(".wrow")).toHaveLength(1);
});

it("with many workers names the newest one at work first and lists every worker in a top-layer menu", async () => {
  const p = await mount(world([
    worker({ id: "a", role: "architect", status: "completed", result: { summary: "Planned", checks: [] } as never }),
    worker({ id: "b", role: "backend-dev", status: "working", now: "Writing the API" }),
    worker({ id: "c", role: "frontend-dev", status: "working" }),
    worker({ id: "d", role: "code-reviewer", status: "pending" }),
  ]));
  const b = bar()!;
  // The newest worker at work leads: workers read newest first everywhere.
  expect(b.querySelector(".ws-team-role")?.textContent).toBe("frontend-dev");
  expect(b.querySelector(".ws-team-more")?.textContent).toBe("+1 working");
  expect(b.querySelector(".ws-team-all")?.textContent).toMatch(/^Workers: 3 of 4 working/);
  const pop = menu();
  // A native popover: shown in the top layer, outside the feed, so opening it inserts nothing into the feed or the dock.
  expect(pop.getAttribute("popover")).toBe("auto");
  expect(document.querySelector(".timeline")!.contains(pop)).toBe(false);
  expect(b.contains(pop)).toBe(false);
  expect([...pop.querySelectorAll(".wrow .role")].map(r => r.textContent)).toEqual(["code-reviewer", "frontend-dev", "backend-dev", "architect"]);
  const feed = document.querySelector(".timeline .tl-in")!, before = [...feed.querySelectorAll("*")];
  (b.querySelector(".ws-team-all") as HTMLButtonElement).click(); await settle();
  expect([...feed.querySelectorAll("*")]).toEqual(before);
  expect(b.isConnected).toBe(true);
  // A background read with changed facts updates the bar in place: the same elements, the new words.
  const lead = b.querySelector(".ws-team-lead")!, row = pop.querySelector(".wrow")!;
  const next = world([
    worker({ id: "a", role: "architect", status: "completed", result: { summary: "Planned", checks: [] } as never }),
    worker({ id: "b", role: "backend-dev", status: "completed", result: { summary: "API done", checks: [] } as never }),
    worker({ id: "c", role: "frontend-dev", status: "working", now: "Styling the form" }),
    worker({ id: "d", role: "code-reviewer", status: "pending" }),
  ]);
  p.emit(projection("2", next)); await settle();
  expect(bar()).toBe(b);
  expect(b.querySelector(".ws-team-lead")).toBe(lead);
  expect(pop.querySelector(".wrow")).toBe(row);
  expect(b.querySelector(".ws-team-role")?.textContent).toBe("frontend-dev");
  expect(b.querySelector(".ws-team-now")?.textContent).toBe("Styling the form");
  expect(b.querySelector(".ws-team-more")).toBeNull();
});

it("opens a worker's activity from the bar", async () => {
  await mount(world([worker({ id: "b", role: "backend-dev" })]));
  (bar()!.querySelector(".ws-team-lead") as HTMLButtonElement).click(); await settle();
  expect(location.pathname).toBe(`/m/${LOCAL_MACHINE_ID}/t/t/workers/b`);
});
