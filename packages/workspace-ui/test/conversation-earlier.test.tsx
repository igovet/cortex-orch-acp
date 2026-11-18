import { render } from "solid-js/web";
import { createRoot } from "solid-js";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Decision as WireDecision } from "@cortex/workspace-contract";
import { emptyConfig, LOCAL_MACHINE_ID, type NativeTaskDelta, type TimelineEntry } from "@cortex/workspace-domain";
import { Workspace } from "../src";
import { createRealPort } from "../src/port/real";
import { createWorkspaceStore } from "../src/store";
import { STEP_PREFIX } from "../src/tool-names";
import { conversationOf } from "../src/port/history";
import { capabilitiesWire, flush, scriptedTransport, wireDecision, wireTask } from "./support";

/**
 * A native service whose history reads behave like the core's: `history.page` pages forward and names a forward cursor;
 * `history.earlier` reads from the end backwards (`prevCursor` for the rows before, `tailCursor` on the newest page only).
 * `backward: false` is a service that does not know `history.earlier` and refuses it as the core's decoder does.
 */
function service(options: { backward?: boolean; earlierDelay?: number; decisions?: WireDecision[] } = {}) {
  let version = 1, high = 3, failNext = false;
  const task = () => wireTask({ version: String(version), description: "Index the repository", executionId: "execution", phase: "execution", executionState: "running", runtimeState: "EXECUTING" });
  const rows: Record<string, unknown>[] = [];
  const at = (n: number) => new Date(Date.UTC(2026, 10, 17, 10) + n * 1000).toISOString();
  const add = (kind: string, text: string, toolStatus: string | null = null) => rows.push({ historyId: `h${rows.length}`, sequence: String(rows.length + 1), revision: "1", kind, text, toolStatus,
    recordedAt: at(rows.length), availability: "available" });
  const envelope = (data: unknown) => ({ protocol: { major: 2, mapping: 2 }, projectId: "p", generation: "g", version: "2", highWatermark: String(high), data });
  const reads: { operation: string; cursor: string | null }[] = [];
  const forward = (p: { cursor: string | null; limit: number }) => {
    const last = rows.length - 1;
    let after = -1, top = last;
    if (p.cursor?.startsWith("nt1.")) after = Number(p.cursor.slice(4));
    else if (p.cursor) [top, after] = p.cursor.slice(3).split(".").map(Number);
    const items = rows.slice(after + 1, top + 1).slice(0, p.limit);
    const consumed = after + items.length, next = consumed < top ? `nh.${top}.${consumed}` : null;
    return { items, nextCursor: next, highWatermark: String(top + 1), tailCursor: `nt1.${next ? consumed : top}` };
  };
  const earlier = (p: { cursor: string | null; limit: number }) => {
    const before = p.cursor ? Number(p.cursor.slice(4)) : rows.length, from = Math.max(0, before - p.limit);
    return { items: rows.slice(from, before), nextCursor: null, highWatermark: String(rows.length), ...(from > 0 ? { prevCursor: `np1.${from}` } : {}), ...(p.cursor ? {} : { tailCursor: `nt1.${rows.length - 1}` }) };
  };
  const s = scriptedTransport({
    "service.capabilities": () => ({ methods: ["native.query", "native.board"], machine: true }),
    "projects.list": () => [{ projectId: "p", displayName: "Project", canonicalRoot: "/p", activation: {} }],
    "capabilities.get": () => capabilitiesWire(["capabilities.get", "project.snapshot", "tasks.create", "tasks.start", "decisions.list", "decisions.answer", "history.page"]),
    "native.board": () => envelope({ type: "board", value: { items: [task()], nextCursor: null, highWatermark: String(high) } }),
    "native.query": async raw => {
      const { operation, payload } = raw as { operation: string; payload: { cursor: string | null; limit: number } };
      reads.push({ operation, cursor: payload.cursor });
      if (operation === "history.page") return envelope({ type: "history", value: forward(payload) });
      if (options.backward === false) throw new Error("Refused: invalid_request");
      if (payload.cursor && options.earlierDelay) await new Promise(resolve => setTimeout(resolve, options.earlierDelay));
      if (payload.cursor && failNext) { failNext = false; throw new Error("Refused: storage_unavailable"); }
      return envelope({ type: "history", value: earlier(payload) });
    },
    "decisions.list": () => ({ items: options.decisions ?? [], nextCursor: null, highWatermark: "1" }),
    "machine.call": raw => {
      const op = (raw as { op: string }).op;
      if (op === "overview") return { ok: true, result: { name: "This machine" } };
      if (op === "project.config.read") return { ok: true, result: { value: {}, tokens: {} } };
      if (op === "config.read") return { ok: true, result: { config: emptyConfig(), connections: [], tokens: {} } };
      return { ok: true, result: op === "repo.snapshot" ? null : op === "workers" ? [] : { tasks: {}, sprints: [] } };
    },
  });
  /** A row was written: the task's version moves, as the service's stream says. */
  const changed = () => {
    const base = version, from = high + 1;
    version++; high++;
    const delta: NativeTaskDelta = { projectId: "p", batch: { generation: "g", streamKey: { scope: "project", project_id: "p" }, fromSequence: String(from), throughSequence: String(high),
      changes: [{ kind: "task", id: "task-1", baseVersion: String(base), newVersion: String(version), operation: { type: "upsert", data: { kind: "task_projection", data: task() } } }] } };
    s.send({ type: "native_delta", ...delta });
  };
  return { s, rows, add, at, changed, reads, fail: () => { failNext = true; } };
}
const step = (call: string, status: string) => STEP_PREFIX + JSON.stringify({ title: "Read a.ts", callId: call, kind: "read", path: `a${call}.ts`, status });
/** `n` rows of a long task: an owner message every 50 rows, between them the coordinator's message and a tool call in turn. */
function long(svc: ReturnType<typeof service>, n: number) {
  for (let i = 0; i < n; i++) {
    if (i % 50 === 0) svc.add("user", `Message ${i}`);
    else if (i % 2 === 1) svc.add("assistant", `Reply ${i}.`);
    else svc.add("tool", step(`c${i}`, "completed"), "completed");
  }
}

let dispose: (() => void) | undefined;
beforeEach(() => { localStorage.clear(); sessionStorage.clear(); });
afterEach(() => { dispose?.(); dispose = undefined; document.body.innerHTML = ""; vi.unstubAllGlobals(); vi.restoreAllMocks(); });
function open(s: ReturnType<typeof service>["s"]) {
  const store = createRoot(d => { dispose = d; return createWorkspaceStore(createRealPort({ transport: s.transport, actorId: "owner", registry: true })); });
  const session = () => store.state().sessions.find(x => x.taskId === "task-1");
  const timeline = () => session()?.timeline ?? [];
  const texts = () => timeline().flatMap((e: TimelineEntry) => e.kind === "cortex" || e.kind === "said" ? [e.text] : []);
  return { store, session, timeline, texts };
}

it("opens a 22,000-row conversation on its latest part in a bounded number of reads, and reads only new rows after it", async () => {
  const svc = service();
  long(svc, 22_000);
  const { session, texts } = open(svc.s);
  await vi.waitFor(() => expect(texts().at(-1)).toBe("Reply 21999."), { timeout: 4000 });
  // At most four pages, every one of them from the end backwards: never the 220 pages of a read from the beginning.
  expect(svc.reads.length).toBeLessThanOrEqual(4);
  expect(svc.reads.every(r => r.operation === "history.earlier")).toBe(true);
  expect(session()!.historyEarlier).toEqual({ state: "available", since: Date.parse(svc.at(22_000 - svc.reads.length * 100)) });
  // A new row arrives through the forward cursor and is appended, nothing read again.
  const before = svc.reads.length;
  svc.add("tool", step("late", "completed"), "completed"); svc.add("assistant", "Latest words."); svc.changed();
  await vi.waitFor(() => expect(texts().at(-1)).toBe("Latest words."));
  expect(svc.reads.slice(before).every(r => r.operation === "history.page" && r.cursor?.startsWith("nt1."))).toBe(true);
  expect(session()!.historyEarlier?.state).toBe("available");
});

it("puts each earlier part before the rows held, keeps every entry already shown, and stops at the beginning", async () => {
  const svc = service();
  long(svc, 700);
  const { store, session, timeline, texts } = open(svc.s);
  await vi.waitFor(() => expect(session()?.historyEarlier?.state).toBe("available"), { timeout: 4000 });
  const shown = timeline().map(e => e.id);
  store.loadEarlierHistory("task-1");
  await vi.waitFor(() => expect(timeline().length).toBeGreaterThan(shown.length));
  // What was shown stays, in order and under the same identities, after what was read before it.
  const now = timeline().map(e => e.id);
  expect(now.slice(now.length - shown.length)).toEqual(shown);
  // New rows still append while earlier ones are read.
  svc.add("tool", step("late", "completed"), "completed"); svc.add("assistant", "Streaming on."); svc.changed();
  await vi.waitFor(() => expect(texts().at(-1)).toBe("Streaming on."));
  for (let i = 0; i < 6 && session()?.historyEarlier; i++) {
    const n = timeline().length;
    store.loadEarlierHistory("task-1");
    await vi.waitFor(() => expect(timeline().length > n || !session()?.historyEarlier).toBe(true));
  }
  // The beginning is reached: the conversation is no longer partial, and reads exactly as a whole read of it does.
  expect(session()!.historyEarlier).toBeUndefined();
  const whole = service({ backward: false });
  long(whole, 700); whole.add("tool", step("late", "completed"), "completed"); whole.add("assistant", "Streaming on.");
  const reference = open(whole.s);
  await vi.waitFor(() => expect(reference.texts().at(-1)).toBe("Streaming on."), { timeout: 4000 });
  expect(texts()).toEqual(reference.texts());
});

it("never shows a message whose beginning lies in the part not read, nor a settled question or answer from it", async () => {
  // 250 rows: the newest two pages (rows 50 to 249) begin in the middle of a streamed message (rows 45 to 99). The owner
  // answered "main" at row 10, before them, and later wrote "main" again as a message of their own (row 180).
  const time = (n: number) => new Date(Date.UTC(2026, 10, 17, 10) + n * 1000).toISOString();
  const answered = wireDecision({ decisionId: "old", executionId: "execution", prompt: "Which branch?", status: "answered", answerText: "main",
    createdAt: time(5), answeredAt: time(10), allowedOperations: [] } as Partial<WireDecision>);
  const svc = service({ decisions: [answered] });
  for (let i = 0; i < 10; i++) svc.add("assistant", `early-${i} `);
  svc.add("user", "main");
  for (let i = 11; i < 45; i++) svc.add("tool", step(`s${i}`, "completed"), "completed");
  for (let i = 45; i < 100; i++) svc.add("assistant", `cut-${i} `);
  for (let i = 100; i < 120; i++) svc.add("tool", step(`t${i}`, "completed"), "completed");
  for (let i = 120; i < 180; i++) svc.add("assistant", `whole-${i} `);
  svc.add("user", "main");
  for (let i = 181; i < 250; i++) svc.add("tool", step(`u${i}`, "completed"), "completed");
  const { store, session, timeline } = open(svc.s);
  await vi.waitFor(() => expect(session()?.historyEarlier?.state).toBe("available"), { timeout: 4000 });
  expect(session()!.historyEarlier!.since).toBe(Date.parse(time(50)));
  // The message cut at its start is left out; the turn of steps after it and the next message are whole.
  expect(timeline().map(e => e.kind)).toEqual(["fold", "cortex", "said", "fold"]);
  expect((timeline()[1] as { text: string }).text.startsWith("whole-120")).toBe(true);
  // The answer given before the part read does not hide the later message of the same words; its settled question is not
  // put at the top of these rows.
  expect((timeline()[2] as { text: string }).text).toBe("main");
  expect(timeline().some(e => e.kind === "decision")).toBe(false);
  // Read to the beginning, the message reads whole, the answer reads with its question and the question is in its place.
  for (let i = 0; i < 3 && session()?.historyEarlier; i++) {
    const n = timeline().length;
    store.loadEarlierHistory("task-1");
    await vi.waitFor(() => expect(timeline().length > n || !session()?.historyEarlier).toBe(true));
  }
  expect(session()!.historyEarlier).toBeUndefined();
  expect(timeline().map(e => e.kind)).toEqual(["cortex", "decision", "fold", "cortex", "fold", "cortex", "said", "fold"]);
  expect((timeline()[3] as { text: string }).text.startsWith("cut-45")).toBe(true);
});

it("leaves out a fold whose first steps lie in the part not read, and only when the part before it is not read", () => {
  const row = (n: number, kind: string, text: string, toolStatus: string | null = null) => ({ historyId: `r${n}`, revision: "1", kind, text, toolStatus, recordedAt: new Date(Date.UTC(2026, 10, 17) + n * 1000).toISOString() });
  const rows = [row(0, "tool", step("a", "completed"), "completed"), row(1, "tool", step("b", "completed"), "completed"), row(2, "assistant", "After."), row(3, "user", "Thanks")];
  expect(conversationOf(rows, { description: "", answers: [] }).map(d => d.entry.kind)).toEqual(["fold", "cortex", "said"]);
  expect(conversationOf(rows, { description: "", answers: [], partial: true }).map(d => d.entry.kind)).toEqual(["cortex", "said"]);
  // A part that begins with the owner's message has nothing cut at its start.
  expect(conversationOf(rows.slice(3), { description: "", answers: [], partial: true }).map(d => d.entry.kind)).toEqual(["said"]);
});

it("reads a short conversation in one read and shows it exactly as a read from the beginning does", async () => {
  const fill = (svc: ReturnType<typeof service>) => {
    svc.add("user", "Index the repository");
    svc.add("assistant", "Looking"); svc.add("assistant", " around.");
    svc.add("tool", step("a", "pending"), "pending"); svc.add("tool", step("a", "completed"), "completed");
    svc.add("assistant", "Done.");
  };
  const back = service(), whole = service({ backward: false });
  fill(back); fill(whole);
  const a = open(back.s);
  await vi.waitFor(() => expect(a.texts()).toEqual(["Looking around.", "Done."]), { timeout: 4000 });
  const shown = a.timeline();
  expect(a.session()!.historyEarlier).toBeUndefined();
  dispose?.();
  const b = open(whole.s);
  await vi.waitFor(() => expect(b.texts()).toEqual(["Looking around.", "Done."]), { timeout: 4000 });
  expect(back.reads).toEqual([{ operation: "history.earlier", cursor: null }]);
  expect(b.timeline()).toEqual(shown);
  expect(b.session()!.historyEarlier).toBeUndefined();
  // The service without backward reads was asked once and then read forward from the beginning, as before.
  expect(whole.reads.map(r => r.operation)).toEqual(["history.earlier", "history.page"]);
});

/** The task page with a laid-out feed: every row of the feed is 50 px high, the feed's box 400 px, scrolled to `top`. */
async function openPage(svc: ReturnType<typeof service>) {
  const observers: (() => void)[] = [];
  vi.stubGlobal("ResizeObserver", class { constructor(cb: () => void) { observers.push(cb); } observe() {} disconnect() {} });
  history.replaceState(null, "", `/m/${LOCAL_MACHINE_ID}/t/task-1`);
  const root = document.createElement("div"); document.body.append(root);
  dispose = render(() => <Workspace port={createRealPort({ transport: svc.s.transport, actorId: "owner", registry: true })} />, root);
  await vi.waitFor(() => expect(root.textContent).toContain("Earlier conversation is not shown."), { timeout: 4000 });
  for (let i = 0; i < 12; i++) await flush();
  const box = root.querySelector<HTMLElement>(".task-panel .timeline")!, inner = box.firstElementChild!;
  let top = 0;
  const height = () => inner.children.length * 50;
  Object.defineProperties(box, { clientHeight: { get: () => 400 }, scrollHeight: { get: () => height() }, scrollTop: { get: () => top, set: (v: number) => { top = Math.max(0, Math.min(v, height() - 400)); } } });
  const rect = Element.prototype.getBoundingClientRect;
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
    if (this === box) return { top: 0, bottom: 400, height: 400 } as DOMRect;
    if (this.parentElement === inner) { const y = [...inner.children].indexOf(this) * 50 - top; return { top: y, bottom: y + 50, height: 50 } as DOMRect; }
    return rect.call(this);
  });
  const resize = () => { for (const cb of observers) cb(); };
  return { root, box, inner, resize, scrollTo: (v: number) => { top = v; box.dispatchEvent(new Event("scroll")); }, top: () => top };
}

it("scrolling near the top reads the earlier part with a Loading line, and the row the reader looks at stays where it was", async () => {
  const svc = service({ earlierDelay: 30 });
  long(svc, 3000);
  const { root, box, inner, resize, scrollTo, top } = await openPage(svc);
  // The reader scrolls up to near the top.
  box.dispatchEvent(new WheelEvent("wheel", { deltaY: -100 }));
  scrollTo(120);
  const anchor = [...inner.children].find(row => !row.hasAttribute("data-follow-skip") && row.getBoundingClientRect().bottom > 0)!;
  const y = anchor.getBoundingClientRect().top, rows = inner.children.length;
  await vi.waitFor(() => expect(root.textContent).toContain("Loading earlier…"));
  await vi.waitFor(() => expect(inner.children.length).toBeGreaterThan(rows), { timeout: 4000 });
  resize();
  // The same node, at the same place on screen: the scroll position moved by exactly what was put above it.
  expect(anchor.isConnected).toBe(true);
  expect(anchor.getBoundingClientRect().top).toBe(y);
  expect(top() - 120).toBe((inner.children.length - rows) * 50);
  expect(root.textContent).toContain("Earlier conversation is not shown.");
  expect(root.textContent).not.toContain("Loading earlier…");
  // Nothing new was counted as missed by the reader who scrolled up.
  expect(root.textContent).not.toMatch(/\d+ new/);
});

it("Show earlier reads the earlier part; a failed read says so with Retry, and the beginning shows no such line", async () => {
  const svc = service();
  long(svc, 420);
  const { root, inner, resize } = await openPage(svc);
  const button = (name: string) => [...root.querySelectorAll<HTMLButtonElement>(".ws-earlier button")].find(b => b.textContent === name);
  svc.fail();
  button("Show earlier")!.click();
  await vi.waitFor(() => expect(root.textContent).toContain("Earlier conversation could not be read: "));
  expect(root.querySelector(".ws-earlier [role=alert]")).not.toBeNull();
  const first = inner.querySelector(".ai")!, rows = inner.children.length;
  button("Retry")!.click();
  await vi.waitFor(() => expect(inner.children.length).toBeGreaterThan(rows), { timeout: 4000 });
  resize();
  expect(first.isConnected).toBe(true);
  for (let i = 0; i < 4 && button("Show earlier"); i++) {
    const n = inner.children.length;
    button("Show earlier")!.click();
    await vi.waitFor(() => expect(inner.children.length).not.toBe(n), { timeout: 4000 });
  }
  await vi.waitFor(() => expect(root.querySelector(".ws-earlier")).toBeNull());
  expect(root.textContent).toContain("Message 0");
  expect(root.querySelector(".brief")!.hasAttribute("data-follow-skip")).toBe(false);
});
