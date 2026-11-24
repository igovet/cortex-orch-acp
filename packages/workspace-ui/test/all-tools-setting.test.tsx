import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type Activity, type Worker, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { conversationOf } from "../src/port/history";
import { STEP_PREFIX } from "../src/tool-names";
import { flush, projection, scriptedPort, withTask } from "./support";

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const step = (d: Record<string, string>) => STEP_PREFIX + JSON.stringify(d);
const row = (i: number, text: string) => ({ historyId: `h${i}`, revision: "1", kind: "tool", text, toolStatus: "completed", recordedAt: `2026-11-17T02:12:${String(i).padStart(2, "0")}Z` });
const call = (i: number, name: string, extra: Record<string, string> = {}) => row(i, step({ title: `orchestrator: mcp__cortex__${name}`, callId: `xa:${i}`, kind: "other", status: "completed", ...extra }));
const known = { description: "", answers: [] };
const taskPath = `/m/${LOCAL_MACHINE_ID}/t/t`;

it("keeps the runtime's own steps beside the simplified turn, so the reader can ask for every tool call", () => {
  const [mixed] = conversationOf([call(1, "project_read", { path: "src/a.ts" }), call(2, "read_skill", { ref: "protocol" }), call(3, "plan_submit")], known);
  if (mixed.entry.kind !== "fold") throw new Error("not a fold");
  expect(mixed.entry.text).toMatch(/read 1 file$/i);
  expect(mixed.entry.steps).toHaveLength(1);
  expect(mixed.entry.all?.text).toMatch(/read 1 file and used 2 other tools$/i);
  expect(mixed.entry.all?.steps).toHaveLength(3);
  // A turn of only plumbing is an empty fold, with its full reading kept.
  const [only] = conversationOf([call(1, "read_skill", { ref: "protocol" })], known);
  if (only.entry.kind !== "fold") throw new Error("not a fold");
  expect(only.entry.text).toBe("");
  expect(only.entry.all?.text).toMatch(/used 1 other tool$/i);
  // A turn without plumbing has nothing to add.
  const [plain] = conversationOf([call(1, "project_read", { path: "src/a.ts" })], known);
  if (plain.entry.kind !== "fold") throw new Error("not a fold");
  expect(plain.entry.all).toBeUndefined();
});

function world(): WorkspaceState {
  const state = withTask(emptyWorkspace(), "Work");
  state.machines[0].connection = "connected"; state.tasks[0].status = "working"; state.tasks[0].process = "live"; state.tasks[0].executionId = "e";
  const rows = conversationOf([call(1, "project_read", { path: "src/a.ts" }), call(2, "read_skill", { ref: "protocol" })], known);
  const session = emptySession("t");
  session.timeline = rows.map(r => r.entry);
  state.sessions = [session];
  const activity: Activity[] = [
    { sequence: 1, kind: "tool", text: "mcp__cortex__project_read", args: "src/a.ts", at: 1 },
    { sequence: 2, kind: "command", text: "mcp.cortex.read_skill", code: "mcp.cortex.read_skill", at: 2, command: { state: "native", exit: null, took: null, tail: "", reason: null } },
  ];
  const worker: Worker = { id: "w", taskId: "t", executionId: "e", role: "ux-designer", status: "working", lane: "execution", goal: "Design", now: null, waitsFor: null, scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, activity, result: null };
  state.workers = [worker];
  return state;
}
const go = async (path: string) => { history.pushState(null, "", path); window.dispatchEvent(new PopStateEvent("popstate")); await settle(); };
const open = async (selector: string) => { document.querySelector<HTMLButtonElement>(`${selector} button.fold`)?.click(); await settle(); };

it("shows or hides the runtime's own steps at once, in the conversation and on a worker page, and remembers the choice", async () => {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", "/settings/app/appearance");
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", world())); await settle();
  const toggle = () => document.querySelector<HTMLInputElement>('input[aria-label="Show every tool call"]')!;
  expect(toggle().checked).toBe(false);

  await go(taskPath);
  await open(".timeline");
  expect(document.querySelector(".timeline")!.textContent).not.toContain("Read skill");
  await go(`${taskPath}/workers/w`);
  await open(".feed, .stream");
  expect(document.querySelector(".feed, .stream")!.textContent).not.toContain("Read skill");

  await go("/settings/app/appearance");
  toggle().click(); await settle();
  expect(toggle().checked).toBe(true);
  expect(localStorage.getItem("cw.ws.allTools")).toBe("true");

  await go(taskPath);
  await open(".timeline");
  expect(document.querySelector(".timeline")!.textContent).toContain("Read skill");
  await go(`${taskPath}/workers/w`);
  await open(".feed, .stream");
  expect(document.querySelector(".feed, .stream")!.textContent).toContain("Read skill");
  const skill = [...document.querySelectorAll<HTMLButtonElement>(".stream button.step-row")].find(row => row.textContent?.includes("Read skill"))!;
  skill.click(); await settle();
  expect(skill.getAttribute("aria-expanded")).toBe("true");
  expect(skill.parentElement?.textContent).toContain("No output was recorded.");

  await go("/settings/app/appearance");
  toggle().click(); await settle();
  await go(taskPath);
  expect(document.querySelector(".timeline")!.textContent).not.toContain("Read skill");
  expect(localStorage.getItem("cw.ws.allTools")).toBe("false");
});

it("starts with the stored choice", async () => {
  localStorage.setItem("cw.ws.allTools", "true");
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", taskPath);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", world())); await settle();
  await open(".timeline");
  expect(document.querySelector(".timeline")!.textContent).toContain("Read skill");
});
