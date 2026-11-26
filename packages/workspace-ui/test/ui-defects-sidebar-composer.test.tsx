import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type Sprint, type Task, type WorkspaceCommand, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { draftTask, flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; localStorage.clear(); sessionStorage.clear(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
async function mount(state: WorkspaceState, path: string) {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", state)); await settle();
  return Object.assign(p, { push: async (next: WorkspaceState) => { p.emit(projection("2", next)); await settle(); } });
}

const member = (id: string, title: string, patch: Partial<Task> = {}): Task => ({ ...draftTask(), id, title, status: "working", sprintId: "sp", ...patch } as Task);
const sprint = (taskIds: string[]): Sprint => ({ id: "sp", projectId: "p", description: "Launch", title: "Launch", runMode: "manual", targetBranch: null, planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds, queueState: "running", pauseReason: null } as Sprint);

it("a running sprint's sidebar tree lists only its active tasks, before and after the sprint is opened", async () => {
  const s = withTask(emptyWorkspace(), "unused");
  s.machines[0].connection = "connected";
  const ids = Array.from({ length: 8 }, (_, i) => `d${i}`);
  const at = (status: Task["status"]): WorkspaceState => ({ ...s, tasks: [member("w", "Active one"), ...ids.map((id, i) => member(id, `Finished ${i}`, status === "done" ? { status, doneAt: 1, executionId: `e${i}` } : { status }))], sprints: [sprint(["w", ...ids])], sessions: [] });
  const app = await mount(at("working"), `/m/${LOCAL_MACHINE_ID}`);
  await app.push(at("done"));
  const inProgress = () => {
    const out: string[] = [];
    for (const row of all(".sidebar .task-row")) out.push(row.querySelector(".trunc")?.textContent ?? "");
    return out.filter(t => t.startsWith("Finished"));
  };
  expect(inProgress()).toEqual([]);
  // Open the sprint: its facts are loaded and the members are read in full.
  all<HTMLElement>(".sidebar .task-row, .sidebar .nav").find(r => r.textContent?.includes("Launch"))?.click(); await settle();
  expect(inProgress()).toEqual([]);
  expect(all(".sidebar .task-row").some(r => r.textContent?.includes("Active one"))).toBe(true);
});

const pausedByAutomatic = (): Partial<Task> => ({ status: "paused", process: "stopped", executionId: "e",
  automaticContinuation: { policy: "automatic_continuation", continuationCount: 1, lastContinuationReason: null, pause: { eventId: "ev1", kind: "continuation_cap", reason: "The continuation limit was reached." } } } as Partial<Task>);
function task(patch: Partial<Task>) {
  const state = withTask(emptyWorkspace(), "Fix token refresh race");
  Object.assign(state.tasks[0], patch);
  state.sessions = [emptySession("t")];
  state.machines[0].connection = "connected";
  return state;
}
const action = () => $<HTMLButtonElement>(".dock .mc-act, .dock [aria-label='Resume'], .dock [aria-label='Send message']")!;

it("a task held by Automatic offers no plain Resume the service would refuse", async () => {
  const p = await mount(task(pausedByAutomatic()), `/m/${LOCAL_MACHINE_ID}/t/t`);
  expect(all("button").some(b => b.getAttribute("aria-label") === "Resume" && !!b.closest(".dock"))).toBe(false);
  expect($(".dock .mc-effect")?.textContent ?? "").not.toMatch(/Resume continues the task/);
  expect(p.commands.map(c => c.payload.type)).not.toContain("continue_task");
});

it("a task the owner paused still resumes from the composer", async () => {
  const p = await mount(task({ status: "paused", process: "stopped", executionId: "e" }), `/m/${LOCAL_MACHINE_ID}/t/t`);
  const resume = all<HTMLButtonElement>(".dock button").find(b => b.getAttribute("aria-label") === "Resume")!;
  resume.click(); await settle();
  expect(p.commands.map(c => (c.payload as WorkspaceCommand).type)).toEqual(["continue_task"]);
});

it("the verification card stacks its status under a one-line title with the full text as a tooltip", async () => {
  await mount(task({ status: "working", process: "live", executionId: "e" }), `/m/${LOCAL_MACHINE_ID}/t/t/workers`);
  const card = all(".wcard").find(c => c.textContent?.includes("Runtime verification"));
  expect(card).toBeTruthy();
  const l1 = card!.querySelector(".l1")!;
  expect(l1.querySelector(".st")).toBeNull();
  const status = card!.querySelector<HTMLElement>(".ws-vstatus")!;
  // The status wraps on its own line instead of being cut, so it needs no hover title (none are used on screens).
  expect(status.getAttribute("title")).toBeNull();
  expect(status.textContent).not.toBe("");
});

it("a refusal that the task is no longer paused is followed at once by a fresh read of the project's facts", async () => {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  const reads: string[] = [];
  const query = p.port.query.bind(p.port);
  p.port.query = ((operation: string, parameters: unknown) => { reads.push(operation); return (query as (o: string, q: unknown) => Promise<unknown>)(operation, parameters); }) as typeof p.port.query;
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", task({ status: "paused", process: "stopped", executionId: "e" }))); await settle();
  all<HTMLButtonElement>(".dock button").find(b => b.getAttribute("aria-label") === "Resume")!.click(); await settle();
  await settle();
  const before = reads.filter(r => r === "project.facts").length;
  p.receipt(false, undefined, "task_not_paused", "This task is no longer paused. Refresh its current state."); await settle();
  expect(reads.filter(r => r === "project.facts").length).toBeGreaterThan(before);
});
