import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it } from "vitest";
import { emptyWorkspace, mapProjectSlice, type QueryResults, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { buildIndex } from "../src/selectors";
import { closeBlocked, sessionModel } from "../src/session-model";
import { flush, projection, scriptedPort, wireTask, withTask } from "./support";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
function world(phase: "unknown" | "execution" | "paused" = "unknown"): WorkspaceState {
  const s = withTask(emptyWorkspace(), "Retained task");
  s.machines[0].connection = "connected";
  const slice = mapProjectSlice(s.projects[0], [wireTask({
    taskId: "t", description: "Retained task", title: "Retained task", titleAvailability: "generated", executionId: "e", phase,
    runtimeState: phase === "execution" ? "EXECUTING" : phase === "paused" ? "PAUSED" : "RECOVERING",
    warnings: phase === "unknown" ? ["contradictory_evidence"] : [],
  })]);
  return { ...s, tasks: slice.tasks, sessions: slice.sessions };
}
const model = (s: WorkspaceState) => sessionModel(buildIndex(s, 0), "t")!;
async function mount(path = "/m/local/t/t") {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  let answer = world(), fail = false, reads = 0;
  if (path.includes("/workers")) answer.workers = [{ id: "w", taskId: "t", executionId: "e", role: "debugger", status: "working", lane: "execution", goal: "Inspect the defect", now: null, waitsFor: null, scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, result: null, activity: [] }];
  p.port.query = async operation => {
    if (operation === "project.snapshot") { reads++; if (fail) throw new Error("unavailable"); return [projection("1", answer)] as never; }
    return ({ "capabilities.get": { everything: true, operations: [] }, "project.facts": [], "machine.facts": [] } as Partial<QueryResults>)[operation] as never;
  };
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle();
  return { ...p, reads: () => reads, answer: (s: WorkspaceState) => { answer = s; }, fail: () => { fail = true; } };
}
const strip = () => document.querySelector(".l-task .l-strip")!;
const recheck = () => [...strip().querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent === "Re-check state")!;

it("distinguishes an unconfirmed task from a disconnected machine without promoting its state", () => {
  const s = world();
  expect(model(s).view.unknown).toBe(true);
  expect(model(s).view.task.process).toBe("unknown");
  expect(model(s).mode).toBe("unconfirmed");
  expect(closeBlocked(model(s))).toContain("state is confirmed");
  s.machines[0].connection = "disconnected";
  expect(model(s).mode).toBe("offline");
  expect(model(world("paused")).mode).toBe("paused");
  const stopping = world(); stopping.tasks[0].status = "stopping";
  expect(model(stopping).mode).toBe("stopping");
});

it("shows one strip status with one icon, a readable cause and a read-only way out", async () => {
  const p = await mount();
  expect(strip().querySelectorAll(".i")).toHaveLength(1);
  expect(strip().querySelectorAll(".i-help")).toHaveLength(0);
  const state = [...document.querySelectorAll(".ins-sec")].find(s => s.querySelector(".ins-h")?.textContent === "State")!;
  expect(state.querySelectorAll(".i")).toHaveLength(1);
  expect(state.textContent).not.toContain("Process state unknown");
  expect(strip().textContent?.match(/Task state not confirmed/g)).toHaveLength(1);
  expect(strip().textContent).toContain("machine is connected");
  expect(strip().textContent).toContain("running or stopped");
  expect(strip().textContent).toContain("Diagnostics");
  expect(recheck()).toBeTruthy();
  const input = document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message"]')!;
  expect(input.disabled).toBe(true);
  expect(input.placeholder).toContain("Re-check state above");
  expect(input.placeholder).not.toContain("reconnect");
  expect(p.commands).toHaveLength(0);
});

it("re-check installs even an equal-version confirmed snapshot and restores controls", async () => {
  const p = await mount(), before = p.reads();
  p.answer(world("execution"));
  recheck().click(); await settle();
  expect(p.reads()).toBeGreaterThan(before);
  expect(document.querySelector(".l-task .l-strip")).toBeNull();
  expect(document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message"]')!.disabled).toBe(false);
  expect(p.commands).toHaveLength(0); // A read, never Resume or Stop.
});

it("a repeated unknown snapshot stays blocked and explains the next diagnostic step", async () => {
  const p = await mount();
  recheck().click(); await settle();
  expect(strip().textContent).toContain("Still not confirmed");
  expect(strip().querySelector('a[href="/settings/machine/local/diagnostics"]')).toBeTruthy();
  expect(document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message"]')!.disabled).toBe(true);
  expect(p.commands).toHaveLength(0);
});

it("a worker composer explains task confirmation, not a nonexistent connection loss", async () => {
  await mount("/m/local/t/t/workers/w");
  const input = document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message debugger"]')!;
  expect(input.disabled).toBe(true);
  expect(input.placeholder).toContain("Re-check state above");
  expect(input.placeholder).not.toMatch(/offline|Reconnect/);
});

it("repeated presses join the pending read without sending a task control", async () => {
  const p = await mount(), query = p.port.query;
  let finish!: (snapshots: QueryResults["project.snapshot"]) => void, pending = 0;
  p.port.query = operation => {
    if (operation !== "project.snapshot") return query(operation, {}) as never;
    pending++;
    return new Promise(resolve => { finish = resolve as typeof finish; }) as never;
  };
  recheck().click(); await settle();
  expect(recheck().getAttribute("aria-busy")).toBe("true");
  expect(strip().textContent).toContain("Re-checking");
  recheck().click(); await settle();
  expect(pending).toBe(1);
  finish([projection("1", world())]); await settle();
  expect(recheck().getAttribute("aria-busy")).not.toBe("true");
  expect(p.commands).toHaveLength(0);
});

it("a failed re-check retains the task and an honest retry explanation", async () => {
  const p = await mount(); p.fail();
  recheck().click(); await settle();
  expect(strip().textContent).toContain("could not read");
  expect(recheck().getAttribute("aria-busy")).not.toBe("true");
  expect(document.body.textContent).toContain("Retained task");
  expect(p.commands).toHaveLength(0);
});
