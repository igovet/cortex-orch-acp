import { createRoot, createSignal } from "solid-js";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type Activity, type Decision, type Sprint, type Task, type TimelineEntry, type Worker, type WorkspaceState } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createFollow } from "../src/kit";
import { scrollIntoFeed } from "../src/follow";
import { draftTask, flush, projection, scriptedPort, withTask } from "./support";
import { render } from "solid-js/web";
import { Pill } from "../src/worker";

// Every feed follows one rule: at the very end it follows, the reader leaving the end stops it, nothing that arrives moves a
// reader who left, the owner's own message or answer from that screen brings the end back, and Latest (or "N new") is the one
// control. jsdom has no layout: each scroller gets a box of 400 px and a content height the test sets.
let dispose: (() => void) | undefined;
type Observed = { cb: ResizeObserverCallback; targets: Element[] };
const observers: Observed[] = [];
beforeEach(() => {
  observers.length = 0;
  vi.stubGlobal("ResizeObserver", class { targets: Element[] = []; constructor(cb: ResizeObserverCallback) { observers.push({ cb, targets: this.targets }); } observe(el: Element) { this.targets.push(el); } unobserve() {} disconnect() {} });
});
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; localStorage.clear(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
const settle = async () => { for (let i = 0; i < 10; i++) await flush(); };
const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
/** Layout ran for what is observed in `box` (the feed and its rows). */
const resize = (box: Element) => { for (const o of observers) if (o.targets.includes(box) || o.targets.includes(box.firstElementChild!)) o.cb([], {} as ResizeObserver); };
/** A 400 px feed box with a 15 px scrollbar on its right, whose content is `g.height` tall. */
function layout(box: HTMLElement, height = 2000) {
  const g = { height, top: 0 };
  Object.defineProperties(box, {
    clientHeight: { configurable: true, get: () => 400 }, clientWidth: { configurable: true, get: () => 385 },
    scrollHeight: { configurable: true, get: () => g.height },
    scrollTop: { configurable: true, get: () => g.top, set: (v: number) => { g.top = Math.max(0, Math.min(v, Math.max(0, g.height - 400))); } },
  });
  box.getBoundingClientRect = () => ({ top: 0, bottom: 400, left: 0, right: 400, width: 400, height: 400, x: 0, y: 0 }) as DOMRect;
  // A feed's top edge (its reserve and the line above its earliest row) lies at the top of its content.
  const rect = Element.prototype.getBoundingClientRect;
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
    if (box.contains(this) && this.matches(".ws-reserve, .ws-earlier")) return { top: -g.top, bottom: 30 - g.top, height: 30 } as DOMRect;
    return rect.call(this);
  });
  return g;
}
const wheelUp = (target: Element) => target.dispatchEvent(new WheelEvent("wheel", { deltaY: -100, bubbles: true }));
const scrolled = (box: Element) => box.dispatchEvent(new Event("scroll"));

// ---------- The feeds, as the owner opens them ----------

const M = LOCAL_MACHINE_ID;
const live = { status: "working", process: "live", executionId: "e" } as Partial<Task>;
const notes = (from: number, n: number): Activity[] => Array.from({ length: n }, (_, i) => ({ sequence: from + i, kind: "note" as const, text: `Observation ${from + i}`, at: null }));
const workerOf = (activity = notes(1, 6)): Worker => ({ id: "w", taskId: "t", executionId: "e", role: "frontend-dev", status: "working", lane: "execution", goal: "Build", now: null, waitsFor: null,
  scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, result: null, activity });
const question = (): Decision => ({ id: "d1", kind: "question", from: "", workerId: null, at: null, text: "Which side?", options: [{ label: "North", optionId: "o1" }, { label: "South", optionId: "o2" }], answer: null, withdrawn: false });
type Shape = { asking?: boolean; said?: string[]; more?: number };
const lines = (o: Shape): TimelineEntry[] => [
  { id: "c1", kind: "cortex", text: "Reading the project." }, { id: "c2", kind: "cortex", text: "Working on it." },
  ...(o.said ?? []).map((text, i): TimelineEntry => ({ id: `s${i}`, kind: "said", text })),
  ...(o.asking ? [{ id: "e-d1", kind: "decision" as const, decisionId: "d1" }] : []),
];
function taskWorld(o: Shape = {}): WorkspaceState {
  const s = withTask(emptyWorkspace(), "Follow everywhere");
  Object.assign(s.tasks[0], live);
  s.machines[0].connection = "connected";
  const session = emptySession("t");
  session.timeline = lines(o); session.decisions = o.asking ? [question()] : [];
  s.sessions = [session];
  s.workers = [workerOf(notes(1, 6 + (o.more ?? 0)))];
  return s;
}
const sprintOf = (patch: Partial<Sprint>): Sprint => ({ id: "sp", projectId: "p", description: "Coffee landing page", title: "Coffee landing", runMode: "manual", targetBranch: null,
  planning: "planning", planningTaskId: "plan", conversation: [], plan: null, materializedVersion: null, taskIds: [], queueState: "idle", pauseReason: null, ...patch });
function sprintWorld(): WorkspaceState {
  const s = emptyWorkspace();
  s.machines[0].connection = "connected";
  s.projects = [{ id: "p", machineId: M, name: "Cedar", repository: "/cedar", openBranch: "main", defaultBranch: "main", hasGit: true }];
  return s;
}
/** A sprint planned by a runtime task: the task's own conversation, on its sprint. */
function planningWorld(o: Shape = {}): WorkspaceState {
  const s = sprintWorld();
  s.tasks = [{ ...draftTask(), id: "plan", projectId: "p", title: "Sprint planning task", description: "Plan the sprint", ...live }];
  s.sprints = [sprintOf({ planning: o.asking ? "question" : "planning" })];
  const session = emptySession("plan");
  session.timeline = lines(o); session.decisions = o.asking ? [question()] : [];
  s.sessions = [session];
  return s;
}
/** An earlier version's sprint, planned in its own chat. */
function legacyWorld(o: Shape = {}): WorkspaceState {
  const s = sprintWorld();
  s.sprints = [sprintOf({ planningTaskId: undefined, planning: o.asking ? "question" : "planning", conversation: [
    { kind: "note", text: "Reading the project." }, { kind: "note", text: "Here is a first idea." },
    ...(o.said ?? []).map(text => ({ kind: "message" as const, text })),
    ...(o.asking ? [{ kind: "question" as const, text: "Which side?", options: ["North", "South"] }] : []),
  ] })];
  return s;
}

type Port = ReturnType<typeof scriptedPort>;
async function open(path: string, state: WorkspaceState, prepare?: (p: Port) => void) {
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  prepare?.(p);
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", state)); await settle();
  return p;
}

let coordinatorRows: Activity[] = [];
type Feed = { name: string; path: string; feed: string; world: (o?: Shape) => WorkspaceState; prepare?: (p: Port) => void; composer?: true; answers?: true };
const FEEDS: Feed[] = [
  { name: "task conversation", path: `/m/${M}/t/t`, feed: "section.task-panel .timeline", world: taskWorld, composer: true, answers: true },
  { name: "sprint planning task conversation", path: `/m/${M}/p/p/s/sp/planning`, feed: "section.task-panel .timeline", world: planningWorld, composer: true, answers: true },
  { name: "legacy sprint chat", path: `/m/${M}/p/p/s/sp/planning`, feed: ".sp-conv .timeline", world: legacyWorld, composer: true, answers: true },
  { name: "worker activity", path: `/m/${M}/t/t/workers/w`, feed: ".livepane .stream", world: taskWorld, composer: true },
  { name: "coordinator feed", path: `/m/${M}/t/t/workers/cortex`, feed: ".livepane .stream", world: taskWorld,
    prepare: p => { coordinatorRows = notes(1, 8); p.port.readWorkerActivity = async () => ({ activity: coordinatorRows, activityCursor: null, messages: [] }); } },
];
const SENDERS = FEEDS.filter(f => f.composer), ANSWERERS = FEEDS.filter(f => f.answers);

/** The feed opened and laid out at its end. */
async function feedOf(f: Feed, o: Shape = {}) {
  const p = await open(f.path, f.world(o), f.prepare);
  await vi.waitFor(() => expect($(f.feed)).not.toBeNull());
  const box = $(f.feed)!, g = layout(box);
  resize(box);
  const pill = () => box.nextElementSibling?.querySelector<HTMLButtonElement>(".new-pill") ?? null;
  /** The reader scrolls up with the wheel, as far as `to`. */
  const away = async (to = 900) => { wheelUp(box); g.top = to; scrolled(box); await settle(); };
  return { p, box, g, pill, away };
}
/** Something new arrives from elsewhere: another window's message, the record of a line, or the next activity row. */
async function arrive(f: Feed, p: Port) {
  if (f.name === "coordinator feed") coordinatorRows = [...coordinatorRows, ...notes(9, 1)];
  p.emit(projection("2", f.world({ said: ["Sent from the phone"], more: 1 }))); await settle();
}
const send = async (box: HTMLElement, text: string) => {
  const dock = box.nextElementSibling!, field = dock.querySelector<HTMLTextAreaElement>("textarea")!;
  field.value = text; field.dispatchEvent(new Event("input", { bubbles: true })); await settle();
  dock.querySelector<HTMLButtonElement>(".c-send")!.click(); await settle();
};

describe.each(FEEDS)("$name", f => {
  it("follows at the end, stops when the reader leaves, holds still as content streams, and Latest brings it back", async () => {
    const { box, g, pill, away } = await feedOf(f);
    expect(g.top).toBe(1600); expect(pill()).toBeNull();
    // Streamed text and rows grow the feed: the end stays pinned.
    g.height += 120; resize(box); expect(g.top).toBe(1720);
    // The one control is Latest: no Follow switch anywhere.
    expect($('[aria-label="Follow new activity"]')).toBeNull(); expect(document.body.dataset.follow).toBeUndefined();
    // The reader starts scrolling while content streams: what they read stays where it is.
    wheelUp(box); g.height += 120; resize(box);
    expect(g.top).toBe(1720);
    g.top = 900; scrolled(box); await settle();
    g.height += 120; resize(box); expect(g.top).toBe(900);
    expect(pill()?.textContent).toBe("Latest");
    pill()!.click(); await settle();
    expect(g.top).toBe(g.height - 400); expect(pill()).toBeNull();
    g.height += 50; resize(box); expect(g.top).toBe(g.height - 400);
    // Reaching the very end by scrolling follows again.
    await away(500); expect(pill()).not.toBeNull();
    g.top = g.height - 400; scrolled(box); await settle();
    expect(pill()).toBeNull();
    g.height += 50; resize(box); expect(g.top).toBe(g.height - 400);
  });

  it("does not move a reader who scrolled up when something arrives from elsewhere, and counts it as new", async () => {
    const { p, box, g, pill, away } = await feedOf(f);
    await away();
    await arrive(f, p);
    g.height += 200; resize(box);
    expect(g.top).toBe(900);
    expect(pill()?.textContent).toMatch(/^\d+ new$/);
  });

  it("holds a scrollbar press during streaming, keeps pinning through a click or text selection, and ignores a nested box's wheel", async () => {
    const { box, g, pill } = await feedOf(f);
    const row = box.firstElementChild!.lastElementChild ?? box.firstElementChild!;
    // A press in the rows (a click, the start of a selection) is not a scroll: the end stays pinned and letting go moves nothing.
    row.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, clientX: 100, clientY: 300 }));
    g.height += 100; resize(box); expect(g.top).toBe(g.height - 400);
    document.dispatchEvent(new MouseEvent("pointerup", { bubbles: true })); await settle();
    expect(g.top).toBe(g.height - 400); expect(pill()).toBeNull();
    // A wheel over a command's output that still scrolls up moves that box, not the feed.
    const out = document.createElement("div"); out.style.overflowY = "auto";
    Object.defineProperties(out, { scrollHeight: { get: () => 600 }, clientHeight: { get: () => 280 }, scrollTop: { get: () => 120 } });
    row.append(out);
    wheelUp(out); g.height += 100; resize(box);
    expect(g.top).toBe(g.height - 400); expect(pill()).toBeNull();
    // A press on the feed's scrollbar holds the feed still while content streams, and letting go away from the end leaves it there.
    const before = g.top;
    box.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, clientX: 395, clientY: 300 }));
    g.height += 100; resize(box); expect(g.top).toBe(before);
    document.dispatchEvent(new MouseEvent("pointerup", { bubbles: true })); await settle();
    expect(g.top).toBe(before); expect(pill()).not.toBeNull();
  });

  it("does not stop following for a wheel in a feed shorter than its box", async () => {
    const { box, g, pill } = await feedOf(f);
    g.height = 300; resize(box);
    wheelUp(box); scrolled(box); await settle();
    expect(pill()).toBeNull();
    g.height = 2000; resize(box); expect(g.top).toBe(1600);
  });
});

describe.each(SENDERS)("$name: the owner's message", f => {
  it("sent from this screen brings the end back and turns following on", async () => {
    const { box, g, pill, away } = await feedOf(f);
    await away();
    expect(pill()).not.toBeNull();
    await send(box, "Add a test for the edge case");
    expect(g.top).toBe(g.height - 400); expect(pill()).toBeNull();
    g.height += 80; resize(box); expect(g.top).toBe(g.height - 400);
  });
});

describe.each(ANSWERERS)("$name: an answer", f => {
  it("given here brings the end back and turns following on", async () => {
    const { p, box, g, pill, away } = await feedOf(f, { asking: true });
    await away(300);
    expect(pill()).not.toBeNull();
    box.querySelector<HTMLElement>(".dc .opt")!.click(); await settle();
    box.querySelector<HTMLElement>('.dc [aria-label="Send answer"]')!.click(); await settle();
    expect(p.commands.at(-1)!.payload.type).toMatch(/^answer_(decision|planning)$/);
    expect(g.top).toBe(g.height - 400); expect(pill()).toBeNull();
  });
});

it("keeps the place Answer scrolled to: the following feed does not take it back", async () => {
  const { box, g, pill } = await feedOf(FEEDS[0], { asking: true });
  const card = box.querySelector<HTMLElement>(".dc")!;
  card.scrollIntoView = () => { g.top = 700; };
  [...document.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent?.trim() === "Answer")!.click(); await settle();
  scrolled(box); await settle();
  expect(g.top).toBe(700); expect(pill()?.textContent).toBe("Latest");
  // Later, with no input, content grows and the engine reports a scroll: the reader is still at the card.
  await new Promise(r => setTimeout(r, 20)); g.height += 150; resize(box); scrolled(box); await settle();
  expect(g.top).toBe(700);
});

// ---------- The controller itself ----------

/** A bare feed box for the controller, laid out by hand. */
function bareFeed(height = 2000) {
  const box = document.createElement("div"), inner = document.createElement("div"), row = document.createElement("div");
  inner.append(row); box.append(inner); document.body.append(box);
  const g = layout(box, height);
  return { box, inner, row, g };
}

it("reads Latest without missed items and \"N new\" with them", async () => {
  const { box, g } = bareFeed(), controls = document.createElement("div"); document.body.append(controls);
  const [count, setCount] = createSignal(1);
  const follow = createRoot(d => { dispose = d; return createFollow(() => box, count); });
  const unmount = render(() => <Pill follow={follow} />, controls);
  try {
    await flush();
    wheelUp(box); g.top = 500; follow.onScroll(); await flush();
    expect(controls.textContent).toBe("Latest");
    setCount(3); await flush();
    expect(controls.textContent).toBe("2 new");
    controls.querySelector("button")!.click(); await flush();
    expect(controls.textContent).toBe(""); expect(g.top).toBe(1600);
  } finally { unmount(); }
});

it("pins streaming growth, preserves the reading anchor, and never writes during a scrollbar drag", async () => {
  const box = document.createElement("div"), inner = document.createElement("div"), row = document.createElement("div"); inner.append(row); box.append(inner); document.body.append(box);
  let height = 1000, top = 600, rowY = 0;
  Object.defineProperties(box, { clientHeight: { get: () => 400 }, clientWidth: { get: () => 385 }, scrollHeight: { get: () => height }, scrollTop: { get: () => top, set: (v: number) => { top = v; } } });
  box.getBoundingClientRect = () => ({ top: 0, bottom: 400, left: 0 }) as DOMRect;
  row.getBoundingClientRect = () => ({ top: rowY, bottom: rowY + 100 }) as DOMRect;
  const [n, setN] = createSignal(1);
  const follow = createRoot(d => { dispose = d; return createFollow(() => box, n); });
  await flush();
  height = 1100; resize(box);
  expect(top).toBe(700);
  wheelUp(box); top = 200; follow.onScroll(); expect(follow.following()).toBe(false);
  height = 1200; setN(2); resize(box); expect(top).toBe(200);
  rowY = 50; resize(box); expect(top).toBe(250);
  box.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, clientX: 395 }));
  height = 1300; rowY = 150; resize(box); expect(top).toBe(250);
  top = 900; follow.onScroll(); document.dispatchEvent(new MouseEvent("pointerup", { bubbles: true }));
  expect(follow.following()).toBe(true);
  height = 1400; resize(box); expect(top).toBe(1000);
});

it("takes a scrollbar press while following as holding the feed: nothing is written, and letting go away from the end does not jump back", async () => {
  let now = 10_000;
  vi.spyOn(performance, "now").mockImplementation(() => now);
  const { box, g } = bareFeed(), [n, setN] = createSignal(1);
  const follow = createRoot(d => { dispose = d; return createFollow(() => box, n); });
  await flush();
  expect(g.top).toBe(1600);
  box.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, clientX: 395 }));
  g.height += 200; setN(2); resize(box); await flush();
  expect(g.top).toBe(1600);
  now += 5000;
  document.dispatchEvent(new MouseEvent("pointerup", { bubbles: true }));
  expect(g.top).toBe(1600); expect(follow.following()).toBe(false); expect(follow.missed()).toBe(1);
  // A release the document did not see ends the hold at the next pointer move, and a later growth still does not move the reader.
  box.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, clientX: 395 }));
  document.dispatchEvent(new MouseEvent("pointermove", { bubbles: true, buttons: 0 }));
  g.height += 100; resize(box); expect(g.top).toBe(1600);
  // Letting go at the end follows again.
  box.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, clientX: 395 }));
  g.top = g.height - 400; follow.onScroll();
  window.dispatchEvent(new Event("blur"));
  expect(follow.following()).toBe(true);
  g.height += 100; resize(box); expect(g.top).toBe(g.height - 400);
});

it("keeps pinning through a press in the rows, and a focus that press gave is not the reader's move", async () => {
  let now = 10_000;
  vi.spyOn(performance, "now").mockImplementation(() => now);
  const { box, row, g } = bareFeed();
  const follow = createRoot(d => { dispose = d; return createFollow(() => box, () => 1); });
  await flush();
  const button = document.createElement("button"); row.append(button);
  button.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, clientX: 100, clientY: 300 }));
  button.dispatchEvent(new FocusEvent("focusin", { bubbles: true }));
  g.height += 100; resize(box); expect(g.top).toBe(1700);
  // The engine moves scrollTop by itself while the press is held: still the end that is followed.
  g.top = 1680; follow.onScroll(); expect(g.top).toBe(1700); expect(follow.following()).toBe(true);
  document.dispatchEvent(new MouseEvent("pointerup", { bubbles: true }));
  g.height += 100; resize(box); expect(g.top).toBe(1800); expect(follow.following()).toBe(true);
  // Selecting text past the feed's top edge scrolls it up: that is the reader's.
  row.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, clientX: 100, clientY: 200 }));
  document.dispatchEvent(new MouseEvent("pointermove", { bubbles: true, buttons: 1, clientY: -20 }));
  g.top = 1500; follow.onScroll();
  expect(g.top).toBe(1500); expect(follow.following()).toBe(false);
});

it("stops following for a wheel or touch that moves the feed, never for one a nested box takes, nor in a short feed", async () => {
  const { box, row, g } = bareFeed();
  const follow = createRoot(d => { dispose = d; return createFollow(() => box, () => 1); });
  await flush();
  const out = document.createElement("div"); out.style.overflowY = "auto"; row.append(out);
  let outTop = 50;
  Object.defineProperties(out, { scrollHeight: { get: () => 600 }, clientHeight: { get: () => 280 }, scrollTop: { get: () => outTop } });
  wheelUp(out); expect(follow.following()).toBe(true);
  const touch = (target: Element, y: number, type: string) => target.dispatchEvent(Object.assign(new Event(type, { bubbles: true }), { touches: [{ clientY: y }] }));
  touch(out, 100, "touchstart"); touch(out, 160, "touchmove"); expect(follow.following()).toBe(true);
  // At its top, the nested box hands the move on to the feed.
  outTop = 0;
  wheelUp(out); expect(follow.following()).toBe(false);
  g.top = 1600; follow.onScroll(); expect(follow.following()).toBe(true);
  touch(row, 100, "touchstart"); touch(row, 160, "touchmove"); expect(follow.following()).toBe(false);
  g.top = 1600; follow.onScroll(); expect(follow.following()).toBe(true);
  // Keys move the feed only when it can move.
  box.dispatchEvent(new KeyboardEvent("keydown", { key: "PageUp", bubbles: true })); expect(follow.following()).toBe(false);
  g.top = 1600; follow.onScroll();
  g.height = 300; resize(box);
  wheelUp(box); touch(row, 100, "touchstart"); touch(row, 160, "touchmove");
  box.dispatchEvent(new KeyboardEvent("keydown", { key: "PageUp", bubbles: true }));
  expect(follow.following()).toBe(true);
});

it("brings a card into view through the feed's controller, and the end does not take it back", async () => {
  let now = 10_000;
  vi.spyOn(performance, "now").mockImplementation(() => now);
  const { box, row, g } = bareFeed();
  const follow = createRoot(d => { dispose = d; return createFollow(() => box, () => 1); });
  await flush();
  const card = document.createElement("div"); row.append(card);
  const into = vi.fn(() => { g.top = 600; });
  card.scrollIntoView = into;
  scrollIntoFeed(card);
  expect(into).toHaveBeenCalledWith({ block: "center" });
  expect(follow.following()).toBe(false);
  now += 5000; follow.onScroll(); g.height += 100; resize(box);
  expect(g.top).toBe(600);
  // A card already at the end leaves the feed following.
  card.scrollIntoView = () => { g.top = g.height - 400; };
  scrollIntoFeed(card); expect(follow.following()).toBe(true);
  // Outside any feed it is an ordinary scroll into view.
  const loose = document.createElement("div"); document.body.append(loose);
  loose.scrollIntoView = vi.fn(); scrollIntoFeed(loose, "start");
  expect(loose.scrollIntoView).toHaveBeenCalledWith({ block: "start" });
});

/** A toggle in `row` that opens its block; `at` is where the opened block lies in the box, before the end is pinned again. */
function foldIn(row: Element, at: { top: number; bottom: number }) {
  const toggle = document.createElement("button"); toggle.setAttribute("aria-expanded", "false"); row.append(toggle);
  toggle.addEventListener("click", () => toggle.setAttribute("aria-expanded", "true"));
  toggle.getBoundingClientRect = () => ({ ...at, left: 0, right: 385, width: 385, height: at.bottom - at.top, x: 0, y: at.top }) as DOMRect;
  return toggle;
}

it("keeps a row the reader opens in view in any feed", async () => {
  const { box, row, g } = bareFeed();
  const follow = createRoot(d => { dispose = d; return createFollow(() => box, () => 1); });
  await flush();
  // Opened at the end, its block runs past the bottom of the 400 px box: reading it means leaving the end.
  foldIn(row, { top: 300, bottom: 700 }).click(); await flush();
  expect(follow.following()).toBe(false);
  g.height += 300; resize(box); expect(g.top).toBe(1600);
});

it("keeps following when the opened block is wholly visible in a short feed, and pins the end as it grows", async () => {
  // A 300 px feed in a 400 px box: there is nowhere to scroll, so no scroll event could turn following back on.
  const { box, row, g } = bareFeed(300), controls = document.createElement("div"); document.body.append(controls);
  const follow = createRoot(d => { dispose = d; return createFollow(() => box, () => 1); });
  const unmount = render(() => <Pill follow={follow} />, controls);
  try {
    await flush();
    foldIn(row, { top: 200, bottom: 340 }).click(); await flush();
    expect(follow.following()).toBe(true);
    expect(controls.querySelector(".new-pill")).toBeNull();
    expect(g.top).toBe(0);
    // Streamed rows arrive: the feed outgrows its box and the end stays pinned.
    g.height = 900; resize(box); expect(g.top).toBe(500);
    g.height = 1300; resize(box); expect(g.top).toBe(900);
    expect(follow.following()).toBe(true);
    expect(follow.missed()).toBe(0);
  } finally { unmount(); }
});

it("keeps following when the opened block fits above the pinned end of a long feed", async () => {
  const { box, row, g } = bareFeed();
  const follow = createRoot(d => { dispose = d; return createFollow(() => box, () => 1); });
  await flush();
  expect(g.top).toBe(1600);
  // The block opened 100 px of content below the old end; pinned again, it lies at 150..380 in the 400 px box.
  g.height += 100;
  foldIn(row, { top: 250, bottom: 480 }).click(); await flush();
  expect(follow.following()).toBe(true);
  expect(g.top).toBe(1700);
  g.height += 200; resize(box); expect(g.top).toBe(1900);
  // A block that would no longer fit once the end is pinned still stops following: it moves up only until its end shows,
  // and later growth leaves it where the reader can read it.
  g.height += 300;
  foldIn(row, { top: 100, bottom: 500 }).click(); await flush();
  expect(follow.following()).toBe(false);
  expect(g.top).toBe(2000);
  g.height += 200; resize(box); expect(g.top).toBe(2000);
});

it("waits for earlier rows before restoring a paused worker's offset", async () => {
  const box = document.createElement("div");
  box.append(document.createElement("div")); document.body.append(box);
  let top = 0, height = 500;
  Object.defineProperties(box, {
    clientHeight: { get: () => 400 }, scrollHeight: { get: () => height },
    scrollTop: { get: () => top, set: (v: number) => { top = Math.min(v, height - 400); } },
  });
  const [ready, setReady] = createSignal(false);
  const position = { top: 700 };
  const follow = createRoot(d => { dispose = d; return createFollow(() => box, () => 0, { position, ready }); });
  await flush();
  resize(box);
  expect(top).toBe(0);
  expect(follow.following()).toBe(false);

  height = 1400;
  setReady(true); await flush();
  expect(top).toBe(700);
  expect(follow.following()).toBe(false);
});

// Rendered in WebKitGTK: a background read replaced rows inside the result card at the end, and the engine moved scrollTop up by
// itself (16 px, no height change, no script write). Taken as the reader scrolling up, following stopped and Latest stayed.
it("pins the end again when the engine moves the scroll position without reader input, and leaves it only for the reader", async () => {
  const box = document.createElement("div"), inner = document.createElement("div"); box.append(inner); document.body.append(box);
  let height = 2000, top = 0, now = 10_000;
  Object.defineProperties(box, { clientHeight: { get: () => 500 }, scrollHeight: { get: () => height }, scrollTop: { get: () => top, set: (v: number) => { top = Math.max(0, Math.min(v, height - 500)); } } });
  box.getBoundingClientRect = () => ({ top: 0, bottom: 500 }) as DOMRect;
  vi.spyOn(performance, "now").mockImplementation(() => now);
  const follow = createRoot(d => { dispose = d; return createFollow(() => box, () => 1); });
  await flush();
  expect(top).toBe(1500);
  // The engine's own move: same height, no input.
  top = 1484; follow.onScroll();
  expect(top).toBe(1500); expect(follow.following()).toBe(true);
  // Content that grew and shrank in one update, clamping scrollTop first.
  top = 1392; height = 2028; follow.onScroll();
  expect(top).toBe(1528); expect(follow.following()).toBe(true);
  resize(box); expect(top).toBe(1528);
  // A smooth wheel scroll up whose first step still lies within the end follows again, then leaves it as the reader goes on.
  wheelUp(box);
  top = 1527; follow.onScroll(); expect(follow.following()).toBe(true);
  now += 40; top = 1460; follow.onScroll(); expect(follow.following()).toBe(false);
  // Reaching the end by any means follows again; a later engine move keeps it there.
  now += 5000; top = 1528; follow.onScroll(); expect(follow.following()).toBe(true);
  top = 1500; follow.onScroll(); expect(top).toBe(1528); expect(follow.following()).toBe(true);
  // Focus moved by the keyboard onto a control above is the reader's move.
  inner.dispatchEvent(new FocusEvent("focusin", { bubbles: true }));
  top = 900; follow.onScroll(); expect(top).toBe(900); expect(follow.following()).toBe(false);
  // Back at the end, following again.
  now += 5000; top = 1528; follow.onScroll(); expect(follow.following()).toBe(true);
});

// The pill's band showed in the dock, whose height is the feed's bottom clearance: each time Latest came or went the end moved
// by the band's height, so the reader who had just reached it was no longer there.
it("lays the follow pill's band over the feed above the dock, out of the dock's flow and clear of the feed's scrollbar", () => {
  const css = readFileSync(join(__dirname, "..", "src", "workspace.css"), "utf8");
  expect(css).toMatch(/\.dock > \.pill-band, \.dock-w > \.pill-band \{[^}]*position: absolute;[^}]*bottom: 100%;[^}]*right: calc\(11px \+ var\(--ws-edge\)\)/);
});

// The dock lay over the feed's bottom part, with its measured height as the feed's bottom padding: the feed's scrollbar ran on
// beside the composer. The dock is now the feed's flow sibling below it, so the feed's box and scrollbar end at the dock's top.
it("puts each dock below its feed in the panel's column, with no measured clearance", () => {
  const read = (name: string) => readFileSync(join(__dirname, "..", "src", name), "utf8");
  const css = read("mockup-styles.css") + read("workspace.css");
  for (const dock of ["dock", "dock-w"]) {
    const rule = css.match(new RegExp(`\\n\\.${dock} \\{([^}]*)\\}`))![1];
    expect(rule).toContain("flex: none"); expect(rule).not.toMatch(/position: absolute|bottom: 0/);
  }
  expect(css).not.toMatch(/dock-clear|\.dock(-w)?::before/);
  for (const file of ["session.tsx", "sprint.tsx", "worker.tsx", "composer.tsx", "follow.ts"]) expect(read(file)).not.toMatch(/dock-clear|reportHeight|MutationObserver/);
  // No feed is a reverse-flex box any more, and nothing keys a style off a follow mark.
  expect(css).not.toMatch(/\.(timeline|stream|feed) \{[^}]*column-reverse/);
  expect(css).not.toMatch(/data-follow=/);
});

it("keeps the end, or the reader's row, when the dock below resizes the feed", async () => {
  const box = document.createElement("div"), inner = document.createElement("div"), row = document.createElement("div"); inner.append(row); box.append(inner); document.body.append(box);
  let client = 500, top = 0;
  Object.defineProperties(box, { clientHeight: { get: () => client }, scrollHeight: { get: () => 2000 }, scrollTop: { get: () => top, set: (v: number) => { top = Math.max(0, Math.min(v, 2000 - client)); } } });
  box.getBoundingClientRect = () => ({ top: 0, bottom: client }) as DOMRect;
  row.getBoundingClientRect = () => ({ top: 10, bottom: 110 }) as DOMRect;
  const follow = createRoot(d => { dispose = d; return createFollow(() => box, () => 1); });
  await flush();
  expect(top).toBe(1500);
  // The composer grows a line, the workers bar appears, a queued row comes: the feed is shorter and its end stays in view.
  client = 420; resize(box); expect(top).toBe(1580); expect(follow.following()).toBe(true);
  // The dock shrinks back: the engine clamps scrollTop, which is no reader input, and the end stays pinned.
  client = 500; top = 1500; follow.onScroll(); resize(box); expect(top).toBe(1500); expect(follow.following()).toBe(true);
  // A reader who scrolled up keeps their row where it was whichever way the dock resizes.
  wheelUp(box); top = 600; follow.onScroll();
  client = 380; resize(box); expect(top).toBe(600); client = 520; resize(box); expect(top).toBe(600); expect(follow.following()).toBe(false);
});

describe("worker activity pager", () => {
  const withCursor = (o: Shape = {}) => { const s = taskWorld(o); s.workers[0].activityCursor = "page-1"; return s; };
  it("reads nothing while scrolling in the middle, reads once on approaching the top, and keeps the reader's row in place", async () => {
    let reads = 0, release!: () => void;
    const gate = new Promise<void>(resolve => { release = resolve; });
    const f: Feed = { name: "worker", path: `/m/${M}/t/t/workers/w`, feed: ".livepane .stream", world: withCursor,
      prepare: p => { p.port.readWorkerActivity = async () => { reads++; await gate; return { activity: notes(-10, 10), activityCursor: null, messages: [] }; }; } };
    const { box, g, away } = await feedOf(f);
    // The middle of the feed: 1,400 px from the top, well outside the reach.
    await away(1400);
    for (let y = 1400; y > 900; y -= 25) { g.top = y; scrolled(box); }
    for (let y = 900; y < 1500; y += 25) { g.top = y; scrolled(box); }
    resize(box); await settle();
    expect(reads).toBe(0);
    // Approaching the top: one read for the whole approach, however many scroll events.
    for (let y = 1400; y > 300; y -= 50) { g.top = y; scrolled(box); }
    await settle();
    expect(reads).toBe(1);
    for (let y = 300; y > 100; y -= 10) { g.top = y; scrolled(box); }
    resize(box); await settle();
    expect(reads).toBe(1);
    const row = box.querySelector(".stream-in")!.lastElementChild!;
    release(); await settle();
    expect(reads).toBe(1);
    expect(row.isConnected).toBe(true);
  });
});
