import { render } from "solid-js/web";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type Message, type Task, type TimelineEntry, type Worker } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { flush, projection, scriptedPort, withTask } from "./support";

// Every full message in the conversation copies its recorded Markdown source, not what the screen rendered from it.
let dispose: (() => void) | undefined;
let written: string[] = [];
let refuse = false;
beforeEach(() => {
  written = []; refuse = false;
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: vi.fn(async (text: string) => { if (refuse) throw new Error("denied"); written.push(text); }) } });
});
afterEach(() => {
  dispose?.(); dispose = undefined; document.body.innerHTML = ""; localStorage.clear();
  Reflect.deleteProperty(navigator, "clipboard");
});
const settle = async () => { for (let i = 0; i < 8; i++) await flush(); };
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string) => [...document.querySelectorAll<T>(selector)];

/** Heading, list, fenced code, a link, blank lines and trailing spaces: what a rendered copy would lose or change. */
const OWNER = "# Owner heading\n\n- first item\n- second **bold** item\n\n```ts\nconst x = 1;\n\n  indented();\n```\n\nSee [the docs](https://example.com/docs \"t\").  \n";
const REPLY = "## Coordinator reply\n\n1. one\n2. two\n\n~~~\nraw <b>not html</b>\n~~~\n\n[link](https://example.com/a_b) and `code`";
const STEER = "### Steer\n\n* a\n* b\n\n```\nx\n```\n[x](https://e.x)";

function world() {
  const state = withTask(emptyWorkspace(), "Copy sources");
  Object.assign(state.tasks[0], { status: "working", process: "live", executionId: "e", description: "Task brief with a [link](https://b.example)" } as Partial<Task>);
  const session = emptySession("t");
  session.timeline = [
    { id: "s1", kind: "said", text: OWNER },
    { id: "c1", kind: "cortex", text: REPLY },
    { id: "m1", kind: "message", messageId: "msg1" },
  ] as TimelineEntry[];
  state.sessions = [session];
  state.messages = [{ id: "msg1", taskId: "t", workerId: null, text: STEER, at: null, delivery: "delivered", deliveredAt: null, failure: null } as unknown as Message];
  state.machines[0].connection = "connected";
  return state;
}
async function mount() {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", world())); await settle();
  return p;
}
/** The copy control that follows a message bubble or reply on its own row. */
const copyAfter = (selector: string) => {
  const message = all(`.timeline ${selector}`).find(el => el.nextElementSibling?.classList.contains("ws-msg-tools"))!;
  return { message, tools: message.nextElementSibling as HTMLElement, button: message.nextElementSibling!.querySelector<HTMLButtonElement>("button.ws-copy-btn")! };
};

it("offers a real copy button on the owner's messages and on received ones", async () => {
  await mount();
  const owner = copyAfter(".mine"), reply = copyAfter(".ai");
  for (const c of [owner, reply]) {
    expect(c.button).not.toBeNull();
    expect(c.button.type).toBe("button");
    expect(c.button.getAttribute("aria-label")).toBe("Copy message as Markdown");
    expect(c.button.textContent).toBe("Copy");
  }
  expect(owner.tools.classList.contains("end")).toBe(true);
  expect(reply.tools.classList.contains("end")).toBe(false);
  // The steer's copy sits on its own meta line, and the brief's on the brief's.
  expect($(".timeline .mine-meta .ws-copy-btn")).not.toBeNull();
  expect($(".timeline .brief .meta .ws-copy-btn")).not.toBeNull();
});

it("copies the owner's message as its exact Markdown source and confirms it", async () => {
  await mount();
  const owner = copyAfter(".mine");
  // The bubble shows rendered blocks, never the Markdown marks themselves.
  expect(owner.message.querySelector("h1")?.textContent).toBe("Owner heading");
  expect(owner.message.textContent).not.toContain("```");
  owner.button.click(); await settle();
  expect(written).toEqual([OWNER]);
  expect(owner.tools.querySelector(".ws-copy-note")?.textContent).toBe("Copied");
});

it("copies a received reply as its exact Markdown source and confirms it", async () => {
  await mount();
  const reply = copyAfter(".ai");
  expect(reply.message.querySelector("ol li")?.textContent).toBe("one");
  reply.button.click(); await settle();
  expect(written).toEqual([REPLY]);
  expect(reply.tools.querySelector(".ws-copy-note")?.textContent).toBe("Copied");
});

it("copies a steer and the brief from their meta lines without changing a byte", async () => {
  await mount();
  $<HTMLButtonElement>(".timeline .mine-meta .ws-copy-btn")!.click(); await settle();
  $<HTMLButtonElement>(".timeline .brief .meta .ws-copy-btn")!.click(); await settle();
  expect(written).toEqual([STEER, "Task brief with a [link](https://b.example)"]);
});

it("says nothing was copied when the clipboard refuses, and confirms nothing", async () => {
  await mount();
  refuse = true;
  const owner = copyAfter(".mine");
  owner.button.click(); await settle();
  expect(written).toEqual([]);
  const status = owner.tools.querySelector(".ws-copy-note")!;
  expect(status.textContent).toBe("Not copied");
  expect(status.textContent).not.toContain("Copied");
});

it("confirms nothing when there is no clipboard at all", async () => {
  await mount();
  Reflect.deleteProperty(navigator, "clipboard");
  const reply = copyAfter(".ai");
  reply.button.click(); await settle();
  expect(reply.tools.querySelector(".ws-copy-note")?.textContent).toBe("Not copied");
});

it("copies a worker's note, the owner's message to it and its result from the worker feed", async () => {
  const state = withTask(emptyWorkspace(), "Feed task");
  state.tasks[0].status = "review";
  state.machines[0].connection = "connected";
  state.workers = [{ id: "w", taskId: "t", executionId: "e", role: "frontend-dev", status: "completed", lane: "execution", goal: "Build the page", now: null, waitsFor: null, scope: [], route: { model: "m", effort: "high" }, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null,
    activity: [
      { sequence: 1, kind: "note", text: "[worker message] " + REPLY, at: null },
      { sequence: 2, kind: "message", text: "", at: null, messageId: "wm1" },
    ],
    result: { summary: OWNER, checks: [], reportId: null } } as unknown as Worker];
  state.messages = [{ id: "wm1", taskId: "t", workerId: "w", text: STEER, at: null, delivery: "delivered", deliveredAt: null, failure: null } as unknown as Message];
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t/workers/w`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", state)); await settle();
  const note = $(".stream-in .ev-note")!;
  expect(note.textContent).not.toContain("[worker message]");
  (note.nextElementSibling as HTMLElement).querySelector<HTMLButtonElement>(".ws-copy-btn")!.click(); await settle();
  $<HTMLButtonElement>(".stream-in .me .ms .ws-copy-btn")!.click(); await settle();
  const result = $<HTMLButtonElement>(".ws-worker-result .rcard-h .ws-copy-btn")!;
  expect(result.getAttribute("aria-label")).toBe("Copy the worker result as Markdown");
  result.click(); await settle();
  expect(written).toEqual([REPLY, STEER, OWNER]);
  expect($(".ws-worker-result .rcard-h .ws-copy-note")?.textContent).toBe("Copied");
});

it("copies the task result from its card", async () => {
  const state = world();
  Object.assign(state.tasks[0], { status: "review", process: "stopped", workspaceDigest: "d", result: { version: 1, digest: "d", summary: REPLY, checks: [] } } as unknown as Partial<Task>);
  state.sessions[0].timeline = [];
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/t`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", state)); await settle();
  const copy = $<HTMLButtonElement>(".timeline .rcard .rcard-h .ws-copy-btn")!;
  expect(copy.getAttribute("aria-label")).toBe("Copy the result as Markdown");
  copy.click(); await settle();
  expect(written).toEqual([REPLY]);
});

it("clears the confirmation after a short while", async () => {
  await mount();
  vi.useFakeTimers();
  try {
    const owner = copyAfter(".mine");
    owner.button.click();
    await vi.advanceTimersByTimeAsync(0);
    expect(owner.tools.querySelector(".ws-copy-note")?.textContent).toBe("Copied");
    await vi.advanceTimersByTimeAsync(2000);
    expect(owner.tools.querySelector(".ws-copy-note")?.textContent).toBe("");
  } finally { vi.useRealTimers(); }
});
