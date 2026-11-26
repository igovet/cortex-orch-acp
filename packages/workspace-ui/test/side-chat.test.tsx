import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { SessionOptions, SideChatAsk, SideChatEntry, SideChatPort } from "@cortex/workspace-domain";
import { clampPlace, pillLabel, SIDE_MARGIN, SIDE_MIN, SIDE_POLL_MS, SideChatPanel, toggleSideChat } from "../src/side-chat";
import { createSideChatPort, sideChatEntry } from "../src/port/side-chat";
import { SIDE_CHAT_PLACE, type SideChatPlace } from "../src/prefs";

const VIEW = { w: 1200, h: 800 };
const OPTIONS: SessionOptions = {
  models: [{ id: "route-a", label: "Model A", engine: "Claude", efforts: ["low", "high"] }, { id: "route-b", label: "Model B", engine: "Codex", efforts: ["medium"] }],
  efforts: [{ id: "low", label: "Low" }, { id: "medium", label: "Medium" }, { id: "high", label: "High" }],
  modes: [], languages: [],
};
const entry = (id: string, state: SideChatEntry["state"], extra: Partial<SideChatEntry> = {}): SideChatEntry => ({ id, question: `Question ${id}`, at: 1, state, ...extra });

function fakePort(history: SideChatEntry[] = []) {
  const asked: SideChatAsk[] = [], calls: string[] = [];
  let next: SideChatEntry | null = null;
  const polls = new Map<string, SideChatEntry>();
  const port: SideChatPort = {
    history: async task => { calls.push(`history:${task}`); return history; },
    ask: async request => { calls.push("ask"); asked.push(request); const e = next ?? entry("e1", "done", { question: request.question, answer: "An answer." }); return { ok: true, entry: e }; },
    poll: async (_task, id) => { calls.push(`poll:${id}`); return polls.get(id) ?? null; },
    clear: async () => { calls.push("clear"); return true; },
  };
  return { port, asked, calls, polls, answerNext: (e: SideChatEntry) => { next = e; } };
}

let dispose: (() => void) | undefined;
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; vi.useRealTimers(); });
const tick = () => new Promise(resolve => setTimeout(resolve, 0));

function mount(port: SideChatPort, opts: { place?: Partial<SideChatPlace>; phone?: boolean; task?: string } = {}) {
  const [place, setPlace] = createSignal<SideChatPlace>({ ...SIDE_CHAT_PLACE, open: true, ...opts.place });
  const [task, setTask] = createSignal(opts.task ?? "task-1");
  const saved: SideChatPlace[] = [];
  const holder = document.body.appendChild(document.createElement("div"));
  dispose = render(() => <SideChatPanel taskId={task()} port={port} defaults={() => ({ model: "route-a", effort: "high" })} options={() => OPTIONS}
    place={place} setPlace={p => { saved.push(p); setPlace(p); }} phone={opts.phone} title="Side chat" note="Doesn't affect the task" viewport={() => VIEW} />, holder);
  return { place, saved, setTask, holder };
}
const panel = () => document.querySelector<HTMLElement>("section.sidechat");
const button = (label: string) => document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
const type = (text: string) => {
  const field = document.querySelector<HTMLTextAreaElement>("textarea[aria-label='Side chat question']")!;
  field.value = text; field.dispatchEvent(new Event("input", { bubbles: true }));
  return field;
};
/** Chooses `label` in the picker whose menu is named `menu`. */
const pick = (menu: string, label: string) => {
  const item = [...document.querySelectorAll<HTMLButtonElement>(`[popover][aria-label='${menu}'] [role=menuitem]`)].find(b => b.textContent?.startsWith(label));
  if (!item) throw new Error(`No ${label} in ${menu}`);
  item.click();
};
const pointer = (target: Element, type: string, x: number, y: number) =>
  target.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, clientX: x, clientY: y, button: 0 }));

describe("side chat placement", () => {
  it("starts in the bottom right corner and always stays wholly inside the window", () => {
    expect(clampPlace({ x: -1, y: -1, w: 380, h: 480 }, VIEW)).toEqual({ x: VIEW.w - 380 - SIDE_MARGIN, y: VIEW.h - 480 - SIDE_MARGIN, w: 380, h: 480 });
    // Dragged past the right and bottom edges, it stops at the margin.
    expect(clampPlace({ x: 5000, y: 5000, w: 380, h: 480 }, VIEW)).toEqual({ x: VIEW.w - 380 - SIDE_MARGIN, y: VIEW.h - 480 - SIDE_MARGIN, w: 380, h: 480 });
    expect(clampPlace({ x: -0, y: 0, w: 380, h: 480 }, VIEW)).toMatchObject({ x: SIDE_MARGIN, y: SIDE_MARGIN });
    // Resized below its minimum or past the window, it keeps a usable size that fits.
    expect(clampPlace({ x: 100, y: 100, w: 10, h: 10 }, VIEW)).toMatchObject({ w: SIDE_MIN.w, h: SIDE_MIN.h });
    expect(clampPlace({ x: 100, y: 100, w: 9000, h: 9000 }, VIEW)).toEqual({ x: SIDE_MARGIN, y: SIDE_MARGIN, w: VIEW.w - 2 * SIDE_MARGIN, h: VIEW.h - 2 * SIDE_MARGIN });
    // A window that shrank pulls a remembered place back on screen.
    expect(clampPlace({ x: 900, y: 500, w: 380, h: 480 }, { w: 800, h: 600 })).toEqual({ x: 800 - 380 - SIDE_MARGIN, y: 600 - 480 - SIDE_MARGIN, w: 380, h: 480 });
  });

  it("opens, collapses and expands from one toggle, and the pill counts the exchanges", () => {
    const closed = { ...SIDE_CHAT_PLACE };
    const opened = toggleSideChat(closed);
    expect(opened).toMatchObject({ open: true, collapsed: false });
    expect(toggleSideChat(opened)).toMatchObject({ open: true, collapsed: true });
    expect(toggleSideChat(toggleSideChat(opened))).toMatchObject({ open: true, collapsed: false });
    expect(pillLabel("Side chat", 0)).toBe("Side chat");
    expect(pillLabel("Side chat", 3)).toBe("Side chat (3)");
  });

  it("moves by its header and resizes by its corner, clamped, and remembers the place once it is let go", async () => {
    const { port } = fakePort();
    const { saved } = mount(port, { place: { x: 100, y: 100, w: 380, h: 480 } });
    await tick();
    const head = document.querySelector(".sidechat-head")!;
    pointer(head, "pointerdown", 150, 110);
    pointer(window as unknown as Element, "pointermove", 5150, 5110);
    // While dragging the panel follows, but nothing is stored yet.
    expect(panel()!.style.left).toBe(`${VIEW.w - 380 - SIDE_MARGIN}px`);
    expect(saved).toEqual([]);
    pointer(window as unknown as Element, "pointerup", 5150, 5110);
    expect(saved.at(-1)).toMatchObject({ x: VIEW.w - 380 - SIDE_MARGIN, y: VIEW.h - 480 - SIDE_MARGIN, w: 380, h: 480, open: true });
    const grip = document.querySelector(".sidechat-grip")!;
    pointer(grip, "pointerdown", 0, 0);
    pointer(window as unknown as Element, "pointermove", -1000, -1000);
    pointer(window as unknown as Element, "pointerup", -1000, -1000);
    expect(saved.at(-1)).toMatchObject({ w: SIDE_MIN.w, h: SIDE_MIN.h });
    // A press on a header control is a click, not a drag.
    const before = saved.length;
    pointer(button("Collapse Side chat"), "pointerdown", 10, 10);
    pointer(window as unknown as Element, "pointerup", 400, 400);
    expect(saved.length).toBe(before);
  });
});

describe("side chat panel", () => {
  it("says it does not affect the task, collapses on Esc to a counted pill, expands and closes", async () => {
    const { port } = fakePort([entry("old", "done", { answer: "Kept." })]);
    const { place } = mount(port);
    await tick();
    expect(panel()!.getAttribute("aria-label")).toBe("Side chat");
    expect(panel()!.textContent).toContain("Doesn't affect the task");
    expect(panel()!.textContent).toContain("Kept.");
    panel()!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(place()).toMatchObject({ open: true, collapsed: true });
    expect(panel()).toBeNull();
    expect(button("Expand Side chat").textContent).toContain("Side chat (1)");
    button("Expand Side chat").click();
    expect(panel()).not.toBeNull();
    button("Close Side chat").click();
    expect(place()).toMatchObject({ open: false, collapsed: false });
    expect(document.querySelector(".sidechat, .sidechat-pill")).toBeNull();
  });

  it("asks on the task's route by default, on a picked model and effort when chosen, and polls a running answer", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const fake = fakePort();
    mount(fake.port);
    await tick();
    type("What changed?");
    document.querySelector("form.sidechat-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await tick();
    expect(fake.asked[0]).toEqual({ taskId: "task-1", question: "What changed?", model: "route-a", effort: "high" });
    expect(panel()!.textContent).toContain("An answer.");
    // Another model: its own efforts only.
    // The app's own picker (a trigger and a menu of choices), never a native select.
    expect(document.querySelector(".sidechat select")).toBeNull();
    pick("Side chat model", "Model B");
    await tick();
    const efforts = [...document.querySelectorAll<HTMLButtonElement>("[popover][aria-label='Effort'] [role=menuitem]")].map(o => o.textContent);
    expect(efforts).toEqual(["Medium"]);
    expect(document.querySelector("button[aria-label^='Side chat effort']")!.getAttribute("aria-label")).toBe("Side chat effort: Medium");
    fake.answerNext(entry("e2", "running", { question: "And now?" }));
    const field = type("And now?");
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await tick();
    expect(fake.asked[1]).toEqual({ taskId: "task-1", question: "And now?", model: "route-b", effort: "medium" });
    expect(panel()!.textContent).toContain("Answering…");
    fake.polls.set("e2", entry("e2", "done", { question: "And now?", answer: "Polled answer." }));
    await vi.advanceTimersByTimeAsync(SIDE_POLL_MS + 50);
    expect(panel()!.textContent).toContain("Polled answer.");
    expect(panel()!.textContent).not.toContain("Answering…");
  });

  it("collapses on Esc wherever focus is in the panel, not only in the question field", async () => {
    const { port } = fakePort([entry("old", "done", { answer: "Kept." })]);
    const { place } = mount(port);
    await tick();
    // A press on the answer text (nothing focusable there) leaves focus on the panel itself.
    const box = panel()!;
    expect(box.tabIndex).toBe(-1);
    box.focus();
    expect(document.activeElement).toBe(box);
    box.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    expect(place()).toMatchObject({ open: true, collapsed: true });
    button("Expand Side chat").click();
    // A drag by the header takes focus into the panel too, so Esc still applies after it.
    (document.activeElement as HTMLElement | null)?.blur();
    pointer(document.querySelector(".sidechat-head")!, "pointerdown", 150, 110);
    pointer(window as unknown as Element, "pointerup", 150, 110);
    expect(panel()!.contains(document.activeElement)).toBe(true);
    document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    expect(place()).toMatchObject({ collapsed: true });
  });

  it("renders the answer's Markdown as task results are rendered", async () => {
    const { port } = fakePort([entry("md", "done", { answer: "**Bold** and `code`\n\n- one\n- two" })]);
    mount(port);
    await tick();
    const answer = document.querySelector(".sidechat-a")!;
    expect(answer.querySelector("strong")?.textContent).toBe("Bold");
    expect(answer.querySelector("code")?.textContent).toBe("code");
    expect([...answer.querySelectorAll("li")].map(li => li.textContent)).toEqual(["one", "two"]);
    expect(answer.textContent).not.toContain("**");
  });

  it("talks to the side-chat port alone: no task command, store write or conversation message", async () => {
    const fake = fakePort();
    mount(fake.port);
    await tick();
    type("Is it done?");
    document.querySelector("form.sidechat-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await tick();
    expect(fake.calls).toEqual(["history:task-1", "ask"]);
    // The component's source never reaches the task's commands, sends, drafts or conversation.
    const here = dirname(fileURLToPath(import.meta.url));
    const source = readFileSync(join(here, "../src/side-chat.tsx"), "utf8");
    for (const forbidden of [/ws\.run\(/, /ws\.ok\(/, /ws\.drafts/, /send_message/, /ws\.store\.(?!sideChat\b|state\(\)\.options)/, /from "\.\/sends"/]) expect(source).not.toMatch(forbidden);
  });

  it("shows a failed or interrupted answer as such, never as an answer, and a refusal in words", async () => {
    const fake = fakePort([entry("a", "failed", { reason: "timeout" }), entry("b", "interrupted")]);
    mount(fake.port);
    await tick();
    expect(panel()!.textContent).toContain("No answer in time");
    expect(panel()!.textContent).toContain("Interrupted");
    fake.port.ask = async () => ({ ok: false, code: "side_chat_no_route", message: "No model can answer here." });
    type("Anyone?");
    document.querySelector("form.sidechat-form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await tick();
    expect(document.querySelector("[role=alert]")!.textContent).toBe("No model can answer here.");
  });

  it("asks the machine nothing and shows nothing while closed", async () => {
    const fake = fakePort();
    const { saved } = mount(fake.port, { place: { open: false } });
    await tick();
    expect(fake.calls).toEqual([]);
    expect(document.querySelector(".sidechat, .sidechat-pill")).toBeNull();
    expect(saved).toEqual([]);
  });

  it("reads each task's own history when the task changes", async () => {
    const fake = fakePort();
    const { setTask } = mount(fake.port);
    await tick();
    setTask("task-2");
    await tick();
    expect(fake.calls).toEqual(["history:task-1", "history:task-2"]);
  });

  it("is a bottom sheet without drag or resize on a phone", async () => {
    const { port } = fakePort();
    const { saved } = mount(port, { phone: true });
    await tick();
    expect(panel()!.classList.contains("sidechat-sheet")).toBe(true);
    expect(panel()!.style.left).toBe("");
    expect(document.querySelector(".sidechat-grip")).toBeNull();
    pointer(document.querySelector(".sidechat-head")!, "pointerdown", 10, 10);
    pointer(window as unknown as Element, "pointermove", 300, 300);
    pointer(window as unknown as Element, "pointerup", 300, 300);
    expect(saved).toEqual([]);
  });
});

describe("side chat port", () => {
  it("speaks product.task side-chat actions for the task's project and keeps only valid entries", async () => {
    const sent: { projectId: string; params: Record<string, unknown> }[] = [];
    const port = createSideChatPort(async (projectId, params) => {
      sent.push({ projectId, params });
      if (params.action === "side_chat_history") return { ok: true, result: { entries: [entry("x", "done", { answer: "a" }), { id: 7 }, { id: "y", question: "q", state: "weird" }] } };
      if (params.action === "side_question") return { ok: true, result: { entryId: "z", entry: entry("z", "running") } };
      return { ok: false, code: "unknown_entry", message: "Gone." };
    }, task => task === "t1" ? "p1" : null);
    expect((await port.history("t1")).map(e => e.id)).toEqual(["x"]);
    expect(await port.ask({ taskId: "t1", question: "q", model: "route-a" })).toMatchObject({ ok: true, entry: { id: "z", state: "running" } });
    expect(await port.poll("t1", "z")).toBeNull();
    expect(await port.ask({ taskId: "unknown", question: "q" })).toMatchObject({ ok: false, code: "offline" });
    expect(sent).toEqual([
      { projectId: "p1", params: { taskId: "t1", action: "side_chat_history" } },
      { projectId: "p1", params: { taskId: "t1", action: "side_question", question: "q", model: "route-a" } },
      { projectId: "p1", params: { taskId: "t1", action: "side_chat_poll", entryId: "z" } },
    ]);
    expect(sideChatEntry({ id: "a", question: "q", state: "done", route: { fileAccess: "bogus", model: "m" } })?.route).toEqual({ engine: undefined, model: "m", effort: null, fileAccess: undefined });
  });
});
