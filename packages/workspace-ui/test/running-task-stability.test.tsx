import { render } from "solid-js/web";
import { createEffect, createRoot, createSignal } from "solid-js";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Decision as WireDecision } from "@cortex/workspace-contract";
import { emptyConfig, LOCAL_MACHINE_ID, type NativeTaskDelta, type TimelineEntry, type Worker } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { Keyed } from "../src/kit";
import { createRealPort } from "../src/port/real";
import { createWorkspaceStore } from "../src/store";
import { capabilitiesWire, flush, scriptedTransport, wireDecision, wireTask } from "./support";

/**
 * A running task on a native service, as the desktop sees it: the board read, the conversation read on its own, and a stream
 * of events while the task works. Each step of work adds rows to the conversation and moves the task's version.
 */
function runningService(more: { question?: boolean; workers?: Worker[]; runtime?: { state: string } } = {}) {
  let version = 1, high = 3;
  const task = () => wireTask({ version: String(version), description: "Fix the login race", executionId: "execution", phase: "execution", executionState: "running", runtimeState: more.runtime?.state ?? "EXECUTING" });
  const rows: Record<string, unknown>[] = [];
  const row = (kind: string, text: string, toolStatus: string | null = null) => rows.push({ historyId: `h${rows.length}`, sequence: String(rows.length + 1), revision: "1", kind, text, toolStatus,
    recordedAt: `2026-11-17T10:00:${String(rows.length).padStart(2, "0")}.000000+00:00`, availability: "available" });
  row("assistant", "Reading the login flow first.");
  row("tool", "Read src/login.ts", "in_progress");
  row("assistant", "The race is in the token refresh.");
  const envelope = (data: unknown) => ({ protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "g", version: "2", highWatermark: String(high), data });
  const question = wireDecision({ decisionId: "ask", executionId: "execution", prompt: "Which token store?", options: [{ optionId: "mem", label: "Memory" }, { optionId: "disk", label: "Disk" }],
    createdAt: "2026-11-17T10:00:02.500000+00:00" } as Partial<WireDecision>);
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["native.query", "native.board"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Project", canonicalRoot: "/p", activation: {} }],
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer", "history.page"]),
    "native.board": () => envelope({ type: "board", value: { items: [task()], nextCursor: null, highWatermark: String(high) } }),
    // A conversation read takes a moment on a real machine, so the board's own update reaches the screens first.
    "native.query": () => { const reply = envelope({ type: "history", value: { items: [...rows], nextCursor: null, highWatermark: String(rows.length) } }); return new Promise(resolve => setTimeout(() => resolve(reply), 20)); },
    "decisions.list": () => ({ items: more.question ? [question] : [], nextCursor: null, highWatermark: "1" }),
    "machine.call": raw => {
      const op = (raw as { op: string }).op;
      if (op === "overview") return { ok: true, result: { name: "This machine" } };
      if (op === "project.config.read") return { ok: true, result: { value: {}, tokens: {} } };
      if (op === "config.read") return { ok: true, result: { config: emptyConfig(), connections: [], tokens: {} } };
      return { ok: true, result: op === "repo.snapshot" ? null : op === "workers" ? [...(more.workers ?? [])] : { tasks: {}, sprints: [] } };
    },
  });
  /** One step of work: a tool call and what the coordinator said, then the event that says so (a native delta, or a refresh). */
  const work = (text: string, via: "delta" | "refresh" = "delta") => {
    row("tool", `Edit ${text}`, "in_progress");
    row("assistant", text);
    const base = version, from = high + 1;
    version++; high++;
    const delta: NativeTaskDelta = { projectId: "p", batch: { generation: "g", streamKey: { scope: "project", project_id: "p" }, fromSequence: String(from), throughSequence: String(high),
      changes: [{ kind: "task", id: "task-1", baseVersion: String(base), newVersion: String(version), operation: { type: "upsert", data: { kind: "task_projection", data: task() } } }] } };
    s.send(via === "delta" ? { type: "native_delta", ...delta } : { type: "refresh" });
    return String(high);
  };
  return { s, work };
}

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; });
const settle = async () => { for (let i = 0; i < 12; i++) await flush(); };
/** The task page, with the screens as the port's only subscriber, as in the app. */
async function openTask(s: ReturnType<typeof runningService>["s"]) {
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/task-1`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={createRealPort({ transport: s.transport, actorId: "owner", registry: true })} />, root);
  await vi.waitFor(() => expect(root.querySelectorAll(".timeline .ai")).toHaveLength(2), { timeout: 4000 });
  await settle();
  return root;
}

it("a running task's live updates never publish its conversation without the rows already read", async () => {
  const { s, work } = runningService();
  let stop!: () => void;
  const store = createRoot(d => { stop = d; return createWorkspaceStore(createRealPort({ transport: s.transport, actorId: "owner", registry: true })); });
  const said = () => (store.state().sessions.find(x => x.taskId === "task-1")?.timeline ?? []).flatMap((e: TimelineEntry) => e.kind === "cortex" ? [e.text] : []);
  // How many of the coordinator's messages every published state held, in order.
  const seen: number[] = [];
  try {
    await vi.waitFor(() => expect(said()).toEqual(["Reading the login flow first.", "The race is in the token refresh."]));
    createRoot(() => createEffect(() => seen.push(said().length)));
    for (let i = 0; i < 6; i++) {
      // Most steps arrive as native deltas; a refresh rereads the whole board.
      const at = work(`Step ${i}`, i === 3 ? "refresh" : "delta");
      await vi.waitFor(() => expect(store.version("project:p")).toBe(at));
      await vi.waitFor(() => expect(said().at(-1)).toBe(`Step ${i}`));
    }
    // A message read once stays until the next read replaces it: the count only ever grows.
    expect(seen.every((n, i) => n >= 2 && (i === 0 || n >= seen[i - 1]))).toBe(true);
    expect(seen.at(-1)).toBe(8);
  } finally { stop(); }
});

it("a running task's page keeps every row it already shows while updates stream in", async () => {
  const { s, work } = runningService();
  const root = await openTask(s);
  const timeline = root.querySelector(".timeline .tl-in")!;
  const shown = [...timeline.children], first = timeline.querySelector(".ai")!;
  // How often a live update took away a row the owner already saw, and how often the conversation thinned out.
  let removed = 0, thinned = 0;
  const observer = new MutationObserver(records => {
    for (const r of records) for (const node of r.removedNodes) if (shown.includes(node as Element)) removed++;
    if (timeline.querySelectorAll(".ai").length < 2) thinned++;
  });
  observer.observe(timeline, { childList: true, subtree: true });
  for (let i = 0; i < 5; i++) {
    work(`Step ${i}`);
    await vi.waitFor(() => expect(timeline.textContent).toContain(`Step ${i}`), { timeout: 4000 });
    await settle();
  }
  observer.disconnect();
  expect({ removed, thinned }).toEqual({ removed: 0, thinned: 0 });
  expect(shown.every(row => row.isConnected && row.parentElement === timeline)).toBe(true);
  expect(timeline.querySelector(".ai")).toBe(first);
  expect(timeline.querySelectorAll(".ai")).toHaveLength(7);
});

it("a running task's page with a question, its workers and its waiting line removes no element while updates stream in", async () => {
  const finished = { id: "w1", taskId: "task-1", executionId: "execution", role: "frontend-dev", status: "completed", lane: "execution", goal: "Patch the refresh", now: null, waitsFor: null, scope: [],
    route: { model: "opus", effort: "high" }, attempt: { n: 1, of: 1 }, elapsedMs: 4000, runningSince: null, context: null, usage: null, activity: [], result: { summary: "Patched", checks: [] } } as unknown as Worker;
  const { s, work } = runningService({ question: true, workers: [finished] });
  const root = await openTask(s);
  await vi.waitFor(() => expect(root.querySelector('.timeline .dc [aria-label="Send answer"]')).not.toBeNull(), { timeout: 4000 });
  await settle();
  const page = root.querySelector(".l-task")!;
  expect(page.textContent).toContain("Workers: 1 finished");
  // The question is open and no worker runs: the last line says so, as the header does, never "Working…".
  expect(page.querySelector(".timeline .ws-waiting")?.textContent).toBe("Waiting for you");
  expect(page.textContent).not.toContain("Working…");
  const card = page.querySelector(".timeline .dc")!, plane = card.querySelector('[aria-label="Send answer"]')!, answer = card.querySelector("textarea")!;
  // Every element taken off the page, counted with all it held.
  let removed = 0;
  const observer = new MutationObserver(records => { for (const r of records) for (const node of r.removedNodes) if (node.nodeType === 1) removed += 1 + (node as Element).querySelectorAll("*").length; });
  observer.observe(page, { childList: true, subtree: true });
  for (let i = 0; i < 5; i++) {
    work(`Step ${i}`);
    await vi.waitFor(() => expect(page.textContent).toContain(`Step ${i}`), { timeout: 4000 });
    await settle();
  }
  // A second's tick moves the waiting and elapsed figures, as text in place.
  await new Promise(resolve => setTimeout(resolve, 1100)); await settle();
  observer.disconnect();
  expect(removed).toBe(0);
  expect(card.isConnected && plane.isConnected && answer.isConnected).toBe(true);
  // The answer goes into the running session: its form offers no model or effort.
  expect(card.querySelector(".c-route")).toBeNull();
});

it("a keyed row hears only a change of its own item, not every new read of the list", () => {
  const a = { id: "a", text: "A" }, b = { id: "b", text: "B" };
  let built = 0;
  const host = document.createElement("div");
  const [list, setList] = createSignal([a, b]);
  const stop = render(() => <Keyed each={list()} by={x => x.id}>{item => <span>{(built++, item().text)}</span>}</Keyed>, host);
  try {
    const shown = [...host.querySelectorAll("span")];
    expect(built).toBe(2);
    // The same records in a new list, as every read of the machine gives: nothing is built again.
    setList([a, b]); setList([...list()]);
    expect(built).toBe(2);
    expect([...host.querySelectorAll("span")]).toEqual(shown);
    // A record that really changed is the only one that hears it.
    setList([a, { id: "b", text: "B2" }]);
    expect(built).toBe(3);
    expect(host.querySelectorAll("span")[0]).toBe(shown[0]);
    expect(host.textContent).toBe("AB2");
  } finally { stop(); }
});

it("a streaming task on screen rereads its facts at most once per interval, never its Git without a Git screen, while its conversation keeps up", async () => {
  const { s, work } = runningService();
  let stop!: () => void;
  const store = createRoot(d => { stop = d; return createWorkspaceStore(createRealPort({ transport: s.transport, actorId: "owner", registry: true, streamFactsMs: 1500 })); });
  // The task page is on view: it holds interest in its project's facts, as the screens do.
  store.interest("p", "facts", "task-1");
  const said = () => (store.state().sessions.find(x => x.taskId === "task-1")?.timeline ?? []).flatMap((e: TimelineEntry) => e.kind === "cortex" ? [e.text] : []);
  const reads = (op: string) => s.calls.filter(c => c.operation === "machine.call" && (c.message as { op: string }).op === op).length;
  try {
    await vi.waitFor(() => expect(said()).toHaveLength(2));
    // The opening reads (the board arriving moves the task's state) settle first: the burst starts a full interval after the last.
    for (let n = -1; n !== reads("product.facts");) { n = reads("product.facts"); await new Promise(resolve => setTimeout(resolve, 1600)); }
    const facts = reads("product.facts"), repo = reads("repo.snapshot");
    const started = Date.now();
    for (let i = 0; i < 8; i++) {
      const at = work(`Step ${i}`);
      // Every step's conversation still arrives promptly.
      await vi.waitFor(() => expect(store.version("project:p")).toBe(at));
      await vi.waitFor(() => expect(said().at(-1)).toBe(`Step ${i}`));
    }
    // One leading read, then at most one per interval of the burst (eight steps would otherwise read eight times).
    const bound = 1 + Math.ceil((Date.now() - started) / 1500);
    expect(reads("product.facts") - facts).toBeLessThanOrEqual(bound);
    // Without a Git screen on view the repository read on opening is all there is: streamed work reads no Git.
    expect(reads("repo.snapshot") - repo).toBe(0);
    expect(repo).toBeLessThanOrEqual(1);
    // One trailing read follows the interval, then nothing while nothing streams.
    await new Promise(resolve => setTimeout(resolve, 1800));
    const settled = reads("product.facts");
    expect(settled - facts).toBeGreaterThanOrEqual(2);
    expect(settled - facts).toBeLessThanOrEqual(bound + 1);
    await new Promise(resolve => setTimeout(resolve, 1800));
    expect(reads("product.facts")).toBe(settled);
  } finally { stop(); }
}, 15_000);

it("a task whose state moves reads its workers at once, not a facts interval later", async () => {
  // Workers finish and the task moves on (the runtime state changes) well inside the facts interval: the workers must arrive
  // with that state, or the task bar's Work details count and the team bar change seconds after the screen settled.
  const workers: Worker[] = [], runtime = { state: "EXECUTING" };
  const { s, work } = runningService({ workers, runtime });
  let stop!: () => void;
  const store = createRoot(d => { stop = d; return createWorkspaceStore(createRealPort({ transport: s.transport, actorId: "owner", registry: true, streamFactsMs: 60_000 })); });
  // The task page is on view.
  store.interest("p", "facts", "task-1");
  const said = () => (store.state().sessions.find(x => x.taskId === "task-1")?.timeline ?? []).flatMap((e: TimelineEntry) => e.kind === "cortex" ? [e.text] : []);
  const reads = () => s.calls.filter(c => c.operation === "machine.call" && (c.message as { op: string }).op === "product.facts").length;
  try {
    await vi.waitFor(() => expect(said()).toHaveLength(2));
    await vi.waitFor(() => expect(reads()).toBeGreaterThan(0));
    await new Promise(resolve => setTimeout(resolve, 200));
    const before = reads();
    // Streamed text alone stays throttled.
    work("Step 0");
    await vi.waitFor(() => expect(said().at(-1)).toBe("Step 0"));
    await new Promise(resolve => setTimeout(resolve, 200));
    expect(reads()).toBe(before);
    expect(store.state().workers).toHaveLength(0);
    workers.push({ id: "w1", taskId: "task-1", executionId: "execution", role: "backend-dev", status: "completed", lane: "execution", goal: "Patch", now: null, waitsFor: null, scope: [],
      route: null, attempt: { n: 1, of: 1 }, elapsedMs: 4000, runningSince: null, context: null, usage: null, activity: [], result: { summary: "Patched", checks: [] } } as unknown as Worker);
    runtime.state = "RECONCILING";
    work("Step 1");
    await vi.waitFor(() => expect(store.state().workers.map(w => w.id)).toEqual(["w1"]), { timeout: 2000 });
  } finally { stop(); }
}, 15_000);
