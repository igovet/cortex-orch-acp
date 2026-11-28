import { render } from "solid-js/web";
import { createRoot, createSignal } from "solid-js";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Decision as WireDecision } from "@cortex/workspace-contract";
import { emptyConfig, emptySession, emptyWorkspace, LOCAL_MACHINE_ID, type PortUpdate, type ProjectFacts, type Worker, type WorkspacePort } from "@cortex/workspace-domain";
import { mountPhone, Workspace, type PhoneShell } from "../src";
import { createRealPort } from "../src/port/real";
import { readBoards, writeBoards } from "../src/port/board-cache";
import { createFirstView, firstWindowEntries, type FirstRead } from "../src/first-view";
import { capabilitiesWire, flush, projection, scriptedPort, scriptedTransport, wireDecision, wireTask, withTask } from "./support";

/** A read held until the test lets it go. */
function hold() { let release!: () => void; const done = new Promise<void>(resolve => { release = resolve; }); return { done, release }; }
const at = (n: number) => new Date(Date.UTC(2026, 10, 20, 10) + n * 1000).toISOString();
const baseline = "1".repeat(64), newer = "2".repeat(64);
const FORMER_FIRST_VIEW_MS = 5_000;
const position = (n: number) => String(9007199254740992n + BigInt(n));

/**
 * A native machine with `count` running tasks. Each conversation holds an owner message, an answered question and the
 * coordinator's reply. Its three first reads (transcript, questions, the facts with its workers) can each be held.
 */
function machine(count = 1, options: { open?: boolean } = {}) {
  const tasks = Array.from({ length: count }, (_, i) => wireTask({ taskId: `task-${i + 1}`, description: `Task ${i + 1}`, version: "1", executionId: `exec-${i + 1}`,
    phase: "execution", executionState: "running", runtimeState: "EXECUTING" }));
  const row = (task: string, n: number, kind: string, text: string) => ({ historyId: `${task}-h${n}`, sequence: position(n), revision: "1", kind, text, toolStatus: null, recordedAt: at(9 - n), availability: "available" });
  const rows = (task: string) => [row(task, 0, "user", `Owner words for ${task}`),
    row(task, 1, "assistant", `Before the question for ${task}.`), row(task, 2, "assistant", `Coordinator reply for ${task}.`)];
  const question = (exec: string) => options.open
    ? wireDecision({ decisionId: `d-${exec}`, executionId: exec, prompt: `Which branch for ${exec}?`, createdAt: at(2), conversationSequence: position(1), conversationTieBreaker: "01" } as Partial<WireDecision>)
    : wireDecision({ decisionId: `d-${exec}`, executionId: exec, prompt: `Which branch for ${exec}?`, status: "answered", answerText: "main",
      createdAt: at(2), answeredAt: at(2), conversationSequence: position(1), conversationTieBreaker: "01", allowedOperations: [] } as Partial<WireDecision>);
  const holds: { history?: Promise<void>; decisions?: Promise<void>; facts?: Promise<void>; board?: Promise<void> } = {};
  const snapshot = { history: baseline as string | undefined, decisions: baseline };
  const decisionCalls: string[] = [];
  const envelope = (data: unknown) => ({ protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "g", version: "2", highWatermark: "3", data });
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["native.query", "native.board"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Project", canonicalRoot: "/p", activation: {} }],
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer", "history.page"], 12),
    "native.board": async () => { await holds.board; return envelope({ type: "board", value: { items: tasks, nextCursor: null, highWatermark: "3" } }); },
    "native.query": async raw => {
      const { operation, payload } = raw as { operation: string; payload: { target: string; cursor: string | null } };
      if (operation === "history.earlier") {
        await holds.history;
        return envelope({ type: "history", value: { items: rows(payload.target), nextCursor: null, highWatermark: "2", tailCursor: "nt1.1", ...(snapshot.history === undefined ? {} : { decisionRevision: snapshot.history }) } });
      }
      return envelope({ type: "history", value: { items: [], nextCursor: null, highWatermark: "2", tailCursor: "nt1.1", ...(snapshot.history === undefined ? {} : { decisionRevision: snapshot.history }) } });
    },
    "decisions.list": async raw => {
      const target = (raw as { target: string }).target;
      decisionCalls.push(target);
      await holds.decisions;
      await new Promise(resolve => setTimeout(resolve, 2));
      return { items: [question(target)], nextCursor: null, highWatermark: "1", decisionRevision: snapshot.decisions };
    },
    "machine.call": async raw => {
      const op = (raw as { op: string }).op;
      if (op === "overview") return { ok: true, result: { name: "This machine" } };
      if (op === "project.config.read") return { ok: true, result: { value: {}, tokens: {} } };
      if (op === "config.read") return { ok: true, result: { machineName: "Local", config: emptyConfig(), connections: [], tokens: {} } };
      if (op === "workers") { await holds.facts; return { ok: true, result: [] }; }
      return { ok: true, result: op === "repo.snapshot" ? null : { tasks: {}, sprints: [] } };
    },
  });
  return { s, holds, decisionCalls, snapshot, tasks, question };
}

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; vi.useRealTimers(); vi.unstubAllGlobals(); vi.restoreAllMocks(); delete document.documentElement.dataset.shell; history.replaceState(null, "", "/"); });
const settle = async (n = 12) => { for (let i = 0; i < n; i++) await flush(); };
const feed = () => document.querySelector<HTMLElement>(".task-panel .timeline");

/** Opens a task's conversation and records every state of the feed it ever showed. */
function open(svc: ReturnType<typeof machine>, task = "task-1", boardStorage?: Storage, updates?: PortUpdate[]) {
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/${task}`);
  const root = document.createElement("div"); document.body.append(root);
  const seen: string[] = [];
  const watch = new MutationObserver(() => { const f = feed(); if (f && !f.querySelector(".ws-skel")) seen.push(f.textContent ?? ""); });
  watch.observe(root, { childList: true, subtree: true, characterData: true });
  const port = createRealPort({ transport: svc.s.transport, actorId: "owner", registry: true, boardStorage });
  if (updates) {
    const subscribe = port.subscribe;
    port.subscribe = notify => subscribe(update => { updates.push(update); notify(update); });
  }
  dispose = render(() => <Workspace port={port} />, root);
  return { root, seen, stop: () => watch.disconnect() };
}

it("shows the saved machine name on the task screen before the board finishes and without opening Settings", async () => {
  const svc = machine();
  const wait = hold();
  svc.holds.facts = wait.done;
  const { root, stop } = open(svc);
  try {
    await vi.waitFor(() => expect(root.querySelector(".titlebar .machine .trunc")?.textContent).toBe("Local"));
  } finally { wait.release(); stop(); }
});

it.each(["history", "decisions", "facts"] as const)("shows the conversation skeleton until its %s read is in, then the whole conversation at once", async late => {
  const svc = machine();
  const wait = hold();
  svc.holds[late] = wait.done;
  const { root, seen, stop } = open(svc);
  await vi.waitFor(() => expect(feed()?.querySelector(".ws-skel")).not.toBeNull(), { timeout: 4000 });
  await settle();
  // The two other reads have arrived meanwhile; nothing of them shows on its own.
  const busy = feed()!;
  expect(busy.getAttribute("aria-busy")).toBe("true");
  expect(busy.querySelector(".ws-skel [role=status]")?.textContent).toBe("Loading the conversation…");
  // Loading shows only the skeleton: the words are for screen readers (ws-sr), and nothing else in the feed carries text.
  expect(busy.querySelector(".ws-skel [role=status]")?.classList.contains("ws-sr")).toBe(true);
  const visible = [...busy.querySelectorAll(".ws-skel *")].filter(el => !el.closest(".ws-sr")).flatMap(el => [...el.childNodes].filter(n => n.nodeType === 3 && n.textContent!.trim()));
  expect(visible).toEqual([]);
  expect(busy.querySelector(".ws-skel-rows")?.getAttribute("aria-hidden")).toBe("true");
  expect(busy.textContent).not.toContain("Which branch");
  expect(busy.textContent).not.toContain("Owner words");
  expect(root.querySelector(".dock .ws-team")).toBeNull();
  expect(seen).toEqual([]);
  wait.release();
  await vi.waitFor(() => expect(feed()?.textContent).toContain("Coordinator reply for task-1."), { timeout: 4000 });
  stop();
  // One reveal: every state the feed ever showed already held the transcript and its answered question together.
  expect(seen.length).toBeGreaterThan(0);
  for (const text of seen) {
    expect(text).toContain("Owner words for task-1");
    const before = text.indexOf("Before the question for task-1.");
    const card = text.indexOf("Which branch for exec-1?");
    expect(before).toBeGreaterThanOrEqual(0);
    expect(card).toBeGreaterThan(before);
    expect(text.indexOf("Coordinator reply for task-1.")).toBeGreaterThan(card);
  }
  expect(feed()!.hasAttribute("aria-busy")).toBe(false);
  expect(root.querySelector(".ws-first-notice")).toBeNull();
});

it("does not reveal a cached-board conversation or combine mismatched native revisions", async () => {
  const svc = machine(1, { open: true });
  const board = hold(), history = hold();
  svc.holds.board = board.done;
  svc.holds.history = history.done;
  svc.snapshot.history = newer;
  writeBoards(localStorage, "owner", [{ project: { projectId: "p", name: "Project", repository: "/p",
    openBranch: "", defaultBranch: "", activated: true }, items: svc.tasks, product: { tasks: {}, sprints: [] } }]);
  expect(readBoards(localStorage, "owner")[0]?.items[0]?.executionId).toBe("exec-1");
  const { root, seen, stop } = open(svc, "task-1", localStorage);
  await vi.waitFor(() => expect(feed()?.getAttribute("aria-busy"), root.textContent ?? "").toBe("true"));
  expect(seen).toEqual([]);
  board.release();
  await vi.waitFor(() => expect(svc.decisionCalls).toContain("exec-1"));
  expect(feed()?.textContent).not.toContain("Which branch");
  history.release();
  // The first returned decision revision is not the transcript revision. Wait for a restarted history read.
  await vi.waitFor(() => expect(svc.s.calls.filter(c => c.operation === "native.query" && (c.message as { operation: string }).operation === "history.earlier").length).toBeGreaterThanOrEqual(2));
  expect(feed()?.getAttribute("aria-busy")).toBe("true");
  expect(seen).toEqual([]);
  svc.snapshot.decisions = newer;
  await vi.waitFor(() => expect(feed()?.textContent).toContain("Coordinator reply for task-1."), {timeout:4000});
  stop();
  expect(seen.length).toBeGreaterThan(0);
  for (const text of seen) {
    // A pending question stays actionable below the transcript, including its latest reply.
    expect(text).toMatch(/Before the question for task-1\.[\s\S]*Coordinator reply for task-1\.[\s\S]*Which branch for exec-1\?/);
  }
});

it("uses a running conversation's confirmed revision before asking for its board decisions again", async () => {
  const options = { open: false };
  const svc = machine(1, options);
  const initialHistory = svc.s.answers["native.query"]!;
  let version = 1;
  svc.s.answers["native.query"] = raw => {
    if ((raw as { operation: string }).operation !== "history.page") return initialHistory(raw);
    const value = { items: [{ historyId: "fragment-" + version, sequence: position(version + 1), revision: "1",
      kind: "assistant", text: "Streamed fragment " + version + ".", toolStatus: null, recordedAt: at(version + 10), availability: "available" }],
      nextCursor: null, highWatermark: "2", tailCursor: "nt1." + version, decisionRevision: svc.snapshot.history };
    return { protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "g", version: "2", highWatermark: String(version + 2),
      data: { type: "history", value } };
  };
  const changed = () => {
    const base = String(version++);
    const versionNow = String(version), sequence = String(version + 2);
    svc.s.send({ type: "native_delta", projectId: "p", batch: { generation: "g", streamKey: { scope: "project", project_id: "p" },
      fromSequence: sequence, throughSequence: sequence, changes: [{ kind: "task", id: "task-1", baseVersion: base, newVersion: versionNow,
        operation: { type: "upsert", data: { kind: "task_projection", data: { ...svc.tasks[0], version: versionNow } } } }] } });
  };
  const updates: PortUpdate[] = [];
  const { stop } = open(svc, "task-1", undefined, updates);
  try {
    await vi.waitFor(() => expect(feed()?.textContent).toContain("Coordinator reply for task-1."));
    await settle();
    const before = svc.decisionCalls.length;
    const observed = updates.length;
    changed();
    await vi.waitFor(() => expect(feed()?.textContent).toContain("Streamed fragment 2."));
    await settle();
    expect(svc.decisionCalls).toHaveLength(before);
    expect(updates.slice(observed).find(update => update.kind === "conversation")).toMatchObject({
      projectId: "p", executionId: "exec-1", version: "4", conversation: { taskId: "task-1", historyRead: undefined },
    });

    options.open = true;
    svc.snapshot.history = svc.snapshot.decisions = newer;
    changed();
    await vi.waitFor(() => expect(feed()?.textContent).toContain("Streamed fragment 3."));
    // The verified conversation's cards also update the board at the covered watermark.
    await vi.waitFor(() => expect(updates.some(update => update.kind === "enrichment" && update.update.version === "5"
      && JSON.stringify(update.update).includes("Which branch for exec-1?"))).toBe(true));
    expect(feed()?.querySelector(".dc")?.textContent).toContain("Which branch for exec-1?");
    await settle();
    expect(svc.decisionCalls).toHaveLength(before + 1);

    const confirmed = svc.decisionCalls.length;
    svc.snapshot.history = undefined;
    changed();
    await vi.waitFor(() => expect(svc.decisionCalls.length).toBeGreaterThan(confirmed));
    expect(feed()?.textContent).not.toContain("Streamed fragment 4.");
    expect(feed()?.textContent).toContain("Streamed fragment 3.");
  } finally { stop(); }
});

it("restarts a stale decision walk without showing cards from its partial pages", async () => {
  const svc = machine();
  const finish = hold();
  let refused = false;
  svc.s.answers["decisions.list"] = async raw => {
    const { cursor, target } = raw as { cursor: string | null; target: string };
    if (cursor === "stale") { refused = true; throw new Error("invalid_cursor"); }
    const decision = svc.question(target);
    if (!refused) return { items: [{ ...decision, prompt: "Stale page card" }], nextCursor: "stale", highWatermark: "1", decisionRevision: baseline };
    await finish.done;
    return { items: [decision], nextCursor: null, highWatermark: "2", decisionRevision: baseline };
  };
  const { seen, stop } = open(svc);
  await vi.waitFor(() => expect(refused).toBe(true));
  expect(feed()?.getAttribute("aria-busy")).toBe("true");
  expect(seen).toEqual([]);
  finish.release();
  await vi.waitFor(() => expect(feed()?.textContent).toContain("Which branch for exec-1?"));
  stop();
  expect(seen.length).toBeGreaterThan(0);
  expect(seen.every(text => !text.includes("Stale page card") && text.includes("Coordinator reply for task-1."))).toBe(true);
});

it("does not settle a 1.12 feed whose history omitted the decision revision", async () => {
  const svc = machine();
  svc.snapshot.history = undefined;
  const { stop } = open(svc);
  await vi.waitFor(() => expect(feed()?.querySelector('[role="alert"]')?.textContent).toContain("did not confirm"));
  expect(feed()?.textContent).not.toContain("Coordinator reply");
  expect(feed()?.textContent).not.toContain("Which branch");
  svc.snapshot.history = baseline;
  feed()?.querySelector<HTMLButtonElement>('[role="alert"] button')?.click();
  await vi.waitFor(() => expect(feed()?.textContent).toContain("Coordinator reply for task-1."));
  stop();
});

it("keeps the conversation skeleton, with no error, for as long as a slow read takes, then shows all of it at once", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true, toFake: ["setTimeout", "clearTimeout"] });
  const svc = machine();
  const wait = hold();
  svc.holds.history = wait.done;
  const { root } = open(svc);
  await vi.waitFor(() => expect(feed()?.querySelector(".ws-skel")).not.toBeNull(), { timeout: 4000 });
  await settle();
  // Far past the old five-second budget and every former request deadline: still the skeleton, and nothing says it failed.
  for (const step of [FORMER_FIRST_VIEW_MS + 100, 30_000, 60_000, 120_000]) {
    vi.advanceTimersByTime(step);
    await settle(2);
    expect(feed()?.querySelector(".ws-skel")).not.toBeNull();
    expect(root.querySelector(".ws-first-notice")).toBeNull();
    expect(root.querySelector(".timeline [role=alert], .ws-app > [role=alert]")).toBeNull();
    expect(root.textContent).not.toContain("Couldn’t finish loading");
    expect(root.textContent).not.toContain("could not be read");
    expect(feed()!.textContent).not.toContain("Which branch");
  }
  wait.release();
  await vi.waitFor(() => expect(feed()?.textContent).toContain("Coordinator reply for task-1."), { timeout: 4000 });
  expect(feed()!.textContent).toContain("Which branch for exec-1?");
  expect(root.querySelector(".ws-first-notice")).toBeNull();
});

it("reads the questions of the task on screen first, before the board's other tasks", async () => {
  const svc = machine(3);
  open(svc, "task-3");
  await vi.waitFor(() => expect(feed()?.textContent).toContain("Coordinator reply for task-3."), { timeout: 4000 });
  await vi.waitFor(() => expect(new Set(svc.decisionCalls)).toEqual(new Set(["exec-1", "exec-2", "exec-3"])), { timeout: 4000 });
  // At most the one board read already under way when the task's identity was confirmed comes before it.
  expect(svc.decisionCalls.indexOf("exec-3")).toBeLessThanOrEqual(1);
  expect(svc.decisionCalls.slice(0, svc.decisionCalls.indexOf("exec-3"))).not.toContain("exec-2");
});

it("gates the phone shell's conversation the same way", async () => {
  const svc = machine();
  const wait = hold();
  svc.holds.history = wait.done;
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/task-1`);
  const shell: PhoneShell = {
    state: () => Promise.resolve({ active: "m1", machines: [{ id: "m1", name: "Desk", state: "connected", path: "direct", lastConnectedAt: null }] }),
    subscribe: () => () => {}, scan: () => Promise.resolve(null), pairBegin: () => new Promise(() => {}), pairFinish: () => new Promise(() => {}),
    pairCancel: () => Promise.resolve(), select: () => Promise.resolve(), rename: () => Promise.resolve(), forget: () => Promise.resolve(),
    transport: () => svc.s.transport, retry: () => Promise.resolve(),
    relay: { read: async () => ({ url: "https://relay.example.org", isDefault: true }), write: async () => undefined },
    version: "1.0.6", deviceName: "Pixel 9",
  };
  const root = document.createElement("div"); document.body.append(root);
  const app = mountPhone(root, shell);
  dispose = app.dispose;
  // The phone opens on its machine's Home; the owner then opens the task, as from a notification.
  await vi.waitFor(() => expect(document.body.textContent).toContain("Projects"), { timeout: 4000 });
  expect(document.documentElement.dataset.shell).toBe("phone");
  history.pushState(null, "", `/m/${LOCAL_MACHINE_ID}/t/task-1`);
  window.dispatchEvent(new PopStateEvent("popstate"));
  await vi.waitFor(() => expect(feed()?.querySelector(".ws-skel")).not.toBeNull(), { timeout: 4000 });
  await settle();
  expect(feed()!.querySelector(".ws-skel")).not.toBeNull();
  expect(feed()!.textContent).not.toContain("Which branch");
  wait.release();
  await vi.waitFor(() => expect(feed()?.textContent).toContain("Coordinator reply for task-1."), { timeout: 4000 });
  expect(feed()!.textContent).toContain("Which branch for exec-1?");
});

/** A started task whose session a test sets directly, through a port that offers no reads of its own. */
async function scripted(session: ReturnType<typeof emptySession>, workers: Worker[] = [], path = `/m/${LOCAL_MACHINE_ID}/t/t`, extend?: (port: WorkspacePort) => void) {
  const state = withTask(emptyWorkspace(), "Started task");
  state.machines[0].connection = "connected";
  Object.assign(state.tasks[0], { status: "working", process: "live", executionId: "e" });
  state.sessions = [session]; state.workers = workers;
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  extend?.(p.port);
  history.replaceState(null, "", path);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", structuredClone(state))); await settle();
  return { p, root, state };
}
const said = (id: string, text: string) => ({ id, kind: "said" as const, text });

it("stops the skeleton at once when the machine is offline, and names what is missing", async () => {
  const session = { ...emptySession("t"), historyRead: { state: "loading" as const } };
  const { p, root } = await scripted(session);
  expect(feed()?.querySelector(".ws-skel")).not.toBeNull();
  p.emit({ kind: "connection", connection: { state: "disconnected", reason: null, at: null } });
  await settle(2);
  expect(feed()?.querySelector(".ws-skel")).toBeNull();
  expect(root.querySelector(".ws-first-notice")?.textContent).toContain("Couldn’t finish loading the conversation.");
});

it("keeps a conversation already on screen, its draft and its rows, through a refresh of the same execution", async () => {
  const session = { ...emptySession("t"), timeline: [said("h1", "Earlier words")] };
  const { p, root, state } = await scripted(session);
  expect(feed()?.querySelector(".ws-skel")).toBeNull();
  const row = feed()!.querySelector(".mine")!;
  const box = root.querySelector<HTMLTextAreaElement>(".dock textarea")!;
  box.value = "Half-written"; box.dispatchEvent(new Event("input", { bubbles: true }));
  // The same execution is reread: its transcript reads as loading again for a moment, and the service reconnects.
  state.sessions = [{ ...session, historyRead: { state: "loading" } }];
  p.emit({ kind: "connection", connection: { state: "disconnected", reason: null, at: null } });
  p.emit({ kind: "connection", connection: { state: "connected", reason: null, at: null } });
  p.emit(projection("2", structuredClone(state))); await settle();
  expect(feed()?.querySelector(".ws-skel")).toBeNull();
  expect(feed()!.querySelector(".mine")).toBe(row);
  expect(root.querySelector<HTMLTextAreaElement>(".dock textarea")!.value).toBe("Half-written");
});

it("opens a worker on about one screen of its newest rows and shows earlier ones as the reader scrolls up, without moving the row read", async () => {
  const observers: (() => void)[] = [];
  vi.stubGlobal("ResizeObserver", class { constructor(cb: () => void) { observers.push(cb); } observe() {} disconnect() {} });
  const worker = { id: "w", taskId: "t", executionId: "e", role: "frontend-dev", status: "working", lane: "execution", goal: "Build", now: null, waitsFor: null, scope: [], route: null,
    attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, result: null, activityCursor: null,
    activity: Array.from({ length: 100 }, (_, i) => ({ sequence: i + 1, kind: "note" as const, text: `Observation ${i}`, at: null })) } as Worker;
  await scripted(emptySession("t"), [worker], `/m/${LOCAL_MACHINE_ID}/t/t/workers/w`);
  const box = document.querySelector<HTMLElement>(".livepane .stream")!, inner = box.firstElementChild!;
  const notes = () => [...inner.querySelectorAll(".ev-note")];
  expect(notes()).toHaveLength(firstWindowEntries());
  expect(notes()[0].textContent).toBe(`Observation ${100 - firstWindowEntries()}`);
  // jsdom has no layout: every row is 50 px, the feed 400 px high.
  let top = 0;
  // The reserve above the rows (FollowEdge in follow.ts) is as tall as the follow controller sets it.
  const h = (r: Element) => r.classList.contains("ws-reserve") ? parseFloat((r as HTMLElement).style.height) || 0 : 50;
  const height = () => [...inner.children].reduce((n, r) => n + h(r), 0);
  Object.defineProperties(box, { clientHeight: { get: () => 400 }, scrollHeight: { get: () => height() }, scrollTop: { get: () => top, set: (v: number) => { top = Math.max(0, Math.min(v, height() - 400)); } } });
  const rect = Element.prototype.getBoundingClientRect;
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
    if (this === box) return { top: 0, bottom: 400, height: 400 } as DOMRect;
    if (this.parentElement === inner) { const rows = [...inner.children], y = rows.slice(0, rows.indexOf(this)).reduce((n, r) => n + h(r), 0) - top; return { top: y, bottom: y + h(this), height: h(this) } as DOMRect; }
    return rect.call(this);
  });
  box.dispatchEvent(new WheelEvent("wheel", { deltaY: -100 }));
  top = (parseFloat(inner.querySelector<HTMLElement>(".ws-reserve")?.style.height ?? "") || 0) + 100; box.dispatchEvent(new Event("scroll"));
  const anchor = [...inner.children].find(r => !r.hasAttribute("data-follow-skip") && r.getBoundingClientRect().bottom > 0)!;
  const y = anchor.getBoundingClientRect().top;
  // Drawn in steps over a few frames, into the reserve above.
  await vi.waitFor(() => expect(notes()).toHaveLength(2 * firstWindowEntries()));
  for (const cb of observers) cb();
  expect(anchor.isConnected).toBe(true);
  expect(anchor.getBoundingClientRect().top).toBe(y);
});

/** A worker of a started task, with one line of activity. */
const workerOf = (id: string, taskId: string, executionId: string, role: string) => ({ id, taskId, executionId, role, status: "working", lane: "execution", goal: `Goal of ${role}`, now: null, waitsFor: null,
  scope: [], route: null, attempt: null, elapsedMs: null, runningSince: null, context: null, usage: null, result: null, activityCursor: null,
  activity: [{ sequence: 1, kind: "note" as const, text: `Note of ${role}`, at: null }] }) as Worker;
/** A facts read scoped to the tasks on view, as the real port sends it. */
const scopedFacts = (scope: Record<string, string>, workers: Worker[]) => ({ kind: "facts" as const, facts: { projectId: "p", openBranch: "main", defaultBranch: "main", repo: null, workers, reviews: [],
  overlays: {}, sprints: [], messages: [], workerScope: Object.keys(scope), workerExecutions: scope } as ProjectFacts });

it("keeps a task's confirmed workers when another task's facts arrive, and never carries them into a new execution", async () => {
  const state = withTask(emptyWorkspace(), "Task A");
  state.machines[0].connection = "connected";
  Object.assign(state.tasks[0], { id: "a", status: "working", process: "live", executionId: "ea" });
  state.tasks.push({ ...structuredClone(state.tasks[0]), id: "b", title: "Task B", executionId: "eb" });
  state.sessions = [emptySession("a"), emptySession("b")];
  const p = scriptedPort({ capabilities: { everything: true, operations: [] } });
  Object.assign(p.port, { scopesFacts: () => true, retryFirstView: () => undefined });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/a/workers/wa`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={p.port} />, root);
  await settle(); p.emit(projection("1", structuredClone(state))); await settle();
  const pane = () => root.querySelector<HTMLElement>(".livepane")!;
  // A's Work details waits for A's own facts.
  expect(pane().querySelector(".ws-skel")).not.toBeNull();
  p.emit(scopedFacts({ a: "ea" }, [workerOf("wa", "a", "ea", "frontend-dev")])); await settle();
  expect(pane().getAttribute("aria-label")).toBe("frontend-dev activity");
  expect(pane().textContent).toContain("Note of frontend-dev");
  // The owner opened B meanwhile, and a read scoped to B arrived: A, opened again, still has its worker.
  p.emit(scopedFacts({ b: "eb" }, [workerOf("wb", "b", "eb", "backend-dev")])); await settle();
  expect(pane().getAttribute("aria-label")).toBe("frontend-dev activity");
  expect(pane().textContent).toContain("Note of frontend-dev");
  expect(root.textContent).not.toContain("This worker is not part of the task.");
  // A starts again under a new execution: what was read for the old one does not make the new one ready.
  Object.assign(state.tasks[0], { executionId: "ea2" });
  p.emit(projection("2", structuredClone(state))); await settle();
  p.emit(scopedFacts({ b: "eb" }, [workerOf("wb", "b", "eb", "backend-dev")])); await settle();
  expect(pane().textContent).toContain("This worker's record has not arrived yet.");
  expect(pane().textContent).not.toContain("Note of frontend-dev");
  // Its own read for the new execution makes it ready again.
  p.emit(scopedFacts({ a: "ea2" }, [workerOf("wa", "a", "ea2", "frontend-dev")])); await settle();
  expect(pane().textContent).toContain("Note of frontend-dev");
});

/** A port that answers the coordinator's activity reads only when the test says so. */
function heldActivity() {
  const reads: { resolve: (page: unknown) => void; reject: (error: Error) => void }[] = [];
  const page = (text: string) => ({ activity: [{ sequence: 1, kind: "note", text, at: null }], activityCursor: null, messages: [] });
  const extend = (port: WorkspacePort) => Object.assign(port, { readWorkerActivity: () => new Promise((resolve, reject) => reads.push({ resolve, reject })) });
  return { reads, page, extend };
}
const coordinatorPath = `/m/${LOCAL_MACHINE_ID}/t/t/workers/cortex`;

it("gates the coordinator's own activity: the skeleton for as long as its read is slow, and a late reply only for its own execution", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true, toFake: ["setTimeout", "clearTimeout"] });
  const held = heldActivity();
  const { p, root, state } = await scripted(emptySession("t"), [], coordinatorPath, held.extend);
  const pane = () => root.querySelector<HTMLElement>(".livepane")!;
  const notice = () => pane().querySelector<HTMLElement>(".ws-first-notice");
  await vi.waitFor(() => expect(held.reads.length).toBeGreaterThan(0));
  expect(pane().querySelector(".ws-skel [role=status]")?.textContent).toBe("Loading activity…");
  vi.advanceTimersByTime(FORMER_FIRST_VIEW_MS + 100); await settle(4);
  // Slow is not failed: the skeleton stays and no notice comes.
  expect(pane().querySelector(".ws-skel")).not.toBeNull();
  expect(notice()).toBeNull();
  // An older service's deadline answers a slow read with a timeout: that is still not a failure.
  held.reads[0].reject(new Error("service_timeout")); await settle(4);
  expect(pane().querySelector(".ws-skel")).not.toBeNull();
  expect(notice()).toBeNull();
  // The task runs again under a new execution: the reply the old one waited for is not this feed's.
  const old = held.reads[held.reads.length - 1], asked = held.reads.length;
  Object.assign(state.tasks[0], { executionId: "e2" });
  p.emit(projection("2", structuredClone(state))); await settle();
  await vi.waitFor(() => expect(held.reads.length).toBeGreaterThan(asked));
  old.resolve(held.page("Old note")); await settle();
  expect(pane().textContent).not.toContain("Old note");
  held.reads.at(-1)!.resolve(held.page("New note"));
  await vi.waitFor(() => expect(pane().textContent).toContain("New note"));
  expect(notice()).toBeNull();
  // A refresh that fails keeps the feed shown, with the failure and Retry above it.
  // The next read is the running task's own periodic one.
  const shown = held.reads.length;
  vi.advanceTimersByTime(2600);
  await vi.waitFor(() => expect(held.reads.length).toBeGreaterThan(shown), { timeout: 4000 });
  held.reads.at(-1)!.reject(new Error("The machine is busy"));
  await vi.waitFor(() => expect(pane().querySelector(".ws-cf-err")?.textContent).toContain("The machine is busy"));
  expect(pane().textContent).toContain("New note");
  expect(pane().querySelector(".ws-skel")).toBeNull();
});

it("stops the coordinator's activity skeleton at once offline, with the notice and Retry", async () => {
  const held = heldActivity();
  const { p, root } = await scripted(emptySession("t"), [], coordinatorPath, held.extend);
  const pane = () => root.querySelector<HTMLElement>(".livepane")!;
  expect(pane().querySelector(".ws-skel")).not.toBeNull();
  p.emit({ kind: "connection", connection: { state: "disconnected", reason: null, at: null } });
  await settle(2);
  expect(pane().querySelector(".ws-skel")).toBeNull();
  expect(pane().querySelector(".ws-first-notice")?.textContent).toContain("Couldn’t finish loading activity.");
  // The reply that comes later is this feed's: it shows, and the notice goes.
  held.reads[0].resolve(held.page("Late note"));
  await vi.waitFor(() => expect(pane().textContent).toContain("Late note"));
  expect(pane().querySelector(".ws-first-notice")).toBeNull();
});

it("keeps naming what a view was shown without, with Retry, when it is opened again before those reads arrive", () => {
  vi.useFakeTimers();
  const conversation = (state: FirstRead["state"]): FirstRead[] => [{ section: "the conversation", state }];
  const [reads, setReads] = createSignal(conversation("pending"));
  const [offline, setOffline] = createSignal(true);
  const scope = {};
  const mount = (id: string) => createRoot(done => ({ view: createFirstView({ scope, identity: () => id, reads, offline }), done }));
  // Offline: shown at once without its transcript, then closed before the reply.
  let shown = mount("a");
  expect(shown.view.loading()).toBe(false);
  expect(shown.view.missing()).toEqual(["the conversation"]);
  shown.done();
  // Opened again, back online, still without it: no skeleton, and the notice stays.
  setOffline(false);
  shown = mount("a");
  expect(shown.view.loading()).toBe(false);
  expect(shown.view.missing()).toEqual(["the conversation"]);
  // The reply arrives: the notice goes, and a later refresh of the same view does not bring it back.
  setReads(conversation("settled"));
  expect(shown.view.missing()).toEqual([]);
  shown.done();
  setReads(conversation("pending"));
  shown = mount("a");
  expect(shown.view.loading()).toBe(false);
  expect(shown.view.missing()).toEqual([]);
  shown.done();
  // Online and only slow, it is never marked incomplete: closed and opened again before the reply, it is still loading.
  shown = mount("b");
  expect(shown.view.loading()).toBe(true);
  vi.advanceTimersByTime(FORMER_FIRST_VIEW_MS * 20);
  expect(shown.view.loading()).toBe(true);
  expect(shown.view.missing()).toEqual([]);
  shown.done();
  shown = mount("b");
  expect(shown.view.loading()).toBe(true);
  expect(shown.view.missing()).toEqual([]);
  setReads(conversation("settled"));
  expect(shown.view.loading()).toBe(false);
  expect(shown.view.missing()).toEqual([]);
  shown.done();
});

it("brings the question into view when Answer is pressed while the conversation is still loading, once it shows", async () => {
  const into: Element[] = [];
  const original = Element.prototype.scrollIntoView;
  Element.prototype.scrollIntoView = function (this: Element) { into.push(this); };
  try {
    const svc = machine(1, { open: true });
    const wait = hold();
    svc.holds.history = wait.done;
    const { root } = open(svc);
    await vi.waitFor(() => expect(feed()?.querySelector(".ws-skel")).not.toBeNull(), { timeout: 4000 });
    // The question is known from its own read, so the header offers Answer; its card is not on the page yet.
    const answer = () => [...root.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent === "Answer");
    await vi.waitFor(() => expect(answer()).toBeDefined(), { timeout: 4000 });
    expect(root.querySelector(".dc")).toBeNull();
    answer()!.click();
    await settle(4);
    expect(into).toEqual([]);
    wait.release();
    await vi.waitFor(() => expect(feed()?.textContent).toContain("Coordinator reply for task-1."), { timeout: 4000 });
    await vi.waitFor(() => {
      const choice = root.querySelector(".dc button[role=radio]");
      expect(choice?.closest(".dc")?.textContent).toContain("Which branch for exec-1?");
      expect(document.activeElement).toBe(choice);
      expect(into).toContain(choice);
    });
  } finally { Element.prototype.scrollIntoView = original; }
});

it("holds the skeleton for as long as a first read is pending, never on a timer, and names only a read that failed", () => {
  vi.useFakeTimers();
  const [reads, setReads] = createSignal<FirstRead[]>([{ section: "the conversation", state: "pending" }, { section: "questions", state: "pending" }]);
  const [id, setId] = createSignal("a");
  const { view, done } = createRoot(done => ({ view: createFirstView({ scope: {}, identity: id, reads, offline: () => false }), done }));
  expect(view.loading()).toBe(true);
  vi.advanceTimersByTime(FORMER_FIRST_VIEW_MS * 100);
  setReads([{ section: "the conversation", state: "pending" }, { section: "questions", state: "settled" }]);
  expect(view.loading()).toBe(true);
  expect(view.missing()).toEqual([]);
  // A read that genuinely failed is named, with what arrived shown.
  setReads([{ section: "the conversation", state: "settled" }, { section: "questions", state: "failed", reason: "decision_store_unreadable" }]);
  expect(view.loading()).toBe(false);
  expect(view.missing()).toEqual(["questions"]);
  setReads([{ section: "the conversation", state: "settled" }, { section: "questions", state: "settled" }]);
  expect(view.missing()).toEqual([]);
  // Another identity starts cold, and is waited for just the same.
  setReads([{ section: "the conversation", state: "pending" }]);
  setId("b");
  expect(view.loading()).toBe(true);
  vi.advanceTimersByTime(FORMER_FIRST_VIEW_MS * 100);
  expect(view.loading()).toBe(true);
  expect(view.missing()).toEqual([]);
  done();
});
