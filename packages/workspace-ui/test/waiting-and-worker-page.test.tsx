import { createRoot, createSignal } from "solid-js";
import { render } from "solid-js/web";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type Decision, type Task, type TimelineEntry, type Worker, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createFollow } from "../src/follow";
import { flush, projection, scriptedPort, withTask } from "./support";

// Seen in a paid live run of the dev build: a "Working…" spinner under a question the task was waiting on, and a worker
// page whose long goal left the activity a sliver, where opening a row scrolled it away.
let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; localStorage.clear(); vi.unstubAllGlobals(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };

const worker = (patch: Partial<Worker> = {}): Worker => ({ id: "w", taskId: "t", executionId: "e", role: "backend-dev", status: "working", lane: "execution", goal: "Add formatAll",
  now: null, waitsFor: null, scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, activity: [], result: null, ...patch } as Worker);
const question: Decision = { id: "d1", kind: "question", from: "Coordinator", workerId: null, at: null, text: "How to continue after the stop?",
  options: [{ label: "Start again", optionId: "o1" }, { label: "Finish as is", optionId: "o2" }], answer: null, withdrawn: false };

function world(patch: Partial<Task>, workers: Worker[], decisions: Decision[], entries: TimelineEntry[] = []): WorkspaceState {
  const state = withTask(emptyWorkspace(), "Library");
  Object.assign(state.tasks[0], { status: "working", process: "live", executionId: "e" }, patch);
  state.machines[0].connection = "connected";
  const session = emptySession("t");
  session.timeline = entries; session.decisions = decisions;
  state.sessions = [session]; state.workers = workers;
  return state;
}
async function mount(state: WorkspaceState, path = `/m/${LOCAL_MACHINE_ID}/t/t`) {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", state)); await settle();
  return p;
}

it("says the task waits for the owner, not that it is working, while its question is open and nothing runs", async () => {
  const stopped = worker({ status: "cancelled" });
  const p = await mount(world({ status: "needs_you" }, [stopped], [question], [{ id: "e-d1", kind: "decision", decisionId: "d1" }]));
  const line = document.querySelector(".timeline [role=status]");
  expect(line?.textContent).toBe("Waiting for you");
  expect(line?.querySelector(".spin")).toBeNull();
  expect(document.querySelector(".timeline")!.textContent).not.toContain("Working…");
  // The header says the same thing.
  expect(document.body.textContent).toContain("Needs you");
  // A sibling still running is work going on: the line has a spinner again, and speaking for the coordinator it waits for that worker.
  const next = world({ status: "needs_you" }, [stopped, worker({ id: "w2", role: "frontend-dev" })], [question], [{ id: "e-d1", kind: "decision", decisionId: "d1" }]);
  p.emit(projection("2", next)); await settle();
  expect(document.querySelector(".timeline [role=status]")?.textContent).toBe("Waiting for frontend-dev…");
  expect(document.querySelector(".timeline [role=status] .spin")).not.toBeNull();
  // Still working once the answer is in, while the worker runs.
  p.emit(projection("3", world({ status: "working" }, [worker({ id: "w2" })], []))); await settle();
  expect(document.querySelector(".timeline [role=status]")?.textContent).toBe("Waiting for backend-dev…");
  p.emit(projection("4", world({ status: "working" }, [], []))); await settle();
  expect(document.querySelector(".timeline [role=status]")?.textContent).toBe("Working…");
});

it("explains a wait while decision details are unavailable", async () => {
  await mount(world({ status: "needs_you", issue: null }, [], []));
  expect(document.querySelector("#decision-issue")?.textContent).toContain("Use Answer in the task header");
});

it("renders an available question even before its timeline reference arrives", async () => {
  await mount(world({ status: "needs_you" }, [], [question]));
  expect(document.querySelector(".timeline")?.textContent).toContain(question.text);
  expect(document.querySelector(".timeline")?.textContent).toContain("Start again");
});

it("shows sent attachment chips when runtime history trims the message", async () => {
  const state = world({ status: "working" }, [], [], [{id:"h",kind:"said",text:"Look here"}]);
  state.messages = [{id:"sent",taskId:"t",workerId:null,text:" Look here\n",mode:"send_now",delivery:"recorded",at:1,deliveredAt:1,failure:null,attachments:["clipboard.png"]}];
  await mount(state);
  expect(document.querySelector(".timeline .mine")?.textContent).toContain("clipboard.png");
});

const LONG_GOAL = ["Project: tally library (plain Node.js, CommonJS, no dependencies).", "Goal: add formatAll(registry) to src/format.js.", "Requirements:",
  ...Array.from({ length: 12 }, (_, i) => `${i + 1}. Requirement number ${i + 1} with enough words to take a full line on the worker page.`)].join("\n");

it("folds a long worker goal to its first lines, with a way to read all of it", async () => {
  await mount(world({}, [worker({ goal: LONG_GOAL })], []), `/m/${LOCAL_MACHINE_ID}/t/t/workers/w`);
  const goal = document.querySelector<HTMLElement>(".livepane .d-goal")!;
  expect(goal.classList.contains("ws-goal-folded")).toBe(true);
  const toggle = document.querySelector<HTMLButtonElement>(".livepane .ws-goal-toggle")!;
  expect(toggle.textContent).toBe("Show the whole goal");
  expect(toggle.getAttribute("aria-expanded")).toBe("false");
  expect(toggle.getAttribute("aria-controls")).toBe(goal.id);
  toggle.click(); await settle();
  expect(goal.classList.contains("ws-goal-open")).toBe(true);
  expect(toggle.getAttribute("aria-expanded")).toBe("true");
  // The whole goal is there either way; folding only limits the height (workspace.css).
  expect(goal.textContent).toContain("Requirement number 12");
  const src = ["src", join("packages", "workspace-ui", "src")].find(dir => existsSync(join(dir, "workspace.css")))!;
  const css = readFileSync(join(src, "workspace.css"), "utf8");
  expect(css).toMatch(/\.d-goal\.ws-goal-folded \{[^}]*max-height: 72px/);
  expect(css).toMatch(/\.d-goal\.ws-goal-open \{[^}]*max-height: 40vh[^}]*overflow-y: auto/);
});

it("leaves a short worker goal as it is", async () => {
  await mount(world({}, [worker()], []), `/m/${LOCAL_MACHINE_ID}/t/t/workers/w`);
  expect(document.querySelector(".livepane .d-goal")!.className).toBe("d-goal");
  expect(document.querySelector(".livepane .ws-goal-toggle")).toBeNull();
});

it("stops following when the reader opens a block on a worker's page, so the end does not pull it out of view", async () => {
  const read = { sequence: 1, kind: "tool" as const, text: "Read", at: null, args: JSON.stringify({ title: "Read", kind: "read", path: "src/format.js", output: "module.exports = {}" }) };
  await mount(world({}, [worker({ activity: [read] })], []), `/m/${LOCAL_MACHINE_ID}/t/t/workers/w`);
  const following = () => document.querySelector<HTMLInputElement>('.livepane input[aria-label="Follow new activity"]')!.checked;
  expect(following()).toBe(true);
  document.querySelector<HTMLButtonElement>(".livepane .stream button.fold")!.click(); await settle();
  expect(following()).toBe(false);
});

it("brings an opened row's content into view without scrolling its heading away", async () => {
  const box = document.createElement("div"), inner = document.createElement("div"), row = document.createElement("div");
  inner.append(row); box.append(inner); document.body.append(box);
  let top = 1000;
  Object.defineProperties(box, { clientHeight: { get: () => 400 }, scrollHeight: { get: () => 3000 }, scrollTop: { get: () => top, set: (v: number) => { top = v; } } });
  box.getBoundingClientRect = () => ({ top: 0, bottom: 400 }) as DOMRect;
  let rect = { top: 300, bottom: 900 };
  row.getBoundingClientRect = () => rect as DOMRect;
  const [n] = createSignal(1);
  const follow = createRoot(d => { dispose = d; return createFollow(() => box, n); });
  await flush();
  top = 1000; // Where the reader is: mounting pinned the end.
  // Opened near the bottom, its content runs 500px past a 400px box with a 100px composer: it moves up only as far as its heading.
  follow.reveal(row, 100);
  expect(follow.following()).toBe(false);
  expect(top).toBe(1300);
  // A short opened row that already fits is left where it is.
  rect = { top: 50, bottom: 120 }; follow.reveal(row, 100);
  expect(top).toBe(1300);
  // One whose heading is above the box comes back down to it.
  rect = { top: -40, bottom: 200 }; follow.reveal(row, 100);
  expect(top).toBe(1260);
});
