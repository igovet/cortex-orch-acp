import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type Decision, type Task, type WorkspaceCommand } from "@cortex/workspace-domain";
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
  all<HTMLButtonElement>(".mc-delivery button").find(b => b.textContent === "Queue")!.click(); await settle();
  expect(all(".mc-delivery button").map(b => [b.textContent, b.getAttribute("aria-pressed")])).toEqual([["Send now", "false"], ["Queue", "true"]]);
  expect(named("Queue message")).toBeTruthy();
  typeMessage("Then add the test").dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  await settle();
  expect(sent(p).map(c => c.mode)).toEqual(["send_now", "queue"]);
});

it("keeps Attach and Voice first and ends with the paper plane, which never turns into Stop", async () => {
  await mount(world(live));
  const bar = $(".dock .c-bar")!;
  const buttons = [...bar.querySelectorAll<HTMLButtonElement>("button")];
  expect(buttons.slice(0, 2).map(b => b.getAttribute("aria-label"))).toEqual(["Attach files", "Voice input"]);
  const plane = buttons.at(-1)!;
  expect(plane.classList.contains("mc-send")).toBe(true);
  expect(plane.querySelector(".i-send")).not.toBeNull();
  expect(plane.getAttribute("aria-label")).toBe("Send message now");
  // An empty draft disables the plane with its reason; Stop stays a task action in the title band.
  expect(plane.getAttribute("aria-disabled")).toBe("true");
  expect(plane.dataset.why).toBe("Write a message first");
  expect(all(".dock button").some(b => /Stop/.test(b.textContent ?? ""))).toBe(false);
});

it("reads progress in words with details one activation away, and names no product", async () => {
  await mount(world(live));
  expect($(".phases")).toBeNull();
  expect(all('nav[aria-label="Task details"] a').map(a => a.textContent?.trim().split(/\s/)[0])).toEqual(["Work", "Changes", "Code", "Terminal"]);
  expect(document.body.textContent).not.toMatch(/cortex/i);
});
