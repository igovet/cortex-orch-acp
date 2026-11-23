import { render } from "solid-js/web";
import { afterEach, expect, it } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type PortCapabilities, type Worker, type WorkerStatus } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

// A worker is stopped from its composer, as a task is from its own: while it runs and the field is empty the action button is
// Stop, which opens the stop dialog with its optional reason. The header has no Stop of its own, and the activity has no
// search or Follow strip above it.
let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; localStorage.clear(); });
const settle = async () => { for (let i = 0; i < 10; i++) await flush(); };
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = (selector: string) => [...document.querySelectorAll<HTMLElement>(selector)];

const workerOf = (status: WorkerStatus): Worker => ({ id: "w", taskId: "t", executionId: "e", role: "frontend-dev", status, lane: "execution", goal: "Build the page", now: null, waitsFor: null,
  scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, result: null,
  activity: [{ sequence: 1, kind: "note", text: "Reading the page", at: null }] });

async function open(status: WorkerStatus = "working", capabilities: PortCapabilities = { everything: true, operations: [] }) {
  const s = withTask(emptyWorkspace(), "Stop from the composer");
  Object.assign(s.tasks[0], { status: "working", process: "live", executionId: "e" });
  s.machines[0].connection = "connected";
  s.sessions = [emptySession("t")];
  s.workers = [workerOf(status)];
  const p = scriptedPort({ capabilities });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t/workers/w`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", s)); await settle();
  return p;
}
const composer = () => $('.livepane [aria-label="Worker composer"]')!;
const action = () => composer().querySelector<HTMLButtonElement>(".c-send")!;
const write = async (field: HTMLTextAreaElement, value: string) => { field.value = value; field.dispatchEvent(new Event("input", { bubbles: true })); await settle(); };

it("has no Stop worker button in the worker's header", async () => {
  await open();
  expect($(".livepane .d-h")).not.toBeNull();
  expect(all(".livepane .d-h button").filter(b => /stop/i.test(`${b.textContent} ${b.getAttribute("aria-label") ?? ""}`))).toEqual([]);
});

it("makes the composer's action button Stop while the worker runs and the field is empty, and Send once text is typed", async () => {
  await open();
  // The same slot, size and icon as the task composer's Stop, with its effect line.
  expect(action().getAttribute("aria-label")).toBe("Stop worker…");
  expect(action().classList.contains("mc-send")).toBe(true);
  expect(action().querySelector("i.i-stop")).not.toBeNull();
  expect(action().getAttribute("aria-disabled")).toBeNull();
  expect(composer().querySelector(".mc-effect")?.textContent).toMatch(/Stop ends only frontend-dev; the task keeps running/);
  // Enter in an empty field never presses Stop.
  const field = composer().querySelector<HTMLTextAreaElement>('textarea[aria-label="Message frontend-dev"]')!;
  field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  await settle();
  expect($("dialog[open]")).toBeNull();
  await write(field, "Use the old colors");
  expect(action().getAttribute("aria-label")).toBe("Send to frontend-dev");
  expect(action().querySelector("i.i-send")).not.toBeNull();
  expect(composer().querySelector(".mc-effect")?.textContent).not.toMatch(/Stop ends/);
  await write(field, "");
  expect(action().getAttribute("aria-label")).toBe("Stop worker…");
});

it("opens the stop dialog from the composer's Stop, and its confirmation stops the worker with the typed reason", async () => {
  const p = await open();
  action().click(); await settle();
  expect($("dialog[open] h2")?.textContent).toBe("Stop frontend-dev?");
  // Nothing is sent until the dialog's own confirmation.
  expect(p.commands.some(c => c.payload.type === "stop_worker")).toBe(false);
  await write($<HTMLTextAreaElement>('dialog[open] textarea[aria-label="Reason, optional"]')!, "Wrong approach");
  all("dialog[open] button").find(b => b.textContent?.trim() === "Stop worker")!.click(); await settle();
  expect(p.commands.at(-1)!.payload).toMatchObject({ type: "stop_worker", workerId: "w", reason: "Wrong approach" });
});

it("keeps Stop while the worker waits for the owner, whose field is closed", async () => {
  await open("waiting_you");
  expect(action().getAttribute("aria-label")).toBe("Stop worker…");
  expect(action().getAttribute("aria-disabled")).toBeNull();
  action().click(); await settle();
  expect($("dialog[open] h2")?.textContent).toBe("Stop frontend-dev?");
});

it("leaves Stop in its place, off with the reason, where the machine does not offer stopping a worker", async () => {
  const p = await open("working", { everything: false, operations: [] });
  expect(action().getAttribute("aria-label")).toBe("Stop worker…");
  expect(action().getAttribute("aria-disabled")).toBe("true");
  expect(action().getAttribute("data-why")).toBeTruthy();
  action().click(); await settle();
  expect($("dialog[open]")).toBeNull();
  expect(p.commands.some(c => c.payload.type === "stop_worker")).toBe(false);
});

it("renders no search or Follow strip above the worker's activity", async () => {
  await open();
  expect($(".livepane .stream")).not.toBeNull();
  expect($('[aria-label="Find in this activity"]')).toBeNull();
  expect($('[aria-label="Follow new activity"]')).toBeNull();
  expect($(".livepane .live-tools, .livepane .d-tools")).toBeNull();
});
