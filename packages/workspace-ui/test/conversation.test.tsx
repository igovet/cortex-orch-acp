import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type Decision, type Sprint, type Task, type WorkspaceCommand } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

// The task screen read as a conversation: decisions and results inline, one composer, details one activation away.
let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; localStorage.clear(); });
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];
const named = (label: string) => all<HTMLButtonElement>("button").find(b => b.getAttribute("aria-label") === label || b.textContent?.trim() === label);

const question = (patch: Partial<Decision> = {}): Decision => ({ id: "d1", kind: "question", from: "QA Engineer", workerId: null, at: null, text: "Which concurrency level?", options: [{ label: "50 parallel", optionId: "o1" }, { label: "200 parallel", optionId: "o2" }], answer: null, withdrawn: false, ...patch });
const result = { version: 1, digest: "d", summary: "Locked the refresh path.", checks: [] } as unknown as Task["result"];

function world(task: Partial<Task>, decisions: Decision[] = []) {
  const state = withTask(emptyWorkspace(), "Fix token refresh race");
  Object.assign(state.tasks[0], task);
  const session = emptySession("t");
  session.decisions = decisions;
  session.timeline = decisions.map(d => ({ id: `e-${d.id}`, kind: "decision" as const, decisionId: d.id }));
  state.sessions = [session];
  state.machines[0].connection = "connected";
  return state;
}
async function mount(state: ReturnType<typeof world>) {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", state)); await settle();
  return p;
}
const live = { status: "working", process: "live", executionId: "e" } as Partial<Task>;
const typeMessage = (text: string) => { const el = $<HTMLTextAreaElement>('.dock [aria-label="Message"]')!; el.value = text; el.dispatchEvent(new Event("input", { bubbles: true })); return el; };
const sent = (p: ReturnType<typeof scriptedPort>) => p.commands.map(c => c.payload as WorkspaceCommand).filter(c => c.type === "send_message") as Extract<WorkspaceCommand, { type: "send_message" }>[];

it("keeps an earlier result as read-only history while a question waits, and offers no Accept beside it", async () => {
  await mount(world({ ...live, result, workspaceDigest: "d" }, [question()]));
  expect($(".timeline .dc")?.textContent).toContain("Which concurrency level?");
  expect($(".rcard")).toBeNull();
  expect($(".ws-prev-result")?.textContent).toContain("Previous result v1, read-only. A decision is pending.");
  expect(all(".timeline button").some(b => /^Accept/.test(b.textContent?.trim() ?? ""))).toBe(false);
});

it("shows the result inline once nothing is pending", async () => {
  await mount(world({ status: "review", result, workspaceDigest: "d" }));
  expect($(".timeline .rcard")?.textContent).toContain("Result v1");
  expect($(".ws-prev-result")).toBeNull();
});

it("shows the actual integration action beside the composer for an Automatic different-target hold", async () => {
  const state = world({ status: "ready_to_merge", process: "stopped", checkout: { kind: "main", branch: "feature" }, result, workspaceDigest: "d" });
  const sprint: Sprint = { id: "sp", projectId: "p", title: "Automatic delivery", description: "Automatic delivery", runMode: "automatic", targetBranch: "main", planning: "created", conversation: [], plan: null, materializedVersion: 1, taskIds: ["t"], queueState: "paused", pauseReason: "Confirm the target", automaticQueue: { kind: "owner_integration", taskId: "t", code: "different_target", reason: "Confirm another target", needsOwner: true,
    nextActor: "owner", ownerActions: ["confirm_integration", "close_as_is"], asks: ["Confirm how to bring this result into the target branch."] } };
  state.tasks[0].sprintId = "sp"; state.sprints = [sprint];
  await mount(state);
  const hold = document.querySelector(".chat-review-hold");
  expect(hold?.textContent).toContain("Confirm another target");
  expect(hold?.textContent).toContain("Review and confirm integration");
  expect(hold?.querySelector("a")?.getAttribute("href")).toContain("/changes");
});

it("folds the general composer while a question waits, and opening it says it is not an answer", async () => {
  await mount(world(live, [question()]));
  expect($('.dock [aria-label="Message"]')).toBeNull();
  named("Send a separate message")!.click(); await settle();
  expect($<HTMLTextAreaElement>('.dock [aria-label="Message"]')).not.toBeNull();
  expect($(".dock .mc-effect")?.textContent).toBe("To the coordinator, not an answer to the question.");
  // The answer has its own composer inside the question, with its own plane.
  expect($('.dc [aria-label="Your answer"]')).not.toBeNull();
  expect($('.dc [aria-label="Send answer"]')?.getAttribute("aria-disabled")).toBe("true");
});

it("delivers by the visible choice only: Ctrl+Enter does not queue, and Enter follows Queue once it is chosen", async () => {
  const p = await mount(world(live));
  typeMessage("Lock the row").dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", ctrlKey: true, bubbles: true, cancelable: true }));
  await settle();
  expect(sent(p).map(c => c.mode)).toEqual(["send_now"]);
  p.receipt(); await settle();
  all<HTMLButtonElement>(".mc-delivery button").find(b => b.getAttribute("aria-label") === "Queue")!.click(); await settle();
  expect(all(".mc-delivery button").map(b => [b.getAttribute("aria-label"), b.getAttribute("aria-pressed")])).toEqual([["Queue", "true"]]);
  // An empty field's button is Stop; with text it is the plane, named by the delivery chosen.
  expect(named("Queue message")).toBeUndefined();
  const field = typeMessage("Then add the test");
  await settle();
  expect(named("Queue message")).toBeTruthy();
  field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  await settle();
  expect(sent(p).map(c => c.mode)).toEqual(["send_now", "queue"]);
});

it("keeps Attach and Voice first and ends with one action button: Stop while the field is empty, the plane once it has text", async () => {
  const p = await mount(world(live));
  const bar = $(".dock .c-bar")!;
  const buttons = [...bar.querySelectorAll<HTMLButtonElement>("button")];
  expect(buttons.slice(0, 2).map(b => b.getAttribute("aria-label"))).toEqual(["Attach files", "Voice input"]);
  const last = () => [...$(".dock .c-bar")!.querySelectorAll<HTMLButtonElement>("button")].at(-1)!;
  expect(last().classList.contains("mc-send")).toBe(true);
  expect(last().getAttribute("aria-label")).toBe("Stop");
  expect(last().querySelector(".i-stop")).not.toBeNull();
  expect(last().getAttribute("aria-disabled")).toBeNull();
  expect($(".dock .mc-effect")?.textContent).toMatch(/^Stop pauses the task/);
  // The composer is one named group; its action button is the only Stop on the page, and there is no second one beside the plane.
  expect($('.dock [role="group"][aria-label="Task composer"]')).not.toBeNull();
  const field = typeMessage("Lock the row"); await settle();
  expect(last().getAttribute("aria-label")).toBe("Send message now");
  expect(last().querySelector(".i-send")).not.toBeNull();
  expect(all(".dock button").some(b => b.getAttribute("aria-label") === "Stop" || /^Stop$/.test(b.textContent?.trim() ?? ""))).toBe(false);
  // Enter with an empty field never stops: clear it, press Enter, nothing is sent.
  field.value = ""; field.dispatchEvent(new Event("input", { bubbles: true })); await settle();
  expect(last().getAttribute("aria-label")).toBe("Stop");
  field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true })); await settle();
  expect(p.commands.filter(c => c.payload.type === "stop_task")).toEqual([]);
  last().click(); await settle();
  expect(p.commands.map(c => c.payload.type)).toContain("stop_task");
});

it("reads progress in words with details one activation away, and names no product", async () => {
  await mount(world(live));
  expect($(".phases")).toBeNull();
  expect(all('nav[aria-label="Task details"] a').map(a => a.textContent?.trim().split(/\s/)[0])).toEqual(["Work", "Changes", "Code", "Terminal"]);
  expect(document.body.textContent).not.toMatch(/cortex/i);
});

// The composer is the one place that controls the run: its action button is Start, Stop or Resume while the field is empty.
const action = () => [...$('[aria-label="Task composer"] .c-bar')!.querySelectorAll<HTMLButtonElement>("button")].at(-1)!;
const band = () => all<HTMLElement>(".tb-actions button, .tb-actions a").map(b => (b.getAttribute("aria-label") ?? "") + " " + (b.textContent ?? ""));

it("has no state chip, Start, Stop or Resume in the title band; the task menu keeps Stop", async () => {
  await mount(world(live));
  expect($(".tb-actions .tb-state, .tb-actions .chip")).toBeNull();
  expect(band().some(t => /\b(Stop|Resume|Start)\b/.test(t))).toBe(false);
  named("Task actions")!.click(); await settle();
  expect(all('[role="menuitem"], .mi').some(e => e.textContent?.trim().startsWith("Stop"))).toBe(true);
});

it("stops from the task menu with a draft in the field, and keeps the draft", async () => {
  const p = await mount(world(live));
  typeMessage("Half written"); await settle();
  expect(action().getAttribute("aria-label")).toBe("Send message now");
  named("Task actions")!.click(); await settle();
  all<HTMLElement>('[role="menuitem"], .mi').find(e => e.textContent?.trim().startsWith("Stop"))!.click(); await settle();
  expect(p.commands.map(c => c.payload.type)).toContain("stop_task");
  expect($<HTMLTextAreaElement>('.dock [aria-label="Message"]')!.value).toBe("Half written");
});

it("resumes a paused task from the empty composer, and sends and resumes once it has text", async () => {
  const p = await mount(world({ status: "paused", process: "stopped", executionId: "e" }));
  expect(action().getAttribute("aria-label")).toBe("Resume");
  expect($(".dock .mc-effect")?.textContent).toMatch(/^Resume continues the task/);
  typeMessage("Carry on"); await settle();
  expect(action().getAttribute("aria-label")).toBe("Send and resume");
  typeMessage(""); await settle();
  action().click(); await settle();
  expect(p.commands.map(c => c.payload.type)).toEqual(["continue_task"]);
});

it("starts a draft from the composer", async () => {
  const p = await mount(world({ status: "draft", process: "stopped", executionId: null }));
  expect(action().getAttribute("aria-label")).toBe("Start");
  expect(action().getAttribute("aria-disabled")).toBeNull();
  expect(band().some(t => /\bStart\b/.test(t))).toBe(false);
  action().click(); await settle();
  expect(p.commands.map(c => c.payload.type)).toEqual(["start_task"]);
});

it("offers no Stop where nothing runs: a question waits, or the task is in review", async () => {
  await mount(world({ status: "needs_you", process: "live", executionId: "e" }, [question()]));
  expect(all("button").some(b => b.getAttribute("aria-label") === "Stop")).toBe(false);
  dispose?.(); document.body.innerHTML = "";
  await mount(world({ status: "review", result, workspaceDigest: "d" }));
  expect(action().getAttribute("aria-label")).toBe("Send and reopen");
});
